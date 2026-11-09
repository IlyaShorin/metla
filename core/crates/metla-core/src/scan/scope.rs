//! Which directories a scan descends into: policy as data.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use super::ScanRoot;
use crate::domain::{EntryMeta, Skip};
use crate::fs::{Mount, mounts};

/// Where macOS lists its firmlinks: `source<TAB>target` per line, the
/// source absolute on the system volume, the target relative to the data
/// volume's root.
const FIRMLINKS: &str = "/usr/share/firmlinks";

/// Entries of `/` that are views of the file system, not directories.
const MAGIC_ROOT_DIRS: [&str; 3] = [".vol", ".nofollow", ".resolve"];

/// The devices a scan stays on and the paths it does not descend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanScope {
    devices: BTreeSet<u64>,
    paths: BTreeMap<PathBuf, Skip>,
}

impl ScanScope {
    /// Stay on `device`; descend everywhere on it.
    pub fn new(device: u64) -> ScanScope {
        ScanScope {
            devices: BTreeSet::from([device]),
            paths: BTreeMap::new(),
        }
    }

    /// Do not descend `path`: it is reached elsewhere or is not a real
    /// directory.
    pub fn exclude(self, path: PathBuf) -> ScanScope {
        self.skip_path(path, Skip::Excluded)
    }

    /// Do not descend `path`: a second mount of a volume being walked.
    pub fn skip_mount(self, path: PathBuf) -> ScanScope {
        self.skip_path(path, Skip::MountPoint)
    }

    fn skip_path(mut self, path: PathBuf, skip: Skip) -> ScanScope {
        self.paths.insert(path, skip);
        self
    }

    /// Why the directory at `path` is not descended, if it is not.
    pub fn decide(&self, path: &Path, meta: &EntryMeta) -> Option<Skip> {
        if meta.dataless {
            return Some(Skip::Dataless);
        }
        if !self.devices.contains(&meta.device) {
            return Some(Skip::OtherVolume);
        }
        self.paths.get(path).copied()
    }

    /// The scope for `root` on `device`, given the mounted file systems and
    /// the firmlink list.
    ///
    /// In an APFS volume group the system and data volumes report the same
    /// device, the data volume is reachable through firmlinks (`/Users`,
    /// `/Applications`, ...) and is also mounted at `/System/Volumes/Data`.
    /// Such a second mount of the root's device is walked for what only it
    /// holds, minus the firmlink targets already reached through the root;
    /// without a firmlink list it is skipped whole.
    pub fn plan(root: &Path, device: u64, mounts: &[Mount], firmlinks: Option<&str>) -> ScanScope {
        let mut scope = ScanScope::new(device);
        if root == Path::new("/") {
            for name in MAGIC_ROOT_DIRS {
                scope = scope.exclude(root.join(name));
            }
        }
        let twins = mounts
            .iter()
            .filter(|m| m.device == device && m.path != root && m.path.starts_with(root));
        for twin in twins {
            scope = match firmlinks {
                Some(list) => firmlink_targets(list, root)
                    .fold(scope, |scope, target| scope.exclude(twin.path.join(target))),
                None => scope.skip_mount(twin.path.clone()),
            };
        }
        scope
    }

    /// The scope for `root` on this machine.
    pub fn detect(root: &ScanRoot) -> io::Result<ScanScope> {
        let firmlinks = std::fs::read_to_string(FIRMLINKS).ok();
        Ok(ScanScope::plan(
            root.path(),
            root.device(),
            &mounts()?,
            firmlinks.as_deref(),
        ))
    }
}

/// The targets of the firmlinks whose source lies under `root`.
fn firmlink_targets<'a>(list: &'a str, root: &'a Path) -> impl Iterator<Item = &'a str> {
    list.lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(move |(source, _)| Path::new(source).starts_with(root))
        .map(|(_, target)| target)
}
