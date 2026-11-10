//! Bringing a picture up to date in place: one rescan in one
//! transaction. Rows that went are deleted with their subtrees, rescanned
//! rows are written, and then every total that depends on them is
//! recomputed from what the table holds: the tree totals of each
//! directory above the rescanned one, the bytes by kind, and the coverage.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::codec::{count, int, kind, kinds_of, path_from, unread};
use super::lineage::Lineage;
use super::writer::{current_picture, set_meta, write_coverage, write_kinds, write_rows};
use super::{IndexError, PictureId};
use crate::domain::{
    Bytes, Coverage, DirId, DirRecord, DirState, GapAt, KindTotals, OwnTotals, Rescanned,
    TreeTotals, UnixTime,
};

/// Applies `change` to the picture `picture`; false, and nothing changed,
/// when another picture has replaced it.
pub fn apply(
    conn: &Connection,
    picture: PictureId,
    change: &Rescanned,
    now: UnixTime,
) -> Result<bool, IndexError> {
    let tx = conn.unchecked_transaction()?;
    if current_picture(&tx)? != Some(picture) {
        return Ok(false);
    }
    let kinds = replace(&tx, change)?;
    recount(&tx, change, &kinds, now)?;
    tx.commit()?;
    Ok(true)
}

/// Deletes what went and writes what was read; the bytes by kind after.
fn replace(conn: &Connection, change: &Rescanned) -> Result<KindTotals, IndexError> {
    let mut kinds = stored_kinds(conn)?;
    for id in &change.removed {
        remove_subtree(conn, *id, &mut kinds)?;
    }
    for row in &change.rows {
        swap_kinds(conn, row, &mut kinds)?;
    }
    write_rows(conn, "INSERT INTO dirs", &change.rows)?;
    Ok(kinds)
}

/// Forgets what the picture holds for `row` and counts the row's own
/// kinds instead of the stored ones.
fn swap_kinds(
    conn: &Connection,
    row: &DirRecord,
    kinds: &mut KindTotals,
) -> Result<(), IndexError> {
    if let Some(own) = own_kinds(conn, row.id)? {
        kinds.remove(&own);
        forget(conn, row.id)?;
    }
    if let DirState::Read(own) = &row.state {
        kinds.merge(&own.kinds);
    }
    Ok(())
}

fn remove_subtree(conn: &Connection, id: DirId, kinds: &mut KindTotals) -> Result<(), IndexError> {
    for (gone, own) in subtree(conn, id)? {
        kinds.remove(&own);
        forget(conn, gone)?;
    }
    Ok(())
}

/// Everything derived: the totals above the rescanned directory, the
/// bytes by kind, the coverage, the next id and the update time.
fn recount(
    conn: &Connection,
    change: &Rescanned,
    kinds: &KindTotals,
    now: UnixTime,
) -> Result<(), IndexError> {
    let origin = change.rows.iter().find(|r| r.id == change.origin);
    let mut above = origin.and_then(|r| r.parent);
    while let Some(id) = above {
        above = refresh(conn, id)?;
    }
    write_kinds(conn, kinds)?;
    write_coverage(conn, &coverage(conn)?)?;
    let next = change.rows.iter().map(|r| r.id.0 + 1).max();
    conn.execute(
        "UPDATE meta SET value = max(value, ?1) WHERE key = 'next_id'",
        [next.unwrap_or_default()],
    )?;
    set_meta(conn, "picture_updated", now.0)
}

fn stored_kinds(conn: &Connection) -> Result<KindTotals, IndexError> {
    let mut kinds = KindTotals::default();
    let mut statement = conn.prepare("SELECT code, bytes FROM kinds")?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (code, bytes) = row?;
        kinds.add(kind(code)?, Bytes(count(bytes)));
    }
    Ok(kinds)
}

/// The directory `id` and every directory below it, with their own bytes
/// by kind. `CROSS JOIN` keeps SQLite from walking the whole table and
/// probing the subtree: it walks the subtree and looks each row up.
fn subtree(conn: &Connection, id: DirId) -> Result<Vec<(DirId, KindTotals)>, IndexError> {
    let mut statement = conn.prepare(
        "WITH RECURSIVE sub(id) AS (
             SELECT id FROM dirs WHERE id = ?1
             UNION ALL
             SELECT dirs.id FROM dirs JOIN sub ON dirs.parent_id = sub.id
         )
         SELECT dirs.id, dirs.kinds FROM sub CROSS JOIN dirs ON dirs.id = sub.id",
    )?;
    let rows = statement.query_map([id.0], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let (id, kinds) = row?;
        found.push((DirId(id), kinds_of(kinds.as_deref())?));
    }
    Ok(found)
}

/// The own bytes by kind of the directory `id`, if the table has it.
fn own_kinds(conn: &Connection, id: DirId) -> Result<Option<KindTotals>, IndexError> {
    let blob: Option<Option<Vec<u8>>> = conn
        .query_row("SELECT kinds FROM dirs WHERE id = ?1", [id.0], |r| r.get(0))
        .optional()?;
    blob.map(|blob| kinds_of(blob.as_deref())).transpose()
}

/// Deletes the row `id`, and its mark as cleaned.
fn forget(conn: &Connection, id: DirId) -> Result<(), IndexError> {
    conn.execute("DELETE FROM dirs WHERE id = ?1", [id.0])?;
    conn.execute("DELETE FROM stale WHERE dir_id = ?1", [id.0])?;
    Ok(())
}

/// Recomputes the tree totals of the directory `id` from its own totals
/// and its children's tree totals; returns its parent.
fn refresh(conn: &Connection, id: DirId) -> Result<Option<DirId>, IndexError> {
    let (parent, mut tree) = own_tree(conn, id)?;
    tree.add_tree(&children_tree(conn, id)?);
    conn.execute(
        "UPDATE dirs SET tree_bytes = ?2, tree_files = ?3, tree_dirs = ?4, tree_mtime = ?5
         WHERE id = ?1",
        params![
            id.0,
            int(tree.bytes.0),
            int(tree.files),
            int(tree.dirs),
            tree.newest.map(|t| t.0)
        ],
    )?;
    Ok(parent)
}

/// The directory `id` with its own files, and its parent.
fn own_tree(conn: &Connection, id: DirId) -> Result<(Option<DirId>, TreeTotals), IndexError> {
    let (parent, bytes, files, newest, mtime): (Option<i64>, i64, i64, Option<i64>, i64) = conn
        .query_row(
            "SELECT parent_id, coalesce(own_bytes, 0), coalesce(own_files, 0), own_mtime, mtime
             FROM dirs WHERE id = ?1",
            [id.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    let mut tree = TreeTotals::directory(UnixTime(mtime));
    tree.add_own(&OwnTotals {
        bytes: Bytes(count(bytes)),
        files: count(files),
        newest: newest.map(UnixTime),
        kinds: KindTotals::default(),
    });
    Ok((parent.map(DirId), tree))
}

/// The tree totals of every child of `id` together.
fn children_tree(conn: &Connection, id: DirId) -> Result<TreeTotals, IndexError> {
    let sums: (i64, i64, i64, Option<i64>) = conn.query_row(
        "SELECT coalesce(sum(tree_bytes), 0), coalesce(sum(tree_files), 0),
                coalesce(sum(tree_dirs), 0), max(tree_mtime)
         FROM dirs WHERE parent_id = ?1",
        [id.0],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    Ok(TreeTotals {
        bytes: Bytes(count(sums.0)),
        files: count(sums.1),
        dirs: count(sums.2),
        newest: sums.3.map(UnixTime),
    })
}

/// A value of the index's own metadata.
fn meta<T: rusqlite::types::FromSql>(
    conn: &Connection,
    key: &str,
) -> Result<Option<T>, IndexError> {
    let value = conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0));
    Ok(value.optional()?)
}

/// The coverage the table's unread rows make up, as a scan of the same
/// tree would count it.
fn coverage(conn: &Connection) -> Result<Coverage, IndexError> {
    let root = path_from(meta::<Vec<u8>>(conn, "picture_root")?.unwrap_or_default());
    let limit = meta::<i64>(conn, "gap_limit")?.unwrap_or_default();
    let mut coverage = Coverage::new(usize::try_from(limit).unwrap_or_default());
    let mut lineage = Lineage::new(conn, root.clone())?;
    for (id, state) in unread_rows(conn)? {
        count_unread(&mut coverage, &mut lineage, &root, id, state)?;
    }
    Ok(coverage)
}

/// The table's unread directories, in id order.
fn unread_rows(conn: &Connection) -> Result<Vec<(DirId, DirState)>, IndexError> {
    let mut statement = conn.prepare(
        "SELECT id, flags FROM dirs INDEXED BY dirs_unread WHERE flags & 255 != 0 ORDER BY id",
    )?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, flags)| Ok((DirId(id), unread(flags)?)))
        .collect()
}

/// Counts one unread directory: a skip by kind, a gap with its depth and
/// path.
fn count_unread(
    coverage: &mut Coverage,
    lineage: &mut Lineage<'_>,
    root: &Path,
    id: DirId,
    state: DirState,
) -> Result<(), IndexError> {
    match state {
        DirState::Skipped(skip) => coverage.record_skip(skip),
        DirState::Gap(gap) => {
            let path = lineage.path(id)?.unwrap_or_default();
            let below = (path.components().count()).saturating_sub(root.components().count());
            let depth = u32::try_from(below).unwrap_or(u32::MAX);
            coverage.record_gap(GapAt { depth, path, gap });
        }
        DirState::Read(_) => {}
    }
    Ok(())
}
