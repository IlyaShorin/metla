//! Bringing a picture up to date in place: one rescan in one
//! transaction. Rows that went are deleted with their subtrees, rescanned
//! rows are written, and then every total that depends on them is
//! recomputed from what the table holds: the tree totals of each
//! directory above the rescanned one, the bytes by kind, and the coverage.

use rusqlite::{Connection, OptionalExtension, params};

use super::codec::{count, int, kind, kinds_of, path_from, unread};
use super::lineage::Lineage;
use super::writer::{current_picture, set_meta, write_coverage, write_kinds, write_rows};
use super::{IndexError, PictureId};
use crate::domain::{
    Bytes, Coverage, DirId, DirState, GapAt, KindTotals, OwnTotals, Rescanned, TreeTotals, UnixTime,
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
    let mut kinds = stored_kinds(&tx)?;
    for id in &change.removed {
        for (gone, own) in subtree(&tx, *id)? {
            kinds.remove(&own);
            forget(&tx, gone)?;
        }
    }
    for row in &change.rows {
        if let Some(own) = own_kinds(&tx, row.id)? {
            kinds.remove(&own);
            forget(&tx, row.id)?;
        }
        if let DirState::Read(own) = &row.state {
            kinds.merge(&own.kinds);
        }
    }
    write_rows(&tx, "INSERT INTO dirs", &change.rows)?;
    let origin = change.rows.iter().find(|r| r.id == change.origin);
    let mut above = origin.and_then(|r| r.parent);
    while let Some(id) = above {
        above = refresh(&tx, id)?;
    }
    write_kinds(&tx, &kinds)?;
    write_coverage(&tx, &coverage(&tx)?)?;
    let next = change
        .rows
        .iter()
        .map(|r| r.id.0 + 1)
        .max()
        .unwrap_or_default();
    tx.execute(
        "UPDATE meta SET value = max(value, ?1) WHERE key = 'next_id'",
        [next],
    )?;
    set_meta(&tx, "picture_updated", now.0)?;
    tx.commit()?;
    Ok(true)
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
/// by kind.
fn subtree(conn: &Connection, id: DirId) -> Result<Vec<(DirId, KindTotals)>, IndexError> {
    let mut statement = conn.prepare(
        "WITH RECURSIVE sub(id) AS (
             SELECT id FROM dirs WHERE id = ?1
             UNION ALL
             SELECT dirs.id FROM dirs JOIN sub ON dirs.parent_id = sub.id
         )
         SELECT dirs.id, dirs.kinds FROM dirs JOIN sub ON dirs.id = sub.id",
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
    let children: (i64, i64, i64, Option<i64>) = conn.query_row(
        "SELECT coalesce(sum(tree_bytes), 0), coalesce(sum(tree_files), 0),
                coalesce(sum(tree_dirs), 0), max(tree_mtime)
         FROM dirs WHERE parent_id = ?1",
        [id.0],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    tree.add_tree(&TreeTotals {
        bytes: Bytes(count(children.0)),
        files: count(children.1),
        dirs: count(children.2),
        newest: children.3.map(UnixTime),
    });
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
    Ok(parent.map(DirId))
}

/// The coverage the table's unread rows make up, as a scan of the same
/// tree would count it.
fn coverage(conn: &Connection) -> Result<Coverage, IndexError> {
    let limit: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key = 'gap_limit'", [], |r| {
            r.get(0)
        })
        .optional()?;
    let root: Option<Vec<u8>> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'picture_root'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let root = path_from(root.unwrap_or_default());
    let limit = usize::try_from(limit.unwrap_or_default()).unwrap_or_default();
    let mut coverage = Coverage::new(limit);
    let mut lineage = Lineage::new(conn, root.clone())?;
    let mut statement = conn.prepare(
        "SELECT id, flags FROM dirs INDEXED BY dirs_unread WHERE flags & 255 != 0 ORDER BY id",
    )?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (id, flags) = row?;
        match unread(flags)? {
            DirState::Skipped(skip) => coverage.record_skip(skip),
            DirState::Gap(gap) => {
                let path = lineage.path(DirId(id))?.unwrap_or_default();
                let below = path
                    .strip_prefix(&root)
                    .map_or(0, |p| p.components().count());
                let depth = u32::try_from(below).unwrap_or(u32::MAX);
                coverage.record_gap(GapAt { depth, path, gap });
            }
            DirState::Read(_) => {}
        }
    }
    Ok(coverage)
}
