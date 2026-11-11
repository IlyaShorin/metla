//! The engine: one long-lived object that owns the index, keeps the
//! picture fresh and answers everything the app shows.
//!
//! API calls are short and return at once; they read the picture through
//! a connection of their own. Everything that changes the picture — full
//! scans, rescans, cleanups — runs on the engine's scheduler thread, one
//! at a time, and reports through the observer (`Changed`, no data).

mod kept;
mod listing;
mod profile;
mod refresh;
mod scheduler;
mod settings;
mod shared;
mod view;
mod worktrees;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::clean::{CleanError, CleanProgress, CleanSettings, Cleaner, Production, TrashHost};
use crate::domain::{
    Bytes, Category, CleanupPreview, CleanupReport, Confirmation, DirId, DirRecord, Exclusion,
    PreviewId, ProgressSnapshot, QuickCategory, Recency, Reclaimable, Tally,
};
use crate::fs::{now, volume};
use crate::git::{Git, WorktreeSettings};
use crate::index::{
    IndexEditor, IndexError, IndexReader, IndexWriter, Order, Page, Picture, PictureId, Snapshot,
};
use crate::rules::RuleSet;
use crate::scan::{ScanRoot, ScanScope};
use crate::watch::Coalescer;

pub use kept::{decode_report, encode_report};
pub use listing::{LISTED_FILES, Largest, Rest};
pub use profile::{OFF, PowerProfile, Profile, Profiles};
pub use refresh::{MAX_SUBTREE, RefreshError, Refreshed, refresh};
pub use scheduler::{EVENTS_NOTE, Watching};
pub use settings::{HotkeyMode, Settings};
pub use view::{
    CandidatePage, Changed, Completeness, Counters, DirRef, EngineObserver, EngineState, Failure,
    Folder, FolderEntry, Freshness, Listed, Overview, Removability, TopFolder, VolumeSpace,
};

use scheduler::{ALL, Command, Scheduler, resumable};
use shared::{Shared, Status, lock};
use worktrees::Statuses;

/// The index file in the engine's data directory.
pub const INDEX_FILE: &str = "index.sqlite";
/// Where the settings are kept in the index.
const SETTINGS_NOTE: &str = "settings";
/// Where the last cleanup's report is kept in the index.
pub(crate) const REPORT_NOTE: &str = "report";

/// What an engine is made for.
pub struct EngineConfig {
    /// The directory analysed: `/` in the app, a fixture in tests.
    pub root: PathBuf,
    /// The user's home directory and name, for the rules.
    pub home: PathBuf,
    pub user: OsString,
    /// Where the index lives; events inside it are ignored.
    pub data_dir: PathBuf,
    pub git: Git,
    /// Lets the cleaner act on a root that is not a fixture.
    pub production: Option<Production>,
    /// Moves things to the Trash.
    pub trash: Arc<dyn TrashHost + Send + Sync>,
    pub watching: Watching,
    pub profiles: Profiles,
    /// The profile at the start.
    pub profile: PowerProfile,
    /// The most the index file may grow to; past it a scan fails as on
    /// a full disk (`Failure::NoSpace`). `None`: as much as the disk has.
    pub index_limit: Option<Bytes>,
}

#[derive(Debug)]
pub enum EngineError {
    Index(IndexError),
    /// The root or the data directory cannot be used.
    Setup(String),
    Clean(CleanError),
    /// The picture has no such directory.
    NotInPicture,
    /// The engine's scheduler has stopped.
    Stopped,
}

impl From<IndexError> for EngineError {
    fn from(error: IndexError) -> EngineError {
        EngineError::Index(error)
    }
}

impl From<CleanError> for EngineError {
    fn from(error: CleanError) -> EngineError {
        EngineError::Clean(error)
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            EngineError::Index(error) => error.to_string(),
            EngineError::Setup(why) => why.clone(),
            EngineError::Clean(error) => error.to_string(),
            EngineError::NotInPicture => "not in the picture".into(),
            EngineError::Stopped => "the engine has stopped".into(),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for EngineError {}

pub struct Engine {
    shared: Arc<Shared>,
    reader: Mutex<IndexReader>,
    editor: IndexEditor,
    commands: Sender<Command>,
    scheduler: Option<JoinHandle<()>>,
    worktrees: Option<JoinHandle<()>>,
}

/// Opens the index, replacing one of another schema version: the index
/// holds nothing that a scan cannot make again.
fn open_index(index: &Path, limit: Option<Bytes>) -> Result<IndexWriter, EngineError> {
    match IndexWriter::open_limited(index, limit) {
        Err(IndexError::UnknownSchema(Some(_))) => {
            for suffix in ["", "-wal", "-shm"] {
                let mut file = index.as_os_str().to_owned();
                file.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(file));
            }
            Ok(IndexWriter::open_limited(index, limit)?)
        }
        other => Ok(other?),
    }
}

/// The data directory, made if needed and canonical, and the index in it.
fn data_files(data_dir: &Path) -> Result<(PathBuf, PathBuf), EngineError> {
    setup(data_dir, std::fs::create_dir_all(data_dir))?;
    let data_dir = setup(data_dir, std::fs::canonicalize(data_dir))?;
    let index = data_dir.join(INDEX_FILE);
    Ok((data_dir, index))
}

/// The last cleanup's report kept in the index, if there is one.
fn kept_report(reader: &mut IndexReader) -> Result<Option<CleanupReport>, EngineError> {
    let note = reader.read(|snap| snap.note(REPORT_NOTE))?;
    Ok(note.and_then(|bytes| decode_report(&bytes)))
}

/// The settings kept in the index, or the defaults.
fn stored_settings(reader: &mut IndexReader) -> Result<Settings, EngineError> {
    let note = reader.read(|snap| snap.note(SETTINGS_NOTE))?;
    let text = note.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    Ok(text.map_or_else(Settings::default, |text| Settings::decode(&text)))
}

/// Whether only a full scan makes the picture right: there is no picture
/// of this root, its last scan never finished, or the event history it
/// would resume from is lost.
fn needs_full_scan(
    reader: &mut IndexReader,
    root: &ScanRoot,
    config: &EngineConfig,
    settings: &Settings,
) -> Result<bool, EngineError> {
    let scan = reader.read(|snap| snap.scan())?;
    let current = scan.as_ref().is_some_and(|scan| scan.is_current());
    let pictured = scan.and_then(|scan| scan.picture);
    let usable = current && pictured.is_some_and(|picture| picture.root == root.path());
    let streams = config.watching != Watching::Manual;
    let timing = config.profiles.of(config.profile).timing;
    let watched = settings.background && timing.is_some();
    let follows = !(streams && watched) || resumable(reader, root.device()).is_some();
    Ok(!usable || !follows)
}

/// What the engine shares between its threads, and the worktree worker.
fn share(
    parts: Parts,
    root: &ScanRoot,
    settings: Settings,
    needs_full_scan: bool,
) -> (Arc<Shared>, JoinHandle<()>) {
    let rules = Arc::new(RuleSet::new(&parts.home, &parts.user));
    let cleaner = Cleaner::new(
        root.path(),
        parts.production,
        Arc::clone(&rules),
        parts.git.clone(),
        clean_settings(&parts.home, &settings),
    );
    let observer = shared::ObserverSlot::default();
    let ready = Arc::clone(&observer);
    let (statuses, worktrees) = Statuses::start(parts.git.clone(), move || {
        if let Some(observer) = lock(&ready).clone() {
            observer.changed(Changed::Worktrees);
        }
    });
    let shared = Arc::new(Shared {
        root: root.path().to_path_buf(),
        home: parts.home,
        rules,
        git: parts.git,
        profiles: parts.profiles,
        trash: parts.trash,
        observer,
        status: Mutex::new(Status {
            profile: parts.profile,
            needs_full_scan,
            ..Status::default()
        }),
        scan: Mutex::default(),
        cleanup: Mutex::default(),
        report: Mutex::default(),
        settings: Mutex::new(settings),
        cleaner: Mutex::new(cleaner),
        statuses,
        counters: Mutex::default(),
    });
    (shared, worktrees)
}

/// Starts the engine's thread: the scheduler with a reader of its own and
/// the writer, and a coalescer that ignores the data directory and what
/// the scan's scope leaves out.
fn start_scheduler(
    shared: &Arc<Shared>,
    (data_dir, index): (PathBuf, PathBuf),
    writer: IndexWriter,
    root: ScanRoot,
    watching: Watching,
    (sender, commands): (&Sender<Command>, Receiver<Command>),
) -> Result<JoinHandle<()>, EngineError> {
    let scope = setup(root.path(), ScanScope::detect(&root))?;
    let mut ignored = vec![data_dir];
    ignored.extend(scope.excluded().map(Path::to_path_buf));
    let coalescer = Coalescer::new(root.path(), ignored, shared.profile().timing);
    let scheduler = Scheduler {
        shared: Arc::clone(shared),
        reader: IndexReader::open(&index)?,
        writer,
        root,
        scope,
        coalescer,
        watching,
        commands,
        sender: sender.clone(),
        stream: None,
        origin: None,
        clock: Instant::now(),
        next_batch: None,
    };
    let thread = thread::Builder::new().name("metla-engine".into());
    setup(
        Path::new("engine thread"),
        thread.spawn(move || scheduler.run()),
    )
}

/// The parts of a configuration that move into the shared state.
struct Parts {
    home: PathBuf,
    user: OsString,
    git: Git,
    production: Option<Production>,
    trash: Arc<dyn TrashHost + Send + Sync>,
    profiles: Profiles,
    profile: PowerProfile,
}

fn setup<T>(what: &Path, result: std::io::Result<T>) -> Result<T, EngineError> {
    result.map_err(|e| EngineError::Setup(format!("{}: {e}", what.display())))
}

impl Engine {
    /// Opens the engine: the saved picture answers at once; watching and
    /// any full scan that is needed start on the engine's thread.
    pub fn open(config: EngineConfig) -> Result<Engine, EngineError> {
        let (data_dir, index) = data_files(&config.data_dir)?;
        let writer = open_index(&index, config.index_limit)?;
        let mut reader = IndexReader::open(&index)?;
        let settings = stored_settings(&mut reader)?;
        let report = kept_report(&mut reader)?;
        let root = setup(&config.root, ScanRoot::new(&config.root))?;
        let needs_full_scan = needs_full_scan(&mut reader, &root, &config, &settings)?;
        let watching = config.watching;
        let parts = Parts {
            // The scan reads the root's canonical path: the home must be
            // spelled the same way, or no rule under it matches.
            home: std::fs::canonicalize(&config.home).unwrap_or(config.home),
            user: config.user,
            git: config.git,
            production: config.production,
            trash: config.trash,
            profiles: config.profiles,
            profile: config.profile,
        };
        let (shared, worktrees) = share(parts, &root, settings, needs_full_scan);
        *lock(&shared.report) = report.map(Ok);
        let (commands, queue) = mpsc::channel();
        let editor = writer.editor();
        let files = (data_dir, index);
        let thread = start_scheduler(&shared, files, writer, root, watching, (&commands, queue));
        Ok(Engine {
            shared,
            reader: Mutex::new(reader),
            editor,
            commands,
            scheduler: Some(thread?),
            worktrees: Some(worktrees),
        })
    }

    pub fn set_observer(&self, observer: Arc<dyn EngineObserver>) {
        *lock(&self.shared.observer) = Some(observer);
    }

    fn send(&self, command: Command) -> Result<(), EngineError> {
        let sent = self.commands.send(command);
        sent.or(Err(EngineError::Stopped))
    }

    fn read<T>(
        &self,
        query: impl FnOnce(&Snapshot<'_>) -> Result<T, IndexError>,
    ) -> Result<T, EngineError> {
        Ok(lock(&self.reader).read(query)?)
    }

    fn recency(&self) -> Recency {
        self.shared.recency()
    }

    /// Everything the Overview shows, from the saved picture.
    pub fn snapshot(&self) -> Result<Overview, EngineError> {
        let recency = self.recency();
        let root = self.shared.root.clone();
        let (scan, totals, (categories, owned), per) = self.read(|snap| {
            Ok((
                snap.scan()?,
                snap.root()?.map(|row| row.tree),
                (snap.category_tallies()?, snap.owned_by_category()?),
                snap.reclaimable_by_category(&recency)?,
            ))
        })?;
        let mut reclaimable = Reclaimable::default();
        per.values().for_each(|one| reclaimable.merge(one));
        let picture = scan.and_then(|s| s.picture).filter(|p| p.root == root);
        let freshness = self.fresh(picture.as_ref());
        let space = volume(&root).ok().map(|v| VolumeSpace {
            total: v.total,
            available: v.available,
        });
        Ok(Overview {
            totals: picture.as_ref().and(totals),
            kinds: picture.as_ref().map(|p| p.kinds).unwrap_or_default(),
            coverage: picture.map(|p| p.coverage),
            root,
            volume: space,
            categories,
            reclaimable,
            reclaimable_by_category: per,
            owned,
            freshness,
            profile: self.shared.effective(),
        })
    }

    /// Only how fresh the picture is: cheaper than a snapshot, which also
    /// counts the candidates.
    pub fn freshness(&self) -> Result<Freshness, EngineError> {
        let root = self.shared.root.clone();
        let scan = self.read(|snap| snap.scan())?;
        let picture = scan.and_then(|s| s.picture).filter(|p| p.root == root);
        Ok(self.fresh(picture.as_ref()))
    }

    fn fresh(&self, picture: Option<&Picture>) -> Freshness {
        let status = *lock(&self.shared.status);
        let completeness = match picture {
            None => Completeness::Missing,
            Some(p) if p.coverage.is_complete() => Completeness::Complete,
            Some(_) => Completeness::Partial,
        };
        Freshness {
            full_scan: picture.map(|p| p.finished),
            updated: picture.map(|p| p.updated.unwrap_or(p.finished)),
            pending: status.pending,
            state: self.shared.state(),
            picture: completeness,
            watching: status.watching,
            needs_full_scan: status.needs_full_scan,
            full_scans_ended: self.counters().full_scans,
        }
    }

    /// One page of the offered candidates of `category` (all when
    /// `None`), largest first; worktrees with their status as far as it
    /// is known.
    pub fn candidates(
        &self,
        category: Option<Category>,
        page: Page,
    ) -> Result<Vec<Listed>, EngineError> {
        Ok(self
            .candidate_page(category, Order::default(), page)?
            .listed)
    }

    /// One page of the offered candidates of `category` in `order`, and
    /// how many are offered.
    pub fn candidate_page(
        &self,
        category: Option<Category>,
        order: Order,
        page: Page,
    ) -> Result<CandidatePage, EngineError> {
        let recency = self.recency();
        let (picture, (found, total)) = self.read(|snap| {
            let picture = snap.scan()?.and_then(|s| s.picture).map(|p| p.id());
            Ok((
                picture,
                snap.candidate_page(category, order, page, &recency)?,
            ))
        })?;
        let worktrees = found.iter().any(|c| c.category() == Category::GitWorktree);
        if let Some(picture) = picture.filter(|_| worktrees) {
            self.ask_statuses(picture)?;
        }
        let listed = found.into_iter().map(|candidate| {
            let worktree = candidate.category() == Category::GitWorktree;
            let removability = picture
                .filter(|_| worktree)
                .map(|picture| self.shared.statuses.removability(picture, candidate.id));
            Listed {
                candidate,
                removability,
            }
        });
        Ok(CandidatePage {
            listed: listed.collect(),
            total,
        })
    }

    /// The first time the worktree list of `picture` is asked for, its
    /// statuses are computed in the background.
    fn ask_statuses(&self, picture: PictureId) -> Result<(), EngineError> {
        if !self.shared.statuses.ask(picture) {
            return Ok(());
        }
        let recency = self.recency();
        let all = self.read(|snap| snap.candidates(Some(Category::GitWorktree), ALL, &recency))?;
        self.shared.compute_statuses(picture, all);
        Ok(())
    }

    /// The Overview's largest folders: the `limit` largest that hold none
    /// of the others, from the picture, or from the live tree while the
    /// first scan runs.
    pub fn largest_folders(&self, limit: usize) -> Result<Vec<TopFolder>, EngineError> {
        let root = self.shared.root.clone();
        let (pictured, largest) = self.read(|snap| {
            let pictured = snap
                .scan()?
                .and_then(|s| s.picture)
                .filter(|p| p.root == root);
            Ok((pictured.is_some(), snap.largest(listing::sample(limit))?))
        })?;
        let running = lock(&self.shared.scan).as_ref().map(|s| s.control.live());
        let folders = match (pictured, running) {
            (false, Some(live)) => listing::live(&root, live),
            _ => listing::pictured(largest),
        };
        Ok(listing::tops(folders, limit))
    }

    /// Offered candidates per category; every category is present.
    pub fn category_totals(
        &self,
    ) -> Result<std::collections::BTreeMap<Category, Tally>, EngineError> {
        self.read(|snap| snap.category_tallies())
    }

    /// One page of a directory's contents, largest first: its
    /// subdirectories from the picture and its files listed now, at
    /// most [`LISTED_FILES`] of them and the rest as one entry. Reads
    /// only the subdirectories up to the page's end.
    pub fn children(&self, dir: &DirRef, page: Page) -> Result<Folder, EngineError> {
        let end = page.offset.saturating_add(page.limit);
        let found = self.read(|snap| {
            let Some((row, path)) = located(snap, dir)? else {
                return Ok(None);
            };
            let children = snap.largest_children(row.id, end)?;
            let count = snap.child_count(row.id)?;
            Ok(Some((row, path, children, count)))
        })?;
        let (row, path, children, count) = found.ok_or(EngineError::NotInPicture)?;
        Ok(listing::folder(row, path, children, count, page))
    }

    /// Starts a full scan with the `Interactive` profile's threads; it
    /// lifts a hold and forgets the last failure.
    pub fn start_scan(&self) -> Result<(), EngineError> {
        self.shared.update(|s| {
            s.held = false;
            s.failure = None;
        });
        self.send(Command::Scan)
    }

    /// Stops a running full scan. With a picture to fall back on, the
    /// scan is given up; the first scan ends as a partial picture. Either
    /// way no full scan starts on its own until the user asks for one.
    pub fn cancel_scan(&self) {
        lock(&self.shared.status).held = true;
        if let Some(scan) = lock(&self.shared.scan).as_ref() {
            scan.control.cancel();
        }
    }

    /// The running full scan's counters.
    pub fn progress(&self) -> Option<ProgressSnapshot> {
        lock(&self.shared.scan)
            .as_ref()
            .map(|s| s.control.progress())
    }

    /// Honoured at once: `Off` stops watching and cancels a full scan the
    /// user did not ask for; `Saver` defers the full scans the engine
    /// would start on its own.
    pub fn set_power_profile(&self, profile: PowerProfile) -> Result<(), EngineError> {
        lock(&self.shared.status).profile = profile;
        if !self.shared.power().full_scans {
            self.shared.cancel_background_scan();
        }
        self.shared.notify(Changed::Activity);
        self.send(Command::Profile)
    }

    /// Whether someone looks at the picture (the app's window or panel is
    /// visible). While they do, changes are followed as `Interactive`
    /// follows them and what waits is applied at once; then the power
    /// profile applies again.
    pub fn set_attention(&self, looking: bool) -> Result<(), EngineError> {
        self.shared.update(|s| s.attention = looking);
        self.send(Command::Attention)
    }

    /// Background analysis on or off (a setting, kept).
    pub fn set_background(&self, enabled: bool) -> Result<(), EngineError> {
        let mut settings = self.settings();
        settings.background = enabled;
        self.set_settings(settings)
    }

    pub fn exclusions(&self) -> Result<Vec<Exclusion>, EngineError> {
        self.read(|snap| snap.exclusions())
    }

    /// The candidates `exclusion` hides (or would hide).
    pub fn hidden_by(&self, exclusion: &Exclusion) -> Result<Tally, EngineError> {
        self.read(|snap| snap.hidden_by(exclusion))
    }

    /// Keeps `path`; false when it was kept already.
    pub fn add_exclusion(&self, exclusion: &Exclusion) -> Result<bool, EngineError> {
        let added = self.editor.exclude(exclusion)?;
        self.shared.notify(Changed::Exclusions);
        Ok(added)
    }

    /// No longer keeps `path`; false when it was not kept.
    pub fn remove_exclusion(&self, exclusion: &Exclusion) -> Result<bool, EngineError> {
        let removed = self.editor.include(exclusion)?;
        self.shared.notify(Changed::Exclusions);
        Ok(removed)
    }

    /// The preview of the candidates `selection` (ids of the picture);
    /// ids the picture no longer offers are left out.
    pub fn preview(&self, selection: &[DirId]) -> Result<CleanupPreview, EngineError> {
        let recency = self.recency();
        let candidates = self.read(|snap| {
            let found = selection.iter().map(|id| snap.candidate(*id, &recency));
            let found: Result<Vec<_>, _> = found.collect();
            Ok(found?.into_iter().flatten().collect())
        })?;
        match &mut *lock(&self.shared.cleaner) {
            Ok(cleaner) => Ok(cleaner.preview(candidates, now())),
            Err(why) => Err(EngineError::Clean(why.clone())),
        }
    }

    /// Executes the preview `id` as confirmed, on the engine's thread;
    /// follow it with `cleanup_progress` and `last_report`.
    pub fn execute_confirmed(
        &self,
        id: PreviewId,
        confirmation: Confirmation,
    ) -> Result<(), EngineError> {
        self.send(Command::Execute(id, confirmation))
    }

    /// Cleans the preselected candidates of exactly `categories` without
    /// a confirmation step, on the engine's thread.
    pub fn quick_clean(&self, categories: Vec<QuickCategory>) -> Result<(), EngineError> {
        self.send(Command::Quick(categories))
    }

    pub fn cleanup_progress(&self) -> Option<CleanProgress> {
        lock(&self.shared.cleanup).as_ref().map(|c| c.progress())
    }

    pub fn cancel_cleanup(&self) {
        if let Some(control) = lock(&self.shared.cleanup).as_ref() {
            control.cancel();
        }
    }

    /// The last cleanup's report, or why it did not run.
    pub fn last_report(&self) -> Option<Result<CleanupReport, CleanError>> {
        lock(&self.shared.report).clone()
    }

    pub fn settings(&self) -> Settings {
        lock(&self.shared.settings).clone()
    }

    /// Takes and keeps new settings.
    pub fn set_settings(&self, settings: Settings) -> Result<(), EngineError> {
        self.editor
            .set_note(SETTINGS_NOTE, settings.encode().as_bytes())?;
        if let Ok(cleaner) = &mut *lock(&self.shared.cleaner) {
            cleaner.set_settings(clean_settings(&self.shared.home, &settings));
        }
        *lock(&self.shared.settings) = settings;
        if !self.shared.power().full_scans {
            self.shared.cancel_background_scan();
        }
        self.shared.notify(Changed::Settings);
        self.send(Command::Profile)
    }

    /// Tells the engine that these directories changed, as an event
    /// stream would. They count as pending from now on.
    pub fn changed(&self, paths: Vec<PathBuf>) -> Result<(), EngineError> {
        let reported = paths.len() as u64;
        lock(&self.shared.status).pending += reported;
        self.send(Command::Changed(paths))
    }

    /// What the engine has done since it opened.
    pub fn counters(&self) -> Counters {
        *lock(&self.shared.counters)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.cancel_cleanup();
        if let Some(scan) = lock(&self.shared.scan).as_ref() {
            scan.control.cancel();
        }
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.scheduler.take() {
            let _ = thread.join();
        }
        self.shared.statuses.stop();
        if let Some(thread) = self.worktrees.take() {
            let _ = thread.join();
        }
    }
}

/// The directory `dir` names and its path, if the picture has it.
fn located(snap: &Snapshot<'_>, dir: &DirRef) -> Result<Option<(DirRecord, PathBuf)>, IndexError> {
    let found = match dir {
        DirRef::Id(id) => (snap.record(*id)?, snap.path_of(*id)?),
        DirRef::Path(path) => (snap.dir(path)?, Some(path.clone())),
    };
    Ok(found.0.zip(found.1))
}

fn clean_settings(home: &Path, settings: &Settings) -> CleanSettings {
    CleanSettings {
        worktrees: WorktreeSettings {
            home: home.to_path_buf(),
            stale_after: settings.stale_after,
        },
        recent_age: settings.recent_age,
    }
}
