//! Read-only access through consistent snapshots.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rusqlite::types::FromSql;
use rusqlite::{Connection, OptionalExtension, Params, Statement, params};

use super::codec::{COLUMNS, Row, count, gap, gap_kind, int, kind, path_from, skip};
use super::schema::open_for_reading;
use super::{IndexError, Picture, ScanRecord};
use crate::domain::{
    Bytes, Candidate, Category, Coverage, DirId, DirRecord, Exclusion, GapAt, GapKind, KindTotals,
    Recency, RuleId, Skip, Tally, UnixTime,
};

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

/// Which part of a list of candidates to return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub offset: usize,
    pub limit: usize,
}

/// A candidate row with its rule and path, before it is shown.
struct Found {
    row: DirRecord,
    rule: RuleId,
    path: PathBuf,
}

impl Found {
    fn candidate(self, recency: &Recency) -> Candidate {
        let activity = self.row.tree.newest;
        Candidate {
            id: self.row.id,
            preselected: self.rule.category().preselected(activity, recency),
            path: self.path,
            rule: self.rule,
            bytes: self.row.tree.bytes,
            files: self.row.tree.files,
            activity,
            device: self.row.device,
            inode: self.row.inode,
        }
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
        self.lineage()?.path(id)
    }

    /// The paths the user keeps, in path order.
    pub fn exclusions(&self) -> Result<Vec<Exclusion>, IndexError> {
        let sql = "SELECT path FROM exclusions ORDER BY path";
        let paths = self.query(sql, |r| r.get::<_, Vec<u8>>(0))?;
        paths
            .into_iter()
            .map(|bytes| {
                let path = path_from(bytes);
                Exclusion::new(&path)
                    .ok_or_else(|| IndexError::Corrupt(format!("exclusion {}", path.display())))
            })
            .collect()
    }

    /// Candidates no exclusion hides, counted per rule.
    pub fn rule_tallies(&self) -> Result<BTreeMap<RuleId, Tally>, IndexError> {
        let mut tallies = BTreeMap::<RuleId, Tally>::new();
        for found in self.visible(None)? {
            tallies
                .entry(found.rule)
                .or_default()
                .add(found.row.tree.bytes);
        }
        Ok(tallies)
    }

    /// Candidates no exclusion hides, counted per category; every category
    /// is present.
    pub fn category_tallies(&self) -> Result<BTreeMap<Category, Tally>, IndexError> {
        let mut tallies: BTreeMap<Category, Tally> =
            Category::ALL.map(|c| (c, Tally::default())).into();
        for (rule, tally) in self.rule_tallies()? {
            tallies.entry(rule.category()).or_default().merge(&tally);
        }
        Ok(tallies)
    }

    /// One page of the candidates of `category` (of every category when
    /// `None`) that no exclusion hides, largest first.
    pub fn candidates(
        &self,
        category: Option<Category>,
        page: Page,
        recency: &Recency,
    ) -> Result<Vec<Candidate>, IndexError> {
        let visible = self.visible(category)?.into_iter();
        let page = visible.skip(page.offset).take(page.limit);
        Ok(page.map(|found| found.candidate(recency)).collect())
    }

    /// The candidate with this id, unless it is not one or an exclusion
    /// hides it.
    pub fn candidate(&self, id: DirId, recency: &Recency) -> Result<Option<Candidate>, IndexError> {
        let row = self.by_id(id)?;
        let Some((row, rule)) = row.and_then(|row| Some((row.clone(), row.rule?))) else {
            return Ok(None);
        };
        let path = self.path_of(id)?.unwrap_or_default();
        let found = Found { row, rule, path };
        let hidden = hides(&self.exclusions()?, &found.path);
        Ok((!hidden).then(|| found.candidate(recency)))
    }

    /// The candidates of `category` (all when `None`) that no exclusion
    /// hides, largest first. Reads only candidate rows.
    fn visible(&self, category: Option<Category>) -> Result<Vec<Found>, IndexError> {
        let exclusions = self.exclusions()?;
        let sql = format!(
            "SELECT {COLUMNS} FROM dirs INDEXED BY dirs_candidates
             WHERE rule_id IS NOT NULL ORDER BY tree_bytes DESC, id"
        );
        let rows = self.records(&sql, [])?.into_iter();
        let rows = rows.filter_map(|row| Some((row.rule.clone()?, row)));
        let mut lineage = self.lineage()?;
        let mut found = Vec::new();
        for (rule, row) in rows.filter(|(rule, _)| category.is_none_or(|c| rule.category() == c)) {
            let path = lineage.path(row.id)?.unwrap_or_default();
            if !hides(&exclusions, &path) {
                found.push(Found { row, rule, path });
            }
        }
        Ok(found)
    }

    fn lineage(&self) -> Result<Lineage<'_>, IndexError> {
        Ok(Lineage {
            statement: self
                .conn
                .prepare("SELECT parent_id, name FROM dirs WHERE id = ?1")?,
            root: self.picture_root()?.unwrap_or_default(),
        })
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
            kinds: self.kinds()?,
            scan,
        }))
    }

    fn kinds(&self) -> Result<KindTotals, IndexError> {
        let mut kinds = KindTotals::default();
        let sql = "SELECT code, bytes FROM kinds";
        for (code, bytes) in self.query(sql, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            kinds.add(kind(code)?, Bytes(count(bytes)));
        }
        Ok(kinds)
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

/// Whether any exclusion hides a candidate at `path`.
fn hides(exclusions: &[Exclusion], path: &Path) -> bool {
    exclusions.iter().any(|exclusion| exclusion.hides(path))
}

/// Builds paths of the picture's directories from their parent links, with
/// one prepared statement for any number of paths.
struct Lineage<'c> {
    statement: Statement<'c>,
    root: PathBuf,
}

impl Lineage<'_> {
    fn path(&mut self, id: DirId) -> Result<Option<PathBuf>, IndexError> {
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
