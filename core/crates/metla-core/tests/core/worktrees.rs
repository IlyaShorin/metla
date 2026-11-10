//! Worktree status in real temporary repositories: only a clean, pushed,
//! unlocked, idle worktree nobody else manages is removable, and Git
//! removes it. Every blocked worktree is left exactly as it was.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use metla_core::domain::{Manager, Reason, UnixTime, WorktreeStatus};
use metla_core::git::{Git, WorktreeSettings, inspect_worktree, remove_worktree};

use crate::repo::{
    commit, git, git_code, isolated, next_year, now, publish, pushed_repository, pushed_worktree,
    script, tree_state,
};
use crate::support::Fixture;

struct World {
    fx: Fixture,
    git: Git,
    repo: PathBuf,
}

impl World {
    fn new() -> World {
        let fx = Fixture::new();
        let git = isolated(&fx);
        let repo = pushed_repository(&fx, &git);
        World { fx, git, repo }
    }

    /// A clean worktree on a pushed branch.
    fn worktree(&self, relative: &str) -> PathBuf {
        let path = self.fx.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let branch = relative.replace('/', "-");
        pushed_worktree(&self.git, &self.repo, &path, &branch);
        path
    }

    fn settings(&self) -> WorktreeSettings {
        WorktreeSettings {
            home: self.fx.path("home"),
            stale_after: WorktreeSettings::DEFAULT_STALE_AFTER,
        }
    }

    /// The status a year from now, when nothing made here is recent.
    fn status(&self, path: &Path) -> WorktreeStatus {
        self.status_at(path, next_year())
    }

    fn status_at(&self, path: &Path, at: UnixTime) -> WorktreeStatus {
        let activity = Some(UnixTime(at.0 - 60 * 24 * 3600));
        inspect_worktree(&self.git, path, activity, at, &self.settings())
    }

    fn listed(&self) -> String {
        git(&self.git, &self.repo, &["worktree", "list", "--porcelain"])
    }
}

fn reasons(status: &WorktreeStatus) -> BTreeSet<Reason> {
    match status {
        WorktreeStatus::Removable { .. } => BTreeSet::new(),
        WorktreeStatus::Blocked(reasons) => reasons.iter().cloned().collect(),
    }
}

/// The worktree is blocked for exactly `expected`, and inspecting it (and
/// asking Git to remove it, where Git itself would refuse) changed
/// nothing.
fn assert_blocked(world: &World, path: &Path, expected: &[Reason]) {
    let before = tree_state(path);
    let status = world.status(path);
    assert_eq!(
        reasons(&status),
        expected.iter().cloned().collect(),
        "{status:?}"
    );
    assert_eq!(tree_state(path), before, "inspection changed the worktree");
}

#[test]
fn a_clean_pushed_worktree_is_removable_and_git_removes_it() {
    let world = World::new();
    let wt = world.worktree("wt");
    fs::create_dir_all(wt.join("target/debug")).unwrap();
    fs::write(wt.join("target/debug/app"), "binary").unwrap();
    assert_eq!(
        world.status(&wt),
        WorktreeStatus::Removable {
            branch: Some("wt".into())
        }
    );
    remove_worktree(&world.git, &wt).unwrap();
    assert!(!wt.exists(), "ignored build output goes with it");
    assert!(!world.listed().contains("/wt\n"));
    assert!(world.repo.join("README").exists());
}

#[test]
fn modified_or_staged_tracked_files_are_uncommitted() {
    let world = World::new();
    let modified = world.worktree("modified");
    fs::write(modified.join("README"), "changed\n").unwrap();
    assert_blocked(&world, &modified, &[Reason::Uncommitted]);
    let staged = world.worktree("staged");
    fs::write(staged.join("new.txt"), "new\n").unwrap();
    git(&world.git, &staged, &["add", "new.txt"]);
    assert_blocked(&world, &staged, &[Reason::Uncommitted]);
    let refused = remove_worktree(&world.git, &modified).unwrap_err();
    assert!(refused.contains("modified or untracked"), "{refused}");
    assert_eq!(
        fs::read_to_string(modified.join("README")).unwrap(),
        "changed\n"
    );
}

#[test]
fn untracked_files_block_removal_but_ignored_ones_do_not() {
    let world = World::new();
    let wt = world.worktree("wt");
    fs::create_dir_all(wt.join("node_modules/x")).unwrap();
    fs::write(wt.join("node_modules/x/index.js"), "").unwrap();
    assert!(matches!(
        world.status(&wt),
        WorktreeStatus::Removable { .. }
    ));
    fs::create_dir_all(wt.join("notes/deep")).unwrap();
    fs::write(wt.join("notes/deep/idea.txt"), "keep me").unwrap();
    assert_blocked(&world, &wt, &[Reason::Untracked]);
    assert!(remove_worktree(&world.git, &wt).is_err());
    assert!(wt.join("notes/deep/idea.txt").exists());
}

#[test]
fn commits_ahead_of_the_upstream_are_unpushed() {
    let world = World::new();
    let wt = world.worktree("wt");
    commit(&world.git, &wt, "local.txt");
    assert_blocked(&world, &wt, &[Reason::Unpushed]);
}

#[test]
fn without_an_upstream_commits_no_remote_has_are_unpushed() {
    let world = World::new();
    let path = world.fx.path("local");
    let at = path.to_str().unwrap();
    git(
        &world.git,
        &world.repo,
        &["worktree", "add", "-q", "-b", "local", at],
    );
    assert!(
        matches!(world.status(&path), WorktreeStatus::Removable { .. }),
        "a new branch at a pushed commit loses nothing"
    );
    commit(&world.git, &path, "only-here.txt");
    assert_blocked(&world, &path, &[Reason::Unpushed]);
}

#[test]
fn an_upstream_that_is_a_local_branch_does_not_hide_unpushed_commits() {
    let world = World::new();
    let path = world.fx.path("tracking");
    let at = path.to_str().unwrap();
    git(
        &world.git,
        &world.repo,
        &["worktree", "add", "-q", "-b", "tracking", at],
    );
    commit(&world.git, &world.repo, "main-only.txt");
    git(
        &world.git,
        &path,
        &["branch", "-q", "--set-upstream-to=main"],
    );
    git(&world.git, &path, &["merge", "-q", "--ff-only", "main"]);
    assert_blocked(&world, &path, &[Reason::Unpushed]);
}

#[test]
fn in_a_repository_without_remotes_every_commit_is_unpushed() {
    let fx = Fixture::new();
    let git_ = isolated(&fx);
    let repo = fx.dir("solo");
    git(&git_, &repo, &["init", "-q", "-b", "main"]);
    commit(&git_, &repo, "a.txt");
    let wt = fx.path("solo-wt");
    git(
        &git_,
        &repo,
        &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
    );
    let settings = WorktreeSettings {
        home: fx.path("home"),
        stale_after: WorktreeSettings::DEFAULT_STALE_AFTER,
    };
    let status = inspect_worktree(&git_, &wt, Some(now()), next_year(), &settings);
    assert_eq!(reasons(&status), BTreeSet::from([Reason::Unpushed]));
}

#[test]
fn a_detached_head_is_judged_like_a_branch_without_upstream() {
    let world = World::new();
    let path = world.fx.path("detached");
    let at = path.to_str().unwrap();
    git(
        &world.git,
        &world.repo,
        &["worktree", "add", "-q", "--detach", at],
    );
    assert_eq!(
        world.status(&path),
        WorktreeStatus::Removable { branch: None }
    );
    commit(&world.git, &path, "detached.txt");
    assert_blocked(&world, &path, &[Reason::Unpushed]);
}

#[test]
fn a_locked_worktree_is_blocked_and_git_refuses_it() {
    let world = World::new();
    let wt = world.worktree("wt");
    let at = wt.to_str().unwrap();
    git(
        &world.git,
        &world.repo,
        &["worktree", "lock", "--reason", "on a drive", at],
    );
    assert_blocked(&world, &wt, &[Reason::Locked]);
    let refused = remove_worktree(&world.git, &wt).unwrap_err();
    assert!(refused.contains("locked"), "{refused}");
    assert!(wt.join("README").exists());
}

#[test]
fn a_rebase_in_progress_blocks_removal() {
    let world = World::new();
    let wt = world.worktree("wt");
    fs::write(wt.join("README"), "theirs\n").unwrap();
    git(&world.git, &wt, &["commit", "-q", "-am", "theirs"]);
    publish(&world.git, &wt, "wt");
    fs::write(world.repo.join("README"), "ours\n").unwrap();
    git(&world.git, &world.repo, &["commit", "-q", "-am", "ours"]);
    assert_ne!(git_code(&world.git, &wt, &["rebase", "main"]), 0);
    let status = reasons(&world.status(&wt));
    assert!(status.contains(&Reason::OperationInProgress), "{status:?}");
    assert!(
        status.contains(&Reason::Uncommitted),
        "a conflict: {status:?}"
    );
}

#[test]
fn every_operation_git_leaves_a_mark_for_blocks_removal() {
    let world = World::new();
    let wt = world.worktree("wt");
    let own = world.repo.join(".git/worktrees/wt");
    for mark in [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
    ] {
        fs::write(own.join(mark), "").unwrap();
        assert_blocked(&world, &wt, &[Reason::OperationInProgress]);
        fs::remove_file(own.join(mark)).unwrap();
    }
    assert!(matches!(
        world.status(&wt),
        WorktreeStatus::Removable { .. }
    ));
}

#[test]
fn a_recently_active_worktree_is_in_use() {
    let world = World::new();
    let wt = world.worktree("wt");
    let later = next_year();
    let settings = world.settings();
    let at = |days: i64| Some(UnixTime(later.0 - days * 24 * 3600));
    let status = |activity| inspect_worktree(&world.git, &wt, activity, later, &settings);
    assert_eq!(reasons(&status(at(30))), BTreeSet::from([Reason::InUse]));
    assert!(matches!(status(at(31)), WorktreeStatus::Removable { .. }));
    assert_eq!(
        reasons(&status(None)),
        BTreeSet::from([Reason::InUse]),
        "unknown activity counts as recent"
    );
    let shorter = WorktreeSettings {
        stale_after: Duration::from_secs(29 * 24 * 3600),
        ..world.settings()
    };
    let configured = inspect_worktree(&world.git, &wt, at(30), later, &shorter);
    assert!(matches!(configured, WorktreeStatus::Removable { .. }));
}

#[test]
fn a_worktree_git_just_used_is_in_use_whatever_the_scan_saw() {
    let world = World::new();
    let wt = world.worktree("wt");
    let long_ago = Some(UnixTime(now().0 - 400 * 24 * 3600));
    let status = inspect_worktree(&world.git, &wt, long_ago, now(), &world.settings());
    assert_eq!(reasons(&status), BTreeSet::from([Reason::InUse]));
}

#[test]
fn a_locked_index_means_git_is_at_work() {
    let world = World::new();
    let wt = world.worktree("wt");
    fs::write(world.repo.join(".git/worktrees/wt/index.lock"), "").unwrap();
    assert_blocked(&world, &wt, &[Reason::InUse]);
}

#[test]
fn worktrees_codex_or_claude_manage_are_never_removable() {
    let world = World::new();
    let codex = world.worktree("home/.codex/worktrees/a1/repo");
    assert_blocked(&world, &codex, &[Reason::Managed(Manager::Codex)]);
    let claude = world.worktree("repo/.claude/worktrees/feature");
    assert_blocked(&world, &claude, &[Reason::Managed(Manager::Claude)]);
}

#[test]
fn the_main_worktree_and_unlisted_directories_are_not_linked_worktrees() {
    let world = World::new();
    assert_blocked(&world, &world.repo, &[Reason::NotListed]);
    let wt = world.worktree("wt");
    let copy = world.fx.dir("copy");
    fs::copy(wt.join(".git"), copy.join(".git")).unwrap();
    assert_blocked(&world, &copy, &[Reason::NotListed]);
    let plain = world.fx.dir("plain");
    assert_blocked(&world, &plain, &[Reason::NotListed]);
    let pointer = world.fx.dir("pointer");
    for head in ["gitdir: /nowhere/.git/modules/x\n", "not a pointer\n"] {
        fs::write(pointer.join(".git"), head).unwrap();
        assert_blocked(&world, &pointer, &[Reason::NotListed]);
    }
    fs::remove_file(pointer.join(".git")).unwrap();
    fs::create_dir(pointer.join(".git")).unwrap();
    assert_blocked(&world, &pointer, &[Reason::NotListed]);
}

#[test]
fn a_pointer_into_a_repository_that_does_not_exist_cannot_be_inspected() {
    let world = World::new();
    let ghost = world.fx.dir("ghost");
    fs::write(
        ghost.join(".git"),
        "gitdir: /nowhere/.git/worktrees/ghost\n",
    )
    .unwrap();
    let status = reasons(&world.status(&ghost));
    assert!(
        matches!(status.iter().next(), Some(Reason::InspectionFailed(_))),
        "{status:?}"
    );
}

#[test]
fn a_failing_git_blocks_every_worktree() {
    let world = World::new();
    let wt = world.worktree("wt");
    let failing = [
        Git::new(world.fx.path("no-such-git")),
        Git::new(script(&world.fx.path("false-git"), "exit 1")),
    ];
    for git_ in failing {
        let status = inspect_worktree(&git_, &wt, None, next_year(), &world.settings());
        let reasons = reasons(&status);
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(matches!(
            reasons.iter().next(),
            Some(Reason::InspectionFailed(_))
        ));
        assert!(remove_worktree(&git_, &wt).is_err());
    }
    assert!(wt.join("README").exists());
}

#[test]
fn git_that_prints_nonsense_blocks_the_worktree() {
    let world = World::new();
    let wt = world.worktree("wt");
    let listing = format!(
        "printf 'worktree {}\\0\\0worktree {}\\0\\0'",
        world.repo.display(),
        wt.display()
    );
    let body = format!(
        "case \"$*\" in\n*'worktree list'*) {listing};;\n*status*) printf 'x nonsense\\0';;\nesac"
    );
    let liar = Git::new(script(&world.fx.path("liar-git"), &body));
    let status = inspect_worktree(&liar, &wt, Some(now()), next_year(), &world.settings());
    let reasons = reasons(&status);
    assert!(
        matches!(reasons.iter().next(), Some(Reason::InspectionFailed(what)) if what.contains("nonsense")),
        "{reasons:?}"
    );
}

#[test]
fn a_directory_listed_as_the_main_worktree_is_not_a_linked_one() {
    let world = World::new();
    let wt = world.worktree("wt");
    let listing = format!(
        "printf 'worktree {}\\0\\0worktree {}\\0\\0'",
        wt.display(),
        world.repo.display()
    );
    let body = format!("case \"$*\" in\n*'worktree list'*) {listing};;\nesac");
    let liar = Git::new(script(&world.fx.path("first-git"), &body));
    let status = inspect_worktree(&liar, &wt, Some(now()), next_year(), &world.settings());
    assert_eq!(reasons(&status), BTreeSet::from([Reason::NotListed]));
}

#[test]
fn every_reason_is_reported_at_once() {
    let world = World::new();
    let wt = world.worktree("home/.codex/worktrees/b2/repo");
    fs::write(wt.join("README"), "changed\n").unwrap();
    fs::write(wt.join("untracked.txt"), "").unwrap();
    let at = wt.to_str().unwrap();
    git(&world.git, &world.repo, &["worktree", "lock", at]);
    let status = reasons(&world.status_at(&wt, now()));
    assert_eq!(
        status,
        BTreeSet::from([
            Reason::Uncommitted,
            Reason::Untracked,
            Reason::Locked,
            Reason::InUse,
            Reason::Managed(Manager::Codex),
        ])
    );
}
