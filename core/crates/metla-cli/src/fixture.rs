//! `fixture create`: a synthetic tree marked `.metla-fixture` to scan and
//! clean by hand, and for the app's demo mode. Everything in it is made
//! up; it is written only into a directory that is new or empty.

use std::ffi::OsStr;
use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use metla_core::clean::FIXTURE_MARKER;
use metla_core::git::Git;

/// How old everything in the fixture looks, so that caches and build
/// output are preselected and worktrees are not in use.
const AGE: Duration = Duration::from_secs(60 * 24 * 3600);

/// Files of the fake home and their sizes.
const FILES: [(&str, usize); 13] = [
    ("Library/Caches/com.example.editor/cache.db", 200_000),
    ("Library/Caches/com.example.editor/images/1.bin", 80_000),
    ("Library/Caches/Homebrew/downloads/tool.tar.gz", 300_000),
    (".npm/_cacache/content-v2/sha512/aa/blob", 150_000),
    ("Library/Logs/com.example.editor/today.log", 20_000),
    ("Library/Logs/com.example.editor/yesterday.log", 20_000),
    ("Library/Logs/DiagnosticReports/editor.crash", 12_000),
    (".Trash/old-installer.dmg", 100_000),
    (".Trash/folder/notes.txt", 2_000),
    ("Documents/letter.txt", 3_000),
    ("code/app/target/debug/app", 500_000),
    ("code/app/node_modules/left-pad/index.js", 40_000),
    ("code/app/target/debug/deps/libapp.rlib", 250_000),
];

/// The project's tracked files.
const TRACKED: [(&str, &str); 5] = [
    ("Cargo.toml", "[package]\nname = \"app\"\n"),
    ("package.json", "{\"name\": \"app\"}\n"),
    ("src/main.rs", "fn main() {}\n"),
    ("README.md", "A made-up project.\n"),
    (".gitignore", "target/\nnode_modules/\n.claude/\n"),
];

/// Linked worktrees: where, and what makes each one what it is.
const WORKTREES: [(&str, State); 7] = [
    ("code/app-clean", State::Clean),
    ("code/app-dirty", State::Modified),
    ("code/app-untracked", State::Untracked),
    ("code/app-unpushed", State::Unpushed),
    ("code/app-locked", State::Locked),
    (".codex/worktrees/a1b2/app", State::Clean),
    ("code/app/.claude/worktrees/feature", State::Clean),
];

#[derive(Clone, Copy)]
enum State {
    Clean,
    Modified,
    Untracked,
    Unpushed,
    Locked,
}

/// Builds the fixture in `dir`, which must not exist or be empty; returns
/// what was made, one line each.
pub fn create(dir: &Path, git: &Git) -> Result<String, String> {
    let empty = fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none());
    if !empty {
        return Err(format!("{} is not empty", dir.display()));
    }
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let root = fs::canonicalize(dir).map_err(|e| e.to_string())?;
    build(&root, &isolated(git, &root)).map_err(|e| format!("fixture: {e}"))?;
    backdate(&root, SystemTime::now() - AGE).map_err(|e| format!("fixture: {e}"))?;
    let lines = [
        format!("fixture  {}", root.display()),
        format!("home     {}", root.join("home").display()),
        format!("files    {}", FILES.len()),
        format!("worktrees {}", WORKTREES.len()),
    ];
    Ok(lines.join("\n") + "\n")
}

/// Git that ignores the user's configuration and signs as a made-up user.
fn isolated(git: &Git, root: &Path) -> Git {
    git.clone()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", root)
        .env("GIT_AUTHOR_NAME", "Metla Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@metla.invalid")
        .env("GIT_COMMITTER_NAME", "Metla Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@metla.invalid")
}

fn build(root: &Path, git: &Git) -> Result<(), String> {
    write(&root.join(FIXTURE_MARKER), b"")?;
    let home = root.join("home");
    for (file, len) in FILES {
        write(&home.join(file), &vec![7; len])?;
    }
    project(git, &home)?;
    for (place, state) in WORKTREES {
        worktree(git, &home, &home.join(place), state)?;
    }
    Ok(())
}

/// The project's repository, committed and pushed to a bare remote.
fn project(git: &Git, home: &Path) -> Result<(), String> {
    for (file, text) in TRACKED {
        write(&home.join("code/app").join(file), text.as_bytes())?;
    }
    let remote = home.join("code/remote.git");
    fs::create_dir_all(&remote).map_err(|e| e.to_string())?;
    let remote = text(&remote);
    let steps: [(&str, &[&str]); 6] = [
        ("code/remote.git", &["init", "-q", "--bare", "-b", "main"]),
        ("code/app", &["init", "-q", "-b", "main"]),
        ("code/app", &["add", "."]),
        ("code/app", &["commit", "-q", "-m", "A made-up project"]),
        ("code/app", &["remote", "add", "origin", &remote]),
        ("code/app", &["push", "-q", "-u", "origin", "main"]),
    ];
    steps
        .iter()
        .try_for_each(|(dir, args)| run(git, &home.join(dir), args))
}

/// A linked worktree at `path` on a pushed branch of its own, with build
/// output, then put in `state`.
fn worktree(git: &Git, home: &Path, path: &Path, state: State) -> Result<(), String> {
    let app = home.join("code/app");
    let branch = path.file_name().map(text_of).unwrap_or_default();
    fs::create_dir_all(path.parent().unwrap_or(path)).map_err(|e| e.to_string())?;
    run(
        git,
        &app,
        &["worktree", "add", "-q", "-b", &branch, &text(path)],
    )?;
    run(git, path, &["push", "-q", "-u", "origin", &branch])?;
    write(&path.join("target/debug/app"), &vec![7; 120_000])?;
    put(git, &app, path, state)
}

/// Makes the worktree at `path` what `state` says.
fn put(git: &Git, app: &Path, path: &Path, state: State) -> Result<(), String> {
    match state {
        State::Clean => Ok(()),
        State::Modified => write(&path.join("README.md"), b"Changed here.\n"),
        State::Untracked => write(&path.join("notes.txt"), b"Not added yet.\n"),
        State::Unpushed => {
            write(&path.join("src/new.rs"), b"// new\n")?;
            run(git, path, &["add", "src/new.rs"])?;
            run(git, path, &["commit", "-q", "-m", "Local work"])
        }
        State::Locked => {
            let at = text(path);
            run(
                git,
                app,
                &["worktree", "lock", "--reason", "on a drive", &at],
            )
        }
    }
}

fn run(git: &Git, dir: &Path, args: &[&str]) -> Result<(), String> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let output = git.run(dir, &args, &[]).map_err(|e| e.to_string())?;
    match output.code {
        0 => Ok(()),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or(path);
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn text_of(name: &OsStr) -> String {
    name.to_string_lossy().into_owned()
}

/// Sets every file's and directory's times below `root` to `when`.
fn backdate(root: &Path, when: SystemTime) -> Result<(), String> {
    let times = FileTimes::new().set_modified(when).set_accessed(when);
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                pending.push(path.clone());
            }
            touch(&path, times)?;
        }
    }
    touch(root, times)
}

fn touch(path: &Path, times: FileTimes) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.set_times(times)
        .map_err(|e| format!("{}: {e}", path.display()))
}
