//! Removing a directory's contents and surveying a tree: descriptor-
//! relative, never following a symlink and never leaving the directory's
//! device. The top levels of a tree are emptied in parallel; below
//! `PARALLEL_DEPTH` a subtree is emptied by an iterative walk (no
//! recursion on the stack).

use std::ffi::OsStr;
use std::path::PathBuf;

use rayon::prelude::*;

use super::dir::{DirHandle, ListError, list_dir};
use super::process::raise_open_file_limit;
use crate::domain::{Bytes, Entry, EntryKind, Errno, Leftovers, UnixTime};

/// What removing a directory's contents did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// Allocated bytes of what was unlinked; a file with `n` links counts
    /// `1/n`, as in a scan.
    pub removed: Bytes,
    /// Names removed: files, links and directories.
    pub unlinked: u64,
    /// What is still there, and why.
    pub leftovers: Leftovers,
}

/// A directory being emptied.
struct Frame {
    dir: DirHandle,
    /// Relative to the directory being emptied.
    path: PathBuf,
    /// Entries still to remove.
    entries: Vec<Entry>,
    /// Something below could not be removed, so this directory stays.
    kept: bool,
}

impl Frame {
    fn open(dir: DirHandle, path: PathBuf) -> Result<Frame, Errno> {
        let entries = list_dir(&dir).map_err(errno)?;
        Ok(Frame {
            dir,
            path,
            entries,
            kept: false,
        })
    }
}

/// Removes everything inside `dir`, which lies on `device`; `dir` itself
/// stays. Symlinks are unlinked, never followed; a directory on another
/// device is left and reported; an entry that cannot be removed is
/// recorded and the rest continues. Nothing is made writable to remove it.
/// `cancelled` is asked before each entry. One thread, iteratively: what
/// a `Remover` does below its parallel levels.
pub fn remove_contents(dir: &DirHandle, device: u64, cancelled: &dyn Fn() -> bool) -> Removal {
    raise_open_file_limit();
    let walk = Walk {
        device,
        cancelled,
        removed: &|_| {},
    };
    match dir.reopen() {
        Ok(dir) => iterative(dir, PathBuf::new(), &walk).removal,
        Err(error) => failed_at(PathBuf::new(), errno(error)),
    }
}

/// Levels of a tree emptied in parallel, the directory being emptied the
/// first; deeper subtrees are emptied by the iterative walk, so a very deep
/// tree cannot overflow a worker's stack.
pub const PARALLEL_DEPTH: usize = 32;

/// Empties directories with a pool of threads of its own, made once for a
/// cleanup: the subdirectories of a directory are emptied in parallel.
pub struct Remover {
    pool: Option<rayon::ThreadPool>,
}

impl Remover {
    /// A remover with `threads` threads (at least one); without a pool
    /// (the system refused the threads) it empties on the caller's thread.
    pub fn new(threads: usize) -> Remover {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .thread_name(|n| format!("metla-remove-{n}"))
            .build();
        Remover { pool: pool.ok() }
    }

    /// As `remove_contents`, with the subdirectories of each of the top
    /// `PARALLEL_DEPTH` levels emptied in parallel; `removed` is told the
    /// bytes of every entry as it is unlinked. The result does not depend
    /// on timing: the leftovers come in the order the sequential walk
    /// records them.
    pub fn remove_contents(
        &self,
        dir: &DirHandle,
        device: u64,
        cancelled: &(dyn Fn() -> bool + Sync),
        removed: &(dyn Fn(Bytes) + Sync),
    ) -> Removal {
        raise_open_file_limit();
        let walk = Walk {
            device,
            cancelled,
            removed,
        };
        let Ok(dir) = dir.reopen() else {
            return failed_at(PathBuf::new(), Errno(libc::EBADF));
        };
        match &self.pool {
            Some(pool) => {
                pool.install(|| parallel(dir, PathBuf::new(), 0, &walk))
                    .removal
            }
            None => iterative(dir, PathBuf::new(), &walk).removal,
        }
    }
}

/// What a walk needs: the device it stays on, whether to stop, and where
/// to report removed bytes.
struct Walk<'a, C: ?Sized, R: ?Sized> {
    device: u64,
    cancelled: &'a C,
    removed: &'a R,
}

/// A directory emptied as far as it could be, and whether it stays (it
/// holds something, or the walk stopped in it).
struct Emptied {
    removal: Removal,
    kept: bool,
}

fn failed_at(path: PathBuf, error: Errno) -> Removal {
    let mut removal = Removal::default();
    removal.leftovers.fail(path, error);
    removal
}

impl Removal {
    /// Adds what `later` did after this, as one walk would have recorded
    /// it: counts add up, the first failures and the mounts follow on.
    fn then(&mut self, later: Removal) {
        self.removed += later.removed;
        self.unlinked += later.unlinked;
        let room = Leftovers::FIRST.saturating_sub(self.leftovers.first.len());
        self.leftovers.failures += later.leftovers.failures;
        self.leftovers
            .first
            .extend(later.leftovers.first.into_iter().take(room));
        self.leftovers.mounts.extend(later.leftovers.mounts);
        self.leftovers.cancelled |= later.leftovers.cancelled;
    }
}

/// One entry of a directory being emptied in parallel: done already, or a
/// subdirectory to empty and then remove.
enum Part {
    Done(Removal, bool),
    Child(DirHandle, PathBuf),
}

/// Empties `dir` (at `path`, `depth` levels below the top): its files go
/// first, on this thread, then its subdirectories in parallel, each then
/// removed when it kept nothing. The parts are joined in the order the
/// sequential walk visits them, the listing from its end.
fn parallel<C, R>(dir: DirHandle, path: PathBuf, depth: usize, walk: &Walk<'_, C, R>) -> Emptied
where
    C: Fn() -> bool + Sync + ?Sized,
    R: Fn(Bytes) + Sync + ?Sized,
{
    if depth >= PARALLEL_DEPTH {
        return iterative(dir, path, walk);
    }
    let entries = match list_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            return Emptied {
                removal: failed_at(path, errno(error)),
                kept: true,
            };
        }
    };
    let mut parts = Vec::with_capacity(entries.len());
    let mut stopped = false;
    for entry in entries.into_iter().rev() {
        if (walk.cancelled)() {
            stopped = true;
            break;
        }
        parts.push(part(&dir, &path, entry, walk));
    }
    let done: Vec<(Removal, bool)> = parts
        .into_par_iter()
        .map(|part| match part {
            Part::Done(removal, kept) => (removal, kept),
            Part::Child(child, at) => {
                let emptied = parallel(child, at.clone(), depth + 1, walk);
                close_child(&dir, at, emptied)
            }
        })
        .collect();
    let mut removal = Removal::default();
    let mut kept = stopped;
    for (part, part_kept) in done {
        removal.then(part);
        kept |= part_kept;
    }
    removal.leftovers.cancelled |= stopped;
    Emptied { removal, kept }
}

/// Unlinks a non-directory now, or opens a subdirectory to empty.
fn part<C, R>(dir: &DirHandle, path: &std::path::Path, entry: Entry, walk: &Walk<'_, C, R>) -> Part
where
    C: Fn() -> bool + Sync + ?Sized,
    R: Fn(Bytes) + Sync + ?Sized,
{
    let at = path.join(&entry.name);
    let outcome = match entry.meta.kind {
        EntryKind::Dir => {
            open_on(dir, &entry.name, walk.device).map(|child| Part::Child(child, at.clone()))
        }
        kind => dir
            .remove(&entry.name, kind)
            .map_err(Problem::Failed)
            .map(|_| {
                (walk.removed)(entry.meta.share());
                Part::Done(
                    Removal {
                        removed: entry.meta.share(),
                        unlinked: 1,
                        leftovers: Leftovers::default(),
                    },
                    false,
                )
            }),
    };
    outcome.unwrap_or_else(|problem| {
        let mut removal = Removal::default();
        let kept = leave(&mut removal, at, problem);
        Part::Done(removal, kept)
    })
}

/// Removes an emptied subdirectory from `dir` unless it stays; its
/// removal and whether it stays.
fn close_child(dir: &DirHandle, at: PathBuf, emptied: Emptied) -> (Removal, bool) {
    let Emptied { mut removal, kept } = emptied;
    if kept {
        return (removal, true);
    }
    let name = at.file_name().unwrap_or_default();
    match dir.remove(name, EntryKind::Dir) {
        Ok(()) => {
            removal.unlinked += 1;
            (removal, false)
        }
        Err(error) => {
            let kept = leave(&mut removal, at, Problem::Failed(error));
            (removal, kept)
        }
    }
}

/// The subdirectory `name` of `dir`, opened without following a symlink,
/// when it is on `device`.
fn open_on(dir: &DirHandle, name: &OsStr, device: u64) -> Result<DirHandle, Problem> {
    let failed = |error| Problem::Failed(errno(error));
    let child = dir.open_child(name).map_err(failed)?;
    let meta = child.meta().map_err(failed)?;
    if meta.device != device {
        return Err(Problem::OtherDevice);
    }
    Ok(child)
}

/// Empties `dir` (at `path`) one entry at a time, depth first, with a
/// stack of open directories instead of recursion.
fn iterative<C, R>(dir: DirHandle, path: PathBuf, walk: &Walk<'_, C, R>) -> Emptied
where
    C: Fn() -> bool + ?Sized,
    R: Fn(Bytes) + ?Sized,
{
    let mut removal = Removal::default();
    let root = match Frame::open(dir, path.clone()) {
        Ok(root) => root,
        Err(error) => {
            return Emptied {
                removal: failed_at(path, error),
                kept: true,
            };
        }
    };
    let mut stack = vec![root];
    let mut kept = false;
    while let Some(frame) = stack.last_mut() {
        if (walk.cancelled)() {
            removal.leftovers.cancelled = true;
            kept = true;
            break;
        }
        match frame.entries.pop() {
            Some(entry) => step(&mut stack, entry, walk, &mut removal),
            None => kept = close(&mut stack, &mut removal),
        }
    }
    Emptied { removal, kept }
}

/// Removes one entry of the innermost directory, or descends into it.
fn step<C, R>(stack: &mut Vec<Frame>, entry: Entry, walk: &Walk<'_, C, R>, removal: &mut Removal)
where
    C: Fn() -> bool + ?Sized,
    R: Fn(Bytes) + ?Sized,
{
    let frame = stack.last_mut().expect("a frame to step in");
    let path = frame.path.join(&entry.name);
    let outcome = match entry.meta.kind {
        EntryKind::Dir => descend(frame, &entry.name, walk.device).map(Some),
        kind => frame
            .dir
            .remove(&entry.name, kind)
            .map_err(Problem::Failed)
            .map(|_| {
                (walk.removed)(entry.meta.share());
                removal.removed += entry.meta.share();
                removal.unlinked += 1;
                None
            }),
    };
    match outcome {
        Ok(Some(child)) => stack.push(Frame { path, ..child }),
        Ok(None) => {}
        Err(problem) => frame.kept |= leave(removal, path, problem),
    }
}

/// Why an entry stays.
enum Problem {
    OtherDevice,
    Failed(Errno),
}

/// Records what stays at `path`; false when nothing does (it was gone
/// already).
fn leave(removal: &mut Removal, path: PathBuf, problem: Problem) -> bool {
    match problem {
        Problem::OtherDevice => removal.leftovers.mounts.push(path),
        Problem::Failed(Errno(libc::ENOENT)) => return false,
        Problem::Failed(error) => removal.leftovers.fail(path, error),
    }
    true
}

/// The subdirectory `name`, opened without following a symlink, when it
/// is on `device`.
fn descend(frame: &Frame, name: &OsStr, device: u64) -> Result<Frame, Problem> {
    let child = open_on(&frame.dir, name, device)?;
    Frame::open(child, PathBuf::new()).map_err(Problem::Failed)
}

/// Removes the innermost directory, now empty, from its parent, unless
/// something inside it stayed; at the top, whether the walk's directory
/// stays.
fn close(stack: &mut Vec<Frame>, removal: &mut Removal) -> bool {
    let done = stack.pop().expect("a frame to close");
    let Some(parent) = stack.last_mut() else {
        return done.kept;
    };
    if done.kept {
        parent.kept = true;
        return false;
    }
    let name = done.path.file_name().unwrap_or_default();
    match parent.dir.remove(name, EntryKind::Dir) {
        Ok(()) => removal.unlinked += 1,
        Err(error) => parent.kept |= leave(removal, done.path, Problem::Failed(error)),
    }
    false
}

fn errno(error: ListError) -> Errno {
    match error {
        ListError::Denied => Errno(libc::EACCES),
        ListError::Gone => Errno(libc::ENOENT),
        ListError::Io(errno) => errno,
    }
}

/// What a tree holds, on its own device: allocated bytes (by the share
/// rule), files and the newest modification time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Survey {
    pub bytes: Bytes,
    pub files: u64,
    pub newest: Option<UnixTime>,
}

/// Surveys the tree below `dir` without following symlinks or leaving
/// `device`. Directories that cannot be read are passed over.
pub fn survey(dir: &DirHandle, device: u64) -> Survey {
    raise_open_file_limit();
    let mut survey = Survey::default();
    let Ok(meta) = dir.meta() else {
        return survey;
    };
    survey.newest = Some(meta.mtime);
    let mut pending: Vec<DirHandle> = dir.reopen().into_iter().collect();
    while let Some(current) = pending.pop() {
        for entry in list_dir(&current).unwrap_or_default() {
            survey.newest = survey.newest.max(Some(entry.meta.mtime));
            match entry.meta.kind {
                EntryKind::Dir if entry.meta.device == device => {
                    pending.extend(current.open_child(&entry.name));
                }
                EntryKind::Dir => {}
                _ => {
                    survey.bytes += entry.meta.share();
                    survey.files += 1;
                }
            }
        }
    }
    survey
}
