//! A cleanup the user asks for starts at once: a full scan with a picture
//! to fall back on gives way to it and is done again afterwards, a batch
//! of rescans stops after the task it is on, and the events below what
//! the cleanup removed are dropped. Scans are held at a directory by a
//! gate in the scan's directory observer, not by sleeping.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use metla_core::clean::TrashHost;
use metla_core::domain::{
    Category, Confirmation, DirFacts, DirId, PreviewId, QuickCategory, RescanTask, Separate,
};
use metla_core::engine::{
    Completeness, Engine, EngineConfig, EngineState, Losses, MAX_REPLAYS, OnLoss, PowerProfile,
    Profile, QUIET_AFTER, Quiet, ScanReason, ScanStart, applied, deferred_line, needs, news,
    news_of, on_loss, quiet_after, redone, replays_after, scan_start, started_line,
    stream_exclusions, tally,
};
use metla_core::fs::Mount;
use metla_core::scan::{DirObserver, other_volumes};
use metla_core::watch::{Event, Flags, FullCause};

use crate::engine::{HOME, Place, quick, settled, until};

/// Holds a scan at the directory named `at` while closed, and notes every
/// directory read and every move to the Trash, in order.
struct Gate {
    at: OsString,
    closed: Mutex<bool>,
    opened: Condvar,
    holding: AtomicBool,
    log: Mutex<Vec<String>>,
}

impl Gate {
    fn new(at: &str) -> Arc<Gate> {
        Arc::new(Gate {
            at: OsString::from(at),
            closed: Mutex::new(false),
            opened: Condvar::new(),
            holding: AtomicBool::new(false),
            log: Mutex::default(),
        })
    }

    fn close(&self) {
        *self.closed.lock().unwrap() = true;
    }

    fn open(&self) {
        *self.closed.lock().unwrap() = false;
        self.opened.notify_all();
    }

    fn holding(&self) -> bool {
        self.holding.load(Ordering::SeqCst)
    }

    fn note(&self, what: String) {
        self.log.lock().unwrap().push(what);
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl DirObserver for Gate {
    fn observe(&self, facts: &DirFacts<'_>) {
        let name = facts.path.file_name().unwrap_or_default();
        self.note(name.to_string_lossy().into_owned());
        if name != self.at {
            return;
        }
        let mut closed = self.closed.lock().unwrap();
        while *closed {
            self.holding.store(true, Ordering::SeqCst);
            closed = self.opened.wait(closed).unwrap();
        }
        self.holding.store(false, Ordering::SeqCst);
    }
}

/// Moves to a directory of its own and notes it in the gate's log.
struct Noted(Arc<Gate>, PathBuf);

impl TrashHost for Noted {
    fn trash(&self, path: &Path) -> Result<(), String> {
        self.0.note("trash".into());
        let name = path.file_name().unwrap();
        fs::rename(path, self.1.join(name)).map_err(|e| e.to_string())
    }
}

/// HOME, a directory the gate holds at, and enough others that a scan
/// stopped at the gate is visibly short of its end.
fn place() -> Place {
    let place = Place::new(HOME);
    place.fx.build(&["~/Documents/gate/kept=1"]);
    for i in 0..200 {
        fs::create_dir_all(place.fx.path(&format!("many/{}/{i}", i % 20))).unwrap();
    }
    place
}

/// An engine whose scans run on one thread, watched by `gate`.
fn open(place: &Place, gate: &Arc<Gate>) -> Engine {
    let mut config = EngineConfig {
        dirs: Some(gate.clone()),
        trash: Arc::new(Noted(gate.clone(), place.aux.dir("trash"))),
        ..place.config(PowerProfile::Background)
    };
    config.profiles.background.threads = 1;
    config.profiles.interactive.threads = 1;
    Engine::open(config).unwrap()
}

/// The log's preview, confirmed with the move to the Trash it needs.
fn log_cleanup(engine: &Engine) -> (PreviewId, Confirmation) {
    let listed = engine.candidates(Some(Category::Log), crate::engine::ALL);
    let ids: Vec<DirId> = listed.unwrap().iter().map(|l| l.candidate.id).collect();
    let preview = engine.preview(&ids).unwrap();
    let confirmation = Confirmation {
        groups: [Separate::MoveToTrash].into(),
        items: Default::default(),
    };
    (preview.id, confirmation)
}

/// Directories the scan read after the gate let it go and before the
/// cleanup moved the log away.
fn read_between(log: &[String]) -> usize {
    let held = log.iter().rposition(|w| w == "gate").unwrap();
    let trashed = log.iter().position(|w| w == "trash").unwrap();
    assert!(held < trashed, "{log:?}");
    trashed - held - 1
}

fn scanning(engine: &Engine) -> Option<bool> {
    match engine.snapshot().unwrap().freshness.state {
        EngineState::Scanning { interactive, .. } => Some(interactive),
        _ => None,
    }
}

/// The root's changed children, past the cap of the test profiles: a
/// full scan of the engine's own.
fn too_many_changes(engine: &Engine, place: &Place) {
    let cap = quick(true).timing.unwrap().max_dirs;
    let children = (0..=cap).map(|i| place.path(&format!("c{i}"))).collect();
    engine.changed(children).unwrap();
}

#[test]
fn a_cleanup_asked_for_during_a_full_scan_starts_before_the_scan_ends_and_the_scan_is_done_again() {
    let place = place();
    let gate = Gate::new("gate");
    let engine = open(&place, &gate);
    settled(&engine);
    gate.close();
    too_many_changes(&engine, &place);
    until(&engine, |_| gate.holding());
    assert_eq!(scanning(&engine), Some(false), "the engine's own");
    let (id, confirmation) = log_cleanup(&engine);
    engine.execute_confirmed(id, confirmation).unwrap();
    gate.open();
    until(&engine, |e| e.last_report().is_some());
    assert!(engine.last_report().unwrap().is_ok());
    assert!(read_between(&gate.log()) < 5, "{:?}", gate.log());
    settled(&engine);
    assert_eq!(engine.counters().full_scans, 3, "first, given way, again");
    let freshness = engine.freshness().unwrap();
    assert_eq!(freshness.scan_reason, Some(ScanReason::Redone));
    assert_eq!(freshness.picture, Completeness::Complete);
    let after = engine.snapshot().unwrap();
    assert_eq!(after.categories[&Category::Log].count, 0, "moved away");
    assert!(engine.candidates(None, crate::engine::ALL).unwrap().len() == 1);
}

#[test]
fn the_users_own_scan_gives_way_to_a_cleanup_and_still_counts_as_ended() {
    let place = place();
    let gate = Gate::new("gate");
    let engine = open(&place, &gate);
    settled(&engine);
    let ended = engine.freshness().unwrap().full_scans_ended;
    gate.close();
    engine.start_scan().unwrap();
    until(&engine, |_| gate.holding());
    assert_eq!(scanning(&engine), Some(true), "the user's");
    let (id, confirmation) = log_cleanup(&engine);
    engine.execute_confirmed(id, confirmation).unwrap();
    gate.open();
    until(&engine, |e| e.last_report().is_some());
    assert!(read_between(&gate.log()) < 5, "{:?}", gate.log());
    let now = engine.freshness().unwrap().full_scans_ended;
    assert!(now > ended, "the user's scan ended: {ended} -> {now}");
    settled(&engine);
    let freshness = engine.freshness().unwrap();
    assert_eq!(
        freshness.scan_reason,
        Some(ScanReason::UserAsked),
        "done again at once, as the user's"
    );
    assert_eq!(
        freshness.full_scans_ended,
        ended + 2,
        "given way, then again"
    );
    assert_eq!(freshness.picture, Completeness::Complete);
}

#[test]
fn a_cleanup_asked_for_during_a_batch_starts_after_the_task_it_is_on() {
    let place = place();
    for i in 0..10 {
        fs::create_dir_all(place.fx.path(&format!("b{i}"))).unwrap();
    }
    let gate = Gate::new("b0");
    let engine = open(&place, &gate);
    settled(&engine);
    gate.close();
    let changed = (0..10).map(|i| place.path(&format!("b{i}"))).collect();
    engine.changed(changed).unwrap();
    until(&engine, |_| gate.holding());
    let (id, confirmation) = log_cleanup(&engine);
    engine.execute_confirmed(id, confirmation).unwrap();
    gate.open();
    until(&engine, |e| e.last_report().is_some());
    settled(&engine);
    let log = gate.log();
    let first = log.iter().rposition(|w| w == "b0").unwrap();
    let trashed = log.iter().position(|w| w == "trash").unwrap();
    let rest = log.iter().rposition(|w| w == "b1").unwrap();
    assert!(first < trashed && trashed < rest, "{log:?}");
    assert_eq!(engine.counters().tasks, 10, "the rest after the cleanup");
    assert_eq!(engine.counters().full_scans, 1);
}

#[test]
fn a_cleanup_asked_for_during_the_first_scan_does_not_stop_it() {
    let place = place();
    let gate = Gate::new("gate");
    gate.close();
    let engine = open(&place, &gate);
    until(&engine, |_| gate.holding());
    engine.quick_clean(vec![QuickCategory::AppCache]).unwrap();
    gate.open();
    until(&engine, |e| e.last_report().is_some());
    settled(&engine);
    let freshness = engine.freshness().unwrap();
    assert_eq!(freshness.picture, Completeness::Complete, "not cut short");
    assert_eq!(freshness.scan_reason, Some(ScanReason::NoPicture));
    assert_eq!(engine.counters().full_scans, 1);
}

#[test]
fn events_below_what_a_cleanup_removed_are_dropped_for_a_while_after_it() {
    let place = place();
    let gate = Gate::new("none");
    let engine = open(&place, &gate);
    settled(&engine);
    let (id, confirmation) = log_cleanup(&engine);
    engine.execute_confirmed(id, confirmation).unwrap();
    until(&engine, |e| e.last_report().is_some());
    settled(&engine);
    let tasks = engine.counters().tasks;
    let log = place.path("~/Library/Logs/com.example.app");
    engine.changed(vec![log.join("deep"), log.clone()]).unwrap();
    settled(&engine);
    assert_eq!(engine.counters().tasks, tasks, "no news");
    engine.changed(vec![place.path("~/Documents")]).unwrap();
    until(&engine, |e| e.counters().tasks == tasks + 1);
}

#[test]
fn an_event_is_news_unless_it_is_below_a_quiet_path_and_says_nothing_was_lost() {
    let quiet = [PathBuf::from("/r/cache")];
    let event = |path: &str, flags: Flags| Event {
        path: PathBuf::from(path),
        flags,
        id: 1,
    };
    let rows = [
        ("/r/cache", Flags::NONE, false),
        ("/r/cache/deep/er", Flags::NONE, false),
        ("/r/cache/x", Flags::USER_DROPPED, false),
        ("/r/cache/x", Flags::ROOT_CHANGED, true),
        ("/r/cache/x", Flags::EVENT_IDS_WRAPPED, true),
        ("/r/cached", Flags::NONE, true),
        ("/r", Flags::NONE, true),
        ("/r/other", Flags::MUST_SCAN_SUBDIRS, true),
    ];
    for (path, flags, expected) in rows {
        assert_eq!(
            news(&event(path, flags), &quiet),
            expected,
            "{path} {flags:?}"
        );
    }
    assert!(news(&event("/r/cache", Flags::NONE), &[]));
}

#[test]
fn a_cleanup_keeps_the_last_ones_paths_quiet_while_they_still_are() {
    let start = Instant::now();
    let second = Duration::from_secs(1);
    let mut quiet = Quiet::default();
    quiet_after(&mut quiet, vec![PathBuf::from("/a")], start, second);
    assert_eq!(quiet.paths, [PathBuf::from("/a")]);
    assert_eq!(quiet.until, Some(start + second + QUIET_AFTER));
    let soon = start + second;
    quiet_after(&mut quiet, vec![PathBuf::from("/b")], soon, Duration::ZERO);
    assert_eq!(
        quiet.paths,
        ["/a", "/b"].map(PathBuf::from),
        "/a still quiet"
    );
    assert_eq!(quiet.until, Some(soon + QUIET_AFTER));
    let later = soon + QUIET_AFTER;
    quiet_after(&mut quiet, vec![PathBuf::from("/c")], later, second);
    assert_eq!(quiet.paths, [PathBuf::from("/c")], "the rest had ended");
}

#[test]
fn quiet_paths_hold_until_their_end_and_a_batch_with_no_news_is_dropped() {
    let start = Instant::now();
    let quiet = Quiet {
        paths: vec![PathBuf::from("/r/cache")],
        until: Some(start + QUIET_AFTER),
    };
    assert!(quiet.holds(start));
    assert!(!quiet.holds(start + QUIET_AFTER), "ended at its end");
    assert!(!Quiet::default().holds(start));
    let event = |path: &str| Event {
        path: PathBuf::from(path),
        flags: Flags::NONE,
        id: 1,
    };
    assert_eq!(news_of(vec![event("/r/cache/a")], &quiet.paths), None);
    let kept = news_of(vec![event("/r/cache/a"), event("/r/b")], &quiet.paths);
    assert_eq!(kept.map(|e| e.len()), Some(1));
}

#[test]
fn why_a_full_scan_is_needed_for_a_task_and_what_it_says() {
    let path = || PathBuf::from("/r/a");
    let rows = [
        (
            RescanTask::Full,
            Some(FullCause::Overflow),
            ScanReason::TooManyChanges,
        ),
        (
            RescanTask::Full,
            Some(FullCause::UserDropped),
            ScanReason::UserDropped,
        ),
        (
            RescanTask::Full,
            Some(FullCause::KernelDropped),
            ScanReason::KernelDropped,
        ),
        (
            RescanTask::Full,
            Some(FullCause::RootChanged),
            ScanReason::RootChanged,
        ),
        (
            RescanTask::Full,
            Some(FullCause::IdsWrapped),
            ScanReason::IdsWrapped,
        ),
        (
            RescanTask::Full,
            Some(FullCause::RootWalk),
            ScanReason::RootWalk,
        ),
        (RescanTask::Full, None, ScanReason::RootWalk),
        (
            RescanTask::Recursive(path()),
            None,
            ScanReason::RescanTooLarge,
        ),
        (
            RescanTask::Emptied(path()),
            Some(FullCause::Overflow),
            ScanReason::RescanTooLarge,
        ),
        (
            RescanTask::Shallow(path()),
            Some(FullCause::UserDropped),
            ScanReason::RescanTooLarge,
        ),
    ];
    for (task, cause, reason) in rows {
        assert_eq!(needs(&task, cause), reason, "{task:?} {cause:?}");
    }
    let said = [
        (ScanReason::NoPicture, "no picture of the root yet"),
        (
            ScanReason::HistoryLost,
            "the event history could not be resumed",
        ),
        (
            ScanReason::UserDropped,
            "events lost on their way to the stream (user dropped)",
        ),
        (
            ScanReason::KernelDropped,
            "events lost by the kernel (kernel dropped)",
        ),
        (
            ScanReason::RootWalk,
            "the root reported for a walk of its own (must scan)",
        ),
        (ScanReason::RootChanged, "the root changed"),
        (ScanReason::IdsWrapped, "the event ids wrapped"),
        (
            ScanReason::TooManyChanges,
            "more changed directories than tracked",
        ),
        (
            ScanReason::RescanTooLarge,
            "a change too large to rescan in place",
        ),
        (ScanReason::RescanFailed, "a rescan failed"),
        (ScanReason::Redone, "a stopped scan is done again"),
        (ScanReason::UserAsked, "the user asked"),
    ];
    for (reason, says) in said {
        assert_eq!(reason.says(), says);
    }
    assert_eq!(
        started_line(ScanReason::Redone, 2),
        "metla: full scan started (a stopped scan is done again), 2 threads"
    );
}

#[test]
fn other_volumes_below_the_root_are_the_mounts_of_other_devices_under_it() {
    let mount = |path: &str, device: u64| Mount {
        path: PathBuf::from(path),
        device,
    };
    let mounts = [
        mount("/", 1),
        mount("/System/Volumes/Data", 1),
        mount("/Volumes/Build", 2),
        mount("/System/Volumes/VM", 3),
        mount("/Users/me/disk", 4),
        mount("/elsewhere", 5),
    ];
    let below_root = other_volumes(Path::new("/"), 1, &mounts);
    let expected = [
        "/Volumes/Build",
        "/System/Volumes/VM",
        "/Users/me/disk",
        "/elsewhere",
    ];
    assert_eq!(below_root, expected.map(PathBuf::from));
    let below_users = other_volumes(Path::new("/Users"), 1, &mounts);
    assert_eq!(below_users, [PathBuf::from("/Users/me/disk")]);
    assert!(other_volumes(Path::new("/Volumes/Build"), 2, &mounts).is_empty());
}

/// When the engine's own full scan may start: at once without a picture,
/// for the user and for a history lost at opening; otherwise a gap after
/// the last full scan and a quiet after the last event that asked for
/// it, whichever is later; never under a profile without full scans.
#[test]
fn a_full_scan_the_engine_needs_waits_for_its_gap_and_for_quiet() {
    let t = Instant::now();
    let s = Duration::from_secs;
    let profile = Profile {
        scan_gap: s(600),
        scan_quiet: s(60),
        ..quick(true)
    };
    let saving = Profile {
        full_scans: false,
        ..profile
    };
    let rows = [
        (
            ScanReason::NoPicture,
            true,
            None,
            s(0),
            &profile,
            ScanStart::Now,
        ),
        (
            ScanReason::UserAsked,
            true,
            None,
            s(0),
            &profile,
            ScanStart::Now,
        ),
        (
            ScanReason::HistoryLost,
            true,
            None,
            s(0),
            &profile,
            ScanStart::Now,
        ),
        (
            ScanReason::UserDropped,
            false,
            None,
            s(0),
            &profile,
            ScanStart::Now,
        ),
        (
            ScanReason::UserDropped,
            true,
            None,
            s(0),
            &profile,
            ScanStart::NotBefore(t + s(600)),
        ),
        (
            ScanReason::TooManyChanges,
            true,
            Some(s(590)),
            s(595),
            &profile,
            ScanStart::NotBefore(t + s(650)),
        ),
        (
            ScanReason::RescanTooLarge,
            true,
            Some(s(10)),
            s(600),
            &profile,
            ScanStart::Now,
        ),
        (
            ScanReason::RescanFailed,
            true,
            Some(s(10)),
            s(599),
            &profile,
            ScanStart::NotBefore(t + s(600)),
        ),
        (
            ScanReason::Redone,
            true,
            None,
            s(601),
            &profile,
            ScanStart::Now,
        ),
        (
            ScanReason::UserAsked,
            true,
            None,
            s(0),
            &saving,
            ScanStart::Never,
        ),
        (
            ScanReason::NoPicture,
            false,
            None,
            s(0),
            &saving,
            ScanStart::Never,
        ),
    ];
    for (reason, picture, trouble, now, profile, start) in rows {
        let trouble = trouble.map(|at| t + at);
        let case = format!("{reason:?}, picture {picture}, trouble {trouble:?}, now +{now:?}");
        assert_eq!(
            scan_start(reason, picture, t, trouble, t + now, profile),
            start,
            "{case}"
        );
    }
    assert_eq!(redone(ScanReason::UserAsked, true), ScanReason::UserAsked);
    assert_eq!(redone(ScanReason::UserDropped, true), ScanReason::Redone);
    assert_eq!(
        redone(ScanReason::UserDropped, false),
        ScanReason::UserDropped
    );
    assert_eq!(
        deferred_line(ScanReason::UserDropped, s(90)),
        "metla: full scan deferred (events lost on their way to the stream (user dropped)), may start in 90 s"
    );
}

/// The engine's profiles with `gap` and `quiet` for its own full scans.
fn waiting(place: &Place, gap: Duration, quiet: Duration) -> Engine {
    let mut config = place.config(PowerProfile::Background);
    for profile in [
        &mut config.profiles.interactive,
        &mut config.profiles.background,
    ] {
        profile.scan_gap = gap;
        profile.scan_quiet = quiet;
    }
    Engine::open(config).unwrap()
}

#[test]
fn a_full_scan_the_engine_needs_waits_and_the_picture_shows_its_age_until_refresh() {
    let place = place();
    let engine = waiting(&place, Duration::from_secs(600), Duration::from_secs(60));
    settled(&engine);
    too_many_changes(&engine, &place);
    until(&engine, |e| {
        let f = e.freshness().unwrap();
        f.needs_full_scan && f.state == EngineState::Idle
    });
    too_many_changes(&engine, &place);
    std::thread::sleep(Duration::from_millis(200));
    let freshness = engine.freshness().unwrap();
    assert_eq!(freshness.state, EngineState::Idle, "not updating: waiting");
    assert_eq!(freshness.scan_reason, Some(ScanReason::TooManyChanges));
    assert_eq!(engine.counters().full_scans, 1, "deferred");
    engine.start_scan().unwrap();
    until(&engine, |e| e.counters().full_scans == 2);
    settled(&engine);
    assert!(
        !engine.freshness().unwrap().needs_full_scan,
        "the user's refresh did it"
    );
}

#[test]
fn two_drops_a_moment_apart_make_one_full_scan_once_it_is_quiet() {
    let place = place();
    let engine = waiting(&place, Duration::ZERO, Duration::from_millis(400));
    settled(&engine);
    too_many_changes(&engine, &place);
    std::thread::sleep(Duration::from_millis(150));
    too_many_changes(&engine, &place);
    until(&engine, |e| e.counters().full_scans == 2);
    settled(&engine);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(engine.counters().full_scans, 2, "one scan for both");
}

/// The stream leaves out the engine's data directory and the other
/// volumes below the root, those outside /System first, at most eight;
/// what does not fit is filtered after delivery, as before.
#[test]
fn the_stream_leaves_out_the_data_directory_and_other_volumes_below_the_root() {
    let paths = |list: &[&str]| -> Vec<PathBuf> { list.iter().map(PathBuf::from).collect() };
    let volumes = paths(&[
        "/System/Volumes/VM",
        "/Volumes/B",
        "/Volumes/A",
        "/System/Volumes/Preboot",
        "/",
    ]);
    let data = Path::new("/Users/u/Library/Application Support/Metla");
    let (left_out, rest) = stream_exclusions(Path::new("/"), data, &volumes);
    let expected = paths(&[
        "/Users/u/Library/Application Support/Metla",
        "/Volumes/A",
        "/Volumes/B",
        "/System/Volumes/Preboot",
        "/System/Volumes/VM",
    ]);
    assert_eq!((left_out, rest), (expected, vec![]));
    let fixture = stream_exclusions(
        Path::new("/fx"),
        Path::new("/aux/data"),
        &paths(&["/fx/vol", "/other"]),
    );
    assert_eq!(
        fixture,
        (paths(&["/fx/vol"]), vec![]),
        "only what is below the root"
    );
    let many: Vec<PathBuf> = (0..10)
        .map(|i| PathBuf::from(format!("/Volumes/v{i}")))
        .collect();
    let (left_out, rest) = stream_exclusions(Path::new("/"), Path::new("/d"), &many);
    assert_eq!(left_out.len(), 8);
    assert_eq!(left_out[0], PathBuf::from("/d"));
    assert_eq!(rest, paths(&["/Volumes/v7", "/Volumes/v8", "/Volumes/v9"]));
}

/// An engine whose scans run on one thread, watched by `gate`, whose own
/// full scans wait ten minutes after the last one and a minute of quiet.
fn open_waiting(place: &Place, gate: &Arc<Gate>) -> Engine {
    let mut config = EngineConfig {
        dirs: Some(gate.clone()),
        ..place.config(PowerProfile::Background)
    };
    for profile in [
        &mut config.profiles.interactive,
        &mut config.profiles.background,
    ] {
        profile.threads = 1;
        profile.scan_gap = Duration::from_secs(600);
        profile.scan_quiet = Duration::from_secs(60);
    }
    Engine::open(config).unwrap()
}

/// Once a root has had a complete picture there always is one to serve
/// and to fall back on until a newer scan completes: a scan killed in its
/// middle leaves the last picture served, and its redo waits like any
/// full scan the engine needs.
#[test]
fn a_scan_killed_in_its_middle_leaves_the_last_picture_served_and_its_redo_waits() {
    let place = place();
    let gate = Gate::new("none");
    let engine = open_waiting(&place, &gate);
    settled(&engine);
    let before = engine.snapshot().unwrap().totals;
    drop(engine);
    let index = place.aux.path("data/index.sqlite");
    let mut writer = metla_core::index::IndexWriter::open(&index).unwrap();
    let killed = writer.start_scan(metla_core::domain::UnixTime(1)).unwrap();
    killed.send(Vec::new());
    drop(writer);
    let again = open_waiting(&place, &gate);
    let freshness = again.freshness().unwrap();
    assert_eq!(
        freshness.picture,
        Completeness::Complete,
        "the last picture"
    );
    assert_eq!(again.snapshot().unwrap().totals, before);
    assert_eq!(freshness.scan_reason, Some(ScanReason::Redone));
    until(&again, |e| {
        e.freshness().unwrap().state == EngineState::Idle
    });
    too_many_changes(&again, &place);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(again.counters().full_scans, 0, "deferred, a drop too");
    assert_eq!(again.freshness().unwrap().state, EngineState::Idle);
}

/// Changes that ask for a full scan while one runs do not start another
/// when it ends: the scan finishes and stores its picture, and the next
/// waits for the gap after it.
#[test]
fn a_drop_during_a_full_scan_waits_for_the_gap_after_it() {
    let place = place();
    let gate = Gate::new("gate");
    let engine = open_waiting(&place, &gate);
    settled(&engine);
    gate.close();
    engine.start_scan().unwrap();
    until(&engine, |_| gate.holding());
    too_many_changes(&engine, &place);
    gate.open();
    until(&engine, |e| e.counters().full_scans == 2);
    until(&engine, |e| {
        let f = e.freshness().unwrap();
        f.needs_full_scan && f.state == EngineState::Idle
    });
    std::thread::sleep(Duration::from_millis(300));
    let freshness = engine.freshness().unwrap();
    assert_eq!(engine.counters().full_scans, 2, "no third scan");
    assert_eq!(
        freshness.picture,
        Completeness::Complete,
        "the user's scan stored its picture"
    );
    assert_eq!(
        freshness.scan_reason,
        Some(ScanReason::TooManyChanges),
        "the last scan's reason"
    );
}

/// A batch holding events lost on its way is replayed from the history,
/// unless it comes from a stream already replaced (its other events are
/// applied) or the replays have not caught up MAX_REPLAYS times.
#[test]
fn a_loss_is_replayed_until_replays_keep_losing() {
    let rows = [
        (false, true, 0, OnLoss::Feed),
        (false, false, 5, OnLoss::Feed),
        (true, false, 0, OnLoss::FeedOthers),
        (true, true, 0, OnLoss::Replay),
        (true, true, MAX_REPLAYS - 1, OnLoss::Replay),
        (true, true, MAX_REPLAYS, OnLoss::Feed),
    ];
    for (dropped, current, replays, action) in rows {
        assert_eq!(
            on_loss(dropped, current, replays),
            action,
            "{dropped} {current} {replays}"
        );
    }
    let event = |flags: Flags| Event {
        path: PathBuf::from("/"),
        flags,
        id: 1,
    };
    let mut losses = Losses::default();
    tally(
        &mut losses,
        &[
            event(Flags::MUST_SCAN_SUBDIRS.with(Flags::USER_DROPPED)),
            event(Flags::MUST_SCAN_SUBDIRS.with(Flags::KERNEL_DROPPED)),
            event(Flags::ROOT_CHANGED),
            event(Flags::EVENT_IDS_WRAPPED),
            event(Flags::NONE),
        ],
    );
    let expected = Losses {
        user_dropped: 1,
        kernel_dropped: 1,
        must_scan: 2,
        root_changed: 1,
        ids_wrapped: 1,
        replays: 0,
    };
    assert_eq!(losses, expected);
}

/// After a batch, the replays not caught up: none once the running
/// stream's history has handed over its end without a new loss; and what
/// is applied: everything, but the loss markers of a replaced stream.
#[test]
fn a_replay_catches_up_at_the_end_of_its_history_and_a_replaced_streams_losses_are_dropped() {
    let event = |path: &str, flags: Flags| Event {
        path: PathBuf::from(path),
        flags,
        id: 1,
    };
    let done = [event("/", Flags::HISTORY_DONE)];
    let lost_and_done = [
        event("/", Flags::HISTORY_DONE),
        event("/", Flags::USER_DROPPED),
    ];
    let plain = [event("/a", Flags::NONE)];
    let rows: [(u32, bool, bool, &[Event], u32); 5] = [
        (2, true, false, &done, 0),
        (2, false, false, &done, 2),
        (2, true, true, &lost_and_done, 2),
        (2, true, false, &plain, 2),
        (0, true, false, &done, 0),
    ];
    for (replays, current, dropped, events, after) in rows {
        assert_eq!(
            replays_after(replays, current, dropped, events),
            after,
            "{replays} {current} {dropped} {events:?}"
        );
    }
    let batch = [
        event("/a", Flags::NONE),
        event("/", Flags::MUST_SCAN_SUBDIRS.with(Flags::USER_DROPPED)),
    ];
    assert_eq!(applied(OnLoss::FeedOthers, &batch), [batch[0].clone()]);
    assert_eq!(applied(OnLoss::Feed, &batch), batch);
    assert_eq!(
        applied(OnLoss::Replay, &batch),
        batch,
        "nothing to replay with: applied"
    );
}
