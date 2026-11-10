//! Folders as the app lists them. A folder's page: its subdirectories
//! from the picture and its files listed now, largest first; a directory
//! may hold a million files, so only the largest are kept (a bounded
//! heap) and the rest are counted. The Overview's largest folders.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::view::{Folder, FolderEntry, TopFolder};
use crate::domain::{Bytes, DirId, DirRecord, EntryKind, LiveDir};
use crate::fs::{DirHandle, visit_dir};
use crate::index::Page;

/// The most files a folder listing keeps; the rest are one entry.
pub const LISTED_FILES: usize = 10_000;

/// Files a listing did not keep: how many, and their bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rest {
    pub files: u64,
    pub bytes: Bytes,
}

/// The `limit` largest files seen, in bounded memory; of files of equal
/// size the names that sort first are kept.
pub struct Largest {
    limit: usize,
    /// The kept files, the one a listing shows last on top.
    heap: BinaryHeap<Reverse<(Bytes, Reverse<OsString>)>>,
    rest: Rest,
}

impl Largest {
    pub fn new(limit: usize) -> Largest {
        Largest {
            limit,
            heap: BinaryHeap::new(),
            rest: Rest::default(),
        }
    }

    pub fn add(&mut self, name: OsString, bytes: Bytes) {
        self.heap.push(Reverse((bytes, Reverse(name))));
        if self.heap.len() > self.limit {
            let Some(Reverse((bytes, _))) = self.heap.pop() else {
                return;
            };
            self.rest.files += 1;
            self.rest.bytes += bytes;
        }
    }

    /// The kept files, largest first, and the rest.
    pub fn finish(self) -> (Vec<(OsString, Bytes)>, Rest) {
        let kept = self.heap.into_sorted_vec();
        let kept = kept
            .into_iter()
            .map(|Reverse((bytes, Reverse(name)))| (name, bytes));
        (kept.collect(), self.rest)
    }
}

/// The page of `dir` at `path`: `dirs` are its largest subdirectories
/// up to the page's end, `count` how many it has in all.
pub fn folder(
    dir: DirRecord,
    path: PathBuf,
    dirs: Vec<DirRecord>,
    count: usize,
    page: Page,
) -> Folder {
    let mut largest = Largest::new(LISTED_FILES);
    if let Ok(handle) = DirHandle::open(&path) {
        let _ = visit_dir(&handle, &mut |entry| {
            if entry.meta.kind != EntryKind::Dir {
                largest.add(entry.name, entry.meta.share());
            }
        });
    }
    let (files, rest) = largest.finish();
    let total = count + files.len() + usize::from(rest.files > 0);
    let files = files
        .into_iter()
        .map(|(name, bytes)| FolderEntry::File { name, bytes });
    let mut entries: Vec<FolderEntry> = dirs
        .into_iter()
        .map(|row| FolderEntry::Dir(Box::new(row)))
        .collect();
    entries.extend(files);
    entries.sort_by(|a, b| size(b).cmp(&size(a)).then_with(|| name(a).cmp(&name(b))));
    if rest.files > 0 {
        entries.push(FolderEntry::MoreFiles {
            files: rest.files,
            bytes: rest.bytes,
        });
    }
    let entries = entries
        .into_iter()
        .skip(page.offset)
        .take(page.limit)
        .collect();
    Folder {
        dir,
        path,
        entries,
        total,
    }
}

fn size(entry: &FolderEntry) -> Bytes {
    match entry {
        FolderEntry::Dir(row) => row.tree.bytes,
        FolderEntry::File { bytes, .. } | FolderEntry::MoreFiles { bytes, .. } => *bytes,
    }
}

fn name(entry: &FolderEntry) -> Option<&OsString> {
    match entry {
        FolderEntry::Dir(row) => Some(&row.name),
        FolderEntry::File { name, .. } => Some(name),
        FolderEntry::MoreFiles { .. } => None,
    }
}

/// Directories looked at for every largest folder shown.
const SAMPLE: usize = 8;

/// How many of the largest directories to look at for `limit` folders.
pub fn sample(limit: usize) -> usize {
    limit.saturating_mul(SAMPLE)
}

/// The `limit` largest of `folders` that hold none of the others: a
/// folder holding a larger or equal one shows nothing new.
pub fn tops(mut folders: Vec<TopFolder>, limit: usize) -> Vec<TopFolder> {
    folders.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let holds = |outer: &TopFolder| {
        let below =
            |inner: &TopFolder| inner.path != outer.path && inner.path.starts_with(&outer.path);
        folders.iter().any(below)
    };
    let kept: Vec<TopFolder> = folders
        .iter()
        .filter(|f| !holds(f))
        .take(limit)
        .cloned()
        .collect();
    kept
}

/// The picture's largest directories as folders.
pub fn pictured(largest: Vec<(PathBuf, DirRecord)>) -> Vec<TopFolder> {
    let folders = largest.into_iter().map(|(path, row)| TopFolder {
        path,
        bytes: row.tree.bytes,
        files: row.tree.files,
        counting: false,
    });
    folders.collect()
}

/// The live tree of a running scan as folders below `root`, still being
/// counted; the root itself is left out.
pub fn live(root: &Path, dirs: Vec<LiveDir>) -> Vec<TopFolder> {
    let links: BTreeMap<DirId, (Option<DirId>, OsString)> = dirs
        .iter()
        .map(|d| (d.id, (d.parent, d.name.clone())))
        .collect();
    let path = |mut id: DirId| {
        let mut names = Vec::new();
        while let Some((Some(parent), name)) = links.get(&id) {
            names.push(name.clone());
            id = *parent;
        }
        names
            .iter()
            .rev()
            .fold(root.to_path_buf(), |path, name| path.join(name))
    };
    let below = dirs.iter().filter(|d| d.parent.is_some());
    let folders = below.map(|d| TopFolder {
        path: path(d.id),
        bytes: d.bytes,
        files: d.files,
        counting: true,
    });
    folders.collect()
}
