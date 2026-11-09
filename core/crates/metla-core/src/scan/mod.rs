//! Scanning a tree: a parallel, metadata-only walk that emits one record
//! per directory, post-order, to a caller-supplied sink.

mod control;
mod scope;
mod walk;

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::domain::{DirFacts, DirRecord, ScanSummary, UnixTime};
use crate::fs::{now, raise_open_file_limit};

pub use control::{LIVE_DEPTH, ScanControl};
pub use scope::ScanScope;

/// Directory records are handed to the sink in batches of at most this
/// many.
pub const ROW_BATCH: usize = 1024;

/// How many of the shallowest gaps a scan keeps by default.
pub const GAP_LIMIT: usize = 32;

/// The directory a scan starts from: canonical, existing, a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRoot {
    path: PathBuf,
    device: u64,
    mtime: UnixTime,
}

impl ScanRoot {
    pub fn new(path: &Path) -> io::Result<ScanRoot> {
        let path = std::fs::canonicalize(path)?;
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_dir() {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }
        Ok(ScanRoot {
            path,
            device: meta.dev(),
            mtime: UnixTime(meta.mtime()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn device(&self) -> u64 {
        self.device
    }

    pub fn mtime(&self) -> UnixTime {
        self.mtime
    }
}

/// Sees every directory the walk reads, with the facts a classifier needs.
/// Called on scan threads, before the directory's subdirectories are read.
pub trait DirObserver: Send + Sync {
    fn observe(&self, facts: &DirFacts<'_>);
}

/// What to scan and how.
pub struct ScanOptions {
    root: ScanRoot,
    scope: ScanScope,
    threads: usize,
    markers: Vec<OsString>,
    gap_limit: usize,
    observer: Option<Arc<dyn DirObserver>>,
}

impl ScanOptions {
    /// One thread, no markers, no observer.
    pub fn new(root: ScanRoot, scope: ScanScope) -> ScanOptions {
        ScanOptions {
            root,
            scope,
            threads: 1,
            markers: Vec::new(),
            gap_limit: GAP_LIMIT,
            observer: None,
        }
    }

    pub fn threads(mut self, threads: usize) -> ScanOptions {
        self.threads = threads.max(1);
        self
    }

    /// Entry names the observer is told about in each child's facts.
    pub fn markers(mut self, markers: Vec<OsString>) -> ScanOptions {
        self.markers = markers;
        self
    }

    pub fn gap_limit(mut self, limit: usize) -> ScanOptions {
        self.gap_limit = limit;
        self
    }

    pub fn observer(mut self, observer: Arc<dyn DirObserver>) -> ScanOptions {
        self.observer = Some(observer);
        self
    }

    pub fn root(&self) -> &ScanRoot {
        &self.root
    }
}

/// The scan could not start.
#[derive(Debug)]
pub struct ScanError {
    reason: String,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "scan could not start: {}", self.reason)
    }
}

impl std::error::Error for ScanError {}

/// Scans `options.root` on a pool of `options.threads` threads and returns
/// when every directory has been recorded. Records go to `sink` in
/// batches; errors become gaps and never stop the scan.
pub fn run(
    options: &ScanOptions,
    control: &ScanControl,
    sink: &(dyn Fn(Vec<DirRecord>) + Sync),
) -> Result<ScanSummary, ScanError> {
    raise_open_file_limit();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads)
        .thread_name(|n| format!("metla-scan-{n}"))
        .build()
        .map_err(|e| ScanError {
            reason: e.to_string(),
        })?;
    let started = now();
    let clock = Instant::now();
    let walk = walk::Walk::new(options, control, sink);
    pool.scope(|scope| walk.run(scope));
    let (totals, coverage) = walk.finish();
    Ok(ScanSummary {
        root: options.root.path.clone(),
        started,
        finished: now(),
        elapsed: clock.elapsed(),
        threads: options.threads,
        totals,
        coverage,
    })
}
