//! Synthetic trees under a temporary directory.

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::symlink;
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

    /// Builds a tree from lines, relative to the root, where `~` stands for
    /// the directory `home`:
    ///
    /// * `a/b/` a directory,
    /// * `a/f` an empty file,
    /// * `a/f=text` a file holding `text`,
    /// * `a/l->target` a symlink to `target`.
    pub fn build(&self, lines: &[&str]) {
        for line in lines {
            let line = line.replacen('~', "home", 1);
            if let Some((link, target)) = line.split_once("->") {
                self.parent_of(link);
                symlink(target, self.path(link)).unwrap();
            } else if let Some(dir) = line.strip_suffix('/') {
                self.dir(dir);
            } else {
                let (file, text) = line.split_once('=').unwrap_or((&line, ""));
                self.parent_of(file);
                fs::write(self.path(file), text).unwrap();
            }
        }
    }

    /// The home directory of trees made by [`Fixture::build`].
    pub fn home(&self) -> PathBuf {
        self.path("home")
    }

    /// A path relative to the root as `build` lines spell it.
    pub fn spelled(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.root).unwrap();
        match relative.strip_prefix("home") {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => relative.display().to_string(),
        }
    }

    fn parent_of(&self, relative: &str) {
        if let Some(parent) = Path::new(relative).parent() {
            self.dir(&parent.to_string_lossy());
        }
    }
}

/// A file of `len` written (not sparse) bytes.
pub fn write_file(path: &Path, len: usize) {
    let mut file = File::create(path).unwrap();
    file.write_all(&vec![7u8; len]).unwrap();
}
