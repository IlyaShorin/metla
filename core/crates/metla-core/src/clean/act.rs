//! Acting on an item that passed revalidation, as its disposal says.

use std::path::{Path, PathBuf};

use super::TrashHost;
use super::check::Opened;
use super::control::CleanControl;
use crate::domain::{
    Bytes, Disposal, EntryKind, Errno, Outcome, PreviewItem, SkipReason, UnixTime, WorktreeStatus,
};
use crate::fs::{ListError, list_dir, remove_contents, survey};
use crate::git::{Git, WorktreeSettings, inspect_worktree, remove_worktree};
use crate::rules::{Removes, RuleSet};

/// What acting needs.
pub struct Actor<'a> {
    pub rules: &'a RuleSet,
    pub git: &'a Git,
    pub worktrees: &'a WorktreeSettings,
    pub now: UnixTime,
    pub control: &'a CleanControl,
    pub trash: &'a dyn TrashHost,
}

impl Actor<'_> {
    /// Removes the item; returns what happened and how many bytes went.
    pub fn act(&self, item: &PreviewItem, opened: Opened) -> (Outcome, Bytes) {
        match item.disposal() {
            Disposal::DeletePermanently => self.delete(item, opened),
            Disposal::MoveToTrash => (self.move_to_trash(item, &opened), Bytes::default()),
            Disposal::GitWorktreeRemove => self.remove_worktree(item, &opened),
        }
    }

    /// Whether the item's rule keeps the directory and removes only what
    /// it holds.
    fn keeps(&self, item: &PreviewItem) -> bool {
        let rule = self.rules.rule(item.rule());
        rule.is_some_and(|rule| rule.removes == Removes::Contents)
    }

    /// Deletes what the directory holds, then the directory itself (once
    /// empty, unless its rule keeps it).
    fn delete(&self, item: &PreviewItem, opened: Opened) -> (Outcome, Bytes) {
        let cancelled = || self.control.is_cancelled();
        let mut removal = remove_contents(&opened.dir, item.candidate.device, &cancelled);
        if removal.leftovers.is_empty() && !self.keeps(item) {
            drop(opened.dir);
            match opened.parent.remove(&opened.name, EntryKind::Dir) {
                Ok(()) | Err(Errno(libc::ENOENT)) => {}
                Err(error) => removal.leftovers.fail(PathBuf::from("."), error),
            }
        }
        let outcome = match removal.leftovers.is_empty() {
            true => Outcome::Removed,
            false => Outcome::Partial(removal.leftovers),
        };
        (outcome, removal.removed)
    }

    /// Hands the directory, or what it holds when its rule keeps it, to
    /// the host's Trash.
    fn move_to_trash(&self, item: &PreviewItem, opened: &Opened) -> Outcome {
        let targets = match self.keeps(item) {
            true => contents(item.path(), opened),
            false => Ok(vec![item.path().to_path_buf()]),
        };
        let Ok(targets) = targets else {
            return Outcome::Failed("its contents could not be listed".into());
        };
        let failed: Vec<String> = targets
            .iter()
            .filter_map(|target| self.trash.trash(target).err())
            .collect();
        match failed.first() {
            None => Outcome::MovedToTrash,
            Some(first) => Outcome::Failed(format!(
                "{} of {} not moved to the Trash: {first}",
                failed.len(),
                targets.len()
            )),
        }
    }

    /// Asks Git for the worktree's status now (with its tree's activity
    /// read now), and removes it through Git only when it is removable.
    fn remove_worktree(&self, item: &PreviewItem, opened: &Opened) -> (Outcome, Bytes) {
        let device = item.candidate.device;
        let before = survey(&opened.dir, device);
        let path = item.path();
        let status = inspect_worktree(self.git, path, before.newest, self.now, self.worktrees);
        if let WorktreeStatus::Blocked(reasons) = status {
            return (
                Outcome::Skipped(SkipReason::Blocked(reasons)),
                Bytes::default(),
            );
        }
        let removed = remove_worktree(self.git, path);
        let after = survey(&opened.dir, device);
        let gone = Bytes(before.bytes.0.saturating_sub(after.bytes.0));
        match removed {
            Ok(()) => (Outcome::Removed, gone),
            Err(said) => (Outcome::Failed(said), gone),
        }
    }
}

/// The paths of the entries of the opened directory.
fn contents(path: &Path, opened: &Opened) -> Result<Vec<PathBuf>, ListError> {
    let entries = list_dir(&opened.dir)?;
    Ok(entries
        .into_iter()
        .map(|entry| path.join(entry.name))
        .collect())
}
