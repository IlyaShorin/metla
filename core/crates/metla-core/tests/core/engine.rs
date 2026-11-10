//! The engine on a synthetic root, with changes reported by hand and
//! profiles that settle in milliseconds: opening from a saved picture,
//! applying changes, profiles, exclusions, settings, folders, Git's
//! verdicts, worktree statuses and cleaning.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metla_core::clean::{CleanError, FIXTURE_MARKER, TrashHost};
use metla_core::domain::{
    Bytes, Category, Confirmation, DirId, Exclusion, Outcome, QuickCategory, Separate,
};
use metla_core::engine::{
    Changed, Completeness, DirRef, Engine, EngineConfig, EngineError, EngineObserver, EngineState,
    Failure, FolderEntry, HotkeyMode, LISTED_FILES, Largest, Listed, PowerProfile, Profile,
    Profiles, Removability, Settings, Watching,
};
use metla_core::index::{Order, Page, SortKey};
use metla_core::watch::Timing;

use crate::repo::{git, isolated, pushed_repository, pushed_worktree};
use crate::support::{Fixture, write_file};

const ALL: Page = Page {
    offset: 0,
    limit: 1000,
};

/// Everything the observer was told.
#[derive(Default)]
struct Told(Mutex<Vec<Changed>>);

impl EngineObserver for Told {
    fn changed(&self, what: Changed) {
        self.0.lock().unwrap().push(what);
    }
}

impl Told {
    fn saw(&self, what: Changed) -> bool {
        self.0.lock().unwrap().contains(&what)
    }
}

/// Moves into a directory outside the tree.
pub struct Away(pub PathBuf);

impl TrashHost for Away {
    fn trash(&self, path: &Path) -> Result<(), String> {
        let name = path.file_name().unwrap();
        fs::rename(path, self.0.join(name)).map_err(|e| e.to_string())
    }
}

pub fn quick(full_scans: bool) -> Profile {
    Profile {
        threads: 2,
        latency: Duration::from_millis(20),
        timing: Some(Timing {
            settle: Duration::from_millis(5),
            factor: 2,
            cap: Duration::from_millis(40),
            batch: 64,
            collapse: 16,
            max_dirs: 512,
        }),
        pause: Duration::ZERO,
        full_scans,
        throttled: false,
    }
}

pub fn profiles() -> Profiles {
    Profiles {
        interactive: quick(true),
        background: quick(true),
        saver: Profile {
            threads: 1,
            ..quick(false)
        },
    }
}

/// A tree, the directory for the engine's data and Git, and a Trash.
pub struct Place {
    pub fx: Fixture,
    pub aux: Fixture,
}

impl Place {
    pub fn new(lines: &[&str]) -> Place {
        let fx = Fixture::new();
        fx.build(lines);
        fs::write(fx.path(FIXTURE_MARKER), "").unwrap();
        Place {
            fx,
            aux: Fixture::new(),
        }
    }

    pub fn config(&self, profile: PowerProfile) -> EngineConfig {
        EngineConfig {
            root: self.fx.root().to_path_buf(),
            home: self.fx.home(),
            user: "tester".into(),
            data_dir: self.aux.path("data"),
            git: isolated(&self.aux),
            production: None,
            trash: Arc::new(Away(self.aux.dir("trash"))),
            watching: Watching::Manual,
            profiles: profiles(),
            profile,
            index_limit: None,
        }
    }

    pub fn open(&self) -> Engine {
        Engine::open(self.config(PowerProfile::Background)).unwrap()
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.fx.path(&relative.replacen('~', "home", 1))
    }
}

/// Waits until `done` holds, at most ten seconds.
pub fn until(engine: &Engine, done: impl Fn(&Engine) -> bool) {
    within(engine, Duration::from_secs(10), done);
}

/// Waits until `done` holds, at most `limit`.
pub fn within(engine: &Engine, limit: Duration, done: impl Fn(&Engine) -> bool) {
    let started = Instant::now();
    while !done(engine) {
        assert!(started.elapsed() < limit, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until the engine has nothing to do.
pub fn settled(engine: &Engine) {
    until(engine, |e| {
        let freshness = e.snapshot().unwrap().freshness;
        freshness.state == EngineState::Idle && !freshness.needs_full_scan
    });
}

#[test]
fn a_reported_change_is_pending_at_once() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    engine.changed(vec![place.path("~/Documents")]).unwrap();
    let freshness = engine.snapshot().unwrap().freshness;
    assert!(freshness.pending >= 1 || engine.counters().tasks >= 1);
    assert!(!freshness.is_current() || engine.counters().tasks >= 1);
}

pub fn offered(engine: &Engine, place: &Place) -> BTreeSet<String> {
    let listed = engine.candidates(None, ALL).unwrap();
    let relative = listed.iter().map(|l| {
        let path = l.candidate.path.strip_prefix(place.fx.root()).unwrap();
        path.display().to_string()
    });
    relative.collect()
}

pub const HOME: &[&str] = &[
    "~/Library/Caches/com.example.app/blob=cached",
    "~/Library/Logs/com.example.app/today.log=log",
    "~/Documents/notes.txt=notes",
];

#[test]
fn a_first_opening_scans_and_a_later_one_answers_from_the_saved_picture() {
    let place = Place::new(HOME);
    let engine = place.open();
    let told = Arc::new(Told::default());
    engine.set_observer(told.clone());
    settled(&engine);
    let first = engine.snapshot().unwrap();
    assert_eq!(first.freshness.picture, Completeness::Complete);
    assert!(first.freshness.is_current(), "{:?}", first.freshness);
    assert!(first.totals.unwrap().files >= 3);
    assert_eq!(first.categories[&Category::AppCache].count, 1);
    assert_eq!(first.root, place.fx.root());
    assert_eq!(
        engine.freshness().unwrap(),
        first.freshness,
        "the cheap read agrees"
    );
    assert!(first.volume.is_some());
    assert_eq!(engine.counters().full_scans, 1);
    assert!(told.saw(Changed::Picture) && told.saw(Changed::Activity));
    drop(engine);
    let again = place.open();
    let reopened = again.snapshot().unwrap();
    assert_eq!(reopened.totals, first.totals);
    assert_eq!(reopened.freshness.full_scan, first.freshness.full_scan);
    settled(&again);
    assert_eq!(again.counters().full_scans, 0, "nothing to scan again");
}

#[test]
fn a_reported_change_is_applied_once_it_settles() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    let caches = place.path("~/Library/Caches");
    place
        .fx
        .build(&["~/Library/Caches/org.other.tool/data=more"]);
    engine.changed(vec![caches.clone()]).unwrap();
    settled(&engine);
    assert!(offered(&engine, &place).contains("home/Library/Caches/org.other.tool"));
    let after = engine.snapshot().unwrap();
    assert!(after.freshness.updated >= after.freshness.full_scan);
    assert_eq!(after.categories[&Category::AppCache].count, 2);
    fs::remove_dir_all(caches.join("org.other.tool")).unwrap();
    engine.changed(vec![caches]).unwrap();
    settled(&engine);
    assert!(!offered(&engine, &place).contains("home/Library/Caches/org.other.tool"));
    let counters = engine.counters();
    assert_eq!(counters.full_scans, 1);
    assert!(counters.tasks >= 2);
    assert!(counters.wakeups >= 2, "a turn for each report");
}

#[test]
fn a_storm_of_changes_ends_in_few_rescans() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    let busy = place.fx.dir("busy");
    let before = engine.counters().tasks;
    let started = Instant::now();
    for i in 0..200 {
        write_file(&busy.join(format!("f{}", i % 20)), 10);
        engine.changed(vec![busy.clone()]).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    let storm = started.elapsed();
    settled(&engine);
    let tasks = engine.counters().tasks - before;
    let per_cap = (storm.as_millis() / 40) as u64 + 3;
    assert!(tasks <= per_cap, "{tasks} rescans in {storm:?}");
    assert!(engine.counters().events >= 200);
}

#[test]
fn off_stops_watching_and_applies_nothing_until_it_is_on_again_without_rescanning() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    engine.set_power_profile(PowerProfile::Off).unwrap();
    until(&engine, |e| !e.snapshot().unwrap().freshness.watching);
    let overview = engine.snapshot().unwrap();
    assert_eq!(overview.freshness.state, EngineState::Off);
    assert_eq!(overview.profile, PowerProfile::Off);
    assert!(!overview.freshness.is_current());
    let tasks = engine.counters().tasks;
    engine.changed(vec![place.path("~/Documents")]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(engine.counters().tasks, tasks);
    engine.set_power_profile(PowerProfile::Background).unwrap();
    until(&engine, |e| e.snapshot().unwrap().freshness.watching);
    settled(&engine);
    assert_eq!(engine.counters().full_scans, 1, "nothing was lost");
}

#[test]
fn saver_defers_the_full_scan_the_engine_needs_until_the_profile_allows_it() {
    let place = Place::new(HOME);
    let engine = Engine::open(place.config(PowerProfile::Saver)).unwrap();
    until(&engine, |e| {
        e.snapshot().unwrap().freshness.state == EngineState::Paused(PowerProfile::Saver)
    });
    let overview = engine.snapshot().unwrap();
    assert_eq!(overview.freshness.picture, Completeness::Missing);
    assert!(overview.freshness.needs_full_scan);
    assert_eq!(engine.counters().full_scans, 0);
    engine.set_power_profile(PowerProfile::Background).unwrap();
    settled(&engine);
    assert_eq!(engine.counters().full_scans, 1);
}

#[test]
fn a_scan_the_user_asks_for_runs_even_when_background_analysis_is_off() {
    let place = Place::new(HOME);
    let engine = Engine::open(place.config(PowerProfile::Off)).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.counters().full_scans, 0);
    assert_eq!(engine.snapshot().unwrap().freshness.state, EngineState::Off);
    engine.start_scan().unwrap();
    until(&engine, |e| e.counters().full_scans == 1);
    let overview = engine.snapshot().unwrap();
    assert_eq!(overview.freshness.picture, Completeness::Complete);
    assert!(!overview.freshness.watching);
    assert_eq!(engine.progress(), None, "no scan runs any more");
}

#[test]
fn exclusions_hide_candidates_at_once_and_are_kept() {
    let place = Place::new(HOME);
    let engine = place.open();
    let told = Arc::new(Told::default());
    engine.set_observer(told.clone());
    settled(&engine);
    let cache = Exclusion::new(&place.path("~/Library/Caches")).unwrap();
    assert!(engine.add_exclusion(&cache).unwrap());
    assert!(!engine.add_exclusion(&cache).unwrap());
    assert!(told.saw(Changed::Exclusions));
    assert!(!offered(&engine, &place).contains("home/Library/Caches/com.example.app"));
    assert_eq!(engine.exclusions().unwrap(), std::slice::from_ref(&cache));
    assert!(engine.remove_exclusion(&cache).unwrap());
    assert!(!engine.remove_exclusion(&cache).unwrap());
    assert!(offered(&engine, &place).contains("home/Library/Caches/com.example.app"));
}

#[test]
fn the_power_profiles_are_the_documented_table() {
    let table = Profiles::standard(12);
    let rows = [
        (table.interactive, 8, 1, 1, 30, 256, 4096, 0, true, false),
        (
            table.background,
            2,
            15,
            30,
            300,
            1024,
            4096,
            60_000,
            true,
            false,
        ),
        (table.saver, 1, 15, 60, 900, 256, 1024, 180_000, false, true),
    ];
    for (profile, threads, latency, settle, cap, batch, max_dirs, pause, full, throttled) in rows {
        let timing = profile.timing.unwrap();
        assert_eq!(profile.threads, threads);
        assert_eq!(profile.latency, Duration::from_secs(latency));
        assert_eq!(timing.settle, Duration::from_secs(settle));
        assert_eq!((timing.factor, timing.collapse), (2, 32));
        assert_eq!(timing.cap, Duration::from_secs(cap));
        assert_eq!((timing.batch, timing.max_dirs), (batch, max_dirs));
        assert_eq!(profile.pause, Duration::from_millis(pause));
        assert_eq!((profile.full_scans, profile.throttled), (full, throttled));
    }
    assert_eq!(Profiles::standard(2).interactive.threads, 1, "never none");
    assert_eq!(Profiles::standard(9).interactive.threads, 7, "cores - 2");
    assert_eq!(Profiles::standard(10).interactive.threads, 8);
    let off = table.of(PowerProfile::Off);
    assert_eq!((off.threads, off.timing, off.full_scans), (0, None, false));
    assert_eq!(table.of(PowerProfile::Saver), table.saver);
    assert_eq!(table.of(PowerProfile::Interactive), table.interactive);
    assert_eq!(PowerProfile::default(), PowerProfile::Background);
    use metla_core::scan::Priority;
    assert_eq!(table.saver.priority(), Priority::Background);
    assert_eq!(table.background.priority(), Priority::Normal);
}

#[test]
fn engine_errors_explain_themselves() {
    let cases = [
        (EngineError::Setup("no root".into()), "no root"),
        (EngineError::NotInPicture, "not in the picture"),
        (EngineError::Stopped, "the engine has stopped"),
        (
            EngineError::Clean(CleanError::Root("gone".into())),
            "root: gone",
        ),
        (
            EngineError::Index(metla_core::index::IndexError::WriterGone),
            "index writer stopped",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn settings_are_kept_and_switching_background_analysis_off_stops_watching() {
    let defaults = Settings::default();
    assert!(defaults.background && defaults.reduce_on_battery);
    assert_eq!(defaults.hotkey, HotkeyMode::Confirm);
    assert!(defaults.quick.is_empty());
    assert_eq!(defaults.recent_age, Duration::from_secs(14 * 24 * 3600));
    assert_eq!(defaults.stale_after, Duration::from_secs(30 * 24 * 3600));
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    assert_eq!(engine.settings(), Settings::default());
    let chosen = Settings {
        recent_age: Duration::from_secs(3600),
        stale_after: Duration::from_secs(7200),
        hotkey: HotkeyMode::Immediate,
        quick: [QuickCategory::DevCache, QuickCategory::AppCache].into(),
        reduce_on_battery: false,
        ..Settings::default()
    };
    engine.set_settings(chosen.clone()).unwrap();
    engine.set_background(false).unwrap();
    until(&engine, |e| !e.snapshot().unwrap().freshness.watching);
    assert_eq!(engine.snapshot().unwrap().profile, PowerProfile::Off);
    drop(engine);
    let again = place.open();
    let kept = Settings {
        background: false,
        ..chosen
    };
    assert_eq!(again.settings(), kept);
    assert_eq!(Settings::decode(&kept.encode()), kept);
    assert_eq!(
        Settings::decode("garbage\nrecent_age=x\nhotkey=?"),
        Settings::default()
    );
}

#[test]
fn counters_since_earlier_ones_are_what_was_done_in_between() {
    let earlier = metla_core::engine::Counters {
        full_scans: 1,
        tasks: 7,
        batches: 3,
        events: 40,
        wakeups: 9,
        streams: 1,
    };
    let later = metla_core::engine::Counters {
        full_scans: 2,
        tasks: 17,
        batches: 8,
        events: 140,
        wakeups: 30,
        streams: 4,
    };
    let done = later.since(&earlier);
    let fields = [
        done.full_scans,
        done.tasks,
        done.batches,
        done.events,
        done.wakeups,
        done.streams,
    ];
    assert_eq!(fields, [1, 10, 5, 100, 21, 3]);
    assert_eq!(earlier.since(&later), Default::default(), "never negative");
}

#[test]
fn a_folder_lists_its_subdirectories_and_files_largest_first_then_by_name() {
    let place = Place::new(&["top/small/x=1", "top/big/y=2"]);
    write_file(&place.path("top/big/z"), 50_000);
    write_file(&place.path("top/file"), 20_000);
    write_file(&place.path("top/twin-b"), 8_000);
    write_file(&place.path("top/twin-a"), 8_000);
    let engine = place.open();
    settled(&engine);
    let folder = engine
        .children(&DirRef::Path(place.path("top")), ALL)
        .unwrap();
    let names: Vec<String> = folder
        .entries
        .iter()
        .map(|e| match e {
            FolderEntry::Dir(row) => format!("{}/", row.name.to_string_lossy()),
            FolderEntry::File { name, .. } => name.to_string_lossy().into_owned(),
            FolderEntry::MoreFiles { .. } => "…".into(),
        })
        .collect();
    assert_eq!(names, ["big/", "file", "twin-a", "twin-b", "small/"]);
    assert_eq!(folder.total, 5);
    let by_id = engine.children(
        &DirRef::Id(folder.dir.id),
        Page {
            offset: 1,
            limit: 1,
        },
    );
    let by_id = by_id.unwrap();
    assert_eq!(by_id.path, place.path("top"));
    assert_eq!(by_id.entries.len(), 1);
    let missing = engine.children(&DirRef::Path(place.path("nowhere")), ALL);
    assert!(matches!(missing, Err(EngineError::NotInPicture)));
}

#[test]
fn a_folder_lists_at_most_ten_thousand_files_and_counts_the_rest() {
    let place = Place::new(&["many/sub/x=1"]);
    let many = place.path("many");
    for i in 0..LISTED_FILES {
        fs::write(many.join(format!("f{i:05}")), "x").unwrap();
    }
    let engine = place.open();
    settled(&engine);
    let list = |engine: &Engine| {
        let everything = Page {
            offset: 0,
            limit: 2 * LISTED_FILES,
        };
        engine
            .children(&DirRef::Path(many.clone()), everything)
            .unwrap()
    };
    let folder = list(&engine);
    assert_eq!(
        folder.total,
        LISTED_FILES + 1,
        "every file and the directory"
    );
    assert!(
        !folder
            .entries
            .iter()
            .any(|e| matches!(e, FolderEntry::MoreFiles { .. }))
    );
    write_file(&many.join("f99999"), 1);
    let folder = list(&engine);
    assert_eq!(folder.total, LISTED_FILES + 2);
    let more = folder.entries.last().unwrap();
    let FolderEntry::MoreFiles { files, bytes } = more else {
        panic!("{more:?}");
    };
    assert_eq!(*files, 1, "the name that sorts last of the smallest");
    assert!(bytes.0 > 0);
    let kept = folder
        .entries
        .iter()
        .filter(|e| matches!(e, FolderEntry::File { .. }));
    assert_eq!(kept.count(), LISTED_FILES);
}

#[test]
fn a_folder_page_needs_only_the_largest_subdirectories_up_to_its_end() {
    let lines: Vec<String> = (0..30).map(|i| format!("wide/d{i:02}/x=1")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let place = Place::new(&lines);
    write_file(&place.path("wide/d07/big"), 40_000);
    let engine = place.open();
    settled(&engine);
    let page = Page {
        offset: 2,
        limit: 3,
    };
    let folder = engine
        .children(&DirRef::Path(place.path("wide")), page)
        .unwrap();
    assert_eq!(folder.total, 30);
    let names: Vec<String> = folder
        .entries
        .iter()
        .map(|e| match e {
            FolderEntry::Dir(row) => row.name.to_string_lossy().into_owned(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(names, ["d01", "d02", "d03"], "after d07 and d00");
}

#[test]
fn the_largest_files_are_kept_and_the_rest_counted() {
    let mut largest = Largest::new(3);
    for (name, bytes) in [("a", 5), ("b", 9), ("c", 1), ("d", 5), ("e", 7), ("f", 5)] {
        largest.add(name.into(), Bytes(bytes));
    }
    let (kept, rest) = largest.finish();
    let names: Vec<&str> = kept.iter().map(|(n, _)| n.to_str().unwrap()).collect();
    assert_eq!(
        names,
        ["b", "e", "a"],
        "largest first; of equals, the first name"
    );
    assert_eq!((rest.files, rest.bytes), (3, Bytes(11)));
    let mut exact = Largest::new(2);
    exact.add("x".into(), Bytes(1));
    exact.add("y".into(), Bytes(2));
    let (kept, rest) = exact.finish();
    assert_eq!(kept.len(), 2);
    assert_eq!((rest.files, rest.bytes), (0, Bytes(0)));
    let mut none = Largest::new(0);
    none.add("z".into(), Bytes(4));
    let (kept, rest) = none.finish();
    assert!(kept.is_empty());
    assert_eq!((rest.files, rest.bytes), (1, Bytes(4)));
}

/// Three caches of 8, 24 and 16 KB, active 30, 10 and 20 days ago.
fn three_caches() -> Place {
    let place = Place::new(&[]);
    let now = std::time::SystemTime::now();
    for (name, size, days) in [
        ("org.a", 8_000, 30),
        ("org.b", 24_000, 10),
        ("org.c", 16_000, 20),
    ] {
        let file = place.path(&format!("~/Library/Caches/{name}/blob"));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        write_file(&file, size);
        let at = now - Duration::from_secs(days * 86_400);
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(at)
            .unwrap();
        let dir = fs::File::open(file.parent().unwrap()).unwrap();
        dir.set_modified(at).unwrap();
    }
    place
}

#[test]
fn a_page_of_candidates_comes_in_the_order_asked_for_with_the_total() {
    let place = three_caches();
    let engine = place.open();
    settled(&engine);
    let names = |key: SortKey, descending: bool, page: Page| {
        let order = Order { key, descending };
        let found = engine
            .candidate_page(Some(Category::AppCache), order, page)
            .unwrap();
        assert_eq!(found.total, 3);
        let names = found
            .listed
            .iter()
            .map(|l| l.candidate.path.file_name().unwrap().to_owned());
        names
            .map(|n| n.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(SortKey::Size, true, ALL), ["org.b", "org.c", "org.a"]);
    assert_eq!(
        names(SortKey::Size, false, ALL),
        ["org.a", "org.c", "org.b"]
    );
    assert_eq!(
        names(SortKey::Activity, true, ALL),
        ["org.b", "org.c", "org.a"]
    );
    assert_eq!(
        names(SortKey::Activity, false, ALL),
        ["org.a", "org.c", "org.b"]
    );
    assert_eq!(
        names(SortKey::Path, false, ALL),
        ["org.a", "org.b", "org.c"]
    );
    assert_eq!(names(SortKey::Path, true, ALL), ["org.c", "org.b", "org.a"]);
    let second = Page {
        offset: 1,
        limit: 1,
    };
    assert_eq!(names(SortKey::Path, false, second), ["org.b"]);
    assert_eq!(
        Order::default(),
        Order {
            key: SortKey::Size,
            descending: true
        }
    );
}

#[test]
fn the_largest_folders_are_the_largest_that_hold_none_of_the_others() {
    let place = Place::new(&["a/b/c/x=1", "a/d/y=1", "e/z=1"]);
    write_file(&place.path("a/b/c/big"), 90_000);
    write_file(&place.path("a/d/mid"), 50_000);
    write_file(&place.path("e/small"), 20_000);
    let engine = place.open();
    settled(&engine);
    let tops = engine.largest_folders(4).unwrap();
    let paths: Vec<PathBuf> = tops.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        paths,
        [place.path("a/b/c"), place.path("a/d"), place.path("e")],
        "not a or a/b, which hold a/b/c"
    );
    assert!(tops.iter().all(|f| !f.counting));
    assert!(tops[0].bytes.0 >= 90_000 && tops[0].files == 2);
    assert_eq!(engine.largest_folders(1).unwrap().len(), 1);
}

#[test]
fn during_the_first_scan_the_largest_folders_come_from_the_live_tree() {
    let place = Place::new(HOME);
    wide(&place);
    let engine = Engine::open(slow(&place)).unwrap();
    let started = Instant::now();
    let mut seen = false;
    while !seen && started.elapsed() < Duration::from_secs(10) {
        let before = engine.progress().is_some();
        let tops = engine.largest_folders(3).unwrap();
        let after = engine.progress().is_some();
        if before && after && !tops.is_empty() {
            assert!(tops.iter().all(|f| f.counting), "{tops:?}");
            seen = true;
        }
    }
    assert!(seen, "never caught the first scan running");
}

#[test]
fn the_engine_asks_git_about_project_output_after_a_scan_and_after_an_update() {
    let place = Place::new(&["code/app/Cargo.toml", "code/app/target/debug/out=binary"]);
    let git_ = isolated(&place.aux);
    git(
        &git_,
        &place.path("code/app"),
        &["init", "-q", "-b", "main"],
    );
    fs::write(place.path("code/app/.gitignore"), "target/\n").unwrap();
    let engine = place.open();
    settled(&engine);
    assert!(offered(&engine, &place).contains("code/app/target"));
    place
        .fx
        .build(&["code/app/web/Cargo.toml", "code/app/web/target/x=1"]);
    fs::write(place.path("code/app/.gitignore"), "target/\n").unwrap();
    engine.changed(vec![place.path("code/app")]).unwrap();
    settled(&engine);
    until(&engine, |e| {
        offered(e, &place).contains("code/app/web/target")
    });
}

#[test]
fn worktree_statuses_come_with_the_page_once_they_are_computed() {
    let place = Place::new(&[]);
    let git_ = isolated(&place.aux);
    let repo = pushed_repository(&place.fx, &git_);
    pushed_worktree(&git_, &repo, &place.path("wt"), "feature");
    fs::write(place.path("wt/untracked.txt"), "work").unwrap();
    let engine = place.open();
    let told = Arc::new(Told::default());
    engine.set_observer(told.clone());
    settled(&engine);
    let blocked = |e: &Engine| {
        let listed = e.candidates(Some(Category::GitWorktree), ALL).unwrap();
        matches!(
            listed.as_slice(),
            [Listed {
                removability: Some(Removability::Blocked(_)),
                ..
            }]
        )
    };
    until(&engine, blocked);
    assert!(told.saw(Changed::Worktrees));
    let others = engine.candidates(Some(Category::AppCache), ALL).unwrap();
    assert!(others.iter().all(|l| l.removability.is_none()));
}

#[test]
fn a_cleanup_runs_on_the_engine_and_the_picture_follows_it() {
    let place = Place::new(HOME);
    let engine = place.open();
    let told = Arc::new(Told::default());
    engine.set_observer(told.clone());
    settled(&engine);
    let before = engine.snapshot().unwrap().totals.unwrap();
    let listed = engine.candidates(None, ALL).unwrap();
    let totals = engine.category_totals().unwrap();
    let counts: Vec<u64> = Category::ALL.iter().map(|c| totals[c].count).collect();
    assert_eq!(counts, [1, 0, 0, 1, 0, 0], "an app cache and a log");
    let ids: Vec<DirId> = listed.iter().map(|l| l.candidate.id).collect();
    let preview = engine.preview(&ids).unwrap();
    assert_eq!(preview.items.len(), 2);
    let confirmation = Confirmation {
        groups: [Separate::MoveToTrash].into(),
        items: Default::default(),
    };
    engine.execute_confirmed(preview.id, confirmation).unwrap();
    until(&engine, |e| e.last_report().is_some());
    settled(&engine);
    let report = engine.last_report().unwrap().unwrap();
    let outcomes: Vec<Outcome> = report.items.iter().map(|i| i.outcome.clone()).collect();
    assert!(outcomes.contains(&Outcome::Removed));
    assert!(outcomes.contains(&Outcome::MovedToTrash));
    assert!(told.saw(Changed::Cleanup));
    assert_eq!(engine.cleanup_progress(), None);
    let after = engine.snapshot().unwrap();
    assert!(
        after.totals.unwrap().bytes < before.bytes,
        "{before:?} {after:?}"
    );
    assert!(offered(&engine, &place).contains("home/Library/Caches/com.example.app"));
    assert_eq!(after.categories[&Category::Log].count, 0, "moved away");
    let again = engine.execute_confirmed(preview.id, Confirmation::default());
    assert!(again.is_ok());
    until(&engine, |e| {
        matches!(e.last_report(), Some(Err(CleanError::PreviewUsed(_))))
    });
}

#[test]
fn a_quick_clean_takes_only_the_quick_categories_it_is_given() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    engine.quick_clean(vec![QuickCategory::AppCache]).unwrap();
    until(&engine, |e| e.last_report().is_some());
    let report = engine.last_report().unwrap().unwrap();
    let cleaned: Vec<&Path> = report.items.iter().map(|i| i.path.as_path()).collect();
    assert_eq!(cleaned, [place.path("~/Library/Caches/com.example.app")]);
    assert!(
        place
            .path("~/Library/Logs/com.example.app/today.log")
            .exists()
    );
}

#[test]
fn a_root_without_the_fixture_marker_cannot_be_cleaned() {
    let place = Place::new(HOME);
    fs::remove_file(place.fx.path(FIXTURE_MARKER)).unwrap();
    let engine = place.open();
    settled(&engine);
    let ids: Vec<DirId> = engine
        .candidates(None, ALL)
        .unwrap()
        .iter()
        .map(|l| l.candidate.id)
        .collect();
    let refused = engine.preview(&ids);
    assert!(matches!(
        refused,
        Err(EngineError::Clean(CleanError::NotAFixture(_)))
    ));
    engine.quick_clean(vec![QuickCategory::AppCache]).unwrap();
    until(&engine, |e| e.last_report().is_some());
    assert!(matches!(
        engine.last_report(),
        Some(Err(CleanError::NotAFixture(_)))
    ));
    assert!(place.path("~/Library/Caches/com.example.app/blob").exists());
}

#[test]
fn an_index_of_another_schema_is_replaced_by_a_new_scan() {
    let place = Place::new(HOME);
    let data = place.aux.dir("data");
    let old = rusqlite::Connection::open(data.join("index.sqlite")).unwrap();
    old.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value) WITHOUT ROWID;
         INSERT INTO meta VALUES ('schema_version', 3);",
    )
    .unwrap();
    drop(old);
    let engine = place.open();
    settled(&engine);
    assert_eq!(engine.counters().full_scans, 1);
    let foreign = Place::new(HOME);
    let data = foreign.aux.dir("data");
    let other = rusqlite::Connection::open(data.join("index.sqlite")).unwrap();
    other.execute_batch("CREATE TABLE notes (x);").unwrap();
    drop(other);
    let refused = Engine::open(foreign.config(PowerProfile::Background));
    assert!(matches!(refused, Err(EngineError::Index(_))));
    assert!(
        data.join("index.sqlite").exists(),
        "a file that is not an index is left alone"
    );
}

#[test]
fn a_picture_whose_last_scan_was_interrupted_is_scanned_again() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    drop(engine);
    let index = place.aux.path("data/index.sqlite");
    let mut writer = metla_core::index::IndexWriter::open(&index).unwrap();
    let interrupted = writer.start_scan(metla_core::domain::UnixTime(1)).unwrap();
    interrupted.send(Vec::new());
    drop(writer);
    let again = place.open();
    assert!(again.snapshot().unwrap().freshness.needs_full_scan);
    settled(&again);
    assert_eq!(again.counters().full_scans, 1);
}

#[test]
fn after_a_cancel_no_full_scan_starts_until_the_user_asks() {
    let place = Place::new(HOME);
    let engine = Engine::open(place.config(PowerProfile::Saver)).unwrap();
    until(&engine, |e| {
        e.snapshot().unwrap().freshness.state == EngineState::Paused(PowerProfile::Saver)
    });
    engine.cancel_scan();
    engine.set_power_profile(PowerProfile::Background).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(engine.counters().full_scans, 0, "held after the cancel");
    assert!(engine.snapshot().unwrap().freshness.needs_full_scan);
    engine.start_scan().unwrap();
    settled(&engine);
    assert_eq!(engine.counters().full_scans, 1);
}

/// A tree a single thread takes a while to walk.
fn wide(place: &Place) {
    for i in 0..2000 {
        fs::create_dir_all(place.fx.path(&format!("wide/{}/{i}", i % 40))).unwrap();
    }
}

fn slow(place: &Place) -> EngineConfig {
    let mut config = place.config(PowerProfile::Background);
    config.profiles.background.threads = 1;
    config
}

#[test]
fn switching_off_cancels_the_first_background_scan() {
    let place = Place::new(HOME);
    wide(&place);
    let engine = Engine::open(slow(&place)).unwrap();
    until(&engine, |e| e.progress().is_some());
    engine.set_power_profile(PowerProfile::Off).unwrap();
    until(&engine, |e| {
        let state = e.snapshot().unwrap().freshness.state;
        state == EngineState::Off && e.progress().is_none()
    });
    let freshness = engine.snapshot().unwrap().freshness;
    assert_ne!(freshness.picture, Completeness::Complete);
    assert!(freshness.needs_full_scan);
}

#[test]
fn switching_background_analysis_off_cancels_the_first_background_scan() {
    let place = Place::new(HOME);
    wide(&place);
    let engine = Engine::open(slow(&place)).unwrap();
    until(&engine, |e| e.progress().is_some());
    engine.set_background(false).unwrap();
    until(&engine, |e| {
        let state = e.snapshot().unwrap().freshness.state;
        state == EngineState::Off && e.progress().is_none()
    });
    let freshness = engine.snapshot().unwrap().freshness;
    assert_ne!(freshness.picture, Completeness::Complete);
}

/// The engine's index file.
fn index_of(place: &Place) -> PathBuf {
    place.aux.path("data").join(metla_core::engine::INDEX_FILE)
}

#[test]
fn a_scan_that_runs_out_of_disk_space_fails_and_the_picture_stays() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    let before = engine.snapshot().unwrap();
    drop(engine);
    wide(&place);
    let size = fs::metadata(index_of(&place)).unwrap().len();
    let limited = EngineConfig {
        index_limit: Some(Bytes(size + 32 * 1024)),
        ..place.config(PowerProfile::Background)
    };
    let engine = Engine::open(limited).unwrap();
    engine.start_scan().unwrap();
    let failed = EngineState::Failed(Failure::NoSpace);
    until(&engine, |e| e.freshness().unwrap().state == failed);
    let after = engine.snapshot().unwrap();
    assert_eq!(after.totals, before.totals, "the old picture");
    assert_eq!(after.freshness.picture, Completeness::Complete);
    engine.changed(vec![place.path("~/Documents")]).unwrap();
    until(&engine, |e| e.counters().tasks == 1);
    assert_eq!(
        engine.freshness().unwrap().state,
        failed,
        "until the user asks"
    );
    drop(engine);
    let engine = place.open();
    engine.start_scan().unwrap();
    until(&engine, |e| e.counters().full_scans == 1);
    settled(&engine);
    assert!(engine.snapshot().unwrap().totals.unwrap().dirs > before.totals.unwrap().dirs);
}

#[test]
fn a_first_scan_without_disk_space_fails_once_and_waits_for_the_user() {
    let place = Place::new(HOME);
    wide(&place);
    let limited = EngineConfig {
        index_limit: Some(Bytes(160 * 1024)),
        ..place.config(PowerProfile::Background)
    };
    let engine = Engine::open(limited).unwrap();
    let failed = EngineState::Failed(Failure::NoSpace);
    until(&engine, |e| e.freshness().unwrap().state == failed);
    let freshness = engine.freshness().unwrap();
    assert_eq!(freshness.picture, Completeness::Missing);
    assert!(freshness.needs_full_scan);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(engine.counters().full_scans, 1, "no retry on its own");
    engine.start_scan().unwrap();
    until(&engine, |e| e.counters().full_scans == 2);
    until(&engine, |e| e.freshness().unwrap().state == failed);
}

/// Profiles whose settle time is `settle`, with `pause` between batches/// Profiles whose settle time is `settle`, with `pause` between batches
/// of one task.
fn patient(place: &Place, settle: u64, pause: u64) -> EngineConfig {
    let mut config = place.config(PowerProfile::Background);
    let timing = config.profiles.background.timing.as_mut().unwrap();
    timing.settle = Duration::from_millis(settle);
    timing.cap = Duration::from_millis(settle * 8);
    timing.batch = 1;
    config.profiles.background.pause = Duration::from_millis(pause);
    config
}

#[test]
fn attention_applies_what_waits_at_once_and_follows_changes_interactively() {
    let place = Place::new(HOME);
    let engine = Engine::open(patient(&place, 60_000, 60_000)).unwrap();
    settled(&engine);
    engine.changed(vec![place.path("~/Documents")]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(engine.counters().tasks, 0, "Background lets it settle");
    engine.set_attention(true).unwrap();
    until(&engine, |e| e.counters().tasks == 1);
    settled(&engine);
    assert!(engine.freshness().unwrap().is_current());
    engine.changed(vec![place.path("~/Library/Logs")]).unwrap();
    until(&engine, |e| e.counters().tasks == 2);
    engine.set_attention(false).unwrap();
    engine
        .changed(vec![place.path("~/Library/Caches")])
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(engine.counters().tasks, 2, "Background again");
    assert_eq!(
        engine.freshness().unwrap().state,
        EngineState::Updating { pending: 1 },
        "a deferred change never looks current"
    );
}

#[test]
fn attention_does_not_watch_what_background_analysis_leaves_alone() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    let woken = engine.counters().wakeups;
    engine.set_background(false).unwrap();
    engine.set_attention(true).unwrap();
    until(&engine, |e| e.counters().wakeups >= woken + 2);
    let freshness = engine.freshness().unwrap();
    assert_eq!(freshness.state, EngineState::Off);
    assert!(
        !freshness.watching,
        "both commands handled, nothing watched"
    );
}

#[test]
fn an_idle_engine_sleeps_until_its_next_deadline() {
    let place = Place::new(HOME);
    let engine = Engine::open(patient(&place, 400, 0)).unwrap();
    settled(&engine);
    let idle = engine.counters().wakeups;
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(engine.counters().wakeups, idle, "nothing to do: no wake-up");
    engine.changed(vec![place.path("~/Documents")]).unwrap();
    let freshness = engine.snapshot().unwrap().freshness;
    assert_eq!(freshness.state, EngineState::Updating { pending: 1 });
    std::thread::sleep(Duration::from_millis(200));
    let waiting = engine.counters().wakeups - idle;
    assert!(waiting <= 2, "{waiting} wake-ups while a change settles");
    settled(&engine);
    assert_eq!(engine.counters().tasks, 1);
}

#[test]
fn batches_are_apart_by_the_profile_pause() {
    let place = Place::new(HOME);
    let engine = Engine::open(patient(&place, 5, 250)).unwrap();
    settled(&engine);
    let dirs = vec![place.path("~/Documents"), place.path("~/Library/Logs")];
    engine.changed(dirs).unwrap();
    until(&engine, |e| e.counters().tasks >= 1);
    let first = Instant::now();
    until(&engine, |e| e.counters().tasks >= 2);
    assert!(
        first.elapsed() >= Duration::from_millis(150),
        "{:?}",
        first.elapsed()
    );
    assert_eq!(engine.counters().batches, 2);
}

#[test]
fn many_changed_children_of_the_root_are_one_full_scan() {
    let place = Place::new(HOME);
    let engine = place.open();
    settled(&engine);
    let children: Vec<PathBuf> = (0..17).map(|i| place.path(&format!("c{i}"))).collect();
    engine.changed(children).unwrap();
    until(&engine, |e| e.counters().full_scans == 2);
    settled(&engine);
    assert_eq!(engine.counters().tasks, 1, "one task: the full scan");
}

#[test]
fn after_a_cancel_the_engine_holds_and_the_user_lifts_the_hold() {
    let place = Place::new(HOME);
    let engine = Engine::open(place.config(PowerProfile::Saver)).unwrap();
    until(&engine, |e| {
        e.snapshot().unwrap().freshness.state == EngineState::Paused(PowerProfile::Saver)
    });
    engine.cancel_scan();
    engine.set_power_profile(PowerProfile::Background).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let held = engine.snapshot().unwrap().freshness;
    assert_eq!(held.state, EngineState::Idle, "held, nothing runs");
    assert!(held.needs_full_scan);
    assert_eq!(engine.counters().full_scans, 0);
    engine.start_scan().unwrap();
    settled(&engine);
    let children: Vec<PathBuf> = (0..17).map(|i| place.path(&format!("c{i}"))).collect();
    engine.changed(children).unwrap();
    until(&engine, |e| e.counters().full_scans == 2);
}

#[test]
fn the_engine_says_whose_scan_runs_and_off_never_stops_the_users() {
    let place = Place::new(HOME);
    wide(&place);
    let engine = Engine::open(slow(&place)).unwrap();
    let interactive = |e: &Engine| match e.snapshot().unwrap().freshness.state {
        EngineState::Scanning { interactive, .. } => Some(interactive),
        _ => None,
    };
    until(&engine, |e| interactive(e).is_some());
    assert_eq!(interactive(&engine), Some(false), "the engine's own");
    settled(&engine);
    engine.start_scan().unwrap();
    until(&engine, |e| interactive(e).is_some());
    assert_eq!(interactive(&engine), Some(true));
    engine.set_power_profile(PowerProfile::Off).unwrap();
    until(&engine, |e| e.progress().is_none());
    assert_eq!(engine.counters().full_scans, 2);
    let freshness = engine.snapshot().unwrap().freshness;
    assert_eq!(freshness.picture, Completeness::Complete, "not cancelled");
}

#[test]
fn a_user_scan_replaces_the_picture_and_a_cancelled_one_keeps_it() {
    let place = Place::new(HOME);
    wide(&place);
    let engine = Engine::open(slow(&place)).unwrap();
    settled(&engine);
    place.fx.build(&["~/Library/Caches/org.unreported/x=1"]);
    engine.start_scan().unwrap();
    until(&engine, |e| e.counters().full_scans == 2);
    settled(&engine);
    assert!(offered(&engine, &place).contains("home/Library/Caches/org.unreported"));
    engine.start_scan().unwrap();
    until(&engine, |e| e.progress().is_some());
    engine.cancel_scan();
    until(&engine, |e| e.progress().is_none());
    let freshness = engine.snapshot().unwrap().freshness;
    assert_eq!(freshness.picture, Completeness::Complete);
    assert!(!freshness.needs_full_scan, "the old picture is still right");
    assert!(offered(&engine, &place).contains("home/Library/Caches/org.unreported"));
}

#[test]
fn switching_off_and_on_again_resumes_the_scan_the_engine_needs() {
    let place = Place::new(HOME);
    wide(&place);
    let engine = Engine::open(slow(&place)).unwrap();
    engine.set_power_profile(PowerProfile::Off).unwrap();
    until(&engine, |e| e.progress().is_none());
    engine.set_power_profile(PowerProfile::Background).unwrap();
    settled(&engine);
    let freshness = engine.snapshot().unwrap().freshness;
    assert_eq!(freshness.picture, Completeness::Complete);
}

#[test]
fn a_cleanup_shows_as_cleaning_while_it_runs() {
    let place = Place::new(HOME);
    let cache = place.fx.dir("home/Library/Caches/com.example.big");
    for i in 0..1500 {
        fs::write(cache.join(format!("f{i}")), "x").unwrap();
    }
    let engine = place.open();
    settled(&engine);
    let seen = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let started = Instant::now();
            while engine.last_report().is_none() {
                assert!(started.elapsed() < Duration::from_secs(10), "no report");
                let state = engine.snapshot().unwrap().freshness.state;
                if state == EngineState::Cleaning {
                    seen.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        engine.quick_clean(vec![QuickCategory::AppCache]).unwrap();
    });
    assert!(seen.into_inner(), "never seen cleaning");
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
}

fn worktree_status(engine: &Engine) -> Option<Removability> {
    let listed = engine.candidates(Some(Category::GitWorktree), ALL).unwrap();
    listed.into_iter().next().and_then(|l| l.removability)
}

#[test]
fn an_engine_opened_on_a_saved_picture_computes_the_worktree_statuses() {
    let place = Place::new(&[]);
    let git_ = isolated(&place.aux);
    let repo = pushed_repository(&place.fx, &git_);
    pushed_worktree(&git_, &repo, &place.path("wt"), "feature");
    let engine = place.open();
    settled(&engine);
    until(&engine, |e| {
        worktree_status(e) != Some(Removability::Unknown)
    });
    drop(engine);
    let again = place.open();
    assert_eq!(again.counters().full_scans, 0, "no scan: the saved picture");
    until(&again, |e| {
        worktree_status(e) != Some(Removability::Unknown)
    });
}

#[test]
fn a_change_below_a_worktree_has_its_status_computed_again() {
    let place = Place::new(&[]);
    let git_ = isolated(&place.aux);
    let repo = pushed_repository(&place.fx, &git_);
    pushed_worktree(&git_, &repo, &place.path("wt"), "feature");
    fs::create_dir(place.path("wt/sub")).unwrap();
    let engine = place.open();
    let idle_at_once = Settings {
        stale_after: Duration::ZERO,
        ..Settings::default()
    };
    engine.set_settings(idle_at_once).unwrap();
    settled(&engine);
    let removable = |e: &Engine| matches!(worktree_status(e), Some(Removability::Removable { .. }));
    std::thread::sleep(Duration::from_millis(1100));
    engine.changed(vec![place.path("wt")]).unwrap();
    until(&engine, removable);
    fs::write(place.path("wt/sub/new.txt"), "work").unwrap();
    engine.changed(vec![place.path("wt/sub")]).unwrap();
    until(&engine, |e| {
        matches!(worktree_status(e), Some(Removability::Blocked(_)))
    });
}

/// Twelve directories that look like linked worktrees, and a Git that
/// notes each call's start and end in a log, takes 200 ms and fails.
fn slow_git_place() -> (Place, EngineConfig, PathBuf) {
    let place = Place::new(&[]);
    for i in 0..12 {
        let wt = place.fx.dir(&format!("wts/wt{i:02}"));
        fs::write(
            wt.join(".git"),
            format!("gitdir: /nowhere/.git/worktrees/wt{i}\n"),
        )
        .unwrap();
    }
    let log = place.aux.path("calls.log");
    let body = format!(
        "echo + >> '{log}'\nsleep 0.2\necho - >> '{log}'\nexit 1",
        log = log.display()
    );
    let slow = crate::repo::script(&place.aux.path("slow-git"), &body);
    let config = EngineConfig {
        git: metla_core::git::Git::new(slow),
        ..place.config(PowerProfile::Background)
    };
    (place, config, log)
}

/// How many Git calls started, and the most that ran at once.
fn calls(log: &Path) -> (usize, usize) {
    let text = fs::read_to_string(log).unwrap_or_default();
    let (mut running, mut most, mut started) = (0usize, 0usize, 0usize);
    for line in text.lines() {
        match line {
            "+" => {
                running += 1;
                started += 1;
                most = most.max(running);
            }
            _ => running = running.saturating_sub(1),
        }
    }
    (started, most)
}

#[test]
fn worktree_statuses_are_computed_in_the_background_two_at_a_time() {
    let (_place, config, log) = slow_git_place();
    let engine = Engine::open(config).unwrap();
    settled(&engine);
    let known = |e: &Engine| {
        let listed = e.candidates(Some(Category::GitWorktree), ALL).unwrap();
        let known = listed
            .iter()
            .filter(|l| l.removability != Some(Removability::Unknown));
        known.count()
    };
    within(&engine, Duration::from_secs(60), |e| known(e) == 12);
    assert_eq!(calls(&log), (12, 2), "one call each, two at a time");
}

#[test]
fn a_batch_has_only_the_worktrees_it_changed_or_lies_in_inspected_again() {
    let (place, config, log) = slow_git_place();
    fs::create_dir(place.path("wts/wt05/deep")).unwrap();
    let engine = Engine::open(config).unwrap();
    settled(&engine);
    let log = log.as_path();
    let started = |n: usize| move |_: &Engine| calls(log).0 == n;
    within(&engine, Duration::from_secs(60), started(12));
    let quiet = |engine: &Engine| {
        settled(engine);
        std::thread::sleep(Duration::from_millis(400));
    };
    engine.changed(vec![place.path("wts")]).unwrap();
    quiet(&engine);
    assert_eq!(calls(log).0, 12, "beside them: none inspected again");
    engine.changed(vec![place.path("wts/wt03")]).unwrap();
    within(&engine, Duration::from_secs(10), started(13));
    engine.changed(vec![place.path("wts/wt05/deep")]).unwrap();
    within(&engine, Duration::from_secs(10), started(14));
    quiet(&engine);
    assert_eq!(calls(log).0, 14, "one worktree each");
}

#[test]
fn closing_the_engine_abandons_the_statuses_still_to_compute() {
    let (_place, config, log) = slow_git_place();
    let engine = Engine::open(config).unwrap();
    settled(&engine);
    drop(engine);
    let (started, _) = calls(&log);
    assert!(
        started < 12,
        "{started} statuses computed after closing began"
    );
}
