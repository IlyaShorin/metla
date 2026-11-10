//! Building a preview from selected candidates: worktrees that cannot be
//! removed and items nested inside other items are set aside with the
//! reason; the rest carry their warnings.

use std::collections::BTreeSet;

use crate::domain::{
    Candidate, Category, CleanupPreview, DirId, Disposal, PreviewId, PreviewItem, Recency,
    Rejected, Rejection, Warning, WorktreeStatus, holders,
};

/// The preview `id` of `candidates`; `inspect` gives a worktree's status.
pub fn build(
    id: PreviewId,
    candidates: Vec<Candidate>,
    inspect: impl Fn(&Candidate) -> WorktreeStatus,
    recency: &Recency,
) -> CleanupPreview {
    let mut seen = BTreeSet::new();
    let mut includable = Vec::new();
    let mut rejected = Vec::new();
    for candidate in candidates.into_iter().filter(|c| seen.insert(c.id)) {
        match blocked(&candidate, &inspect) {
            Some(why) => rejected.push(Rejected { candidate, why }),
            None => includable.push(candidate),
        }
    }
    let (outermost, nested) = outermost(includable);
    rejected.extend(nested);
    let items = outermost.into_iter().map(|candidate| PreviewItem {
        warnings: warnings(&candidate, recency),
        candidate,
    });
    CleanupPreview {
        id,
        items: items.collect(),
        rejected,
    }
}

/// Why a worktree cannot be included, if it is one that cannot.
fn blocked(
    candidate: &Candidate,
    inspect: impl Fn(&Candidate) -> WorktreeStatus,
) -> Option<Rejection> {
    if candidate.disposal() != Disposal::GitWorktreeRemove {
        return None;
    }
    match inspect(candidate) {
        WorktreeStatus::Removable { .. } => None,
        WorktreeStatus::Blocked { reasons, .. } => Some(Rejection::Blocked(reasons)),
    }
}

/// The candidates no other candidate holds, largest first, and the
/// others with the one that holds them.
fn outermost(mut candidates: Vec<Candidate>) -> (Vec<Candidate>, Vec<Rejected>) {
    let held = holders(&mut candidates, |c| &c.path);
    let ids: Vec<DirId> = candidates.iter().map(|c| c.id).collect();
    let mut kept: Vec<Candidate> = Vec::new();
    let mut nested = Vec::new();
    for (candidate, holder) in candidates.into_iter().zip(held) {
        match holder.map(|at| ids[at]) {
            Some(inside) => nested.push(Rejected {
                why: Rejection::Nested { inside },
                candidate,
            }),
            None => kept.push(candidate),
        }
    }
    kept.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    (kept, nested)
}

fn warnings(candidate: &Candidate, recency: &Recency) -> Vec<Warning> {
    let build = candidate.category() == Category::BuildArtifact;
    let recent =
        (build && recency.is_recent(candidate.activity)).then_some(Warning::RecentlyActive);
    let managed = candidate.managed.map(Warning::InsideManagedWorktree);
    recent.into_iter().chain(managed).collect()
}
