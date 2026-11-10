//! Bringing a picture up to date with one rescan task: find the directory
//! in the picture, rescan it with what the picture says about it, and
//! write the change in one transaction.

use std::path::Path;

use crate::domain::{DirRecord, DirState, RescanTask, UnixTime};
use crate::index::{IndexError, IndexReader, IndexWriter, PictureId, Snapshot};
use crate::scan::{Inherited, ScanControl, ScanError, ScanOptions, Stored, Target, rescan};

/// A recursive rescan holds its rows in memory until they are written in
/// one transaction; a subtree larger than this is scanned again whole.
pub const MAX_SUBTREE: u64 = 100_000;

/// What applying a task did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refreshed {
    /// The picture is up to date for it.
    Applied,
    /// There is no picture of this root to bring up to date.
    NoPicture,
    /// Only a full scan brings the picture up to date.
    NeedsFullScan,
}

#[derive(Debug)]
pub enum RefreshError {
    Index(IndexError),
    Scan(ScanError),
}

impl From<IndexError> for RefreshError {
    fn from(error: IndexError) -> RefreshError {
        RefreshError::Index(error)
    }
}

impl From<ScanError> for RefreshError {
    fn from(error: ScanError) -> RefreshError {
        RefreshError::Scan(error)
    }
}

/// What to rescan, read from the picture.
struct Plan {
    picture: PictureId,
    target: Target,
    next_id: crate::domain::DirId,
    /// Directories in the subtree, as last seen.
    size: u64,
}

/// Applies `task` to the picture of `options`' root. A directory that is
/// no longer there makes its parent the one rescanned.
pub fn refresh(
    task: &RescanTask,
    options: &ScanOptions,
    reader: &mut IndexReader,
    writer: &IndexWriter,
    control: &ScanControl,
    now: UnixTime,
) -> Result<Refreshed, RefreshError> {
    let (mut path, mut whole) = match task {
        RescanTask::Full => return Ok(Refreshed::NeedsFullScan),
        RescanTask::Shallow(path) => (path.clone(), false),
        RescanTask::Recursive(path) => (path.clone(), true),
    };
    loop {
        let Some(plan) = reader.read(|snap| plan(snap, &path, whole, options))? else {
            return Ok(Refreshed::NoPicture);
        };
        let root = plan.target.parent.is_none();
        if plan.target.stored.is_none() && (root || plan.size > MAX_SUBTREE) {
            return Ok(Refreshed::NeedsFullScan);
        }
        let parent = plan.target.path.parent().map(Path::to_path_buf);
        match rescan(options, plan.target, plan.next_id, control)? {
            Some(change) => {
                let applied = writer.apply(plan.picture, change, now)?;
                return Ok(match applied {
                    true => Refreshed::Applied,
                    false => Refreshed::NoPicture,
                });
            }
            None if root => return Ok(Refreshed::NeedsFullScan),
            None => {
                path = parent.unwrap_or_default();
                whole = false;
            }
        }
    }
}

/// The target for rescanning `path` (whole or shallow), from the picture.
/// A directory the picture does not have yet is found by a shallow rescan
/// of its nearest ancestor that it has; one that was not read has nothing
/// to compare with and is walked whole.
fn plan(
    snap: &Snapshot<'_>,
    path: &Path,
    whole: bool,
    options: &ScanOptions,
) -> Result<Option<Plan>, IndexError> {
    let picture = snap.scan()?.and_then(|scan| scan.picture);
    let Some(picture) = picture.filter(|p| p.root == options.root().path()) else {
        return Ok(None);
    };
    let Some((at, row)) = snap.nearest(path)? else {
        return Ok(None);
    };
    let read = matches!(row.state, DirState::Read(_));
    let whole = whole && at == path || !read;
    let inherited = match row.parent {
        Some(parent) => inherited(snap, parent)?,
        None => options.root_inherits(),
    };
    let stored = match whole {
        true => None,
        false => Some(Stored {
            children: snap.children(row.id)?,
            row: row.clone(),
        }),
    };
    Ok(Some(Plan {
        picture: picture.id(),
        next_id: snap.next_id()?,
        size: row.tree.dirs,
        target: Target {
            path: at,
            id: row.id,
            parent: row.parent,
            inherited,
            stored,
        },
    }))
}

/// What the directory `parent` hands down to its children: what it passed
/// on, and its own parent's markers.
fn inherited(snap: &Snapshot<'_>, parent: crate::domain::DirId) -> Result<Inherited, IndexError> {
    let missing = || IndexError::Corrupt(format!("directory {} has no row", parent.0));
    let row: DirRecord = snap.record(parent)?.ok_or_else(missing)?;
    let grandparent = match row.parent {
        Some(id) => snap.record(id)?.map(|g| g.passed.markers),
        None => None,
    };
    Ok(Inherited {
        passed: row.passed,
        grandparent_markers: grandparent.unwrap_or_default(),
    })
}
