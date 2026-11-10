//! Which candidates the index offers: project output only with Git's
//! verdict, nothing inside a directory cleaned since the scan, and nothing
//! inside a worktree another tool manages preselected.

use std::path::{Path, PathBuf};

use metla_core::domain::{
    Bytes, Candidate, DirId, DirRecord, Manager, Recency, Reclaimable, ScanSummary, Tally,
    TreeTotals, UnixTime, Verdict,
};
use metla_core::index::{IndexWriter, Page};

use crate::candidates::{picture_id, verify};
use crate::index::{read, reader, summary, write_summary};
use crate::support::Fixture;

const ROOT: &str = "/synthetic/root";
const DAY: i64 = 24 * 3600;
const NOW: UnixTime = UnixTime(10_000 * DAY);
const OLD: Option<UnixTime> = Some(UnixTime(NOW.0 - 100 * DAY));
const ALL: Page = Page {
    offset: 0,
    limit: 100,
};

fn recency() -> Recency {
    Recency::new(NOW, Recency::DEFAULT_AGE)
}

fn row(id: i64, parent: i64, name: &str, bytes: u64, rule: &str) -> DirRecord {
    let tree = TreeTotals {
        bytes: Bytes(bytes),
        files: 1,
        dirs: 1,
        newest: OLD,
    };
    DirRecord {
        rule: metla_core::domain::RuleId::parse(rule),
        ..read(id, Some(parent), name, 0, tree)
    }
}

/// root ── home ── .codex ── worktrees ── a1 ── repo (worktree) ── node_modules
///      ├── wt (worktree, 900) ── target (600)
///      ├── web ── node_modules (NotIgnored), Pods (HasTrackedFiles),
///      │          .build (Unverifiable), dist-newstyle (Verified)
///      └── com.x (an app cache)
fn picture() -> Vec<DirRecord> {
    vec![
        read(1, None, "", 0, TreeTotals::directory(UnixTime(0))),
        row(2, 1, "home", 0, ""),
        row(3, 2, ".codex", 0, ""),
        row(4, 3, "worktrees", 0, ""),
        row(5, 4, "a1", 0, ""),
        row(6, 5, "repo", 400, "worktree.linked"),
        row(7, 6, "node_modules", 300, "build.node-modules"),
        row(8, 1, "wt", 900, "worktree.linked"),
        row(9, 8, "target", 600, "build.cargo-target"),
        row(10, 1, "web", 0, ""),
        row(11, 10, "node_modules", 50, "build.node-modules"),
        row(12, 10, "Pods", 40, "build.cocoapods"),
        row(13, 10, ".build", 30, "build.swiftpm"),
        row(14, 10, "dist-newstyle", 20, "build.haskell"),
        row(15, 1, "com.x", 10, "app-cache.library"),
    ]
}

fn write(index: &Path) {
    let scanned = ScanSummary {
        home: Some(PathBuf::from(ROOT).join("home")),
        ..summary(true)
    };
    write_summary(index, picture(), &scanned);
}

fn by_name(path: &Path) -> Verdict {
    match path.file_name().unwrap().to_str().unwrap() {
        "node_modules" if path.starts_with("/synthetic/root/web") => Verdict::NotIgnored,
        "Pods" => Verdict::HasTrackedFiles,
        ".build" => Verdict::Unverifiable,
        _ => Verdict::Verified,
    }
}

fn written() -> (Fixture, PathBuf) {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index);
    verify(&index, by_name);
    (fx, index)
}

fn offered(index: &Path) -> Vec<Candidate> {
    reader(index)
        .read(|snap| snap.candidates(None, ALL, &recency()))
        .unwrap()
}

fn relative(candidates: &[Candidate]) -> Vec<String> {
    let relative = candidates.iter().map(|c| {
        let path = c.path.strip_prefix(ROOT).unwrap();
        path.display().to_string()
    });
    relative.collect()
}

#[test]
fn project_output_is_offered_only_when_git_verified_it() {
    let (_fx, index) = written();
    assert_eq!(
        relative(&offered(&index)),
        [
            "wt",
            "wt/target",
            "home/.codex/worktrees/a1/repo",
            "home/.codex/worktrees/a1/repo/node_modules",
            "web/dist-newstyle",
            "com.x",
        ]
    );
    let unverified = reader(&index).read(|snap| snap.unverified()).unwrap();
    assert!(unverified.is_empty(), "every verdict is stored");
}

#[test]
fn git_is_asked_only_about_project_output_without_a_verdict() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index);
    let unverified = reader(&index).read(|snap| snap.unverified()).unwrap();
    let ids: Vec<i64> = unverified.iter().map(|(id, _)| id.0).collect();
    assert_eq!(ids, [7, 9, 11, 12, 13, 14]);
    assert_eq!(unverified[1].1, PathBuf::from(ROOT).join("wt/target"));
    assert_eq!(
        relative(&offered(&index)),
        ["wt", "home/.codex/worktrees/a1/repo", "com.x"],
        "unverified project output is not offered"
    );
    let candidate = reader(&index)
        .read(|snap| snap.candidate(DirId(9), &recency()))
        .unwrap();
    assert_eq!(candidate, None);
}

#[test]
fn verdicts_for_a_picture_that_was_replaced_are_dropped() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index);
    let old = reader(&index).read(picture_id).unwrap();
    write(&index);
    let writer = IndexWriter::open(&index).unwrap();
    let stored = writer
        .set_verdicts(old, vec![(DirId(9), Verdict::Verified)])
        .unwrap();
    assert!(!stored);
    let unverified = reader(&index).read(|snap| snap.unverified()).unwrap();
    assert_eq!(unverified.len(), 6);
}

#[test]
fn a_rescan_asks_git_again() {
    let (_fx, index) = written();
    write(&index);
    let unverified = reader(&index).read(|snap| snap.unverified()).unwrap();
    assert_eq!(unverified.len(), 6);
}

#[test]
fn nothing_inside_a_managed_worktree_is_preselected_or_reclaimable() {
    let (_fx, index) = written();
    let found = offered(&index);
    let inside = found.iter().find(|c| c.id == DirId(7)).unwrap();
    assert_eq!(inside.managed, Some(Manager::Codex));
    assert!(!inside.preselected);
    let target = found.iter().find(|c| c.id == DirId(9)).unwrap();
    assert_eq!((target.managed, target.preselected), (None, true));
    let reclaimable = reader(&index)
        .read(|snap| snap.reclaimable(&recency()))
        .unwrap();
    assert_eq!(
        reclaimable.freed,
        Tally {
            count: 3,
            bytes: Bytes(600 + 20 + 10)
        },
        "wt/target, web/dist-newstyle and com.x; worktrees never"
    );
    assert_eq!(reclaimable.trashed, Tally::default());
}

#[test]
fn what_would_move_to_the_trash_is_not_counted_as_freed() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    let rows = vec![
        read(1, None, "", 0, TreeTotals::directory(UnixTime(0))),
        row(2, 1, "com.x", 100, "app-cache.library"),
        row(3, 1, "Logs", 30, "log.user-library"),
        row(4, 1, ".Trash", 7, "temp.trash"),
        row(5, 1, "tmp", 5, "temp.other"),
    ];
    write_summary(&index, rows, &summary(true));
    let reclaimable = reader(&index)
        .read(|snap| snap.reclaimable(&recency()))
        .unwrap();
    let tally = |count, bytes| Tally {
        count,
        bytes: Bytes(bytes),
    };
    assert_eq!(
        reclaimable,
        Reclaimable {
            freed: tally(2, 107),
            trashed: tally(2, 35),
        },
        "the cache and the Trash's contents are removed; logs and temporary data move"
    );
}

#[test]
fn without_a_home_nothing_counts_as_managed() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write_summary(&index, picture(), &summary(true));
    verify(&index, by_name);
    let found = offered(&index);
    assert!(found.iter().all(|c| c.managed.is_none()));
    let picture = reader(&index).read(|snap| snap.scan()).unwrap().unwrap();
    assert_eq!(picture.picture.unwrap().home, None);
}

fn mark_cleaned(index: &Path, ids: &[i64]) -> bool {
    let picture = reader(index).read(picture_id).unwrap();
    let writer = IndexWriter::open(index).unwrap();
    let ids = ids.iter().map(|id| DirId(*id)).collect();
    writer.mark_cleaned(picture, ids).unwrap()
}

#[test]
fn a_cleaned_directory_and_everything_below_it_is_no_longer_offered() {
    let (_fx, index) = written();
    assert!(mark_cleaned(&index, &[8, 15]));
    assert!(mark_cleaned(&index, &[8]), "marking again changes nothing");
    assert_eq!(
        relative(&offered(&index)),
        [
            "home/.codex/worktrees/a1/repo",
            "home/.codex/worktrees/a1/repo/node_modules",
            "web/dist-newstyle",
        ]
    );
    let gone = reader(&index)
        .read(|snap| snap.candidate(DirId(9), &recency()))
        .unwrap();
    assert_eq!(gone, None);
    let tallies = reader(&index).read(|snap| snap.rule_tallies()).unwrap();
    assert_eq!(tallies.values().map(|t| t.count).sum::<u64>(), 3);
}

#[test]
fn the_next_scan_offers_cleaned_directories_again() {
    let (_fx, index) = written();
    mark_cleaned(&index, &[15]);
    write(&index);
    verify(&index, by_name);
    assert_eq!(offered(&index).len(), 6);
}

#[test]
fn cleaning_recorded_against_a_replaced_picture_is_dropped() {
    let (_fx, index) = written();
    let old = reader(&index).read(picture_id).unwrap();
    write(&index);
    verify(&index, by_name);
    let writer = IndexWriter::open(&index).unwrap();
    assert!(!writer.mark_cleaned(old, vec![DirId(15)]).unwrap());
    drop(writer);
    assert_eq!(offered(&index).len(), 6);
}
