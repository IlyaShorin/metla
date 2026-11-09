use std::collections::BTreeMap;
use std::path::PathBuf;

use super::dir::{Gap, GapKind, Skip};

/// A gap and where it is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GapAt {
    /// Depth below the root (the root is 0). Shallow gaps hide more.
    pub depth: u32,
    pub path: PathBuf,
    pub gap: Gap,
}

/// What a scan could not see: gaps and skips by kind, and a bounded list of
/// the shallowest gaps. A scan is complete when it has no gap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    gaps: BTreeMap<GapKind, u64>,
    skips: BTreeMap<Skip, u64>,
    top: Vec<GapAt>,
    limit: usize,
}

impl Coverage {
    /// Empty coverage that keeps at most `limit` gaps in its list.
    pub fn new(limit: usize) -> Coverage {
        Coverage::restore(limit, Vec::new(), Vec::new(), Vec::new())
    }

    /// Coverage rebuilt from stored counts and gaps.
    pub fn restore(
        limit: usize,
        gaps: Vec<(GapKind, u64)>,
        skips: Vec<(Skip, u64)>,
        top: Vec<GapAt>,
    ) -> Coverage {
        Coverage {
            gaps: gaps.into_iter().collect(),
            skips: skips.into_iter().collect(),
            top,
            limit,
        }
    }

    pub fn record_gap(&mut self, at: GapAt) {
        *self.gaps.entry(at.gap.kind()).or_default() += 1;
        self.top.push(at);
        self.top.sort();
        self.top.truncate(self.limit);
    }

    pub fn record_skip(&mut self, skip: Skip) {
        *self.skips.entry(skip).or_default() += 1;
    }

    pub fn gaps(&self, kind: GapKind) -> u64 {
        self.gaps.get(&kind).copied().unwrap_or_default()
    }

    pub fn skips(&self, skip: Skip) -> u64 {
        self.skips.get(&skip).copied().unwrap_or_default()
    }

    pub fn gap_total(&self) -> u64 {
        self.gaps.values().sum()
    }

    pub fn skip_total(&self) -> u64 {
        self.skips.values().sum()
    }

    pub fn is_complete(&self) -> bool {
        self.gap_total() == 0
    }

    /// The shallowest gaps, ordered by depth and then path.
    pub fn top_gaps(&self) -> &[GapAt] {
        &self.top
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn gap_counts(&self) -> impl Iterator<Item = (GapKind, u64)> + '_ {
        self.gaps.iter().map(|(kind, count)| (*kind, *count))
    }

    pub fn skip_counts(&self) -> impl Iterator<Item = (Skip, u64)> + '_ {
        self.skips.iter().map(|(skip, count)| (*skip, *count))
    }
}
