//! Read-only access through consistent snapshots.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rusqlite::types::FromSql;
use rusqlite::{Connection, OptionalExtension, Params, params};

use super::codec::{
    CANDIDATE_COLUMNS, COLUMNS, Row, count, gap, gap_kind, int, kind, path_from, skip, verdict,
};
use super::lineage::Lineage;
use super::schema::open_for_reading;
use super::writer::note_key;
use super::{IndexError, Picture, ScanRecord};
use crate::domain::{
    Bytes, Candidate, Category, Coverage, DirId, DirRecord, Exclusion, GapAt, GapKind, KindTotals,
    Manager, Recency, Reclaimable, RuleId, Skip, Tally, UnixTime, Verdict,
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

/// What a list of candidates is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// The tree size when scanned.
    Size,
    /// The newest modification time in the tree; unknown sorts as oldest.
    Activity,
    Path,
}

/// The order of a list of candidates. Ties keep the largest first, then
/// the order of their ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub key: SortKey,
    pub descending: bool,
}

impl Default for Order {
    /// Largest first.
    fn default() -> Order {
        Order {
            key: SortKey::Size,
            descending: true,
        }
    }
}

impl Order {
    fn compare(&self, a: &Found, b: &Found) -> std::cmp::Ordering {
        let ascending = match self.key {
            SortKey::Size => a.row.tree.bytes.cmp(&b.row.tree.bytes),
            SortKey::Activity => a.row.tree.newest.cmp(&b.row.tree.newest),
            SortKey::Path => a.path.cmp(&b.path),
        };
        match self.descending {
            true => ascending.reverse(),
            false => ascending,
        }
    }
}

/// A candidate row with its rule and path, before it is shown.
struct Found {
    row: DirRecord,
    rule: RuleId,
    path: PathBuf,
    managed: Option<Manager>,
}

impl Found {
    fn candidate(self, recency: &Recency) -> Candidate {
        let activity = self.row.tree.newest;
        let category = self.rule.category();
        Candidate {
            id: self.row.id,
            preselected: category.preselected(activity, recency, self.managed),
            managed: self.managed,
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

/// What decides whether a candidate row is offered: Git's verdict where
/// one is needed, the user's exclusions, and the directories cleaned since
/// the scan.
struct Offer {
    exclusions: Vec<Exclusion>,
    cleaned: Vec<PathBuf>,
    home: Option<PathBuf>,
}

impl Offer {
    /// The row as a candidate, unless it is not offered.
    fn found(&self, row: DirRecord, verdict: Option<Verdict>, path: PathBuf) -> Option<Found> {
        let rule = row.rule.clone()?;
        let offered = rule.category().offered(verdict)
            && !hides(&self.exclusions, &path)
            && !self.cleaned.iter().any(|cleaned| path.starts_with(cleaned));
        let managed = self
            .home
            .as_deref()
            .and_then(|home| Manager::of(&path, home));
        offered.then_some(Found {
            row,
            rule,
            path,
            managed,
        })
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

    /// The deepest directory of the picture on the way to `path`, with
    /// its path: `path` itself when the picture has it, else its nearest
    /// ancestor that the picture has. `None` outside the picture's root.
    pub fn nearest(&self, path: &Path) -> Result<Option<(PathBuf, DirRecord)>, IndexError> {
        let Some(root) = self.picture_root()? else {
            return Ok(None);
        };
        let (Ok(relative), Some(mut found)) = (path.strip_prefix(&root), self.root()?) else {
            return Ok(None);
        };
        let mut at = root;
        for part in relative.components() {
            let Some(child) = self.child(found.id, part.as_os_str())? else {
                break;
            };
            at.push(part);
            found = child;
        }
        Ok(Some((at, found)))
    }

    /// The value kept under `key` with `IndexEditor::set_note`.
    pub fn note(&self, key: &str) -> Result<Option<Vec<u8>>, IndexError> {
        self.meta(&note_key(key))
    }

    /// Every directory of the picture with its path, in id order.
    pub fn rows(&self) -> Result<Vec<(PathBuf, DirRecord)>, IndexError> {
        let sql = format!("SELECT {COLUMNS} FROM dirs ORDER BY id");
        let mut lineage = self.lineage()?;
        let mut rows = Vec::new();
        for row in self.records(&sql, [])? {
            rows.push((lineage.path(row.id)?.unwrap_or_default(), row));
        }
        Ok(rows)
    }

    /// The directory `id`, if the picture has it.
    pub fn record(&self, id: DirId) -> Result<Option<DirRecord>, IndexError> {
        let sql = format!("SELECT {COLUMNS} FROM dirs WHERE id = ?1");
        Ok(self.records(&sql, [id.0])?.pop())
    }

    /// The id the next new directory of the picture gets: no id is given
    /// out twice within one picture.
    pub fn next_id(&self) -> Result<DirId, IndexError> {
        Ok(DirId(self.meta("next_id")?.unwrap_or_default()))
    }

    /// The subdirectories of `id`, largest tree first.
    pub fn children(&self, id: DirId) -> Result<Vec<DirRecord>, IndexError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM dirs WHERE parent_id = ?1 ORDER BY tree_bytes DESC, name"
        );
        self.records(&sql, [id.0])
    }

    /// The `limit` largest subdirectories of `id`, largest tree first.
    pub fn largest_children(&self, id: DirId, limit: usize) -> Result<Vec<DirRecord>, IndexError> {
        self.ranked(id, limit, false)
    }

    /// The `limit` largest directories of the picture but its root, with
    /// their paths, largest first.
    pub fn largest(&self, limit: usize) -> Result<Vec<(PathBuf, DirRecord)>, IndexError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM dirs WHERE parent_id IS NOT NULL
             ORDER BY tree_bytes DESC, id LIMIT ?1"
        );
        let mut lineage = self.lineage()?;
        let mut largest = Vec::new();
        for row in self.records(&sql, [int(limit as u64)])? {
            largest.push((lineage.path(row.id)?.unwrap_or_default(), row));
        }
        Ok(largest)
    }

    /// How many subdirectories `id` has.
    pub fn child_count(&self, id: DirId) -> Result<usize, IndexError> {
        let sql = "SELECT count(*) FROM dirs WHERE parent_id = ?1";
        let count: i64 = self.conn.query_row(sql, [id.0], |r| r.get(0))?;
        Ok(count.unsigned_abs() as usize)
    }

    /// The `limit` largest directories anywhere below `id`.
    pub fn top(&self, id: DirId, limit: usize) -> Result<Vec<DirRecord>, IndexError> {
        self.ranked(id, limit, true)
    }

    /// The `limit` largest directories below `id`: anywhere in its
    /// subtree, or its children only (ties by name).
    fn ranked(&self, id: DirId, limit: usize, subtree: bool) -> Result<Vec<DirRecord>, IndexError> {
        let sql = match subtree {
            true => format!(
                "WITH RECURSIVE below(id) AS (
                     SELECT id FROM dirs WHERE parent_id = ?1
                     UNION ALL
                     SELECT dirs.id FROM dirs JOIN below ON dirs.parent_id = below.id
                 )
                 SELECT {COLUMNS} FROM dirs WHERE id IN below
                 ORDER BY tree_bytes DESC, id LIMIT ?2"
            ),
            false => format!(
                "SELECT {COLUMNS} FROM dirs WHERE parent_id = ?1
                 ORDER BY tree_bytes DESC, name LIMIT ?2"
            ),
        };
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

    /// Candidates offered (see [`Snapshot::candidates`]), counted per rule.
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

    /// Candidates offered, counted per category; every category is
    /// present. Candidates may lie inside a worktree candidate, so the
    /// categories do not add up to what can be freed: see
    /// [`Snapshot::reclaimable`].
    pub fn category_tallies(&self) -> Result<BTreeMap<Category, Tally>, IndexError> {
        let mut tallies: BTreeMap<Category, Tally> =
            Category::ALL.map(|c| (c, Tally::default())).into();
        for (rule, tally) in self.rule_tallies()? {
            tallies.entry(rule.category()).or_default().merge(&tally);
        }
        Ok(tallies)
    }

    /// What cleaning the preselected candidates would free, and what it
    /// would only move to the Trash. Worktrees are never preselected, so
    /// nothing is counted twice.
    pub fn reclaimable(&self, recency: &Recency) -> Result<Reclaimable, IndexError> {
        let mut reclaimable = Reclaimable::default();
        let found = self
            .visible(None)?
            .into_iter()
            .map(|f| f.candidate(recency));
        for candidate in found.filter(|c| c.preselected) {
            reclaimable.add(&candidate);
        }
        Ok(reclaimable)
    }

    /// Project output Git has not been asked about yet, with its path.
    pub fn unverified(&self) -> Result<Vec<(DirId, PathBuf)>, IndexError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM dirs INDEXED BY dirs_candidates
             WHERE rule_id IS NOT NULL AND verdict IS NULL ORDER BY id"
        );
        let rows = self.records(&sql, [])?;
        self.needing_verdict(rows)
    }

    /// [`Snapshot::unverified`] among the directories `ids` only.
    pub fn unverified_among(&self, ids: &[DirId]) -> Result<Vec<(DirId, PathBuf)>, IndexError> {
        let sql = format!("SELECT {COLUMNS} FROM dirs WHERE id = ?1 AND verdict IS NULL");
        let mut rows = Vec::new();
        for id in ids {
            rows.extend(self.records(&sql, [id.0])?);
        }
        self.needing_verdict(rows)
    }

    /// The project output among `rows`, with its path.
    fn needing_verdict(&self, rows: Vec<DirRecord>) -> Result<Vec<(DirId, PathBuf)>, IndexError> {
        let mut lineage = self.lineage()?;
        let mut unverified = Vec::new();
        for row in rows {
            let needs = row
                .rule
                .as_ref()
                .is_some_and(|r| r.category().needs_verdict());
            if needs {
                unverified.push((row.id, lineage.path(row.id)?.unwrap_or_default()));
            }
        }
        Ok(unverified)
    }

    /// The directories holding `path`: the nearest one the picture has
    /// and every one above it, deepest first.
    pub fn enclosing(&self, path: &Path) -> Result<Vec<DirId>, IndexError> {
        let mut ids = Vec::new();
        let mut next = self.nearest(path)?.map(|(_, row)| row.id);
        while let Some(id) = next {
            ids.push(id);
            next = self.record(id)?.and_then(|row| row.parent);
        }
        Ok(ids)
    }

    /// One page of the offered candidates of `category` (of every category
    /// when `None`), largest first. A candidate is offered unless an
    /// exclusion hides it, it lies in a directory cleaned since the scan,
    /// or it is project output Git has not verified.
    pub fn candidates(
        &self,
        category: Option<Category>,
        page: Page,
        recency: &Recency,
    ) -> Result<Vec<Candidate>, IndexError> {
        let order = Order::default();
        Ok(self.candidate_page(category, order, page, recency)?.0)
    }

    /// One page of the offered candidates of `category` in `order`, and
    /// how many are offered in all.
    pub fn candidate_page(
        &self,
        category: Option<Category>,
        order: Order,
        page: Page,
        recency: &Recency,
    ) -> Result<(Vec<Candidate>, usize), IndexError> {
        let mut visible = self.visible(category)?;
        visible.sort_by(|a, b| order.compare(a, b));
        let total = visible.len();
        let page = visible.into_iter().skip(page.offset).take(page.limit);
        Ok((page.map(|found| found.candidate(recency)).collect(), total))
    }

    /// The candidate with this id, if it is one and is offered.
    pub fn candidate(&self, id: DirId, recency: &Recency) -> Result<Option<Candidate>, IndexError> {
        Ok(self.offered_among(None, &[id], recency)?.pop())
    }

    /// The offered candidates of `category` (any when `None`) among the
    /// directories `ids`, in that order.
    pub fn offered_among(
        &self,
        category: Option<Category>,
        ids: &[DirId],
        recency: &Recency,
    ) -> Result<Vec<Candidate>, IndexError> {
        let sql = format!("SELECT {CANDIDATE_COLUMNS} FROM dirs WHERE id = ?1");
        let offer = self.offer()?;
        let mut lineage = self.lineage()?;
        let mut found = Vec::new();
        for id in ids {
            for (row, verdict) in self.candidate_rows(&sql, [id.0])? {
                if !of_category(&row, category) {
                    continue;
                }
                let path = lineage.path(row.id)?.unwrap_or_default();
                found.extend(offer.found(row, verdict, path));
            }
        }
        Ok(found.into_iter().map(|f| f.candidate(recency)).collect())
    }

    /// The offered candidates of `category` (all when `None`), largest
    /// first. Reads only candidate rows.
    fn visible(&self, category: Option<Category>) -> Result<Vec<Found>, IndexError> {
        let sql = format!(
            "SELECT {CANDIDATE_COLUMNS} FROM dirs INDEXED BY dirs_candidates
             WHERE rule_id IS NOT NULL ORDER BY tree_bytes DESC, id"
        );
        let offer = self.offer()?;
        let rows = self.candidate_rows(&sql, [])?.into_iter();
        let mut lineage = self.lineage()?;
        let mut found = Vec::new();
        for (row, verdict) in rows.filter(|(row, _)| of_category(row, category)) {
            let path = lineage.path(row.id)?.unwrap_or_default();
            found.extend(offer.found(row, verdict, path));
        }
        Ok(found)
    }

    fn offer(&self) -> Result<Offer, IndexError> {
        let mut lineage = self.lineage()?;
        let ids = self.query("SELECT dir_id FROM stale ORDER BY dir_id", |r| r.get(0))?;
        let mut cleaned = Vec::new();
        for id in ids {
            cleaned.extend(lineage.path(DirId(id))?);
        }
        Ok(Offer {
            exclusions: self.exclusions()?,
            cleaned,
            home: self.picture_home()?,
        })
    }

    fn lineage(&self) -> Result<Lineage<'_>, IndexError> {
        Lineage::new(self.conn, self.picture_root()?.unwrap_or_default())
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

    fn candidate_rows(
        &self,
        sql: &str,
        params: impl Params,
    ) -> Result<Vec<(DirRecord, Option<Verdict>)>, IndexError> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement
            .query_map(params, Row::with_verdict)?
            .collect::<Result<Vec<_>, _>>()?;
        let rows = rows.into_iter();
        rows.map(|(row, code)| Ok((row.record()?, verdict(code)?)))
            .collect()
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

    fn picture_home(&self) -> Result<Option<PathBuf>, IndexError> {
        Ok(self.meta::<Vec<u8>>("picture_home")?.map(path_from))
    }

    fn picture(&self) -> Result<Option<Picture>, IndexError> {
        let Some(scan) = self.meta::<i64>("picture_seq")? else {
            return Ok(None);
        };
        Ok(Some(Picture {
            root: self.picture_root()?.unwrap_or_default(),
            home: self.picture_home()?,
            started: UnixTime(self.meta("picture_started")?.unwrap_or_default()),
            finished: UnixTime(self.meta("picture_finished")?.unwrap_or_default()),
            updated: self.meta("picture_updated")?.map(UnixTime),
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

/// Whether the row is a candidate of `category` (of any when `None`).
fn of_category(row: &DirRecord, category: Option<Category>) -> bool {
    let rule = row.rule.as_ref().map(RuleId::category);
    category.is_none_or(|c| rule == Some(c))
}
