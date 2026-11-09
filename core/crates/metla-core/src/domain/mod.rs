//! Domain types. No I/O and no dependencies on other modules.

mod coverage;
mod dir;
mod entry;
mod scan;
mod totals;

pub use coverage::{Coverage, GapAt};
pub use dir::{DirId, DirRecord, DirState, Errno, Gap, GapKind, Skip};
pub use entry::{BLOCK_SIZE, Bytes, Entry, EntryKind, EntryMeta, UnixTime};
pub use scan::{DirFacts, LiveDir, ProgressSnapshot, ScanSummary};
pub use totals::{OwnTotals, TreeTotals};
