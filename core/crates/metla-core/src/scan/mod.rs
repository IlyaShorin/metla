//! Scanning a tree: a parallel, metadata-only walk that emits one record
//! per directory, post-order, to a caller-supplied sink; and rescanning
//! one directory of a picture, shallowly or with everything below it.

mod control;
mod scope;
mod walk;

use std::fmt;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use crate::domain::{DirFacts, DirId, DirRecord, EntryKind, EntryMeta, Rescanned, ScanSummary};
use crate::fs::{DirHandle, background_thread, entry_at, now, raise_open_file_limit};
use crate::rules::RuleSet;

pub use control::{LIVE_DEPTH, ScanControl};
pub use scope::ScanScope;
pub use walk::{Inherited, Stored};

use walk::{Identity, Origin, Walk};

/// Directory records are handed to the sink in batches of at most this
/// many.
pub const ROW_BATCH: usize = 1024;

/// How many of the shallowest gaps a scan keeps by default.
pub const GAP_LIMIT: usize = 32;

/// The directory a scan starts from: canonical, existing, a directory.
/// Its times are read again whenever it is scanned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRoot {
    path: PathBuf,
    device: u64,
    inode: u64,
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
            inode: meta.ino(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn device(&self) -> u64 {
        self.device
    }

    pub fn inode(&self) -> u64 {
        self.inode
    }
}

/// Sees every directory the walk reads, with the facts a classifier needs.
/// Called on scan threads, before the directory's subdirectories are read.
pub trait DirObserver: Send + Sync {
    fn observe(&self, facts: &DirFacts<'_>);
}

/// How the scan threads run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
    #[default]
    Normal,
    /// Background priority: CPU and I/O yield to everything else.
    Background,
}

/// What to scan and how.
pub struct ScanOptions {
    root: ScanRoot,
    scope: ScanScope,
    threads: usize,
    throttled: bool,
    classic: bool,
    rules: Option<Arc<RuleSet>>,
    observer: Option<Arc<dyn DirObserver>>,
}

impl ScanOptions {
    /// One thread, no rules, no observer.
    pub fn new(root: ScanRoot, scope: ScanScope) -> ScanOptions {
        ScanOptions {
            root,
            scope,
            threads: 1,
            throttled: false,
            classic: false,
            rules: None,
            observer: None,
        }
    }

    pub fn threads(mut self, threads: usize) -> ScanOptions {
        self.threads = threads.max(1);
        self
    }

    /// Run the scan threads at `priority`.
    pub fn priority(mut self, priority: Priority) -> ScanOptions {
        self.throttled = priority == Priority::Background;
        self
    }

    /// List directories with `readdir` and `fstatat` instead of
    /// `getattrlistbulk`, to compare the two.
    pub fn classic(mut self, classic: bool) -> ScanOptions {
        self.classic = classic;
        self
    }

    /// Classify every directory with `rules` and total the file kinds.
    pub fn rules(mut self, rules: Arc<RuleSet>) -> ScanOptions {
        self.rules = Some(rules);
        self
    }

    pub fn observer(mut self, observer: Arc<dyn DirObserver>) -> ScanOptions {
        self.observer = Some(observer);
        self
    }

    pub fn root(&self) -> &ScanRoot {
        &self.root
    }

    pub fn scope(&self) -> &ScanScope {
        &self.scope
    }

    /// What the root inherits from the unread directories above it.
    pub fn root_inherits(&self) -> Inherited {
        Walk::root_inherited(self)
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
    let pool = pool(options)?;
    let started = now();
    let clock = Instant::now();
    let root = options.root();
    let (_, meta) = locate(&root.path, &root.path).ok_or_else(|| ScanError {
        reason: format!("{} cannot be opened", root.path.display()),
    })?;
    let origin = Origin {
        id: DirId(0),
        parent: None,
        inherited: Walk::root_inherited(options),
        above: None,
        path: root.path.clone(),
        depth: 0,
        mtime: meta.mtime,
        identity: Identity {
            device: meta.device,
            inode: meta.inode,
            changed: meta.changed,
        },
        skip: None,
        shallow: None,
    };
    let walk = Walk::new(options, control, sink, DirId(1));
    pool.scope(|scope| walk.run(scope, origin));
    let walked = walk.finish();
    Ok(ScanSummary {
        root: root.path.clone(),
        home: options.rules.as_ref().map(|r| r.home().to_path_buf()),
        started,
        finished: now(),
        elapsed: clock.elapsed(),
        threads: options.threads,
        totals: walked.totals,
        kinds: walked.kinds,
        coverage: walked.coverage,
    })
}

fn pool(options: &ScanOptions) -> Result<rayon::ThreadPool, ScanError> {
    let throttled = options.throttled;
    rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads)
        .thread_name(|n| format!("metla-scan-{n}"))
        .start_handler(move |_| {
            if throttled {
                background_thread();
            }
        })
        .build()
        .map_err(|e| ScanError {
            reason: e.to_string(),
        })
}

/// A directory of a picture to bring up to date, with what the picture
/// says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Its path, inside the scan root.
    pub path: PathBuf,
    pub id: DirId,
    pub parent: Option<DirId>,
    pub inherited: Inherited,
    /// `Some`: a shallow rescan, which re-reads the directory and walks
    /// only the subdirectories that changed. `None`: the directory and
    /// everything below it are walked again.
    pub stored: Option<Stored>,
}

/// Rescans `target`; new directories get ids from `next_id` on. `None`
/// when it is no longer a directory reached from the root without a
/// symlink: then its parent has changed, and that is what to rescan.
pub fn rescan(
    options: &ScanOptions,
    target: Target,
    next_id: DirId,
    control: &ScanControl,
) -> Result<Option<Rescanned>, ScanError> {
    let Some((above, meta)) = locate(options.root().path(), &target.path) else {
        return Ok(None);
    };
    let pool = pool(options)?;
    let rows = Mutex::new(Vec::new());
    let sink = |batch: Vec<DirRecord>| {
        rows.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(batch)
    };
    let whole = target.stored.is_none();
    let origin = Origin {
        id: target.id,
        parent: target.parent,
        inherited: target.inherited,
        above,
        skip: options.scope.decide(&target.path, &meta),
        path: target.path,
        // A rescan's own coverage is not used: the index counts it again
        // from its rows, so depths below the origin need no offset.
        depth: u32::default(),
        mtime: meta.mtime,
        identity: Identity {
            device: meta.device,
            inode: meta.inode,
            changed: meta.changed,
        },
        shallow: target.stored,
    };
    let walk = Walk::new(options, control, &sink, next_id);
    pool.scope(|scope| walk.run(scope, origin));
    let walked = walk.finish();
    let removed = match whole {
        true => vec![target.id],
        false => walked.removed,
    };
    Ok(Some(Rescanned {
        origin: target.id,
        removed,
        rows: rows.into_inner().unwrap_or_else(PoisonError::into_inner),
    }))
}

/// The open parent of the directory at `path` (`None` for the root) and
/// the directory's metadata as listed there, opening every directory from
/// the root down without following a symlink. `None` when it is not
/// there as a directory.
fn locate(root: &Path, path: &Path) -> Option<(Option<DirHandle>, EntryMeta)> {
    let relative = path.strip_prefix(root).ok()?;
    let Some(name) = relative.file_name() else {
        let meta = entry_at(root).ok().filter(|m| m.kind == EntryKind::Dir)?;
        return Some((None, meta));
    };
    let mut parent = DirHandle::open(root).ok()?;
    for part in relative.parent().into_iter().flat_map(Path::components) {
        parent = parent.open_child(part.as_os_str()).ok()?;
    }
    let meta = parent.entry(name).ok()?;
    (meta.kind == EntryKind::Dir).then_some((Some(parent), meta))
}
