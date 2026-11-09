//! What another thread can see and do while a scan runs: cancel it, sample
//! its counters and poll the shallow live tree.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::domain::{Bytes, DirId, LiveDir, OwnTotals, ProgressSnapshot};

/// Directories this close to the root have live counters.
pub const LIVE_DEPTH: u32 = 3;

/// Shared between a running scan and its observers.
pub struct ScanControl {
    cancelled: AtomicBool,
    pub(super) progress: Progress,
    pub(super) live: LiveTree,
}

impl ScanControl {
    /// A control whose live tree holds at most `live_limit` directories.
    pub fn new(live_limit: usize) -> ScanControl {
        ScanControl {
            cancelled: AtomicBool::new(false),
            progress: Progress::default(),
            live: LiveTree {
                limit: live_limit,
                dirs: Mutex::default(),
            },
        }
    }

    /// Asks the scan to stop; directories not yet read become gaps.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> ProgressSnapshot {
        self.progress.snapshot()
    }

    /// The shallow directories registered so far with their totals so far,
    /// in registration order (a parent before its children).
    pub fn live(&self) -> Vec<LiveDir> {
        self.live.snapshot()
    }
}

#[derive(Default)]
pub(super) struct Progress {
    dirs: AtomicU64,
    files: AtomicU64,
    bytes: AtomicU64,
    denied: AtomicU64,
}

impl Progress {
    pub fn add_read(&self, own: &OwnTotals) {
        self.add_unread(false);
        self.files.fetch_add(own.files, Ordering::Relaxed);
        self.bytes.fetch_add(own.bytes.0, Ordering::Relaxed);
    }

    pub fn add_unread(&self, denied: bool) {
        self.dirs.fetch_add(1, Ordering::Relaxed);
        self.denied.fetch_add(u64::from(denied), Ordering::Relaxed);
    }

    fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            dirs: self.dirs.load(Ordering::Relaxed),
            files: self.files.load(Ordering::Relaxed),
            bytes: Bytes(self.bytes.load(Ordering::Relaxed)),
            denied: self.denied.load(Ordering::Relaxed),
        }
    }
}

/// Live totals of one shallow directory; descendants add to them.
#[derive(Default)]
pub(super) struct LiveCounters {
    bytes: AtomicU64,
    files: AtomicU64,
    dirs: AtomicU64,
    done: AtomicBool,
}

impl LiveCounters {
    pub fn finish(&self) {
        self.done.store(true, Ordering::Release);
    }
}

/// The counters of a directory's registered shallow ancestors, itself
/// included when registered.
pub(super) type LiveChain = Arc<[Arc<LiveCounters>]>;

/// Adds one directory's own contribution to every counter of `chain`.
pub(super) fn add_live(chain: &[Arc<LiveCounters>], bytes: Bytes, files: u64) {
    for counters in chain {
        counters.bytes.fetch_add(bytes.0, Ordering::Relaxed);
        counters.files.fetch_add(files, Ordering::Relaxed);
        counters.dirs.fetch_add(1, Ordering::Relaxed);
    }
}

struct Registered {
    id: DirId,
    parent: Option<DirId>,
    name: OsString,
    depth: u32,
    counters: Arc<LiveCounters>,
}

pub(super) struct LiveTree {
    limit: usize,
    dirs: Mutex<Vec<Registered>>,
}

impl LiveTree {
    /// Registers a shallow directory while the tree has room. Returns the
    /// chain its contributions go to (its ancestors' chain, extended with
    /// its own counters when registered) and its own counters.
    pub fn register(
        &self,
        ancestors: &LiveChain,
        id: DirId,
        parent: Option<DirId>,
        name: &OsString,
        depth: u32,
    ) -> (LiveChain, Option<Arc<LiveCounters>>) {
        let unregistered = || (Arc::clone(ancestors), None);
        if depth > LIVE_DEPTH {
            return unregistered();
        }
        let mut dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        if dirs.len() >= self.limit {
            return unregistered();
        }
        let counters = Arc::new(LiveCounters::default());
        dirs.push(Registered {
            id,
            parent,
            name: name.clone(),
            depth,
            counters: Arc::clone(&counters),
        });
        let chain = ancestors
            .iter()
            .cloned()
            .chain([Arc::clone(&counters)])
            .collect();
        (chain, Some(counters))
    }

    fn snapshot(&self) -> Vec<LiveDir> {
        let dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        dirs.iter()
            .map(|d| LiveDir {
                id: d.id,
                parent: d.parent,
                name: d.name.clone(),
                depth: d.depth,
                bytes: Bytes(d.counters.bytes.load(Ordering::Relaxed)),
                files: d.counters.files.load(Ordering::Relaxed),
                dirs: d.counters.dirs.load(Ordering::Relaxed),
                done: d.counters.done.load(Ordering::Acquire),
            })
            .collect()
    }
}
