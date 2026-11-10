//! The engine with a real FSEvents stream on a synthetic root.
//!
//! One test, with a 20 ms latency and settle times of milliseconds, is in
//! the regular suite. The others use the standard profiles' settle times
//! and a latency of a second, take seconds each, and are ignored by
//! default: `cargo test -- --ignored fsevents` runs them, and
//! `scripts/quality.sh` runs them once, apart from the mutation runs.

use std::fs;
use std::time::{Duration, Instant};

use metla_core::engine::{Engine, EngineConfig, EngineState, PowerProfile, Profiles, Watching};

use crate::engine::{HOME, Place, offered, profiles, settled, until, within};
use crate::support::write_file;

fn real(place: &Place, latency: Duration, profiles: Profiles) -> EngineConfig {
    EngineConfig {
        watching: Watching::FsEvents { latency },
        profiles,
        ..place.config(PowerProfile::Background)
    }
}

/// The standard profiles, with the slow ones' threads.
fn standard() -> Profiles {
    Profiles::standard(4)
}

#[test]
fn the_engine_follows_a_real_stream_and_resumes_after_the_stored_event() {
    let place = Place::new(HOME);
    let quick = || real(&place, Duration::from_millis(20), profiles());
    let engine = Engine::open(quick()).unwrap();
    settled(&engine);
    place.fx.build(&["~/Library/Caches/org.seen.live/x=1"]);
    until(&engine, |e| {
        offered(e, &place).contains("home/Library/Caches/org.seen.live")
    });
    assert!(engine.snapshot().unwrap().freshness.watching);
    drop(engine);
    place
        .fx
        .build(&["~/Library/Caches/org.made.while.down/x=1"]);
    let again = Engine::open(quick()).unwrap();
    until(&again, |e| {
        offered(e, &place).contains("home/Library/Caches/org.made.while.down")
    });
    settled(&again);
    assert_eq!(again.counters().full_scans, 0, "resumed, not rescanned");
}

#[test]
#[ignore = "real FSEvents latency; run with --ignored"]
fn fsevents_a_created_candidate_appears_and_a_deleted_one_disappears() {
    let place = Place::new(HOME);
    let engine = Engine::open(real(&place, Duration::from_secs(1), standard())).unwrap();
    settled(&engine);
    let made = Instant::now();
    place.fx.build(&["~/Library/Caches/org.new.tool/blob=x"]);
    until(&engine, |e| {
        offered(e, &place).contains("home/Library/Caches/org.new.tool")
    });
    let appeared = made.elapsed();
    fs::remove_dir_all(place.path("~/Library/Caches/org.new.tool")).unwrap();
    until(&engine, |e| {
        !offered(e, &place).contains("home/Library/Caches/org.new.tool")
    });
    assert!(appeared < Duration::from_secs(10), "{appeared:?}");
    eprintln!("fsevents: a new candidate appeared after {appeared:?}");
}

#[test]
#[ignore = "real FSEvents latency; run with --ignored"]
fn fsevents_a_storm_of_writes_ends_in_few_rescans() {
    let place = Place::new(HOME);
    let engine = Engine::open(real(&place, Duration::from_secs(1), standard())).unwrap();
    settled(&engine);
    let busy = place.fx.dir("build/out");
    let before = engine.counters();
    let started = Instant::now();
    let mut written = 0;
    while started.elapsed() < Duration::from_secs(15) {
        for i in 0..50 {
            write_file(&busy.join(format!("o{}-{i}", written % 40)), 100);
        }
        written += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
    within(&engine, Duration::from_secs(90), |e| {
        let freshness = e.snapshot().unwrap().freshness;
        freshness.state == EngineState::Idle && freshness.pending == 0
    });
    let after = engine.counters();
    let tasks = after.tasks - before.tasks;
    eprintln!(
        "fsevents: {} events, {tasks} rescans for {} writes in 15 s",
        after.events - before.events,
        written * 50
    );
    assert!(tasks <= 8, "{tasks} rescans");
}

#[test]
#[ignore = "real FSEvents latency; run with --ignored"]
fn fsevents_off_stops_activity_and_cancels_a_background_scan() {
    let place = Place::new(HOME);
    for i in 0..3000 {
        fs::create_dir_all(place.fx.path(&format!("wide/{}/{i}", i % 30))).unwrap();
    }
    let slow = Profiles {
        background: metla_core::engine::Profile {
            threads: 1,
            ..standard().background
        },
        ..standard()
    };
    let engine = Engine::open(real(&place, Duration::from_secs(1), slow)).unwrap();
    until(&engine, |e| e.progress().is_some());
    engine.set_power_profile(PowerProfile::Off).unwrap();
    until(&engine, |e| {
        e.snapshot().unwrap().freshness.state == EngineState::Off
    });
    let overview = engine.snapshot().unwrap();
    assert!(
        overview.freshness.needs_full_scan,
        "the first scan was cancelled"
    );
    let tasks = engine.counters().tasks;
    place.fx.build(&["~/Library/Caches/org.while.off/x=1"]);
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(engine.counters().tasks, tasks);
    assert!(!overview.freshness.watching);
}

#[test]
#[ignore = "real FSEvents latency; run with --ignored"]
fn fsevents_a_restart_resumes_from_the_stored_event_and_catches_what_changed_meanwhile() {
    let place = Place::new(HOME);
    let config = || real(&place, Duration::from_secs(1), standard());
    let engine = Engine::open(config()).unwrap();
    settled(&engine);
    place.fx.build(&["~/Library/Caches/org.before/x=1"]);
    until(&engine, |e| {
        offered(e, &place).contains("home/Library/Caches/org.before")
    });
    drop(engine);
    place.fx.build(&["~/Library/Caches/org.meanwhile/x=1"]);
    std::thread::sleep(Duration::from_secs(2));
    let again = Engine::open(config()).unwrap();
    until(&again, |e| {
        offered(e, &place).contains("home/Library/Caches/org.meanwhile")
    });
    settled(&again);
    assert_eq!(again.counters().full_scans, 0);
}
