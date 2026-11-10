//! Paths of the picture's directories, rebuilt from their parent links.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, Statement};

use super::IndexError;
use crate::domain::DirId;

/// Builds paths of the picture's directories from their parent links, with
/// one prepared statement for any number of paths. The paths of the
/// directories it passed are kept, so the parents shared by many paths
/// are read once.
pub struct Lineage<'c> {
    statement: Statement<'c>,
    root: PathBuf,
    known: HashMap<i64, PathBuf>,
}

impl<'c> Lineage<'c> {
    /// Paths below `root`, the picture's root.
    pub fn new(conn: &'c Connection, root: PathBuf) -> Result<Lineage<'c>, IndexError> {
        let statement = conn.prepare("SELECT parent_id, name FROM dirs WHERE id = ?1")?;
        let known = HashMap::new();
        Ok(Lineage {
            statement,
            root,
            known,
        })
    }

    /// The path of the directory `id`, if the picture has it.
    pub fn path(&mut self, id: DirId) -> Result<Option<PathBuf>, IndexError> {
        let mut walked = Vec::new();
        let mut next = Some(id.0);
        let mut base = self.root.clone();
        while let Some(current) = next {
            if let Some(path) = self.known.get(&current) {
                base = path.clone();
                break;
            }
            let step = self
                .statement
                .query_row([current], |r| {
                    Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Vec<u8>>(1)?))
                })
                .optional()?;
            let Some((parent, name)) = step else {
                return Ok(None);
            };
            walked.push((current, parent.map(|_| name)));
            next = parent;
        }
        for (dir, name) in walked.into_iter().rev() {
            base = name.map_or(base.clone(), |name| base.join(OsStr::from_bytes(&name)));
            self.known.insert(dir, base.clone());
        }
        Ok(Some(base))
    }
}
