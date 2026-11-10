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
//!
//! A walk starts at an *origin*: the scan root, or one directory of a
//! picture being brought up to date, which inherits from the stored rows
//! above it what an unbroken walk would have handed down. A *shallow*
//! origin is compared with its stored row and children: if it hands down
//! the same as before, a subdirectory that is still the same directory
//! with the same modification time keeps its stored subtree, and only the
//! others are walked.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rayon::Scope;

use super::control::{LiveChain, LiveCounters, ScanControl, add_live};
use super::{GAP_LIMIT, ROW_BATCH, ScanOptions};
use crate::domain::{
    Area, Bytes, Coverage, DirFacts, DirId, DirRecord, DirState, Entry, EntryKind, FileKind, Gap,
    GapAt, KindTotals, OwnTotals, Passed, Probe, RuleId, Skip, Stamp, TreeTotals, UnixTime,
};
use crate::fs::{DirHandle, ListError, list_dir, list_dir_classic};
use crate::rules::PROBE_LIMIT;

/// What the directories above a walk's origin hand down to it: what its
/// parent passed on, and its grandparent's markers. The scan root
/// inherits only the area of the unread directories above it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inherited {
    pub passed: Passed,
    pub grandparent_markers: Vec<OsString>,
}

/// A directory as the picture holds it: its row and its subdirectories'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub row: DirRecord,
    pub children: Vec<DirRecord>,
}

/// A directory that has been read and is waiting for its children.
struct Node {
    id: DirId,
    up: Up,
    name: OsString,
    identity: Identity,
    mtime: UnixTime,
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

/// What is above a directory of the walk.
#[derive(Clone)]
enum Up {
    /// A directory of this walk.
    Node(Arc<Node>),
    /// Nothing this walk read: the directory is the origin, below the
    /// directory `parent` (`None` for the root).
    Origin {
        parent: Option<DirId>,
        inherited: Arc<Inherited>,
    },
}

impl Up {
    fn id(&self) -> Option<DirId> {
        match self {
            Up::Node(node) => Some(node.id),
            Up::Origin { parent, .. } => *parent,
        }
    }

    /// What the directory above hands down, and the markers of the one
    /// above that.
    fn above(&self) -> Above<'_> {
        match self {
            Up::Node(node) => Above {
                passed: &node.passed,
                grandparent_markers: node.up.markers(),
            },
            Up::Origin { inherited, .. } => Above {
                passed: &inherited.passed,
                grandparent_markers: &inherited.grandparent_markers,
            },
        }
    }

    /// The markers of the directory above.
    fn markers(&self) -> &[OsString] {
        match self {
            Up::Node(node) => &node.passed.markers,
            Up::Origin { inherited, .. } => &inherited.passed.markers,
        }
    }

    fn node(&self) -> Option<&Arc<Node>> {
        match self {
            Up::Node(node) => Some(node),
            Up::Origin { .. } => None,
        }
    }
}

/// A directory's device, inode and change stamp, as listed in its parent.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Identity {
    pub device: u64,
    pub inode: u64,
    pub changed: Stamp,
}

/// A directory to read.
pub(super) struct Task {
    id: DirId,
    up: Up,
    /// The open directory above; `None` opens the task by path.
    above: Option<Arc<DirHandle>>,
    name: OsString,
    path: PathBuf,
    depth: u32,
    mtime: UnixTime,
    identity: Identity,
}

impl Task {
    fn open(&self) -> Result<DirHandle, ListError> {
        match &self.above {
            Some(parent) => parent.open_child(&self.name),
            None => DirHandle::open(&self.path),
        }
    }
}

/// Where a walk starts.
pub(super) struct Origin {
    pub id: DirId,
    pub parent: Option<DirId>,
    pub inherited: Inherited,
    /// The open parent directory; `None` opens the origin by path.
    pub above: Option<DirHandle>,
    pub path: PathBuf,
    pub depth: u32,
    pub mtime: UnixTime,
    pub identity: Identity,
    /// Why it is not read, if the scope says so.
    pub skip: Option<Skip>,
    /// Compare with what the picture holds and walk only what changed.
    pub shallow: Option<Stored>,
}

/// A subdirectory seen in a listing.
struct Child {
    name: OsString,
    mtime: UnixTime,
    identity: Identity,
    skip: Option<Skip>,
}

impl Child {
    /// Whether the stored row still describes it: the same directory,
    /// unchanged since (its entries, permissions and times), and read or
    /// skipped as before.
    fn unchanged(&self, stored: &DirRecord) -> bool {
        let identity = Identity {
            device: stored.device,
            inode: stored.inode,
            changed: stored.changed,
        };
        let state = match (stored.state, self.skip) {
            (DirState::Read(_), None) => true,
            (DirState::Skipped(was), Some(now)) => was == now,
            _ => false,
        };
        state && identity == self.identity && stored.mtime == self.mtime
    }
}

/// One directory's entries, sorted into what the walk needs.
#[derive(Default)]
struct Listing {
    own: OwnTotals,
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

/// What a finished walk found.
pub(super) struct Walked {
    /// The origin's tree totals.
    pub totals: TreeTotals,
    pub kinds: KindTotals,
    pub coverage: Coverage,
    /// Stored directories (of a shallow origin) that no longer belong.
    pub removed: Vec<DirId>,
}

pub(super) struct Walk<'a> {
    options: &'a ScanOptions,
    control: &'a ScanControl,
    sink: &'a (dyn Fn(Vec<DirRecord>) + Sync),
    rows: Mutex<Vec<DirRecord>>,
    coverage: Mutex<Coverage>,
    next_id: AtomicI64,
    totals: Mutex<TreeTotals>,
    kinds: [AtomicU64; FileKind::ALL.len()],
    shallow: Mutex<Option<Stored>>,
    removed: Mutex<Vec<DirId>>,
}

impl<'a> Walk<'a> {
    /// A walk whose new directories get ids from `next_id` on.
    pub fn new(
        options: &'a ScanOptions,
        control: &'a ScanControl,
        sink: &'a (dyn Fn(Vec<DirRecord>) + Sync),
        next_id: DirId,
    ) -> Walk<'a> {
        Walk {
            options,
            control,
            sink,
            rows: Mutex::default(),
            coverage: Mutex::new(Coverage::new(GAP_LIMIT)),
            next_id: AtomicI64::new(next_id.0),
            totals: Mutex::default(),
            kinds: Default::default(),
            shallow: Mutex::default(),
            removed: Mutex::default(),
        }
    }

    /// What the scan root inherits: the area of the directories above it.
    pub fn root_inherited(options: &ScanOptions) -> Inherited {
        let area = options
            .rules
            .as_ref()
            .map(|r| r.area_above(options.root().path()));
        Inherited {
            passed: Passed {
                area: area.unwrap_or(Area::User),
                ..Passed::default()
            },
            grandparent_markers: Vec::new(),
        }
    }

    /// Walks from `origin` on the pool `scope` belongs to.
    pub fn run<'s>(&'s self, scope: &Scope<'s>, origin: Origin) {
        *lock(&self.shallow) = origin.shallow;
        let task = Task {
            id: origin.id,
            up: Up::Origin {
                parent: origin.parent,
                inherited: Arc::new(origin.inherited),
            },
            above: origin.above.map(Arc::new),
            name: origin.path.file_name().unwrap_or_default().to_owned(),
            path: origin.path,
            depth: origin.depth,
            mtime: origin.mtime,
            identity: origin.identity,
        };
        match origin.skip {
            Some(skip) => self.finish_unread(task, DirState::Skipped(skip)),
            None => self.process(scope, task),
        }
    }

    /// Sends the rows still buffered and returns what the walk found.
    pub fn finish(self) -> Walked {
        let rest = std::mem::take(&mut *lock(&self.rows));
        if !rest.is_empty() {
            (self.sink)(rest);
        }
        let mut kinds = KindTotals::default();
        for (kind, bytes) in FileKind::ALL.into_iter().zip(&self.kinds) {
            kinds.add(kind, Bytes(bytes.load(Ordering::Relaxed)));
        }
        let mut removed = self
            .removed
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        let unread = self
            .shallow
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        removed.extend(unread.iter().flat_map(|s| s.children.iter().map(|c| c.id)));
        Walked {
            totals: *lock(&self.totals),
            kinds,
            coverage: self
                .coverage
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
            removed,
        }
    }

    fn new_id(&self) -> DirId {
        DirId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn process<'s>(&'s self, scope: &Scope<'s>, task: Task) {
        if self.control.is_cancelled() {
            return self.finish_unread(task, DirState::Gap(Gap::Cancelled));
        }
        let list = match self.options.classic {
            true => list_dir_classic,
            false => list_dir,
        };
        let listed = task.open().and_then(|dir| Ok((list(&dir)?, dir)));
        match listed {
            Ok((entries, dir)) => self.read(scope, task, entries, dir),
            Err(error) => self.finish_unread(task, DirState::Gap(gap_of(error))),
        }
    }

    fn read<'s>(&'s self, scope: &Scope<'s>, task: Task, entries: Vec<Entry>, dir: DirHandle) {
        let above = task.up.above();
        let mut listing = self.sort(&task.path, &entries, above.passed.in_candidate);
        let (live, own_live) = self.register(&task.name, task.id, task.depth, &task.up);
        add_live(&live, listing.own.bytes, listing.own.files);
        self.control.progress.add_read(&listing.own);
        let markers = std::mem::take(&mut listing.markers);
        let (rule, passed) =
            self.judge(&task.path, &entries, &dir, &above, markers, listing.has_git);
        listing.own.kinds = self.count_kinds(&entries, passed.area);
        let mut tree = TreeTotals::directory(task.mtime);
        tree.add_own(&listing.own);
        let children = self.compare(task.id, &rule, &passed, listing.children, &mut tree);
        let node = Arc::new(Node {
            id: task.id,
            up: task.up,
            name: task.name,
            identity: task.identity,
            mtime: task.mtime,
            own: listing.own,
            rule,
            passed,
            live,
            own_live,
            tree: Mutex::new(tree),
            pending: AtomicUsize::new(children.len()),
        });
        if children.is_empty() {
            return self.complete(node);
        }
        let dir = Arc::new(dir);
        for child in children {
            let task = Task {
                id: self.new_id(),
                path: task.path.join(&child.name),
                up: Up::Node(Arc::clone(&node)),
                above: Some(Arc::clone(&dir)),
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

    /// The children to walk. A shallow origin that hands down what it
    /// handed down before keeps every child its stored row still
    /// describes, adding the stored totals to `tree`; every stored child
    /// it does not keep is removed.
    fn compare(
        &self,
        id: DirId,
        rule: &Option<RuleId>,
        passed: &Passed,
        children: Vec<Child>,
        tree: &mut TreeTotals,
    ) -> Vec<Child> {
        let Some(stored) = lock(&self.shallow).take_if(|s| s.row.id == id) else {
            return children;
        };
        let same = matches!(stored.row.state, DirState::Read(_))
            && stored.row.rule == *rule
            && stored.row.passed == *passed;
        let mut kept = Vec::new();
        let walked = children.into_iter().filter(|child| {
            let found = stored.children.iter().find(|s| s.name == child.name);
            let keep = found.filter(|s| same && child.unchanged(s));
            kept.extend(keep.map(|s| s.id));
            keep.inspect(|s| tree.add_tree(&s.tree)).is_none()
        });
        let walked = walked.collect();
        let gone = stored.children.iter().filter(|s| !kept.contains(&s.id));
        lock(&self.removed).extend(gone.map(|s| s.id));
        walked
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
                        changed: entry.meta.changed,
                    },
                }),
                _ => listing.own.add(&entry.meta),
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

    /// The directory's own bytes by file kind, also added to the scan's.
    fn count_kinds(&self, entries: &[Entry], area: Area) -> KindTotals {
        let mut own = KindTotals::default();
        let Some(rules) = self.options.rules.as_deref() else {
            return own;
        };
        for entry in entries.iter().filter(|e| e.meta.kind != EntryKind::Dir) {
            own.add(rules.kind(area, &entry.name), entry.meta.share());
        }
        for (kind, bytes) in own.iter().filter(|(_, b)| *b != Bytes::default()) {
            self.kinds[kind as usize].fetch_add(bytes.0, Ordering::Relaxed);
        }
        own
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
        up: &Up,
    ) -> (LiveChain, Option<Arc<LiveCounters>>) {
        let chain = up.node().map_or_else(empty_chain, |p| Arc::clone(&p.live));
        (self.control.live).register(&chain, id, up.id(), name, depth)
    }

    /// Records a directory that is not read: a gap or a skip.
    fn finish_unread(&self, task: Task, state: DirState) {
        let tree = TreeTotals::directory(task.mtime);
        let (live, own_live) = self.register(&task.name, task.id, task.depth, &task.up);
        add_live(&live, tree.bytes, tree.files);
        if let Some(own) = own_live {
            own.finish();
        }
        self.note(&task.path, task.depth, state);
        self.emit(DirRecord {
            id: task.id,
            parent: task.up.id(),
            name: task.name,
            device: task.identity.device,
            inode: task.identity.inode,
            mtime: task.mtime,
            changed: task.identity.changed,
            state,
            tree,
            rule: None,
            passed: Passed::default(),
        });
        match task.up {
            Up::Node(parent) => self.deliver(parent, &tree),
            Up::Origin { .. } => *lock(&self.totals) = tree,
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
                parent: node.up.id(),
                name: node.name.clone(),
                device: node.identity.device,
                inode: node.identity.inode,
                mtime: node.mtime,
                changed: node.identity.changed,
                state: DirState::Read(node.own),
                tree,
                rule: node.rule.clone(),
                passed: node.passed.clone(),
            });
            let Some(parent) = node.up.node().cloned() else {
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
