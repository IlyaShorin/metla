//! Permanent removal of a directory's contents, on synthetic trees only:
//! descriptor-relative, iterative, never following a symlink, never
//! leaving the device, recording what it cannot remove.

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::num::NonZeroUsize;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use metla_core::domain::{Bytes, Errno, Leftovers, UnixTime};
use metla_core::fs::{
    DirHandle, PARALLEL_DEPTH, Removal, Remover, in_parallel, remove_contents, survey,
};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::scan::deep_chain;
use crate::support::{Fixture, write_file};

fn allocated(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().blocks() * 512
}

fn device(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().dev() as u64
}

fn remove(dir: &Path) -> Removal {
    let handle = DirHandle::open(dir).unwrap();
    remove_contents(&handle, device(dir), &|| false)
}

fn entries(dir: &Path) -> Vec<OsString> {
    let mut names: Vec<OsString> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    names
}

#[test]
fn a_nested_tree_is_emptied_and_its_allocated_size_counted() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/a/b/", "c/a/e/", "c/top"]);
    for (file, len) in [("c/a/b/one", 5_000), ("c/a/two", 9_000), ("c/top", 100)] {
        write_file(&fx.path(file), len);
    }
    let sizes: u64 = ["c/a/b/one", "c/a/two", "c/top"]
        .iter()
        .map(|f| allocated(&fx.path(f)))
        .sum();
    let removal = remove(&c);
    assert_eq!(removal.removed, Bytes(sizes));
    assert_eq!(removal.unlinked, 6, "3 files and 3 directories");
    assert!(removal.leftovers.is_empty(), "{:?}", removal.leftovers);
    assert!(c.is_dir(), "the directory itself stays");
    assert!(entries(&c).is_empty());
}

#[test]
fn a_directory_of_twenty_thousand_entries_is_emptied() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    for n in 0..20_000 {
        fs::write(c.join(format!("f{n}")), "").unwrap();
    }
    let removal = remove(&c);
    assert_eq!(removal.unlinked, 20_000);
    assert!(entries(&c).is_empty());
}

#[test]
fn a_chain_sixteen_hundred_deep_is_removed_without_recursion() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    deep_chain(&c, 1_600);
    // A 64 kB stack holds no recursion 1,600 levels deep.
    let small_stack = std::thread::Builder::new().stack_size(64 * 1024);
    let worker = small_stack.spawn({
        let c = c.clone();
        move || remove(&c)
    });
    let removal = worker.unwrap().join().unwrap();
    assert_eq!(removal.unlinked, 1_600);
    assert!(removal.leftovers.is_empty());
    assert!(entries(&c).is_empty());
}

#[test]
fn a_read_only_file_is_removed_without_changing_its_mode() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    write_file(&c.join("ro"), 10);
    fs::set_permissions(c.join("ro"), fs::Permissions::from_mode(0o444)).unwrap();
    let removal = remove(&c);
    assert_eq!(removal.unlinked, 1);
    assert!(entries(&c).is_empty());
}

#[test]
fn what_a_read_only_directory_holds_stays_and_the_rest_goes() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/deep/locked/a", "c/deep/locked/b", "c/free/x", "c/y"]);
    let locked = c.join("deep/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let removal = remove(&c);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(removal.leftovers.failures, 2, "{:?}", removal.leftovers);
    let mut first = removal.leftovers.first.clone();
    first.sort();
    assert_eq!(
        first,
        [
            (PathBuf::from("deep/locked/a"), Errno(libc::EACCES)),
            (PathBuf::from("deep/locked/b"), Errno(libc::EACCES)),
        ],
        "the directories holding them stay without being failures"
    );
    assert!(!removal.leftovers.is_empty());
    assert_eq!(entries(&c), ["deep"], "free/, free/x and y are gone");
    assert_eq!(entries(&locked), ["a", "b"]);
    assert_eq!(fs::metadata(locked.join("a")).unwrap().len(), 0);
}

#[test]
fn an_unlistable_directory_is_recorded_and_left() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/sealed/a", "c/z"]);
    fs::set_permissions(c.join("sealed"), fs::Permissions::from_mode(0o000)).unwrap();
    let removal = remove(&c);
    fs::set_permissions(c.join("sealed"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        removal.leftovers.first,
        [(PathBuf::from("sealed"), Errno(libc::EACCES))]
    );
    assert_eq!(entries(&c), ["sealed"]);
}

#[test]
fn only_the_first_failures_are_kept_with_their_path() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    let locked = fx.dir("c/locked");
    for n in 0..40 {
        fs::write(locked.join(format!("f{n}")), "").unwrap();
    }
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let removal = remove(&c);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(removal.leftovers.failures, 40);
    assert_eq!(removal.leftovers.first.len(), Leftovers::FIRST);
}

#[test]
fn a_symlink_to_a_directory_outside_is_unlinked_and_its_target_kept() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["outside/keep.txt=precious", "c/link->../outside"]);
    let removal = remove(&c);
    assert_eq!(removal.unlinked, 1);
    assert!(entries(&c).is_empty());
    assert_eq!(
        fs::read_to_string(fx.path("outside/keep.txt")).unwrap(),
        "precious"
    );
}

#[test]
fn a_symlink_loop_is_unlinked_without_being_followed() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/a->b", "c/b->a", "c/self->."]);
    let removal = remove(&c);
    assert_eq!(removal.unlinked, 3);
    assert!(removal.leftovers.is_empty());
}

#[test]
fn a_hard_link_counts_its_share_and_the_other_link_survives() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    write_file(&fx.path("outside"), 64_000);
    fs::hard_link(fx.path("outside"), c.join("shared")).unwrap();
    let blocks = allocated(&fx.path("outside"));
    let removal = remove(&c);
    assert_eq!(removal.removed, Bytes(blocks / 2));
    assert_eq!(fs::metadata(fx.path("outside")).unwrap().len(), 64_000);
}

#[test]
fn fifos_and_sockets_are_unlinked() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    let fifo = std::ffi::CString::new(c.join("fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path; mkfifo only creates the node.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    let socket = UnixListener::bind(c.join("sock")).unwrap();
    let removal = remove(&c);
    drop(socket);
    assert_eq!(removal.unlinked, 2);
    assert!(entries(&c).is_empty());
}

#[test]
fn names_with_newlines_and_other_odd_bytes_are_removed() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    let odd = OsString::from_vec(b"line\nbreak\ttab e\xcc\x81 ".to_vec());
    fs::create_dir(c.join(&odd)).unwrap();
    fs::write(c.join(&odd).join(OsStr::from_bytes(b"\x01\n")), "x").unwrap();
    let removal = remove(&c);
    assert_eq!(removal.unlinked, 2);
    assert!(entries(&c).is_empty());
    // Names are handled as bytes throughout; APFS itself refuses a name
    // that is not UTF-8, so no such entry can be made to remove.
    let invalid = fs::create_dir(c.join(OsStr::from_bytes(b"bad\xff")));
    assert_eq!(invalid.unwrap_err().raw_os_error(), Some(libc::EILSEQ));
}

#[test]
fn directories_on_another_device_are_left_and_reported() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/vol/data", "c/file"]);
    let handle = DirHandle::open(&c).unwrap();
    let elsewhere = device(&c) + 1;
    let removal = remove_contents(&handle, elsewhere, &|| false);
    assert_eq!(removal.leftovers.mounts, [PathBuf::from("vol")]);
    assert_eq!(removal.leftovers.failures, 0);
    assert!(!removal.leftovers.is_empty());
    assert_eq!(
        entries(&c),
        ["vol"],
        "files of the directory itself still go"
    );
    assert!(c.join("vol/data").exists());
}

#[test]
fn cancelling_stops_between_entries_and_says_so() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    for n in 0..10 {
        fs::write(c.join(format!("f{n}")), "").unwrap();
    }
    let asked = Cell::new(0);
    let handle = DirHandle::open(&c).unwrap();
    let removal = remove_contents(&handle, device(&c), &|| {
        asked.set(asked.get() + 1);
        asked.get() > 4
    });
    assert!(removal.leftovers.cancelled);
    assert_eq!(removal.unlinked, 4);
    assert_eq!(entries(&c).len(), 6);
}

#[test]
fn a_directory_that_cannot_be_listed_is_one_failure() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    let handle = DirHandle::open(&c).unwrap();
    fs::set_permissions(&c, fs::Permissions::from_mode(0o000)).unwrap();
    let removal = remove_contents(&handle, device(&c), &|| false);
    fs::set_permissions(&c, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(removal.leftovers.failures, 1);
    assert_eq!(removal.unlinked, 0);
}

#[test]
fn an_entry_removed_meanwhile_is_not_a_failure() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/outer/sub/a", "c/outer/sub/b"]);
    let handle = DirHandle::open(&c).unwrap();
    let asked = Cell::new(0);
    let removal = remove_contents(&handle, device(&c), &|| {
        // Once `outer` is listed, its `sub` goes away.
        asked.set(asked.get() + 1);
        if asked.get() == 2 {
            fs::remove_dir_all(c.join("outer/sub")).unwrap();
        }
        false
    });
    assert!(removal.leftovers.is_empty(), "{:?}", removal.leftovers);
    assert!(entries(&c).is_empty());
}

#[test]
fn an_entry_added_meanwhile_keeps_its_directory_and_is_reported() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/sub/a"]);
    let handle = DirHandle::open(&c).unwrap();
    let calls = Cell::new(0);
    let removal = remove_contents(&handle, device(&c), &|| {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            fs::write(c.join("sub/late"), "").unwrap();
        }
        false
    });
    assert_eq!(
        removal.leftovers.first,
        [(PathBuf::from("sub"), Errno(libc::ENOTEMPTY))]
    );
    assert_eq!(entries(&c.join("sub")), ["late"]);
}

#[test]
fn a_survey_totals_a_tree_on_its_device() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/a/b/", "c/link->../elsewhere"]);
    write_file(&c.join("a/b/f"), 7_000);
    write_file(&c.join("g"), 3_000);
    let handle = DirHandle::open(&c).unwrap();
    let found = survey(&handle, device(&c));
    let files = allocated(&c.join("a/b/f")) + allocated(&c.join("g")) + allocated(&c.join("link"));
    assert_eq!(found.bytes, Bytes(files));
    assert_eq!(found.files, 3);
    let newest = ["a", "a/b", "a/b/f", "g", "link"]
        .iter()
        .map(|p| fs::symlink_metadata(c.join(p)).unwrap().mtime())
        .max()
        .unwrap();
    assert!(found.newest >= Some(UnixTime(newest)));
    let elsewhere = survey(&handle, device(&c) + 1);
    assert_eq!(elsewhere.files, 2, "a/ is on another device");
}

/// A file of `len` bytes, its directories made first.
fn put(path: &Path, len: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_file(path, len);
}

/// Empties `dir` with a remover of `threads` threads, adding up the bytes
/// it reports as it goes.
fn threads_of(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

fn remove_with(dir: &Path, threads: usize) -> (Removal, u64) {
    let handle = DirHandle::open(dir).unwrap();
    let reported = AtomicU64::new(0);
    let removal = Remover::new(threads_of(threads)).remove_contents(
        &handle,
        device(dir),
        &|| false,
        &|bytes| {
            reported.fetch_add(bytes.0, Ordering::Relaxed);
        },
    );
    (removal, reported.into_inner())
}

/// Files, a symlink, hard links and nested directories, some of them
/// locked, as the sequential and the parallel removal both meet them.
fn mixed_tree(fx: &Fixture) -> PathBuf {
    let c = fx.dir("c");
    fx.build(&[
        "c/a/one",
        "c/a/two/three",
        "c/b/x/y/z",
        "c/b/w",
        "c/top",
        "c/locked1/f1",
        "c/locked1/f2",
        "c/d/locked2/g",
        "c/d/free/h",
    ]);
    for n in 0..30 {
        put(&c.join(format!("spread/s{}/f{n}", n % 7)), 5000);
    }
    std::os::unix::fs::symlink("/", c.join("a/link")).unwrap();
    fs::hard_link(c.join("top"), c.join("b/top-link")).unwrap();
    for locked in ["locked1", "d/locked2"] {
        fs::set_permissions(c.join(locked), fs::Permissions::from_mode(0o555)).unwrap();
    }
    c
}

fn unlock(c: &Path) {
    for locked in ["locked1", "d/locked2"] {
        fs::set_permissions(c.join(locked), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn the_parallel_removal_does_what_the_sequential_one_does() {
    let (one, two) = (Fixture::new(), Fixture::new());
    let (a, b) = (mixed_tree(&one), mixed_tree(&two));
    let sequential = remove(&a);
    let (parallel, reported) = remove_with(&b, 4);
    unlock(&a);
    unlock(&b);
    assert_eq!(
        parallel, sequential,
        "removed, unlinked and leftovers, in the same order"
    );
    assert_eq!(
        reported, parallel.removed.0,
        "every byte removed was reported as it went"
    );
    assert_eq!(entries(&a), entries(&b));
    assert_eq!(parallel.leftovers.failures, 3);
}

#[test]
fn failures_in_several_subtrees_are_listed_in_the_same_order_every_time() {
    let runs: Vec<Removal> = (0..5)
        .map(|_| {
            let fx = Fixture::new();
            let c = fx.dir("c");
            for d in 0..6 {
                for f in 0..5 {
                    fs::create_dir_all(c.join(format!("d{d}/locked"))).unwrap();
                    fs::write(c.join(format!("d{d}/locked/f{f}")), "").unwrap();
                }
                fs::set_permissions(
                    c.join(format!("d{d}/locked")),
                    fs::Permissions::from_mode(0o555),
                )
                .unwrap();
            }
            let (removal, _) = remove_with(&c, 8);
            for d in 0..6 {
                fs::set_permissions(
                    c.join(format!("d{d}/locked")),
                    fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
            removal
        })
        .collect();
    assert_eq!(runs[0].leftovers.failures, 30);
    assert_eq!(runs[0].leftovers.first.len(), Leftovers::FIRST);
    assert!(
        runs.iter().all(|run| run == &runs[0]),
        "{:?}",
        runs.iter().map(|r| &r.leftovers.first).collect::<Vec<_>>()
    );
}

#[test]
fn a_cancelled_parallel_removal_stops_and_says_so() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    for d in 0..8 {
        for f in 0..50 {
            put(&c.join(format!("d{d}/f{f}")), 1);
        }
    }
    let asked = AtomicU64::new(0);
    let handle = DirHandle::open(&c).unwrap();
    let removal = Remover::new(threads_of(4)).remove_contents(
        &handle,
        device(&c),
        &|| asked.fetch_add(1, Ordering::Relaxed) >= 20,
        &|_| {},
    );
    assert!(removal.leftovers.cancelled);
    assert!(
        removal.unlinked < 400,
        "it stopped: {} unlinked",
        removal.unlinked
    );
    assert!(!entries(&c).is_empty());
}

#[test]
fn the_parallel_removal_leaves_a_directory_on_another_device() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    fx.build(&["c/vol/data", "c/file"]);
    let handle = DirHandle::open(&c).unwrap();
    let removal =
        Remover::new(threads_of(4)).remove_contents(&handle, device(&c) + 1, &|| false, &|_| {});
    assert_eq!(removal.leftovers.mounts, [PathBuf::from("vol")]);
    assert_eq!(entries(&c), ["vol"]);
    assert!(c.join("vol/data").exists());
}

#[test]
fn a_tree_deeper_than_the_parallel_levels_is_removed() {
    assert_eq!(
        PARALLEL_DEPTH, 32,
        "levels emptied in parallel; below them the iterative walk"
    );
    let fx = Fixture::new();
    let c = fx.dir("c");
    let deep = (0..PARALLEL_DEPTH + 40).fold(c.clone(), |p, n| p.join(format!("l{n}")));
    put(&deep.join("leaf"), 4);
    put(&c.join("l0/l1/side"), 4);
    let (removal, reported) = remove_with(&c, 4);
    assert!(removal.leftovers.is_empty(), "{:?}", removal.leftovers);
    assert_eq!(entries(&c), Vec::<OsString>::new());
    assert_eq!(removal.unlinked, (PARALLEL_DEPTH + 40) as u64 + 2);
    assert_eq!(reported, removal.removed.0);
}

#[test]
fn a_remover_of_one_thread_empties_the_tree() {
    let fx = Fixture::new();
    let c = mixed_tree(&fx);
    let (removal, _) = remove_with(&c, 1);
    unlock(&c);
    assert_eq!(removal.leftovers.failures, 3);
    assert_eq!(entries(&c), ["d", "locked1"]);
}

/// Subdirectories a directory holds beyond the descriptor limit below.
const MANY_DIRS: usize = 300;

/// Run by the test below in a process of its own whose descriptor limit
/// is 64, hard and soft: a directory with more subdirectories than that
/// is emptied without a leftover. Anywhere else (`--ignored`) it does
/// nothing, since the limit is the process's.
#[test]
#[ignore = "run in a process with a low descriptor limit by the test below"]
fn emptied_with_few_descriptors() {
    if std::env::var_os("METLA_FEW_DESCRIPTORS").is_none() {
        return;
    }
    let fx = Fixture::new();
    for i in 0..MANY_DIRS {
        put(&fx.path(&format!("top/d{i}/inner/f")), 10);
    }
    let (removal, _) = remove_with(&fx.path("top"), 4);
    assert!(removal.leftovers.is_empty(), "{:?}", removal.leftovers);
    assert_eq!(removal.unlinked, 3 * MANY_DIRS as u64);
    assert_eq!(fs::read_dir(fx.path("top")).unwrap().count(), 0);
}

#[test]
fn a_directory_with_more_subdirectories_than_descriptors_is_emptied() {
    let exe = std::env::current_exe().unwrap();
    let run = "ulimit -n 64 && exec \"$0\" --ignored --exact remove::emptied_with_few_descriptors";
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", run])
        .arg(exe)
        .env("METLA_FEW_DESCRIPTORS", "1")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("1 passed"), "{said}");
}

#[test]
fn the_top_levels_are_emptied_in_parallel_and_the_rest_iteratively() {
    let rows = [(0, true), (1, true), (31, true), (32, false), (100, false)];
    for (depth, parallel) in rows {
        assert_eq!(in_parallel(depth), parallel, "depth {depth}");
    }
    assert_eq!(Remover::new(threads_of(1)).threads(), Some(1));
    assert_eq!(Remover::new(threads_of(3)).threads(), Some(3));
}

/// `name` made unreadable (writable and searchable only), with a file in
/// it; `unreadable` gives it back its permissions.
fn unreadable(dir: &Path) {
    put(&dir.join("held"), 4);
    fs::set_permissions(dir, fs::Permissions::from_mode(0o300)).unwrap();
}

fn readable(dir: &Path) {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_directory_that_cannot_be_listed_stays_with_its_parents_at_any_depth() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    let deep = (0..PARALLEL_DEPTH + 2).fold(c.join("chain"), |p, n| p.join(format!("l{n}")));
    let shallow = c.join("top/closed");
    put(&c.join("top/other"), 4);
    unreadable(&shallow);
    fs::create_dir_all(&deep).unwrap();
    unreadable(&deep);
    let (removal, _) = remove_with(&c, 4);
    readable(&shallow);
    readable(&deep);
    assert_eq!(removal.leftovers.failures, 2, "{:?}", removal.leftovers);
    let errors: Vec<Errno> = removal.leftovers.first.iter().map(|(_, e)| *e).collect();
    assert_eq!(errors, [Errno(libc::EACCES); 2]);
    assert!(shallow.join("held").exists() && deep.join("held").exists());
    assert!(!c.join("top/other").exists());
}

#[test]
fn a_removal_cancelled_below_the_parallel_levels_keeps_what_it_did_not_reach() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    let deep = (0..PARALLEL_DEPTH + 8).fold(c.clone(), |p, n| p.join(format!("l{n}")));
    put(&deep.join("leaf"), 4);
    let asked = AtomicU64::new(0);
    let cancel = || asked.fetch_add(1, Ordering::SeqCst) >= PARALLEL_DEPTH as u64 + 4;
    let handle = DirHandle::open(&c).unwrap();
    let removal =
        Remover::new(threads_of(2)).remove_contents(&handle, device(&c), &cancel, &|_| {});
    assert!(removal.leftovers.cancelled);
    assert_eq!(removal.leftovers.failures, 0, "{:?}", removal.leftovers);
    assert!(deep.join("leaf").exists(), "not reached");
}

/// The subdirectories of the top levels are emptied on more than one
/// thread: a removal waits (until ten seconds after the start at most)
/// until another thread has removed something too.
#[test]
fn the_top_levels_are_emptied_on_more_than_one_thread() {
    let fx = Fixture::new();
    let c = fx.dir("c");
    for i in 0..8 {
        put(&c.join(format!("d{i}/f")), 4);
    }
    let seen = std::sync::Mutex::new(std::collections::HashSet::new());
    let arrived = std::sync::Condvar::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let removed = |_| {
        let mut threads = seen.lock().unwrap();
        threads.insert(std::thread::current().id());
        arrived.notify_all();
        while threads.len() < 2 && std::time::Instant::now() < deadline {
            let wait = std::time::Duration::from_millis(50);
            threads = arrived.wait_timeout(threads, wait).unwrap().0;
        }
    };
    let handle = DirHandle::open(&c).unwrap();
    let remover = Remover::new(threads_of(4));
    let removal = remover.remove_contents(&handle, device(&c), &|| false, &removed);
    assert!(removal.leftovers.is_empty());
    assert!(seen.lock().unwrap().len() >= 2, "one thread did it all");
}
