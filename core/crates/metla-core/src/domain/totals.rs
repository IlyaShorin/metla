use super::entry::{Bytes, EntryMeta, UnixTime};

/// Files directly in a directory (every entry that is not a directory).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnTotals {
    pub bytes: Bytes,
    pub files: u64,
}

impl OwnTotals {
    pub fn add(&mut self, meta: &EntryMeta) {
        self.bytes += meta.share();
        self.files += 1;
    }
}

/// A directory and everything below it, the directory itself included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeTotals {
    pub bytes: Bytes,
    pub files: u64,
    pub dirs: u64,
    /// The newest modification time seen anywhere in the tree.
    pub newest: Option<UnixTime>,
}

impl TreeTotals {
    /// The directory alone, before anything inside it is counted.
    pub fn directory(mtime: UnixTime) -> TreeTotals {
        TreeTotals {
            bytes: Bytes::default(),
            files: 0,
            dirs: 1,
            newest: Some(mtime),
        }
    }

    pub fn add_own(&mut self, own: &OwnTotals, newest: Option<UnixTime>) {
        self.bytes += own.bytes;
        self.files += own.files;
        self.newest = self.newest.max(newest);
    }

    pub fn add_tree(&mut self, other: &TreeTotals) {
        self.bytes += other.bytes;
        self.files += other.files;
        self.dirs += other.dirs;
        self.newest = self.newest.max(other.newest);
    }
}
