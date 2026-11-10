//! Whether directories of project output are Git-ignored and hold nothing
//! tracked: one `check-ignore` and one `ls-files` batch per repository.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::porcelain::fields;
use super::run::Git;
use crate::domain::Verdict;

/// Paths per `ls-files` call, so the command line stays short.
const PATHS_PER_CALL: usize = 512;

/// The repository a directory lies in: its nearest ancestor (not itself)
/// that holds a `.git` entry.
pub fn repository_of(path: &Path) -> Option<PathBuf> {
    let mut above = path.ancestors().skip(1);
    above
        .find(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

/// Git's verdict on each directory of `paths`, in the same order: ignored
/// and holding nothing tracked is `Verified`. A directory outside any
/// repository, or in one where Git fails, is `Unverifiable`.
pub fn verify_outputs(git: &Git, paths: &[PathBuf]) -> Vec<Verdict> {
    let mut verdicts = vec![Verdict::Unverifiable; paths.len()];
    for (repository, members) in by_repository(paths) {
        let relative: Vec<&[u8]> = members.iter().map(|(_, rel)| *rel).collect();
        let judged = judge(git, &repository, &relative);
        for ((at, _), verdict) in members.iter().zip(judged) {
            verdicts[*at] = verdict;
        }
    }
    verdicts
}

type Members<'p> = Vec<(usize, &'p [u8])>;

/// The paths grouped by repository, each with its position and its path
/// relative to the repository.
fn by_repository(paths: &[PathBuf]) -> BTreeMap<PathBuf, Members<'_>> {
    let mut found: HashMap<&Path, Option<PathBuf>> = HashMap::new();
    let mut groups: BTreeMap<PathBuf, Members<'_>> = BTreeMap::new();
    for (at, path) in paths.iter().enumerate() {
        let parent = path.parent().unwrap_or(path);
        let repository = found
            .entry(parent)
            .or_insert_with(|| repository_of(path))
            .clone();
        let Some(repository) = repository else {
            continue;
        };
        let relative = path.strip_prefix(&repository).unwrap_or(path);
        let relative = relative.as_os_str().as_bytes();
        groups.entry(repository).or_default().push((at, relative));
    }
    groups
}

/// The verdicts for directories of one repository, given relative to it.
fn judge(git: &Git, repository: &Path, relative: &[&[u8]]) -> Vec<Verdict> {
    let ignored = ignored(git, repository, relative);
    let tracked = tracked(git, repository, relative);
    let (Some(ignored), Some(tracked)) = (ignored, tracked) else {
        return vec![Verdict::Unverifiable; relative.len()];
    };
    let verdict = |path: &&[u8]| match (ignored.contains(*path), tracked.contains(*path)) {
        (false, _) => Verdict::NotIgnored,
        (true, true) => Verdict::HasTrackedFiles,
        (true, false) => Verdict::Verified,
    };
    relative.iter().map(verdict).collect()
}

/// Which of the paths Git's ignore rules match (`check-ignore`: exit 0
/// when some are, 1 when none are, anything else is a failure). The index
/// is not consulted, so a directory that holds tracked files is still
/// judged by the rules; [`tracked`] finds those files.
fn ignored<'p>(git: &Git, repository: &Path, relative: &[&'p [u8]]) -> Option<HashSet<&'p [u8]>> {
    let input: Vec<u8> = relative
        .iter()
        .flat_map(|p| [p, &[0][..]])
        .flatten()
        .copied()
        .collect();
    let args = ["check-ignore", "--no-index", "--stdin", "-z"].map(OsStr::new);
    let output = git.run(repository, &args, &input).ok()?;
    let answered = matches!(output.code, 0 | 1);
    let named: HashSet<&[u8]> = fields(&output.stdout).into_iter().collect();
    let ignored = relative.iter().filter(|p| named.contains(**p));
    answered.then(|| ignored.copied().collect())
}

/// Which of the paths hold a file Git tracks.
fn tracked<'p>(git: &Git, repository: &Path, relative: &[&'p [u8]]) -> Option<HashSet<&'p [u8]>> {
    let wanted: HashSet<&[u8]> = relative.iter().copied().collect();
    let mut holding = HashSet::new();
    for chunk in relative.chunks(PATHS_PER_CALL) {
        let mut args = ["--literal-pathspecs", "ls-files", "-z", "--"]
            .map(OsStr::new)
            .to_vec();
        args.extend(chunk.iter().map(|p| OsStr::from_bytes(p)));
        let output = git.run(repository, &args, &[]).ok()?;
        if output.code != 0 {
            return None;
        }
        for file in fields(&output.stdout) {
            holding.extend(holders(file, &wanted));
        }
    }
    Some(holding)
}

/// The wanted directories that hold the tracked file `file`.
fn holders<'p>(file: &[u8], wanted: &HashSet<&'p [u8]>) -> Vec<&'p [u8]> {
    let slashes = file.iter().enumerate().filter(|(_, b)| **b == b'/');
    let dirs = slashes.map(|(at, _)| &file[..at]);
    dirs.filter_map(|dir| wanted.get(dir).copied()).collect()
}
