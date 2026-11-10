//! The FSEvents stream itself, on a temporary directory. Delivery is
//! forced with a flush, so these tests do not wait for the stream's
//! latency. The engine's tests with real streams are in `fsevents`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metla_core::watch::{Event, Flags, Stream, current_event_id, volume_uuid};

use crate::support::Fixture;

type Seen = Arc<Mutex<Vec<Event>>>;

fn start(root: &Path, since: Option<u64>) -> (Stream, Seen) {
    let seen = Seen::default();
    let sink = Arc::clone(&seen);
    let stream = Stream::start(root, since, Duration::from_secs(30), move |events| {
        sink.lock().unwrap().extend(events);
    })
    .unwrap();
    (stream, seen)
}

/// Flushes until an event for `dir` (with `flags`) has come, at most ten
/// seconds.
fn wait_for(stream: &Stream, seen: &Seen, dir: &Path, flags: Flags) -> Event {
    let started = Instant::now();
    loop {
        stream.flush();
        let found = seen
            .lock()
            .unwrap()
            .iter()
            .find(|e| {
                let path: PathBuf = e.path.components().collect();
                path == dir && e.flags.contains(flags)
            })
            .cloned();
        if let Some(event) = found {
            return event;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no event for {}",
            dir.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_stream_reports_the_directory_that_changed() {
    let fx = Fixture::new();
    let dir = fx.dir("watched/sub");
    let before = current_event_id();
    let (stream, seen) = start(fx.root(), None);
    std::fs::write(dir.join("new"), "x").unwrap();
    let event = wait_for(&stream, &seen, &dir, Flags::NONE);
    assert!(event.id > before);
    assert!(!event.flags.lost_history());
}

#[test]
fn a_stream_started_from_an_earlier_event_replays_what_happened_since() {
    let fx = Fixture::new();
    let dir = fx.dir("earlier");
    let (live, seen) = start(fx.root(), None);
    std::fs::write(dir.join("one"), "1").unwrap();
    let first = wait_for(&live, &seen, &dir, Flags::NONE);
    drop(live);
    std::fs::write(dir.join("two"), "2").unwrap();
    let (replay, again) = start(fx.root(), Some(first.id));
    let replayed = wait_for(&replay, &again, &dir, Flags::NONE);
    assert!(replayed.id > first.id, "only what came after");
}

#[test]
fn a_stopped_stream_delivers_nothing() {
    let fx = Fixture::new();
    let dir = fx.dir("quiet");
    let calls = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&calls);
    let stream = Stream::start(fx.root(), None, Duration::from_secs(30), move |_| {
        *counted.lock().unwrap() += 1;
    })
    .unwrap();
    std::fs::write(dir.join("x"), "x").unwrap();
    let started = Instant::now();
    while *calls.lock().unwrap() == 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        stream.flush();
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    let after = *calls.lock().unwrap();
    std::fs::write(dir.join("y"), "y").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(*calls.lock().unwrap(), after);
}

#[test]
fn the_history_has_ids_that_grow_and_a_volume_uuid() {
    let fx = Fixture::new();
    let device = std::os::unix::fs::MetadataExt::dev(&std::fs::metadata(fx.root()).unwrap());
    let uuid = volume_uuid(device as u64).unwrap();
    assert_eq!(uuid.len(), 36, "{uuid}");
    assert_eq!(volume_uuid(device as u64), Some(uuid));
    assert!(current_event_id() > 0);
    assert!(Stream::start(Path::new("/no\0where"), None, Duration::ZERO, |_| {}).is_err());
}
