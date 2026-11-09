use std::ffi::OsString;
use std::iter::Sum;
use std::num::NonZeroU64;
use std::ops::{Add, AddAssign};

/// Size of one `st_blocks` unit.
pub const BLOCK_SIZE: u64 = 512;

/// Allocated bytes (`st_blocks × 512`): an estimate of what removal frees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes(pub u64);

impl Add for Bytes {
    type Output = Bytes;
    fn add(self, other: Bytes) -> Bytes {
        Bytes(self.0 + other.0)
    }
}

impl AddAssign for Bytes {
    fn add_assign(&mut self, other: Bytes) {
        *self = *self + other;
    }
}

impl Sum for Bytes {
    fn sum<I: Iterator<Item = Bytes>>(iter: I) -> Bytes {
        iter.fold(Bytes::default(), Add::add)
    }
}

/// Seconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixTime(pub i64);

/// What a directory entry is. Symlinks are never followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// FIFO, socket, device node.
    Other,
}

/// The metadata of one directory entry, read without following symlinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryMeta {
    pub kind: EntryKind,
    /// `st_blocks`, in units of [`BLOCK_SIZE`].
    pub blocks: u64,
    pub links: NonZeroU64,
    pub device: u64,
    pub inode: u64,
    pub mtime: UnixTime,
    /// A cloud placeholder whose data is not on this disk (`SF_DATALESS`).
    pub dataless: bool,
}

impl EntryMeta {
    /// The bytes this name contributes to its directory: the allocated size
    /// divided by the number of hard links, so all links of one file add up
    /// to its allocated size. A directory's own inode contributes nothing;
    /// APFS reports no blocks for directories, and what a directory holds is
    /// counted through its entries.
    pub fn share(&self) -> Bytes {
        match self.kind {
            EntryKind::Dir => Bytes::default(),
            _ => Bytes(self.blocks * BLOCK_SIZE / self.links.get()),
        }
    }
}

/// One named entry of a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: OsString,
    pub meta: EntryMeta,
}
