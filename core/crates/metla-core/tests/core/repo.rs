//! Synthetic Git repositories under a temporary directory, made with the
//! stock `git` binary and none of the user's Git configuration.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use metla_core::domain::UnixTime;
use metla_core::git::Git;

use crate::support::Fixture;

/// The system's Git, looked up once.
fn system() -> Git {
    static SYSTEM: OnceLock<Git> = OnceLock::new();
    SYSTEM.get_or_init(Git::system).clone()
}

/// Git as the tests run it: no system or global configuration, a home
/// and configuration directory of its own, and a fixed identity.
pub fn isolated(fx: &Fixture) -> Git {
    let home = fx.dir(".git-home");
    system()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("GIT_AUTHOR_NAME", "Metla Test")
        .env("GIT_AUTHOR_EMAIL", "test@metla.invalid")
        .env("GIT_COMMITTER_NAME", "Metla Test")
        .env("GIT_COMMITTER_EMAIL", "test@metla.invalid")
}

/// Runs Git in `dir` and returns what it printed; panics when it fails.
pub fn git(git: &Git, dir: &Path, args: &[&str]) -> String {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let output = git.run(dir, &args, &[]).unwrap();
    assert_eq!(
        output.code,
        0,
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Runs Git in `dir` and returns its exit code.
pub fn git_code(git: &Git, dir: &Path, args: &[&str]) -> i32 {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git.run(dir, &args, &[]).unwrap().code
}

/// A repository `repo` with one commit (`README`, `.gitignore` ignoring
/// `target/` and `node_modules/`) whose `main` is published to `origin`.
pub fn pushed_repository(fx: &Fixture, git_: &Git) -> PathBuf {
    let repo = fx.dir("repo");
    git(git_, &repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("README"), "hello\n").unwrap();
    fs::write(repo.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
    git(git_, &repo, &["add", "README", ".gitignore"]);
    git(git_, &repo, &["commit", "-q", "-m", "first"]);
    let remote = fx.path("remote.git");
    git(
        git_,
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    publish(git_, &repo, "main");
    repo
}

/// What a push of `branch` leaves behind, without the cost of one: the
/// remote-tracking ref at HEAD and the branch's upstream set to it.
pub fn publish(git_: &Git, dir: &Path, branch: &str) {
    let tracking = format!("refs/remotes/origin/{branch}");
    git(git_, dir, &["update-ref", &tracking, "HEAD"]);
    let upstream = format!("--set-upstream-to=origin/{branch}");
    git(git_, dir, &["branch", "-q", &upstream]);
}

/// A linked worktree of `repo` at `path` on a new branch `branch`,
/// published with an upstream.
pub fn pushed_worktree(git_: &Git, repo: &Path, path: &Path, branch: &str) {
    let at = path.to_str().unwrap();
    git(git_, repo, &["worktree", "add", "-q", "-b", branch, at]);
    publish(git_, path, branch);
}

/// Commits a new file in `dir`.
pub fn commit(git_: &Git, dir: &Path, name: &str) {
    fs::write(dir.join(name), name).unwrap();
    git(git_, dir, &["add", name]);
    git(git_, dir, &["commit", "-q", "-m", name]);
}

/// Every entry below `dir` with its kind, permissions and content (or
/// link target): enough to prove that nothing changed.
pub fn tree_state(dir: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut state = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let content = if meta.is_dir() {
                pending.push(path.clone());
                Vec::new()
            } else if meta.is_symlink() {
                fs::read_link(&path)
                    .unwrap()
                    .into_os_string()
                    .into_encoded_bytes()
            } else {
                fs::read(&path).unwrap()
            };
            let relative = path.strip_prefix(dir).unwrap().to_path_buf();
            state.insert(relative, (meta.permissions().mode(), content));
        }
    }
    state
}

/// Now, as the core counts time.
pub fn now() -> UnixTime {
    let since = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    UnixTime(since.as_secs() as i64)
}

/// A year from now: everything made during the test is long idle by then.
pub fn next_year() -> UnixTime {
    UnixTime(now().0 + Duration::from_secs(365 * 24 * 3600).as_secs() as i64)
}

/// An executable shell script at `path`.
pub fn script(path: &Path, body: &str) -> PathBuf {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_path_buf()
}
