//! The single writer: a thread that owns the read-write connection. Scans,
//! exclusion edits, Git's verdicts and cleaned directories are commands to
//! it, applied in the order sent.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use rusqlite::{Connection, OptionalExtension, Params, params};

use super::codec::{
    COLUMNS, gap_code, gap_flags, id_bits, int, kind_code, name_bytes, names_blob, path_bytes,
    skip_code, state_columns, verdict_code,
};
use super::schema::{DIRS_INDEXES, dirs_table, open_for_writing};
use super::update::apply;
use super::{IndexError, PictureId};
use crate::domain::{
    Coverage, DirId, DirRecord, Exclusion, KindTotals, Rescanned, ScanSummary, UnixTime, Verdict,
};

/// Batches waiting for the writer; senders block when it falls behind.
const QUEUE_DEPTH: usize = 16;

type Reply<T> = mpsc::Sender<Result<T, IndexError>>;

enum Command {
    Scan(Session),
    Edit(Edit),
    Stop,
}

/// The steps of one scan session, which only the [`IndexWriter`] takes.
enum Session {
    Start(UnixTime, Reply<()>),
    Rows(Vec<DirRecord>),
    Finish(Box<ScanSummary>, Reply<()>),
    Abandon(Reply<()>),
}

/// The edits any [`IndexEditor`] may ask for.
enum Edit {
    Apply(PictureId, Box<Rescanned>, UnixTime, Reply<bool>),
    Exclude(Exclusion, Reply<bool>),
    Include(Exclusion, Reply<bool>),
    Verdicts(PictureId, Vec<(DirId, Verdict)>, Reply<bool>),
    Cleaned(PictureId, Vec<DirId>, Reply<bool>),
    Note(String, Vec<u8>, Reply<()>),
}

/// The only writer of an index. Dropping it stops its thread after the
/// commands already sent. Its [`IndexEditor`] methods are available on
/// it directly.
pub struct IndexWriter {
    editor: IndexEditor,
    thread: Option<JoinHandle<()>>,
}

/// Small edits through the index's writer, from any thread: exclusions,
/// Git's verdicts, cleaned directories, in-place updates and notes. A
/// scan session needs the [`IndexWriter`] itself.
#[derive(Clone)]
pub struct IndexEditor {
    commands: SyncSender<Command>,
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
            editor: IndexEditor { commands },
            thread: Some(thread),
        })
    }

    /// Marks a new scan as started at `started`. Its rows go to a staging
    /// table; the previous picture stays readable, and the index reads as
    /// incomplete until the scan finishes.
    pub fn start_scan(&mut self, started: UnixTime) -> Result<ScanWriter<'_>, IndexError> {
        self.ask(|reply| Command::Scan(Session::Start(started, reply)))?;
        Ok(ScanWriter { writer: self })
    }

    /// A handle for edits that other threads can hold.
    pub fn editor(&self) -> IndexEditor {
        self.editor.clone()
    }
}

impl std::ops::Deref for IndexWriter {
    type Target = IndexEditor;

    fn deref(&self) -> &IndexEditor {
        &self.editor
    }
}

impl IndexEditor {
    /// Brings the picture `picture` up to date with one rescan, in one
    /// transaction: the totals above the rescanned directory, the bytes by
    /// kind and the coverage are recomputed, and `now` is recorded as the
    /// picture's last update. False, and nothing changed, when another
    /// picture has replaced it.
    pub fn apply(
        &self,
        picture: PictureId,
        change: Rescanned,
        now: UnixTime,
    ) -> Result<bool, IndexError> {
        self.request(|reply| Edit::Apply(picture, Box::new(change), now, reply))
    }

    /// Keeps `exclusion` and everything beneath it out of the candidates.
    /// False when it was already excluded.
    pub fn exclude(&self, exclusion: &Exclusion) -> Result<bool, IndexError> {
        self.request(|reply| Edit::Exclude(exclusion.clone(), reply))
    }

    /// Drops an exclusion. False when there was none.
    pub fn include(&self, exclusion: &Exclusion) -> Result<bool, IndexError> {
        self.request(|reply| Edit::Include(exclusion.clone(), reply))
    }

    /// Stores Git's verdicts on project output of the picture `picture`.
    /// False, and nothing stored, when another picture has replaced it.
    pub fn set_verdicts(
        &self,
        picture: PictureId,
        verdicts: Vec<(DirId, Verdict)>,
    ) -> Result<bool, IndexError> {
        self.request(|reply| Edit::Verdicts(picture, verdicts, reply))
    }

    /// Records that the directories `cleaned` of the picture `picture` were
    /// cleaned: until they are rescanned they and everything below them
    /// are no longer offered. False, and nothing stored, when another
    /// picture has replaced it.
    pub fn mark_cleaned(
        &self,
        picture: PictureId,
        cleaned: Vec<DirId>,
    ) -> Result<bool, IndexError> {
        self.request(|reply| Edit::Cleaned(picture, cleaned, reply))
    }

    /// Keeps `value` under `key` in a space of its own, apart from the
    /// index's metadata; read back with `Snapshot::note`.
    pub fn set_note(&self, key: &str, value: &[u8]) -> Result<(), IndexError> {
        self.request(|reply| Edit::Note(key.to_owned(), value.to_vec(), reply))
    }

    fn request<T>(&self, edit: impl FnOnce(Reply<T>) -> Edit) -> Result<T, IndexError> {
        self.ask(|reply| Command::Edit(edit(reply)))
    }

    fn ask<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> Result<T, IndexError> {
        let (reply, answer) = mpsc::channel();
        self.send(command(reply))?;
        answer.recv().map_err(|_| IndexError::WriterGone)?
    }

    fn send(&self, command: Command) -> Result<(), IndexError> {
        self.commands
            .send(command)
            .map_err(|_| IndexError::WriterGone)
    }
}

impl Drop for IndexWriter {
    fn drop(&mut self) {
        let _ = self.editor.send(Command::Stop);
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
        let _ = self.writer.send(Command::Scan(Session::Rows(rows)));
    }

    /// Makes the scan's rows the index's picture, in one transaction.
    pub fn finish(self, summary: &ScanSummary) -> Result<(), IndexError> {
        let summary = Box::new(summary.clone());
        self.writer
            .ask(|reply| Command::Scan(Session::Finish(summary, reply)))
    }

    /// Drops the scan's rows as if it had never started: the picture stays
    /// and reads as current again.
    pub fn abandon(self) -> Result<(), IndexError> {
        self.writer
            .ask(|reply| Command::Scan(Session::Abandon(reply)))
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
        match command {
            Command::Scan(step) => server.step(step),
            Command::Edit(edit) => perform(&server.conn, edit),
            Command::Stop => return,
        }
    }
}

impl Server {
    fn step(&mut self, step: Session) {
        match step {
            Session::Start(started, reply) => answer(reply, self.start(started)),
            Session::Rows(rows) => self.insert(&rows),
            Session::Finish(summary, reply) => answer(reply, self.finish(&summary)),
            Session::Abandon(reply) => answer(reply, abandon(&self.conn)),
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

fn perform(conn: &Connection, edit: Edit) {
    match edit {
        Edit::Apply(picture, change, now, reply) => {
            answer(reply, apply(conn, picture, &change, now))
        }
        Edit::Exclude(exclusion, reply) => answer(reply, exclusion_edit(conn, EXCLUDE, &exclusion)),
        Edit::Include(exclusion, reply) => answer(reply, exclusion_edit(conn, INCLUDE, &exclusion)),
        Edit::Verdicts(picture, verdicts, reply) => {
            let rows = verdicts.iter().map(|(id, v)| (id.0, verdict_code(*v)));
            answer(reply, mark(conn, picture, SET_VERDICT, rows))
        }
        Edit::Cleaned(picture, cleaned, reply) => {
            let rows = cleaned.iter().map(|id| [id.0]);
            answer(reply, mark(conn, picture, CLEANED, rows))
        }
        Edit::Note(key, value, reply) => answer(reply, set_meta(conn, &note_key(&key), value)),
    }
}

/// Where a note lives in `meta`: apart from the index's own keys.
pub(super) fn note_key(key: &str) -> String {
    format!("note:{key}")
}

/// Sends a result back; a requester that stopped waiting needs none.
fn answer<T>(reply: Reply<T>, result: Result<T, IndexError>) {
    let _ = reply.send(result);
}

pub(super) fn set_meta(
    conn: &Connection,
    key: &str,
    value: impl rusqlite::ToSql,
) -> Result<(), IndexError> {
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
    write_rows(&tx, "INSERT INTO dirs_next", rows)?;
    tx.commit()?;
    Ok(())
}

/// Writes rows with `verb` (an INSERT into a table).
pub(super) fn write_rows(
    conn: &Connection,
    verb: &str,
    rows: &[DirRecord],
) -> Result<(), IndexError> {
    let mut statement = conn.prepare(&format!(
        "{verb} ({COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
    ))?;
    for row in rows {
        let state = state_columns(&row.state, &row.passed);
        statement.execute(params![
            row.id.0,
            row.parent.map(|p| p.0),
            name_bytes(&row.name),
            state.own_bytes,
            state.own_files,
            state.own_mtime,
            int(row.tree.bytes.0),
            int(row.tree.files),
            int(row.tree.dirs),
            row.tree.newest.map(|t| t.0),
            row.mtime.0,
            row.changed.0,
            state.flags,
            id_bits(row.device),
            id_bits(row.inode),
            row.rule.as_ref().map(|r| r.as_str()),
            names_blob(&row.passed.markers),
            state.kinds,
        ])?;
    }
    Ok(())
}

/// Replaces the picture with the staged rows and records the scan's
/// coverage and kinds, in one transaction.
fn swap(conn: &Connection, summary: &ScanSummary) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DROP TABLE dirs; ALTER TABLE dirs_next RENAME TO dirs; DELETE FROM stale;
         DELETE FROM meta WHERE key IN ('picture_home', 'picture_updated');",
    )?;
    tx.execute_batch(DIRS_INDEXES)?;
    write_coverage(&tx, &summary.coverage)?;
    write_kinds(&tx, &summary.kinds)?;
    write_picture(&tx, summary)?;
    tx.commit()?;
    Ok(())
}

/// Ends a scan that is given up: no staging table, and the scan number
/// and start of the picture again.
fn abandon(conn: &Connection) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DROP TABLE IF EXISTS dirs_next;
         DELETE FROM meta WHERE key IN ('scan_seq', 'scan_started');
         INSERT INTO meta (key, value)
             SELECT 'scan_seq', value FROM meta WHERE key = 'picture_seq';
         INSERT INTO meta (key, value)
             SELECT 'scan_started', value FROM meta WHERE key = 'picture_started';",
    )?;
    tx.commit()?;
    Ok(())
}

fn write_picture(conn: &Connection, summary: &ScanSummary) -> Result<(), IndexError> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value)
         SELECT 'picture_seq', value FROM meta WHERE key = 'scan_seq'",
        [],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value)
         SELECT 'next_id', coalesce(max(id), -1) + 1 FROM dirs",
        [],
    )?;
    set_meta(conn, "picture_root", path_bytes(&summary.root))?;
    if let Some(home) = &summary.home {
        set_meta(conn, "picture_home", path_bytes(home))?;
    }
    set_meta(conn, "picture_started", summary.started.0)?;
    set_meta(conn, "picture_finished", summary.finished.0)?;
    set_meta(conn, "gap_limit", int(summary.coverage.limit() as u64))
}

/// Replaces the stored coverage: counts by kind and the shallowest gaps.
pub(super) fn write_coverage(conn: &Connection, coverage: &Coverage) -> Result<(), IndexError> {
    conn.execute_batch("DELETE FROM coverage_counts; DELETE FROM coverage_gaps;")?;
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

/// Replaces the stored bytes by kind.
pub(super) fn write_kinds(conn: &Connection, kinds: &KindTotals) -> Result<(), IndexError> {
    conn.execute("DELETE FROM kinds", [])?;
    for (kind, bytes) in kinds.iter() {
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
fn exclusion_edit(conn: &Connection, sql: &str, exclusion: &Exclusion) -> Result<bool, IndexError> {
    let changed = conn.execute(sql, [path_bytes(exclusion.path())])?;
    Ok(changed == 1)
}

const SET_VERDICT: &str = "UPDATE dirs SET verdict = ?2 WHERE id = ?1";
const CLEANED: &str = "INSERT OR IGNORE INTO stale (dir_id) VALUES (?1)";

/// The picture the index holds now, if any.
pub(super) fn current_picture(conn: &Connection) -> Result<Option<PictureId>, IndexError> {
    let current: Option<i64> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'picture_seq'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(current.map(PictureId))
}

/// Runs `sql` once per row of parameters, in one transaction, when
/// `picture` is still the index's picture; true when it was.
fn mark<P: Params>(
    conn: &Connection,
    picture: PictureId,
    sql: &str,
    rows: impl Iterator<Item = P>,
) -> Result<bool, IndexError> {
    let tx = conn.unchecked_transaction()?;
    if current_picture(&tx)? != Some(picture) {
        return Ok(false);
    }
    {
        let mut statement = tx.prepare(sql)?;
        for row in rows {
            statement.execute(row)?;
        }
    }
    tx.commit()?;
    Ok(true)
}
