//! What a row keeps so its directory can be rescanned on its own, and
//! rescans of one directory: shallow ones re-read only what changed,
//! recursive ones walk the subtree again, and the index applies either
//! in place.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use metla_core::domain::{
    Area, Bytes, DirFacts, DirId, DirRecord, DirState, FileKind, KindTotals, Passed, RescanTask,
    Rescanned, Stamp, UnixTime,
};
use metla_core::engine::{Refreshed, refresh};
use metla_core::index::{IndexReader, IndexWriter};
use metla_core::rules::RuleSet;
use metla_core::scan::{
    DirObserver, Inherited, ScanControl, ScanOptions, Stored, Target, rescan, run,
};

use crate::scan::{options, scan_with};
use crate::support::{Fixture, write_file};

fn rules(fx: &Fixture) -> Arc<RuleSet> {
    Arc::new(RuleSet::new(&fx.home(), OsStr::new("tester")))
}

fn ruled(fx: &Fixture) -> ScanOptions {
    options(fx.root()).rules(rules(fx))
}

fn markers(names: &[&str]) -> Vec<OsString> {
    names.iter().map(OsString::from).collect()
}

#[test]
fn a_row_keeps_what_its_directory_hands_down_and_its_own_times_and_kinds() {
    let fx = Fixture::new();
    fx.build(&[
        "code/app/Cargo.toml",
        "code/app/.git/HEAD=ref",
        "code/app/target/debug/out=binary",
        "code/app/src/main.rs=fn main() {}",
        "home/Movies/clip.mov=movie",
        "home/Movies/notes.txt=notes",
    ]);
    let scanned = scan_with(&ruled(&fx), &ScanControl::new(0));
    let app = scanned.at(&fx.path("code/app"));
    assert_eq!(
        app.passed,
        Passed {
            markers: markers(&["Cargo.toml"]),
            in_repo: true,
            in_candidate: false,
            area: Area::Developer,
        }
        .sorted_markers(&app.passed)
    );
    let target = scanned.at(&fx.path("code/app/target"));
    assert!(target.passed.in_candidate && target.passed.in_repo);
    assert!(
        target.passed.markers.is_empty(),
        "no markers inside a candidate"
    );
    let movies = scanned.at(&fx.path("home/Movies"));
    assert_eq!(movies.passed.area, Area::User);
    let DirState::Read(own) = movies.state else {
        panic!("{movies:?}")
    };
    let size = |name| Bytes(fs::metadata(fx.path(name)).unwrap().blocks() * 512);
    assert_eq!(own.kinds.get(FileKind::Media), size("home/Movies/clip.mov"));
    assert_eq!(
        own.kinds.get(FileKind::Documents),
        size("home/Movies/notes.txt")
    );
    let newest = fs::metadata(fx.path("home/Movies/notes.txt")).unwrap();
    let clip = fs::metadata(fx.path("home/Movies/clip.mov")).unwrap();
    assert_eq!(own.newest, Some(UnixTime(newest.mtime().max(clip.mtime()))));
    let listed = fs::symlink_metadata(fx.path("home/Movies")).unwrap();
    assert_eq!(movies.mtime, UnixTime(listed.mtime()));
    let stamp = listed.ctime() * 1_000_000_000 + listed.ctime_nsec();
    assert_eq!(movies.changed, Stamp(stamp));
    let mut total = KindTotals::default();
    for record in scanned.records.values() {
        if let DirState::Read(own) = record.state {
            total.merge(&own.kinds);
        }
    }
    assert_eq!(
        total, scanned.summary.kinds,
        "the scan's kinds are the rows' sum"
    );
}

/// Markers in a stable order, for comparing.
trait SortedMarkers {
    fn sorted_markers(self, like: &Passed) -> Passed;
}

impl SortedMarkers for Passed {
    fn sorted_markers(mut self, like: &Passed) -> Passed {
        let mut sorted = like.markers.clone();
        sorted.sort();
        assert_eq!(sorted, self.markers, "the same markers");
        self.markers = like.markers.clone();
        self
    }
}

#[test]
fn every_row_round_trips_through_the_index() {
    let fx = Fixture::new();
    fx.build(&[
        "code/app/Cargo.toml",
        "code/app/.git/HEAD=ref",
        "code/app/target/debug/out=binary",
        "home/Movies/clip.mov=movie",
        "empty/",
    ]);
    let options = ruled(&fx);
    let scanned = scan_with(&options, &ScanControl::new(0));
    let aux = Fixture::new();
    let index = aux.path("index.sqlite");
    let mut writer = IndexWriter::open(&index).unwrap();
    let session = writer.start_scan(UnixTime(1)).unwrap();
    session.send(scanned.records.values().cloned().collect());
    session.finish(&scanned.summary).unwrap();
    let mut reader = IndexReader::open(&index).unwrap();
    for (path, record) in &scanned.records {
        let stored = reader.read(|snap| snap.record(record.id)).unwrap();
        assert_eq!(stored.as_ref(), Some(record), "{}", path.display());
        let found = reader.read(|snap| snap.nearest(path)).unwrap();
        assert_eq!(found, Some((path.clone(), record.clone())));
    }
    let next = reader.read(|snap| snap.next_id()).unwrap();
    let largest = scanned.records.values().map(|r| r.id.0).max().unwrap();
    assert_eq!(next, DirId(largest + 1));
    let below = fx.path("empty/not/there");
    let nearest = reader.read(|snap| snap.nearest(&below)).unwrap().unwrap();
    assert_eq!(nearest.0, fx.path("empty"));
    let outside = reader
        .read(|snap| snap.nearest(Path::new("/elsewhere")))
        .unwrap();
    assert_eq!(outside, None);
}

/// Every directory the walk reads.
#[derive(Default)]
struct Reads(Mutex<Vec<PathBuf>>);

impl DirObserver for Reads {
    fn observe(&self, facts: &DirFacts<'_>) {
        self.0.lock().unwrap().push(facts.path.to_path_buf());
    }
}

fn target(scanned: &BTreeMap<PathBuf, DirRecord>, path: &Path, shallow: bool) -> Target {
    let row = scanned[path].clone();
    let children = scanned
        .values()
        .filter(|r| r.parent == Some(row.id))
        .cloned()
        .collect();
    let parent = scanned.values().find(|r| Some(r.id) == row.parent).unwrap();
    Target {
        path: path.to_path_buf(),
        id: row.id,
        parent: row.parent,
        depth: 1,
        inherited: Inherited {
            passed: parent.passed.clone(),
            grandparent_markers: Vec::new(),
        },
        stored: shallow.then_some(Stored { row, children }),
    }
}

#[test]
fn a_shallow_rescan_reads_only_the_directory_and_what_changed_below_it() {
    let fx = Fixture::new();
    fx.build(&["top/same/deep/x=1", "top/changed/y=2", "top/gone/z=3"]);
    let reads = Arc::new(Reads::default());
    let options = options(fx.root()).observer(reads.clone());
    let scanned = scan_with(&options, &ScanControl::new(0)).records;
    write_file(&fx.path("top/changed/new"), 100);
    fs::remove_dir_all(fx.path("top/gone")).unwrap();
    fs::create_dir(fx.path("top/added")).unwrap();
    write_file(&fx.path("top/own"), 300);
    reads.0.lock().unwrap().clear();
    let top = fx.path("top");
    let change = rescan(
        &options,
        target(&scanned, &top, true),
        DirId(100),
        &ScanControl::new(0),
    )
    .unwrap()
    .unwrap();
    let mut read = reads.0.lock().unwrap().clone();
    read.sort();
    assert_eq!(
        read,
        [top.clone(), fx.path("top/added"), fx.path("top/changed")]
    );
    assert_eq!(change.origin, scanned[&top].id);
    let mut removed = change.removed.clone();
    removed.sort();
    let mut expected = vec![
        scanned[&fx.path("top/changed")].id,
        scanned[&fx.path("top/gone")].id,
    ];
    expected.sort();
    assert_eq!(removed, expected, "what went, and what is walked again");
    let rows: BTreeMap<OsString, &DirRecord> =
        change.rows.iter().map(|r| (r.name.clone(), r)).collect();
    assert_eq!(rows.len(), 3, "top, changed, added");
    assert_eq!(rows[OsStr::new("top")].id, scanned[&top].id, "keeps its id");
    assert!(rows[OsStr::new("added")].id >= DirId(100));
    let same = &scanned[&fx.path("top/same")];
    let fresh = scan_with(&options, &ScanControl::new(0)).records;
    assert_eq!(
        rows[OsStr::new("top")].tree,
        fresh[&top].tree,
        "kept subtrees count"
    );
    assert_eq!(fresh[&fx.path("top/same")].tree, same.tree);
}

#[test]
fn a_shallow_rescan_walks_everything_again_when_the_directory_hands_down_something_else() {
    let fx = Fixture::new();
    fx.build(&["code/app/src/x=1", "code/app/target/debug/out=2"]);
    fs::create_dir(fx.path("code/.git")).unwrap();
    let reads = Arc::new(Reads::default());
    let options = ruled(&fx).observer(reads.clone());
    let scanned = scan_with(&options, &ScanControl::new(0)).records;
    assert_eq!(scanned[&fx.path("code/app/target")].rule, None);
    fs::write(fx.path("code/app/Cargo.toml"), "[package]").unwrap();
    reads.0.lock().unwrap().clear();
    let app = fx.path("code/app");
    let mut aimed = target(&scanned, &app, true);
    aimed.inherited.passed = scanned[&fx.path("code")].passed.clone();
    let change = rescan(&options, aimed, DirId(100), &ScanControl::new(0))
        .unwrap()
        .unwrap();
    assert_eq!(
        reads.0.lock().unwrap().len(),
        4,
        "app, src, target, debug: all again"
    );
    let target_row = change.rows.iter().find(|r| r.name == "target").unwrap();
    assert_eq!(
        target_row.rule.as_ref().map(|r| r.as_str()),
        Some("build.cargo-target")
    );
    assert_eq!(change.removed.len(), 2, "both children walked again");
}

#[test]
fn a_rescan_of_a_directory_that_is_no_longer_there_says_so() {
    let fx = Fixture::new();
    fx.build(&["a/b/c=1", "f/"]);
    let options = options(fx.root());
    let scanned = scan_with(&options, &ScanControl::new(0)).records;
    let b = fx.path("a/b");
    let aimed = target(&scanned, &b, true);
    fs::remove_dir_all(&b).unwrap();
    let gone = rescan(&options, aimed.clone(), DirId(50), &ScanControl::new(0)).unwrap();
    assert_eq!(gone, None);
    fs::write(&b, "now a file").unwrap();
    let file = rescan(&options, aimed.clone(), DirId(50), &ScanControl::new(0)).unwrap();
    assert_eq!(file, None);
    fs::remove_file(&b).unwrap();
    std::os::unix::fs::symlink(fx.path("f"), &b).unwrap();
    let link = rescan(&options, aimed, DirId(50), &ScanControl::new(0)).unwrap();
    assert_eq!(link, None, "a symlink is not followed");
    let mut below_link = target(&scanned, &fx.path("a/b"), false);
    below_link.path = fx.path("a/b/c");
    let through = rescan(&options, below_link, DirId(50), &ScanControl::new(0)).unwrap();
    assert_eq!(through, None, "nor is one on the way");
}

fn written(fx: &Fixture, options: &ScanOptions) -> (Fixture, PathBuf, IndexWriter) {
    let aux = Fixture::new();
    let index = aux.path("index.sqlite");
    let mut writer = IndexWriter::open(&index).unwrap();
    let session = writer.start_scan(UnixTime(1)).unwrap();
    let summary = run(options, &ScanControl::new(0), &|rows| session.send(rows)).unwrap();
    session.finish(&summary).unwrap();
    let _ = fx;
    (aux, index, writer)
}

#[test]
fn a_change_for_a_picture_that_was_replaced_is_not_applied() {
    let fx = Fixture::new();
    fx.build(&["a/x=1"]);
    let options = options(fx.root());
    let (_aux, index, mut writer) = written(&fx, &options);
    let mut reader = IndexReader::open(&index).unwrap();
    let picture = reader
        .read(|snap| snap.scan())
        .unwrap()
        .unwrap()
        .picture
        .unwrap()
        .id();
    let session = writer.start_scan(UnixTime(2)).unwrap();
    let summary = run(&options, &ScanControl::new(0), &|rows| session.send(rows)).unwrap();
    session.finish(&summary).unwrap();
    let root = reader.read(|snap| snap.root()).unwrap().unwrap();
    let change = Rescanned {
        origin: root.id,
        removed: vec![root.id],
        rows: Vec::new(),
    };
    assert!(!writer.apply(picture, change, UnixTime(3)).unwrap());
    assert!(
        reader.read(|snap| snap.root()).unwrap().is_some(),
        "nothing removed"
    );
}

#[test]
fn an_abandoned_scan_leaves_the_picture_current() {
    let fx = Fixture::new();
    fx.build(&["a/x=1"]);
    let options = options(fx.root());
    let (_aux, index, mut writer) = written(&fx, &options);
    let mut reader = IndexReader::open(&index).unwrap();
    let before = reader.read(|snap| snap.scan()).unwrap().unwrap();
    let session = writer.start_scan(UnixTime(9)).unwrap();
    session.send(Vec::new());
    let started = reader.read(|snap| snap.scan()).unwrap().unwrap();
    assert!(!started.is_current());
    session.abandon().unwrap();
    let after = reader.read(|snap| snap.scan()).unwrap().unwrap();
    assert_eq!(after.picture, before.picture);
    assert!(after.is_current());
    assert_eq!(after.started, after.picture.unwrap().started);
    let fresh = Fixture::new();
    let mut empty = IndexWriter::open(&fresh.path("index.sqlite")).unwrap();
    empty.start_scan(UnixTime(9)).unwrap().abandon().unwrap();
    let none = IndexReader::open(&fresh.path("index.sqlite"))
        .unwrap()
        .read(|snap| snap.scan())
        .unwrap();
    assert_eq!(none, None, "no scan ever finished or runs");
}

#[test]
fn a_cleaned_directory_is_offered_again_once_its_parent_is_rescanned() {
    let fx = Fixture::new();
    fx.build(&["~/Library/Caches/com.example.app/blob=cached"]);
    let options = ruled(&fx);
    let (_aux, index, writer) = written(&fx, &options);
    let mut reader = IndexReader::open(&index).unwrap();
    let cache = fx.path("home/Library/Caches/com.example.app");
    let (picture, row) = reader
        .read(|snap| {
            let picture = snap.scan()?.unwrap().picture.unwrap().id();
            Ok((picture, snap.dir(&cache)?.unwrap()))
        })
        .unwrap();
    writer.mark_cleaned(picture, vec![row.id]).unwrap();
    let offered = |reader: &mut IndexReader| {
        reader
            .read(|snap| snap.rule_tallies())
            .unwrap()
            .values()
            .map(|t| t.count)
            .sum::<u64>()
    };
    assert_eq!(offered(&mut reader), 0);
    fs::remove_file(cache.join("blob")).unwrap();
    let task = RescanTask::Recursive(cache.parent().unwrap().to_path_buf());
    let control = ScanControl::new(0);
    let done = refresh(&task, &options, &mut reader, &writer, &control, UnixTime(7)).unwrap();
    assert_eq!(done, Refreshed::Applied);
    assert_eq!(offered(&mut reader), 1, "the kept, emptied cache directory");
    let stale: i64 = rusqlite::Connection::open(&index)
        .unwrap()
        .query_row("SELECT count(*) FROM stale", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stale, 0);
}
