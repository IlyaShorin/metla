use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use metla_core::domain::{
    Bytes, Coverage, DirId, DirRecord, DirState, Errno, Gap, GapAt, KindTotals, OwnTotals,
    ScanSummary, Skip, TreeTotals, UnixTime,
};
use metla_core::index::{IndexError, IndexReader, IndexWriter};

use crate::support::Fixture;

const ROOT: &str = "/synthetic/root";
const STARTED: UnixTime = UnixTime(990);

fn tree(bytes: u64, files: u64, dirs: u64) -> TreeTotals {
    TreeTotals {
        bytes: Bytes(bytes),
        files,
        dirs,
        newest: Some(UnixTime(50)),
    }
}

pub fn read(id: i64, parent: Option<i64>, name: &str, own: u64, total: TreeTotals) -> DirRecord {
    DirRecord {
        id: DirId(id),
        parent: parent.map(DirId),
        name: OsString::from(name),
        device: 7,
        inode: 1000 + id.unsigned_abs(),
        state: DirState::Read(OwnTotals {
            bytes: Bytes(own),
            files: 1,
        }),
        tree: total,
        rule: None,
    }
}

fn unread(id: i64, parent: i64, name: &str, state: DirState) -> DirRecord {
    DirRecord {
        id: DirId(id),
        parent: Some(DirId(parent)),
        name: OsString::from(name),
        device: 7,
        inode: 1000 + id.unsigned_abs(),
        state,
        tree: TreeTotals {
            newest: None,
            ..tree(0, 0, 1)
        },
        rule: None,
    }
}

/// root (10) ── a (100) ── x (200)
///          ├── b (100)
///          ├── denied, failed, gone (gaps)
///          └── mounted (skip)
fn picture() -> Vec<DirRecord> {
    vec![
        read(5, Some(2), "x", 200, tree(200, 1, 1)),
        read(2, Some(1), "a", 100, tree(300, 2, 2)),
        read(3, Some(1), "b", 100, tree(100, 1, 1)),
        unread(4, 1, "mounted", DirState::Skipped(Skip::OtherVolume)),
        unread(6, 1, "denied", DirState::Gap(Gap::Denied)),
        unread(7, 1, "failed", DirState::Gap(Gap::Failed(Errno(5)))),
        unread(8, 1, "gone", DirState::Gap(Gap::Vanished)),
        read(1, None, "", 10, tree(410, 5, 8)),
    ]
}

pub fn summary(complete: bool) -> ScanSummary {
    let mut coverage = Coverage::new(10);
    coverage.record_skip(Skip::OtherVolume);
    if !complete {
        coverage.record_gap(GapAt {
            depth: 1,
            path: PathBuf::from(ROOT).join("denied"),
            gap: Gap::Denied,
        });
    }
    ScanSummary {
        root: PathBuf::from(ROOT),
        home: None,
        started: UnixTime(1_000),
        finished: UnixTime(1_010),
        elapsed: Duration::from_secs(10),
        threads: 2,
        totals: tree(410, 5, 8),
        kinds: KindTotals::default(),
        coverage,
    }
}

pub fn write(index: &Path, rows: Vec<DirRecord>, complete: bool) {
    write_summary(index, rows, &summary(complete));
}

pub fn write_summary(index: &Path, rows: Vec<DirRecord>, summary: &ScanSummary) {
    let mut writer = IndexWriter::open(index).unwrap();
    let scan = writer.start_scan(STARTED).unwrap();
    for chunk in rows.chunks(3) {
        scan.send(chunk.to_vec());
    }
    scan.finish(summary).unwrap();
}

pub fn reader(index: &Path) -> IndexReader {
    IndexReader::open(index).unwrap()
}

fn names(records: &[DirRecord]) -> Vec<String> {
    records
        .iter()
        .map(|r| r.name.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn children_are_sorted_by_tree_size_and_keep_their_state() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, picture(), true);
    let children = reader(&index).read(|snap| snap.children(DirId(1))).unwrap();
    assert_eq!(
        names(&children),
        ["a", "b", "denied", "failed", "gone", "mounted"]
    );
    let expected = picture();
    for child in &children {
        assert!(expected.contains(child), "{child:?} changed on the way");
    }
}

#[test]
fn the_root_and_a_directory_are_found_by_path() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, picture(), true);
    reader(&index)
        .read(|snap| {
            assert_eq!(snap.root()?.unwrap().id, DirId(1));
            assert_eq!(snap.dir(Path::new(ROOT))?.unwrap().id, DirId(1));
            assert_eq!(
                snap.dir(&Path::new(ROOT).join("a/x"))?.unwrap().id,
                DirId(5)
            );
            assert_eq!(snap.dir(&Path::new(ROOT).join("a/nope"))?, None);
            assert_eq!(snap.dir(Path::new("/elsewhere"))?, None);
            assert_eq!(snap.path_of(DirId(5))?, Some(Path::new(ROOT).join("a/x")));
            assert_eq!(snap.path_of(DirId(1))?, Some(PathBuf::from(ROOT)));
            assert_eq!(snap.path_of(DirId(99))?, None);
            Ok(())
        })
        .unwrap();
}

#[test]
fn top_lists_the_largest_directories_below_a_directory() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, picture(), true);
    reader(&index)
        .read(|snap| {
            assert_eq!(names(&snap.top(DirId(1), 2)?), ["a", "x"]);
            assert_eq!(names(&snap.top(DirId(2), 10)?), ["x"]);
            assert_eq!(snap.top(DirId(5), 10)?, []);
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_scan_record_describes_the_picture() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, picture(), false);
    let record = reader(&index).read(|snap| snap.scan()).unwrap().unwrap();
    let shown = record.picture.as_ref().unwrap();
    assert_eq!(shown.root, PathBuf::from(ROOT));
    assert_eq!(shown.started, UnixTime(1_000));
    assert_eq!(shown.finished, UnixTime(1_010));
    assert_eq!(shown.coverage, summary(false).coverage);
    assert!(!record.is_complete());
    write(&index, picture(), true);
    let record = reader(&index).read(|snap| snap.scan()).unwrap().unwrap();
    assert!(record.is_complete());
    assert_eq!(record.picture.unwrap().coverage, summary(true).coverage);
}

#[test]
fn a_rescan_replaces_the_picture_and_repeating_it_changes_nothing() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, picture(), true);
    let first = dump(&index);
    write(&index, vec![read(1, None, "", 7, tree(7, 1, 1))], true);
    assert_eq!(dump(&index).len(), 1);
    write(&index, picture(), true);
    assert_eq!(dump(&index), first);
}

fn dump(index: &Path) -> Vec<DirRecord> {
    reader(index)
        .read(|snap| {
            let mut all = vec![snap.root()?.unwrap()];
            all.extend(snap.top(DirId(1), 100)?);
            Ok(all)
        })
        .unwrap()
}

#[test]
fn an_interrupted_scan_leaves_the_index_incomplete_with_the_old_picture() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, picture(), true);
    {
        let mut writer = IndexWriter::open(&index).unwrap();
        let scan = writer.start_scan(STARTED).unwrap();
        scan.send(vec![read(1, None, "", 7, tree(7, 1, 1))]);
    }
    let (record, root) = reader(&index)
        .read(|snap| Ok((snap.scan()?.unwrap(), snap.root()?.unwrap())))
        .unwrap();
    assert!(!record.is_complete());
    assert!(record.picture.is_some());
    assert_eq!(record.started, STARTED);
    assert_eq!(root.tree, tree(410, 5, 8));
}

#[test]
fn a_first_scan_that_never_finished_has_no_picture() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    {
        let mut writer = IndexWriter::open(&index).unwrap();
        writer.start_scan(STARTED).unwrap();
    }
    let (record, root) = reader(&index)
        .read(|snap| Ok((snap.scan()?.unwrap(), snap.root()?)))
        .unwrap();
    assert!(!record.is_complete());
    assert!(record.picture.is_none());
    assert_eq!(root, None);
}

#[test]
fn a_failed_write_is_reported_when_the_scan_finishes() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    let mut writer = IndexWriter::open(&index).unwrap();
    let scan = writer.start_scan(STARTED).unwrap();
    scan.send(vec![read(1, None, "", 7, tree(7, 1, 1))]);
    scan.send(vec![read(1, None, "", 7, tree(7, 1, 1))]);
    scan.send(vec![read(2, Some(1), "late", 7, tree(7, 1, 1))]);
    assert!(matches!(
        scan.finish(&summary(true)),
        Err(IndexError::Sqlite(_))
    ));
    let record = reader(&index).read(|snap| snap.scan()).unwrap().unwrap();
    assert!(!record.is_complete());
}

fn foreign_database(path: &Path, sql: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(sql).unwrap();
}

#[test]
fn an_index_with_an_unknown_schema_version_is_rejected() {
    let fx = Fixture::new();
    let index = fx.path("future.sqlite");
    foreign_database(
        &index,
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value); \
         INSERT INTO meta VALUES ('schema_version', 99);",
    );
    assert!(matches!(
        IndexWriter::open(&index),
        Err(IndexError::UnknownSchema(Some(99)))
    ));
    assert!(matches!(
        IndexReader::open(&index),
        Err(IndexError::UnknownSchema(Some(99)))
    ));
}

#[test]
fn a_database_that_is_not_an_index_is_rejected() {
    let fx = Fixture::new();
    let index = fx.path("other.sqlite");
    foreign_database(&index, "CREATE TABLE something (x);");
    assert!(matches!(
        IndexWriter::open(&index),
        Err(IndexError::UnknownSchema(None))
    ));
    assert!(matches!(
        IndexReader::open(&index),
        Err(IndexError::UnknownSchema(None))
    ));
}

#[test]
fn a_missing_index_cannot_be_read() {
    let fx = Fixture::new();
    assert!(IndexReader::open(&fx.path("missing.sqlite")).is_err());
}

/// A wide two-level picture whose root total is `width * own`.
fn wide_picture(width: i64, own: u64) -> Vec<DirRecord> {
    let mut rows: Vec<DirRecord> = (2..width + 2)
        .map(|id| read(id, Some(1), &format!("d{id}"), own, tree(own, 1, 1)))
        .collect();
    let total = own * width.unsigned_abs();
    rows.push(read(1, None, "", 0, tree(total, 0, 1)));
    rows
}

#[test]
fn a_reader_sees_one_consistent_picture_while_scans_are_written() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, wide_picture(300, 3), true);
    let done = AtomicBool::new(false);
    std::thread::scope(|threads| {
        threads.spawn(|| {
            for round in 0..6 {
                write(
                    &index,
                    wide_picture(300 + round * 50, 3 + round as u64),
                    true,
                );
            }
            done.store(true, Ordering::SeqCst);
        });
        let mut reader = reader(&index);
        let mut checks = 0;
        while !done.load(Ordering::SeqCst) || checks == 0 {
            reader
                .read(|snap| {
                    let root = snap.root()?.unwrap();
                    let children = snap.children(root.id)?;
                    let sum: Bytes = children.iter().map(|c| c.tree.bytes).sum();
                    assert_eq!(sum, root.tree.bytes, "a mixed picture was visible");
                    assert!(snap.scan()?.unwrap().picture.is_some());
                    Ok(())
                })
                .unwrap();
            checks += 1;
        }
    });
}

#[test]
fn the_stored_state_codes_are_a_stable_file_format() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    write(&index, picture(), false);
    let conn = rusqlite::Connection::open(&index).unwrap();
    let mut statement = conn
        .prepare("SELECT name, flags, own_bytes IS NULL FROM dirs ORDER BY id")
        .unwrap();
    let stored: Vec<(Vec<u8>, i64, bool)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let expected: Vec<(Vec<u8>, i64, bool)> = [
        ("", 0, false),
        ("a", 0, false),
        ("b", 0, false),
        ("mounted", 16, true),
        ("x", 0, false),
        ("denied", 1, true),
        ("failed", 2 | 5 << 8, true),
        ("gone", 3, true),
    ]
    .into_iter()
    .map(|(name, flags, unread)| (name.as_bytes().to_vec(), flags, unread))
    .collect();
    assert_eq!(stored, expected);
    let counts: Vec<(i64, i64)> = conn
        .prepare("SELECT code, count FROM coverage_counts ORDER BY code")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(counts, [(1, 1), (16, 1)]);
}

#[test]
fn every_gap_and_skip_code_survives_the_round_trip() {
    let fx = Fixture::new();
    let index = fx.path("index.sqlite");
    let states = [
        DirState::Gap(Gap::Denied),
        DirState::Gap(Gap::Failed(Errno(60))),
        DirState::Gap(Gap::Vanished),
        DirState::Gap(Gap::Cancelled),
        DirState::Skipped(Skip::OtherVolume),
        DirState::Skipped(Skip::MountPoint),
        DirState::Skipped(Skip::Dataless),
        DirState::Skipped(Skip::Excluded),
    ];
    let mut rows: Vec<DirRecord> = (0i64..)
        .zip(states)
        .map(|(n, state)| unread(n + 2, 1, &format!("d{n}"), state))
        .collect();
    rows.push(read(1, None, "", 0, tree(0, 0, 9)));
    let mut coverage = Coverage::new(10);
    for (n, state) in (0u32..).zip(states) {
        match state {
            DirState::Gap(gap) => coverage.record_gap(GapAt {
                depth: n,
                path: PathBuf::from(format!("/g{n}")),
                gap,
            }),
            DirState::Skipped(skip) => coverage.record_skip(skip),
            DirState::Read(_) => unreachable!(),
        }
    }
    let scanned = ScanSummary {
        coverage: coverage.clone(),
        ..summary(true)
    };
    write_summary(&index, rows.clone(), &scanned);
    let (children, record) = reader(&index)
        .read(|snap| Ok((snap.children(DirId(1))?, snap.scan()?.unwrap())))
        .unwrap();
    let mut stored: Vec<DirState> = children.iter().map(|c| c.state).collect();
    stored.sort_by_key(|s| format!("{s:?}"));
    let mut sent: Vec<DirState> = states.to_vec();
    sent.sort_by_key(|s| format!("{s:?}"));
    assert_eq!(stored, sent);
    assert_eq!(record.picture.unwrap().coverage, coverage);
}

#[test]
fn index_errors_explain_themselves() {
    let messages = [
        IndexError::UnknownSchema(Some(7)).to_string(),
        IndexError::UnknownSchema(None).to_string(),
        IndexError::Corrupt("gap code 9".into()).to_string(),
        IndexError::Sqlite(rusqlite::Error::InvalidQuery).to_string(),
        IndexError::WriterGone.to_string(),
    ];
    assert_eq!(messages[0], "unknown index schema version 7");
    assert_eq!(messages[1], "not a Metla index");
    assert_eq!(messages[2], "corrupt index: gap code 9");
    assert!(messages[3].starts_with("index: "));
    assert_eq!(messages[4], "index writer stopped");
}
