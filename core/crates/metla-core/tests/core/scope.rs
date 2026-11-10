//! The scan scope policy, on synthetic mount tables.

use std::num::NonZeroU64;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use metla_core::domain::{EntryKind, EntryMeta, Skip, Stamp, UnixTime};
use metla_core::fs::Mount;
use metla_core::scan::{ScanRoot, ScanScope};

use crate::support::Fixture;

const SYSTEM: u64 = 7;
const VM: u64 = 8;
const SNAPSHOT: u64 = 9;
const DISK_IMAGE: u64 = 11;

fn dir_on(device: u64) -> EntryMeta {
    EntryMeta {
        kind: EntryKind::Dir,
        blocks: 0,
        links: NonZeroU64::MIN,
        device,
        inode: 1,
        mtime: UnixTime(0),
        changed: Stamp(0),
        dataless: false,
    }
}

fn mount(path: &str, device: u64) -> Mount {
    Mount {
        path: PathBuf::from(path),
        device,
    }
}

/// A volume group: the sealed system snapshot at `/` (its own file-system
/// id, but files report the data volume's device), the data volume both
/// behind firmlinks and at its own mount point, and unrelated volumes.
fn mounts() -> Vec<Mount> {
    vec![
        mount("/", SNAPSHOT),
        mount("/System/Volumes/VM", VM),
        mount("/System/Volumes/Data", SYSTEM),
        mount("/Volumes/Image", DISK_IMAGE),
    ]
}

const FIRMLINKS: &str = "/Users\tUsers\n/Library\tLibrary\n\
                         /System/Library/Caches\tSystem/Library/Caches\n";

fn decide(scope: &ScanScope, path: &str) -> Option<Skip> {
    scope.decide(Path::new(path), &dir_on(SYSTEM))
}

#[test]
fn the_data_volume_mount_is_walked_without_the_firmlinked_directories() {
    let scope = ScanScope::plan(Path::new("/"), SYSTEM, &mounts(), Some(FIRMLINKS));
    assert_eq!(
        decide(&scope, "/System/Volumes/Data/Users"),
        Some(Skip::Excluded)
    );
    assert_eq!(
        decide(&scope, "/System/Volumes/Data/Library"),
        Some(Skip::Excluded)
    );
    assert_eq!(
        decide(&scope, "/System/Volumes/Data/System/Library/Caches"),
        Some(Skip::Excluded)
    );
    assert_eq!(decide(&scope, "/System/Volumes/Data"), None);
    assert_eq!(decide(&scope, "/System/Volumes/Data/.Spotlight-V100"), None);
    assert_eq!(decide(&scope, "/Users"), None);
    assert_eq!(decide(&scope, "/Library"), None);
}

#[test]
fn without_a_firmlink_list_the_second_mount_is_skipped_whole() {
    let scope = ScanScope::plan(Path::new("/"), SYSTEM, &mounts(), None);
    assert_eq!(
        decide(&scope, "/System/Volumes/Data"),
        Some(Skip::MountPoint)
    );
    assert_eq!(decide(&scope, "/Users"), None);
}

#[test]
fn the_magic_directories_of_the_system_root_are_excluded() {
    let scope = ScanScope::plan(Path::new("/"), SYSTEM, &[], None);
    for magic in ["/.vol", "/.nofollow", "/.resolve"] {
        assert_eq!(decide(&scope, magic), Some(Skip::Excluded), "{magic}");
    }
    let elsewhere = ScanScope::plan(Path::new("/tmp/x"), SYSTEM, &[], None);
    assert_eq!(decide(&elsewhere, "/tmp/x/.vol"), None);
}

#[test]
fn other_devices_are_other_volumes() {
    let scope = ScanScope::plan(Path::new("/"), SYSTEM, &mounts(), Some(FIRMLINKS));
    for device in [VM, DISK_IMAGE, SNAPSHOT] {
        assert_eq!(
            scope.decide(Path::new("/anything"), &dir_on(device)),
            Some(Skip::OtherVolume)
        );
    }
}

#[test]
fn a_dataless_directory_is_never_descended() {
    let scope = ScanScope::new(SYSTEM);
    let placeholder = EntryMeta {
        dataless: true,
        ..dir_on(SYSTEM)
    };
    assert_eq!(
        scope.decide(Path::new("/cloud"), &placeholder),
        Some(Skip::Dataless)
    );
}

#[test]
fn only_firmlinks_from_inside_the_root_are_excluded() {
    let scope = ScanScope::plan(Path::new("/System"), SYSTEM, &mounts(), Some(FIRMLINKS));
    assert_eq!(decide(&scope, "/System/Volumes/Data/Users"), None);
    assert_eq!(
        decide(&scope, "/System/Volumes/Data/System/Library/Caches"),
        Some(Skip::Excluded)
    );
}

#[test]
fn the_root_mount_itself_and_mounts_outside_the_root_change_nothing() {
    let data = ScanScope::plan(
        Path::new("/System/Volumes/Data"),
        SYSTEM,
        &mounts(),
        Some(FIRMLINKS),
    );
    assert_eq!(decide(&data, "/System/Volumes/Data/Users"), None);
    let home = ScanScope::plan(Path::new("/Users/someone"), SYSTEM, &mounts(), None);
    assert_eq!(decide(&home, "/System/Volumes/Data"), None);
    assert_eq!(home, ScanScope::new(SYSTEM));
}

#[test]
fn the_detected_scope_of_an_ordinary_directory_is_its_device() {
    let fx = Fixture::new();
    let root = ScanRoot::new(fx.root()).unwrap();
    let device = std::fs::metadata(fx.root()).unwrap().dev();
    assert_eq!(root.device(), device);
    assert_eq!(ScanScope::detect(&root).unwrap(), ScanScope::new(device));
}

#[test]
fn a_file_is_not_a_scan_root() {
    let fx = Fixture::new();
    std::fs::write(fx.path("file"), b"x").unwrap();
    assert!(ScanRoot::new(&fx.path("file")).is_err());
}

#[test]
fn the_excluded_paths_are_listed_and_second_mounts_are_not() {
    let scope = ScanScope::new(SYSTEM)
        .exclude(PathBuf::from("/a"))
        .skip_mount(PathBuf::from("/m"))
        .exclude(PathBuf::from("/b"));
    let excluded: Vec<&Path> = scope.excluded().collect();
    assert_eq!(excluded, [Path::new("/a"), Path::new("/b")]);
}
