//! A folder's page: its subdirectories from the picture and its files
//! listed now, largest first. A directory may hold a million files, so
//! only the largest are kept (a bounded heap) and the rest are counted.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ffi::OsString;
use std::path::PathBuf;

use super::view::{Folder, FolderEntry};
use crate::domain::{Bytes, DirRecord, EntryKind};
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
