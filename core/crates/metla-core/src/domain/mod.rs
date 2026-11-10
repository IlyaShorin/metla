//! Domain types. No I/O and no dependencies on other modules.

mod candidate;
mod cleanup;
mod coverage;
mod dir;
mod entry;
mod kind;
mod rule;
mod scan;
mod totals;
mod verdict;
mod worktree;

pub use candidate::{Candidate, Exclusion, Recency, Tally};
pub use cleanup::{
    CleanupPreview, CleanupReport, Confirmation, Group, ItemReport, Leftovers, Outcome, PreviewId,
    PreviewItem, Rejected, Rejection, Separate, SkipReason, Warning,
};
pub use coverage::{Coverage, GapAt};
pub use dir::{DirId, DirRecord, DirState, Errno, Gap, GapKind, Skip};
pub use entry::{BLOCK_SIZE, Bytes, Entry, EntryKind, EntryMeta, UnixTime};
pub use kind::{FileKind, KindTotals};
pub use rule::{Category, Disposal, QuickCategory, RuleId};
pub use scan::{DirFacts, LiveDir, Probe, ProgressSnapshot, ScanSummary};
pub use totals::{OwnTotals, TreeTotals};
pub use verdict::Verdict;
pub use worktree::{Manager, Reason, Reasons, WorktreeStatus};
