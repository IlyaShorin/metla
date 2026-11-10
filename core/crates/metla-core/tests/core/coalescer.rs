//! The coalescer, driven with a fake clock: settle, backoff and its cap,
//! collapsing, absorbing, the hard cap, lost history, ignored paths,
//! profile changes, batches, and that every change ends up covered.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use metla_core::domain::RescanTask;
use metla_core::watch::{Coalescer, Event, Flags, Timing};

use crate::support::Rng;

const ROOT: &str = "/r";

fn timing() -> Timing {
    Timing {
        settle: secs(1.0),
        factor: 2,
        cap: secs(16.0),
        batch: 100,
        collapse: 4,
        max_dirs: 32,
    }
}

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

fn coalescer() -> Coalescer {
    Coalescer::new(
        Path::new(ROOT),
        vec![PathBuf::from("/r/data")],
        Some(timing()),
    )
}

fn event(path: &str, flags: Flags, id: u64) -> Event {
    Event {
        path: PathBuf::from(path),
        flags,
        id,
    }
}

fn changed(c: &mut Coalescer, path: &str, at: f64) {
    c.on_event(&event(path, Flags::NONE, 1), secs(at));
}

fn shallow(path: &str) -> RescanTask {
    RescanTask::Shallow(PathBuf::from(path))
}

fn recursive(path: &str) -> RescanTask {
    RescanTask::Recursive(PathBuf::from(path))
}

#[test]
fn a_change_is_due_after_the_settle_time_of_quiet() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    assert_eq!(c.pending(), 1);
    assert_eq!(c.next_deadline(), Some(secs(1.0)));
    assert!(c.due(secs(0.999)).is_empty());
    assert_eq!(c.due(secs(1.0)), [shallow("/r/a")]);
    assert_eq!(c.pending(), 0);
    assert_eq!(c.next_deadline(), None);
    assert!(c.due(secs(100.0)).is_empty(), "handed out once");
}

#[test]
fn every_new_event_restarts_the_wait_and_doubles_the_settle_time() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    changed(&mut c, "/r/a", 0.5);
    assert_eq!(c.next_deadline(), Some(secs(2.5)), "settle 2 s after 0.5");
    changed(&mut c, "/r/a", 1.0);
    assert_eq!(c.next_deadline(), Some(secs(5.0)), "settle 4 s after 1.0");
    assert!(c.due(secs(4.9)).is_empty());
    assert_eq!(c.due(secs(5.0)), [shallow("/r/a")]);
}

#[test]
fn events_handed_over_together_count_once() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    changed(&mut c, "/r/a", 0.0);
    assert_eq!(c.next_deadline(), Some(secs(1.0)));
}

#[test]
fn a_directory_that_never_stops_changing_is_due_once_per_cap() {
    let mut c = coalescer();
    let mut handed = Vec::new();
    for tick in 0..200 {
        let now = f64::from(tick) * 0.5;
        changed(&mut c, "/r/build", now);
        if !c.due(secs(now)).is_empty() {
            handed.push(now);
        }
    }
    assert_eq!(handed.first(), Some(&16.0), "one cap after the first event");
    let gaps: Vec<f64> = handed.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.iter().all(|gap| *gap >= 16.0), "{handed:?}");
    assert!(handed.len() >= 5, "{handed:?}");
}

#[test]
fn a_directory_rescanned_recently_backs_off_and_later_starts_afresh() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    assert_eq!(c.due(secs(1.0)).len(), 1);
    changed(&mut c, "/r/a", 3.0);
    assert_eq!(
        c.next_deadline(),
        Some(secs(5.0)),
        "settle 2 s: it fired 2 s ago"
    );
    assert_eq!(c.due(secs(5.0)).len(), 1);
    changed(&mut c, "/r/a", 6.0);
    assert_eq!(c.next_deadline(), Some(secs(10.0)), "settle 4 s");
    assert_eq!(c.due(secs(10.0)).len(), 1);
    changed(&mut c, "/r/a", 26.0);
    assert_eq!(
        c.next_deadline(),
        Some(secs(27.0)),
        "exactly a cap later: 1 s again"
    );
}

#[test]
fn only_as_many_recent_rescans_as_tracked_directories_are_remembered() {
    let two = Timing {
        max_dirs: 2,
        ..timing()
    };
    let mut c = Coalescer::new(Path::new(ROOT), Vec::new(), Some(two));
    for (name, at) in [("a", 0.0), ("b", 2.0), ("c", 4.0)] {
        changed(&mut c, &format!("/r/{name}"), at);
        assert_eq!(c.due(secs(at + 1.0)).len(), 1);
    }
    changed(&mut c, "/r/c", 6.0);
    assert_eq!(
        c.next_deadline(),
        Some(secs(7.0)),
        "/r/c was not remembered"
    );
    assert_eq!(c.due(secs(7.0)).len(), 1);
    changed(&mut c, "/r/b", 8.0);
    assert_eq!(c.next_deadline(), Some(secs(10.0)), "/r/b was");
}

#[test]
fn many_changed_children_become_one_recursive_task_for_their_parent() {
    let mut c = coalescer();
    for name in ["a", "b", "c", "d"] {
        changed(&mut c, &format!("/r/p/{name}"), 0.0);
    }
    assert_eq!(c.pending(), 4, "four is not more than the limit");
    changed(&mut c, "/r/p/e/deeper", 0.5);
    changed(&mut c, "/r/p/e", 0.5);
    assert_eq!(c.pending(), 1);
    assert_eq!(c.due(secs(100.0)), [recursive("/r/p")]);
}

#[test]
fn a_recursive_task_absorbs_what_lies_below_it_and_keeps_the_earliest_change() {
    let mut c = coalescer();
    changed(&mut c, "/r/a/b", 0.0);
    changed(&mut c, "/r/a/b/c", 0.0);
    changed(&mut c, "/r/ab", 0.0);
    c.on_event(&event("/r/a", Flags::MUST_SCAN_SUBDIRS, 2), secs(0.5));
    assert_eq!(c.pending(), 2, "/r/ab is not below /r/a");
    changed(&mut c, "/r/a/x/y", 0.6);
    assert_eq!(c.pending(), 2, "a change below a recursive task joins it");
    assert_eq!(c.next_deadline(), Some(secs(1.0)), "/r/ab");
    let all = c.due(secs(100.0));
    assert_eq!(all, [shallow("/r/ab"), recursive("/r/a")]);
}

#[test]
fn a_shallow_task_becomes_recursive_when_its_subdirectories_must_be_walked() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    c.on_event(&event("/r/a", Flags::MUST_SCAN_SUBDIRS, 2), secs(0.5));
    assert_eq!(c.due(secs(100.0)), [recursive("/r/a")]);
}

#[test]
fn past_the_hard_cap_the_deepest_collapse_into_their_parents() {
    let mut c = coalescer();
    for i in 0..32 {
        changed(&mut c, &format!("/r/g{}/d{i}", i % 8), 0.0);
    }
    assert_eq!(c.pending(), 32, "four per group is not more than the limit");
    changed(&mut c, "/r/extra/deep/er", 0.0);
    assert_eq!(c.pending(), 30);
    let tasks = c.due(secs(100.0));
    assert!(tasks.contains(&recursive("/r/extra/deep")), "{tasks:?}");
    assert!(tasks.contains(&recursive("/r/g7")), "{tasks:?}");
    let recursive = tasks
        .iter()
        .filter(|t| matches!(t, RescanTask::Recursive(_)));
    assert_eq!(recursive.count(), 2, "{tasks:?}");
}

#[test]
fn a_hard_cap_reached_at_the_root_asks_for_a_full_rescan() {
    let tight = Timing {
        max_dirs: 2,
        collapse: 100,
        ..timing()
    };
    let mut c = Coalescer::new(Path::new(ROOT), Vec::new(), Some(tight));
    for name in ["a", "b", "c"] {
        changed(&mut c, &format!("/r/{name}"), 0.0);
    }
    assert_eq!(c.pending(), 1);
    assert_eq!(c.due(secs(100.0)), [RescanTask::Full]);
}

#[test]
fn a_hard_cap_of_nothing_turns_a_change_of_the_root_into_a_full_rescan() {
    let none = Timing {
        max_dirs: 0,
        ..timing()
    };
    let mut c = Coalescer::new(Path::new(ROOT), Vec::new(), Some(none));
    changed(&mut c, "/r", 0.0);
    assert_eq!(c.due(secs(100.0)), [RescanTask::Full]);
}

#[test]
fn walking_the_whole_root_again_is_a_full_rescan() {
    let mut c = coalescer();
    c.on_event(&event("/r", Flags::MUST_SCAN_SUBDIRS, 2), secs(0.0));
    assert_eq!(c.due(secs(100.0)), [RescanTask::Full]);
    for name in ["a", "b", "c", "d", "e"] {
        changed(&mut c, &format!("/r/{name}"), 200.0);
    }
    assert_eq!(
        c.due(secs(300.0)),
        [RescanTask::Full],
        "five changed children of the root"
    );
}

#[test]
fn lost_events_and_a_changed_root_ask_for_a_full_rescan_that_replaces_the_rest() {
    let lost = [
        Flags::USER_DROPPED,
        Flags::KERNEL_DROPPED,
        Flags::EVENT_IDS_WRAPPED,
        Flags::ROOT_CHANGED,
        Flags::MUST_SCAN_SUBDIRS.with(Flags::KERNEL_DROPPED),
    ];
    for flags in lost {
        let mut c = coalescer();
        changed(&mut c, "/r/a", 0.0);
        c.on_event(&event("/r", flags, 9), secs(0.5));
        assert_eq!(c.pending(), 1, "{flags:?}");
        changed(&mut c, "/r/b", 0.7);
        assert_eq!(c.pending(), 1, "{flags:?}: covered by the full rescan");
        assert!(c.due(secs(0.9)).is_empty());
        let due = c.next_deadline().unwrap();
        assert_eq!(due, secs(0.7 + 4.0), "{flags:?}: settle 1, 2, 4 s");
        assert_eq!(c.due(due), [RescanTask::Full], "{flags:?}");
        assert_eq!(c.pending(), 0);
    }
}

#[test]
fn ignored_paths_paths_outside_the_root_and_the_end_of_history_change_nothing() {
    let mut c = coalescer();
    changed(&mut c, "/r/data", 0.0);
    changed(&mut c, "/r/data/index.sqlite-wal", 0.0);
    changed(&mut c, "/elsewhere/x", 0.0);
    changed(&mut c, "/rr", 0.0);
    c.on_event(&event("/r/a", Flags::HISTORY_DONE, 3), secs(0.0));
    assert_eq!(c.pending(), 0);
    changed(&mut c, "/r/database", 0.0);
    changed(&mut c, "/r/./b/", 0.0);
    assert_eq!(c.due(secs(5.0)), [shallow("/r/b"), shallow("/r/database")]);
}

#[test]
fn a_mount_or_unmount_rereads_the_directory_that_holds_the_mount_point() {
    for flags in [Flags::MOUNT, Flags::UNMOUNT] {
        let mut c = coalescer();
        c.on_event(&event("/r/Volumes/disk", flags, 4), secs(0.0));
        assert_eq!(c.due(secs(5.0)), [shallow("/r/Volumes")], "{flags:?}");
        c.on_event(&event("/r", flags, 5), secs(0.0));
        assert_eq!(c.pending(), 0, "nothing holds the root");
    }
}

#[test]
fn the_off_profile_forgets_everything_and_hands_out_nothing() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    c.set_timing(None);
    assert_eq!(c.pending(), 0);
    assert_eq!(c.next_deadline(), None);
    changed(&mut c, "/r/b", 1.0);
    c.on_event(&event("/r", Flags::USER_DROPPED, 7), secs(1.0));
    assert_eq!(c.pending(), 0);
    assert!(c.due(secs(100.0)).is_empty());
    c.set_timing(Some(timing()));
    assert!(c.due(secs(100.0)).is_empty(), "what came while off is gone");
    changed(&mut c, "/r/c", 200.0);
    assert_eq!(c.due(secs(201.0)), [shallow("/r/c")]);
}

#[test]
fn another_profile_retimes_what_is_waiting() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    changed(&mut c, "/r/a", 0.1);
    changed(&mut c, "/r/a", 0.2);
    changed(&mut c, "/r/a", 0.3);
    assert_eq!(c.next_deadline(), Some(secs(8.3)));
    let quick = Timing {
        cap: secs(2.0),
        ..timing()
    };
    c.set_timing(Some(quick));
    assert_eq!(
        c.next_deadline(),
        Some(secs(2.0)),
        "a cap after the first change"
    );
    assert_eq!(c.due(secs(2.0)).len(), 1);
}

#[test]
fn expediting_makes_every_waiting_change_due_at_once() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    changed(&mut c, "/r/a", 0.5);
    changed(&mut c, "/r/b/c", 0.7);
    c.expedite();
    assert_eq!(c.next_deadline(), Some(secs(0.5)), "each at its last event");
    assert_eq!(c.due(secs(0.8)), [shallow("/r/b/c"), shallow("/r/a")]);
    let mut full = coalescer();
    full.on_event(&event("/r", Flags::USER_DROPPED, 3), secs(2.0));
    full.expedite();
    assert_eq!(full.due(secs(2.0)), [RescanTask::Full]);
}

#[test]
fn after_expediting_a_change_settles_for_at_least_the_profiles_settle_time() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    c.expedite();
    changed(&mut c, "/r/a", 0.2);
    assert_eq!(
        c.next_deadline(),
        Some(secs(1.2)),
        "a new event: settle again"
    );
    assert_eq!(c.due(secs(1.2)).len(), 1);
    changed(&mut c, "/r/b", 2.0);
    c.expedite();
    assert_eq!(c.due(secs(2.0)).len(), 1);
    changed(&mut c, "/r/b", 3.0);
    assert_eq!(
        c.next_deadline(),
        Some(secs(4.0)),
        "rescanned recently, but never quicker than the settle time"
    );
}

/// Someone starts looking: a directory that kept changing forgets how
/// long it had backed off, so a change that comes later (fseventsd hands
/// the end of a churn over late) shows after the base settle time, not
/// after a backed-off one of up to a cap.
#[test]
fn expediting_forgets_how_far_a_busy_directory_had_backed_off() {
    let busy = |expedite: bool| {
        let mut c = coalescer();
        changed(&mut c, "/r/a", 0.0);
        changed(&mut c, "/r/a", 0.5);
        assert_eq!(c.due(secs(2.5)), [shallow("/r/a")], "backed off to 2 s");
        if expedite {
            c.expedite();
        }
        changed(&mut c, "/r/a", 3.0);
        c.next_deadline()
    };
    assert_eq!(busy(false), Some(secs(7.0)), "rescanned recently: 4 s");
    assert_eq!(busy(true), Some(secs(4.0)), "looked at: the base 1 s");
}

#[test]
fn a_profile_with_a_longer_settle_time_holds_what_waits_at_least_that_long() {
    let mut c = coalescer();
    changed(&mut c, "/r/a", 0.0);
    changed(&mut c, "/r/a", 0.1);
    let slow = Timing {
        settle: secs(5.0),
        ..timing()
    };
    c.set_timing(Some(slow));
    changed(&mut c, "/r/a", 0.2);
    assert_eq!(
        c.next_deadline(),
        Some(secs(5.2)),
        "settle 4 s grows to 5 s"
    );
}

#[test]
fn due_tasks_come_in_batches_deepest_first() {
    let small = Timing {
        batch: 2,
        ..timing()
    };
    let mut c = Coalescer::new(Path::new(ROOT), Vec::new(), Some(small));
    changed(&mut c, "/r/a", 0.0);
    changed(&mut c, "/r/a/b/c", 0.1);
    changed(&mut c, "/r/x", 0.2);
    changed(&mut c, "/r/a/b", 0.3);
    changed(&mut c, "/r/y", 5.0);
    assert_eq!(c.due(secs(2.0)), [shallow("/r/a/b/c"), shallow("/r/a")]);
    assert_eq!(c.next_deadline(), Some(secs(1.2)), "already due, held back");
    assert_eq!(c.due(secs(2.0)), [shallow("/r/a/b"), shallow("/r/x")]);
    assert!(c.due(secs(2.0)).is_empty());
    assert_eq!(c.due(secs(6.0)), [shallow("/r/y")]);
}

#[test]
fn a_restarted_stream_resumes_after_the_last_change_handed_out() {
    let mut c = coalescer();
    assert_eq!(c.settled(), None);
    c.on_event(&event("/r/a", Flags::NONE, 10), secs(0.0));
    c.on_event(&event("/r/data/x", Flags::NONE, 11), secs(0.0));
    assert_eq!(c.settled(), Some(9), "event 10 is not handed out yet");
    c.on_event(&event("/r/b", Flags::NONE, 12), secs(5.0));
    assert_eq!(c.due(secs(1.0)), [shallow("/r/a")]);
    assert_eq!(c.settled(), Some(11));
    c.due(secs(100.0));
    assert_eq!(c.settled(), Some(12));
    c.on_event(&event("/r/c", Flags::KERNEL_DROPPED, 20), secs(100.0));
    assert_eq!(c.settled(), Some(19));
}

/// Whether `task` rescans what changed at `path`.
fn covers(task: &RescanTask, path: &Path) -> bool {
    match task {
        RescanTask::Full => true,
        RescanTask::Recursive(dir) => path.starts_with(dir),
        RescanTask::Shallow(dir) => path == dir,
    }
}

#[test]
fn every_change_is_handed_out_after_it_happened_and_the_same_input_gives_the_same_output() {
    for seed in 1..=40u64 {
        let mut rng = Rng::new(seed);
        let run = |rng: &mut Rng| {
            let mut c = coalescer();
            let mut changes: Vec<(f64, PathBuf)> = Vec::new();
            let mut handed: Vec<(f64, RescanTask)> = Vec::new();
            let mut now = 0.0;
            for id in 0..300u64 {
                now += f64::from(rng.below(800) as u32) / 1000.0;
                let depth = 1 + rng.below(4);
                let parts: Vec<String> = (0..depth).map(|_| format!("d{}", rng.below(5))).collect();
                let path = PathBuf::from(ROOT).join(parts.join("/"));
                let flags = match rng.below(50) {
                    0 => Flags::MUST_SCAN_SUBDIRS,
                    1 => Flags::KERNEL_DROPPED,
                    _ => Flags::NONE,
                };
                c.on_event(
                    &Event {
                        path: path.clone(),
                        flags,
                        id,
                    },
                    secs(now),
                );
                changes.push((now, path));
                handed.extend(c.due(secs(now)).into_iter().map(|t| (now, t)));
            }
            handed.extend(
                c.due(secs(now + 1000.0))
                    .into_iter()
                    .map(|t| (now + 1000.0, t)),
            );
            (changes, handed, c.pending())
        };
        let (changes, handed, left) = run(&mut rng);
        assert_eq!(left, 0, "seed {seed}");
        for (at, path) in &changes {
            let later = handed.iter().filter(|(t, _)| t >= at);
            assert!(
                later.clone().any(|(_, task)| covers(task, path)),
                "seed {seed}: the change at {} ({at}) was never handed out",
                path.display()
            );
        }
        let again = run(&mut Rng::new(seed));
        let first: BTreeSet<String> = handed.iter().map(|h| format!("{h:?}")).collect();
        let second: BTreeSet<String> = again.1.iter().map(|h| format!("{h:?}")).collect();
        assert_eq!(first, second, "seed {seed}");
    }
}

#[test]
fn a_stream_resumes_only_within_the_same_history_and_not_from_the_future() {
    use metla_core::watch::resume_after;
    let cases = [
        (Some((40, "A")), Some("A"), 50, Some(40)),
        (Some((50, "A")), Some("A"), 50, Some(50)),
        (Some((51, "A")), Some("A"), 50, None),
        (Some((40, "A")), Some("B"), 50, None),
        (Some((40, "A")), None, 50, None),
        (None, Some("A"), 50, None),
    ];
    for (stored, uuid, current, expected) in cases {
        assert_eq!(
            resume_after(stored, uuid, current),
            expected,
            "{stored:?} {uuid:?}"
        );
    }
}
