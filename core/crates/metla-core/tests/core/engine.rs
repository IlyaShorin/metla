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
    Category, Confirmation, DirId, Exclusion, Outcome, QuickCategory, Separate,
};
use metla_core::engine::{
    Changed, Completeness, DirRef, Engine, EngineConfig, EngineError, EngineObserver, EngineState,
    FolderEntry, HotkeyMode, Listed, PowerProfile, Profile, Profiles, Removability, Settings,
    Watching,
};
use metla_core::index::Page;
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
    assert_eq!(engine.counters().full_scans, 1);
    assert!(engine.counters().tasks >= 2);
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
fn off_stops_watching_and_applies_nothing_until_it_is_on_again() {
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
fn settings_are_kept_and_switching_background_analysis_off_stops_watching() {
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
fn a_folder_lists_its_subdirectories_and_files_largest_first() {
    let place = Place::new(&["top/small/x=1", "top/big/y=2"]);
    write_file(&place.path("top/big/z"), 50_000);
    write_file(&place.path("top/file"), 20_000);
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
        })
        .collect();
    assert_eq!(names, ["big/", "file", "small/"]);
    assert_eq!(folder.total, 3);
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
    engine.set_background(false).unwrap();
    until(&engine, |e| {
        let state = e.snapshot().unwrap().freshness.state;
        state == EngineState::Off && e.progress().is_none()
    });
    let freshness = engine.snapshot().unwrap().freshness;
    assert_ne!(freshness.picture, Completeness::Complete);
}
