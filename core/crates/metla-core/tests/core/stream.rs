//! The FSEvents stream itself, on a temporary directory. The regular
//! suite starts, flushes and stops streams and decodes batches, but never
//! waits for fseventsd to deliver: under load it delivers seconds late,
//! which made the suite fail on its own now and then. The tests that need
//! a real delivery are ignored by default (`--ignored`, run once by
//! `scripts/quality.sh`), like the engine's in `fsevents`.

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metla_core::watch::{Event, Flags, Stream, current_event_id, events_of, volume_uuid};

use crate::support::Fixture;

type Seen = Arc<Mutex<Vec<Event>>>;

fn start(root: &Path, since: Option<u64>) -> (Stream, Seen) {
    let seen = Seen::default();
    let sink = Arc::clone(&seen);
    let stream = Stream::start(root, &[], since, Duration::from_secs(30), move |events| {
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
#[ignore = "real FSEvents delivery; run with --ignored"]
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
#[ignore = "real FSEvents delivery; run with --ignored"]
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
#[ignore = "real FSEvents delivery; run with --ignored"]
fn a_stopped_stream_delivers_nothing() {
    let fx = Fixture::new();
    let dir = fx.dir("quiet");
    let calls = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&calls);
    let stream = Stream::start(fx.root(), &[], None, Duration::from_secs(30), move |_| {
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
    assert!(Stream::start(Path::new("/no\0where"), &[], None, Duration::ZERO, |_| {}).is_err());
}

/// From now on, and replaying from the latest event: either way the
/// stream starts, a flush returns and the drop stops it.
#[test]
fn a_stream_starts_flushes_and_stops_without_waiting_for_a_delivery() {
    let fx = Fixture::new();
    for since in [None, Some(current_event_id())] {
        let (stream, _) = start(fx.root(), since);
        stream.flush();
        drop(stream);
    }
}

#[test]
fn a_batch_is_decoded_event_by_event_in_order() {
    let paths: [&CStr; 2] = [c"/watched/a", c"/watched/b c"];
    let events = events_of(&paths, &[0x01, 0x10], &[7, 9]);
    assert_eq!(
        events,
        [
            Event {
                path: PathBuf::from("/watched/a"),
                flags: Flags::MUST_SCAN_SUBDIRS,
                id: 7
            },
            Event {
                path: PathBuf::from("/watched/b c"),
                flags: Flags::HISTORY_DONE,
                id: 9
            },
        ]
    );
    assert!(events_of(&[], &[], &[]).is_empty());
}

/// A disk image of its own, attached (under /Volumes, where macOS mounts
/// it) and detached when dropped; its file lives in a fixture.
struct Image {
    mount: PathBuf,
}

impl Image {
    fn attach(fx: &Fixture) -> Image {
        let file = fx.path("volume.dmg");
        let name = format!("metla-test-{}", std::process::id());
        let made = std::process::Command::new("/usr/bin/hdiutil")
            .args([
                "create", "-quiet", "-size", "16m", "-fs", "APFS", "-volname",
            ])
            .arg(&name)
            .arg(&file)
            .status()
            .unwrap();
        assert!(made.success(), "hdiutil create");
        let out = std::process::Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse"])
            .arg(&file)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout).into_owned();
        let mount = said
            .lines()
            .filter_map(|line| line.split('\t').next_back())
            .map(str::trim)
            .find(|path| path.starts_with("/Volumes/"))
            .unwrap_or_else(|| panic!("no mount point in {said:?}"));
        Image {
            mount: PathBuf::from(mount),
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let _ = std::process::Command::new("/usr/bin/hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&self.mount)
            .status();
    }
}

/// A stream on / that leaves a volume out gets nothing from churn on it;
/// one that does not gets that churn. Both get a change elsewhere.
#[test]
#[ignore = "mounts a disk image of its own and waits for fseventsd; run with --ignored"]
fn a_volume_left_out_of_the_stream_sends_it_nothing() {
    let fx = Fixture::new();
    let image = Image::attach(&fx);
    let collect = |excluded: &[PathBuf]| {
        let seen = Seen::default();
        let sink = Arc::clone(&seen);
        let stream = Stream::start(
            Path::new("/"),
            excluded,
            None,
            Duration::from_millis(50),
            move |events| {
                sink.lock().unwrap().extend(events);
            },
        )
        .unwrap();
        (stream, seen)
    };
    let (plain, all) = collect(&[]);
    let (left_out, rest) = collect(std::slice::from_ref(&image.mount));
    for i in 0..300 {
        let dir = image.mount.join(format!("d{i}"));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
    }
    let marker = fx.dir("marker");
    std::fs::write(marker.join("f"), b"m").unwrap();
    let marker = marker.canonicalize().unwrap();
    wait_for(&plain, &all, &marker, Flags::NONE);
    wait_for(&left_out, &rest, &marker, Flags::NONE);
    let below = |seen: &Seen| {
        let seen = seen.lock().unwrap();
        let inside = seen
            .iter()
            .filter(|e| e.path.starts_with(&image.mount) && e.path != image.mount);
        inside.count()
    };
    assert!(
        below(&all) >= 100,
        "{} events from inside the image",
        below(&all)
    );
    let rest = rest.lock().unwrap();
    let from_image: Vec<&Event> = rest
        .iter()
        .filter(|e| e.path.starts_with(&image.mount))
        .collect();
    assert!(
        from_image.iter().all(|e| e.path == image.mount),
        "only the left-out directory itself, never what is inside: {from_image:?}"
    );
}
