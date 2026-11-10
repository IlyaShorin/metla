use std::ffi::OsString;

use super::entry::{Stamp, UnixTime};
use super::kind::Area;
use super::rule::RuleId;
use super::totals::{OwnTotals, TreeTotals};

/// Identifies a directory within one scan's picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DirId(pub i64);

/// An operating-system error number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Errno(pub i32);

/// Why a directory the scan wanted to read was not read. Any gap makes the
/// scan partial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Gap {
    /// `EACCES` / `EPERM`: no permission (or no Full Disk Access).
    Denied,
    /// Any other error while opening or listing the directory.
    Failed(Errno),
    /// The directory disappeared between being listed and being read.
    Vanished,
    /// The scan was cancelled before reaching the directory.
    Cancelled,
}

/// A gap without its details, for counting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GapKind {
    Denied,
    Failed,
    Vanished,
    Cancelled,
}

impl GapKind {
    /// Every kind, in a fixed order.
    pub const ALL: [GapKind; 4] = [
        GapKind::Denied,
        GapKind::Failed,
        GapKind::Vanished,
        GapKind::Cancelled,
    ];
}

impl Gap {
    pub fn kind(self) -> GapKind {
        match self {
            Gap::Denied => GapKind::Denied,
            Gap::Failed(_) => GapKind::Failed,
            Gap::Vanished => GapKind::Vanished,
            Gap::Cancelled => GapKind::Cancelled,
        }
    }
}

/// Why the scan deliberately did not descend into a directory. Skips are
/// counted but do not make a scan partial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    /// The directory belongs to a device outside the scan scope (another
    /// volume mounted here, a disk image, a network share).
    OtherVolume,
    /// A second mount of a volume that is already being walked.
    MountPoint,
    /// A cloud placeholder; reading it would download it.
    Dataless,
    /// A system path the scope excludes, because it is reached elsewhere or
    /// is not a real directory.
    Excluded,
}

impl Skip {
    /// Every skip, in a fixed order.
    pub const ALL: [Skip; 4] = [
        Skip::OtherVolume,
        Skip::MountPoint,
        Skip::Dataless,
        Skip::Excluded,
    ];
}

/// What the scan knows about a directory. Only a directory that was read
/// has own totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirState {
    Read(OwnTotals),
    Gap(Gap),
    Skipped(Skip),
}

/// What a directory that was read hands down to the classification of the
/// directories below it. A directory that was not read hands down nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Passed {
    /// Its entries the rules want as markers (none inside a candidate).
    pub markers: Vec<OsString>,
    /// It is, or is inside, a Git repository.
    pub in_repo: bool,
    /// It is, or is inside, a candidate other than a linked worktree:
    /// nothing below it is classified.
    pub in_candidate: bool,
    /// Its area for file kinds.
    pub area: Area,
}

/// One directory of the picture: a row of the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirRecord {
    pub id: DirId,
    /// `None` for the root.
    pub parent: Option<DirId>,
    /// Empty for the root.
    pub name: OsString,
    /// The directory's device, inode, modification time and change stamp
    /// when it was listed.
    pub device: u64,
    pub inode: u64,
    pub mtime: UnixTime,
    pub changed: Stamp,
    pub state: DirState,
    pub tree: TreeTotals,
    /// The rule the directory matched: it is a candidate.
    pub rule: Option<RuleId>,
    /// What it hands down to the directories below it.
    pub passed: Passed,
}
