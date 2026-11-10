//! The last cleanup's report as the engine keeps it in the index: every
//! outcome, reason and path comes back exactly as it was written, and
//! what is not a report is not read as one.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use metla_core::domain::{
    Bytes, CleanupReport, DirId, Errno, ItemReport, Leftovers, Manager, Outcome, PreviewId, Reason,
    RuleId, SkipReason, WorktreeStatus,
};
use metla_core::engine::{decode_report, encode_report};

const SIMPLE_SKIPS: [SkipReason; 13] = [
    SkipReason::NotConfirmed,
    SkipReason::Cancelled,
    SkipReason::Excluded,
    SkipReason::OutsideRoot,
    SkipReason::Symlink,
    SkipReason::NotADirectory,
    SkipReason::Replaced,
    SkipReason::MountPoint,
    SkipReason::AncestorChanged,
    SkipReason::RuleMismatch,
    SkipReason::NotIgnored,
    SkipReason::HasTrackedFiles,
    SkipReason::Unverifiable,
];

fn item(id: i64, path: PathBuf, outcome: Outcome) -> ItemReport {
    ItemReport {
        candidate: DirId(id),
        path,
        rule: RuleId::parse("build.cargo-target").unwrap(),
        found: Bytes(1000 + id.unsigned_abs()),
        removed: Bytes(id.unsigned_abs()),
        outcome,
    }
}

/// Every reason a worktree can be blocked for.
fn every_reason() -> SkipReason {
    let reasons = BTreeSet::from([
        Reason::Uncommitted,
        Reason::Untracked,
        Reason::Unpushed,
        Reason::Locked,
        Reason::Prunable,
        Reason::OperationInProgress,
        Reason::InUse,
        Reason::Managed(Manager::Codex),
        Reason::Managed(Manager::Claude),
        Reason::NotListed,
        Reason::InspectionFailed("git: bad\tthing\nhappened \\ here".into()),
    ]);
    let WorktreeStatus::Blocked { reasons, .. } = WorktreeStatus::new(None, reasons) else {
        unreachable!("reasons block");
    };
    SkipReason::Blocked(reasons)
}

/// A path no string can hold: a tab, a newline, a backslash and a byte
/// that is not UTF-8.
fn odd_path() -> PathBuf {
    PathBuf::from(OsStr::from_bytes(b"/r/a\tb\nc\\d/\xff\xfe"))
}

fn every_outcome() -> Vec<Outcome> {
    let mut outcomes = vec![
        Outcome::Removed,
        Outcome::AlreadyGone,
        Outcome::MovedToTrash,
        Outcome::Failed("refused:\tno\nway \\".into()),
        Outcome::Skipped(every_reason()),
        Outcome::Skipped(SkipReason::Unreadable(Errno(13))),
        Outcome::Partial(Leftovers {
            failures: 20,
            first: vec![(odd_path(), Errno(1)), (PathBuf::from("/r/x"), Errno(66))],
            mounts: vec![PathBuf::from("/r/mnt"), odd_path()],
            cancelled: true,
        }),
        Outcome::Partial(Leftovers::default()),
    ];
    outcomes.extend(SIMPLE_SKIPS.map(Outcome::Skipped));
    outcomes
}

#[test]
fn every_outcome_and_reason_comes_back_as_it_was() {
    let items = every_outcome().into_iter().enumerate();
    let report = CleanupReport {
        preview: PreviewId(u64::MAX),
        items: items
            .map(|(at, outcome)| item(at as i64 - 3, odd_path().join(at.to_string()), outcome))
            .collect(),
        free_before: Bytes(u64::MAX),
        free_after: Bytes(0),
    };
    assert_eq!(decode_report(&encode_report(&report)), Some(report));
}

#[test]
fn a_report_without_items_comes_back() {
    let report = CleanupReport {
        preview: PreviewId(1),
        items: Vec::new(),
        free_before: Bytes(10),
        free_after: Bytes(12),
    };
    assert_eq!(decode_report(&encode_report(&report)), Some(report));
}

#[test]
fn what_is_not_a_report_is_not_read_as_one() {
    let report = CleanupReport {
        preview: PreviewId(2),
        items: vec![item(5, PathBuf::from("/r/t"), Outcome::Removed)],
        free_before: Bytes(1),
        free_after: Bytes(2),
    };
    let good = encode_report(&report);
    let text = String::from_utf8(good.clone()).unwrap();
    let broken = [
        String::new(),
        "x\t1\t2".to_owned(),
        "1\t2".to_owned(),
        "1\t2\t3\t4".to_owned(),
        text.replace("removed", "gone away"),
        text.replace("build.cargo-target", "nothing"),
        text.replace("\t5\t", "\tfive\t"),
        format!("{text}\textra"),
        format!("{text}\n"),
        text.replace("\t/r/t", "\t/r/t\\"),
        text.replace("\t/r/t", "\t/r/\\qt"),
    ];
    for bytes in broken {
        assert_eq!(decode_report(bytes.as_bytes()), None, "{bytes:?}");
    }
    let cut = |outcome: &str| {
        let line = format!("1\t2\t3\n4\t/p\tbuild.cargo-target\t5\t6\t{outcome}");
        decode_report(line.as_bytes())
    };
    for outcome in [
        "failed",
        "skipped",
        "skipped\tskip",
        "skipped\tskip\t13",
        "skipped\tskip\t0\t1",
        "skipped\tunreadable",
        "skipped\tunreadable\t1\t2",
        "skipped\tblocked",
        "skipped\tblocked\t11",
        "skipped\tnone",
        "partial\t1\t0",
        "partial\t1\t2\t0",
        "partial\t1\t0\t1",
        "partial\t1\t0\t1\t/p",
        "partial\t1\t0\t1\t/p\tx",
        "removed\textra",
    ] {
        assert_eq!(cut(outcome), None, "{outcome}");
    }
    assert!(cut("skipped\tblocked\t0\t!git failed").is_some());
    assert!(cut("partial\t1\t1\t0").is_some());
}
