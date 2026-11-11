//! The scheduler: the one thread that changes the picture. It runs full
//! scans, applies the coalescer's tasks in batches, runs cleanups and the
//! work that follows each (Git's verdicts, worktree statuses, the stored
//! event id), one thing at a time, and sleeps until the coalescer's next
//! deadline or the next command — it never polls.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use super::REPORT_NOTE;
use super::kept::encode_report;
use super::profile::Profile;
use super::refresh::{RefreshError, Refreshed, refresh};
use super::shared::{Quiet, RunningScan, Shared, lock};
use super::view::{Changed, Failure, ScanReason};
use crate::clean::{CleanControl, CleanError, Context};
use crate::domain::{
    Candidate, Category, CleanupReport, Confirmation, DirId, PreviewId, QuickCategory, Recency,
    RescanTask,
};
use crate::fs::now;
use crate::git::verify_outputs;
use crate::index::{IndexError, IndexReader, IndexWriter, Page, PictureId, Snapshot};
use crate::scan::{ScanControl, ScanOptions, ScanRoot, ScanScope, mounted_below, run};
use crate::watch::{
    Coalescer, Event, Flags, FullCause, Stream, current_event_id, resume_after, volume_uuid,
};

/// Shallow directories with live counters during a full scan.
const LIVE_LIMIT: usize = 4096;
/// How long after a cleanup, beyond the stream's latency, the events
/// below what it removed are still dropped.
pub const QUIET_AFTER: Duration = Duration::from_secs(5);
/// Where the engine keeps the id of the last event it applied.
pub const EVENTS_NOTE: &str = "events";

/// Every candidate.
pub const ALL: Page = Page {
    offset: 0,
    limit: usize::MAX,
};

/// Where changes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watching {
    /// An FSEvents stream on the root, with the profile's latency.
    FsEvents,
    /// Only what `Engine::changed` reports (tests, and tools that know
    /// what they changed).
    Manual,
}

/// The stream that runs, and the latency it was made with.
pub struct Following {
    /// Held so that the stream runs; dropping it stops the stream.
    _stream: Stream,
    latency: Duration,
}

/// What API calls ask the scheduler to do.
pub enum Command {
    Events(Vec<Event>),
    Changed(Vec<PathBuf>),
    Scan,
    Profile,
    /// Someone started or stopped looking.
    Attention,
    Execute(PreviewId, Confirmation),
    Quick(Vec<QuickCategory>),
    Stop,
}

pub struct Scheduler {
    pub shared: Arc<Shared>,
    pub writer: IndexWriter,
    pub reader: IndexReader,
    pub root: ScanRoot,
    pub scope: ScanScope,
    pub coalescer: Coalescer,
    pub watching: Watching,
    pub commands: Receiver<Command>,
    pub sender: Sender<Command>,
    pub stream: Option<Following>,
    /// The event id the stream started after: where a restart resumes
    /// when no event has come yet.
    pub origin: Option<u64>,
    pub clock: Instant,
    /// When the next batch may start.
    pub next_batch: Option<Instant>,
    /// Tasks of a batch that gave way to a cleanup: due before the
    /// coalescer's next ones.
    pub left: Vec<RescanTask>,
}

impl Scheduler {
    pub fn run(mut self) {
        self.start();
        loop {
            let message = match self.wait() {
                Some(wait) => self.commands.recv_timeout(wait),
                None => self
                    .commands
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            self.shared.count(|c| c.wakeups += 1);
            match message {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.work();
        }
        self.stream = None;
    }

    /// Opening: follow changes from the stored event id when the history
    /// allows it; a full scan is needed without a picture or a history.
    /// No worktree status is computed until the list is asked for.
    fn start(&mut self) {
        self.watch();
        self.work();
    }

    /// How long to sleep: until the coalescer's next deadline (now, when
    /// tasks were left), and not before the pause after a batch; `None`
    /// when nothing waits.
    fn wait(&self) -> Option<Duration> {
        let at = match self.left.is_empty() {
            true => self.clock + self.coalescer.next_deadline()?,
            false => Instant::now(),
        };
        let at = self.next_batch.map_or(at, |next| at.max(next));
        Some(at.saturating_duration_since(Instant::now()))
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Events(events) => self.feed(&events),
            Command::Changed(paths) => {
                let events: Vec<Event> = paths.into_iter().map(reported).collect();
                self.feed(&events);
            }
            Command::Scan => self.full_scan(ScanReason::UserAsked),
            Command::Profile => self.watch(),
            Command::Attention => self.attend(),
            Command::Execute(id, confirmation) => self.clean(|cleaner, context, control, trash| {
                cleaner.execute(id, &confirmation, context, control, trash)
            }),
            Command::Quick(categories) => self.quick(&categories),
            Command::Stop => {}
        }
    }

    /// Hands the events to the coalescer, but those below what a cleanup
    /// removed; a mount or unmount below the root renews the volumes it
    /// ignores.
    fn feed(&mut self, events: &[Event]) {
        let at = self.clock.elapsed();
        let quiet = self.shared.quiet_paths(Instant::now());
        if events.iter().any(|e| e.flags.mount_changed()) {
            self.coalescer.ignore_too(mounted_below(&self.root));
        }
        for event in events.iter().filter(|e| news(e, &quiet)) {
            self.coalescer.on_event(event, at);
        }
        self.shared.count(|c| c.events += events.len() as u64);
        let pending = self.coalescer.pending() as u64;
        self.shared.update(|s| s.pending = pending);
    }

    /// Watches as the profile says (the profile changed, or the engine
    /// opened). A stream that cannot resume the stored history means a
    /// full scan is needed; a stream of another latency is replaced by
    /// one that replays what the old one had not handed over yet.
    fn watch(&mut self) {
        let profile = self.shared.profile();
        self.coalescer.set_timing(profile.timing);
        let watching = profile.timing.is_some();
        self.shared.update(|s| s.watching = watching);
        let wanted = (watching && self.watching == Watching::FsEvents).then_some(profile.latency);
        let running = self.stream.as_ref().map(|following| following.latency);
        match (wanted, running) {
            (None, _) => self.stream = None,
            (Some(latency), Some(now)) if latency == now => {}
            (Some(latency), Some(_)) => {
                let since = self.coalescer.settled().or(self.origin);
                self.stream = None;
                self.open_stream(latency, since);
            }
            (Some(latency), None) => {
                let since = resumable(&mut self.reader, self.root.device());
                if !self.open_stream(latency, since) {
                    let reason = match self.picture().ok().flatten() {
                        Some(_) => ScanReason::HistoryLost,
                        None => ScanReason::NoPicture,
                    };
                    self.shared.need_full_scan(reason);
                }
            }
        }
    }

    /// Follows changes as the profile in force says; when someone starts
    /// looking, what waits is due at once.
    fn attend(&mut self) {
        self.watch();
        if lock(&self.shared.status).attention {
            self.coalescer.expedite();
            self.next_batch = None;
        }
    }

    /// Starts the stream after the event `since` (`None`: from now on);
    /// true when it replays from there.
    /// Events below what a cleanup removes are dropped as they come, so
    /// a large cleanup does not queue them by the hundred thousand.
    fn open_stream(&mut self, latency: Duration, since: Option<u64>) -> bool {
        let sender = self.sender.clone();
        let shared = Arc::clone(&self.shared);
        let sink = move |mut events: Vec<Event>| {
            let quiet = shared.quiet_paths(Instant::now());
            events.retain(|event| news(event, &quiet));
            if !events.is_empty() {
                let _ = sender.send(Command::Events(events));
            }
        };
        self.origin = Some(since.unwrap_or_else(current_event_id));
        let stream = Stream::start(self.root.path(), since, latency, sink).ok();
        self.shared
            .count(|c| c.streams += u64::from(stream.is_some()));
        self.stream = stream.map(|stream| Following {
            _stream: stream,
            latency,
        });
        since.and(self.stream.as_ref()).is_some()
    }

    /// Stores the id after which a restarted stream must resume.
    fn mark_events(&mut self) {
        let settled = self.coalescer.settled().or(self.origin);
        let (Some(_), Some(id)) = (&self.stream, settled) else {
            return;
        };
        let Some(uuid) = volume_uuid(self.root.device()) else {
            return;
        };
        let _ = self
            .writer
            .set_note(EVENTS_NOTE, format!("{id} {uuid}").as_bytes());
    }

    fn picture(&mut self) -> Result<Option<PictureId>, IndexError> {
        let root = self.root.path().to_path_buf();
        self.reader.read(|snap| {
            let picture = snap.scan()?.and_then(|scan| scan.picture);
            Ok(picture.filter(|p| p.root == root).map(|p| p.id()))
        })
    }

    /// Options for a scan with `profile`'s threads and priority.
    fn options(&self, profile: &Profile) -> ScanOptions {
        let options = ScanOptions::new(self.root.clone(), self.scope.clone())
            .rules(Arc::clone(&self.shared.rules))
            .threads(profile.threads)
            .priority(profile.priority());
        match &self.shared.dirs {
            Some(dirs) => options.observer(Arc::clone(dirs)),
            None => options,
        }
    }

    /// What is due: a full scan the profile allows, then one batch;
    /// nothing while a cleanup waits to start.
    fn work(&mut self) {
        if self.shared.cleanup_waiting() {
            return;
        }
        let profile = self.shared.profile();
        self.full_scan_if_due(&profile);
        let waiting = self
            .next_batch
            .map(|next| next.saturating_duration_since(Instant::now()));
        if waiting.is_some_and(|left| !left.is_zero()) {
            return;
        }
        let tasks = match self.left.is_empty() {
            true => self.coalescer.due(self.clock.elapsed()),
            false => std::mem::take(&mut self.left),
        };
        if !tasks.is_empty() {
            self.batch(&tasks, &profile);
            self.next_batch = Some(Instant::now() + profile.pause);
        }
        self.mark_events();
    }

    /// Applies one batch; the status keeps its pending count until the
    /// batch is applied, so the engine reads as updating meanwhile. The
    /// batch is counted only once the status shows it. A cleanup asked
    /// for stops it before its next task; the rest stays due.
    fn batch(&mut self, tasks: &[RescanTask], profile: &Profile) {
        let options = self.options(profile);
        let mut needed = None;
        let mut written = Written::default();
        for (at, task) in tasks.iter().enumerate() {
            if self.shared.cleanup_waiting() {
                self.left = tasks[at..].to_vec();
                break;
            }
            needed = needed.or(self.apply(task, &options, &mut written));
        }
        self.after_change(&written);
        let pending = (self.coalescer.pending() + self.left.len()) as u64;
        self.shared.update(|s| s.pending = pending);
        if let Some(reason) = needed {
            self.shared.need_full_scan(reason);
        }
        let applied = tasks.len() - self.left.len();
        self.shared.count(|c| {
            c.batches += 1;
            c.tasks += applied as u64;
        });
        let now = self.shared.profile();
        self.full_scan_if_due(&now);
    }

    /// The engine's own full scan, when one is needed, not held, the
    /// profile allows it and no cleanup waits.
    fn full_scan_if_due(&mut self, profile: &Profile) {
        let status = *lock(&self.shared.status);
        let due = status.scan_due() && profile.full_scans && status.waiting_cleanups == 0;
        if let Some(reason) = status.reason.filter(|_| due) {
            self.full_scan(reason);
        }
    }

    /// Applies one task and notes what it wrote; why only a full scan
    /// will do, when it will.
    fn apply(
        &mut self,
        task: &RescanTask,
        options: &ScanOptions,
        written: &mut Written,
    ) -> Option<ScanReason> {
        let control = ScanControl::new(usize::MIN);
        let done = refresh(
            task,
            options,
            &mut self.reader,
            &self.writer,
            &control,
            now(),
        );
        written.dirs.extend(task_path(task));
        match done {
            Ok(Refreshed::Applied { candidates }) => {
                written.candidates.extend(candidates);
                None
            }
            Ok(Refreshed::NoPicture) => Some(ScanReason::NoPicture),
            Ok(Refreshed::NeedsFullScan) => Some(needs(task, self.coalescer.full_cause())),
            Err(error) => {
                let failure = refresh_failure(error);
                self.shared.update(|s| s.failure = Some(failure));
                Some(ScanReason::RescanFailed)
            }
        }
    }

    /// After a batch or a cleanup changed the picture: Git's verdicts on
    /// new project output, worktree statuses where the change was, and
    /// the observer.
    fn after_change(&mut self, written: &Written) {
        self.verify(Some(&written.candidates));
        self.worktrees(written);
        self.shared.notify(Changed::Picture);
    }

    /// Asks Git about the project output that has no verdict yet, among
    /// the rows `among` when given.
    fn verify(&mut self, among: Option<&[DirId]>) {
        let read = self.reader.read(|snap| {
            let picture = snap.scan()?.and_then(|scan| scan.picture).map(|p| p.id());
            let unverified = match among {
                Some(ids) => snap.unverified_among(ids)?,
                None => snap.unverified()?,
            };
            Ok((picture, unverified))
        });
        let Ok((Some(picture), unverified)) = read else {
            return;
        };
        if unverified.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = unverified.iter().map(|(_, path)| path.clone()).collect();
        let verdicts = verify_outputs(&self.shared.git, &paths);
        let stored = unverified.iter().map(|(id, _)| *id).zip(verdicts).collect();
        let _ = self.writer.set_verdicts(picture, stored);
    }

    /// Recomputes, in the background, the statuses of the worktrees a
    /// change wrote or happened in, when their list was asked for.
    fn worktrees(&mut self, written: &Written) {
        let recency = self.shared.recency();
        let read = self.reader.read(|snap| {
            let picture = snap.scan()?.and_then(|scan| scan.picture).map(|p| p.id());
            Ok((picture, written.worktrees(snap, &recency)?))
        });
        if let Ok((Some(picture), chosen)) = read {
            self.shared.compute_statuses(picture, chosen);
        }
    }

    /// A full scan for `reason`: with the user's threads when they asked,
    /// else the profile's (`Interactive`'s for the first one,
    /// Profiles::full_scan). A scan cancelled while a picture exists is
    /// given up and the picture kept; without one it becomes a partial
    /// picture. A scan with a picture gives way to a cleanup asked for
    /// (Shared::want_cleanup) and is done again after it.
    fn full_scan(&mut self, reason: ScanReason) {
        let interactive = reason == ScanReason::UserAsked;
        let had_picture = self.picture().ok().flatten().is_some();
        let profile = if interactive {
            self.shared.profiles.interactive
        } else {
            let power = self.shared.effective();
            self.shared.profiles.full_scan(power, had_picture)
        };
        self.shared.update(|s| s.reason = Some(reason));
        eprintln!("{}", started_line(reason, profile.threads));
        self.watch();
        let control = Arc::new(ScanControl::new(LIVE_LIMIT));
        *lock(&self.shared.scan) = Some(RunningScan {
            control: Arc::clone(&control),
            interactive,
            yields: had_picture,
        });
        if had_picture && self.shared.cleanup_waiting() {
            control.cancel();
        }
        self.shared.notify(Changed::Activity);
        let options = self.options(&profile);
        let scanned = scan_into(&mut self.writer, &options, &control, had_picture);
        self.shared.update(|s| s.failure = scanned.err());
        let finished = scanned.unwrap_or_default();
        if finished {
            self.verify(None);
            self.mark_events();
        }
        // Counted before it stops showing as running: a reader that sees
        // no scan running sees this one counted.
        self.shared.count(|c| {
            c.full_scans += 1;
            c.scan_threads += profile.threads as u64;
        });
        *lock(&self.shared.scan) = None;
        let cancelled = control.is_cancelled();
        let gave_way = cancelled && self.shared.cleanup_waiting();
        if finished || gave_way {
            self.shared.update(|s| {
                s.needs_full_scan = cancelled;
                s.reason = Some(ScanReason::Redone).filter(|_| cancelled).or(s.reason);
            });
        }
        self.shared.notify(Changed::Picture);
    }

    fn quick(&mut self, categories: &[QuickCategory]) {
        let recency = self.shared.recency();
        let offered = self
            .reader
            .read(|snap| snap.candidates(None, ALL, &recency));
        let candidates = offered.unwrap_or_default();
        self.clean(|cleaner, context, control, trash| {
            cleaner.quick_clean(categories, candidates, context, control, trash)
        });
    }

    /// Runs a cleanup (nothing else runs meanwhile), records what changed,
    /// and rescans the directories that held what was cleaned.
    fn clean(
        &mut self,
        act: impl FnOnce(
            &mut crate::clean::Cleaner,
            &Context<'_>,
            &CleanControl,
            &dyn crate::clean::TrashHost,
        ) -> Result<CleanupReport, CleanError>,
    ) {
        let control = Arc::new(CleanControl::new());
        *lock(&self.shared.cleanup) = Some(Arc::clone(&control));
        self.shared.update(|s| {
            s.cleaning = true;
            s.waiting_cleanups = s.waiting_cleanups.saturating_sub(1);
        });
        let picture = self.picture().ok().flatten();
        let exclusions = self
            .reader
            .read(|snap| snap.exclusions())
            .unwrap_or_default();
        let context = Context {
            now: now(),
            exclusions: &exclusions,
        };
        let trash = Arc::clone(&self.shared.trash);
        let result = match &mut *lock(&self.shared.cleaner) {
            Ok(cleaner) => act(cleaner, &context, &control, &*trash),
            Err(why) => Err(why.clone()),
        };
        if let Ok(report) = &result {
            let _ = self.writer.set_note(REPORT_NOTE, &encode_report(report));
        }
        if let (Ok(report), Some(picture)) = (&result, picture) {
            self.cleaned(picture, report);
        }
        *lock(&self.shared.report) = Some(result);
        let latency = self.stream.as_ref().map_or(Duration::ZERO, |s| s.latency);
        let mut quiet = lock(&self.shared.quiet);
        quiet_after(&mut quiet, control.begun(), Instant::now(), latency);
        drop(quiet);
        *lock(&self.shared.cleanup) = None;
        self.shared.update(|s| s.cleaning = false);
        self.shared.notify(Changed::Cleanup);
    }

    /// Marks what a cleanup changed, walks again what it emptied or
    /// removed (whatever the picture still held there) and re-reads the
    /// directories holding it, so their totals and their ancestors' are
    /// right again.
    fn cleaned(&mut self, picture: PictureId, report: &CleanupReport) {
        let changed: Vec<_> = report
            .items
            .iter()
            .filter(|i| i.outcome.changed())
            .collect();
        if changed.is_empty() {
            return;
        }
        let ids = changed.iter().map(|item| item.candidate).collect();
        let _ = self.writer.mark_cleaned(picture, ids);
        let parents: BTreeSet<PathBuf> = changed
            .iter()
            .filter_map(|item| item.path.parent().map(Path::to_path_buf))
            .collect();
        let options = self.options(&self.shared.profiles.interactive);
        let mut written = Written::default();
        for item in &changed {
            let task = RescanTask::Emptied(item.path.clone());
            self.apply(&task, &options, &mut written);
        }
        for parent in parents {
            self.apply(&RescanTask::Shallow(parent), &options, &mut written);
        }
        self.after_change(&written);
    }
}

/// What a batch, or the rescans after a cleanup, wrote: the candidate
/// rows and the directories rescanned.
#[derive(Default)]
struct Written {
    candidates: Vec<DirId>,
    dirs: Vec<PathBuf>,
}

impl Written {
    /// The offered worktrees it wrote or that hold a rescanned directory.
    fn worktrees(
        &self,
        snap: &Snapshot<'_>,
        recency: &Recency,
    ) -> Result<Vec<Candidate>, IndexError> {
        let mut ids: BTreeSet<DirId> = self.candidates.iter().copied().collect();
        for dir in &self.dirs {
            ids.extend(snap.enclosing(dir)?);
        }
        let ids: Vec<DirId> = ids.into_iter().collect();
        snap.offered_among(Some(Category::GitWorktree), &ids, recency)
    }
}

/// Scans into the index; true when the scan's picture was stored. A
/// scan cancelled while `keep_old` is given up, keeping the picture; a
/// scan that fails is abandoned and says why.
fn scan_into(
    writer: &mut IndexWriter,
    options: &ScanOptions,
    control: &ScanControl,
    keep_old: bool,
) -> Result<bool, Failure> {
    let session = writer.start_scan(now()).map_err(index_failure)?;
    let summary = run(options, control, &|rows| session.send(rows));
    let give_up = keep_old && control.is_cancelled();
    match summary {
        Ok(summary) if !give_up => session.finish(&summary).map_err(index_failure)?,
        Ok(_) => session.abandon().map_err(index_failure)?,
        Err(_) => {
            let _ = session.abandon();
            return Err(Failure::Scan);
        }
    }
    Ok(!give_up)
}

fn index_failure(error: IndexError) -> Failure {
    match error {
        IndexError::NoSpace => Failure::NoSpace,
        _ => Failure::Index,
    }
}

fn refresh_failure(error: RefreshError) -> Failure {
    match error {
        RefreshError::Index(error) => index_failure(error),
        RefreshError::Scan(_) => Failure::Scan,
    }
}

/// The event id the stored history resumes after, if it can be resumed
/// on the volume `device`.
pub fn resumable(reader: &mut IndexReader, device: u64) -> Option<u64> {
    let note = reader.read(|snap| snap.note(EVENTS_NOTE)).ok()??;
    let text = String::from_utf8(note).ok()?;
    let (id, recorded) = text.split_once(' ')?;
    let stored = Some((id.parse().ok()?, recorded));
    let uuid = volume_uuid(device);
    resume_after(stored, uuid.as_deref(), current_event_id())
}

/// A change reported by hand: an event without an id of its own.
fn reported(path: PathBuf) -> Event {
    Event {
        path,
        flags: Flags::NONE,
        id: u64::MIN,
    }
}

fn task_path(task: &RescanTask) -> Option<PathBuf> {
    match task {
        RescanTask::Shallow(path) | RescanTask::Recursive(path) | RescanTask::Emptied(path) => {
            Some(path.clone())
        }
        RescanTask::Full => None,
    }
}

/// Whether `event` may say something the picture does not know: not
/// when it is below a path in `quiet` (what a cleanup removed and walks
/// again itself), unless it says the ids wrapped or the root changed.
pub fn news(event: &Event, quiet: &[PathBuf]) -> bool {
    event.flags.lost_history() || !quiet.iter().any(|path| event.path.starts_with(path))
}

/// What stays quiet after a cleanup of `removed` that ended `now`, its
/// last events due within the stream's `latency`: those paths, with the
/// last cleanup's while they are still quiet, until `QUIET_AFTER` past
/// the latency.
pub fn quiet_after(quiet: &mut Quiet, removed: Vec<PathBuf>, now: Instant, latency: Duration) {
    if quiet.until.is_none_or(|until| until <= now) {
        quiet.paths.clear();
    }
    quiet.paths.extend(removed);
    quiet.until = Some(now + latency + QUIET_AFTER);
}

/// Why only a full scan applies `task`: for the coalescer's full rescan,
/// its `cause`; for any other task, a subtree too large to walk in place.
pub fn needs(task: &RescanTask, cause: Option<FullCause>) -> ScanReason {
    match (task, cause) {
        (RescanTask::Full, Some(FullCause::Overflow)) => ScanReason::TooManyChanges,
        (RescanTask::Full, _) => ScanReason::EventsDropped,
        _ => ScanReason::RescanTooLarge,
    }
}

/// The line written to stderr when a full scan starts: its reason and
/// threads, never a path.
pub fn started_line(reason: ScanReason, threads: usize) -> String {
    format!(
        "metla: full scan started ({}), {threads} threads",
        reason.says()
    )
}
