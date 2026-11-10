//! How domain values are stored in columns.
//!
//! `flags` holds a directory's state: 0 in the low byte for a read
//! directory, else a gap or skip code. Above the low byte a failed gap
//! keeps its errno, and a read directory what it hands down: bit 8 in a
//! repository, bit 9 in a candidate, bits 10-11 its area. `markers` is
//! the marker names, each followed by a NUL byte; `kinds` the directory's
//! own bytes per file kind, a kind code byte and the bytes as an LEB128
//! number for every kind that has any. Both are NULL when empty.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use super::IndexError;
use crate::domain::{
    Area, Bytes, DirId, DirRecord, DirState, Errno, FileKind, Gap, GapKind, KindTotals, OwnTotals,
    Passed, RuleId, Skip, Stamp, TreeTotals, UnixTime, Verdict,
};

const READ: i64 = 0;
const CODE_BITS: u32 = 8;
const CODE_MASK: i64 = 0xff;
const IN_REPO: i64 = 1 << CODE_BITS;
const IN_CANDIDATE: i64 = 2 << CODE_BITS;
const AREA_SHIFT: u32 = CODE_BITS + 2;
const AREA_MASK: i64 = 0b11;
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

/// A device or inode number, stored with its bit pattern.
pub fn id_bits(value: u64) -> i64 {
    value.cast_signed()
}

pub fn id_from(bits: i64) -> u64 {
    bits.cast_unsigned()
}

/// File kinds are stored as 0, 1, ... in the order of `FileKind::ALL`.
pub fn kind_code(kind: FileKind) -> i64 {
    code_in(&FileKind::ALL, 0, &kind)
}

pub fn kind(code: i64) -> Result<FileKind, IndexError> {
    value_in(&FileKind::ALL, 0, code).ok_or_else(|| corrupt("kind code", code))
}

/// Verdicts are stored as 0, 1, ... in the order of `Verdict::ALL`.
pub fn verdict_code(verdict: Verdict) -> i64 {
    code_in(&Verdict::ALL, 0, &verdict)
}

pub fn verdict(code: Option<i64>) -> Result<Option<Verdict>, IndexError> {
    code.map(|code| value_in(&Verdict::ALL, 0, code).ok_or_else(|| corrupt("verdict", code)))
        .transpose()
}

pub fn rule(id: Option<String>) -> Result<Option<RuleId>, IndexError> {
    id.map(|id| RuleId::parse(&id).ok_or_else(|| IndexError::Corrupt(format!("rule id {id}"))))
        .transpose()
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

/// The state of a directory that was not read, from its `flags`.
pub fn unread(flags: i64) -> Result<DirState, IndexError> {
    match skip(flags) {
        Ok(skip) => Ok(DirState::Skipped(skip)),
        Err(_) => Ok(DirState::Gap(gap(flags)?)),
    }
}

/// What a read directory hands down, as `flags` bits above its state.
fn passed_flags(passed: &Passed) -> i64 {
    let area = code_in(&Area::ALL, 0, &passed.area);
    let repo = i64::from(passed.in_repo) * IN_REPO;
    let candidate = i64::from(passed.in_candidate) * IN_CANDIDATE;
    repo | candidate | area << AREA_SHIFT
}

fn passed(flags: i64, markers: Option<Vec<u8>>) -> Result<Passed, IndexError> {
    let area = (flags >> AREA_SHIFT) & AREA_MASK;
    Ok(Passed {
        markers: markers.map(|blob| names(&blob)).unwrap_or_default(),
        in_repo: flags & IN_REPO != 0,
        in_candidate: flags & IN_CANDIDATE != 0,
        area: value_in(&Area::ALL, 0, area).ok_or_else(|| corrupt("area", area))?,
    })
}

/// Names, each followed by a NUL byte; `None` for none.
pub fn names_blob(names: &[OsString]) -> Option<Vec<u8>> {
    let bytes = names.iter().flat_map(|n| n.as_bytes().iter().chain([&0]));
    let blob: Vec<u8> = bytes.copied().collect();
    (!blob.is_empty()).then_some(blob)
}

fn names(blob: &[u8]) -> Vec<OsString> {
    let parts = blob.split(|b| *b == 0).filter(|part| !part.is_empty());
    parts
        .map(|part| OsString::from_vec(part.to_vec()))
        .collect()
}

/// Every kind with bytes: its code and the bytes as LEB128; `None` for
/// none.
pub fn kinds_blob(kinds: &KindTotals) -> Option<Vec<u8>> {
    let mut blob = Vec::new();
    for (kind, bytes) in kinds.iter().filter(|(_, b)| b.0 > 0) {
        blob.push(kind as u8);
        let mut rest = bytes.0;
        while rest >= 0x80 {
            blob.push((rest & 0x7f) as u8 | 0x80);
            rest >>= 7;
        }
        blob.push(rest as u8);
    }
    (!blob.is_empty()).then_some(blob)
}

pub fn kinds_of(blob: Option<&[u8]>) -> Result<KindTotals, IndexError> {
    let mut kinds = KindTotals::default();
    let mut bytes = blob.unwrap_or_default().iter();
    while let Some(code) = bytes.next() {
        let kind = value_in(&FileKind::ALL, 0, i64::from(*code))
            .ok_or_else(|| corrupt("kind code", i64::from(*code)))?;
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = bytes
                .next()
                .ok_or_else(|| corrupt("kind bytes", i64::from(*code)))?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
        }
        kinds.add(kind, Bytes(value));
    }
    Ok(kinds)
}

/// The stored columns of a directory's state.
pub struct StateColumns {
    pub own_bytes: Option<i64>,
    pub own_files: Option<i64>,
    pub own_mtime: Option<i64>,
    pub flags: i64,
    pub kinds: Option<Vec<u8>>,
}

/// The columns for a directory's state and what it hands down.
pub fn state_columns(state: &DirState, passed: &Passed) -> StateColumns {
    let unread = |flags| StateColumns {
        own_bytes: None,
        own_files: None,
        own_mtime: None,
        flags,
        kinds: None,
    };
    match state {
        DirState::Read(own) => StateColumns {
            own_bytes: Some(int(own.bytes.0)),
            own_files: Some(int(own.files)),
            own_mtime: own.newest.map(|t| t.0),
            flags: READ | passed_flags(passed),
            kinds: kinds_blob(&own.kinds),
        },
        DirState::Gap(gap) => unread(gap_flags(*gap)),
        DirState::Skipped(skip) => unread(skip_code(*skip)),
    }
}

fn state(row: &Row) -> Result<DirState, IndexError> {
    match (row.own_bytes, row.own_files, row.flags & CODE_MASK) {
        (Some(bytes), Some(files), READ) => Ok(DirState::Read(OwnTotals {
            bytes: Bytes(count(bytes)),
            files: count(files),
            newest: row.own_mtime.map(UnixTime),
            kinds: kinds_of(row.kinds.as_deref())?,
        })),
        (None, None, _) => unread(row.flags),
        _ => Err(corrupt("directory state", row.flags)),
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

pub const COLUMNS: &str = "id, parent_id, name, own_bytes, own_files, own_mtime, \
     tree_bytes, tree_files, tree_dirs, tree_mtime, mtime, changed, flags, device, inode, \
     rule_id, markers, kinds";

/// [`COLUMNS`] and then the verdict, for candidate rows.
pub const CANDIDATE_COLUMNS: &str = "id, parent_id, name, own_bytes, own_files, own_mtime, \
     tree_bytes, tree_files, tree_dirs, tree_mtime, mtime, changed, flags, device, inode, \
     rule_id, markers, kinds, verdict";

/// A row as stored, before it is interpreted.
pub struct Row {
    id: i64,
    parent: Option<i64>,
    name: Vec<u8>,
    own_bytes: Option<i64>,
    own_files: Option<i64>,
    own_mtime: Option<i64>,
    tree_bytes: i64,
    tree_files: i64,
    tree_dirs: i64,
    tree_mtime: Option<i64>,
    mtime: i64,
    changed: i64,
    flags: i64,
    device: i64,
    inode: i64,
    rule_id: Option<String>,
    markers: Option<Vec<u8>>,
    kinds: Option<Vec<u8>>,
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
            own_mtime: row.get(5)?,
            tree_bytes: row.get(6)?,
            tree_files: row.get(7)?,
            tree_dirs: row.get(8)?,
            tree_mtime: row.get(9)?,
            mtime: row.get(10)?,
            changed: row.get(11)?,
            flags: row.get(12)?,
            device: row.get(13)?,
            inode: row.get(14)?,
            rule_id: row.get(15)?,
            markers: row.get(16)?,
            kinds: row.get(17)?,
        })
    }

    /// Reads a row selected with [`CANDIDATE_COLUMNS`] and its verdict.
    pub fn with_verdict(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Row, Option<i64>)> {
        Ok((Row::from_sql(row)?, row.get("verdict")?))
    }

    pub fn record(self) -> Result<DirRecord, IndexError> {
        let state = state(&self)?;
        let passed = match state {
            DirState::Read(_) => passed(self.flags, self.markers)?,
            _ => Passed::default(),
        };
        Ok(DirRecord {
            id: DirId(self.id),
            parent: self.parent.map(DirId),
            name: OsString::from_vec(self.name),
            device: id_from(self.device),
            inode: id_from(self.inode),
            mtime: UnixTime(self.mtime),
            changed: Stamp(self.changed),
            state,
            tree: TreeTotals {
                bytes: Bytes(count(self.tree_bytes)),
                files: count(self.tree_files),
                dirs: count(self.tree_dirs),
                newest: self.tree_mtime.map(UnixTime),
            },
            rule: rule(self.rule_id)?,
            passed,
        })
    }
}
