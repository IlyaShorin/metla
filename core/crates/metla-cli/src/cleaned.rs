//! What `clean` prints: the preview, and the report once confirmed. Pure:
//! data in, text out.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use metla_core::domain::{
    CleanupPreview, CleanupReport, Group, Manager, Outcome, Reason, Reasons, Rejection, Separate,
    SkipReason, Warning,
};

use crate::format::{bytes, count, signed_bytes};
use crate::report::name_in;

/// What `clean` did: the preview of `root`, and the report when it was
/// confirmed.
pub struct Cleaned {
    pub root: PathBuf,
    pub preview: CleanupPreview,
    pub report: Option<CleanupReport>,
}

const GROUP_NAMES: [(Group, &str); 4] = [
    (Group::Quick, "permanently"),
    (
        Group::Separate(Separate::MoveToTrash),
        "to Trash (its own confirmation)",
    ),
    (
        Group::Separate(Separate::EmptyTrash),
        "empty the Trash (its own confirmation)",
    ),
    (
        Group::Separate(Separate::RemoveWorktrees),
        "git worktree remove (its own confirmation)",
    ),
];

const REASON_NAMES: [(Reason, &str); 10] = [
    (Reason::Uncommitted, "uncommitted changes"),
    (Reason::Untracked, "untracked files"),
    (Reason::Unpushed, "unpushed commits"),
    (Reason::Locked, "locked"),
    (Reason::Prunable, "prunable"),
    (Reason::OperationInProgress, "an operation in progress"),
    (Reason::InUse, "in use"),
    (Reason::Managed(Manager::Codex), "managed by Codex"),
    (Reason::Managed(Manager::Claude), "managed by Claude"),
    (Reason::NotListed, "not a listed worktree"),
];

const SKIP_NAMES: [(SkipReason, &str); 13] = [
    (SkipReason::NotConfirmed, "not confirmed"),
    (SkipReason::Cancelled, "cancelled"),
    (SkipReason::Excluded, "excluded"),
    (SkipReason::OutsideRoot, "outside the root"),
    (SkipReason::Symlink, "now a symlink"),
    (SkipReason::NotADirectory, "no longer a directory"),
    (SkipReason::Replaced, "another directory now"),
    (SkipReason::MountPoint, "another volume now"),
    (SkipReason::AncestorChanged, "a folder above it changed"),
    (SkipReason::RuleMismatch, "its rule no longer matches"),
    (SkipReason::NotIgnored, "Git does not ignore it"),
    (SkipReason::HasTrackedFiles, "Git tracks files in it"),
    (SkipReason::Unverifiable, "Git could not check it"),
];

fn reason_text(reason: &Reason) -> String {
    match reason {
        Reason::InspectionFailed(why) => format!("git failed: {why}"),
        other => name_in(&REASON_NAMES, other.clone()).to_owned(),
    }
}

fn reasons_text(reasons: &Reasons) -> String {
    let named: Vec<String> = reasons.iter().map(reason_text).collect();
    named.join(", ")
}

fn skip_text(reason: &SkipReason) -> String {
    match reason {
        SkipReason::Blocked(reasons) => format!("blocked: {}", reasons_text(reasons)),
        SkipReason::Unreadable(errno) => format!("unreadable (errno {})", errno.0),
        other => name_in(&SKIP_NAMES, other.clone()).to_owned(),
    }
}

pub fn outcome_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Removed => "removed".into(),
        Outcome::Partial(left) => format!(
            "partly removed: {} entries and {} other volumes left{}",
            count(left.failures),
            count(left.mounts.len() as u64),
            if left.cancelled { ", cancelled" } else { "" }
        ),
        Outcome::Skipped(reason) => format!("skipped: {}", skip_text(reason)),
        Outcome::Failed(why) => format!("failed: {why}"),
        Outcome::AlreadyGone => "already gone".into(),
        Outcome::MovedToTrash => "moved to Trash".into(),
    }
}

fn warning_text(warning: &Warning) -> &'static str {
    match warning {
        Warning::RecentlyActive => "active recently",
        Warning::InsideManagedWorktree(_) => "in a managed worktree: confirm it by itself",
    }
}

fn relative<'p>(root: &Path, path: &'p Path) -> &'p Path {
    path.strip_prefix(root).unwrap_or(path)
}

pub fn clean_text(cleaned: &Cleaned) -> String {
    let mut out = String::new();
    preview_text(&mut out, cleaned);
    match &cleaned.report {
        Some(report) => report_text(&mut out, &cleaned.root, report),
        None => out.push_str("nothing removed: run again with --yes to confirm every group\n"),
    }
    out
}

fn preview_text(out: &mut String, cleaned: &Cleaned) {
    let preview = &cleaned.preview;
    let total = preview.items.iter().map(|i| i.estimated()).sum();
    let _ = writeln!(
        out,
        "preview {}: {} items, about {} (estimate)",
        preview.id.0,
        count(preview.items.len() as u64),
        bytes(total)
    );
    for (group, tally) in preview.totals() {
        let _ = writeln!(
            out,
            "  {}  {}",
            name_in(&GROUP_NAMES, group),
            bytes(tally.bytes)
        );
        for item in preview.items.iter().filter(|i| i.group() == group) {
            let warned: Vec<&str> = item.warnings.iter().map(warning_text).collect();
            let warned = match warned.is_empty() {
                true => String::new(),
                false => format!("  ({})", warned.join(", ")),
            };
            let path = relative(&cleaned.root, item.path()).display();
            let rule = item.rule().as_str();
            let _ = writeln!(
                out,
                "    {:>10}  {rule:<28}  {path}{warned}",
                bytes(item.estimated())
            );
        }
    }
    for rejected in &preview.rejected {
        let why = match &rejected.why {
            Rejection::Blocked(reasons) => format!("blocked: {}", reasons_text(reasons)),
            Rejection::Nested { inside } => format!("inside item {}", inside.0),
        };
        let path = relative(&cleaned.root, &rejected.candidate.path).display();
        let _ = writeln!(out, "  set aside  {path}  {why}");
    }
}

fn report_text(out: &mut String, root: &Path, report: &CleanupReport) {
    let _ = writeln!(
        out,
        "found {}, removed {}, freed on disk {} (measured)",
        bytes(report.found()),
        bytes(report.removed()),
        signed_bytes(report.freed())
    );
    let trashed = report
        .items
        .iter()
        .any(|i| i.outcome == Outcome::MovedToTrash);
    if trashed {
        let _ = writeln!(
            out,
            "moved to Trash {} (not freed until the Trash is emptied)",
            bytes(report.moved_to_trash())
        );
    }
    for item in &report.items {
        let path = relative(root, &item.path).display();
        let _ = writeln!(out, "  {path}: {}", outcome_text(&item.outcome));
    }
}
