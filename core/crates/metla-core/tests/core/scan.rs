use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use metla_core::domain::{
    Bytes, DirFacts, DirId, DirRecord, DirState, Gap, GapKind, ScanSummary, Skip, TreeTotals,
};
use metla_core::index::{IndexReader, IndexWriter};
use metla_core::rules::RuleSet;
use metla_core::scan::{
    DirObserver, ROW_BATCH, ScanControl, ScanOptions, ScanRoot, ScanScope, run,
};

use crate::support::{Fixture, write_file};

/// A finished scan: its summary and every record by path.
pub struct Scanned {
    pub summary: ScanSummary,
    pub records: BTreeMap<PathBuf, DirRecord>,
    /// The size of every batch the sink received, in order.
    pub batches: Vec<usize>,
}

impl Scanned {
    pub fn at(&self, path: &Path) -> &DirRecord {
        self.records
            .get(path)
            .unwrap_or_else(|| panic!("no record for {}", path.display()))
    }
}

pub fn options(root: &Path) -> ScanOptions {
    let root = ScanRoot::new(root).unwrap();
    let scope = ScanScope::detect(&root).unwrap();
    ScanOptions::new(root, scope)
}

pub fn scan_with(options: &ScanOptions, control: &ScanControl) -> Scanned {
    let rows = Mutex::new((Vec::new(), Vec::new()));
    let summary = run(options, control, &|batch: Vec<DirRecord>| {
        let mut rows = rows.lock().unwrap();
        rows.1.push(batch.len());
        rows.0.extend(batch);
    })
    .unwrap();
    let (rows, batches) = rows.into_inner().unwrap();
    let records = by_path(options.root().path(), rows);
    Scanned {
        summary,
        records,
        batches,
    }
}

pub fn scan(root: &Path) -> Scanned {
    scan_with(&options(root), &ScanControl::new(64))
}

/// Records keyed by full path, rebuilt from their parent links.
fn by_path(root: &Path, rows: Vec<DirRecord>) -> BTreeMap<PathBuf, DirRecord> {
    let by_id: BTreeMap<_, _> = rows.iter().map(|r| (r.id, r)).collect();
    assert_eq!(by_id.len(), rows.len(), "a directory was recorded twice");
    let mut paths: BTreeMap<DirId, PathBuf> = BTreeMap::new();
    for row in &rows {
        let mut chain = vec![];
        let mut current = Some(row.id);
        while let Some(id) = current.filter(|id| !paths.contains_key(id)) {
            chain.push(id);
            current = by_id[&id].parent;
        }
        let mut path = current.map_or_else(|| root.to_path_buf(), |id| paths[&id].clone());
        for id in chain.into_iter().rev() {
            let record = by_id[&id];
            if record.parent.is_some() {
                path = path.join(&record.name);
            }
            paths.insert(id, path.clone());
        }
    }
    rows.into_iter()
        .map(|r| (paths[&r.id].clone(), r))
        .collect()
}

fn allocated(path: &Path) -> Bytes {
    Bytes(fs::symlink_metadata(path).unwrap().blocks() * 512)
}

fn own(record: &DirRecord) -> Bytes {
    match record.state {
        DirState::Read(own) => own.bytes,
        other => panic!("not read: {other:?}"),
    }
}

#[test]
fn nested_and_empty_directories_add_up() {
    let fx = Fixture::new();
    fx.dir("a/b/c");
    fx.dir("e");
    write_file(&fx.path("a/f"), 5000);
    write_file(&fx.path("top"), 100);
    let scanned = scan(fx.root());
    let file_bytes = allocated(&fx.path("a/f")) + allocated(&fx.path("top"));
    let root = scanned.at(fx.root());
    assert_eq!(root.tree.dirs, 5);
    assert_eq!(root.tree.files, 2);
    assert_eq!(root.tree.bytes, file_bytes);
    assert_eq!(scanned.summary.totals, root.tree);
    let a = scanned.at(&fx.path("a"));
    assert_eq!((a.tree.dirs, a.tree.files), (3, 1));
    assert_eq!(own(a), allocated(&fx.path("a/f")));
    assert_eq!(scanned.at(&fx.path("a/b/c")).tree.dirs, 1);
    assert_eq!(scanned.records.len(), 5);
    assert!(scanned.summary.is_complete());
    assert_eq!(scanned.summary.root, fx.root());
}

#[test]
fn the_newest_modification_time_comes_from_anywhere_in_the_tree() {
    let fx = Fixture::new();
    fx.dir("old/deeper");
    write_file(&fx.path("old/deeper/new"), 1);
    let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    for dir in ["old/deeper", "old", ""] {
        File::open(fx.path(dir))
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
    let newest = fs::metadata(fx.path("old/deeper/new")).unwrap().mtime();
    let scanned = scan(fx.root());
    assert_eq!(scanned.at(fx.root()).tree.newest.unwrap().0, newest);
    assert_eq!(scanned.at(&fx.path("old")).tree.newest.unwrap().0, newest);
}

#[test]
fn a_sparse_file_counts_by_allocated_size() {
    let fx = Fixture::new();
    let file = File::create(fx.path("sparse")).unwrap();
    file.set_len(1 << 30).unwrap();
    let scanned = scan(fx.root());
    let root = scanned.at(fx.root());
    assert_eq!(root.tree.bytes, allocated(&fx.path("sparse")));
    assert!(root.tree.bytes.0 < 1 << 20);
    assert_eq!(fs::metadata(fx.path("sparse")).unwrap().len(), 1 << 30);
}

#[test]
fn a_hard_linked_file_is_counted_once_across_its_links() {
    let fx = Fixture::new();
    fx.dir("one");
    fx.dir("two");
    write_file(&fx.path("one/data"), 64 * 1024);
    fs::hard_link(fx.path("one/data"), fx.path("two/data")).unwrap();
    let scanned = scan(fx.root());
    let size = allocated(&fx.path("one/data"));
    assert_eq!(scanned.at(fx.root()).tree.bytes, size);
    assert_eq!(own(scanned.at(&fx.path("one"))), Bytes(size.0 / 2));
    assert_eq!(scanned.at(fx.root()).tree.files, 2);
}

#[test]
fn a_symlink_to_a_big_directory_is_not_followed() {
    let fx = Fixture::new();
    let elsewhere = Fixture::new();
    write_file(&elsewhere.path("big"), 1 << 20);
    symlink(elsewhere.root(), fx.path("link")).unwrap();
    let scanned = scan(fx.root());
    let root = scanned.at(fx.root());
    assert_eq!(root.tree.bytes, allocated(&fx.path("link")));
    assert_eq!((root.tree.files, root.tree.dirs), (1, 1));
}

#[test]
fn a_symlink_loop_is_harmless() {
    let fx = Fixture::new();
    symlink(".", fx.path("self")).unwrap();
    symlink("b", fx.path("a")).unwrap();
    symlink("a", fx.path("b")).unwrap();
    let scanned = scan(fx.root());
    assert_eq!(scanned.at(fx.root()).tree.files, 3);
    assert!(scanned.summary.is_complete());
}

fn lock(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
}

fn unlock(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_unreadable_directory_is_a_denied_gap_and_the_rest_is_counted() {
    let fx = Fixture::new();
    fx.dir("open");
    write_file(&fx.path("open/f"), 4096);
    fx.dir("locked/inner");
    lock(&fx.path("locked"));
    let control = ScanControl::new(64);
    let scanned = scan_with(&options(fx.root()), &control);
    unlock(&fx.path("locked"));
    let locked = scanned.at(&fx.path("locked"));
    assert_eq!(locked.state, DirState::Gap(Gap::Denied));
    assert_eq!(locked.tree.dirs, 1);
    let summary = &scanned.summary;
    assert!(!summary.is_complete());
    assert_eq!(summary.coverage.gaps(GapKind::Denied), 1);
    assert_eq!(summary.coverage.top_gaps()[0].path, fx.path("locked"));
    assert_eq!(summary.coverage.top_gaps()[0].depth, 1);
    assert_eq!(summary.totals.bytes, allocated(&fx.path("open/f")));
    assert_eq!(summary.totals.dirs, 3);
    assert_eq!(control.progress().denied, 1);
}

#[test]
fn an_unreadable_root_is_a_partial_scan_of_one_directory() {
    let fx = Fixture::new();
    let root = fx.dir("root");
    let options = options(&root);
    lock(&root);
    let scanned = scan_with(&options, &ScanControl::new(64));
    unlock(&root);
    assert_eq!(scanned.records.len(), 1);
    assert_eq!(scanned.at(&root).state, DirState::Gap(Gap::Denied));
    assert!(!scanned.summary.is_complete());
    assert_eq!(scanned.summary.totals.dirs, 1);
}

/// Calls a closure for every directory read.
struct Hook<F: Fn(&DirFacts<'_>) + Send + Sync>(F);

impl<F: Fn(&DirFacts<'_>) + Send + Sync> DirObserver for Hook<F> {
    fn observe(&self, facts: &DirFacts<'_>) {
        (self.0)(facts)
    }
}

#[test]
fn a_directory_deleted_during_the_scan_is_a_vanished_gap() {
    let fx = Fixture::new();
    fx.dir("doomed/inner");
    fx.dir("kept");
    write_file(&fx.path("kept/f"), 10);
    let root = fx.root().to_path_buf();
    let doomed = fx.path("doomed");
    let hook = Hook(move |facts: &DirFacts<'_>| {
        if facts.path == root {
            fs::remove_dir_all(&doomed).unwrap();
        }
    });
    let options = options(fx.root()).observer(Arc::new(hook));
    let scanned = scan_with(&options, &ScanControl::new(64));
    assert_eq!(
        scanned.at(&fx.path("doomed")).state,
        DirState::Gap(Gap::Vanished)
    );
    assert_eq!(scanned.summary.coverage.gaps(GapKind::Vanished), 1);
    assert_eq!(scanned.summary.totals.files, 1);
}

/// What the observer saw of one directory.
#[derive(Debug, PartialEq)]
struct Seen {
    entries: Vec<OsString>,
    parent_markers: Vec<OsString>,
    grandparent_markers: Vec<OsString>,
    inside_git_repo: bool,
    probes: Vec<(OsString, Vec<u8>)>,
}

fn sorted(names: &[OsString]) -> Vec<OsString> {
    let mut names = names.to_vec();
    names.sort();
    names
}

#[test]
fn the_observer_gets_entries_markers_probes_and_repository_context() {
    let fx = Fixture::new();
    fx.dir("repo/.git");
    fx.dir("repo/target/debug");
    write_file(&fx.path("repo/Cargo.toml"), 1);
    write_file(&fx.path("repo/README"), 1);
    fs::write(fx.path("repo/target/CACHEDIR.TAG"), "Signature: 1").unwrap();
    fx.dir("plain");
    let seen = Arc::new(Mutex::new(BTreeMap::new()));
    let sink = Arc::clone(&seen);
    let hook = Hook(move |facts: &DirFacts<'_>| {
        let names: Vec<OsString> = facts.entries.iter().map(|e| e.name.clone()).collect();
        let probes = facts
            .probes
            .iter()
            .map(|p| (p.name.clone(), p.head.clone()));
        sink.lock().unwrap().insert(
            facts.path.to_path_buf(),
            Seen {
                entries: sorted(&names),
                parent_markers: sorted(facts.parent_markers),
                grandparent_markers: sorted(facts.grandparent_markers),
                inside_git_repo: facts.inside_git_repo,
                probes: probes.collect(),
            },
        );
    });
    let rules = RuleSet::new(&fx.path("home"), OsStr::new("tester"));
    let options = options(fx.root())
        .rules(Arc::new(rules))
        .observer(Arc::new(hook));
    scan_with(&options, &ScanControl::new(64));
    let seen = seen.lock().unwrap();
    let names = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
    assert_eq!(
        seen[&fx.path("repo/target")],
        Seen {
            entries: names(&["CACHEDIR.TAG", "debug"]),
            parent_markers: names(&["Cargo.toml"]),
            grandparent_markers: names(&[]),
            inside_git_repo: true,
            probes: vec![("CACHEDIR.TAG".into(), b"Signature: 1".to_vec())],
        }
    );
    let debug = &seen[&fx.path("repo/target/debug")];
    assert_eq!(debug.grandparent_markers, names(&["Cargo.toml"]));
    assert!(debug.inside_git_repo);
    assert!(!seen[&fx.path("repo")].inside_git_repo);
    assert_eq!(
        seen[&fx.path("repo")].probes,
        [],
        "only the rules' files are read"
    );
    assert!(!seen[&fx.path("plain")].inside_git_repo);
    assert_eq!(
        seen[&fx.path("repo")].entries,
        names(&[".git", "Cargo.toml", "README", "target"])
    );
    assert_eq!(seen.len(), 6);
}

#[test]
fn cancelling_stops_reading_and_leaves_the_index_incomplete() {
    let fx = Fixture::new();
    for n in 0..20 {
        fx.dir(&format!("d{n}/sub"));
    }
    let control = Arc::new(ScanControl::new(64));
    let read = Arc::new(Mutex::new(0));
    let (hook_control, hook_read) = (Arc::clone(&control), Arc::clone(&read));
    let hook = Hook(move |_: &DirFacts<'_>| {
        let mut read = hook_read.lock().unwrap();
        *read += 1;
        if *read == 5 {
            hook_control.cancel();
        }
    });
    let options = options(fx.root()).threads(1).observer(Arc::new(hook));
    let index = fx.path("index.sqlite");
    let mut writer = IndexWriter::open(&index).unwrap();
    let scan = writer.start_scan(metla_core::fs::now()).unwrap();
    let summary = run(&options, &control, &|rows| scan.send(rows)).unwrap();
    scan.finish(&summary).unwrap();
    assert_eq!(*read.lock().unwrap(), 5);
    assert!(!summary.is_complete());
    assert!(summary.coverage.gaps(GapKind::Cancelled) >= 16);
    let record = IndexReader::open(&index)
        .unwrap()
        .read(|snap| snap.scan())
        .unwrap()
        .unwrap();
    assert!(!record.is_complete());
    assert!(control.is_cancelled());
}

fn sorted_records(scanned: &Scanned) -> Vec<(PathBuf, DirState, TreeTotals)> {
    scanned
        .records
        .iter()
        .map(|(path, r)| (path.clone(), r.state, r.tree))
        .collect()
}

#[test]
fn one_thread_and_many_threads_give_identical_results() {
    let fx = Fixture::new();
    for a in 0..8 {
        for b in 0..6 {
            let dir = fx.dir(&format!("a{a}/b{b}"));
            write_file(&dir.join("f"), 100 * (a + b + 1));
        }
        write_file(&fx.path(&format!("a{a}/g")), 3000);
    }
    let single = scan_with(&options(fx.root()).threads(1), &ScanControl::new(64));
    let many = scan_with(&options(fx.root()).threads(6), &ScanControl::new(64));
    assert_eq!(sorted_records(&single), sorted_records(&many));
    assert_eq!(single.summary.totals, many.summary.totals);
    assert_eq!(single.summary.totals.dirs, 57);
    assert_eq!(single.summary.threads, 1);
    assert_eq!(many.summary.threads, 6);
}

const PIECE: usize = 400;

/// A chain `x/x/x/...` of `depth` directories below `base`, built in
/// pieces short enough for `PATH_MAX` and moved into each other.
pub fn deep_chain(base: &Path, depth: usize) -> PathBuf {
    let mut built: Option<PathBuf> = None;
    for piece in 0..depth / PIECE {
        let top = base.join(format!("piece{piece}"));
        let bottom = (1..PIECE).fold(top.clone(), |p, _| p.join("x"));
        fs::create_dir_all(&bottom).unwrap();
        if let Some(lower) = built.take() {
            fs::rename(&lower, bottom.join("x")).unwrap();
        }
        built = Some(top);
    }
    built.unwrap()
}

/// Takes a deep chain apart piece by piece, so that removing it never
/// recurses deeper than one piece.
fn remove_deep_chain(base: &Path, mut top: PathBuf) {
    for piece in 0.. {
        let bottom = (1..PIECE).fold(top.clone(), |p, _| p.join("x"));
        let lower = bottom.join("x");
        let rest = lower.exists().then(|| base.join(format!("rest{piece}")));
        if let Some(rest) = &rest {
            fs::rename(&lower, rest).unwrap();
        }
        fs::remove_dir_all(&top).unwrap();
        match rest {
            Some(rest) => top = rest,
            None => return,
        }
    }
}

#[test]
fn a_very_deep_tree_is_walked_completely() {
    let fx = Fixture::new();
    let chain = deep_chain(fx.root(), 2000);
    let scanned = scan_with(&options(&chain).threads(3), &ScanControl::new(64));
    remove_deep_chain(fx.root(), chain);
    assert!(
        scanned.summary.is_complete(),
        "{:?}",
        scanned.summary.coverage
    );
    assert_eq!(scanned.summary.totals.dirs, 2000);
}

#[test]
fn a_wide_directory_is_walked_completely() {
    let fx = Fixture::new();
    let wide = fx.dir("wide");
    for n in 0..18_900 {
        File::create(wide.join(format!("f{n}"))).unwrap();
    }
    for n in 0..1_100 {
        fs::create_dir(wide.join(format!("d{n}"))).unwrap();
    }
    let control = ScanControl::new(64);
    let scanned = scan_with(&options(fx.root()).threads(4), &control);
    let wide = scanned.at(&fx.path("wide"));
    assert_eq!((wide.tree.files, wide.tree.dirs), (18_900, 1_101));
    let progress = control.progress();
    assert_eq!(progress.dirs, 1_102);
    assert_eq!(progress.files, 18_900);
    let mut batches = scanned.batches.clone();
    batches.sort();
    assert_eq!(batches, [1_102 - ROW_BATCH, ROW_BATCH]);
}

#[test]
fn progress_counts_what_was_recorded() {
    let fx = Fixture::new();
    fx.dir("a/b");
    write_file(&fx.path("a/b/f"), 9000);
    write_file(&fx.path("g"), 10);
    let control = ScanControl::new(64);
    let scanned = scan_with(&options(fx.root()), &control);
    let progress = control.progress();
    assert_eq!(progress.dirs, 3);
    assert_eq!(progress.files, 2);
    assert_eq!(progress.bytes, scanned.summary.totals.bytes);
    assert_eq!(progress.denied, 0);
}

#[test]
fn a_scan_uses_one_thread_unless_told_otherwise_and_never_zero() {
    let fx = Fixture::new();
    let control = ScanControl::new(64);
    assert_eq!(scan_with(&options(fx.root()), &control).summary.threads, 1);
    let zero = options(fx.root()).threads(0);
    assert_eq!(scan_with(&zero, &control).summary.threads, 1);
}

#[test]
fn the_live_tree_ends_with_the_shallow_totals() {
    let fx = Fixture::new();
    fx.dir("a/b/c/d/e");
    write_file(&fx.path("a/b/c/d/e/f"), 8192);
    fx.dir("z");
    let control = ScanControl::new(64);
    let scanned = scan_with(&options(fx.root()), &control);
    let live = control.live();
    let depths: Vec<u32> = live.iter().map(|d| d.depth).collect();
    assert_eq!(depths.iter().max(), Some(&3));
    assert_eq!(live.len(), 5);
    for dir in &live {
        let record = scanned.records.values().find(|r| r.id == dir.id).unwrap();
        assert_eq!(
            (dir.bytes, dir.files, dir.dirs),
            (record.tree.bytes, record.tree.files, record.tree.dirs)
        );
        assert_eq!(dir.parent, record.parent);
        assert_eq!(dir.name, record.name);
        assert!(dir.done);
    }
}

#[test]
fn the_live_tree_is_bounded_and_grows_while_the_scan_runs() {
    let fx = Fixture::new();
    fx.dir("a/b/c/d");
    write_file(&fx.path("a/f"), 4096);
    let control = Arc::new(ScanControl::new(2));
    let during = Arc::new(Mutex::new(Vec::new()));
    let (hook_control, hook_during) = (Arc::clone(&control), Arc::clone(&during));
    let deepest = fx.path("a/b/c/d");
    let hook = Hook(move |facts: &DirFacts<'_>| {
        if facts.path == deepest {
            *hook_during.lock().unwrap() = hook_control.live();
        }
    });
    let options = options(fx.root()).threads(1).observer(Arc::new(hook));
    scan_with(&options, &control);
    let during = during.lock().unwrap();
    assert_eq!(during.len(), 2);
    assert!(during.iter().all(|d| !d.done));
    assert_eq!(during[0].dirs, 5);
    assert_eq!(during[0].bytes, allocated(&fx.path("a/f")));
    assert_eq!(control.live().len(), 2);
}

#[test]
fn an_excluded_path_is_skipped_and_not_descended() {
    let fx = Fixture::new();
    fx.dir("skipme/inner");
    write_file(&fx.path("skipme/inner/f"), 4096);
    fx.dir("kept");
    let root = ScanRoot::new(fx.root()).unwrap();
    let device = fs::metadata(fx.root()).unwrap().dev();
    let scope = ScanScope::new(device).exclude(fx.path("skipme"));
    let scanned = scan_with(&ScanOptions::new(root, scope), &ScanControl::new(64));
    let skipped = scanned.at(&fx.path("skipme"));
    assert_eq!(skipped.state, DirState::Skipped(Skip::Excluded));
    assert_eq!(scanned.records.len(), 3);
    assert_eq!(scanned.summary.totals.bytes, Bytes(0));
    assert!(scanned.summary.is_complete());
    assert_eq!(scanned.summary.coverage.skips(Skip::Excluded), 1);
}

#[test]
fn directories_on_a_device_outside_the_scope_are_skipped() {
    let fx = Fixture::new();
    fx.dir("a");
    let root = ScanRoot::new(fx.root()).unwrap();
    let device = fs::metadata(fx.root()).unwrap().dev();
    let scope = ScanScope::new(device + 1);
    let scanned = scan_with(&ScanOptions::new(root, scope), &ScanControl::new(64));
    assert_eq!(
        scanned.at(&fx.path("a")).state,
        DirState::Skipped(Skip::OtherVolume)
    );
}

#[test]
fn a_scan_root_must_exist() {
    assert!(ScanRoot::new(Path::new("/nonexistent/metla/root")).is_err());
}

#[test]
fn a_scan_root_is_made_canonical() {
    let fx = Fixture::new();
    fx.dir("real");
    symlink(fx.path("real"), fx.path("alias")).unwrap();
    let root = ScanRoot::new(&fx.path("alias/../alias")).unwrap();
    assert_eq!(root.path(), fx.path("real"));
}

#[test]
fn a_scan_with_the_classic_listing_records_exactly_the_same_tree() {
    let fx = Fixture::new();
    fx.build(&["a/b/c=1", "a/x=22", "d/e/f/g=333", "h/"]);
    write_file(&fx.path("a/big"), 70_000);
    symlink("a", fx.path("link")).unwrap();
    let bulk = scan(fx.root());
    let classic = scan_with(&options(fx.root()).classic(true), &ScanControl::new(64));
    assert_eq!(bulk.records, classic.records);
    assert_eq!(bulk.summary.totals, classic.summary.totals);
}

/// Whether the calling thread runs at background priority.
fn in_background() -> bool {
    unsafe { libc::getpriority(libc::PRIO_DARWIN_THREAD, 0) != 0 }
}

#[test]
fn a_throttled_scan_reads_at_background_priority_and_others_do_not() {
    let fx = Fixture::new();
    fx.build(&["a/b/c=1", "d/"]);
    for throttled in [false, true] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let hook = Hook(move |_: &DirFacts<'_>| record.lock().unwrap().push(in_background()));
        let options = options(fx.root()).threads(2).observer(Arc::new(hook));
        let options = match throttled {
            true => options.priority(metla_core::scan::Priority::Background),
            false => options,
        };
        scan_with(&options, &ScanControl::new(8));
        let seen = seen.lock().unwrap();
        assert!(!seen.is_empty());
        assert!(
            seen.iter().all(|b| *b == throttled),
            "{throttled}: {seen:?}"
        );
    }
    assert!(!in_background(), "the test thread itself is untouched");
}
