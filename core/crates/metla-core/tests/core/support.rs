//! Synthetic trees under a temporary directory.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub struct Fixture {
    _dir: TempDir,
    root: PathBuf,
}

impl Fixture {
    pub fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        Fixture { _dir: dir, root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn dir(&self, relative: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

/// A file of `len` written (not sparse) bytes.
pub fn write_file(path: &Path, len: usize) {
    let mut file = File::create(path).unwrap();
    file.write_all(&vec![7u8; len]).unwrap();
}
