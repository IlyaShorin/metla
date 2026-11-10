//! The SQLite index: one row per directory, plus the scan metadata.
//!
//! Only [`IndexWriter`] writes: its read-write connection is owned by one
//! writer thread and never handed out; scans and exclusion edits are
//! commands to that thread. [`IndexReader`]s open their own
//! read-only connections. A scan is written into a staging table and
//! swapped in by one transaction when it finishes, so a reader sees either
//! the previous picture or the new one, never a mix.

mod codec;
mod lineage;
mod reader;
mod schema;
mod update;
mod writer;

use std::fmt;
use std::path::PathBuf;

use crate::domain::{Coverage, KindTotals, UnixTime};

pub use reader::{IndexReader, Page, Snapshot};
pub use writer::{IndexEditor, IndexWriter, ScanWriter};

/// What the index says about scanning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRecord {
    /// When the latest scan started (it may still be running, or may have
    /// been interrupted).
    pub started: UnixTime,
    /// The picture the index holds, if any scan has finished.
    pub picture: Option<Picture>,
    latest: i64,
}

impl ScanRecord {
    /// The picture is from the latest scan: no scan started since has
    /// been left unfinished (or is still running).
    pub fn is_current(&self) -> bool {
        self.picture.as_ref().is_some_and(|p| p.scan == self.latest)
    }

    /// The latest scan finished and saw everything it wanted to see.
    pub fn is_complete(&self) -> bool {
        self.is_current()
            && self
                .picture
                .as_ref()
                .is_some_and(|p| p.coverage.is_complete())
    }
}

/// Which picture: directory ids are only meaningful within one, so edits
/// that name directories say which picture they were read from. A
/// picture brought up to date in place stays the same picture; ids of
/// directories that went are never given out again within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureId(i64);

/// The directory picture held by the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub root: PathBuf,
    /// The home directory the picture was classified for.
    pub home: Option<PathBuf>,
    pub started: UnixTime,
    pub finished: UnixTime,
    /// When changes were last applied in place, if any were since the
    /// scan.
    pub updated: Option<UnixTime>,
    pub coverage: Coverage,
    /// The scan's bytes by file kind.
    pub kinds: KindTotals,
    scan: i64,
}

impl Picture {
    pub fn id(&self) -> PictureId {
        PictureId(self.scan)
    }
}

#[derive(Debug)]
pub enum IndexError {
    /// The file is not an index of this version (`None`: not an index).
    UnknownSchema(Option<i64>),
    /// A stored value this version cannot interpret.
    Corrupt(String),
    Sqlite(rusqlite::Error),
    /// The writer thread stopped before the scan finished.
    WriterGone,
    /// The disk (or the index's limit) has no room for what was written.
    NoSpace,
}

impl From<rusqlite::Error> for IndexError {
    fn from(error: rusqlite::Error) -> IndexError {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DiskFull) => IndexError::NoSpace,
            _ => IndexError::Sqlite(error),
        }
    }
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::UnknownSchema(Some(v)) => write!(f, "unknown index schema version {v}"),
            IndexError::UnknownSchema(None) => write!(f, "not a Metla index"),
            IndexError::Corrupt(what) => write!(f, "corrupt index: {what}"),
            IndexError::Sqlite(error) => write!(f, "index: {error}"),
            IndexError::WriterGone => write!(f, "index writer stopped"),
            IndexError::NoSpace => write!(f, "not enough disk space for the index"),
        }
    }
}

impl std::error::Error for IndexError {}
