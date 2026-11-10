use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::coverage::Coverage;
use super::dir::DirId;
use super::entry::{Bytes, Entry, UnixTime};
use super::kind::KindTotals;
use super::totals::TreeTotals;

/// The result of one scan of a root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanSummary {
    pub root: PathBuf,
    /// The home directory the rules were compiled for (`None` without
    /// rules).
    pub home: Option<PathBuf>,
    pub started: UnixTime,
    pub finished: UnixTime,
    pub elapsed: Duration,
    pub threads: usize,
    /// The root's tree totals.
    pub totals: TreeTotals,
    /// The root's bytes by file kind (all zero when the scan had no rules).
    pub kinds: KindTotals,
    pub coverage: Coverage,
}

impl ScanSummary {
    pub fn is_complete(&self) -> bool {
        self.coverage.is_complete()
    }
}

/// Counters of a running scan, sampled at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    /// Directories recorded so far, read or not.
    pub dirs: u64,
    pub files: u64,
    pub bytes: Bytes,
    /// Directories that could not be read for lack of permission.
    pub denied: u64,
}

/// A shallow directory of the live tree: its totals so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveDir {
    pub id: DirId,
    pub parent: Option<DirId>,
    pub name: OsString,
    pub depth: u32,
    pub bytes: Bytes,
    pub files: u64,
    pub dirs: u64,
    /// Every directory below it has been counted.
    pub done: bool,
}

/// What the walker knows about one directory it has just read: the input a
/// pure classifier needs.
#[derive(Clone, Copy, Debug)]
pub struct DirFacts<'a> {
    pub path: &'a Path,
    pub entries: &'a [Entry],
    /// The parent's entries whose names the rules want as markers.
    pub parent_markers: &'a [OsString],
    /// The same for the grandparent.
    pub grandparent_markers: &'a [OsString],
    /// An ancestor directory contains `.git`.
    pub inside_git_repo: bool,
    /// The heads of the small files the rules asked for, when the
    /// directory holds them as regular files.
    pub probes: &'a [Probe],
}

/// The first bytes of a small file a rule inspects (a `.git` pointer, a
/// `CACHEDIR.TAG`), read by the walker with a bounded read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub name: OsString,
    pub head: Vec<u8>,
}
