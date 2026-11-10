//! A cleanup: the preview the user confirms, the confirmation, and the
//! report of what happened.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::candidate::{Candidate, Tally};
use super::dir::{DirId, Errno};
use super::entry::Bytes;
use super::rule::{Category, Disposal, RuleId};
use super::worktree::{Manager, Reasons};

/// Identifies one preview. A preview is executed at most once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PreviewId(pub u64);

/// Items that are removed the same way and confirmed together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// Caches and build output, deleted permanently: confirming the
    /// preview confirms them.
    Quick,
    /// Everything else needs a confirmation of its own.
    Separate(Separate),
}

/// A group that needs its own confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Separate {
    /// Logs and temporary data, moved to the Trash.
    MoveToTrash,
    /// The Trash's contents, deleted permanently.
    EmptyTrash,
    /// Linked worktrees, removed by `git worktree remove`.
    RemoveWorktrees,
}

impl Group {
    /// The group of a candidate of this rule: the single place this is
    /// decided.
    pub fn of(rule: &RuleId) -> Group {
        match (rule.category().quick(), rule.disposal()) {
            (Some(_), _) => Group::Quick,
            (None, Disposal::MoveToTrash) => Group::Separate(Separate::MoveToTrash),
            (None, Disposal::DeletePermanently) => Group::Separate(Separate::EmptyTrash),
            (None, Disposal::GitWorktreeRemove) => Group::Separate(Separate::RemoveWorktrees),
        }
    }
}

/// Something the user should know before confirming an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Warning {
    /// Build output that changed within the recent-activity age.
    RecentlyActive,
    /// Inside a worktree another tool manages: confirmed one by one.
    InsideManagedWorktree(Manager),
}

/// One thing a cleanup will remove, as shown before it is confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewItem {
    pub candidate: Candidate,
    pub warnings: Vec<Warning>,
}

impl PreviewItem {
    pub fn path(&self) -> &Path {
        &self.candidate.path
    }

    pub fn rule(&self) -> &RuleId {
        &self.candidate.rule
    }

    pub fn category(&self) -> Category {
        self.candidate.category()
    }

    pub fn disposal(&self) -> Disposal {
        self.candidate.disposal()
    }

    /// The size when scanned: an estimate.
    pub fn estimated(&self) -> Bytes {
        self.candidate.bytes
    }

    pub fn group(&self) -> Group {
        Group::of(&self.candidate.rule)
    }

    /// Whether the item must be confirmed on its own: it lies inside a
    /// worktree another tool manages.
    pub fn needs_own_confirmation(&self) -> bool {
        self.candidate.managed.is_some()
    }
}

/// Why a selected candidate is not in the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// A worktree that cannot be removed, and why.
    Blocked(Reasons),
    /// It lies inside `inside`, another item of the preview, which removes
    /// it too.
    Nested { inside: DirId },
}

/// A selected candidate the preview does not include.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub candidate: Candidate,
    pub why: Rejection,
}

/// Exactly what a cleanup will do, for the user to confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupPreview {
    pub id: PreviewId,
    pub items: Vec<PreviewItem>,
    pub rejected: Vec<Rejected>,
}

impl CleanupPreview {
    /// How many items and how many estimated bytes each group holds.
    pub fn totals(&self) -> BTreeMap<Group, Tally> {
        let mut totals = BTreeMap::<Group, Tally>::new();
        for item in &self.items {
            totals
                .entry(item.group())
                .or_default()
                .add(item.estimated());
        }
        totals
    }
}

/// What the user confirmed on top of the preview itself: the groups that
/// need their own confirmation, and the items that need one each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Confirmation {
    pub groups: BTreeSet<Separate>,
    pub items: BTreeSet<DirId>,
}

impl Confirmation {
    /// Whether the item of the confirmed preview may be acted on.
    pub fn covers(&self, item: &PreviewItem) -> bool {
        let group = match item.group() {
            Group::Quick => true,
            Group::Separate(separate) => self.groups.contains(&separate),
        };
        let own = !item.needs_own_confirmation() || self.items.contains(&item.candidate.id);
        group && own
    }
}

/// Why an item was left alone. Every check made right before acting has
/// its own reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Its group, or the item itself, was not confirmed.
    NotConfirmed,
    /// The cleanup was cancelled before the item was reached.
    Cancelled,
    /// An exclusion keeps it, something inside it, or something above it.
    Excluded,
    /// Its path does not lie inside the root.
    OutsideRoot,
    /// It is a symlink now.
    Symlink,
    /// It is no longer a directory.
    NotADirectory,
    /// Another directory is there now (its device or inode changed).
    Replaced,
    /// Another volume is mounted there.
    MountPoint,
    /// A directory above it was replaced by a symlink or a file.
    AncestorChanged,
    /// Its rule no longer matches what is there.
    RuleMismatch,
    /// Git does not ignore it any more.
    NotIgnored,
    /// Git tracks files inside it now.
    HasTrackedFiles,
    /// Git could not say.
    Unverifiable,
    /// The worktree cannot be removed now, and why.
    Blocked(Reasons),
    /// It could not be read to check it.
    Unreadable(Errno),
}

/// What could not be removed from an item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Leftovers {
    /// Entries that could not be removed.
    pub failures: u64,
    /// The first of them, with the error.
    pub first: Vec<(PathBuf, Errno)>,
    /// Directories on another device, left in place.
    pub mounts: Vec<PathBuf>,
    /// The cleanup was cancelled in the middle of the item.
    pub cancelled: bool,
}

impl Leftovers {
    /// How many failures are kept with their path.
    pub const FIRST: usize = 16;

    pub fn fail(&mut self, path: PathBuf, errno: Errno) {
        self.failures += 1;
        if self.first.len() < Leftovers::FIRST {
            self.first.push((path, errno));
        }
    }

    /// Nothing stayed.
    pub fn is_empty(&self) -> bool {
        self.failures == 0 && self.mounts.is_empty() && !self.cancelled
    }
}

/// What happened to one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Removed,
    /// Some of it was removed.
    Partial(Leftovers),
    Skipped(SkipReason),
    Failed(String),
    /// It was not there any more: nothing to do.
    AlreadyGone,
    /// Moved to the Trash: nothing is freed until the Trash is emptied.
    MovedToTrash,
}

impl Outcome {
    /// Whether the item may have changed on disk, so the picture no longer
    /// describes it.
    pub fn changed(&self) -> bool {
        !matches!(self, Outcome::Skipped(_))
    }
}

/// One item of a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemReport {
    pub candidate: DirId,
    pub path: PathBuf,
    pub rule: RuleId,
    /// The size when scanned (an estimate).
    pub found: Bytes,
    /// Allocated bytes of what was actually removed.
    pub removed: Bytes,
    pub outcome: Outcome,
}

/// What a cleanup did. Found, removed and freed come from three
/// different measurements and are never derived from one another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupReport {
    pub preview: PreviewId,
    pub items: Vec<ItemReport>,
    /// The volume's available space before and after, as measured.
    pub free_before: Bytes,
    pub free_after: Bytes,
}

impl CleanupReport {
    /// The estimated size of every item.
    pub fn found(&self) -> Bytes {
        self.items.iter().map(|item| item.found).sum()
    }

    /// What was actually removed.
    pub fn removed(&self) -> Bytes {
        self.items.iter().map(|item| item.removed).sum()
    }

    /// The estimated size of what went to the Trash, not freed yet.
    pub fn moved_to_trash(&self) -> Bytes {
        let moved = self
            .items
            .iter()
            .filter(|i| i.outcome == Outcome::MovedToTrash);
        moved.map(|item| item.found).sum()
    }

    /// The measured change in available space; negative when something
    /// else used more meanwhile.
    pub fn freed(&self) -> i128 {
        i128::from(self.free_after.0) - i128::from(self.free_before.0)
    }
}
