//! Linked Git worktrees: a directory whose `.git` is a file that points
//! into `<git dir>/worktrees/<name>`.

use crate::domain::{DirFacts, Probe};

/// The name of the pointer file.
pub const GIT: &str = ".git";

/// Whether the directory's `.git` is a pointer file into a repository's
/// `worktrees` directory. A submodule's pointer (`…/.git/modules/…`) and
/// anything else are not.
pub fn is_linked(facts: &DirFacts<'_>) -> bool {
    facts.probes.iter().any(points_into_worktrees)
}

fn points_into_worktrees(probe: &Probe) -> bool {
    let first_line = probe.head.split(|b| *b == b'\n').next().unwrap_or_default();
    let target = first_line.strip_prefix(b"gitdir:").unwrap_or_default();
    let mut parts = target
        .trim_ascii()
        .split(|b| *b == b'/')
        .filter(|p| !p.is_empty());
    let mut tail = [b"" as &[u8]; 3];
    for part in parts.by_ref() {
        tail = [tail[1], tail[2], part];
    }
    let [git_dir, worktrees, name] = tail;
    probe.name == GIT
        && git_dir.ends_with(b".git")
        && worktrees == b"worktrees"
        && !matches!(name, b"" | b"." | b"..")
}
