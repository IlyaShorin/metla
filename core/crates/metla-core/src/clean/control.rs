//! What another thread can see and do while a cleanup runs: cancel it and
//! sample its progress.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::domain::Bytes;

/// Shared between a running cleanup and its observers.
#[derive(Debug, Default)]
pub struct CleanControl {
    cancelled: AtomicBool,
    items: AtomicU64,
    done: AtomicU64,
    removed: AtomicU64,
    /// Bytes of the current item already counted in `removed`.
    item_removed: AtomicU64,
    planned: AtomicU64,
    current: Mutex<Option<PathBuf>>,
}

/// A running cleanup's counters, sampled at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanProgress {
    /// Items in the preview being executed.
    pub items: u64,
    /// Items finished, whatever their outcome.
    pub done: u64,
    /// Bytes removed so far, growing as entries are unlinked.
    pub removed: Bytes,
    /// What the preview estimated for the items being executed.
    pub planned: Bytes,
    /// The item being cleaned now; `None` between items.
    pub current: Option<PathBuf>,
}

impl CleanControl {
    pub fn new() -> CleanControl {
        CleanControl::default()
    }

    /// Asks the cleanup to stop between entries; items not reached are
    /// skipped as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> CleanProgress {
        CleanProgress {
            items: self.items.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            removed: Bytes(self.removed.load(Ordering::Relaxed)),
            planned: Bytes(self.planned.load(Ordering::Relaxed)),
            current: self.lock_current().clone(),
        }
    }

    pub(super) fn start(&self, items: usize, planned: Bytes) {
        self.items.store(items as u64, Ordering::Relaxed);
        self.planned.store(planned.0, Ordering::Relaxed);
    }

    /// Counts bytes of the current item as they are removed.
    pub(super) fn removed_now(&self, bytes: Bytes) {
        self.removed.fetch_add(bytes.0, Ordering::Relaxed);
        self.item_removed.fetch_add(bytes.0, Ordering::Relaxed);
    }

    fn lock_current(&self) -> std::sync::MutexGuard<'_, Option<PathBuf>> {
        let current = self.current.lock();
        current.unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn begin_item(&self, path: &Path) {
        self.item_removed.store(0, Ordering::Relaxed);
        *self.lock_current() = Some(path.to_path_buf());
    }

    /// The item is done: what it removed and was not counted yet (a
    /// worktree's, measured at the end) is added now.
    pub(super) fn finish_item(&self, removed: Bytes) {
        *self.lock_current() = None;
        let counted = self.item_removed.swap(0, Ordering::Relaxed);
        self.removed
            .fetch_add(removed.0.saturating_sub(counted), Ordering::Relaxed);
        self.done.fetch_add(1, Ordering::Relaxed);
    }
}
