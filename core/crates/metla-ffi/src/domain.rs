//! The domain's types as they cross the boundary: declared again for
//! UniFFI (`remote`), which converts them field by field and fails to
//! compile when a declaration and the core's type differ. Newtypes cross
//! as their number, paths and names as strings (APFS names are UTF-8).

use std::ffi::OsString;
use std::path::PathBuf;

use metla_core::clean::CleanProgress;
use metla_core::domain::{
    Bytes, Candidate, Category, CleanupPreview, CleanupReport, DirId, Disposal, Errno, FileKind,
    GapKind, Group, ItemReport, Manager, Outcome, PreviewId, PreviewItem, ProgressSnapshot,
    QuickCategory, Reason, Reasons, Reclaimable, Rejected, Rejection, RuleId, Separate, Skip,
    SkipReason, Tally, TreeTotals, UnixTime, Warning, WorktreeStatus,
};
use metla_core::engine::{
    Changed, Completeness, DirRef, EngineState, Failure, Freshness, HotkeyMode, Listed,
    PowerProfile, Removability, TopFolder, VolumeSpace,
};
use metla_core::index::{Order, SortKey};
use uniffi::deps::anyhow::anyhow;

uniffi::custom_type!(Bytes, u64, {
    remote,
    lower: |bytes| bytes.0,
    try_lift: |bytes| Ok(Bytes(bytes)),
});

uniffi::custom_type!(UnixTime, i64, {
    remote,
    lower: |time| time.0,
    try_lift: |time| Ok(UnixTime(time)),
});

uniffi::custom_type!(DirId, i64, {
    remote,
    lower: |id| id.0,
    try_lift: |id| Ok(DirId(id)),
});

uniffi::custom_type!(PreviewId, u64, {
    remote,
    lower: |id| id.0,
    try_lift: |id| Ok(PreviewId(id)),
});

uniffi::custom_type!(Errno, i32, {
    remote,
    lower: |errno| errno.0,
    try_lift: |errno| Ok(Errno(errno)),
});

uniffi::custom_type!(PathBuf, String, {
    remote,
    lower: |path| path.to_string_lossy().into_owned(),
    try_lift: |path| Ok(PathBuf::from(path)),
});

uniffi::custom_type!(OsString, String, {
    remote,
    lower: |name| name.to_string_lossy().into_owned(),
    try_lift: |name| Ok(OsString::from(name)),
});

uniffi::custom_type!(RuleId, String, {
    remote,
    lower: |rule| rule.as_str().to_owned(),
    try_lift: |id| RuleId::parse(&id).ok_or_else(|| anyhow!("no rule id: {id}")),
});

uniffi::custom_type!(Reasons, Vec<Reason>, {
    remote,
    lower: |reasons| reasons.iter().cloned().collect(),
    try_lift: |reasons| match WorktreeStatus::new(None, reasons.into_iter().collect()) {
        WorktreeStatus::Blocked { reasons, .. } => Ok(reasons),
        WorktreeStatus::Removable { .. } => Err(anyhow!("no reason")),
    },
});

#[uniffi::remote(Enum)]
pub enum Category {
    AppCache,
    DevCache,
    BuildArtifact,
    Log,
    GitWorktree,
    Temp,
}

/// No value for logs, worktrees or temporary data: Swift cannot ask to
/// quick-clean them.
#[uniffi::remote(Enum)]
pub enum QuickCategory {
    AppCache,
    DevCache,
    BuildArtifact,
}

#[uniffi::remote(Enum)]
pub enum Disposal {
    DeletePermanently,
    MoveToTrash,
    GitWorktreeRemove,
}

#[uniffi::remote(Enum)]
pub enum FileKind {
    Applications,
    Developer,
    Media,
    Documents,
    Archives,
    System,
    Other,
}

#[uniffi::remote(Enum)]
pub enum GapKind {
    Denied,
    Failed,
    Vanished,
    Cancelled,
}

#[uniffi::remote(Enum)]
pub enum Skip {
    OtherVolume,
    MountPoint,
    Dataless,
    Excluded,
}

#[uniffi::remote(Enum)]
pub enum Manager {
    Codex,
    Claude,
}

#[uniffi::remote(Enum)]
pub enum Reason {
    Uncommitted,
    Untracked,
    Unpushed,
    Locked,
    Prunable,
    OperationInProgress,
    InUse,
    Managed(Manager),
    NotListed,
    InspectionFailed(String),
}

#[uniffi::remote(Enum)]
pub enum PowerProfile {
    Interactive,
    Background,
    Saver,
    Off,
}

#[uniffi::remote(Enum)]
pub enum Completeness {
    Missing,
    Partial,
    Complete,
}

/// What changed: the observer's coarse topic.
#[uniffi::remote(Enum)]
pub enum Changed {
    Picture,
    Activity,
    Worktrees,
    Cleanup,
    Settings,
    Exclusions,
}

#[uniffi::remote(Enum)]
pub enum Failure {
    NoSpace,
    Index,
    Scan,
}

#[uniffi::remote(Record)]
pub struct ProgressSnapshot {
    pub dirs: u64,
    pub files: u64,
    pub bytes: Bytes,
    pub denied: u64,
}

#[uniffi::remote(Enum)]
pub enum EngineState {
    Idle,
    Scanning {
        progress: ProgressSnapshot,
        interactive: bool,
    },
    Updating {
        pending: u64,
    },
    Cleaning,
    Paused(PowerProfile),
    Failed(Failure),
    Off,
}

#[uniffi::remote(Record)]
pub struct Freshness {
    pub full_scan: Option<UnixTime>,
    pub updated: Option<UnixTime>,
    pub pending: u64,
    pub state: EngineState,
    pub picture: Completeness,
    pub watching: bool,
    pub needs_full_scan: bool,
}

#[uniffi::remote(Record)]
pub struct Tally {
    pub count: u64,
    pub bytes: Bytes,
}

#[uniffi::remote(Record)]
pub struct Reclaimable {
    pub freed: Tally,
    pub trashed: Tally,
}

#[uniffi::remote(Record)]
pub struct VolumeSpace {
    pub total: Bytes,
    pub available: Bytes,
}

#[uniffi::remote(Record)]
pub struct TreeTotals {
    pub bytes: Bytes,
    pub files: u64,
    pub dirs: u64,
    pub newest: Option<UnixTime>,
}

#[uniffi::remote(Record)]
pub struct TopFolder {
    pub path: PathBuf,
    pub bytes: Bytes,
    pub files: u64,
    pub counting: bool,
}

#[uniffi::remote(Record)]
pub struct Candidate {
    pub id: DirId,
    pub path: PathBuf,
    pub rule: RuleId,
    pub bytes: Bytes,
    pub files: u64,
    pub activity: Option<UnixTime>,
    pub device: u64,
    pub inode: u64,
    pub managed: Option<Manager>,
    pub preselected: bool,
}

#[uniffi::remote(Enum)]
pub enum Removability {
    Unknown,
    Removable {
        branch: Option<String>,
    },
    Blocked {
        branch: Option<String>,
        reasons: Reasons,
    },
}

#[uniffi::remote(Record)]
pub struct Listed {
    pub candidate: Candidate,
    pub removability: Option<Removability>,
}

#[uniffi::remote(Enum)]
pub enum SortKey {
    Size,
    Activity,
    Path,
}

#[uniffi::remote(Record)]
pub struct Order {
    pub key: SortKey,
    pub descending: bool,
}

#[uniffi::remote(Enum)]
pub enum DirRef {
    Id(DirId),
    Path(PathBuf),
}

#[uniffi::remote(Enum)]
pub enum Separate {
    MoveToTrash,
    EmptyTrash,
    RemoveWorktrees,
}

#[uniffi::remote(Enum)]
pub enum Group {
    Quick,
    Separate(Separate),
}

#[uniffi::remote(Enum)]
pub enum HotkeyMode {
    Confirm,
    Immediate,
}

#[uniffi::remote(Enum)]
pub enum Warning {
    RecentlyActive,
    InsideManagedWorktree(Manager),
}

#[uniffi::remote(Record)]
pub struct PreviewItem {
    pub candidate: Candidate,
    pub warnings: Vec<Warning>,
}

#[uniffi::remote(Enum)]
pub enum Rejection {
    Blocked(Reasons),
    Nested { inside: DirId },
}

#[uniffi::remote(Record)]
pub struct Rejected {
    pub candidate: Candidate,
    pub why: Rejection,
}

#[uniffi::remote(Record)]
pub struct CleanupPreview {
    pub id: PreviewId,
    pub items: Vec<PreviewItem>,
    pub rejected: Vec<Rejected>,
}

#[uniffi::remote(Record)]
pub struct CleanProgress {
    pub items: u64,
    pub done: u64,
    pub removed: Bytes,
    pub current: Option<PathBuf>,
}

#[uniffi::remote(Enum)]
pub enum SkipReason {
    NotConfirmed,
    Cancelled,
    Excluded,
    OutsideRoot,
    Symlink,
    NotADirectory,
    Replaced,
    MountPoint,
    AncestorChanged,
    RuleMismatch,
    NotIgnored,
    HasTrackedFiles,
    Unverifiable,
    Blocked(Reasons),
    Unreadable(Errno),
}

#[uniffi::remote(Enum)]
pub enum Outcome {
    Removed,
    Partial(Leftovers),
    Skipped(SkipReason),
    Failed(String),
    AlreadyGone,
    MovedToTrash,
}

#[uniffi::remote(Record)]
pub struct ItemReport {
    pub candidate: DirId,
    pub path: PathBuf,
    pub rule: RuleId,
    pub found: Bytes,
    pub removed: Bytes,
    pub outcome: Outcome,
}

#[uniffi::remote(Record)]
pub struct CleanupReport {
    pub preview: PreviewId,
    pub items: Vec<ItemReport>,
    pub free_before: Bytes,
    pub free_after: Bytes,
}

use metla_core::domain::Leftovers;

uniffi::custom_type!(Leftovers, crate::view::Leftover, {
    remote,
    lower: |leftovers| crate::view::Leftover::from(leftovers),
    try_lift: |_leftover| Err(anyhow!("leftovers only come from a cleanup")),
});
