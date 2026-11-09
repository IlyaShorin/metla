//! Read-only access through consistent snapshots.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rusqlite::types::FromSql;
use rusqlite::{Connection, OptionalExtension, Params, params};

use super::codec::{COLUMNS, Row, count, gap, gap_kind, int, path_from, skip};
use super::schema::open_for_reading;
use super::{IndexError, Picture, ScanRecord};
use crate::domain::{Coverage, DirId, DirRecord, GapAt, GapKind, Skip, UnixTime};

/// Stored gap counts and skip counts.
type Counts = (Vec<(GapKind, u64)>, Vec<(Skip, u64)>);

/// A read-only connection to an index.
pub struct IndexReader {
    conn: Connection,
}

impl IndexReader {
    pub fn open(index: &Path) -> Result<IndexReader, IndexError> {
        Ok(IndexReader {
            conn: open_for_reading(index)?,
        })
    }

    /// Runs `query` against one consistent version of the index: a scan
    /// that finishes meanwhile is not seen halfway.
    pub fn read<T>(
        &mut self,
        query: impl FnOnce(&Snapshot<'_>) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        let tx = self.conn.transaction()?;
        let result = query(&Snapshot { conn: &tx });
        tx.commit()?;
        result
    }
}

/// One consistent version of the index.
pub struct Snapshot<'a> {
    conn: &'a Connection,
}

impl Snapshot<'_> {
    /// The scan metadata, or `None` if no scan was ever started.
    pub fn scan(&self) -> Result<Option<ScanRecord>, IndexError> {
        let Some(latest) = self.meta::<i64>("scan_seq")? else {
            return Ok(None);
        };
        Ok(Some(ScanRecord {
            started: UnixTime(self.meta("scan_started")?.unwrap_or_default()),
            picture: self.picture()?,
            latest,
        }))
    }

    /// The root directory of the picture.
    pub fn root(&self) -> Result<Option<DirRecord>, IndexError> {
        let sql = format!("SELECT {COLUMNS} FROM dirs WHERE parent_id IS NULL");
        Ok(self.records(&sql, [])?.pop())
    }

    /// The directory at `path`, if the picture has it.
    pub fn dir(&self, path: &Path) -> Result<Option<DirRecord>, IndexError> {
        let Some(root) = self.picture_root()? else {
            return Ok(None);
        };
        let Ok(relative) = path.strip_prefix(&root) else {
            return Ok(None);
        };
        relative
            .components()
            .try_fold(self.root()?, |current, part| match current {
                Some(dir) => self.child(dir.id, part.as_os_str()),
                None => Ok(None),
            })
    }

    /// The subdirectories of `id`, largest tree first.
    pub fn children(&self, id: DirId) -> Result<Vec<DirRecord>, IndexError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM dirs WHERE parent_id = ?1 ORDER BY tree_bytes DESC, name"
        );
        self.records(&sql, [id.0])
    }

    /// The `limit` largest directories anywhere below `id`.
    pub fn top(&self, id: DirId, limit: usize) -> Result<Vec<DirRecord>, IndexError> {
        let sql = format!(
            "WITH RECURSIVE below(id) AS (
                 SELECT id FROM dirs WHERE parent_id = ?1
                 UNION ALL
                 SELECT dirs.id FROM dirs JOIN below ON dirs.parent_id = below.id
             )
             SELECT {COLUMNS} FROM dirs WHERE id IN below
             ORDER BY tree_bytes DESC, id LIMIT ?2"
        );
        self.records(&sql, params![id.0, int(limit as u64)])
    }

    /// The full path of a directory of the picture.
    pub fn path_of(&self, id: DirId) -> Result<Option<PathBuf>, IndexError> {
        let mut names = Vec::new();
        let mut next = Some(id);
        while let Some(current) = next {
            let Some(row) = self.by_id(current)? else {
                return Ok(None);
            };
            if row.parent.is_some() {
                names.push(row.name);
            }
            next = row.parent;
        }
        let root = self.picture_root()?.unwrap_or_default();
        Ok(Some(
            names.iter().rev().fold(root, |path, name| path.join(name)),
        ))
    }

    fn by_id(&self, id: DirId) -> Result<Option<DirRecord>, IndexError> {
        let sql = format!("SELECT {COLUMNS} FROM dirs WHERE id = ?1");
        Ok(self.records(&sql, [id.0])?.pop())
    }

    fn child(&self, parent: DirId, name: &OsStr) -> Result<Option<DirRecord>, IndexError> {
        let sql = format!("SELECT {COLUMNS} FROM dirs WHERE parent_id = ?1 AND name = ?2");
        Ok(self
            .records(&sql, params![parent.0, name.as_bytes()])?
            .pop())
    }

    fn records(&self, sql: &str, params: impl Params) -> Result<Vec<DirRecord>, IndexError> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement
            .query_map(params, Row::from_sql)?
            .collect::<Result<Vec<Row>, _>>()?;
        rows.into_iter().map(Row::record).collect()
    }

    fn meta<T: FromSql>(&self, key: &str) -> Result<Option<T>, IndexError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    fn picture_root(&self) -> Result<Option<PathBuf>, IndexError> {
        Ok(self.meta::<Vec<u8>>("picture_root")?.map(path_from))
    }

    fn picture(&self) -> Result<Option<Picture>, IndexError> {
        let Some(scan) = self.meta::<i64>("picture_seq")? else {
            return Ok(None);
        };
        Ok(Some(Picture {
            root: self.picture_root()?.unwrap_or_default(),
            started: UnixTime(self.meta("picture_started")?.unwrap_or_default()),
            finished: UnixTime(self.meta("picture_finished")?.unwrap_or_default()),
            coverage: self.coverage()?,
            scan,
        }))
    }

    fn coverage(&self) -> Result<Coverage, IndexError> {
        let (gaps, skips) = self.coverage_counts()?;
        let limit = count(self.meta("gap_limit")?.unwrap_or_default());
        Ok(Coverage::restore(
            usize::try_from(limit).unwrap_or(usize::MAX),
            gaps,
            skips,
            self.top_gaps()?,
        ))
    }

    fn coverage_counts(&self) -> Result<Counts, IndexError> {
        let mut gaps = Vec::new();
        let mut skips = Vec::new();
        let sql = "SELECT code, count FROM coverage_counts";
        for (code, n) in self.query(sql, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            match skip(code) {
                Ok(skip) => skips.push((skip, count(n))),
                Err(_) => gaps.push((gap_kind(code)?, count(n))),
            }
        }
        Ok((gaps, skips))
    }

    fn top_gaps(&self) -> Result<Vec<GapAt>, IndexError> {
        let sql = "SELECT depth, path, flags FROM coverage_gaps ORDER BY rank";
        let rows = self.query(sql, |r| Ok((r.get::<_, u32>(0)?, r.get(1)?, r.get(2)?)))?;
        rows.into_iter()
            .map(|(depth, path, flags)| {
                Ok(GapAt {
                    depth,
                    path: path_from(path),
                    gap: gap(flags)?,
                })
            })
            .collect()
    }

    fn query<T>(
        &self,
        sql: &str,
        map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, IndexError> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement
            .query_map([], map)?
            .collect::<Result<Vec<T>, _>>()?;
        Ok(rows)
    }
}
