//! Revalidation: everything checked about an item right before it is acted
//! on. The item is opened from the root one component at a time, without
//! following a symlink, and the opened directory is what is acted on.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path};

use crate::domain::{
    DirFacts, Entry, EntryKind, EntryMeta, Errno, Exclusion, Outcome, PreviewItem, Probe,
    SkipReason, Verdict,
};
use crate::fs::{DirHandle, ListError, list_dir};
use crate::git::{Git, verify_outputs};
use crate::rules::{PROBE_LIMIT, RuleSet};

/// An item that passed every check, opened.
pub struct Opened {
    pub parent: DirHandle,
    pub dir: DirHandle,
    pub name: OsString,
}

/// What an item is checked against.
pub struct Checker<'a> {
    pub root: &'a Path,
    pub root_dir: &'a DirHandle,
    pub rules: &'a RuleSet,
    pub git: &'a Git,
    pub exclusions: &'a [Exclusion],
}

/// The directories above an item, read on the way down.
struct Above {
    grandparent: Option<DirHandle>,
    parent: DirHandle,
    in_repo: bool,
}

type Checked<T> = Result<T, Outcome>;

fn skip(reason: SkipReason) -> Outcome {
    Outcome::Skipped(reason)
}

impl Checker<'_> {
    /// The item, opened, when everything about it still holds; otherwise
    /// what happens to it instead (`AlreadyGone` or a skip with the
    /// reason). Nothing is changed either way.
    pub fn check(&self, item: &PreviewItem) -> Checked<Opened> {
        let names = self.names(item.path())?;
        if self.exclusions.iter().any(|e| e.hides(item.path())) {
            return Err(skip(SkipReason::Excluded));
        }
        let (last, ancestors) = names.split_last().expect("at least one name");
        let above = self.descend(ancestors)?;
        let dir = open_item(&above.parent, last, item)?;
        self.matches(item, &above, &dir)?;
        self.verified(item)?;
        Ok(Opened {
            parent: above.parent,
            dir,
            name: last.to_os_string(),
        })
    }

    /// The item's path below the root, as names.
    fn names<'p>(&self, path: &'p Path) -> Checked<Vec<&'p OsStr>> {
        let relative = path
            .strip_prefix(self.root)
            .map_err(|_| skip(SkipReason::OutsideRoot))?;
        let names = relative.components().map(|part| match part {
            Component::Normal(name) => Ok(name),
            _ => Err(skip(SkipReason::OutsideRoot)),
        });
        let names = names.collect::<Checked<Vec<&OsStr>>>()?;
        match names.is_empty() {
            true => Err(skip(SkipReason::OutsideRoot)),
            false => Ok(names),
        }
    }

    /// Opens the directories from the root down to the item's parent,
    /// noting whether any of them holds `.git`.
    fn descend(&self, names: &[&OsStr]) -> Checked<Above> {
        let mut above = Above {
            grandparent: None,
            parent: self.root_dir.reopen().map_err(unreadable)?,
            in_repo: false,
        };
        for name in names {
            above.in_repo |= has_git(&above.parent);
            let meta = above.parent.entry(name).map_err(unreadable)?;
            if meta.kind != EntryKind::Dir {
                return Err(skip(SkipReason::AncestorChanged));
            }
            let child = above.parent.open_child(name).map_err(|e| match e {
                ListError::Io(Errno(libc::ELOOP)) => skip(SkipReason::AncestorChanged),
                other => unreadable(other),
            })?;
            above.grandparent = Some(std::mem::replace(&mut above.parent, child));
        }
        above.in_repo |= has_git(&above.parent);
        Ok(above)
    }

    /// Whether the item's rule still matches what is there now.
    fn matches(&self, item: &PreviewItem, above: &Above, dir: &DirHandle) -> Checked<()> {
        let entries = list_dir(dir).map_err(unreadable)?;
        let parent_markers = self.markers(Some(&above.parent))?;
        let grandparent_markers = self.markers(above.grandparent.as_ref())?;
        let probes = probes(self.rules, dir, &entries);
        let facts = DirFacts {
            path: item.path(),
            entries: &entries,
            parent_markers: &parent_markers,
            grandparent_markers: &grandparent_markers,
            inside_git_repo: above.in_repo,
            probes: &probes,
        };
        match self.rules.classify(&facts) == Some(item.rule()) {
            true => Ok(()),
            false => Err(skip(SkipReason::RuleMismatch)),
        }
    }

    /// The entries of `dir` the rules want as markers.
    fn markers(&self, dir: Option<&DirHandle>) -> Checked<Vec<OsString>> {
        let entries = dir.map(list_dir).transpose().map_err(unreadable)?;
        let names = entries.into_iter().flatten().map(|entry| entry.name);
        Ok(names.filter(|name| self.rules.is_marker(name)).collect())
    }

    /// For project output: Git still ignores it and tracks nothing in it.
    fn verified(&self, item: &PreviewItem) -> Checked<()> {
        if !item.category().needs_verdict() {
            return Ok(());
        }
        let verdict = verify_outputs(self.git, &[item.path().to_path_buf()]);
        match verdict.first() {
            Some(Verdict::Verified) => Ok(()),
            Some(Verdict::NotIgnored) => Err(skip(SkipReason::NotIgnored)),
            Some(Verdict::HasTrackedFiles) => Err(skip(SkipReason::HasTrackedFiles)),
            _ => Err(skip(SkipReason::Unverifiable)),
        }
    }
}

/// Opens the item's directory in `parent`: still a directory, not a
/// symlink, and the very directory the scan saw.
fn open_item(parent: &DirHandle, name: &OsStr, item: &PreviewItem) -> Checked<DirHandle> {
    let meta = parent.entry(name).map_err(unreadable)?;
    match meta.kind {
        EntryKind::Dir => {}
        EntryKind::Symlink => return Err(skip(SkipReason::Symlink)),
        _ => return Err(skip(SkipReason::NotADirectory)),
    }
    same(&meta, item)?;
    let dir = parent.open_child(name).map_err(unreadable)?;
    same(&dir.meta().map_err(unreadable)?, item)?;
    Ok(dir)
}

/// Whether `meta` is the directory the scan recorded for the item: on the
/// same device (another volume mounted there is not), with the same
/// inode.
fn same(meta: &EntryMeta, item: &PreviewItem) -> Checked<()> {
    if meta.device != item.candidate.device {
        return Err(skip(SkipReason::MountPoint));
    }
    match meta.inode == item.candidate.inode {
        true => Ok(()),
        false => Err(skip(SkipReason::Replaced)),
    }
}

fn has_git(dir: &DirHandle) -> bool {
    dir.entry(OsStr::new(".git")).is_ok()
}

/// The heads of the files the rules inspect, read the way the scan reads
/// them.
fn probes(rules: &RuleSet, dir: &DirHandle, entries: &[Entry]) -> Vec<Probe> {
    let wanted = rules.probes();
    let files = entries
        .iter()
        .filter(|e| e.meta.kind == EntryKind::File && !e.meta.dataless);
    let named = files.filter(|e| wanted.iter().any(|name| e.name == *name));
    named
        .filter_map(|e| {
            let head = dir.read_head(&e.name, PROBE_LIMIT).ok()?;
            Some(Probe {
                name: e.name.clone(),
                head,
            })
        })
        .collect()
}

/// A failure to read: gone means the item is gone.
fn unreadable(error: ListError) -> Outcome {
    match error {
        ListError::Gone => Outcome::AlreadyGone,
        ListError::Denied => skip(SkipReason::Unreadable(Errno(libc::EACCES))),
        ListError::Io(errno) => skip(SkipReason::Unreadable(errno)),
    }
}
