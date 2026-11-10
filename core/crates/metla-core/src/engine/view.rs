//! What the engine answers: plain data the UI shows as it is.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use super::profile::PowerProfile;
use crate::domain::{
    Bytes, Candidate, Category, Coverage, DirId, DirRecord, KindTotals, ProgressSnapshot, Reasons,
    Reclaimable, Tally, TreeTotals, UnixTime,
};

/// What changed; the observer then asks for what it shows. Carries no
/// data, so a notification that is merged or lost does no harm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Changed {
    /// The picture: totals, candidates, folders.
    Picture,
    /// What the engine is doing: its state and progress.
    Activity,
    /// A batch of worktree statuses is ready.
    Worktrees,
    /// A cleanup progressed or finished.
    Cleanup,
    Settings,
    Exclusions,
}

/// Told when something changed. Called on engine threads.
pub trait EngineObserver: Send + Sync {
    fn changed(&self, what: Changed);
}

/// What the engine is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineState {
    /// Up to date as far as it knows, nothing to do.
    Idle,
    /// A full scan runs.
    Scanning {
        progress: ProgressSnapshot,
        /// The user asked for it.
        interactive: bool,
    },
    /// Changes are being applied or wait for their settle time, or a
    /// full scan the engine needs is about to start.
    Updating { pending: u64 },
    /// A cleanup runs; rescans wait for it.
    Cleaning,
    /// A full scan is needed and the profile does not allow it now.
    Paused(PowerProfile),
    /// The last full scan or update failed for this reason; the picture
    /// stays as it was, and no full scan starts until the user asks.
    Failed(Failure),
    /// Background analysis is off: nothing is watched.
    Off,
}

/// Why the picture could not be brought up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The disk had no room for the index ("not enough disk space to
    /// refresh").
    NoSpace,
    /// The index could not be written or read.
    Index,
    /// The root could not be scanned.
    Scan,
}

/// How much of the disk the picture saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    /// No scan has finished.
    Missing,
    /// It has gaps: directories it could not read.
    Partial,
    Complete,
}

/// When the picture was last true, and whether it still is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Freshness {
    /// When the last full scan finished.
    pub full_scan: Option<UnixTime>,
    /// When the picture last changed: that scan, or a later update.
    pub updated: Option<UnixTime>,
    /// Changes seen and not applied yet.
    pub pending: u64,
    pub state: EngineState,
    pub picture: Completeness,
    /// Changes on disk are being followed.
    pub watching: bool,
    /// Only a full scan can make the picture right again (lost history,
    /// an interrupted scan, a scan never made).
    pub needs_full_scan: bool,
}

impl Freshness {
    /// Whether the picture is complete and shows the disk as it is now,
    /// as far as the engine can know.
    pub fn is_current(&self) -> bool {
        let busy = matches!(self.state, EngineState::Scanning { .. });
        self.picture == Completeness::Complete
            && self.pending == 0
            && self.watching
            && !self.needs_full_scan
            && !busy
    }
}

/// The volume the root is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeSpace {
    pub total: Bytes,
    pub available: Bytes,
}

/// Everything the Overview shows, from one read of the picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overview {
    pub root: PathBuf,
    pub volume: Option<VolumeSpace>,
    /// The root's totals; `None` without a picture.
    pub totals: Option<TreeTotals>,
    /// Every offered candidate, per category.
    pub categories: BTreeMap<Category, Tally>,
    /// What cleaning the preselected candidates would do: in all, and per
    /// category (every category present; they add up to the total).
    pub reclaimable: Reclaimable,
    pub reclaimable_by_category: BTreeMap<Category, Reclaimable>,
    pub kinds: KindTotals,
    pub coverage: Option<Coverage>,
    pub freshness: Freshness,
    pub profile: PowerProfile,
}

/// Whether a worktree in a list can be selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Removability {
    /// Not computed yet.
    Unknown,
    Removable {
        branch: Option<String>,
    },
    Blocked {
        branch: Option<String>,
        reasons: Reasons,
    },
}

/// A candidate as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub candidate: Candidate,
    /// For a worktree; `None` for everything else.
    pub removability: Option<Removability>,
}

/// A directory of the picture, by id or by path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirRef {
    Id(DirId),
    Path(PathBuf),
}

/// One of the Overview's largest folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopFolder {
    pub path: PathBuf,
    pub bytes: Bytes,
    pub files: u64,
    /// From the live tree of a first scan: still growing.
    pub counting: bool,
}

/// One page of candidates as a list shows them, and how many are offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidatePage {
    pub listed: Vec<Listed>,
    pub total: usize,
}

/// One entry of a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderEntry {
    /// A subdirectory, from the picture.
    Dir(Box<DirRecord>),
    /// A file (anything but a directory), listed now.
    File { name: OsString, bytes: Bytes },
    /// The files past the most a listing keeps: "N more files". Always
    /// last.
    MoreFiles { files: u64, bytes: Bytes },
}

/// One page of a directory's contents, largest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub dir: DirRecord,
    pub path: PathBuf,
    pub entries: Vec<FolderEntry>,
    /// Entries on every page.
    pub total: usize,
}

/// What the engine has done since it opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub full_scans: u64,
    /// Rescan tasks applied in place.
    pub tasks: u64,
    /// Batches of tasks.
    pub batches: u64,
    /// File-system events received.
    pub events: u64,
    /// Times the scheduler woke up: for a command, an event batch or a
    /// deadline. An idle engine does not wake up.
    pub wakeups: u64,
    /// FSEvents streams started: one at opening, and one more for each
    /// change of latency.
    pub streams: u64,
}

impl Counters {
    /// What the engine did between `earlier` and these counters.
    pub fn since(&self, earlier: &Counters) -> Counters {
        Counters {
            full_scans: self.full_scans.saturating_sub(earlier.full_scans),
            tasks: self.tasks.saturating_sub(earlier.tasks),
            batches: self.batches.saturating_sub(earlier.batches),
            events: self.events.saturating_sub(earlier.events),
            wakeups: self.wakeups.saturating_sub(earlier.wakeups),
            streams: self.streams.saturating_sub(earlier.streams),
        }
    }
}
