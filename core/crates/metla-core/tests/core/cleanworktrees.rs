//! Cleaning worktrees: only through Git, only when removable at that very
//! moment, only with their own confirmation; their build output is offered
//! on its own, and inside a worktree another tool manages only one item at
//! a time.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use metla_core::clean::Cleaner;
use metla_core::domain::{
    ActionFailure, Confirmation, Group, Manager, Outcome, Reason, Rejection, Separate, SkipReason,
    Warning,
};
use metla_core::git::Git;

use crate::repo::{
    commit, git, next_year, publish, pushed_repository, pushed_worktree, script, tree_state,
};
use crate::world::{World, all_groups, outcomes};

/// A pushed repository whose `main` has a `Cargo.toml`, and a clean
/// worktree `wt` with build output in `wt/target`.
fn world() -> World {
    let world = World::new(&[]);
    let repo = pushed_repository(&world.fx, &world.git);
    commit(&world.git, &repo, "Cargo.toml");
    publish(&world.git, &repo, "main");
    worktree(&world, "wt");
    world
}

fn worktree(world: &World, relative: &str) -> PathBuf {
    let repo = world.path("repo");
    let path = world.path(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    pushed_worktree(&world.git, &repo, &path, &relative.replace('/', "-"));
    fs::create_dir_all(path.join("target/debug")).unwrap();
    fs::write(path.join("target/debug/app"), vec![1u8; 50_000]).unwrap();
    path
}

fn worktrees_only() -> Confirmation {
    Confirmation {
        groups: [Separate::RemoveWorktrees].into(),
        items: Default::default(),
    }
}

#[test]
fn a_removable_worktree_is_removed_by_git_once_confirmed() {
    let world = world();
    let report = world.clean(&["wt"], &worktrees_only());
    assert_eq!(report.items[0].outcome, Outcome::Removed);
    assert!(report.items[0].removed.0 >= 50_000, "{:?}", report.items[0]);
    assert!(!world.path("wt").exists());
    let listed = git(&world.git, &world.path("repo"), &["worktree", "list"]);
    assert_eq!(listed.lines().count(), 1);
}

#[test]
fn a_worktree_is_never_removed_without_its_own_confirmation() {
    let world = world();
    let before = tree_state(&world.path("wt"));
    let report = world.clean(&["wt"], &Confirmation::default());
    assert_eq!(
        report.items[0].outcome,
        Outcome::Skipped(SkipReason::NotConfirmed)
    );
    assert_eq!(tree_state(&world.path("wt")), before);
}

#[test]
fn a_blocked_worktree_is_left_out_but_its_build_output_is_offered() {
    let world = world();
    fs::write(world.path("wt/README"), "edited\n").unwrap();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &["wt", "wt/target"]), next_year());
    assert_eq!(preview.rejected.len(), 1);
    let Rejection::Blocked(reasons) = &preview.rejected[0].why else {
        panic!("{:?}", preview.rejected[0]);
    };
    assert!(reasons.contains(&Reason::Uncommitted));
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].group(), Group::Quick);
    let report = world.execute(&mut cleaner, &preview, &all_groups(), &[]);
    assert_eq!(report.items[0].outcome, Outcome::Removed);
    assert!(!world.path("wt/target").exists());
    assert_eq!(
        fs::read_to_string(world.path("wt/README")).unwrap(),
        "edited\n"
    );
}

#[test]
fn build_output_inside_a_worktree_that_goes_is_not_listed_twice() {
    let world = world();
    let found = world.scan();
    let picked = world.picked(&found, &["wt/target", "wt"]);
    let preview = world.cleaner().preview(picked, next_year());
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].path(), world.path("wt"));
    assert_eq!(
        preview.rejected[0].why,
        Rejection::Nested {
            inside: found["wt"].id
        }
    );
}

#[test]
fn a_worktree_that_changed_after_the_preview_is_left_exactly_as_it_is() {
    let world = world();
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let preview = cleaner.preview(world.picked(&found, &["wt"]), next_year());
    assert_eq!(preview.items.len(), 1, "removable when previewed");
    fs::write(world.path("wt/notes.txt"), "new work").unwrap();
    let before = tree_state(&world.path("wt"));
    let report = world.execute(&mut cleaner, &preview, &worktrees_only(), &[]);
    let Outcome::Skipped(SkipReason::Blocked(reasons)) = &report.items[0].outcome else {
        panic!("{:?}", report.items[0].outcome);
    };
    assert!(reasons.contains(&Reason::Untracked));
    assert_eq!(tree_state(&world.path("wt")), before);
}

#[test]
fn what_git_refuses_is_a_failure_with_its_message() {
    let world = world();
    let real = world.git.program().to_path_buf();
    let body = format!(
        "case \"$*\" in\n*'worktree remove'*) echo 'fatal: not today' >&2; exit 128;;\n*) exec '{}' \"$@\";;\nesac",
        real.display()
    );
    let refusing = Git::new(script(&world.fx.path("refusing-git"), &body));
    let refusing = isolate(&world, refusing);
    let found = world.scan();
    let rules = Arc::new(world.rules());
    let mut cleaner =
        Cleaner::new(world.fx.root(), None, rules, refusing, world.settings()).unwrap();
    let preview = cleaner.preview(world.picked(&found, &["wt"]), next_year());
    let before = tree_state(&world.path("wt"));
    let report = world.execute(&mut cleaner, &preview, &worktrees_only(), &[]);
    assert_eq!(
        report.items[0].outcome,
        Outcome::Failed(ActionFailure::GitRefused(
            "git exited with 128: fatal: not today".into()
        ))
    );
    assert_eq!(report.items[0].removed.0, 0);
    assert_eq!(tree_state(&world.path("wt")), before);
}

/// `git` with the test environment of `world`.
fn isolate(world: &World, git: Git) -> Git {
    let home = world.fx.path(".git-home-refusing");
    fs::create_dir_all(&home).unwrap();
    git.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
}

#[test]
fn build_output_in_a_worktree_codex_manages_is_cleaned_only_one_by_one() {
    let world = world();
    worktree(&world, "home/.codex/worktrees/a1/repo");
    let found = world.scan();
    let codex_target = "home/.codex/worktrees/a1/repo/target";
    let inside = &found[codex_target];
    assert_eq!(inside.managed, Some(Manager::Codex));
    assert!(!inside.preselected);
    let mut cleaner = world.cleaner();
    let picked = world.picked(&found, &["home/.codex/worktrees/a1/repo", codex_target]);
    let preview = cleaner.preview(picked.clone(), next_year());
    assert!(matches!(
        &preview.rejected[0].why,
        Rejection::Blocked(reasons) if reasons.contains(&Reason::Managed(Manager::Codex))
    ));
    assert_eq!(
        preview.items[0].warnings,
        [Warning::InsideManagedWorktree(Manager::Codex)]
    );
    let report = world.execute(&mut cleaner, &preview, &all_groups(), &[]);
    assert_eq!(
        report.items[0].outcome,
        Outcome::Skipped(SkipReason::NotConfirmed)
    );
    assert!(world.path(codex_target).exists());
    let preview = cleaner.preview(picked, next_year());
    let one_by_one = Confirmation {
        items: [inside.id].into(),
        ..Confirmation::default()
    };
    let report = world.execute(&mut cleaner, &preview, &one_by_one, &[]);
    assert_eq!(
        outcomes(&report, world.fx.root())[codex_target],
        Outcome::Removed
    );
    assert!(!world.path(codex_target).exists());
    assert!(world.path("home/.codex/worktrees/a1/repo/README").exists());
}
