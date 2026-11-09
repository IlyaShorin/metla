//! File kinds for "By data type": where a file is decides first, its
//! extension second.
//!
//! A directory's *area* is the strongest of its parent's area and its own:
//! Developer (inside a Git repository, a developer cache or build output,
//! or `~/Library/Developer`) over Applications (inside a `.app` bundle)
//! over System (below `/System`, `/Library`, `/private`, `/usr`, `/bin`,
//! `/sbin`, `/cores`) over the user's own files. Only files in the user's
//! area are sorted by extension.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::domain::{Category, FileKind};

/// Where a directory is, as far as file kinds go. Ordered by strength.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Area {
    #[default]
    User,
    System,
    Applications,
    Developer,
}

const SYSTEM_ROOTS: [&str; 7] = [
    "/System", "/Library", "/private", "/usr", "/bin", "/sbin", "/cores",
];

const EXTENSIONS: [(FileKind, &[&str]); 3] = [
    (
        FileKind::Media,
        &[
            "aac", "aif", "aiff", "arw", "avi", "bmp", "cr2", "cr3", "dng", "flac", "gif", "heic",
            "heif", "jpeg", "jpg", "m4a", "m4v", "mkv", "mov", "mp3", "mp4", "mpeg", "mpg", "nef",
            "ogg", "opus", "png", "psd", "raw", "tif", "tiff", "wav", "webm", "webp",
        ],
    ),
    (
        FileKind::Documents,
        &[
            "csv", "doc", "docx", "epub", "key", "md", "numbers", "odp", "ods", "odt", "pages",
            "pdf", "ppt", "pptx", "rtf", "txt", "xls", "xlsx",
        ],
    ),
    (
        FileKind::Archives,
        &[
            "7z",
            "bz2",
            "dmg",
            "gz",
            "img",
            "iso",
            "pkg",
            "rar",
            "sparseimage",
            "tar",
            "tgz",
            "xip",
            "xz",
            "zip",
            "zst",
        ],
    ),
];

/// Extensions longer than this are never in the table.
const LONGEST_EXTENSION: usize = 16;

#[derive(Debug)]
pub struct Kinds {
    developer: PathBuf,
    extensions: HashMap<Vec<u8>, FileKind>,
}

impl Kinds {
    pub fn new(home: &Path) -> Kinds {
        let extensions = EXTENSIONS
            .iter()
            .flat_map(|(kind, names)| names.iter().map(|name| (name.as_bytes().to_vec(), *kind)));
        Kinds {
            developer: home.join("Library/Developer"),
            extensions: extensions.collect(),
        }
    }

    /// The area of the directory at `path`.
    pub fn area(
        &self,
        parent: Area,
        path: &Path,
        in_repo: bool,
        category: Option<Category>,
    ) -> Area {
        let developer = in_repo
            || matches!(category, Some(Category::DevCache | Category::BuildArtifact))
            || path == self.developer;
        let own = if developer {
            Area::Developer
        } else if path.as_os_str().as_bytes().ends_with(b".app") {
            Area::Applications
        } else if SYSTEM_ROOTS.iter().any(|root| path == Path::new(root)) {
            Area::System
        } else {
            Area::User
        };
        parent.max(own)
    }

    /// The area a scan root inherits from the directories above it, which
    /// the scan does not read.
    pub fn above(&self, root: &Path) -> Area {
        let mut ancestors: Vec<&Path> = root.ancestors().skip(1).collect();
        ancestors.reverse();
        ancestors
            .into_iter()
            .fold(Area::User, |area, dir| self.area(area, dir, false, None))
    }

    /// The kind of a file named `name` in a directory of `area`.
    pub fn kind(&self, area: Area, name: &OsStr) -> FileKind {
        match area {
            Area::Developer => FileKind::Developer,
            Area::Applications => FileKind::Applications,
            Area::System => FileKind::System,
            Area::User => self.by_extension(name.as_bytes()),
        }
    }

    fn by_extension(&self, name: &[u8]) -> FileKind {
        let dot = name.iter().rposition(|b| *b == b'.').filter(|at| *at > 0);
        let extension = dot.map_or(&[][..], |at| &name[at + 1..]);
        let mut lower = [0u8; LONGEST_EXTENSION];
        let lower = lower.get_mut(..extension.len()).map(|buffer| {
            buffer.copy_from_slice(extension);
            buffer.make_ascii_lowercase();
            &*buffer
        });
        let found = lower.and_then(|ext| self.extensions.get(ext));
        found.copied().unwrap_or(FileKind::Other)
    }
}
