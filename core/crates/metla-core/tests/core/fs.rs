use std::ffi::OsStr;
use std::fs;
use std::num::NonZeroU64;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use metla_core::domain::{Entry, EntryKind, EntryMeta, Errno, Stamp, UnixTime};
use metla_core::fs::{
    Bulk, DirHandle, ListError, decode_bulk, list_dir, list_dir_classic, mounts, peak_rss,
    raise_open_file_limit, raised_open_file_limit, retry_interrupted, volume,
};

use crate::support::{Fixture, write_file};

fn names(path: &Path) -> Vec<String> {
    let dir = DirHandle::open(path).unwrap();
    let mut names: Vec<String> = list_dir(&dir)
        .unwrap()
        .into_iter()
        .map(|e| e.name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn listing_returns_every_entry_except_dot_and_dot_dot() {
    let fx = Fixture::new();
    write_file(&fx.path("a"), 10);
    fs::create_dir(fx.path("d")).unwrap();
    assert_eq!(names(fx.root()), vec!["a", "d"]);
}

#[test]
fn listing_twice_returns_the_same_entries() {
    let fx = Fixture::new();
    write_file(&fx.path("a"), 10);
    let dir = DirHandle::open(fx.root()).unwrap();
    let first = list_dir(&dir).unwrap();
    assert_eq!(list_dir(&dir).unwrap(), first);
    assert_eq!(first.len(), 1);
}

#[test]
fn entries_carry_kind_size_links_device_inode_and_mtime() {
    let fx = Fixture::new();
    write_file(&fx.path("file"), 10_000);
    fs::hard_link(fx.path("file"), fx.path("link")).unwrap();
    fs::create_dir(fx.path("dir")).unwrap();
    symlink("dir", fx.path("sym")).unwrap();
    let dir = DirHandle::open(fx.root()).unwrap();
    let entries = list_dir(&dir).unwrap();
    let find = |name: &str| entries.iter().find(|e| e.name == name).unwrap().meta;
    let std_meta = fs::symlink_metadata(fx.path("file")).unwrap();
    let file = find("file");
    assert_eq!(file.kind, EntryKind::File);
    assert_eq!(file.blocks, std_meta.blocks());
    assert!(file.blocks * 512 >= 10_000);
    assert_eq!(file.links.get(), 2);
    assert_eq!(file.device, std_meta.dev());
    assert_eq!(file.inode, std_meta.ino());
    assert_eq!(file.mtime.0, std_meta.mtime());
    assert!(!file.dataless);
    assert_eq!(find("dir").kind, EntryKind::Dir);
    assert_eq!(find("sym").kind, EntryKind::Symlink);
}

#[test]
fn a_fifo_is_an_other_entry() {
    let fx = Fixture::new();
    let status = std::process::Command::new("mkfifo")
        .arg(fx.path("pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    let dir = DirHandle::open(fx.root()).unwrap();
    assert_eq!(list_dir(&dir).unwrap()[0].meta.kind, EntryKind::Other);
}

#[test]
fn a_child_directory_opens_relative_to_its_parent() {
    let fx = Fixture::new();
    write_file(&fx.dir("d").join("inner"), 1);
    let parent = DirHandle::open(fx.root()).unwrap();
    let child = parent.open_child(OsStr::new("d")).unwrap();
    assert_eq!(list_dir(&child).unwrap()[0].name, "inner");
}

#[test]
fn a_directory_reports_its_own_metadata() {
    let fx = Fixture::new();
    let dir = DirHandle::open(fx.root()).unwrap();
    let meta = dir.meta().unwrap();
    let std_meta = fs::metadata(fx.root()).unwrap();
    assert_eq!(meta.kind, EntryKind::Dir);
    assert_eq!(meta.inode, std_meta.ino());
}

#[test]
fn a_missing_child_is_gone() {
    let fx = Fixture::new();
    let parent = DirHandle::open(fx.root()).unwrap();
    assert_eq!(
        parent.open_child(OsStr::new("nope")).err(),
        Some(ListError::Gone)
    );
    assert_eq!(
        DirHandle::open(&fx.path("nope")).err(),
        Some(ListError::Gone)
    );
}

#[test]
fn a_child_that_is_a_file_is_gone() {
    let fx = Fixture::new();
    write_file(&fx.path("f"), 1);
    let parent = DirHandle::open(fx.root()).unwrap();
    assert_eq!(
        parent.open_child(OsStr::new("f")).err(),
        Some(ListError::Gone)
    );
}

#[test]
fn a_symlinked_child_is_not_followed() {
    let fx = Fixture::new();
    fs::create_dir(fx.path("d")).unwrap();
    symlink("d", fx.path("s")).unwrap();
    let parent = DirHandle::open(fx.root()).unwrap();
    assert!(parent.open_child(OsStr::new("s")).is_err());
}

#[test]
fn an_unreadable_directory_is_denied() {
    let fx = Fixture::new();
    fs::create_dir(fx.path("locked")).unwrap();
    fs::set_permissions(fx.path("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let parent = DirHandle::open(fx.root()).unwrap();
    let result = parent.open_child(OsStr::new("locked")).err();
    fs::set_permissions(fx.path("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(result, Some(ListError::Denied));
}

#[test]
fn a_directory_without_search_permission_cannot_be_listed() {
    let fx = Fixture::new();
    fs::create_dir(fx.path("names-only")).unwrap();
    write_file(&fx.path("names-only/f"), 1);
    fs::set_permissions(fx.path("names-only"), fs::Permissions::from_mode(0o444)).unwrap();
    let parent = DirHandle::open(fx.root()).unwrap();
    let child = parent.open_child(OsStr::new("names-only")).unwrap();
    let result = list_dir(&child).err();
    fs::set_permissions(fx.path("names-only"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(result, Some(ListError::Denied));
}

#[test]
fn errors_are_classified_by_errno() {
    assert_eq!(ListError::from_errno(libc::EACCES), ListError::Denied);
    assert_eq!(ListError::from_errno(libc::EPERM), ListError::Denied);
    assert_eq!(ListError::from_errno(libc::ENOENT), ListError::Gone);
    assert_eq!(ListError::from_errno(libc::ENOTDIR), ListError::Gone);
    assert_eq!(
        ListError::from_errno(libc::EIO),
        ListError::Io(Errno(libc::EIO))
    );
}

#[test]
fn the_volume_of_a_path_has_size_free_space_and_mount_point() {
    let fx = Fixture::new();
    let vol = volume(fx.root()).unwrap();
    assert!(vol.total >= vol.available);
    assert!(vol.available.0 >= 1 << 20);
    // Through a firmlink the path need not start with the mount point.
    assert!(vol.mount_point.is_absolute());
    let listed = mounts().unwrap();
    let mount = listed.iter().find(|m| m.path == vol.mount_point).unwrap();
    assert_eq!(mount.device, vol.device);
    assert_eq!(vol.device, fs::metadata(fx.root()).unwrap().dev());
}

#[test]
fn the_volume_of_a_missing_path_is_an_error() {
    assert!(volume(Path::new("/nonexistent/metla/volume")).is_err());
}

#[test]
fn the_process_reports_its_peak_resident_size() {
    assert!(peak_rss().0 >= 1 << 20);
}

#[test]
fn the_open_file_limit_is_raised_for_deep_trees() {
    assert!(raise_open_file_limit() >= 4096);
}

#[test]
fn the_open_file_limit_is_raised_up_to_the_hard_limit_and_open_max() {
    assert_eq!(raised_open_file_limit(256, u64::MAX), Some(10_240));
    assert_eq!(raised_open_file_limit(256, 4096), Some(4096));
    assert_eq!(raised_open_file_limit(10_240, u64::MAX), None);
    assert_eq!(raised_open_file_limit(50_000, u64::MAX), None);
}

#[test]
fn an_interrupted_call_is_repeated_until_it_succeeds_or_really_fails() {
    let mut calls = 0;
    let result = retry_interrupted(|| {
        calls += 1;
        match calls {
            1 | 2 => Err(libc::EINTR),
            _ => Ok(7),
        }
    });
    assert_eq!((result, calls), (Ok(7), 3));
    let mut calls = 0;
    let result: Result<i32, i32> = retry_interrupted(|| {
        calls += 1;
        Err(libc::EACCES)
    });
    assert_eq!((result, calls), (Err(libc::EACCES), 1));
}

fn head(dir: &Path, name: &str, limit: usize) -> Result<Vec<u8>, ListError> {
    DirHandle::open(dir)
        .unwrap()
        .read_head(OsStr::new(name), limit)
}

#[test]
fn the_head_of_a_small_file_is_read_up_to_a_limit() {
    let fx = Fixture::new();
    fs::write(fx.path(".git"), b"gitdir: /x/.git/worktrees/a\n").unwrap();
    assert_eq!(
        head(fx.root(), ".git", 1024).unwrap(),
        b"gitdir: /x/.git/worktrees/a\n"
    );
    assert_eq!(head(fx.root(), ".git", 6).unwrap(), b"gitdir");
    let big = vec![b'x'; 70_000];
    fs::write(fx.path("big"), &big).unwrap();
    assert_eq!(head(fx.root(), "big", 65_536).unwrap(), &big[..65_536]);
    fs::write(fx.path("empty"), b"").unwrap();
    assert_eq!(head(fx.root(), "empty", 10).unwrap(), b"");
}

#[test]
fn only_a_regular_file_is_read_and_a_symlink_is_not_followed() {
    let fx = Fixture::new();
    fs::write(fx.path("real"), b"data").unwrap();
    symlink("real", fx.path("link")).unwrap();
    fs::create_dir(fx.path("dir")).unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(fx.path("pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(head(fx.root(), "link", 10).is_err());
    assert!(head(fx.root(), "dir", 10).is_err());
    assert!(
        head(fx.root(), "pipe", 10).is_err(),
        "a FIFO must not block"
    );
    assert_eq!(head(fx.root(), "missing", 10).err(), Some(ListError::Gone));
}

fn differences(bulk: &[Entry], classic: &[Entry]) {
    for (b, c) in bulk.iter().zip(classic) {
        if b != c {
            eprintln!(
                "DIFF {:?}\n  bulk    {:?}\n  classic {:?}",
                b.name, b.meta, c.meta
            );
        }
    }
}

/// Both listings of `path`, sorted by name.
fn both(path: &Path) -> (Vec<Entry>, Vec<Entry>) {
    let dir = DirHandle::open(path).unwrap();
    let mut bulk = list_dir(&dir).unwrap();
    let mut classic = list_dir_classic(&dir).unwrap();
    bulk.sort_by(|a, b| a.name.cmp(&b.name));
    classic.sort_by(|a, b| a.name.cmp(&b.name));
    (bulk, classic)
}

#[test]
fn the_bulk_listing_gives_exactly_what_readdir_and_fstatat_give() {
    let fx = Fixture::new();
    write_file(&fx.path("big"), 300_000);
    write_file(&fx.path("small"), 10);
    fs::write(fx.path("empty"), "").unwrap();
    fs::hard_link(fx.path("small"), fx.path("small-link")).unwrap();
    symlink("big", fx.path("to-big")).unwrap();
    fx.dir("sub/inner");
    fx.dir("é ü 名前");
    let fifo = std::ffi::CString::new(fx.path("pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    for i in 0..2500 {
        let long = format!("{i:05}-{}", "x".repeat(60));
        fs::write(fx.path(&long), [1u8; 1]).unwrap();
    }
    let (bulk, classic) = both(fx.root());
    assert_eq!(bulk.len(), 2508, "more entries than one call returns");
    differences(&bulk, &classic);
    assert_eq!(bulk, classic);
    let kinds: Vec<EntryKind> = ["pipe", "sub", "to-big", "big"]
        .iter()
        .map(|name| bulk.iter().find(|e| e.name == *name).unwrap().meta.kind)
        .collect();
    let expected = [
        EntryKind::Other,
        EntryKind::Dir,
        EntryKind::Symlink,
        EntryKind::File,
    ];
    assert_eq!(kinds, expected);
    let (sub_bulk, sub_classic) = both(&fx.path("sub"));
    assert_eq!(sub_bulk, sub_classic);
}

#[test]
fn the_bulk_listing_describes_mount_points_and_firmlinks_by_what_they_lead_to() {
    for system in ["/", "/Volumes"] {
        let (bulk, classic) = both(Path::new(system));
        differences(&bulk, &classic);
        assert_eq!(bulk, classic, "{system}");
    }
}

/// `VREG`, `VDIR`, `VLNK` and `VFIFO` of `enum vtype`.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;
const VFIFO: u32 = 7;
/// `SF_FIRMLINK` and `SF_DATALESS` from `<sys/stat.h>`.
const SF_FIRMLINK: u32 = 0x0080_0000;
const SF_DATALESS: u32 = 0x4000_0000;

/// One record as `getattrlistbulk` packs it for the listing.
struct Packed {
    /// Common, volume, directory, file and fork attributes returned.
    returned: [u32; 5],
    error: u32,
    device: u32,
    kind: u32,
    mtime: [i64; 2],
    changed: [i64; 2],
    flags: u32,
    inode: u64,
    links: u32,
    /// A directory's mount status, or anything else's allocated bytes.
    last: u64,
}

impl Packed {
    fn of(kind: u32) -> Packed {
        Packed {
            returned: [u32::MAX; 5],
            error: 0,
            device: 7,
            kind,
            mtime: [1_700_000_000, 5],
            changed: [1_700_000_100, 250],
            flags: 0,
            inode: 42,
            links: 2,
            last: 12_288,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        for set in self.returned {
            out.extend(set.to_ne_bytes());
        }
        out.extend(self.error.to_ne_bytes());
        let reference = out.len();
        out.extend([0u8; 8]);
        out.extend(self.device.to_ne_bytes());
        out.extend(self.kind.to_ne_bytes());
        for time in self.mtime.into_iter().chain(self.changed) {
            out.extend(time.to_ne_bytes());
        }
        out.extend(self.flags.to_ne_bytes());
        out.extend(self.inode.to_ne_bytes());
        out.extend(self.links.to_ne_bytes());
        match self.kind {
            VDIR => out.extend((self.last as u32).to_ne_bytes()),
            _ => out.extend(self.last.to_ne_bytes()),
        }
        let name = b"entry\0";
        let offset = (out.len() - reference) as u32;
        out[reference..reference + 4].copy_from_slice(&offset.to_ne_bytes());
        out[reference + 4..reference + 8].copy_from_slice(&(name.len() as u32).to_ne_bytes());
        out.extend(name);
        let length = out.len() as u32;
        out[..4].copy_from_slice(&length.to_ne_bytes());
        out
    }

    fn decoded(&self) -> Bulk {
        decode_bulk(&self.bytes())
    }
}

fn described(meta: EntryMeta) -> Bulk {
    let name = "entry".into();
    Bulk::Described(Entry { name, meta })
}

#[test]
fn a_bulk_record_gives_a_file_its_metadata() {
    let meta = EntryMeta {
        kind: EntryKind::File,
        blocks: 24,
        links: NonZeroU64::new(2).unwrap(),
        device: 7,
        inode: 42,
        mtime: UnixTime(1_700_000_000),
        changed: Stamp(1_700_000_100_000_000_250),
        dataless: false,
    };
    assert_eq!(Packed::of(VREG).decoded(), described(meta));
    let placeholder = Packed {
        flags: SF_DATALESS,
        ..Packed::of(VREG)
    };
    assert_eq!(
        placeholder.decoded(),
        described(EntryMeta {
            dataless: true,
            ..meta
        })
    );
    let unlinked = Packed {
        links: 0,
        ..Packed::of(VREG)
    };
    assert_eq!(
        unlinked.decoded(),
        described(EntryMeta {
            links: NonZeroU64::MIN,
            ..meta
        })
    );
}

#[test]
fn a_bulk_record_tells_symlinks_directories_and_other_entries_apart() {
    let kind = |packed: Packed| match packed.decoded() {
        Bulk::Described(entry) => (entry.meta.kind, entry.meta.links, entry.meta.blocks),
        Bulk::AskAgain(_) => panic!("described"),
    };
    let two = NonZeroU64::new(2).unwrap();
    assert_eq!(kind(Packed::of(VLNK)), (EntryKind::Symlink, two, 24));
    assert_eq!(kind(Packed::of(VFIFO)), (EntryKind::Other, two, 24));
    let dir = Packed {
        last: 0,
        ..Packed::of(VDIR)
    };
    assert_eq!(kind(dir), (EntryKind::Dir, NonZeroU64::MIN, 0));
}

#[test]
fn what_a_bulk_record_cannot_describe_is_asked_again() {
    let again = Bulk::AskAgain("entry".into());
    let failed = Packed {
        error: libc::EACCES as u32,
        ..Packed::of(VREG)
    };
    let firmlink = Packed {
        flags: SF_FIRMLINK,
        ..Packed::of(VDIR)
    };
    let mount = Packed {
        last: u64::from(libc::DIR_MNTSTATUS_MNTPOINT),
        ..Packed::of(VDIR)
    };
    let mut no_id = Packed::of(VREG);
    no_id.returned[0] &= !libc::ATTR_CMN_FILEID;
    let mut no_mount_status = Packed {
        last: 0,
        ..Packed::of(VDIR)
    };
    no_mount_status.returned[2] &= !libc::ATTR_DIR_MOUNTSTATUS;
    let mut no_size = Packed::of(VREG);
    no_size.returned[3] &= !libc::ATTR_FILE_DATAALLOCSIZE;
    for packed in [failed, firmlink, mount, no_id, no_mount_status, no_size] {
        assert_eq!(packed.decoded(), again);
    }
}

#[test]
fn usage_counts_cpu_time_switches_and_the_peak_rss() {
    let thread = metla_core::fs::thread_cpu();
    let before = metla_core::fs::usage();
    let started = std::time::Instant::now();
    let mut spin = 0u64;
    while started.elapsed() < std::time::Duration::from_millis(30) {
        spin = spin.wrapping_mul(31).wrapping_add(1);
        std::hint::black_box(spin);
    }
    std::thread::sleep(std::time::Duration::from_millis(5));
    let after = metla_core::fs::usage();
    let spent = metla_core::fs::thread_cpu() - thread;
    assert!(spent >= std::time::Duration::from_millis(10), "{spent:?}");
    assert!(after.user + after.system >= before.user + before.system + spent / 2);
    assert!(after.switches > before.switches);
    assert!(after.peak_rss.0 > 1 << 20);
    assert_eq!(peak_rss(), metla_core::fs::usage().peak_rss);
}

#[test]
fn usage_reads_the_cpu_time_the_system_reports() {
    let used = metla_core::fs::usage();
    let mut raw = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let raw = unsafe {
        libc::getrusage(libc::RUSAGE_SELF, raw.as_mut_ptr());
        raw.assume_init()
    };
    let seconds = std::time::Duration::from_secs(raw.ru_utime.tv_sec as u64);
    let after = seconds + std::time::Duration::from_micros(raw.ru_utime.tv_usec as u64);
    assert!(used.user <= after, "{:?} > {after:?}", used.user);
    assert!(after - used.user < std::time::Duration::from_millis(500));
    assert!(used.user >= seconds.saturating_sub(std::time::Duration::from_millis(1)));
}

fn raw_usage() -> libc::rusage {
    let mut raw = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    unsafe {
        libc::getrusage(libc::RUSAGE_SELF, raw.as_mut_ptr());
        raw.assume_init()
    }
}

#[test]
fn usage_counts_the_context_switches_the_system_reports() {
    let switches = |raw: &libc::rusage| (raw.ru_nvcsw + raw.ru_nivcsw).unsigned_abs();
    let before = raw_usage();
    let counted = metla_core::fs::usage().switches;
    let after = raw_usage();
    assert!(switches(&before) > 0, "the process was switched out");
    assert!(switches(&before) <= counted && counted <= switches(&after));
}
