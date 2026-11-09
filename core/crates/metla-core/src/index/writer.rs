//! The single writer: a thread that owns the read-write connection.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use rusqlite::{Connection, params};

use super::IndexError;
use super::codec::{
    COLUMNS, gap_code, gap_flags, int, name_bytes, path_bytes, skip_code, state_columns,
};
use super::schema::{DIRS_INDEXES, dirs_table, open_for_writing};
use crate::domain::{DirRecord, ScanSummary, UnixTime};

/// Batches waiting for the writer; senders block when it falls behind.
const QUEUE_DEPTH: usize = 16;

enum Command {
    Rows(Vec<DirRecord>),
    Finish(Box<ScanSummary>, mpsc::Sender<Result<(), IndexError>>),
}

/// Writes one scan into an index. Rows go to a staging table; `finish`
/// swaps them in atomically. Dropping the writer without finishing leaves
/// the previous picture in place and the index marked incomplete.
pub struct IndexWriter {
    commands: Option<SyncSender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl IndexWriter {
    /// Opens (or creates) the index, marks a new scan as started at
    /// `started` and starts the writer thread.
    pub fn start_scan(index: &Path, started: UnixTime) -> Result<IndexWriter, IndexError> {
        let conn = open_for_writing(index)?;
        begin(&conn, started)?;
        let (commands, queue) = mpsc::sync_channel(QUEUE_DEPTH);
        let thread = thread::Builder::new()
            .name("metla-index-writer".into())
            .spawn(move || serve(conn, queue))
            .map_err(|_| IndexError::WriterGone)?;
        Ok(IndexWriter {
            commands: Some(commands),
            thread: Some(thread),
        })
    }

    /// Queues directory rows; blocks while the queue is full.
    pub fn send(&self, rows: Vec<DirRecord>) {
        if let Some(commands) = &self.commands {
            // A stopped writer reports its error from `finish`.
            let _ = commands.send(Command::Rows(rows));
        }
    }

    /// Makes the scan's rows the index's picture, in one transaction.
    pub fn finish(mut self, summary: &ScanSummary) -> Result<(), IndexError> {
        let (reply, result) = mpsc::channel();
        let commands = self.commands.take().ok_or(IndexError::WriterGone)?;
        commands
            .send(Command::Finish(Box::new(summary.clone()), reply))
            .map_err(|_| IndexError::WriterGone)?;
        result.recv().map_err(|_| IndexError::WriterGone)?
    }
}

impl Drop for IndexWriter {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(conn: Connection, queue: Receiver<Command>) {
    let mut failure = None;
    for command in queue {
        match command {
            Command::Rows(rows) => failure = failure.or_else(|| insert(&conn, &rows).err()),
            Command::Finish(summary, reply) => {
                let result = match failure.take() {
                    Some(error) => Err(error),
                    None => swap(&conn, &summary),
                };
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn set_meta(conn: &Connection, key: &str, value: impl rusqlite::ToSql) -> Result<(), IndexError> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

/// Starts a scan: a fresh staging table and a new scan number, committed
/// before any row is written.
fn begin(conn: &Connection, started: UnixTime) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("DROP TABLE IF EXISTS dirs_next;")?;
    tx.execute_batch(&dirs_table("dirs_next"))?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value)
         SELECT 'scan_seq', coalesce(max(value), 0) + 1 FROM meta WHERE key = 'scan_seq'",
        [],
    )?;
    set_meta(&tx, "scan_started", started.0)?;
    tx.commit()?;
    Ok(())
}

fn insert(conn: &Connection, rows: &[DirRecord]) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut statement = tx.prepare(&format!(
            "INSERT INTO dirs_next ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        ))?;
        for row in rows {
            let (own_bytes, own_files, flags) = state_columns(&row.state);
            statement.execute(params![
                row.id.0,
                row.parent.map(|p| p.0),
                name_bytes(&row.name),
                own_bytes,
                own_files,
                int(row.tree.bytes.0),
                int(row.tree.files),
                int(row.tree.dirs),
                row.tree.newest.map(|t| t.0),
                flags,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Replaces the picture with the staged rows and records the scan's
/// coverage, in one transaction.
fn swap(conn: &Connection, summary: &ScanSummary) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DROP TABLE dirs; ALTER TABLE dirs_next RENAME TO dirs;
         DELETE FROM coverage_counts; DELETE FROM coverage_gaps;",
    )?;
    tx.execute_batch(DIRS_INDEXES)?;
    write_coverage(&tx, summary)?;
    write_picture(&tx, summary)?;
    tx.commit()?;
    Ok(())
}

fn write_picture(conn: &Connection, summary: &ScanSummary) -> Result<(), IndexError> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value)
         SELECT 'picture_seq', value FROM meta WHERE key = 'scan_seq'",
        [],
    )?;
    set_meta(conn, "picture_root", path_bytes(&summary.root))?;
    set_meta(conn, "picture_started", summary.started.0)?;
    set_meta(conn, "picture_finished", summary.finished.0)?;
    set_meta(conn, "gap_limit", int(summary.coverage.limit() as u64))
}

fn write_coverage(conn: &Connection, summary: &ScanSummary) -> Result<(), IndexError> {
    let coverage = &summary.coverage;
    let codes = coverage
        .gap_counts()
        .map(|(kind, n)| (gap_code(kind), n))
        .chain(coverage.skip_counts().map(|(skip, n)| (skip_code(skip), n)));
    for (code, n) in codes {
        conn.execute(
            "INSERT INTO coverage_counts (code, count) VALUES (?1, ?2)",
            params![code, int(n)],
        )?;
    }
    for (rank, at) in coverage.top_gaps().iter().enumerate() {
        conn.execute(
            "INSERT INTO coverage_gaps (rank, depth, path, flags) VALUES (?1, ?2, ?3, ?4)",
            params![
                int(rank as u64),
                at.depth,
                path_bytes(&at.path),
                gap_flags(at.gap)
            ],
        )?;
    }
    Ok(())
}
