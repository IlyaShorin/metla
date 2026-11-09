//! The parallel walk.
//!
//! Every directory is one task on the pool. A task opens its directory
//! relative to its parent's descriptor, lists it, and spawns one task per
//! subdirectory; nothing recurses on the stack. A directory's row is
//! emitted when its last child has been delivered (post-order), so its
//! tree totals are final; then its totals are delivered to its parent.
//! Memory and descriptors are bounded by the directories in flight.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rayon::Scope;

use super::control::{LiveChain, LiveCounters, ScanControl, add_live};
use super::{ROW_BATCH, ScanOptions};
use crate::domain::{
    Coverage, DirFacts, DirId, DirRecord, DirState, Entry, EntryKind, Gap, GapAt, OwnTotals, Skip,
    TreeTotals, UnixTime,
};
use crate::fs::{DirHandle, ListError, list_dir};

/// A directory that has been read and is waiting for its children.
struct Node {
    id: DirId,
    parent: Option<Arc<Node>>,
    name: OsString,
    own: OwnTotals,
    live: LiveChain,
    own_live: Option<Arc<LiveCounters>>,
    /// Entries named in the scan's marker list, for the children's facts.
    markers: Vec<OsString>,
    /// This directory or an ancestor contains `.git`.
    children_in_repo: bool,
    tree: Mutex<TreeTotals>,
    pending: AtomicUsize,
}

impl Node {
    /// Adds a finished child; true when it was the last one.
    fn deliver(&self, child: &TreeTotals) -> bool {
        lock(&self.tree).add_tree(child);
        self.pending.fetch_sub(1, Ordering::AcqRel) == 1
    }
}

/// A directory to read.
struct Task {
    id: DirId,
    parent: Option<(Arc<Node>, Arc<DirHandle>)>,
    name: OsString,
    path: PathBuf,
    depth: u32,
    mtime: UnixTime,
}

impl Task {
    fn open(&self) -> Result<DirHandle, ListError> {
        match &self.parent {
            Some((_, parent)) => parent.open_child(&self.name),
            None => DirHandle::open(&self.path),
        }
    }
}

/// A subdirectory seen in a listing.
struct Child {
    name: OsString,
    mtime: UnixTime,
    skip: Option<Skip>,
}

/// One directory's entries, sorted into what the walk needs.
#[derive(Default)]
struct Listing {
    own: OwnTotals,
    newest: Option<UnixTime>,
    children: Vec<Child>,
    markers: Vec<OsString>,
    has_git: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) struct Walk<'a> {
    options: &'a ScanOptions,
    control: &'a ScanControl,
    sink: &'a (dyn Fn(Vec<DirRecord>) + Sync),
    rows: Mutex<Vec<DirRecord>>,
    coverage: Mutex<Coverage>,
    next_id: AtomicI64,
    totals: Mutex<TreeTotals>,
}

impl<'a> Walk<'a> {
    pub fn new(
        options: &'a ScanOptions,
        control: &'a ScanControl,
        sink: &'a (dyn Fn(Vec<DirRecord>) + Sync),
    ) -> Walk<'a> {
        Walk {
            options,
            control,
            sink,
            rows: Mutex::default(),
            coverage: Mutex::new(Coverage::new(options.gap_limit)),
            next_id: AtomicI64::default(),
            totals: Mutex::default(),
        }
    }

    /// Walks the whole tree on the pool `scope` belongs to.
    pub fn run<'s>(&'s self, scope: &Scope<'s>) {
        let root = self.options.root();
        let task = Task {
            id: self.new_id(),
            parent: None,
            name: OsString::new(),
            path: root.path().to_path_buf(),
            depth: 0,
            mtime: root.mtime(),
        };
        self.process(scope, task);
    }

    /// Sends the rows still buffered and returns the root totals and the
    /// coverage.
    pub fn finish(self) -> (TreeTotals, Coverage) {
        let rest = std::mem::take(&mut *lock(&self.rows));
        if !rest.is_empty() {
            (self.sink)(rest);
        }
        let totals = *lock(&self.totals);
        (
            totals,
            self.coverage
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    fn new_id(&self) -> DirId {
        DirId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn process<'s>(&'s self, scope: &Scope<'s>, task: Task) {
        if self.control.is_cancelled() {
            return self.finish_unread(task, DirState::Gap(Gap::Cancelled));
        }
        let listed = task.open().and_then(|dir| Ok((list_dir(&dir)?, dir)));
        match listed {
            Ok((entries, dir)) => self.read(scope, task, entries, dir),
            Err(error) => self.finish_unread(task, DirState::Gap(gap_of(error))),
        }
    }

    fn read<'s>(&'s self, scope: &Scope<'s>, task: Task, entries: Vec<Entry>, dir: DirHandle) {
        let parent = task.parent.map(|(node, _)| node);
        let listing = self.sort(&task.path, &entries);
        let chain = parent
            .as_ref()
            .map_or_else(empty_chain, |p| Arc::clone(&p.live));
        let parent_id = parent.as_ref().map(|p| p.id);
        let (live, own_live) =
            (self.control.live).register(&chain, task.id, parent_id, &task.name, task.depth);
        add_live(&live, listing.own.bytes, listing.own.files);
        self.control.progress.add_read(&listing.own);
        let inside_repo = parent.as_ref().is_some_and(|p| p.children_in_repo);
        self.observe(&task.path, &entries, parent.as_deref(), inside_repo);
        let mut tree = TreeTotals::directory(task.mtime);
        tree.add_own(&listing.own, listing.newest);
        let node = Arc::new(Node {
            id: task.id,
            parent,
            name: task.name,
            own: listing.own,
            live,
            own_live,
            markers: listing.markers,
            children_in_repo: inside_repo || listing.has_git,
            tree: Mutex::new(tree),
            pending: AtomicUsize::new(listing.children.len()),
        });
        if listing.children.is_empty() {
            return self.complete(node);
        }
        let dir = Arc::new(dir);
        for child in listing.children {
            let task = Task {
                id: self.new_id(),
                path: task.path.join(&child.name),
                parent: Some((Arc::clone(&node), Arc::clone(&dir))),
                name: child.name,
                depth: task.depth + 1,
                mtime: child.mtime,
            };
            match child.skip {
                Some(skip) => self.finish_unread(task, DirState::Skipped(skip)),
                None => scope.spawn(move |scope| self.process(scope, task)),
            }
        }
    }

    /// Own totals, subdirectories with their scope decision, and the names
    /// the classifier will want.
    fn sort(&self, path: &Path, entries: &[Entry]) -> Listing {
        let mut listing = Listing::default();
        for entry in entries {
            listing.has_git |= entry.name == ".git";
            if self.options.markers.contains(&entry.name) {
                listing.markers.push(entry.name.clone());
            }
            match entry.meta.kind {
                EntryKind::Dir => listing.children.push(Child {
                    skip: self
                        .options
                        .scope
                        .decide(&path.join(&entry.name), &entry.meta),
                    name: entry.name.clone(),
                    mtime: entry.meta.mtime,
                }),
                _ => {
                    listing.own.add(&entry.meta);
                    listing.newest = listing.newest.max(Some(entry.meta.mtime));
                }
            }
        }
        listing
    }

    fn observe(&self, path: &Path, entries: &[Entry], parent: Option<&Node>, inside_repo: bool) {
        if let Some(observer) = &self.options.observer {
            observer.observe(&DirFacts {
                path,
                entries,
                parent_markers: parent.map_or(&[], |p| &p.markers),
                inside_git_repo: inside_repo,
            });
        }
    }

    /// Records a directory that is not read: a gap or a skip.
    fn finish_unread(&self, task: Task, state: DirState) {
        let parent = task.parent.map(|(node, _)| node);
        let tree = TreeTotals::directory(task.mtime);
        let chain = parent
            .as_ref()
            .map_or_else(empty_chain, |p| Arc::clone(&p.live));
        let parent_id = parent.as_ref().map(|p| p.id);
        let (live, own_live) =
            (self.control.live).register(&chain, task.id, parent_id, &task.name, task.depth);
        add_live(&live, tree.bytes, tree.files);
        if let Some(own) = own_live {
            own.finish();
        }
        self.note(&task.path, task.depth, state);
        self.emit(DirRecord {
            id: task.id,
            parent: parent_id,
            name: task.name,
            state,
            tree,
        });
        match parent {
            Some(parent) => self.deliver(parent, &tree),
            None => *lock(&self.totals) = tree,
        }
    }

    /// Counts an unread directory in the coverage and the progress.
    fn note(&self, path: &Path, depth: u32, state: DirState) {
        self.control
            .progress
            .add_unread(state == DirState::Gap(Gap::Denied));
        let mut coverage = lock(&self.coverage);
        match state {
            DirState::Gap(gap) => coverage.record_gap(GapAt {
                depth,
                path: path.to_path_buf(),
                gap,
            }),
            DirState::Skipped(skip) => coverage.record_skip(skip),
            DirState::Read(_) => {}
        }
    }

    fn deliver(&self, parent: Arc<Node>, child: &TreeTotals) {
        if parent.deliver(child) {
            self.complete(parent);
        }
    }

    /// Emits a finished directory and delivers it upwards, completing every
    /// ancestor whose last child it was.
    fn complete(&self, mut node: Arc<Node>) {
        loop {
            let tree = *lock(&node.tree);
            if let Some(own) = &node.own_live {
                own.finish();
            }
            self.emit(DirRecord {
                id: node.id,
                parent: node.parent.as_ref().map(|p| p.id),
                name: node.name.clone(),
                state: DirState::Read(node.own),
                tree,
            });
            let Some(parent) = node.parent.clone() else {
                *lock(&self.totals) = tree;
                return;
            };
            drop(node);
            if !parent.deliver(&tree) {
                return;
            }
            node = parent;
        }
    }

    fn emit(&self, record: DirRecord) {
        let full = {
            let mut rows = lock(&self.rows);
            rows.push(record);
            (rows.len() >= ROW_BATCH).then(|| std::mem::take(&mut *rows))
        };
        if let Some(batch) = full {
            (self.sink)(batch);
        }
    }
}

fn empty_chain() -> LiveChain {
    Arc::new([])
}

fn gap_of(error: ListError) -> Gap {
    match error {
        ListError::Denied => Gap::Denied,
        ListError::Gone => Gap::Vanished,
        ListError::Io(errno) => Gap::Failed(errno),
    }
}
