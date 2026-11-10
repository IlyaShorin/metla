//! Permanent removal of a directory's contents, on synthetic trees only:
//! descriptor-relative, iterative, never following a symlink, never
//! leaving the device, recording what it cannot remove.

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use metla_core::domain::{Bytes, Errno, Leftovers, UnixTime};
use metla_core::fs::{DirHandle, Removal, remove_contents, survey};

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
    fx.build(&["c/locked/a", "c/locked/b", "c/free/x", "c/y"]);
    fs::set_permissions(c.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    let removal = remove(&c);
    fs::set_permissions(c.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(removal.leftovers.failures, 2);
    let mut first = removal.leftovers.first.clone();
    first.sort();
    assert_eq!(
        first,
        [
            (PathBuf::from("locked/a"), Errno(libc::EACCES)),
            (PathBuf::from("locked/b"), Errno(libc::EACCES)),
        ]
    );
    assert!(!removal.leftovers.is_empty());
    assert_eq!(entries(&c), ["locked"], "free/, free/x and y are gone");
    assert_eq!(entries(&c.join("locked")), ["a", "b"]);
    assert_eq!(fs::metadata(c.join("locked/a")).unwrap().len(), 0);
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
    fx.build(&["c/sub/a", "c/sub/b"]);
    let handle = DirHandle::open(&c).unwrap();
    let removed_meanwhile = Cell::new(false);
    let removal = remove_contents(&handle, device(&c), &|| {
        if !removed_meanwhile.replace(true) {
            fs::remove_file(c.join("sub/a")).unwrap();
            fs::remove_file(c.join("sub/b")).unwrap();
            fs::remove_dir(c.join("sub")).unwrap();
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
