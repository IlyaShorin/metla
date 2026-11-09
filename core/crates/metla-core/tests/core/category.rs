//! The decisions ARCHITECTURE.md §3 makes per category, and the small
//! domain values around candidates.

use std::path::{Path, PathBuf};
use std::time::Duration;

use metla_core::domain::{
    Bytes, Candidate, Category, DirId, Disposal, Exclusion, FileKind, KindTotals, QuickCategory,
    Recency, RuleId, Tally, UnixTime,
};

/// ARCHITECTURE.md §3, row by row: disposal and quick-clean category.
const TABLE: [(Category, &str, Disposal, Option<QuickCategory>); 6] = [
    (
        Category::AppCache,
        "app-cache",
        Disposal::DeletePermanently,
        Some(QuickCategory::AppCache),
    ),
    (
        Category::DevCache,
        "dev-cache",
        Disposal::DeletePermanently,
        Some(QuickCategory::DevCache),
    ),
    (
        Category::BuildArtifact,
        "build",
        Disposal::DeletePermanently,
        Some(QuickCategory::BuildArtifact),
    ),
    (Category::Log, "log", Disposal::MoveToTrash, None),
    (
        Category::GitWorktree,
        "worktree",
        Disposal::GitWorktreeRemove,
        None,
    ),
    (Category::Temp, "temp", Disposal::MoveToTrash, None),
];

#[test]
fn every_category_has_the_disposal_and_quick_category_of_the_architecture_table() {
    assert_eq!(Category::ALL.len(), TABLE.len());
    for (category, (expected, name, disposal, quick)) in Category::ALL.into_iter().zip(TABLE) {
        assert_eq!(category, expected);
        assert_eq!(category.name(), name);
        assert_eq!(category.disposal(), disposal, "{name}");
        assert_eq!(category.quick(), quick, "{name}");
    }
}

#[test]
fn logs_temporary_data_and_worktrees_can_never_be_quick_cleaned() {
    for category in [Category::Log, Category::Temp, Category::GitWorktree] {
        assert_eq!(category.quick(), None);
    }
    let quick: Vec<Category> = Category::ALL
        .into_iter()
        .filter(|c| c.quick().is_some())
        .collect();
    assert_eq!(
        quick,
        [
            Category::AppCache,
            Category::DevCache,
            Category::BuildArtifact
        ]
    );
}

#[test]
fn a_category_is_found_by_its_name() {
    for category in Category::ALL {
        assert_eq!(Category::from_name(category.name()), Some(category));
    }
    assert_eq!(Category::from_name("cache"), None);
    assert_eq!(Category::from_name(""), None);
}

#[test]
fn a_rule_id_is_its_category_name_a_dot_and_its_own_name() {
    let id = RuleId::new(Category::DevCache, "npm");
    assert_eq!(id.as_str(), "dev-cache.npm");
    assert_eq!(id.category(), Category::DevCache);
    assert_eq!(id.to_string(), "dev-cache.npm");
}

#[test]
fn a_stored_rule_id_is_parsed_back_with_its_category() {
    let id = RuleId::parse("build.cargo-target").unwrap();
    assert_eq!(id, RuleId::new(Category::BuildArtifact, "cargo-target"));
    assert_eq!(id.category(), Category::BuildArtifact);
    for bad in ["", "npm", "cache.npm", "dev-cache.", ".npm", "dev-cache"] {
        assert_eq!(RuleId::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn a_rule_is_removed_as_its_category_says_except_the_trash_which_is_emptied() {
    assert_eq!(
        RuleId::new(Category::Log, "user-library").disposal(),
        Disposal::MoveToTrash
    );
    assert_eq!(
        RuleId::new(Category::Temp, "other").disposal(),
        Disposal::MoveToTrash
    );
    assert_eq!(RuleId::trash().as_str(), "temp.trash");
    assert_eq!(RuleId::trash().disposal(), Disposal::DeletePermanently);
    assert_eq!(
        RuleId::new(Category::GitWorktree, "linked").disposal(),
        Disposal::GitWorktreeRemove
    );
}

const DAY: i64 = 24 * 3600;
const NOW: UnixTime = UnixTime(1_000 * DAY);

fn recency() -> Recency {
    Recency::new(NOW, Recency::DEFAULT_AGE)
}

#[test]
fn the_default_recent_activity_age_is_fourteen_days() {
    assert_eq!(Recency::DEFAULT_AGE, Duration::from_secs(14 * 24 * 3600));
}

#[test]
fn activity_within_the_age_is_recent_and_one_second_older_is_not() {
    let recency = recency();
    let edge = UnixTime(NOW.0 - 14 * DAY);
    assert!(recency.is_recent(Some(edge)));
    assert!(!recency.is_recent(Some(UnixTime(edge.0 - 1))));
    assert!(recency.is_recent(Some(NOW)));
    assert!(recency.is_recent(Some(UnixTime(NOW.0 + DAY))));
    assert!(recency.is_recent(None), "unknown activity counts as recent");
}

#[test]
fn recently_active_build_output_and_worktrees_are_not_preselected() {
    let recency = recency();
    let fresh = Some(UnixTime(NOW.0 - 14 * DAY));
    let stale = Some(UnixTime(NOW.0 - 14 * DAY - 1));
    assert!(!Category::BuildArtifact.preselected(fresh, &recency));
    assert!(Category::BuildArtifact.preselected(stale, &recency));
    assert!(!Category::BuildArtifact.preselected(None, &recency));
    assert!(!Category::GitWorktree.preselected(stale, &recency));
    for category in [
        Category::AppCache,
        Category::DevCache,
        Category::Log,
        Category::Temp,
    ] {
        assert!(category.preselected(fresh, &recency), "{category:?}");
    }
}

#[test]
fn a_candidate_takes_category_and_disposal_from_its_rule() {
    let candidate = Candidate {
        id: DirId(4),
        path: PathBuf::from("/r/.Trash"),
        rule: RuleId::trash(),
        bytes: Bytes(10),
        files: 2,
        activity: Some(NOW),
        device: 1,
        inode: 2,
        preselected: true,
    };
    assert_eq!(candidate.category(), Category::Temp);
    assert_eq!(candidate.disposal(), Disposal::DeletePermanently);
}

#[test]
fn tallies_add_up() {
    let mut tally = Tally::default();
    tally.add(Bytes(5));
    tally.add(Bytes(7));
    assert_eq!(
        tally,
        Tally {
            count: 2,
            bytes: Bytes(12)
        }
    );
    let mut more = Tally::default();
    more.merge(&tally);
    more.merge(&tally);
    assert_eq!(more.count, 4);
    assert_eq!(more.bytes, Bytes(24));
}

#[test]
fn every_file_kind_has_its_place_in_a_fixed_order() {
    assert_eq!(
        FileKind::ALL,
        [
            FileKind::Applications,
            FileKind::Developer,
            FileKind::Media,
            FileKind::Documents,
            FileKind::Archives,
            FileKind::System,
            FileKind::Other,
        ]
    );
}

#[test]
fn kind_totals_add_per_kind_and_in_total() {
    let mut totals = KindTotals::default();
    totals.add(FileKind::Media, Bytes(5));
    totals.add(FileKind::Media, Bytes(6));
    totals.add(FileKind::Other, Bytes(1));
    assert_eq!(totals.get(FileKind::Media), Bytes(11));
    assert_eq!(totals.get(FileKind::Other), Bytes(1));
    assert_eq!(totals.get(FileKind::System), Bytes(0));
    assert_eq!(totals.total(), Bytes(12));
    let mut sum = KindTotals::default();
    sum.merge(&totals);
    sum.merge(&totals);
    assert_eq!(sum.get(FileKind::Media), Bytes(22));
    let listed: Vec<(FileKind, Bytes)> = sum.iter().filter(|(_, b)| b.0 > 0).collect();
    assert_eq!(
        listed,
        [(FileKind::Media, Bytes(22)), (FileKind::Other, Bytes(2))]
    );
}

#[test]
fn an_exclusion_is_an_absolute_path_without_dots() {
    let excluded = Exclusion::new(Path::new("/a//b/./c/")).unwrap();
    assert_eq!(excluded.path(), Path::new("/a/b/c"));
    assert_eq!(
        Exclusion::new(Path::new("/")).unwrap().path(),
        Path::new("/")
    );
    assert_eq!(Exclusion::new(Path::new("a/b")), None);
    assert_eq!(Exclusion::new(Path::new("/a/../b")), None);
    assert_eq!(Exclusion::new(Path::new("")), None);
}

#[test]
fn an_exclusion_keeps_its_path_and_what_is_beneath_and_what_holds_it() {
    let excluded = Exclusion::new(Path::new("/a/b")).unwrap();
    assert!(excluded.hides(Path::new("/a/b")));
    assert!(excluded.hides(Path::new("/a/b/c")));
    assert!(excluded.hides(Path::new("/a")), "removing /a removes /a/b");
    assert!(excluded.hides(Path::new("/")));
    assert!(!excluded.hides(Path::new("/a/bc")));
    assert!(!excluded.hides(Path::new("/a/c")));
}
