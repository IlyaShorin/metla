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

use super::profile::Profile;
use super::refresh::{RefreshError, Refreshed, refresh};
use super::shared::{RunningScan, Shared, lock};
use super::view::{Changed, Failure};
use crate::clean::{CleanControl, CleanError, Context};
use crate::domain::{
    Candidate, Category, CleanupReport, Confirmation, DirId, PreviewId, QuickCategory, Recency,
    RescanTask,
};
use crate::fs::now;
use crate::git::{WorktreeSettings, verify_outputs};
use crate::index::{IndexError, IndexReader, IndexWriter, Page, PictureId, Snapshot};
use crate::scan::{ScanControl, ScanOptions, ScanRoot, ScanScope, run};
use crate::watch::{Coalescer, Event, Flags, Stream, current_event_id, resume_after, volume_uuid};

/// Shallow directories with live counters during a full scan.
const LIVE_LIMIT: usize = 4096;
/// Where the engine keeps the id of the last event it applied.
pub const EVENTS_NOTE: &str = "events";

/// Every candidate.
const ALL: Page = Page {
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
    /// The saved picture's worktree statuses are computed in the
    /// background, since no scan may come to compute them.
    fn start(&mut self) {
        self.watch();
        self.worktrees(None);
        self.work();
    }

    /// How long to sleep: until the coalescer's next deadline, and not
    /// before the pause after a batch; `None` when nothing waits.
    fn wait(&self) -> Option<Duration> {
        let deadline = self.coalescer.next_deadline()?;
        let at = self.clock + deadline;
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
            Command::Scan => self.full_scan(true),
            Command::Profile => self.watch(),
            Command::Attention => self.attend(),
            Command::Execute(id, confirmation) => self.clean(|cleaner, context, control, trash| {
                cleaner.execute(id, &confirmation, context, control, trash)
            }),
            Command::Quick(categories) => self.quick(&categories),
            Command::Stop => {}
        }
    }

    fn feed(&mut self, events: &[Event]) {
        let at = self.clock.elapsed();
        for event in events {
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
                let resumed = self.open_stream(latency, since);
                self.shared.update(|s| s.needs_full_scan |= !resumed);
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
    fn open_stream(&mut self, latency: Duration, since: Option<u64>) -> bool {
        let sender = self.sender.clone();
        let sink = move |events| {
            let _ = sender.send(Command::Events(events));
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
        ScanOptions::new(self.root.clone(), self.scope.clone())
            .rules(Arc::clone(&self.shared.rules))
            .threads(profile.threads)
            .priority(profile.priority())
    }

    /// What is due: a full scan the profile allows, then one batch.
    fn work(&mut self) {
        let profile = self.shared.profile();
        self.full_scan_if_due(&profile);
        let waiting = self
            .next_batch
            .map(|next| next.saturating_duration_since(Instant::now()));
        if waiting.is_some_and(|left| !left.is_zero()) {
            return;
        }
        let tasks = self.coalescer.due(self.clock.elapsed());
        if !tasks.is_empty() {
            self.batch(&tasks, &profile);
            self.next_batch = Some(Instant::now() + profile.pause);
        }
        self.mark_events();
    }

    /// Applies one batch; the status keeps its pending count until the
    /// batch is applied, so the engine reads as updating meanwhile.
    fn batch(&mut self, tasks: &[RescanTask], profile: &Profile) {
        let options = self.options(profile);
        let mut full = false;
        let mut written = Written::default();
        for task in tasks {
            full |= !self.apply(task, &options, &mut written);
        }
        self.shared.count(|c| {
            c.batches += 1;
            c.tasks += tasks.len() as u64;
        });
        self.after_change(Some(&written));
        let pending = self.coalescer.pending() as u64;
        self.shared.update(|s| {
            s.pending = pending;
            s.needs_full_scan |= full;
        });
        let now = self.shared.profile();
        self.full_scan_if_due(&now);
    }

    /// The engine's own full scan, when one is needed, not held, and the
    /// profile allows it.
    fn full_scan_if_due(&mut self, profile: &Profile) {
        let status = *lock(&self.shared.status);
        if status.scan_due() && profile.full_scans {
            self.full_scan(false);
        }
    }

    /// Applies one task and notes what it wrote; false when only a full
    /// scan will do.
    fn apply(&mut self, task: &RescanTask, options: &ScanOptions, written: &mut Written) -> bool {
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
                true
            }
            Ok(_) => false,
            Err(error) => {
                let failure = refresh_failure(&error);
                self.shared.update(|s| s.failure = Some(failure));
                false
            }
        }
    }

    /// After the picture changed (`None`: a full scan wrote all of it):
    /// Git's verdicts on new project output, worktree statuses where the
    /// change was, and the observer.
    fn after_change(&mut self, written: Option<&Written>) {
        self.verify(written.map(|w| w.candidates.as_slice()));
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
    /// change wrote or happened in (all of them for `None`).
    fn worktrees(&mut self, written: Option<&Written>) {
        let recency = Recency::new(now(), lock(&self.shared.settings).recent_age);
        let read = self.reader.read(|snap| {
            let picture = snap.scan()?.and_then(|scan| scan.picture).map(|p| p.id());
            let worktrees = match written {
                Some(written) => written.worktrees(snap, &recency)?,
                None => snap.candidates(Some(Category::GitWorktree), ALL, &recency)?,
            };
            Ok((picture, worktrees))
        });
        let Ok((Some(picture), chosen)) = read else {
            return;
        };
        if chosen.is_empty() {
            return;
        }
        let settings = WorktreeSettings {
            home: self.shared.home.clone(),
            stale_after: lock(&self.shared.settings).stale_after,
        };
        (self.shared.statuses).compute(picture, chosen, now(), settings);
    }

    /// A full scan with the user's threads (`interactive`) or the
    /// profile's. A scan cancelled while a picture exists is given up and
    /// the picture kept; without one it becomes a partial picture.
    fn full_scan(&mut self, interactive: bool) {
        let profile = if interactive {
            self.shared.profiles.interactive
        } else {
            self.shared.power()
        };
        self.watch();
        let control = Arc::new(ScanControl::new(LIVE_LIMIT));
        *lock(&self.shared.scan) = Some(RunningScan {
            control: Arc::clone(&control),
            interactive,
        });
        self.shared.notify(Changed::Activity);
        let options = self.options(&profile);
        let had_picture = self.picture().ok().flatten().is_some();
        let scanned = scan_into(&mut self.writer, &options, &control, had_picture);
        self.shared.update(|s| s.failure = scanned.err());
        let finished = scanned.unwrap_or_default();
        if finished {
            self.verify(None);
            self.worktrees(None);
            self.mark_events();
        }
        *lock(&self.shared.scan) = None;
        self.shared.count(|c| c.full_scans += 1);
        let cancelled = control.is_cancelled();
        if finished {
            self.shared.update(|s| s.needs_full_scan = cancelled);
        }
        self.shared.notify(Changed::Picture);
    }

    fn quick(&mut self, categories: &[QuickCategory]) {
        let recency = Recency::new(now(), lock(&self.shared.settings).recent_age);
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
        self.shared.update(|s| s.cleaning = true);
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
        if let (Ok(report), Some(picture)) = (&result, picture) {
            self.cleaned(picture, report);
        }
        *lock(&self.shared.report) = Some(result);
        *lock(&self.shared.cleanup) = None;
        self.shared.update(|s| s.cleaning = false);
        self.shared.notify(Changed::Cleanup);
    }

    /// Marks what a cleanup changed and re-reads the directories holding
    /// it, so their totals and their ancestors' are right again.
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
        for parent in parents {
            self.apply(&RescanTask::Shallow(parent), &options, &mut written);
        }
        self.after_change(Some(&written));
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

fn refresh_failure(error: &RefreshError) -> Failure {
    match error {
        RefreshError::Index(IndexError::NoSpace) => Failure::NoSpace,
        RefreshError::Index(_) => Failure::Index,
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
        RescanTask::Shallow(path) | RescanTask::Recursive(path) => Some(path.clone()),
        RescanTask::Full => None,
    }
}
