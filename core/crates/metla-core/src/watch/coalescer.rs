//! The coalescer: file-system events in, a bounded set of due rescan
//! tasks out. A pure state machine — the time is passed in, nothing is
//! read or written — so every decision can be tested with a fake clock.
//!
//! * Events are kept per directory. A directory becomes due after a
//!   *settle time* of quiet; every new event for it (or below it, for a
//!   recursive task) restarts the wait and multiplies the settle time, up
//!   to a cap. A directory is due at the latest one cap after its first
//!   event, so one that never stops changing is rescanned once per cap.
//!   A directory rescanned less than a cap ago starts from its last
//!   settle time multiplied again, so steady churn backs off too.
//! * A recursive task absorbs the tasks below it; more changed children
//!   of one directory than `collapse` become one recursive task for it.
//! * Memory is bounded: past `max_dirs` tracked directories the deepest
//!   collapse into their parents, and at the root that is a full rescan.
//! * Lost events, wrapped ids and a changed root ask for a full rescan;
//!   events outside the root or inside an ignored path are dropped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::flags::Flags;
use crate::domain::RescanTask;

/// How the coalescer waits, from the power profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// Quiet time before a directory is rescanned.
    pub settle: Duration,
    /// What each new event multiplies the settle time by.
    pub factor: u32,
    /// The longest settle time, and the longest a change waits.
    pub cap: Duration,
    /// The most tasks handed out at once.
    pub batch: usize,
    /// Changed children of one directory that become one recursive task.
    pub collapse: usize,
    /// The most directories tracked at once.
    pub max_dirs: usize,
}

impl Timing {
    /// The settle time after `settle` for one more event: multiplied by
    /// the factor, and never shorter than the settle time or longer than
    /// the cap.
    fn longer(&self, settle: Duration) -> Duration {
        (settle * self.factor).clamp(self.settle, self.cap)
    }
}

/// One file-system event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub path: PathBuf,
    pub flags: Flags,
    /// The stream's event id: events are replayed from a stored one.
    pub id: u64,
}

/// How long a change waits: since its first and latest event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wait {
    first: Duration,
    last: Duration,
    settle: Duration,
    /// The id of the first event not yet handed out.
    since: u64,
}

/// A directory with changes not yet handed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dirty {
    recursive: bool,
    wait: Wait,
}

impl Wait {
    fn new(now: Duration, settle: Duration, id: u64) -> Wait {
        Wait {
            first: now,
            last: now,
            settle,
            since: id,
        }
    }

    fn grow(&mut self, now: Duration, id: u64, timing: &Timing) {
        self.last = self.last.max(now);
        self.settle = timing.longer(self.settle);
        self.since = self.since.min(id);
    }

    fn merge(&mut self, other: &Wait) {
        self.first = self.first.min(other.first);
        self.last = self.last.max(other.last);
        self.settle = self.settle.max(other.settle);
        self.since = self.since.min(other.since);
    }

    fn due(&self, timing: &Timing) -> Duration {
        let quiet = self.last + self.settle.min(timing.cap);
        quiet.min(self.first + timing.cap)
    }
}

/// When a directory was last handed out, and with what settle time.
#[derive(Clone, Copy, Debug)]
struct Cooling {
    fired: Duration,
    settle: Duration,
}

impl Cooling {
    /// Handed out less than a cap ago: a new change backs off further.
    fn recent(&self, now: Duration, timing: &Timing) -> bool {
        now < self.fired + timing.cap
    }
}

pub struct Coalescer {
    root: PathBuf,
    ignored: Vec<PathBuf>,
    timing: Option<Timing>,
    dirty: BTreeMap<PathBuf, Dirty>,
    full: Option<Wait>,
    cooling: BTreeMap<PathBuf, Cooling>,
    /// The id of the latest event seen.
    latest: Option<u64>,
}

impl Coalescer {
    /// Watches `root`, dropping events inside `ignored`; `None` timing is
    /// the `Off` profile, which tracks and hands out nothing.
    pub fn new(root: &Path, ignored: Vec<PathBuf>, timing: Option<Timing>) -> Coalescer {
        Coalescer {
            root: root.to_path_buf(),
            ignored,
            timing,
            dirty: BTreeMap::new(),
            full: None,
            cooling: BTreeMap::new(),
            latest: None,
        }
    }

    /// Takes the timing of another profile; `None` forgets every change.
    pub fn set_timing(&mut self, timing: Option<Timing>) {
        if timing.is_none() {
            self.dirty.clear();
            self.full = None;
            self.cooling.clear();
        }
        self.timing = timing;
    }

    pub fn on_event(&mut self, event: &Event, now: Duration) {
        self.latest = self.latest.max(Some(event.id));
        let Some(timing) = self.timing else {
            return;
        };
        if event.flags.lost_history() {
            return self.mark_full(event.id, now, &timing);
        }
        let Some(path) = self.inside(&event.path) else {
            return;
        };
        let mount = event.flags.mount_changed();
        let path = match mount {
            true => path
                .parent()
                .filter(|p| p.starts_with(&self.root))
                .map(Path::to_path_buf),
            false => Some(path),
        };
        let recursive = event.flags.contains(Flags::MUST_SCAN_SUBDIRS) && !mount;
        if let Some(path) = path.filter(|_| !event.flags.contains(Flags::HISTORY_DONE)) {
            self.mark(path, recursive, event.id, now, &timing);
        }
    }

    /// The tasks due at `now`, at most a batch, deepest first. A full
    /// rescan comes alone and replaces everything else.
    pub fn due(&mut self, now: Duration) -> Vec<RescanTask> {
        let Some(timing) = self.timing else {
            return Vec::new();
        };
        if self.full.is_some_and(|full| full.due(&timing) <= now) {
            self.full = None;
            self.dirty.clear();
            return vec![RescanTask::Full];
        }
        let mut due: Vec<(Duration, PathBuf)> = self
            .dirty
            .iter()
            .filter(|(_, d)| d.wait.due(&timing) <= now)
            .map(|(path, d)| (d.wait.due(&timing), path.clone()))
            .collect();
        due.sort();
        due.truncate(timing.batch);
        let mut tasks: Vec<(usize, RescanTask)> = Vec::new();
        for (_, path) in due {
            let dirty = self.dirty.remove(&path).unwrap_or_else(|| unreachable!());
            self.cool(&path, dirty.wait.settle, now, &timing);
            let depth = path.components().count();
            let task = match dirty.recursive {
                true => RescanTask::Recursive(path),
                false => RescanTask::Shallow(path),
            };
            tasks.push((depth, task));
        }
        tasks.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        tasks.into_iter().map(|(_, task)| task).collect()
    }

    /// Makes every waiting change due at its last event, so the next
    /// `due` hands it out; a later event settles again.
    pub fn expedite(&mut self) {
        let waits = self
            .full
            .iter_mut()
            .chain(self.dirty.values_mut().map(|d| &mut d.wait));
        for wait in waits {
            wait.settle = Duration::ZERO;
        }
    }

    /// When the next task becomes due, if any is waiting.
    pub fn next_deadline(&self) -> Option<Duration> {
        let timing = self.timing?;
        let full = self.full.iter().map(|f| f.due(&timing));
        let dirs = self.dirty.values().map(|d| d.wait.due(&timing));
        full.chain(dirs).min()
    }

    /// How many tasks are waiting (a full rescan counts as one).
    pub fn pending(&self) -> usize {
        self.dirty.len() + usize::from(self.full.is_some())
    }

    /// The id of the last event whose change has been handed out with
    /// everything before it: where a restarted stream resumes.
    pub fn settled(&self) -> Option<u64> {
        let dirs = self.dirty.values().map(|d| &d.wait);
        match self.full.iter().chain(dirs).map(|w| w.since).min() {
            Some(since) => Some(since.saturating_sub(1)),
            None => self.latest,
        }
    }

    /// The event's path below the root, unless it is ignored.
    fn inside(&self, path: &Path) -> Option<PathBuf> {
        let path: PathBuf = path.components().collect();
        let ignored = self.ignored.iter().any(|i| path.starts_with(i));
        (path.starts_with(&self.root) && !ignored).then_some(path)
    }

    fn mark_full(&mut self, id: u64, now: Duration, timing: &Timing) {
        let mut full = self.full.unwrap_or(Wait::new(now, timing.settle, id));
        for dirty in self.dirty.values() {
            full.merge(&dirty.wait);
        }
        full.grow(now, id, timing);
        self.dirty.clear();
        self.full = Some(full);
    }

    fn mark(&mut self, path: PathBuf, recursive: bool, id: u64, now: Duration, timing: &Timing) {
        if let Some(full) = &mut self.full {
            return full.grow(now, id, timing);
        }
        if recursive && path == self.root {
            return self.mark_full(id, now, timing);
        }
        let covering = path
            .ancestors()
            .find(|a| self.dirty.get(*a).is_some_and(|d| d.recursive))
            .map(Path::to_path_buf);
        if let Some(dirty) = covering.and_then(|a| self.dirty.get_mut(&a)) {
            return dirty.wait.grow(now, id, timing);
        }
        let settle = self.start_settle(&path, now, timing);
        let entry = self.dirty.entry(path.clone()).or_insert(Dirty {
            recursive,
            wait: Wait::new(now, settle, id),
        });
        if entry.wait.first < now || entry.recursive != recursive {
            entry.wait.grow(now, id, timing);
        }
        entry.recursive |= recursive;
        if recursive {
            self.absorb(&path);
        }
        self.collapse_children(&path, timing);
        self.bound(timing);
    }

    /// The settle time a new entry starts with: longer when the directory
    /// was rescanned less than a cap ago.
    fn start_settle(&self, path: &Path, now: Duration, timing: &Timing) -> Duration {
        let recent = self.cooling.get(path).filter(|c| c.recent(now, timing));
        recent.map_or(timing.settle, |c| timing.longer(c.settle))
    }

    /// Merges every entry below `path` into it.
    fn absorb(&mut self, path: &Path) {
        let below: Vec<PathBuf> = self
            .dirty
            .range(path.to_path_buf()..)
            .skip(1)
            .take_while(|(p, _)| p.starts_with(path))
            .map(|(p, _)| p.clone())
            .collect();
        for inner in below {
            let dirty = self.dirty.remove(&inner).unwrap_or_else(|| unreachable!());
            if let Some(outer) = self.dirty.get_mut(path) {
                outer.wait.merge(&dirty.wait);
            }
        }
    }

    /// Too many changed children of one directory become one recursive
    /// task for it.
    fn collapse_children(&mut self, path: &Path, timing: &Timing) {
        let Some(parent) = path.parent().filter(|p| p.starts_with(&self.root)) else {
            return;
        };
        let siblings = self
            .dirty
            .keys()
            .filter(|p| p.parent() == Some(parent))
            .count();
        if siblings > timing.collapse {
            self.collapse_into(parent, timing);
        }
    }

    /// Replaces every entry below `parent` by one recursive entry for it;
    /// for the root, by a full rescan.
    fn collapse_into(&mut self, parent: &Path, timing: &Timing) {
        let below: Vec<PathBuf> = self
            .dirty
            .keys()
            .filter(|p| p.starts_with(parent))
            .cloned()
            .collect();
        let mut merged: Option<Wait> = None;
        for path in below {
            let dirty = self.dirty.remove(&path).unwrap_or_else(|| unreachable!());
            merged.get_or_insert(dirty.wait).merge(&dirty.wait);
        }
        let Some(merged) = merged else {
            return;
        };
        match parent == self.root {
            true => {
                self.mark_full(merged.since, merged.last, timing);
                if let Some(full) = &mut self.full {
                    full.merge(&merged);
                }
            }
            false => {
                let dirty = Dirty {
                    recursive: true,
                    wait: merged,
                };
                self.dirty.insert(parent.to_path_buf(), dirty);
            }
        }
    }

    /// Past `max_dirs`, the deepest entries collapse into their parents
    /// until the rest fits; collapsing into the root is a full rescan.
    fn bound(&mut self, timing: &Timing) {
        while self.dirty.len() > timing.max_dirs {
            let deepest = self.dirty.keys().max_by_key(|p| p.components().count());
            let Some(deepest) = deepest else {
                return;
            };
            let inside = deepest.parent().filter(|p| p.starts_with(&self.root));
            let parent = inside.unwrap_or(&self.root).to_path_buf();
            self.collapse_into(&parent, timing);
        }
    }

    fn cool(&mut self, path: &Path, settle: Duration, now: Duration, timing: &Timing) {
        self.cooling.retain(|_, c| c.recent(now, timing));
        if self.cooling.len() >= timing.max_dirs {
            return;
        }
        let fired = now;
        self.cooling
            .insert(path.to_path_buf(), Cooling { fired, settle });
    }
}
