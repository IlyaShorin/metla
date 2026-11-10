//! What another thread can see and do while a cleanup runs: cancel it and
//! sample its progress.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::domain::Bytes;

/// Shared between a running cleanup and its observers.
#[derive(Debug, Default)]
pub struct CleanControl {
    cancelled: AtomicBool,
    items: AtomicU64,
    done: AtomicU64,
    removed: AtomicU64,
}

/// A running cleanup's counters, sampled at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanProgress {
    /// Items in the preview being executed.
    pub items: u64,
    /// Items finished, whatever their outcome.
    pub done: u64,
    /// Bytes removed so far.
    pub removed: Bytes,
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
        }
    }

    pub(super) fn start(&self, items: usize) {
        self.items.store(items as u64, Ordering::Relaxed);
    }

    pub(super) fn finish_item(&self, removed: Bytes) {
        self.removed.fetch_add(removed.0, Ordering::Relaxed);
        self.done.fetch_add(1, Ordering::Relaxed);
    }
}
