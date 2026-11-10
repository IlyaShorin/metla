//! Candidate queries of the index: tallies, pages, preselection and
//! exclusions applied when reading.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use metla_core::domain::{
    Bytes, Candidate, Category, DirId, DirRecord, Disposal, Exclusion, FileKind, KindTotals,
    Recency, RuleId, ScanSummary, Tally, TreeTotals, UnixTime, Verdict,
};
use metla_core::index::{IndexError, IndexWriter, Page, PictureId, Snapshot};

use crate::index::{read, reader, summary, write, write_summary};
use crate::support::Fixture;

const ROOT: &str = "/synthetic/root";
const DAY: i64 = 24 * 3600;
const NOW: UnixTime = UnixTime(10_000 * DAY);

fn at(days_ago: i64) -> Option<UnixTime> {
    Some(UnixTime(NOW.0 - days_ago * DAY))
}

fn recency() -> Recency {
    Recency::new(NOW, Recency::DEFAULT_AGE)
}

fn rule(id: &str) -> Option<RuleId> {
    RuleId::parse(id)
}

fn dir(
    id: i64,
    parent: i64,
    name: &str,
    bytes: u64,
    newest: Option<UnixTime>,
    rule_id: &str,
) -> DirRecord {
    let tree = TreeTotals {
        bytes: Bytes(bytes),
        files: bytes / 10,
        dirs: 1,
        newest,
    };
    DirRecord {
        rule: rule(rule_id),
        ..read(id, Some(parent), name, 0, tree)
    }
}

/// root ── a ── node_modules (300, active yesterday), target (500, a month old)
///      ├── Caches ── com.x (200), com.y (100)
///      ├── wt (50, a worktree)
///      └── ab (40, logs; a sibling whose name starts like `a`)
fn picture() -> Vec<DirRecord> {
    let plain = |id, parent, name| dir(id, parent, name, 0, at(400), "");
    vec![
        dir(3, 2, "node_modules", 300, at(1), "build.node-modules"),
        dir(4, 2, "target", 500, at(30), "build.cargo-target"),
        plain(2, 1, "a"),
        dir(6, 5, "com.x", 200, at(3), "app-cache.library"),
        dir(7, 5, "com.y", 100, at(3), "app-cache.library"),
        plain(5, 1, "Caches"),
        dir(8, 1, "wt", 50, at(90), "worktree.linked"),
        dir(9, 1, "ab", 40, at(2), "log.user-library"),
        read(1, None, "", 0, TreeTotals::directory(UnixTime(0))),
    ]
}

pub fn picture_id(snap: &Snapshot<'_>) -> Result<PictureId, IndexError> {
    let picture = snap.scan()?.and_then(|scan| scan.picture);
    Ok(picture.expect("a picture").id())
}

/// Stores Git's verdict on every project output not asked about yet.
pub fn verify(index: &Path, verdict: impl Fn(&Path) -> Verdict) -> bool {
    let (picture, unverified) = reader(index)
        .read(|snap| Ok((picture_id(snap)?, snap.unverified()?)))
        .unwrap();
    let verdicts = unverified
        .iter()
        .map(|(id, path)| (*id, verdict(path)))
        .collect();
    let writer = IndexWriter::open(index).unwrap();
    writer.set_verdicts(picture, verdicts).unwrap()
}

/// Writes a picture whose project output Git has verified.
pub fn write_verified(index: &Path, rows: Vec<DirRecord>) {
    write(index, rows, true);
    verify(index, |_| Verdict::Verified);
}

fn indexed() -> (Fixture, PathBuf) {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write_verified(&index, picture());
    (fx, index)
}

const ALL: Page = Page {
    offset: 0,
    limit: 100,
};

fn names(index: &Path, category: Option<Category>, page: Page) -> Vec<String> {
    candidates(index, category, page)
        .iter()
        .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn candidates(index: &Path, category: Option<Category>, page: Page) -> Vec<Candidate> {
    reader(index)
        .read(|snap| snap.candidates(category, page, &recency()))
        .unwrap()
}

fn exclude(index: &Path, path: &str) -> bool {
    let writer = IndexWriter::open(index).unwrap();
    writer
        .exclude(&Exclusion::new(Path::new(path)).unwrap())
        .unwrap()
}

fn include(index: &Path, path: &str) -> bool {
    let writer = IndexWriter::open(index).unwrap();
    writer
        .include(&Exclusion::new(Path::new(path)).unwrap())
        .unwrap()
}

#[test]
fn candidates_are_listed_largest_first_with_everything_a_cleanup_needs() {
    let (_fx, index) = indexed();
    assert_eq!(
        names(&index, None, ALL),
        ["target", "node_modules", "com.x", "com.y", "wt", "ab"]
    );
    let target = &candidates(&index, None, ALL)[0];
    assert_eq!(
        *target,
        Candidate {
            id: DirId(4),
            path: PathBuf::from(ROOT).join("a/target"),
            rule: RuleId::new(Category::BuildArtifact, "cargo-target"),
            bytes: Bytes(500),
            files: 50,
            activity: at(30),
            device: 7,
            inode: 1004,
            managed: None,
            preselected: true,
        }
    );
    assert_eq!(target.category(), Category::BuildArtifact);
    assert_eq!(target.disposal(), Disposal::DeletePermanently);
}

#[test]
fn candidates_are_listed_by_category_and_page() {
    let (_fx, index) = indexed();
    let build = Some(Category::BuildArtifact);
    assert_eq!(names(&index, build, ALL), ["target", "node_modules"]);
    assert_eq!(
        names(&index, Some(Category::Temp), ALL),
        Vec::<String>::new()
    );
    let page = Page {
        offset: 1,
        limit: 2,
    };
    assert_eq!(names(&index, None, page), ["node_modules", "com.x"]);
    let past = Page {
        offset: 6,
        limit: 2,
    };
    assert_eq!(names(&index, None, past), Vec::<String>::new());
}

#[test]
fn recent_build_output_and_worktrees_are_not_preselected() {
    let (_fx, index) = indexed();
    let preselected: BTreeMap<String, bool> = candidates(&index, None, ALL)
        .into_iter()
        .map(|c| {
            (
                c.path.file_name().unwrap().to_string_lossy().into_owned(),
                c.preselected,
            )
        })
        .collect();
    let expected: BTreeMap<String, bool> = [
        ("target", true),
        ("node_modules", false),
        ("com.x", true),
        ("com.y", true),
        ("wt", false),
        ("ab", true),
    ]
    .into_iter()
    .map(|(name, pre)| (name.to_owned(), pre))
    .collect();
    assert_eq!(preselected, expected);
}

#[test]
fn preselection_turns_on_one_second_after_the_recent_age() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    let edge = NOW.0 - 14 * DAY;
    let rows = vec![
        dir(
            2,
            1,
            "fresh",
            20,
            Some(UnixTime(edge)),
            "build.node-modules",
        ),
        dir(
            3,
            1,
            "stale",
            10,
            Some(UnixTime(edge - 1)),
            "build.node-modules",
        ),
        read(1, None, "", 0, TreeTotals::directory(UnixTime(0))),
    ];
    write_verified(&index, rows);
    let found = candidates(&index, None, ALL);
    assert_eq!((found[0].preselected, found[1].preselected), (false, true));
    let shorter = Recency::new(NOW, Duration::from_secs(14 * 24 * 3600 - 1));
    let found = reader(&index)
        .read(|snap| snap.candidates(None, ALL, &shorter))
        .unwrap();
    assert!(found[0].preselected, "the age is configurable");
}

#[test]
fn tallies_count_candidates_and_bytes_per_rule_and_per_category() {
    let (_fx, index) = indexed();
    let (by_rule, by_category) = reader(&index)
        .read(|snap| Ok((snap.rule_tallies()?, snap.category_tallies()?)))
        .unwrap();
    let tally = |count, bytes| Tally {
        count,
        bytes: Bytes(bytes),
    };
    let rules: Vec<(String, Tally)> = by_rule
        .into_iter()
        .map(|(r, t)| (r.to_string(), t))
        .collect();
    assert_eq!(
        rules,
        [
            ("app-cache.library".to_owned(), tally(2, 300)),
            ("build.cargo-target".to_owned(), tally(1, 500)),
            ("build.node-modules".to_owned(), tally(1, 300)),
            ("log.user-library".to_owned(), tally(1, 40)),
            ("worktree.linked".to_owned(), tally(1, 50)),
        ]
    );
    let categories: Vec<(Category, Tally)> = by_category.into_iter().collect();
    assert_eq!(
        categories,
        [
            (Category::AppCache, tally(2, 300)),
            (Category::DevCache, tally(0, 0)),
            (Category::BuildArtifact, tally(2, 800)),
            (Category::Log, tally(1, 40)),
            (Category::GitWorktree, tally(1, 50)),
            (Category::Temp, tally(0, 0)),
        ]
    );
}

#[test]
fn a_candidate_is_found_by_id_with_its_identity() {
    let (_fx, index) = indexed();
    reader(&index)
        .read(|snap| {
            let wt = snap.candidate(DirId(8), &recency())?.unwrap();
            assert_eq!(wt.path, PathBuf::from(ROOT).join("wt"));
            assert_eq!((wt.device, wt.inode), (7, 1008));
            assert!(!wt.preselected);
            assert_eq!(
                snap.candidate(DirId(2), &recency())?,
                None,
                "not a candidate"
            );
            assert_eq!(snap.candidate(DirId(99), &recency())?, None, "not there");
            Ok(())
        })
        .unwrap();
}

fn visible(index: &Path) -> Vec<String> {
    names(index, None, ALL)
}

#[test]
fn an_exclusion_hides_its_path_and_everything_beneath_but_not_a_sibling_prefix() {
    let (_fx, index) = indexed();
    assert!(exclude(&index, "/synthetic/root/a/target"));
    assert_eq!(
        visible(&index),
        ["node_modules", "com.x", "com.y", "wt", "ab"]
    );
    assert!(exclude(&index, "/synthetic/root/Caches"));
    assert_eq!(visible(&index), ["node_modules", "wt", "ab"]);
    assert!(exclude(&index, "/synthetic/root/a"));
    assert_eq!(visible(&index), ["wt", "ab"], "/a does not hide /ab");
    let hidden = reader(&index)
        .read(|snap| snap.candidate(DirId(4), &recency()))
        .unwrap();
    assert_eq!(hidden, None);
    let tallies = reader(&index).read(|snap| snap.category_tallies()).unwrap();
    assert_eq!(tallies[&Category::BuildArtifact].count, 0);
    assert_eq!(tallies[&Category::Log].bytes, Bytes(40));
}

#[test]
fn a_candidate_that_holds_an_excluded_path_is_hidden_too() {
    let (_fx, index) = indexed();
    exclude(&index, "/synthetic/root/wt/src/keep.txt");
    exclude(&index, "/elsewhere/entirely");
    assert_eq!(
        visible(&index),
        ["target", "node_modules", "com.x", "com.y", "ab"]
    );
}

#[test]
fn excluding_the_root_or_above_hides_every_candidate() {
    let (_fx, index) = indexed();
    exclude(&index, "/synthetic");
    assert_eq!(visible(&index), Vec::<String>::new());
}

#[test]
fn exclusions_are_added_listed_and_removed_idempotently() {
    let (_fx, index) = indexed();
    assert!(exclude(&index, "/synthetic/root/b"));
    assert!(!exclude(&index, "/synthetic/root/b/"), "already excluded");
    assert!(exclude(&index, "/synthetic/root/a"));
    let listed = reader(&index).read(|snap| snap.exclusions()).unwrap();
    let paths: Vec<&Path> = listed.iter().map(Exclusion::path).collect();
    assert_eq!(
        paths,
        [
            Path::new("/synthetic/root/a"),
            Path::new("/synthetic/root/b")
        ]
    );
    assert!(include(&index, "/synthetic/root/a"));
    assert!(!include(&index, "/synthetic/root/a"), "already gone");
    assert_eq!(visible(&index).len(), 6);
}

#[test]
fn exclusions_survive_a_rescan_and_need_none() {
    let (_fx, index) = indexed();
    exclude(&index, "/synthetic/root/Caches");
    write_verified(&index, picture());
    assert_eq!(visible(&index), ["target", "node_modules", "wt", "ab"]);
}

#[test]
fn exclusions_can_be_edited_before_any_scan() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    assert!(exclude(&index, "/a"));
    assert_eq!(
        reader(&index).read(|snap| snap.exclusions()).unwrap().len(),
        1
    );
    assert_eq!(reader(&index).read(|snap| snap.scan()).unwrap(), None);
}

#[test]
fn the_picture_keeps_the_scans_bytes_by_kind() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    let mut kinds = KindTotals::default();
    kinds.add(FileKind::Media, Bytes(12));
    kinds.add(FileKind::Other, Bytes(3));
    let scanned = ScanSummary {
        kinds,
        ..summary(true)
    };
    write_summary(&index, picture(), &scanned);
    let picture = reader(&index)
        .read(|snap| snap.scan())
        .unwrap()
        .unwrap()
        .picture
        .unwrap();
    assert_eq!(picture.kinds, kinds);
}

fn sql(index: &Path, statement: &str) {
    rusqlite::Connection::open(index)
        .unwrap()
        .execute_batch(statement)
        .unwrap();
}

#[test]
fn rule_ids_identity_and_kinds_are_stored_in_a_stable_format() {
    let (_fx, index) = indexed();
    let conn = rusqlite::Connection::open(&index).unwrap();
    let (rule, device, inode): (String, i64, i64) = conn
        .query_row(
            "SELECT rule_id, device, inode FROM dirs WHERE id = 4",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (rule.as_str(), device, inode),
        ("build.cargo-target", 7, 1004)
    );
    let codes: Vec<i64> = conn
        .prepare("SELECT code FROM kinds ORDER BY code")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(codes, [0, 1, 2, 3, 4, 5, 6]);
    let version: i64 = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, 4);
    let verdict: i64 = conn
        .query_row("SELECT verdict FROM dirs WHERE id = 4", [], |r| r.get(0))
        .unwrap();
    assert_eq!(verdict, 0, "Verified is stored as 0");
}

#[test]
fn a_huge_device_or_inode_number_survives_the_round_trip() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    let root = DirRecord {
        device: u64::MAX,
        inode: u64::MAX - 1,
        ..read(1, None, "", 0, TreeTotals::directory(UnixTime(0)))
    };
    write(&index, vec![root], true);
    let stored = reader(&index).read(|snap| snap.root()).unwrap().unwrap();
    assert_eq!((stored.device, stored.inode), (u64::MAX, u64::MAX - 1));
}

#[test]
fn an_unknown_rule_id_or_an_invalid_exclusion_is_reported_as_corrupt() {
    let (_fx, index) = indexed();
    sql(&index, "UPDATE dirs SET rule_id = 'nonsense' WHERE id = 4");
    let error = reader(&index).read(|snap| snap.rule_tallies()).unwrap_err();
    assert!(matches!(error, IndexError::Corrupt(ref what) if what == "rule id nonsense"));
    let (_fx, index) = indexed();
    sql(&index, "INSERT INTO exclusions (path) VALUES (x'72656c')");
    let error = reader(&index).read(|snap| snap.exclusions()).unwrap_err();
    assert!(matches!(error, IndexError::Corrupt(ref what) if what == "exclusion rel"));
    let (_fx, index) = indexed();
    sql(&index, "UPDATE kinds SET code = 42 WHERE code = 0");
    let error = reader(&index).read(|snap| snap.scan()).unwrap_err();
    assert!(matches!(error, IndexError::Corrupt(ref what) if what == "kind code 42"));
}

#[test]
fn an_index_of_an_earlier_schema_version_is_rejected() {
    for old in [1, 2] {
        let fx = Fixture::new();
        let index = fx.path("old.sqlite");
        sql(
            &index,
            &format!(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value); \
                 INSERT INTO meta VALUES ('schema_version', {old});"
            ),
        );
        assert!(matches!(
            IndexWriter::open(&index),
            Err(IndexError::UnknownSchema(Some(v))) if v == old
        ));
    }
}
