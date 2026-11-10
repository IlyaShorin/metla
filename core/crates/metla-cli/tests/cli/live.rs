//! `watch` and `bench`: parsing, what they print, and short runs on a
//! small fixture with settle times and a latency of milliseconds.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use metla_cli::{
    BURST, Bench, CHURN, Command, LATENCY, Place, freshness_line, parse, profiles, spent,
    state_text,
};
use metla_core::domain::{Bytes, ProgressSnapshot, UnixTime};
use metla_core::engine::{Completeness, Counters, EngineState, Freshness, PowerProfile, Profiles};
use metla_core::fs::Usage;

fn args(line: &str) -> Vec<OsString> {
    line.split_whitespace().map(OsString::from).collect()
}

fn place(profile: PowerProfile) -> Place {
    Place {
        root: PathBuf::from("/r"),
        data_dir: PathBuf::from("/d"),
        home: None,
        latency: LATENCY,
        settle: None,
        profile,
    }
}

#[test]
fn watch_takes_where_how_and_for_how_long() {
    assert_eq!(
        parse(&args("watch --root /r --data-dir /d")),
        Ok(Command::Watch {
            place: place(PowerProfile::Background),
            limit: None,
        })
    );
    let line = "watch --root /r --data-dir /d --home /h --seconds 5 --latency-ms 20 \
                --settle-ms 7 --profile saver";
    let expected = Place {
        home: Some(PathBuf::from("/h")),
        latency: Duration::from_millis(20),
        settle: Some(Duration::from_millis(7)),
        ..place(PowerProfile::Saver)
    };
    assert_eq!(
        parse(&args(line)),
        Ok(Command::Watch {
            place: expected,
            limit: Some(Duration::from_secs(5)),
        })
    );
    assert_eq!(LATENCY, Duration::from_secs(1));
}

#[test]
fn bench_takes_what_to_measure() {
    let bench = |line: &str| parse(&args(&format!("bench {line} --root /r --data-dir /d")));
    let at = |bench| Command::Bench {
        place: place(PowerProfile::Background),
        bench,
    };
    assert_eq!(bench("open"), Ok(at(Bench::Open)));
    assert_eq!(bench("refresh"), Ok(at(Bench::Refresh)));
    let churn = Bench::Churn {
        length: CHURN,
        per_burst: BURST,
    };
    assert_eq!(bench("churn"), Ok(at(churn)));
    let short = Bench::Churn {
        length: Duration::from_secs(3),
        per_burst: 9,
    };
    assert_eq!(bench("churn --seconds 3 --files 9"), Ok(at(short)));
    assert_eq!((CHURN, BURST), (Duration::from_secs(120), 400));
    let interactive = parse(&args(
        "bench refresh --root /r --data-dir /d --profile interactive",
    ));
    assert!(matches!(
        interactive,
        Ok(Command::Bench { place, .. }) if place.profile == PowerProfile::Interactive
    ));
    let off = parse(&args("bench open --root /r --data-dir /d --profile off"));
    assert!(matches!(off, Ok(Command::Bench { place, .. }) if place.profile == PowerProfile::Off));
    assert_eq!(
        bench("nonsense"),
        Err("bench needs open, churn or refresh".into())
    );
    assert_eq!(
        parse(&args("watch --root /r --data-dir /d --profile turbo")),
        Err("unknown profile turbo".into())
    );
    assert_eq!(
        parse(&args("bench open --root /r")),
        Err("--data-dir is required".into())
    );
}

#[test]
fn the_state_reads_in_a_few_words() {
    let progress = ProgressSnapshot {
        dirs: 12_345,
        ..ProgressSnapshot::default()
    };
    let cases = [
        (EngineState::Idle, "idle"),
        (
            EngineState::Scanning {
                progress,
                interactive: true,
            },
            "scanning, 12,345 directories",
        ),
        (EngineState::Updating { pending: 3 }, "updating, 3 pending"),
        (EngineState::Cleaning, "cleaning"),
        (EngineState::Paused(PowerProfile::Saver), "paused (Saver)"),
        (EngineState::Off, "off"),
    ];
    for (state, text) in cases {
        assert_eq!(state_text(&state), text);
    }
}

#[test]
fn the_freshness_line_says_what_was_done_and_whether_it_may_be_out_of_date() {
    let counters = Counters {
        full_scans: 1,
        tasks: 7,
        batches: 3,
        events: 40,
        wakeups: 9,
    };
    let fresh = Freshness {
        full_scan: Some(UnixTime(100)),
        updated: Some(UnixTime(1000)),
        pending: 0,
        state: EngineState::Idle,
        picture: Completeness::Complete,
        watching: true,
        needs_full_scan: false,
    };
    assert_eq!(
        freshness_line(UnixTime(1090), &counters, &fresh),
        "batch 3  tasks 7  events 40  pending 0  idle  updated 1 min ago  complete"
    );
    let stale = Freshness {
        updated: None,
        pending: 2,
        state: EngineState::Updating { pending: 2 },
        picture: Completeness::Partial,
        ..fresh.clone()
    };
    assert_eq!(
        freshness_line(UnixTime(1090), &counters, &stale),
        "batch 3  tasks 7  events 40  pending 2  updating, 2 pending  updated never  partial  \
         (may be out of date)"
    );
    let none = Freshness {
        picture: Completeness::Missing,
        ..fresh
    };
    assert!(
        freshness_line(UnixTime(1090), &counters, &none)
            .ends_with("  no picture  (may be out of date)")
    );
}

#[test]
fn what_was_spent_is_the_difference_of_two_samples() {
    let before = Usage {
        user: Duration::from_millis(500),
        system: Duration::from_millis(200),
        switches: 10,
        peak_rss: Bytes(1_000_000),
    };
    let after = Usage {
        user: Duration::from_millis(1700),
        system: Duration::from_millis(600),
        switches: 1_510,
        peak_rss: Bytes(42_000_000),
    };
    assert_eq!(
        spent(&before, &after),
        "CPU 1.2 s user + 0.4 s system, 1,500 context switches, peak RSS 42.0 MB"
    );
}

#[test]
fn the_settle_time_can_be_set_for_every_profile() {
    let standard = profiles(None);
    assert_eq!(standard.background.threads, 2);
    assert_eq!(
        standard,
        Profiles::standard(standard.interactive.threads + 2)
    );
    let quick = profiles(Some(Duration::from_millis(5)));
    for profile in [quick.interactive, quick.background, quick.saver] {
        let timing = profile.timing.unwrap();
        assert_eq!(timing.settle, Duration::from_millis(5));
        assert_eq!(timing.cap, Duration::from_millis(40));
        assert_eq!(profile.pause, Duration::ZERO);
    }
}

/// A small fixture and a data directory beside it.
struct Bed {
    _dir: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
}

impl Bed {
    fn new() -> Bed {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let root = base.join("root");
        for path in [
            "home/Library/Caches/com.example.app",
            "home/Documents",
            "work",
        ] {
            fs::create_dir_all(root.join(path)).unwrap();
        }
        fs::write(root.join("home/Library/Caches/com.example.app/blob"), "x").unwrap();
        fs::write(root.join(".metla-fixture"), "").unwrap();
        Bed {
            _dir: dir,
            data: base.join("data"),
            root,
        }
    }

    fn run(&self, line: &str) -> (i32, String, String) {
        let home = self.root.join("home");
        let words = format!(
            "{line} --root {} --data-dir {} --home {} --latency-ms 20 --settle-ms 5",
            self.root.display(),
            self.data.display(),
            home.display()
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = metla_cli::run(&args(&words), &mut out, &mut err);
        let text = |bytes| String::from_utf8(bytes).unwrap();
        (code, text(out), text(err))
    }
}

fn later(path: &Path, after: Duration) -> std::thread::JoinHandle<()> {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("x"), "x").unwrap();
    })
}

#[test]
fn watch_scans_a_new_root_and_prints_every_batch() {
    let bed = Bed::new();
    let writer = later(&bed.root.join("work/new"), Duration::from_millis(400));
    let (code, out, err) = bed.run("watch --seconds 2");
    writer.join().unwrap();
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.starts_with("watching "), "{out}");
    assert!(out.contains("\nbatch 1  tasks "), "{out}");
    assert!(out.contains(" batches, "), "{out}");
    let (code, opened, _) = bed.run("bench open");
    assert_eq!(code, 0);
    assert!(opened.starts_with("open + first snapshot: "), "{opened}");
}

#[test]
fn bench_open_without_a_picture_counts_no_directories() {
    let bed = Bed::new();
    let (code, opened, err) = bed.run("bench open");
    assert_eq!((code, err.as_str()), (0, ""), "{opened}");
    assert!(opened.contains(" for 0 directories; "), "{opened}");
}

#[test]
fn bench_churn_follows_the_churn_and_ends_exactly_right() {
    let bed = Bed::new();
    let (code, out, err) = bed.run("bench churn --seconds 1 --files 20");
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.contains("picture exactly right"), "{out}");
    let words: Vec<&str> = out.split_whitespace().collect();
    let files: u64 = words[1].replace(',', "").parse().unwrap();
    let bursts: u64 = words[7].parse().unwrap();
    assert!(bursts > 0, "{out}");
    assert_eq!(
        files,
        2 * 20 * bursts,
        "every file written, then removed: {out}"
    );
    assert!(
        !bed.root.join("churn").exists(),
        "the churn cleans up after itself"
    );
}

#[test]
fn bench_refresh_times_three_small_changes() {
    let bed = Bed::new();
    let (code, out, err) = bed.run("bench refresh");
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.starts_with("refresh with Background: "), "{out}");
    assert_eq!(out.matches(" s").count(), 3, "{out}");
    let shown: Vec<f64> = out
        .split_whitespace()
        .filter_map(|word| word.parse().ok())
        .collect();
    assert_eq!(shown.len(), 3, "{out}");
    assert!(
        shown.iter().all(|s| *s >= 0.020),
        "nothing shows before the stream's latency: {out}"
    );
}

#[test]
fn benchmarks_that_write_need_a_fixture_and_a_data_directory_outside_it() {
    let bed = Bed::new();
    fs::remove_file(bed.root.join(".metla-fixture")).unwrap();
    let (code, _, err) = bed.run("bench churn --seconds 1");
    assert_eq!(code, 1);
    assert!(
        err.ends_with("benchmarks write only into fixtures\n"),
        "{err}"
    );
    let (code, _, err) = bed.run("bench refresh");
    assert_eq!(code, 1);
    assert!(
        err.ends_with("benchmarks write only into fixtures\n"),
        "{err}"
    );
    fs::write(bed.root.join(".metla-fixture"), "").unwrap();
    let inside = format!(
        "bench churn --seconds 1 --root {} --data-dir {}",
        bed.root.display(),
        bed.root.join("data").display()
    );
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(metla_cli::run(&args(&inside), &mut out, &mut err), 1);
    assert!(
        String::from_utf8(err)
            .unwrap()
            .contains("must lie outside the root")
    );
}
