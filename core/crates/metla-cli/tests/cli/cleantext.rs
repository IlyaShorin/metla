//! Rendering of `clean`: the preview and the report, on synthetic data.

use std::collections::BTreeSet;
use std::path::PathBuf;

use metla_cli::{Cleaned, clean_text, outcome_text};
use metla_core::domain::{
    Bytes, Candidate, CleanupPreview, CleanupReport, DirId, Errno, ItemReport, Leftovers, Manager,
    Outcome, PreviewId, PreviewItem, Reason, Rejected, Rejection, RuleId, SkipReason, Warning,
    WorktreeStatus,
};

fn candidate(id: i64, path: &str, rule: &str, bytes: u64) -> Candidate {
    Candidate {
        id: DirId(id),
        path: PathBuf::from("/f").join(path),
        rule: RuleId::parse(rule).unwrap(),
        bytes: Bytes(bytes),
        files: 1,
        activity: None,
        device: 1,
        inode: 2,
        managed: None,
        preselected: true,
    }
}

fn item(candidate: Candidate, warnings: &[Warning]) -> PreviewItem {
    PreviewItem {
        candidate,
        warnings: warnings.to_vec(),
    }
}

fn blocked(reasons: &[Reason]) -> Rejection {
    let set: BTreeSet<Reason> = reasons.iter().cloned().collect();
    match WorktreeStatus::new(None, set) {
        WorktreeStatus::Blocked(reasons) => Rejection::Blocked(reasons),
        WorktreeStatus::Removable { .. } => unreachable!(),
    }
}

fn preview() -> CleanupPreview {
    CleanupPreview {
        id: PreviewId(3),
        items: vec![
            item(
                candidate(1, "repo/target", "build.cargo-target", 2_000_000),
                &[Warning::RecentlyActive],
            ),
            item(
                candidate(2, "wt/node_modules", "build.node-modules", 30_000),
                &[Warning::InsideManagedWorktree(Manager::Codex)],
            ),
            item(
                candidate(3, "home/Library/Logs/x", "log.user-library", 40_000),
                &[],
            ),
            item(candidate(4, "home/.Trash", "temp.trash", 5_000), &[]),
            item(candidate(5, "clean-wt", "worktree.linked", 70_000), &[]),
        ],
        rejected: vec![
            Rejected {
                candidate: candidate(6, "wt", "worktree.linked", 9),
                why: blocked(&[
                    Reason::Managed(Manager::Codex),
                    Reason::InspectionFailed("no git".into()),
                ]),
            },
            Rejected {
                candidate: candidate(7, "clean-wt/target", "build.cargo-target", 9),
                why: Rejection::Nested { inside: DirId(5) },
            },
        ],
    }
}

#[test]
fn a_preview_reads_by_group_with_warnings_and_what_was_set_aside() {
    let cleaned = Cleaned {
        root: PathBuf::from("/f"),
        preview: preview(),
        report: None,
    };
    let expected = "\
preview 3: 5 items, about 2.1 MB (estimate)
  permanently  2.0 MB
        2.0 MB  build.cargo-target            repo/target  (active recently)
       30.0 kB  build.node-modules            wt/node_modules  (in a managed worktree: confirm it by itself)
  to Trash (its own confirmation)  40.0 kB
       40.0 kB  log.user-library              home/Library/Logs/x
  empty the Trash (its own confirmation)  5.0 kB
        5.0 kB  temp.trash                    home/.Trash
  git worktree remove (its own confirmation)  70.0 kB
       70.0 kB  worktree.linked               clean-wt
  set aside  wt  blocked: managed by Codex, git failed: no git
  set aside  clean-wt/target  inside item 5
nothing removed: run again with --yes to confirm every group
";
    assert_eq!(clean_text(&cleaned), expected);
}

#[test]
fn a_report_keeps_found_removed_and_freed_apart() {
    let report = CleanupReport {
        preview: PreviewId(3),
        items: vec![
            ItemReport {
                candidate: DirId(1),
                path: PathBuf::from("/f/repo/target"),
                rule: RuleId::parse("build.cargo-target").unwrap(),
                found: Bytes(2_000_000),
                removed: Bytes(1_500_000),
                outcome: Outcome::Removed,
            },
            ItemReport {
                candidate: DirId(3),
                path: PathBuf::from("/f/home/Library/Logs/x"),
                rule: RuleId::parse("log.user-library").unwrap(),
                found: Bytes(40_000),
                removed: Bytes(0),
                outcome: Outcome::MovedToTrash,
            },
        ],
        free_before: Bytes(10_000_000),
        free_after: Bytes(9_000_000),
    };
    let cleaned = Cleaned {
        root: PathBuf::from("/f"),
        preview: CleanupPreview {
            id: PreviewId(3),
            items: Vec::new(),
            rejected: Vec::new(),
        },
        report: Some(report),
    };
    let expected = "\
preview 3: 0 items, about 0 B (estimate)
found 2.0 MB, removed 1.5 MB, freed on disk -1.0 MB (measured)
moved to Trash 40.0 kB (not freed until the Trash is emptied)
  repo/target: removed
  home/Library/Logs/x: moved to Trash
";
    assert_eq!(clean_text(&cleaned), expected);
}

#[test]
fn a_report_without_anything_trashed_does_not_mention_the_trash() {
    let report = CleanupReport {
        preview: PreviewId(1),
        items: vec![ItemReport {
            candidate: DirId(1),
            path: PathBuf::from("/f/repo/target"),
            rule: RuleId::parse("build.cargo-target").unwrap(),
            found: Bytes(1_000),
            removed: Bytes(1_000),
            outcome: Outcome::Removed,
        }],
        free_before: Bytes(0),
        free_after: Bytes(1_000),
    };
    let cleaned = Cleaned {
        root: PathBuf::from("/f"),
        preview: CleanupPreview {
            id: PreviewId(1),
            items: Vec::new(),
            rejected: Vec::new(),
        },
        report: Some(report),
    };
    let text = clean_text(&cleaned);
    assert!(text.contains("found 1.0 kB, removed 1.0 kB, freed on disk 1.0 kB (measured)\n"));
    assert!(!text.contains("Trash"), "{text}");
}

#[test]
fn every_outcome_says_what_happened() {
    let left = Leftovers {
        failures: 2,
        first: vec![(PathBuf::from("a"), Errno(13))],
        mounts: vec![PathBuf::from("vol")],
        cancelled: true,
    };
    let reasons = match blocked(&[Reason::Uncommitted, Reason::Unpushed]) {
        Rejection::Blocked(reasons) => reasons,
        Rejection::Nested { .. } => unreachable!(),
    };
    let cases = [
        (Outcome::Removed, "removed"),
        (
            Outcome::Partial(left),
            "partly removed: 2 entries and 1 other volumes left, cancelled",
        ),
        (
            Outcome::Partial(Leftovers::default()),
            "partly removed: 0 entries and 0 other volumes left",
        ),
        (Outcome::Failed("git said no".into()), "failed: git said no"),
        (Outcome::AlreadyGone, "already gone"),
        (Outcome::MovedToTrash, "moved to Trash"),
        (
            Outcome::Skipped(SkipReason::Blocked(reasons)),
            "skipped: blocked: uncommitted changes, unpushed commits",
        ),
        (
            Outcome::Skipped(SkipReason::Unreadable(Errno(13))),
            "skipped: unreadable (errno 13)",
        ),
    ];
    for (outcome, said) in cases {
        assert_eq!(outcome_text(&outcome), said);
    }
    let skips = [
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
    for (reason, said) in skips {
        assert_eq!(
            outcome_text(&Outcome::Skipped(reason)),
            format!("skipped: {said}")
        );
    }
}

#[test]
fn every_worktree_reason_has_words() {
    let cases = [
        (Reason::Untracked, "untracked files"),
        (Reason::Locked, "locked"),
        (Reason::Prunable, "prunable"),
        (Reason::OperationInProgress, "an operation in progress"),
        (Reason::InUse, "in use"),
        (Reason::Managed(Manager::Claude), "managed by Claude"),
        (Reason::NotListed, "not a listed worktree"),
    ];
    for (reason, said) in cases {
        let Rejection::Blocked(reasons) = blocked(&[reason]) else {
            unreachable!()
        };
        let outcome = Outcome::Skipped(SkipReason::Blocked(reasons));
        assert_eq!(outcome_text(&outcome), format!("skipped: blocked: {said}"));
    }
}
