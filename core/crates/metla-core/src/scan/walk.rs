//! The parallel walk.
//!
//! Every directory is one task on the pool. A task opens its directory
//! relative to its parent's descriptor, lists it, and spawns one task per
//! subdirectory; nothing recurses on the stack. A directory's row is
//! emitted when its last child has been delivered (post-order), so its
//! tree totals are final; then its totals are delivered to its parent.
//! Memory and descriptors are bounded by the directories in flight.
//!
//! With a rule set, every directory read is classified from its facts
//! unless it lies inside a candidate (a linked worktree does not count:
//! its build output is classified too). What a directory passes on to its
//! children (its marker entries, whether it is in a repository or a
//! candidate, its area) travels down the way totals travel up.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rayon::Scope;

use super::control::{LiveChain, LiveCounters, ScanControl, add_live};
use super::{ROW_BATCH, ScanOptions};
use crate::domain::{
    Bytes, Coverage, DirFacts, DirId, DirRecord, DirState, Entry, EntryKind, FileKind, Gap, GapAt,
    KindTotals, OwnTotals, Probe, RuleId, Skip, TreeTotals, UnixTime,
};
use crate::fs::{DirHandle, ListError, list_dir};
use crate::rules::{Area, PROBE_LIMIT};

/// What a directory passes on to its children's classification.
#[derive(Default)]
struct Passed {
    /// Its entries the rules want as markers.
    markers: Vec<OsString>,
    /// It is, or is inside, a Git repository.
    in_repo: bool,
    /// It is, or is inside, a candidate: nothing below is classified.
    in_candidate: bool,
    area: Area,
}

/// A directory that has been read and is waiting for its children.
struct Node {
    id: DirId,
    parent: Option<Arc<Node>>,
    name: OsString,
    identity: Identity,
    own: OwnTotals,
    rule: Option<RuleId>,
    passed: Passed,
    live: LiveChain,
    own_live: Option<Arc<LiveCounters>>,
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

/// A directory's device and inode, as listed in its parent.
#[derive(Clone, Copy)]
struct Identity {
    device: u64,
    inode: u64,
}

/// A directory to read.
struct Task {
    id: DirId,
    parent: Option<(Arc<Node>, Arc<DirHandle>)>,
    name: OsString,
    path: PathBuf,
    depth: u32,
    mtime: UnixTime,
    identity: Identity,
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
    identity: Identity,
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

/// What the parent passed down, and the grandparent's markers.
struct Above<'n> {
    passed: &'n Passed,
    grandparent_markers: &'n [OsString],
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) struct Walk<'a> {
    options: &'a ScanOptions,
    control: &'a ScanControl,
    sink: &'a (dyn Fn(Vec<DirRecord>) + Sync),
    /// What the root inherits from the unread directories above it.
    root: Passed,
    rows: Mutex<Vec<DirRecord>>,
    coverage: Mutex<Coverage>,
    next_id: AtomicI64,
    totals: Mutex<TreeTotals>,
    kinds: [AtomicU64; FileKind::ALL.len()],
}

impl<'a> Walk<'a> {
    pub fn new(
        options: &'a ScanOptions,
        control: &'a ScanControl,
        sink: &'a (dyn Fn(Vec<DirRecord>) + Sync),
    ) -> Walk<'a> {
        let area = options
            .rules
            .as_ref()
            .map(|r| r.area_above(options.root().path()));
        Walk {
            options,
            control,
            sink,
            root: Passed {
                area: area.unwrap_or_default(),
                ..Passed::default()
            },
            rows: Mutex::default(),
            coverage: Mutex::new(Coverage::new(options.gap_limit)),
            next_id: AtomicI64::default(),
            totals: Mutex::default(),
            kinds: Default::default(),
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
            identity: Identity {
                device: root.device(),
                inode: root.inode(),
            },
        };
        self.process(scope, task);
    }

    /// Sends the rows still buffered and returns the root totals, the bytes
    /// by file kind and the coverage.
    pub fn finish(self) -> (TreeTotals, KindTotals, Coverage) {
        let rest = std::mem::take(&mut *lock(&self.rows));
        if !rest.is_empty() {
            (self.sink)(rest);
        }
        let mut kinds = KindTotals::default();
        for (kind, bytes) in FileKind::ALL.into_iter().zip(&self.kinds) {
            kinds.add(kind, Bytes(bytes.load(Ordering::Relaxed)));
        }
        let totals = *lock(&self.totals);
        let coverage = self.coverage.into_inner();
        (
            totals,
            kinds,
            coverage.unwrap_or_else(PoisonError::into_inner),
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
        let above = self.above(parent.as_deref());
        let mut listing = self.sort(&task.path, &entries, above.passed.in_candidate);
        let (live, own_live) = self.register(&task.name, task.id, task.depth, parent.as_ref());
        add_live(&live, listing.own.bytes, listing.own.files);
        self.control.progress.add_read(&listing.own);
        let markers = std::mem::take(&mut listing.markers);
        let (rule, passed) =
            self.judge(&task.path, &entries, &dir, &above, markers, listing.has_git);
        self.count_kinds(&entries, passed.area);
        let mut tree = TreeTotals::directory(task.mtime);
        tree.add_own(&listing.own, listing.newest);
        let node = Arc::new(Node {
            id: task.id,
            parent,
            name: task.name,
            identity: task.identity,
            own: listing.own,
            rule,
            passed,
            live,
            own_live,
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
                identity: child.identity,
            };
            match child.skip {
                Some(skip) => self.finish_unread(task, DirState::Skipped(skip)),
                None => scope.spawn(move |scope| self.process(scope, task)),
            }
        }
    }

    fn above<'n>(&'n self, parent: Option<&'n Node>) -> Above<'n> {
        match parent {
            Some(node) => Above {
                passed: &node.passed,
                grandparent_markers: node.parent.as_deref().map_or(&[], |g| &g.passed.markers),
            },
            None => Above {
                passed: &self.root,
                grandparent_markers: &[],
            },
        }
    }

    /// Own totals, subdirectories with their scope decision, and the names
    /// the children's classification will want (none inside a candidate).
    fn sort(&self, path: &Path, entries: &[Entry], in_candidate: bool) -> Listing {
        let rules = self.options.rules.as_deref().filter(|_| !in_candidate);
        let mut listing = Listing::default();
        for entry in entries {
            listing.has_git |= entry.name == ".git";
            if rules.is_some_and(|rules| rules.is_marker(&entry.name)) {
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
                    identity: Identity {
                        device: entry.meta.device,
                        inode: entry.meta.inode,
                    },
                }),
                _ => {
                    listing.own.add(&entry.meta);
                    listing.newest = listing.newest.max(Some(entry.meta.mtime));
                }
            }
        }
        listing
    }

    /// Shows the directory's facts to the observer, classifies it (unless
    /// it is inside a candidate) and decides what it passes on.
    fn judge(
        &self,
        path: &Path,
        entries: &[Entry],
        dir: &DirHandle,
        above: &Above<'_>,
        markers: Vec<OsString>,
        has_git: bool,
    ) -> (Option<RuleId>, Passed) {
        let probes = self.probes(dir, entries, above.passed.in_candidate);
        let facts = DirFacts {
            path,
            entries,
            parent_markers: &above.passed.markers,
            grandparent_markers: above.grandparent_markers,
            inside_git_repo: above.passed.in_repo,
            probes: &probes,
        };
        self.observe(&facts);
        let rules = self.options.rules.as_deref();
        let classifying = rules.filter(|_| !above.passed.in_candidate);
        let rule = classifying
            .and_then(|rules| rules.classify(&facts))
            .cloned();
        let in_repo = above.passed.in_repo || has_git;
        let area = rules.map(|rules| rules.area(above.passed.area, path, in_repo, rule.as_ref()));
        let encloses = rule
            .as_ref()
            .is_some_and(|r| !r.category().classifies_within());
        let passed = Passed {
            markers,
            in_repo,
            in_candidate: above.passed.in_candidate || encloses,
            area: area.unwrap_or_default(),
        };
        (rule, passed)
    }

    /// The heads of the files the rules inspect, when the directory holds
    /// them as regular files on this disk. A file that cannot be read is
    /// left out, as if it were not there.
    fn probes(&self, dir: &DirHandle, entries: &[Entry], in_candidate: bool) -> Vec<Probe> {
        let Some(rules) = self.options.rules.as_deref().filter(|_| !in_candidate) else {
            return Vec::new();
        };
        let wanted = rules.probes();
        let files = entries.iter().filter(|e| e.meta.kind == EntryKind::File);
        files
            .filter(|e| !e.meta.dataless)
            .filter(|e| wanted.iter().any(|name| e.name == *name))
            .filter_map(|e| {
                let head = dir.read_head(&e.name, PROBE_LIMIT).ok()?;
                let name = e.name.clone();
                Some(Probe { name, head })
            })
            .collect()
    }

    /// Adds the directory's own files to the scan's bytes by kind.
    fn count_kinds(&self, entries: &[Entry], area: Area) {
        let Some(rules) = self.options.rules.as_deref() else {
            return;
        };
        let mut own = KindTotals::default();
        for entry in entries.iter().filter(|e| e.meta.kind != EntryKind::Dir) {
            own.add(rules.kind(area, &entry.name), entry.meta.share());
        }
        for (kind, bytes) in own.iter().filter(|(_, b)| *b != Bytes::default()) {
            self.kinds[kind as usize].fetch_add(bytes.0, Ordering::Relaxed);
        }
    }

    fn observe(&self, facts: &DirFacts<'_>) {
        if let Some(observer) = &self.options.observer {
            observer.observe(facts);
        }
    }

    /// Registers the directory in the live tree; returns the chain its
    /// contributions go to and its own counters.
    fn register(
        &self,
        name: &OsString,
        id: DirId,
        depth: u32,
        parent: Option<&Arc<Node>>,
    ) -> (LiveChain, Option<Arc<LiveCounters>>) {
        let chain = parent.map_or_else(empty_chain, |p| Arc::clone(&p.live));
        let parent_id = parent.map(|p| p.id);
        (self.control.live).register(&chain, id, parent_id, name, depth)
    }

    /// Records a directory that is not read: a gap or a skip.
    fn finish_unread(&self, task: Task, state: DirState) {
        let parent = task.parent.map(|(node, _)| node);
        let tree = TreeTotals::directory(task.mtime);
        let (live, own_live) = self.register(&task.name, task.id, task.depth, parent.as_ref());
        add_live(&live, tree.bytes, tree.files);
        if let Some(own) = own_live {
            own.finish();
        }
        self.note(&task.path, task.depth, state);
        self.emit(DirRecord {
            id: task.id,
            parent: parent.as_ref().map(|p| p.id),
            name: task.name,
            device: task.identity.device,
            inode: task.identity.inode,
            state,
            tree,
            rule: None,
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
                device: node.identity.device,
                inode: node.identity.inode,
                state: DirState::Read(node.own),
                tree,
                rule: node.rule.clone(),
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
