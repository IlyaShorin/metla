//! The cleanup values of the domain: groups, confirmation, outcomes and
//! the report's three numbers.

use std::path::PathBuf;

use metla_core::domain::{
    ActionFailure, Bytes, Candidate, Category, CleanupReport, Confirmation, DirId, Errno, Group,
    ItemReport, Leftovers, Manager, Outcome, PreviewId, PreviewItem, RuleId, Separate, SkipReason,
};

fn item(rule: &str, managed: Option<Manager>) -> PreviewItem {
    let rule = RuleId::parse(rule).unwrap();
    PreviewItem {
        candidate: Candidate {
            id: DirId(7),
            path: PathBuf::from("/r/x"),
            rule,
            bytes: Bytes(100),
            files: 1,
            activity: None,
            device: 1,
            inode: 2,
            managed,
            preselected: false,
        },
        warnings: Vec::new(),
    }
}

#[test]
fn every_rule_falls_in_one_group_by_how_it_is_removed() {
    let cases = [
        ("app-cache.library", Group::Quick),
        ("dev-cache.npm", Group::Quick),
        ("build.cargo-target", Group::Quick),
        ("log.user-library", Group::Separate(Separate::MoveToTrash)),
        ("temp.other", Group::Separate(Separate::MoveToTrash)),
        ("temp.trash", Group::Separate(Separate::EmptyTrash)),
        (
            "worktree.linked",
            Group::Separate(Separate::RemoveWorktrees),
        ),
    ];
    for (rule, group) in cases {
        assert_eq!(Group::of(&RuleId::parse(rule).unwrap()), group, "{rule}");
        assert_eq!(item(rule, None).group(), group);
    }
}

#[test]
fn a_preview_item_shows_its_candidate() {
    let shown = item("log.user-library", None);
    assert_eq!(shown.path(), PathBuf::from("/r/x"));
    assert_eq!(shown.rule().as_str(), "log.user-library");
    assert_eq!(shown.category(), Category::Log);
    assert_eq!(shown.estimated(), Bytes(100));
    assert!(!shown.needs_own_confirmation());
    assert!(item("build.node-modules", Some(Manager::Claude)).needs_own_confirmation());
}

#[test]
fn a_confirmation_covers_quick_items_its_groups_and_its_own_items() {
    let none = Confirmation::default();
    assert!(none.covers(&item("dev-cache.npm", None)));
    assert!(!none.covers(&item("log.user-library", None)));
    let logs = Confirmation {
        groups: [Separate::MoveToTrash].into(),
        items: Default::default(),
    };
    assert!(logs.covers(&item("log.user-library", None)));
    assert!(!logs.covers(&item("temp.trash", None)));
    let managed = item("build.node-modules", Some(Manager::Codex));
    assert!(!none.covers(&managed));
    let one = Confirmation {
        items: [DirId(7)].into(),
        ..Confirmation::default()
    };
    assert!(one.covers(&managed));
    let other = Confirmation {
        items: [DirId(8)].into(),
        ..Confirmation::default()
    };
    assert!(!other.covers(&managed));
}

#[test]
fn only_a_skipped_item_is_known_to_be_unchanged() {
    let changed = [
        Outcome::Removed,
        Outcome::Partial(Leftovers::default()),
        Outcome::Failed(ActionFailure::Unlisted),
        Outcome::AlreadyGone,
        Outcome::MovedToTrash,
    ];
    assert!(changed.iter().all(Outcome::changed));
    assert!(!Outcome::Skipped(SkipReason::Excluded).changed());
}

#[test]
fn leftovers_count_every_failure_and_keep_the_first() {
    let mut left = Leftovers::default();
    assert!(left.is_empty());
    for n in 0..20 {
        left.fail(PathBuf::from(format!("f{n}")), Errno(13));
    }
    assert_eq!(left.failures, 20);
    assert_eq!(left.first.len(), Leftovers::FIRST);
    assert_eq!(left.first[0], (PathBuf::from("f0"), Errno(13)));
    assert!(!left.is_empty());
    let mounted = Leftovers {
        mounts: vec![PathBuf::from("vol")],
        ..Leftovers::default()
    };
    assert!(!mounted.is_empty());
    let cancelled = Leftovers {
        cancelled: true,
        ..Leftovers::default()
    };
    assert!(!cancelled.is_empty());
}

fn report(items: &[(u64, u64, Outcome)]) -> CleanupReport {
    let items = items.iter().map(|(found, removed, outcome)| ItemReport {
        candidate: DirId(1),
        path: PathBuf::from("/r/x"),
        rule: RuleId::trash(),
        found: Bytes(*found),
        removed: Bytes(*removed),
        outcome: outcome.clone(),
    });
    CleanupReport {
        preview: PreviewId(1),
        items: items.collect(),
        free_before: Bytes(1_000),
        free_after: Bytes(1_600),
    }
}

#[test]
fn a_report_states_found_removed_trashed_and_freed_separately() {
    let report = report(&[
        (500, 400, Outcome::Partial(Leftovers::default())),
        (300, 0, Outcome::MovedToTrash),
        (200, 0, Outcome::Skipped(SkipReason::NotConfirmed)),
    ]);
    assert_eq!(report.found(), Bytes(1_000));
    assert_eq!(report.removed(), Bytes(400));
    assert_eq!(report.moved_to_trash(), Bytes(300));
    assert_eq!(report.freed(), 600);
    let shrunk = CleanupReport {
        free_after: Bytes(900),
        ..report
    };
    assert_eq!(shrunk.freed(), -100, "something else used space meanwhile");
}
