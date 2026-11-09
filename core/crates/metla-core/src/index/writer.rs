//! The single writer: a thread that owns the read-write connection. Scans
//! and exclusion edits are commands to it, applied in the order sent.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use rusqlite::{Connection, params};

use super::IndexError;
use super::codec::{
    COLUMNS, gap_code, gap_flags, id_bits, int, kind_code, name_bytes, path_bytes, skip_code,
    state_columns,
};
use super::schema::{DIRS_INDEXES, dirs_table, open_for_writing};
use crate::domain::{DirRecord, Exclusion, ScanSummary, UnixTime};

/// Batches waiting for the writer; senders block when it falls behind.
const QUEUE_DEPTH: usize = 16;

type Reply<T> = mpsc::Sender<Result<T, IndexError>>;

enum Command {
    Start(UnixTime, Reply<()>),
    Rows(Vec<DirRecord>),
    Finish(Box<ScanSummary>, Reply<()>),
    Exclude(Exclusion, Reply<bool>),
    Include(Exclusion, Reply<bool>),
}

/// The only writer of an index. Dropping it stops its thread after the
/// commands already sent.
pub struct IndexWriter {
    commands: Option<SyncSender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl IndexWriter {
    /// Opens (or creates) the index and starts the writer thread.
    pub fn open(index: &Path) -> Result<IndexWriter, IndexError> {
        let conn = open_for_writing(index)?;
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

    /// Marks a new scan as started at `started`. Its rows go to a staging
    /// table; the previous picture stays readable, and the index reads as
    /// incomplete until the scan finishes.
    pub fn start_scan(&mut self, started: UnixTime) -> Result<ScanWriter<'_>, IndexError> {
        self.ask(|reply| Command::Start(started, reply))?;
        Ok(ScanWriter { writer: self })
    }

    /// Keeps `exclusion` and everything beneath it out of the candidates.
    /// False when it was already excluded.
    pub fn exclude(&self, exclusion: &Exclusion) -> Result<bool, IndexError> {
        self.ask(|reply| Command::Exclude(exclusion.clone(), reply))
    }

    /// Drops an exclusion. False when there was none.
    pub fn include(&self, exclusion: &Exclusion) -> Result<bool, IndexError> {
        self.ask(|reply| Command::Include(exclusion.clone(), reply))
    }

    fn ask<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> Result<T, IndexError> {
        let (reply, answer) = mpsc::channel();
        self.send(command(reply))?;
        answer.recv().map_err(|_| IndexError::WriterGone)?
    }

    fn send(&self, command: Command) -> Result<(), IndexError> {
        let commands = self.commands.as_ref().ok_or(IndexError::WriterGone)?;
        commands.send(command).map_err(|_| IndexError::WriterGone)
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

/// One scan being written. Dropping it without [`ScanWriter::finish`]
/// leaves the previous picture in place and the index incomplete.
pub struct ScanWriter<'w> {
    writer: &'w IndexWriter,
}

impl ScanWriter<'_> {
    /// Queues directory rows; blocks while the queue is full.
    pub fn send(&self, rows: Vec<DirRecord>) {
        // A failed write is reported by `finish`.
        let _ = self.writer.send(Command::Rows(rows));
    }

    /// Makes the scan's rows the index's picture, in one transaction.
    pub fn finish(self, summary: &ScanSummary) -> Result<(), IndexError> {
        let summary = Box::new(summary.clone());
        self.writer.ask(|reply| Command::Finish(summary, reply))
    }
}

/// The writer thread's state: the connection and the first failure of the
/// scan being written.
struct Server {
    conn: Connection,
    failure: Option<IndexError>,
}

fn serve(conn: Connection, queue: Receiver<Command>) {
    let mut server = Server {
        conn,
        failure: None,
    };
    for command in queue {
        server.handle(command);
    }
}

impl Server {
    fn handle(&mut self, command: Command) {
        match command {
            Command::Start(started, reply) => answer(reply, self.start(started)),
            Command::Rows(rows) => self.insert(&rows),
            Command::Finish(summary, reply) => answer(reply, self.finish(&summary)),
            Command::Exclude(exclusion, reply) => {
                answer(reply, edit(&self.conn, EXCLUDE, &exclusion))
            }
            Command::Include(exclusion, reply) => {
                answer(reply, edit(&self.conn, INCLUDE, &exclusion))
            }
        }
    }

    fn start(&mut self, started: UnixTime) -> Result<(), IndexError> {
        self.failure = None;
        begin(&self.conn, started)
    }

    fn insert(&mut self, rows: &[DirRecord]) {
        if self.failure.is_none() {
            self.failure = insert(&self.conn, rows).err();
        }
    }

    fn finish(&mut self, summary: &ScanSummary) -> Result<(), IndexError> {
        match self.failure.take() {
            Some(error) => Err(error),
            None => swap(&self.conn, summary),
        }
    }
}

/// Sends a result back; a requester that stopped waiting needs none.
fn answer<T>(reply: Reply<T>, result: Result<T, IndexError>) {
    let _ = reply.send(result);
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
            "INSERT INTO dirs_next ({COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
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
                id_bits(row.device),
                id_bits(row.inode),
                row.rule.as_ref().map(|r| r.as_str()),
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Replaces the picture with the staged rows and records the scan's
/// coverage and kinds, in one transaction.
fn swap(conn: &Connection, summary: &ScanSummary) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DROP TABLE dirs; ALTER TABLE dirs_next RENAME TO dirs;
         DELETE FROM coverage_counts; DELETE FROM coverage_gaps; DELETE FROM kinds;",
    )?;
    tx.execute_batch(DIRS_INDEXES)?;
    write_coverage(&tx, summary)?;
    write_kinds(&tx, summary)?;
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

fn write_kinds(conn: &Connection, summary: &ScanSummary) -> Result<(), IndexError> {
    for (kind, bytes) in summary.kinds.iter() {
        conn.execute(
            "INSERT INTO kinds (code, bytes) VALUES (?1, ?2)",
            params![kind_code(kind), int(bytes.0)],
        )?;
    }
    Ok(())
}

const EXCLUDE: &str = "INSERT OR IGNORE INTO exclusions (path) VALUES (?1)";
const INCLUDE: &str = "DELETE FROM exclusions WHERE path = ?1";

/// Runs an exclusion edit; true when it changed the list.
fn edit(conn: &Connection, sql: &str, exclusion: &Exclusion) -> Result<bool, IndexError> {
    let changed = conn.execute(sql, [path_bytes(exclusion.path())])?;
    Ok(changed == 1)
}
