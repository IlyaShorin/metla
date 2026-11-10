//! Paths of the picture's directories, rebuilt from their parent links.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, Statement};

use super::IndexError;
use crate::domain::DirId;

/// Builds paths of the picture's directories from their parent links, with
/// one prepared statement for any number of paths.
pub struct Lineage<'c> {
    statement: Statement<'c>,
    root: PathBuf,
}

impl<'c> Lineage<'c> {
    /// Paths below `root`, the picture's root.
    pub fn new(conn: &'c Connection, root: PathBuf) -> Result<Lineage<'c>, IndexError> {
        let statement = conn.prepare("SELECT parent_id, name FROM dirs WHERE id = ?1")?;
        Ok(Lineage { statement, root })
    }

    /// The path of the directory `id`, if the picture has it.
    pub fn path(&mut self, id: DirId) -> Result<Option<PathBuf>, IndexError> {
        let mut names = Vec::new();
        let mut next = Some(id);
        while let Some(current) = next {
            let step = self
                .statement
                .query_row([current.0], |r| {
                    Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Vec<u8>>(1)?))
                })
                .optional()?;
            let Some((parent, name)) = step else {
                return Ok(None);
            };
            names.extend(parent.map(|_| name));
            next = parent.map(DirId);
        }
        let path = names.iter().rev().fold(self.root.clone(), |path, name| {
            path.join(OsStr::from_bytes(name))
        });
        Ok(Some(path))
    }
}
