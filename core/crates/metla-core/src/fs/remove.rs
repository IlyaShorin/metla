//! Removing a directory's contents and surveying a tree: descriptor-
//! relative, iterative (no recursion on the stack), never following a
//! symlink and never leaving the directory's device.

use std::ffi::OsStr;
use std::path::PathBuf;

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
/// `cancelled` is asked before each entry.
pub fn remove_contents(dir: &DirHandle, device: u64, cancelled: &dyn Fn() -> bool) -> Removal {
    raise_open_file_limit();
    let mut removal = Removal::default();
    let root = dir
        .reopen()
        .map_err(errno)
        .and_then(|dir| Frame::open(dir, PathBuf::new()));
    let root = match root {
        Ok(root) => root,
        Err(error) => {
            removal.leftovers.fail(PathBuf::new(), error);
            return removal;
        }
    };
    let mut stack = vec![root];
    while let Some(frame) = stack.last_mut() {
        if cancelled() {
            removal.leftovers.cancelled = true;
            break;
        }
        match frame.entries.pop() {
            Some(entry) => step(&mut stack, entry, device, &mut removal),
            None => close(&mut stack, &mut removal),
        }
    }
    removal
}

/// Removes one entry of the innermost directory, or descends into it.
fn step(stack: &mut Vec<Frame>, entry: Entry, device: u64, removal: &mut Removal) {
    let frame = stack.last_mut().expect("a frame to step in");
    let path = frame.path.join(&entry.name);
    let outcome = match entry.meta.kind {
        EntryKind::Dir => descend(frame, &entry.name, device).map(Some),
        kind => frame
            .dir
            .remove(&entry.name, kind)
            .map_err(Problem::Failed)
            .map(|_| {
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
    let failed = |error| Problem::Failed(errno(error));
    let child = frame.dir.open_child(name).map_err(failed)?;
    let meta = child.meta().map_err(failed)?;
    if meta.device != device {
        return Err(Problem::OtherDevice);
    }
    Frame::open(child, PathBuf::new()).map_err(Problem::Failed)
}

/// Removes the innermost directory, now empty, from its parent, unless
/// something inside it stayed.
fn close(stack: &mut Vec<Frame>, removal: &mut Removal) {
    let done = stack.pop().expect("a frame to close");
    let Some(parent) = stack.last_mut() else {
        return;
    };
    if done.kept {
        parent.kept = true;
        return;
    }
    let name = done.path.file_name().unwrap_or_default();
    match parent.dir.remove(name, EntryKind::Dir) {
        Ok(()) => removal.unlinked += 1,
        Err(error) => parent.kept |= leave(removal, done.path, Problem::Failed(error)),
    }
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
