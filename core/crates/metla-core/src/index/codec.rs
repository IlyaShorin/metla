//! How domain values are stored in columns.
//!
//! `flags` holds a directory's state: 0 for a read directory, a gap or skip
//! code in the low byte, and for an I/O failure the errno above it.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use super::IndexError;
use crate::domain::{
    Bytes, DirId, DirRecord, DirState, Errno, Gap, GapKind, OwnTotals, Skip, TreeTotals, UnixTime,
};

const READ: i64 = 0;
const CODE_BITS: u32 = 8;
const CODE_MASK: i64 = 0xff;
/// Gap kinds are stored as 1, 2, ... in the order of `GapKind::ALL`,
/// skips as 16, 17, ... in the order of `Skip::ALL`. Part of the file
/// format: new kinds go at the end.
const GAP_BASE: i64 = 1;
const SKIP_BASE: i64 = 16;

pub fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub fn count(value: i64) -> u64 {
    value.unsigned_abs()
}

fn code_in<T: PartialEq>(all: &[T], base: i64, value: &T) -> i64 {
    let position = all.iter().position(|v| v == value).unwrap_or(all.len());
    base + int(position as u64)
}

fn value_in<T: Copy>(all: &[T], base: i64, code: i64) -> Option<T> {
    let position = usize::try_from(code - base).ok()?;
    all.get(position).copied()
}

pub fn gap_code(kind: GapKind) -> i64 {
    code_in(&GapKind::ALL, GAP_BASE, &kind)
}

pub fn skip_code(skip: Skip) -> i64 {
    code_in(&Skip::ALL, SKIP_BASE, &skip)
}

pub fn gap_kind(code: i64) -> Result<GapKind, IndexError> {
    value_in(&GapKind::ALL, GAP_BASE, code).ok_or_else(|| corrupt("gap code", code))
}

pub fn skip(code: i64) -> Result<Skip, IndexError> {
    value_in(&Skip::ALL, SKIP_BASE, code).ok_or_else(|| corrupt("skip code", code))
}

fn corrupt(what: &str, value: i64) -> IndexError {
    IndexError::Corrupt(format!("{what} {value}"))
}

pub fn gap_flags(gap: Gap) -> i64 {
    let errno = match gap {
        Gap::Failed(Errno(errno)) => i64::from(errno),
        _ => READ,
    };
    gap_code(gap.kind()) | errno << CODE_BITS
}

pub fn gap(flags: i64) -> Result<Gap, IndexError> {
    let errno = Errno(i32::try_from(flags >> CODE_BITS).map_err(|_| corrupt("errno", flags))?);
    Ok(match gap_kind(flags & CODE_MASK)? {
        GapKind::Denied => Gap::Denied,
        GapKind::Failed => Gap::Failed(errno),
        GapKind::Vanished => Gap::Vanished,
        GapKind::Cancelled => Gap::Cancelled,
    })
}

fn unread(flags: i64) -> Result<DirState, IndexError> {
    match skip(flags) {
        Ok(skip) => Ok(DirState::Skipped(skip)),
        Err(_) => Ok(DirState::Gap(gap(flags)?)),
    }
}

/// The columns `own_bytes`, `own_files` and `flags` for a state.
pub fn state_columns(state: &DirState) -> (Option<i64>, Option<i64>, i64) {
    match state {
        DirState::Read(own) => (Some(int(own.bytes.0)), Some(int(own.files)), READ),
        DirState::Gap(gap) => (None, None, gap_flags(*gap)),
        DirState::Skipped(skip) => (None, None, skip_code(*skip)),
    }
}

fn state(
    own_bytes: Option<i64>,
    own_files: Option<i64>,
    flags: i64,
) -> Result<DirState, IndexError> {
    match (own_bytes, own_files, flags) {
        (Some(bytes), Some(files), READ) => Ok(DirState::Read(OwnTotals {
            bytes: Bytes(count(bytes)),
            files: count(files),
        })),
        (None, None, flags) => unread(flags),
        _ => Err(corrupt("directory state", flags)),
    }
}

pub fn name_bytes(name: &OsString) -> &[u8] {
    name.as_bytes()
}

pub fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

pub fn path_from(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

pub const COLUMNS: &str = "id, parent_id, name, own_bytes, own_files, \
     tree_bytes, tree_files, tree_dirs, tree_mtime, flags";

/// The stored columns of a row, before they are interpreted.
pub struct Row {
    id: i64,
    parent: Option<i64>,
    name: Vec<u8>,
    flags: i64,
    own_bytes: Option<i64>,
    own_files: Option<i64>,
    tree: StoredTree,
}

struct StoredTree {
    bytes: i64,
    files: i64,
    dirs: i64,
    mtime: Option<i64>,
}

impl Row {
    /// Reads a row selected with [`COLUMNS`].
    pub fn from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        Ok(Row {
            id: row.get(0)?,
            parent: row.get(1)?,
            name: row.get(2)?,
            own_bytes: row.get(3)?,
            own_files: row.get(4)?,
            tree: StoredTree::from_sql(row)?,
            flags: row.get(9)?,
        })
    }

    pub fn record(self) -> Result<DirRecord, IndexError> {
        Ok(DirRecord {
            id: DirId(self.id),
            parent: self.parent.map(DirId),
            name: OsString::from_vec(self.name),
            state: state(self.own_bytes, self.own_files, self.flags)?,
            tree: TreeTotals {
                bytes: Bytes(count(self.tree.bytes)),
                files: count(self.tree.files),
                dirs: count(self.tree.dirs),
                newest: self.tree.mtime.map(UnixTime),
            },
        })
    }
}

impl StoredTree {
    fn from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredTree> {
        Ok(StoredTree {
            bytes: row.get(5)?,
            files: row.get(6)?,
            dirs: row.get(7)?,
            mtime: row.get(8)?,
        })
    }
}
