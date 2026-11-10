//! End to end on a fixture made by `fixture create`: scan it, preview a
//! cleanup, confirm it, and see that nothing outside a fixture is ever
//! cleaned.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

struct Made {
    _dir: tempfile::TempDir,
    fixture: PathBuf,
    index: PathBuf,
}

fn cli(args: &[OsString]) -> (i32, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = metla_cli::run(args, &mut out, &mut err);
    let text = |bytes| String::from_utf8(bytes).unwrap();
    (code, text(out), text(err))
}

fn args(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

fn ok(words: &[&str]) -> String {
    let (code, out, err) = cli(&args(words));
    assert_eq!((code, err.as_str()), (0, ""), "{words:?}");
    out
}

impl Made {
    /// A fixture, scanned with its own home.
    fn new() -> Made {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let fixture = base.join("fixture");
        let index = base.join("index.sqlite");
        let made = Made {
            _dir: dir,
            fixture,
            index,
        };
        ok(&["fixture", "create", made.text(&made.fixture)]);
        made.scan();
        made
    }

    fn text<'a>(&self, path: &'a Path) -> &'a str {
        path.to_str().unwrap()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.fixture.join(relative)
    }

    fn scan(&self) {
        let home = self.path("home");
        let (root, index) = (self.text(&self.fixture), self.text(&self.index));
        ok(&[
            "scan",
            "--root",
            root,
            "--index",
            index,
            "--home",
            home.to_str().unwrap(),
        ]);
    }

    fn clean(&self, extra: &[&str]) -> String {
        let (root, index) = (self.text(&self.fixture), self.text(&self.index));
        let mut words = vec!["clean", "--index", index, "--root", root];
        words.extend(extra);
        ok(&words)
    }
}

/// The outcome of every item in a report, by path in the fixture.
fn outcomes(out: &str) -> BTreeMap<String, String> {
    let lines = out.lines().filter_map(|line| line.strip_prefix("  home/"));
    let pairs = lines.filter_map(|line| line.split_once(": "));
    pairs
        .map(|(path, outcome)| (format!("home/{path}"), outcome.to_owned()))
        .collect()
}

#[test]
fn a_fixture_holds_every_kind_of_candidate_and_worktrees_in_several_states() {
    let made = Made::new();
    assert!(made.path(".metla-fixture").is_file());
    let candidates = ok(&[
        "candidates",
        "--index",
        made.text(&made.index),
        "--limit",
        "100",
    ]);
    for rule in [
        "app-cache.library",
        "dev-cache.homebrew",
        "dev-cache.npm",
        "build.cargo-target",
        "build.node-modules",
        "log.user-library",
        "log.diagnostic-reports",
        "temp.trash",
        "worktree.linked",
    ] {
        assert!(candidates.contains(rule), "{rule}: {candidates}");
    }
    assert_eq!(candidates.matches("worktree.linked").count(), 7);
    assert!(made.path("home/Documents/letter.txt").exists());
}

#[test]
fn without_yes_the_preview_is_printed_and_nothing_is_removed() {
    let made = Made::new();
    let out = made.clean(&[]);
    assert!(out.starts_with("preview 1: 13 items, about "), "{out}");
    assert!(out.contains("  permanently  "));
    assert!(out.contains("  to Trash (its own confirmation)  "));
    assert!(out.contains("  empty the Trash (its own confirmation)  "));
    assert!(out.ends_with("nothing removed: run again with --yes to confirm every group\n"));
    assert!(made.path("home/code/app/target/debug/app").exists());
    assert!(made.path("home/.Trash/old-installer.dmg").exists());
}

#[test]
fn worktrees_are_previewed_with_why_the_blocked_ones_stay() {
    let made = Made::new();
    let out = made.clean(&["--category", "worktree"]);
    assert!(out.starts_with("preview 1: 1 items"), "{out}");
    for (place, why) in [
        ("home/code/app-dirty", "blocked: uncommitted changes"),
        ("home/code/app-untracked", "blocked: untracked files"),
        ("home/code/app-unpushed", "blocked: unpushed commits"),
        ("home/code/app-locked", "blocked: locked"),
        (
            "home/.codex/worktrees/a1b2/app",
            "blocked: managed by Codex",
        ),
        (
            "home/code/app/.claude/worktrees/feature",
            "blocked: managed by Claude",
        ),
    ] {
        assert!(
            out.contains(&format!("  set aside  {place}  {why}\n")),
            "{place}: {out}"
        );
    }
    let out = made.clean(&["--category", "worktree", "--yes"]);
    assert_eq!(outcomes(&out)["home/code/app-clean"], "removed");
    assert!(!made.path("home/code/app-clean").exists());
    assert!(made.path("home/code/app-dirty/README.md").exists());
}

#[test]
fn with_yes_the_default_selection_is_cleaned_and_reported() {
    let made = Made::new();
    let out = made.clean(&["--yes"]);
    let outcomes = outcomes(&out);
    assert_eq!(outcomes.len(), 13);
    assert_eq!(outcomes["home/code/app/target"], "removed");
    assert_eq!(
        outcomes["home/Library/Logs/com.example.editor"],
        "moved to Trash"
    );
    assert_eq!(outcomes["home/.Trash"], "removed");
    assert!(out.contains("moved to Trash "), "{out}");
    assert!(!made.path("home/code/app/target").exists());
    assert!(made.path("home/Library/Caches/com.example.editor").is_dir());
    assert!(made.path("home/code/app/src/main.rs").exists());
    assert!(
        made.path("home/.codex/worktrees/a1b2/app/target").exists(),
        "managed"
    );
    assert!(made.path("home/Documents/letter.txt").exists());
    let again = made.clean(&["--yes"]);
    assert!(
        again.starts_with("preview 1: 0 items"),
        "cleaned is no longer offered: {again}"
    );
}

#[test]
fn the_fixture_trash_keeps_both_of_two_items_with_one_name() {
    let made = Made::new();
    let trash = made.path("home/.Trash");
    fs::create_dir_all(trash.join("com.example.editor")).unwrap();
    let out = made.clean(&["--category", "log", "--yes"]);
    let outcomes = outcomes(&out);
    assert_eq!(
        outcomes["home/Library/Logs/com.example.editor"],
        "moved to Trash"
    );
    assert!(
        trash.join("com.example.editor 2/today.log").exists(),
        "{out}"
    );
    assert!(trash.join("editor.crash").exists());
}

#[test]
fn a_root_without_the_fixture_marker_is_refused() {
    let made = Made::new();
    fs::remove_file(made.path(".metla-fixture")).unwrap();
    made.scan();
    let (root, index) = (made.text(&made.fixture), made.text(&made.index));
    let (code, out, err) = cli(&args(&["clean", "--index", index, "--root", root, "--yes"]));
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(
        err.ends_with("has no .metla-fixture: development builds clean only fixtures\n"),
        "{err}"
    );
    assert!(made.path("home/code/app/target/debug/app").exists());
}

#[test]
fn a_picture_of_another_root_or_without_rules_is_refused() {
    let made = Made::new();
    let index = made.text(&made.index);
    let other = made.path("home");
    let (code, _, err) = cli(&args(&[
        "clean",
        "--index",
        index,
        "--root",
        other.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert!(
        err.starts_with("error: the index holds a picture of "),
        "{err}"
    );
    let root = made.text(&made.fixture);
    ok(&["scan", "--root", root, "--index", index, "--no-rules"]);
    let (code, _, err) = cli(&args(&["clean", "--index", index, "--root", root]));
    assert_eq!(
        (code, err.as_str()),
        (1, "error: the picture was scanned without rules\n")
    );
    ok(&["scan", "--root", root, "--index", index, "--home", "/"]);
    let (code, _, err) = cli(&args(&["clean", "--index", index, "--root", root]));
    assert_eq!(
        (code, err.as_str()),
        (1, "error: the home directory must lie inside the fixture\n")
    );
}

#[test]
fn an_index_without_a_picture_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let index = dir.path().join("index.sqlite");
    let index = index.to_str().unwrap();
    ok(&["exclude", "add", "--index", index, "/x"]);
    let (code, _, err) = cli(&args(&["clean", "--index", index, "--root", "/"]));
    assert_eq!(
        (code, err.as_str()),
        (1, "error: the index holds no picture: scan first\n")
    );
}

#[test]
fn a_fixture_is_made_only_in_a_new_or_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("precious"), "keep").unwrap();
    let (code, _, err) = cli(&args(&["fixture", "create", dir.path().to_str().unwrap()]));
    assert_eq!(code, 1);
    assert!(err.ends_with(" is not empty\n"), "{err}");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn the_cli_never_holds_the_production_capability() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in fs::read_dir(src).unwrap() {
        let source = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!source.contains("Production"));
        assert!(!source.contains("release_app"));
    }
}
