use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use super::dir::DirId;
use super::entry::{Bytes, UnixTime};
use super::rule::{Category, Disposal, RuleId};
use super::worktree::Manager;

/// A directory a rule matched and no exclusion hides: the only thing that
/// can be cleaned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: DirId,
    pub path: PathBuf,
    pub rule: RuleId,
    /// The directory's tree size when scanned: an estimate.
    pub bytes: Bytes,
    pub files: u64,
    /// The newest modification time anywhere in the tree.
    pub activity: Option<UnixTime>,
    /// The directory's identity when scanned, for revalidation.
    pub device: u64,
    pub inode: u64,
    /// The tool that manages the worktree it is or lies in, if any.
    pub managed: Option<Manager>,
    /// Checked by default in a cleanup selection.
    pub preselected: bool,
}

impl Candidate {
    pub fn category(&self) -> Category {
        self.rule.category()
    }

    pub fn disposal(&self) -> Disposal {
        self.rule.disposal()
    }
}

/// What "recently active" means for one query: activity no older than
/// `age` before `now`. There is no clock in the core's pure parts; the
/// caller passes the time in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recency {
    now: UnixTime,
    age: Duration,
}

impl Recency {
    /// Build output modified within this age is not preselected.
    pub const DEFAULT_AGE: Duration = Duration::from_secs(14 * 24 * 3600);

    pub fn new(now: UnixTime, age: Duration) -> Recency {
        Recency { now, age }
    }

    /// Whether `activity` is within the age. Unknown activity counts as
    /// recent, so nothing unknown is preselected.
    pub fn is_recent(&self, activity: Option<UnixTime>) -> bool {
        let age = i64::try_from(self.age.as_secs()).unwrap_or(i64::MAX);
        activity.is_none_or(|time| self.now.0.saturating_sub(time.0) <= age)
    }
}

/// How many candidates and how many bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub count: u64,
    pub bytes: Bytes,
}

impl Tally {
    pub fn add(&mut self, bytes: Bytes) {
        self.count += 1;
        self.bytes += bytes;
    }

    pub fn merge(&mut self, other: &Tally) {
        self.count += other.count;
        self.bytes += other.bytes;
    }
}

/// A path the user wants kept: absolute, without `.` or `..` components
/// and without a trailing slash.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Exclusion(PathBuf);

impl Exclusion {
    pub fn new(path: &Path) -> Option<Exclusion> {
        let parent = path.components().any(|c| c == Component::ParentDir);
        (path.is_absolute() && !parent).then(|| Exclusion(path.components().collect()))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Whether a candidate at `path` is hidden: it is the kept path, lies
    /// beneath it, or holds it (removing it would remove the kept path).
    /// Paths are compared by whole components.
    pub fn hides(&self, path: &Path) -> bool {
        path.starts_with(&self.0) || self.0.starts_with(path)
    }
}
