//! The SQLite index: one row per directory, plus the scan metadata.
//!
//! Only [`IndexWriter`] writes: its read-write connection is owned by one
//! writer thread and never handed out. [`IndexReader`]s open their own
//! read-only connections. A scan is written into a staging table and
//! swapped in by one transaction when it finishes, so a reader sees either
//! the previous picture or the new one, never a mix.

mod codec;
mod reader;
mod schema;
mod writer;

use std::fmt;
use std::path::PathBuf;

use crate::domain::{Coverage, UnixTime};

pub use reader::{IndexReader, Snapshot};
pub use writer::IndexWriter;

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
    /// The latest scan finished and saw everything it wanted to see.
    pub fn is_complete(&self) -> bool {
        self.picture
            .as_ref()
            .is_some_and(|p| p.scan == self.latest && p.coverage.is_complete())
    }
}

/// The directory picture held by the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub root: PathBuf,
    pub started: UnixTime,
    pub finished: UnixTime,
    pub coverage: Coverage,
    scan: i64,
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
}

impl From<rusqlite::Error> for IndexError {
    fn from(error: rusqlite::Error) -> IndexError {
        IndexError::Sqlite(error)
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
        }
    }
}

impl std::error::Error for IndexError {}
