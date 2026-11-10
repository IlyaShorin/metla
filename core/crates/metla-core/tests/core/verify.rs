//! Git's verdict on project output: ignored and holding nothing tracked.

use std::fs;
use std::path::PathBuf;

use metla_core::domain::Verdict;
use metla_core::git::{Git, repository_of, verify_outputs};

use crate::repo::{git, isolated, script};
use crate::support::Fixture;

struct Repo {
    fx: Fixture,
    git: Git,
    root: PathBuf,
}

impl Repo {
    /// A repository ignoring `target/`, `node_modules/`, `/build` and a
    /// directory with awkward characters.
    fn new() -> Repo {
        let fx = Fixture::new();
        let git_ = isolated(&fx);
        let root = fx.dir("repo");
        git(&git_, &root, &["init", "-q", "-b", "main"]);
        let ignore = "target/\nnode_modules/\n/build\nwe[[]i]rd*/\n";
        fs::write(root.join(".gitignore"), ignore).unwrap();
        Repo {
            fx,
            git: git_,
            root,
        }
    }

    fn dirs(&self, relative: &[&str]) -> Vec<PathBuf> {
        let made = relative.iter().map(|r| {
            let path = self.root.join(r);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("file"), "x").unwrap();
            path
        });
        made.collect()
    }

    fn track(&self, relative: &str) {
        git(&self.git, &self.root, &["add", "-f", relative]);
    }
}

#[test]
fn ignored_output_with_nothing_tracked_is_verified() {
    let repo = Repo::new();
    let paths = repo.dirs(&[
        "target",
        "web/node_modules",
        "build/sub/node_modules",
        "we[i]rd* dir",
        "src",
        "Pods",
    ]);
    assert_eq!(
        verify_outputs(&repo.git, &paths),
        [
            Verdict::Verified,
            Verdict::Verified,
            Verdict::Verified,
            Verdict::Verified,
            Verdict::NotIgnored,
            Verdict::NotIgnored,
        ]
    );
}

#[test]
fn ignored_output_that_holds_a_tracked_file_is_not_verified() {
    let repo = Repo::new();
    let paths = repo.dirs(&["target", "node_modules", "node_modules2", "Pods"]);
    repo.track("target/file");
    repo.track("node_modules2/file");
    repo.track("Pods/file");
    assert_eq!(
        verify_outputs(&repo.git, &paths),
        [
            Verdict::HasTrackedFiles,
            Verdict::Verified,
            Verdict::NotIgnored,
            Verdict::NotIgnored,
        ],
        "a tracked file in node_modules2 says nothing about node_modules"
    );
}

#[test]
fn each_directory_is_judged_by_its_own_repository() {
    let repo = Repo::new();
    let inner = repo.root.join("target/vendored");
    fs::create_dir_all(&inner).unwrap();
    git(&repo.git, &inner, &["init", "-q", "-b", "main"]);
    fs::write(inner.join(".gitignore"), "out/\n").unwrap();
    let outside = repo.fx.dir("loose/target");
    let paths = vec![
        inner.join("out"),
        repo.root.join("target"),
        inner.join("target"),
        outside,
    ];
    for path in &paths {
        fs::create_dir_all(path).unwrap();
    }
    assert_eq!(
        verify_outputs(&repo.git, &paths),
        [
            Verdict::Verified,
            Verdict::Verified,
            Verdict::NotIgnored,
            Verdict::Unverifiable,
        ]
    );
    assert_eq!(repository_of(&paths[0]), Some(inner));
    assert_eq!(repository_of(&paths[1]), Some(repo.root.clone()));
    assert_eq!(
        repository_of(&repo.root),
        None,
        "a repository is not its own"
    );
}

#[test]
fn many_directories_are_checked_in_batches() {
    let repo = Repo::new();
    let names: Vec<String> = (0..1100).map(|n| format!("p{n}/node_modules")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let paths = repo.dirs(&names);
    repo.track("p1099/node_modules/file");
    let verdicts = verify_outputs(&repo.git, &paths);
    let verified = verdicts.iter().filter(|v| **v == Verdict::Verified).count();
    assert_eq!(verified, 1099);
    assert_eq!(verdicts[1099], Verdict::HasTrackedFiles);
}

#[test]
fn when_git_fails_nothing_is_verified() {
    let repo = Repo::new();
    let paths = repo.dirs(&["target", "node_modules"]);
    let failing = [
        Git::new(repo.fx.path("no-such-git")),
        Git::new(script(&repo.fx.path("false-git"), "exit 1")),
        Git::new(script(
            &repo.fx.path("half-git"),
            "case \"$*\" in *ls-files*) exit 128;; esac\nprintf 'target\\0'",
        )),
    ];
    for git_ in failing {
        assert_eq!(
            verify_outputs(&git_, &paths),
            [Verdict::Unverifiable, Verdict::Unverifiable]
        );
    }
}
