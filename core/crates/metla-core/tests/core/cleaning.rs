//! Cleaning a synthetic fixture: previews, typed confirmation, disposals,
//! the fixture guard, executing once, idempotence and cancellation.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use metla_core::clean::{CleanControl, CleanError, Cleaner, Context, FIXTURE_MARKER, TrashHost};
use metla_core::domain::{
    ActionFailure, Bytes, Confirmation, DirId, Group, Outcome, PreviewId, QuickCategory, Rejection,
    Separate, SkipReason, Tally, Warning,
};

use crate::repo::{next_year, now, tree_state};
use crate::world::{FakeTrash, World, all_groups, outcomes};

/// Caches, logs, the Trash and a repository's build output.
const TREE: &[&str] = &[
    "~/Library/Caches/com.example.app/blob=cached",
    "~/Library/Caches/com.example.app/sub/more=cached",
    "~/.npm/_cacache/index/x=npm",
    "~/Library/Logs/com.example.app/today.log=log",
    "~/Library/Logs/DiagnosticReports/crash.ips=report",
    "~/.Trash/old.zip=old",
    "~/.Trash/folder/x=old",
    "code/app/Cargo.toml",
    "code/app/src/main.rs=fn main() {}",
    "code/app/target/debug/app=binary",
    "code/app/web/package.json",
    "code/app/web/node_modules/x/index.js=js",
];

fn world() -> World {
    let world = World::new(TREE);
    world.repo("code/app", "target/\nnode_modules/\n");
    world
}

const CACHE: &str = "home/Library/Caches/com.example.app";
const NPM: &str = "home/.npm/_cacache";
const LOGS: &str = "home/Library/Logs/com.example.app";
const REPORTS: &str = "home/Library/Logs/DiagnosticReports";
const TRASH: &str = "home/.Trash";
const TARGET: &str = "code/app/target";
const MODULES: &str = "code/app/web/node_modules";
const EVERYTHING: [&str; 7] = [CACHE, NPM, LOGS, REPORTS, TRASH, TARGET, MODULES];

fn entries(world: &World, relative: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(world.path(relative))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_root_without_the_fixture_marker_is_refused_before_anything_is_touched() {
    let world = World::unmarked(TREE);
    let before = tree_state(world.fx.root());
    let rules = Arc::new(world.rules());
    let refused = Cleaner::new(
        world.fx.root(),
        None,
        rules.clone(),
        world.git.clone(),
        world.settings(),
    );
    assert!(matches!(refused, Err(CleanError::NotAFixture(ref root)) if root == world.fx.root()));
    fs::create_dir(world.fx.path(FIXTURE_MARKER)).unwrap();
    let directory = Cleaner::new(
        world.fx.root(),
        None,
        rules,
        world.git.clone(),
        world.settings(),
    );
    assert!(
        matches!(directory, Err(CleanError::NotAFixture(_))),
        "the marker must be a file"
    );
    fs::remove_dir(world.fx.path(FIXTURE_MARKER)).unwrap();
    assert_eq!(tree_state(world.fx.root()), before);
    let said = CleanError::NotAFixture(world.fx.root().to_path_buf()).to_string();
    assert!(said.ends_with("has no .metla-fixture: development builds clean only fixtures"));
}

#[test]
fn a_fixture_whose_marker_went_away_is_not_cleaned() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET]), next_year());
    fs::remove_file(world.fx.path(FIXTURE_MARKER)).unwrap();
    let before = tree_state(world.fx.root());
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let refused = cleaner.execute(
        preview.id,
        &Confirmation::default(),
        &context,
        &CleanControl::new(),
        &world.trash,
    );
    assert!(matches!(refused, Err(CleanError::NotAFixture(_))));
    assert_eq!(tree_state(world.fx.root()), before);
}

#[test]
fn a_root_replaced_after_the_cleaner_was_made_is_refused() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET]), next_year());
    let moved = world.fx.root().with_extension("moved");
    fs::rename(world.fx.root(), &moved).unwrap();
    fs::create_dir(world.fx.root()).unwrap();
    fs::write(world.fx.path(FIXTURE_MARKER), "").unwrap();
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let refused = cleaner.execute(
        preview.id,
        &Confirmation::default(),
        &context,
        &CleanControl::new(),
        &world.trash,
    );
    fs::remove_dir_all(world.fx.root()).unwrap();
    fs::rename(&moved, world.fx.root()).unwrap();
    assert_eq!(
        refused.unwrap_err(),
        CleanError::Root("it is another directory now".into())
    );
    assert!(world.path(TARGET).join("debug/app").exists());
}

#[test]
fn a_preview_groups_items_by_how_they_are_removed() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &EVERYTHING), next_year());
    assert_eq!(preview.id, PreviewId(1));
    assert!(preview.rejected.is_empty());
    let groups: BTreeMap<Group, u64> = preview
        .totals()
        .into_iter()
        .map(|(group, tally)| (group, tally.count))
        .collect();
    assert_eq!(
        groups,
        BTreeMap::from([
            (Group::Quick, 4),
            (Group::Separate(Separate::MoveToTrash), 2),
            (Group::Separate(Separate::EmptyTrash), 1),
        ])
    );
    let total: u64 = preview.items.iter().map(|i| i.estimated().0).sum();
    let tallied: u64 = preview.totals().values().map(|t: &Tally| t.bytes.0).sum();
    assert_eq!(total, tallied);
    let sizes: Vec<Bytes> = preview.items.iter().map(|i| i.estimated()).collect();
    let mut sorted = sizes.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(sizes, sorted, "largest first");
    let second = cleaner.preview(Vec::new(), next_year());
    assert_eq!(second.id, PreviewId(2));
}

#[test]
fn only_the_latest_preview_can_be_executed() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let older = cleaner.preview(world.picked(&found, &[TARGET]), next_year());
    let latest = cleaner.preview(world.picked(&found, &[TARGET]), next_year());
    assert_ne!(older.id, latest.id);
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let control = CleanControl::new();
    let none = Confirmation::default();
    let stale = cleaner.execute(older.id, &none, &context, &control, &world.trash);
    assert_eq!(stale.unwrap_err(), CleanError::UnknownPreview(older.id));
    assert!(world.path(TARGET).join("debug/app").exists());
    let report = world.execute(&mut cleaner, &latest, &none, &[]);
    assert_eq!(report.items[0].outcome, Outcome::Removed);
    let next = cleaner.preview(Vec::new(), next_year());
    assert_eq!(next.id, PreviewId(latest.id.0 + 1));
    let used = cleaner.execute(latest.id, &none, &context, &control, &world.trash);
    assert_eq!(used.unwrap_err(), CleanError::UnknownPreview(latest.id));
}

#[test]
fn a_preview_lists_each_candidate_once() {
    let world = world();
    let found = world.scan();
    let target = world.picked(&found, &[TARGET]);
    let twice = [target.clone(), target].concat();
    let preview = world.cleaner().preview(twice, next_year());
    assert_eq!(preview.items.len(), 1);
}

#[test]
fn recent_build_output_is_flagged_in_the_preview() {
    let world = world();
    let found = world.scan();
    let picked = world.picked(&found, &[TARGET, CACHE]);
    let preview = world.cleaner().preview(picked.clone(), now());
    let warned: BTreeMap<String, Vec<Warning>> = preview
        .items
        .iter()
        .map(|i| {
            (
                i.path().file_name().unwrap().to_string_lossy().into_owned(),
                i.warnings.clone(),
            )
        })
        .collect();
    assert_eq!(warned["target"], [Warning::RecentlyActive]);
    assert_eq!(warned["com.example.app"], []);
    let later = world.cleaner().preview(picked, next_year());
    assert!(later.items.iter().all(|i| i.warnings.is_empty()));
}

#[test]
fn confirming_the_preview_cleans_caches_and_build_output_only() {
    let world = world();
    let report = world.clean(&EVERYTHING, &Confirmation::default());
    let outcomes = outcomes(&report, world.fx.root());
    for quick in [CACHE, NPM, TARGET, MODULES] {
        assert_eq!(outcomes[quick], Outcome::Removed, "{quick}");
    }
    for separate in [LOGS, REPORTS, TRASH] {
        assert_eq!(
            outcomes[separate],
            Outcome::Skipped(SkipReason::NotConfirmed),
            "{separate}"
        );
    }
    assert!(world.path(LOGS).join("today.log").exists());
    assert!(world.path(REPORTS).join("crash.ips").exists());
    assert_eq!(entries(&world, TRASH), ["folder", "old.zip"]);
    assert!(world.trash.moved.borrow().is_empty());
}

#[test]
fn caches_keep_their_directory_and_build_output_goes_whole() {
    let world = world();
    world.clean(&[CACHE, NPM, TARGET, MODULES], &Confirmation::default());
    assert!(
        entries(&world, CACHE).is_empty(),
        "the app's cache directory stays"
    );
    assert!(entries(&world, NPM).is_empty());
    assert!(!world.path(TARGET).exists());
    assert!(!world.path(MODULES).exists());
    assert!(world.path("code/app/Cargo.toml").exists());
    assert!(world.path("code/app/src/main.rs").exists());
    assert!(world.path("code/app/web/package.json").exists());
}

#[test]
fn logs_go_to_the_trash_and_the_trash_is_emptied_in_place() {
    let world = world();
    let report = world.clean(&[LOGS, REPORTS, TRASH], &all_groups());
    let outcomes = outcomes(&report, world.fx.root());
    assert_eq!(outcomes[LOGS], Outcome::MovedToTrash);
    assert_eq!(outcomes[REPORTS], Outcome::MovedToTrash);
    assert_eq!(outcomes[TRASH], Outcome::Removed);
    assert!(!world.path(LOGS).exists(), "an app's log folder goes whole");
    assert!(
        entries(&world, REPORTS).is_empty(),
        "the reports folder stays"
    );
    assert!(entries(&world, TRASH).is_empty(), "the Trash itself stays");
    let moved = world.trash.moved.borrow();
    assert_eq!(moved.len(), 2);
    assert!(moved.contains(&world.path(LOGS)));
    assert!(moved.contains(&world.path(REPORTS).join("crash.ips")));
}

#[test]
fn the_report_keeps_found_removed_and_freed_apart() {
    let world = world();
    let found = world.scan();
    let estimate: u64 = [TARGET, LOGS].iter().map(|p| found[*p].bytes.0).sum();
    let report = world.clean(&[TARGET, LOGS], &all_groups());
    assert_eq!(report.found(), Bytes(estimate));
    let target = report
        .items
        .iter()
        .find(|i| i.path.ends_with("target"))
        .unwrap();
    assert_eq!(
        target.removed, target.found,
        "nothing changed since the scan"
    );
    let logs = report
        .items
        .iter()
        .find(|i| i.path.ends_with("com.example.app"))
        .unwrap();
    assert_eq!(logs.removed, Bytes(0), "moved, not freed");
    assert_eq!(report.removed(), target.removed);
    assert_eq!(report.moved_to_trash(), logs.found);
    assert!(report.free_before.0 > 0 && report.free_after.0 > 0);
    assert_eq!(
        report.freed(),
        i128::from(report.free_after.0) - i128::from(report.free_before.0)
    );
}

#[test]
fn a_trash_that_refuses_fails_the_item_and_keeps_it() {
    let mut world = world();
    world.trash.refuse = true;
    let report = world.clean(&[LOGS, REPORTS], &all_groups());
    let outcomes = outcomes(&report, world.fx.root());
    let refused = Outcome::Failed(ActionFailure::TrashRefused {
        refused: 1,
        of: 1,
        said: "the Trash said no".into(),
    });
    assert_eq!(outcomes[LOGS], refused);
    assert_eq!(outcomes[REPORTS], refused);
    assert!(world.path(LOGS).join("today.log").exists());
}

#[test]
fn each_separate_group_needs_its_own_confirmation() {
    let world = world();
    let only_trash = Confirmation {
        groups: [Separate::EmptyTrash].into(),
        items: Default::default(),
    };
    let report = world.clean(&[LOGS, TRASH], &only_trash);
    let outcomes = outcomes(&report, world.fx.root());
    assert_eq!(outcomes[TRASH], Outcome::Removed);
    assert_eq!(outcomes[LOGS], Outcome::Skipped(SkipReason::NotConfirmed));
}

#[test]
fn a_preview_is_executed_once_and_unknown_ones_never() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET]), next_year());
    let first = world.execute(&mut cleaner, &preview, &Confirmation::default(), &[]);
    assert_eq!(first.items[0].outcome, Outcome::Removed);
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let control = CleanControl::new();
    let none = Confirmation::default();
    let again = cleaner.execute(preview.id, &none, &context, &control, &world.trash);
    assert_eq!(again.unwrap_err(), CleanError::PreviewUsed(preview.id));
    let unknown = cleaner.execute(PreviewId(99), &none, &context, &control, &world.trash);
    assert_eq!(
        unknown.unwrap_err(),
        CleanError::UnknownPreview(PreviewId(99))
    );
    assert_eq!(
        CleanError::PreviewUsed(PreviewId(3)).to_string(),
        "preview 3 was executed already"
    );
    assert_eq!(
        CleanError::UnknownPreview(PreviewId(4)).to_string(),
        "no preview 4"
    );
    assert_eq!(CleanError::Root("x".into()).to_string(), "root: x");
}

#[test]
fn running_the_same_selection_again_finds_it_already_gone() {
    let world = world();
    let found = world.scan();
    let picked = world.picked(&found, &[TARGET, MODULES, CACHE]);
    let mut cleaner = world.cleaner();
    let first = cleaner.preview(picked.clone(), next_year());
    world.execute(&mut cleaner, &first, &Confirmation::default(), &[]);
    let second = cleaner.preview(picked, next_year());
    let report = world.execute(&mut cleaner, &second, &Confirmation::default(), &[]);
    let outcomes = outcomes(&report, world.fx.root());
    assert_eq!(outcomes[TARGET], Outcome::AlreadyGone);
    assert_eq!(outcomes[MODULES], Outcome::AlreadyGone);
    assert_eq!(
        outcomes[CACHE],
        Outcome::Removed,
        "a kept directory is still there, and stays empty"
    );
    assert!(entries(&world, CACHE).is_empty());
    assert_eq!(report.removed(), Bytes(0));
}

#[test]
fn a_run_cut_short_is_finished_by_the_next_one() {
    let world = World::new(&[
        "code/app/Cargo.toml",
        "code/app/target/a/1=one",
        "code/app/target/a/2=two",
        "code/app/target/b/3=three",
        "code/app/web/package.json",
        "code/app/web/node_modules/x=x",
    ]);
    world.repo("code/app", "target/\nnode_modules/\n");
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET, MODULES]), next_year());
    // What a run that died half-way leaves: one item gone, one half gone.
    fs::remove_dir_all(world.path(MODULES)).unwrap();
    fs::remove_dir_all(world.path(TARGET).join("a")).unwrap();
    let report = world.execute(&mut cleaner, &preview, &Confirmation::default(), &[]);
    let outcomes = outcomes(&report, world.fx.root());
    assert_eq!(outcomes[TARGET], Outcome::Removed);
    assert_eq!(outcomes[MODULES], Outcome::AlreadyGone);
    assert!(!world.path(TARGET).exists());
    let target = report
        .items
        .iter()
        .find(|i| i.path.ends_with("target"))
        .unwrap();
    assert!(
        target.removed < target.found,
        "only what was left was removed"
    );
}

/// A Trash that cancels the cleanup once it has taken something.
struct CancellingTrash<'a> {
    inner: &'a FakeTrash,
    control: &'a CleanControl,
}

impl TrashHost for CancellingTrash<'_> {
    fn trash(&self, path: &Path) -> Result<(), String> {
        let moved = self.inner.trash(path);
        self.control.cancel();
        moved
    }
}

#[test]
fn cancelling_between_items_skips_the_rest() {
    let world = world();
    fs::write(world.path(LOGS).join("big.log"), vec![b'x'; 200_000]).unwrap();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[LOGS, TARGET]), next_year());
    assert_eq!(
        preview.items[0].path(),
        world.path(LOGS),
        "the larger first"
    );
    let control = CleanControl::new();
    let trash = CancellingTrash {
        inner: &world.trash,
        control: &control,
    };
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let report = cleaner
        .execute(preview.id, &all_groups(), &context, &control, &trash)
        .unwrap();
    let outcomes = outcomes(&report, world.fx.root());
    assert_eq!(outcomes[LOGS], Outcome::MovedToTrash);
    assert_eq!(outcomes[TARGET], Outcome::Skipped(SkipReason::Cancelled));
    assert!(world.path(TARGET).exists());
}

#[test]
fn cancelling_before_the_start_skips_everything() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET, CACHE]), next_year());
    let control = CleanControl::new();
    control.cancel();
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let report = cleaner
        .execute(
            preview.id,
            &Confirmation::default(),
            &context,
            &control,
            &world.trash,
        )
        .unwrap();
    assert!(
        report
            .items
            .iter()
            .all(|i| i.outcome == Outcome::Skipped(SkipReason::Cancelled))
    );
    assert_eq!(control.progress().done, 2);
    assert!(world.path(TARGET).exists());
}

#[test]
fn progress_counts_items_and_removed_bytes() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET, LOGS]), next_year());
    let control = CleanControl::new();
    assert_eq!(control.progress().items, 0);
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let report = cleaner
        .execute(preview.id, &all_groups(), &context, &control, &world.trash)
        .unwrap();
    let progress = control.progress();
    assert_eq!((progress.items, progress.done), (2, 2));
    assert_eq!(progress.removed, report.removed());
    assert!(!control.is_cancelled());
}

/// Moves to the Trash, noting the item the cleanup says it is at.
struct Watching<'c> {
    control: &'c CleanControl,
    trash: &'c FakeTrash,
    seen: std::cell::RefCell<Vec<Option<std::path::PathBuf>>>,
}

impl TrashHost for Watching<'_> {
    fn trash(&self, path: &Path) -> Result<(), String> {
        self.seen.borrow_mut().push(self.control.progress().current);
        self.trash.trash(path)
    }
}

#[test]
fn progress_names_the_item_being_cleaned_and_none_after() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET, LOGS]), next_year());
    let control = CleanControl::new();
    assert_eq!(control.progress().current, None);
    let host = Watching {
        control: &control,
        trash: &world.trash,
        seen: Default::default(),
    };
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    cleaner
        .execute(preview.id, &all_groups(), &context, &control, &host)
        .unwrap();
    assert_eq!(host.seen.into_inner(), [Some(world.path(LOGS))]);
    assert_eq!(control.progress().current, None, "nothing after the last");
}

#[test]
fn what_cannot_be_removed_stays_and_the_rest_goes() {
    let world = world();
    fs::create_dir_all(world.path(TARGET).join("locked")).unwrap();
    fs::write(world.path(TARGET).join("locked/keep"), "x").unwrap();
    let found = world.scan();
    let locked = world.path(TARGET).join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &[TARGET]), next_year());
    let report = world.execute(&mut cleaner, &preview, &Confirmation::default(), &[]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let Outcome::Partial(left) = &report.items[0].outcome else {
        panic!("{:?}", report.items[0].outcome);
    };
    assert_eq!(left.failures, 1);
    assert!(report.items[0].removed.0 > 0, "debug/app went");
    assert!(!world.path(TARGET).join("debug").exists());
    assert!(locked.join("keep").exists());
}

#[test]
fn quick_clean_takes_only_preselected_candidates_of_the_categories_asked() {
    let world = world();
    let found = world.scan();
    let all: Vec<_> = found.values().cloned().collect();
    let mut cleaner = world.cleaner();
    let context = Context {
        now: next_year(),
        exclusions: &[],
    };
    let control = CleanControl::new();
    let report = cleaner
        .quick_clean(
            &[QuickCategory::BuildArtifact],
            all,
            &context,
            &control,
            &world.trash,
        )
        .unwrap();
    let cleaned: Vec<String> = outcomes(&report, world.fx.root()).into_keys().collect();
    assert_eq!(cleaned, [TARGET, MODULES]);
    assert!(world.path(CACHE).join("blob").exists());
    let all: Vec<_> = found.values().cloned().collect();
    let fresh: Vec<_> = all
        .into_iter()
        .map(|mut c| {
            c.preselected = false;
            c
        })
        .collect();
    let report = cleaner
        .quick_clean(
            &[QuickCategory::AppCache],
            fresh,
            &context,
            &control,
            &world.trash,
        )
        .unwrap();
    assert!(
        report.items.is_empty(),
        "nothing preselected, nothing cleaned"
    );
    let report = cleaner
        .quick_clean(
            &[QuickCategory::AppCache, QuickCategory::DevCache],
            found.values().cloned().collect(),
            &context,
            &control,
            &world.trash,
        )
        .unwrap();
    let cleaned: Vec<String> = outcomes(&report, world.fx.root()).into_keys().collect();
    assert_eq!(cleaned, [NPM, CACHE]);
    assert!(world.path(LOGS).exists() && world.path(TRASH).join("old.zip").exists());
}

#[test]
fn an_item_nested_in_another_is_set_aside_with_the_one_that_holds_it() {
    let world = World::new(&["~/Library/Caches/com.example.app/x"]);
    let found = world.scan();
    let mut outer = found[CACHE].clone();
    let mut inner = outer.clone();
    inner.id = DirId(outer.id.0 + 1000);
    inner.path = outer.path.join("x");
    outer.bytes = Bytes(1);
    // Sorts between the two as text ('-' before '/'), but not by path.
    let mut sibling = outer.clone();
    sibling.id = DirId(outer.id.0 + 2000);
    sibling.path = outer.path.with_file_name("com.example.app-2");
    let picked = vec![inner.clone(), sibling.clone(), outer.clone()];
    let preview = world.cleaner().preview(picked, next_year());
    let kept: Vec<DirId> = preview.items.iter().map(|i| i.candidate.id).collect();
    assert_eq!(kept, [outer.id, sibling.id], "equal sizes keep path order");
    assert_eq!(preview.rejected.len(), 1);
    assert_eq!(preview.rejected[0].candidate, inner);
    assert_eq!(
        preview.rejected[0].why,
        Rejection::Nested { inside: outer.id }
    );
}
