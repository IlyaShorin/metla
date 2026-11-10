//! Revalidation: whatever changed between the scan and the cleanup, an item
//! that no longer is what was previewed is skipped with the reason and
//! nothing of it, or around it, is touched.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metla_core::clean::Cleaner;
use metla_core::domain::{Candidate, Confirmation, Errno, Exclusion, Outcome, SkipReason};
use metla_core::git::Git;

use crate::repo::{git, next_year, tree_state};
use crate::world::World;

const TARGET: &str = "code/app/target";

fn world() -> World {
    let world = World::new(&[
        "code/app/Cargo.toml",
        "code/app/src/main.rs=fn main() {}",
        "code/app/target/debug/app=binary",
        "elsewhere/keep=precious",
    ]);
    world.repo("code/app", "target/\n");
    world
}

/// Scans, previews the target, lets `change` happen, then executes.
fn after(world: &World, change: impl FnOnce(&World)) -> Outcome {
    after_with(world, |c| c, &[], change)
}

fn after_with(
    world: &World,
    alter: impl FnOnce(Candidate) -> Candidate,
    exclusions: &[Exclusion],
    change: impl FnOnce(&World),
) -> Outcome {
    let found = world.scan();
    let mut cleaner = world.cleaner();
    let target = world.picked(&found, &[TARGET]).remove(0);
    let picked = vec![alter(target)];
    let preview = cleaner.preview(picked, next_year());
    change(world);
    let report = world.execute(&mut cleaner, &preview, &Confirmation::default(), exclusions);
    report.items[0].outcome.clone()
}

fn skipped(reason: SkipReason) -> Outcome {
    Outcome::Skipped(reason)
}

#[test]
fn a_candidate_replaced_by_a_symlink_is_skipped_and_the_target_kept() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::rename(w.path(TARGET), w.path("code/app/old-target")).unwrap();
        symlink(w.path("elsewhere"), w.path(TARGET)).unwrap();
    });
    assert_eq!(outcome, skipped(SkipReason::Symlink));
    assert_eq!(
        fs::read_to_string(world.path("elsewhere/keep")).unwrap(),
        "precious"
    );
    assert!(world.path(TARGET).is_symlink());
}

#[test]
fn a_candidate_replaced_by_another_directory_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::rename(w.path(TARGET), w.path("code/app/old-target")).unwrap();
        fs::create_dir_all(w.path(TARGET).join("new")).unwrap();
        fs::write(w.path(TARGET).join("new/file"), "new").unwrap();
    });
    assert_eq!(outcome, skipped(SkipReason::Replaced));
    assert!(world.path(TARGET).join("new/file").exists());
}

#[test]
fn a_directory_on_another_device_than_scanned_is_skipped() {
    let world = world();
    let before = tree_state(&world.path(TARGET));
    let elsewhere = |mut c: Candidate| {
        c.device += 1;
        c
    };
    let outcome = after_with(&world, elsewhere, &[], |_| {});
    assert_eq!(outcome, skipped(SkipReason::MountPoint));
    assert_eq!(tree_state(&world.path(TARGET)), before);
}

#[test]
fn a_candidate_that_became_a_file_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::remove_dir_all(w.path(TARGET)).unwrap();
        fs::write(w.path(TARGET), "a file now").unwrap();
    });
    assert_eq!(outcome, skipped(SkipReason::NotADirectory));
    assert_eq!(
        fs::read_to_string(world.path(TARGET)).unwrap(),
        "a file now"
    );
}

#[test]
fn a_candidate_whose_marker_went_away_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::remove_file(w.path("code/app/Cargo.toml")).unwrap()
    });
    assert_eq!(outcome, skipped(SkipReason::RuleMismatch));
    assert!(world.path(TARGET).join("debug/app").exists());
}

#[test]
fn a_candidate_no_longer_in_a_repository_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::remove_dir_all(w.path("code/app/.git")).unwrap()
    });
    assert_eq!(outcome, skipped(SkipReason::RuleMismatch));
}

#[test]
fn a_candidate_git_now_tracks_files_in_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        git(
            &w.git,
            &w.path("code/app"),
            &["add", "-f", "target/debug/app"],
        );
    });
    assert_eq!(outcome, skipped(SkipReason::HasTrackedFiles));
    assert!(world.path(TARGET).join("debug/app").exists());
}

#[test]
fn a_candidate_git_no_longer_ignores_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::write(w.path("code/app/.gitignore"), "").unwrap()
    });
    assert_eq!(outcome, skipped(SkipReason::NotIgnored));
}

#[test]
fn project_output_git_cannot_judge_is_skipped() {
    let world = world();
    let found = world.scan();
    let rules = Arc::new(world.rules());
    let broken = Git::new(world.fx.path("no-such-git"));
    let mut cleaner = Cleaner::new(world.fx.root(), None, rules, broken, world.settings()).unwrap();
    let preview = cleaner.preview(world.picked(&found, &[TARGET]), next_year());
    let report = world.execute(&mut cleaner, &preview, &Confirmation::default(), &[]);
    assert_eq!(report.items[0].outcome, skipped(SkipReason::Unverifiable));
}

#[test]
fn a_candidate_whose_parent_became_a_symlink_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::rename(w.path("code/app"), w.path("code/moved")).unwrap();
        symlink("moved", w.path("code/app")).unwrap();
    });
    assert_eq!(outcome, skipped(SkipReason::AncestorChanged));
    assert!(world.path("code/moved/target/debug/app").exists());
}

#[test]
fn a_candidate_whose_parent_became_a_file_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::rename(w.path("code/app"), w.path("code/moved")).unwrap();
        fs::write(w.path("code/app"), "a file").unwrap();
    });
    assert_eq!(outcome, skipped(SkipReason::AncestorChanged));
}

#[test]
fn a_candidate_excluded_after_the_preview_is_skipped() {
    let world = world();
    let kept = |path: PathBuf| vec![Exclusion::new(&path).unwrap()];
    for exclusions in [
        kept(world.path(TARGET)),
        kept(world.path(TARGET).join("debug/app")),
        kept(world.path("code")),
    ] {
        let outcome = after_with(&world, |c| c, &exclusions, |_| {});
        assert_eq!(outcome, skipped(SkipReason::Excluded));
    }
    assert!(world.path(TARGET).join("debug/app").exists());
}

#[test]
fn a_candidate_removed_meanwhile_is_already_gone() {
    let world = world();
    let outcome = after(&world, |w| fs::remove_dir_all(w.path(TARGET)).unwrap());
    assert_eq!(outcome, Outcome::AlreadyGone);
    let outcome = after(&self::world(), |w| {
        fs::remove_dir_all(w.path("code/app")).unwrap()
    });
    assert_eq!(outcome, Outcome::AlreadyGone, "its parent too");
}

#[test]
fn a_candidate_that_cannot_be_read_is_skipped() {
    let world = world();
    let outcome = after(&world, |w| {
        fs::set_permissions(w.path(TARGET), fs::Permissions::from_mode(0o000)).unwrap();
    });
    fs::set_permissions(world.path(TARGET), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        outcome,
        skipped(SkipReason::Unreadable(Errno(libc::EACCES)))
    );
}

#[test]
fn a_candidate_outside_the_root_is_skipped() {
    let world = world();
    let outside = |path: &str| {
        let path = PathBuf::from(path);
        move |mut c: Candidate| {
            c.path = path;
            c
        }
    };
    let other = world.fx.root().parent().unwrap().join("other/target");
    let up = world.path("code/../code/app/target");
    for alter in [
        outside(other.to_str().unwrap()),
        outside(up.to_str().unwrap()),
        outside(world.fx.root().to_str().unwrap()),
    ] {
        assert_eq!(
            after_with(&world, alter, &[], |_| {}),
            skipped(SkipReason::OutsideRoot)
        );
    }
    assert!(Path::new(&world.path(TARGET)).join("debug/app").exists());
}
