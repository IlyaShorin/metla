//! Opening an index and checking its schema version.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::IndexError;

/// Version 5 (M5): the file is made with incremental auto-vacuum, so the
/// pages a replaced picture leaves free are given back to the disk.
/// Version 4 (M4): what a directory needs to be rescanned on its own —
/// its modification time, its newest file, what it hands down (in
/// `flags` and `markers`) and its own bytes by kind — and an index of the
/// rows that were not read. Version 3 (M3) added Git's verdict on project
/// output, the directories cleaned since the scan (`stale`) and the
/// picture's home directory; version 2 (M2) directory identity, rule ids,
/// the candidate index, exclusions and bytes by file kind.
pub const SCHEMA_VERSION: i64 = 5;

const WAL_SIZE_LIMIT: i64 = 32 << 20;

/// How long a connection waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// One row per directory. `own_*` are NULL for a directory that was not
/// read (`own_mtime` also when it holds no file); `mtime` and `changed`
/// (its ctime in nanoseconds) are the directory's own; `flags` holds its state and what it hands down,
/// `markers` and `kinds` the rest (see `codec`); `rule_id` is set for a
/// candidate; `verdict` is Git's verdict on project output, NULL until it
/// is asked.
pub fn dirs_table(name: &str) -> String {
    format!(
        "CREATE TABLE {name} (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER,
            name BLOB NOT NULL,
            own_bytes INTEGER,
            own_files INTEGER,
            own_mtime INTEGER,
            tree_bytes INTEGER NOT NULL,
            tree_files INTEGER NOT NULL,
            tree_dirs INTEGER NOT NULL,
            tree_mtime INTEGER,
            mtime INTEGER NOT NULL,
            changed INTEGER NOT NULL,
            flags INTEGER NOT NULL,
            device INTEGER NOT NULL,
            inode INTEGER NOT NULL,
            rule_id TEXT,
            verdict INTEGER,
            markers BLOB,
            kinds BLOB
        );"
    )
}

/// `dirs_candidates` holds only the candidate rows and `dirs_unread` only
/// the rows of directories that were not read, so candidate and coverage
/// queries never read the whole table.
pub const DIRS_INDEXES: &str = "CREATE INDEX dirs_by_parent ON dirs(parent_id, name);
     CREATE INDEX dirs_by_size ON dirs(tree_bytes);
     CREATE INDEX dirs_candidates ON dirs(rule_id) WHERE rule_id IS NOT NULL;
     CREATE INDEX dirs_unread ON dirs(id) WHERE flags & 255 != 0;";

const OTHER_TABLES: &str = "CREATE TABLE meta (key TEXT PRIMARY KEY, value) WITHOUT ROWID;
     CREATE TABLE coverage_counts (code INTEGER PRIMARY KEY, count INTEGER NOT NULL);
     CREATE TABLE coverage_gaps (
         rank INTEGER PRIMARY KEY,
         depth INTEGER NOT NULL,
         path BLOB NOT NULL,
         flags INTEGER NOT NULL
     );
     CREATE TABLE kinds (code INTEGER PRIMARY KEY, bytes INTEGER NOT NULL);
     CREATE TABLE exclusions (path BLOB PRIMARY KEY) WITHOUT ROWID;
     CREATE TABLE stale (dir_id INTEGER PRIMARY KEY);";

/// Opens (creating if needed) an index for writing.
pub fn open_for_writing(path: &Path) -> Result<Connection, IndexError> {
    let conn = Connection::open(path)?;
    configure_writer(&conn)?;
    ensure_schema(&conn)?;
    Ok(conn)
}

fn configure_writer(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // Takes effect only in a file that has no table yet: a new index.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Automatic checkpoints never wait for readers; after one, the WAL is
    // cut back to this size instead of keeping its largest size.
    conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT)
}

fn ensure_schema(conn: &Connection) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    if needs_schema(&tx)? {
        create(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

/// Whether the file is new and needs the schema; an error for a file that
/// is not an index of this version.
fn needs_schema(conn: &Connection) -> Result<bool, IndexError> {
    match version(conn)? {
        Some(SCHEMA_VERSION) => Ok(false),
        None if is_empty(conn)? => Ok(true),
        other => Err(IndexError::UnknownSchema(other)),
    }
}

/// Opens an existing index read-only.
pub fn open_for_reading(path: &Path) -> Result<Connection, IndexError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    match version(&conn)? {
        Some(SCHEMA_VERSION) => Ok(conn),
        other => Err(IndexError::UnknownSchema(other)),
    }
}

fn create(conn: &Connection) -> Result<(), IndexError> {
    conn.execute_batch(&dirs_table("dirs"))?;
    conn.execute_batch(DIRS_INDEXES)?;
    conn.execute_batch(OTHER_TABLES)?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION],
    )?;
    Ok(())
}

fn is_empty(conn: &Connection) -> Result<bool, IndexError> {
    let objects: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    Ok(objects == 0)
}

/// The schema version, or `None` when the file has no Metla metadata.
fn version(conn: &Connection) -> Result<Option<i64>, IndexError> {
    let has_meta: bool = conn.query_row(
        "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?)
}
