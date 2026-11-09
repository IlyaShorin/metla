use super::entry::Bytes;

/// What a file is, for the Overview's "By data type" row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    Applications,
    Developer,
    Media,
    Documents,
    /// Archives and disk images.
    Archives,
    System,
    Other,
}

impl FileKind {
    /// Every kind, in a fixed order (also the stored order).
    pub const ALL: [FileKind; 7] = [
        FileKind::Applications,
        FileKind::Developer,
        FileKind::Media,
        FileKind::Documents,
        FileKind::Archives,
        FileKind::System,
        FileKind::Other,
    ];
}

/// Allocated bytes per file kind, for a whole scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KindTotals([Bytes; FileKind::ALL.len()]);

impl KindTotals {
    pub fn add(&mut self, kind: FileKind, bytes: Bytes) {
        self.0[kind as usize] += bytes;
    }

    pub fn merge(&mut self, other: &KindTotals) {
        for kind in FileKind::ALL {
            self.add(kind, other.get(kind));
        }
    }

    pub fn get(&self, kind: FileKind) -> Bytes {
        self.0[kind as usize]
    }

    pub fn total(&self) -> Bytes {
        self.0.iter().copied().sum()
    }

    /// Every kind with its bytes, in the order of [`FileKind::ALL`].
    pub fn iter(&self) -> impl Iterator<Item = (FileKind, Bytes)> + '_ {
        FileKind::ALL.into_iter().map(|kind| (kind, self.get(kind)))
    }
}
