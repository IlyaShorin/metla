//! Whether a linked worktree can be removed, decided with stock Git
//! commands, and its removal through `git worktree remove` (never forced).

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::porcelain::{Unparsable, count, status, worktree_list};
use super::run::{Git, Output, RunError};
use crate::domain::{Manager, Reason, UnixTime, WorktreeStatus};

/// What a worktree's status depends on besides the worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeSettings {
    /// The user's home directory: Codex Desktop keeps worktrees in it.
    pub home: PathBuf,
    /// A worktree changed more recently than this is in use.
    pub stale_after: Duration,
}

impl WorktreeSettings {
    pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);
}

/// The longest `.git` pointer file read.
const POINTER_LIMIT: u64 = 4096;

/// Files in a worktree's git directory that mean an operation is under
/// way.
const OPERATIONS: [&str; 6] = [
    "rebase-merge",
    "rebase-apply",
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
];

/// Files in a worktree's git directory that Git touches when it is used.
const TOUCHED: [&str; 3] = ["index", "HEAD", "logs/HEAD"];

/// A linked worktree that its repository lists.
struct Linked {
    /// The worktree's own git directory, `<repository git dir>/worktrees/<name>`.
    own: PathBuf,
    locked: bool,
    prunable: bool,
}

/// Whether the linked worktree at `path` may be removed. `activity` is
/// the newest change seen in its tree (`None`: unknown, which counts as
/// in use). Any failure of Git blocks it.
pub fn inspect_worktree(
    git: &Git,
    path: &Path,
    activity: Option<UnixTime>,
    now: UnixTime,
    settings: &WorktreeSettings,
) -> WorktreeStatus {
    let mut reasons = BTreeSet::new();
    reasons.extend(Manager::of(path, &settings.home).map(Reason::Managed));
    let linked = match linked(git, path) {
        Ok(linked) => linked,
        Err(reason) => {
            reasons.insert(reason);
            return WorktreeStatus::new(None, reasons);
        }
    };
    let fresh = Freshness {
        activity,
        now,
        age: settings.stale_after,
    };
    reasons.extend(state_reasons(&linked, path, &fresh));
    match git_reasons(git, path) {
        Ok((branch, found)) => {
            reasons.extend(found);
            WorktreeStatus::new(branch, reasons)
        }
        Err(failure) => {
            reasons.insert(Reason::InspectionFailed(failure));
            WorktreeStatus::new(None, reasons)
        }
    }
}

/// Removes the linked worktree at `path` with `git worktree remove`, run
/// in its repository. Git refuses a worktree that is dirty or locked; its
/// refusal is the error.
pub fn remove_worktree(git: &Git, path: &Path) -> Result<(), String> {
    let pointer = pointer(path).map_err(|_| "not a linked worktree".to_owned())?;
    let args = [
        OsStr::new("worktree"),
        OsStr::new("remove"),
        path.as_os_str(),
    ];
    let output = git.run(&pointer.common, &args, &[]);
    succeeded(output.map_err(|e| e.to_string())?).map(|_| ())
}

/// Where a worktree's `.git` pointer file leads.
struct Pointer {
    /// The repository's git directory.
    common: PathBuf,
    /// The worktree's own git directory, `<common>/worktrees/<name>`.
    own: PathBuf,
}

fn pointer(path: &Path) -> Result<Pointer, Reason> {
    let file = path.join(".git");
    let meta = std::fs::symlink_metadata(&file).map_err(|_| Reason::NotListed)?;
    if !meta.is_file() {
        return Err(Reason::NotListed);
    }
    let mut head = Vec::new();
    let read = File::open(&file).and_then(|f| f.take(POINTER_LIMIT).read_to_end(&mut head));
    read.map_err(|e| Reason::InspectionFailed(e.to_string()))?;
    let line = head.split(|b| *b == b'\n').next().unwrap_or_default();
    let target = line.strip_prefix(b"gitdir:").ok_or(Reason::NotListed)?;
    let own = path.join(OsStr::from_bytes(target.trim_ascii()));
    let worktrees = own
        .parent()
        .filter(|p| p.file_name() == Some(OsStr::new("worktrees")));
    let common = worktrees.and_then(Path::parent).ok_or(Reason::NotListed)?;
    Ok(Pointer {
        common: common.to_path_buf(),
        own,
    })
}

/// The worktree as its repository lists it: exactly one listed entry is
/// this directory, and it is not the main worktree.
fn linked(git: &Git, path: &Path) -> Result<Linked, Reason> {
    let Pointer { common, own } = pointer(path)?;
    let args = ["worktree", "list", "--porcelain", "-z"].map(OsStr::new);
    let output = git.run(&common, &args, &[]).map_err(failed)?;
    let listed = worktree_list(&succeeded(output).map_err(Reason::InspectionFailed)?);
    let listed = listed.map_err(|Unparsable(what)| Reason::InspectionFailed(what))?;
    let canonical =
        std::fs::canonicalize(path).map_err(|e| Reason::InspectionFailed(e.to_string()))?;
    let same = |entry: &&super::porcelain::Listed| {
        std::fs::canonicalize(&entry.path).is_ok_and(|p| p == canonical)
    };
    let matches: Vec<(usize, _)> = listed.iter().enumerate().filter(|(_, e)| same(e)).collect();
    match matches.as_slice() {
        [(at, entry)] if *at > 0 => Ok(Linked {
            own,
            locked: entry.locked,
            prunable: entry.prunable,
        }),
        _ => Err(Reason::NotListed),
    }
}

/// How recent "in use" is.
struct Freshness {
    activity: Option<UnixTime>,
    now: UnixTime,
    age: Duration,
}

/// Reasons read from the worktree's git directory, without running Git.
fn state_reasons(linked: &Linked, path: &Path, fresh: &Freshness) -> Vec<Reason> {
    let exists = |name: &str| std::fs::symlink_metadata(linked.own.join(name)).is_ok();
    let flags = [
        (linked.locked, Reason::Locked),
        (linked.prunable, Reason::Prunable),
        (
            OPERATIONS.iter().any(|op| exists(op)),
            Reason::OperationInProgress,
        ),
        (
            exists("index.lock") || recent(linked, path, fresh),
            Reason::InUse,
        ),
    ];
    flags
        .into_iter()
        .filter(|(set, _)| *set)
        .map(|(_, r)| r)
        .collect()
}

/// Whether the worktree changed within the age: in its tree as last seen
/// (unknown counts as recent), or in the files Git touches when it is
/// used.
fn recent(linked: &Linked, path: &Path, fresh: &Freshness) -> bool {
    let Some(activity) = fresh.activity else {
        return true;
    };
    let touched = TOUCHED.iter().map(|name| linked.own.join(name));
    let times = touched
        .chain([path.to_path_buf()])
        .filter_map(|p| mtime(&p));
    let newest = times.fold(activity, UnixTime::max);
    let age = i64::try_from(fresh.age.as_secs()).unwrap_or(i64::MAX);
    fresh.now.0.saturating_sub(newest.0) <= age
}

fn mtime(path: &Path) -> Option<UnixTime> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| UnixTime(m.mtime()))
}

/// The branch, and the reasons `git status` and `git rev-list` give.
fn git_reasons(git: &Git, path: &Path) -> Result<(Option<String>, Vec<Reason>), String> {
    let args = [
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
        "-z",
    ]
    .map(OsStr::new);
    let output = succeeded(git.run(path, &args, &[]).map_err(|e| e.to_string())?)?;
    let status = status(&output).map_err(|Unparsable(what)| what)?;
    let unpushed = unpushed(git, path)?;
    let flags = [
        (
            status.staged + status.modified + status.conflicted > 0,
            Reason::Uncommitted,
        ),
        (status.untracked > 0, Reason::Untracked),
        (unpushed, Reason::Unpushed),
    ];
    let reasons = flags.into_iter().filter(|(set, _)| *set).map(|(_, r)| r);
    Ok((status.branch, reasons.collect()))
}

/// Commits reachable from HEAD and from no remote ref: this covers
/// commits ahead of a remote upstream, a branch without upstream, a
/// detached HEAD, an upstream that is a local branch, and a repository
/// without remotes (every commit is unpushed).
fn unpushed(git: &Git, path: &Path) -> Result<bool, String> {
    let args = ["rev-list", "--count", "HEAD", "--not", "--remotes"].map(OsStr::new);
    let output = succeeded(git.run(path, &args, &[]).map_err(|e| e.to_string())?)?;
    Ok(count(&output).map_err(|Unparsable(what)| what)? > 0)
}

/// Standard output of a run that succeeded; otherwise what Git said.
fn succeeded(output: Output) -> Result<Vec<u8>, String> {
    match output.code {
        0 => Ok(output.stdout),
        code => {
            let said = String::from_utf8_lossy(&output.stderr);
            Err(format!("git exited with {code}: {}", said.trim()))
        }
    }
}

fn failed(error: RunError) -> Reason {
    Reason::InspectionFailed(error.to_string())
}
