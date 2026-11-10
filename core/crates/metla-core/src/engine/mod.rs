//! The engine: one long-lived object that owns the index, keeps the
//! picture fresh and answers everything the app shows.
//!
//! API calls are short and return at once; they read the picture through
//! a connection of their own. Everything that changes the picture — full
//! scans, rescans, cleanups — runs on the engine's scheduler thread, one
//! at a time, and reports through the observer (`Changed`, no data).

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
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::clean::{CleanError, CleanProgress, CleanSettings, Cleaner, Production, TrashHost};
use crate::domain::{
    Category, CleanupPreview, CleanupReport, Confirmation, DirId, DirRecord, EntryKind, Exclusion,
    PreviewId, ProgressSnapshot, QuickCategory, Recency, Tally,
};
use crate::fs::{DirHandle, list_dir, now, volume};
use crate::git::{Git, WorktreeSettings};
use crate::index::{IndexEditor, IndexError, IndexReader, IndexWriter, Page, Snapshot};
use crate::rules::RuleSet;
use crate::scan::{ScanRoot, ScanScope};
use crate::watch::Coalescer;

pub use profile::{OFF, PowerProfile, Profile, Profiles};
pub use refresh::{MAX_SUBTREE, RefreshError, Refreshed, refresh};
pub use scheduler::{EVENTS_NOTE, Watching};
pub use settings::{HotkeyMode, Settings};
pub use view::{
    Changed, Completeness, Counters, DirRef, EngineObserver, EngineState, Folder, FolderEntry,
    Freshness, Listed, Overview, Removability, VolumeSpace,
};

use scheduler::{Command, Scheduler};
use shared::{Shared, Status, lock};
use worktrees::Statuses;

/// The index file in the engine's data directory.
pub const INDEX_FILE: &str = "index.sqlite";
/// Where the settings are kept in the index.
const SETTINGS_NOTE: &str = "settings";
/// The most files a folder listing keeps (the largest).
pub const FILE_LIMIT: usize = 10_000;

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
        match self {
            EngineError::Index(error) => write!(f, "{error}"),
            EngineError::Setup(why) => write!(f, "{why}"),
            EngineError::Clean(error) => write!(f, "{error}"),
            EngineError::NotInPicture => write!(f, "not in the picture"),
            EngineError::Stopped => write!(f, "the engine has stopped"),
        }
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
fn open_index(index: &Path) -> Result<IndexWriter, EngineError> {
    match IndexWriter::open(index) {
        Err(IndexError::UnknownSchema(Some(_))) => {
            for suffix in ["", "-wal", "-shm"] {
                let mut file = index.as_os_str().to_owned();
                file.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(file));
            }
            Ok(IndexWriter::open(index)?)
        }
        other => Ok(other?),
    }
}

fn setup<T>(what: &Path, result: std::io::Result<T>) -> Result<T, EngineError> {
    result.map_err(|e| EngineError::Setup(format!("{}: {e}", what.display())))
}

impl Engine {
    /// Opens the engine: the saved picture answers at once; watching and
    /// any full scan that is needed start on the engine's thread.
    pub fn open(config: EngineConfig) -> Result<Engine, EngineError> {
        setup(&config.data_dir, std::fs::create_dir_all(&config.data_dir))?;
        let data_dir = setup(&config.data_dir, std::fs::canonicalize(&config.data_dir))?;
        let index = data_dir.join(INDEX_FILE);
        let writer = open_index(&index)?;
        let mut reader = IndexReader::open(&index)?;
        let settings = reader.read(|snap| snap.note(SETTINGS_NOTE))?;
        let settings = settings.map_or_else(Settings::default, |bytes| {
            Settings::decode(&String::from_utf8_lossy(&bytes))
        });
        let root = setup(&config.root, ScanRoot::new(&config.root))?;
        let pictured = reader.read(|snap| Ok(snap.scan()?.and_then(|scan| scan.picture)))?;
        let has_picture = pictured.is_some_and(|picture| picture.root == root.path());
        let scope = setup(&config.root, ScanScope::detect(&root))?;
        let rules = Arc::new(RuleSet::new(&config.home, &config.user));
        let cleaner = Cleaner::new(
            root.path(),
            config.production,
            Arc::clone(&rules),
            config.git.clone(),
            clean_settings(&config.home, &settings),
        );
        let observer = shared::ObserverSlot::default();
        let ready = Arc::clone(&observer);
        let (statuses, worktrees) = Statuses::start(config.git.clone(), move || {
            if let Some(observer) = lock(&ready).clone() {
                observer.changed(Changed::Worktrees);
            }
        });
        let shared = Arc::new(Shared {
            root: root.path().to_path_buf(),
            home: config.home,
            rules,
            git: config.git,
            profiles: config.profiles,
            trash: config.trash,
            observer,
            status: Mutex::new(Status {
                profile: config.profile,
                pending: 0,
                watching: false,
                needs_full_scan: !has_picture,
                updating: false,
                cleaning: false,
                held: false,
            }),
            scan: Mutex::default(),
            cleanup: Mutex::default(),
            report: Mutex::default(),
            settings: Mutex::new(settings),
            cleaner: Mutex::new(cleaner),
            statuses,
            counters: Mutex::default(),
        });
        let mut ignored = vec![data_dir];
        ignored.extend(scope.excluded().map(Path::to_path_buf));
        let coalescer = Coalescer::new(root.path(), ignored, shared.profile().timing);
        let (commands, queue) = mpsc::channel();
        let scheduler = Scheduler {
            shared: Arc::clone(&shared),
            reader: IndexReader::open(&index)?,
            writer,
            root,
            scope,
            coalescer,
            watching: config.watching,
            commands: queue,
            sender: commands.clone(),
            stream: None,
            origin: None,
            clock: Instant::now(),
            manual: 0,
            next_batch: None,
        };
        let editor = scheduler.writer.editor();
        let scheduler = setup(
            Path::new("engine thread"),
            thread::Builder::new()
                .name("metla-engine".into())
                .spawn(move || scheduler.run()),
        )?;
        Ok(Engine {
            shared,
            reader: Mutex::new(reader),
            editor,
            commands,
            scheduler: Some(scheduler),
            worktrees: Some(worktrees),
        })
    }

    pub fn set_observer(&self, observer: Arc<dyn EngineObserver>) {
        *lock(&self.shared.observer) = Some(observer);
    }

    fn send(&self, command: Command) -> Result<(), EngineError> {
        self.commands
            .send(command)
            .map_err(|_| EngineError::Stopped)
    }

    fn read<T>(
        &self,
        query: impl FnOnce(&Snapshot<'_>) -> Result<T, IndexError>,
    ) -> Result<T, EngineError> {
        Ok(lock(&self.reader).read(query)?)
    }

    fn recency(&self) -> Recency {
        Recency::new(now(), lock(&self.shared.settings).recent_age)
    }

    /// Everything the Overview shows, from the saved picture.
    pub fn snapshot(&self) -> Result<Overview, EngineError> {
        let recency = self.recency();
        let root = self.shared.root.clone();
        let (scan, totals, categories, reclaimable) = self.read(|snap| {
            Ok((
                snap.scan()?,
                snap.root()?.map(|row| row.tree),
                snap.category_tallies()?,
                snap.reclaimable(&recency)?,
            ))
        })?;
        let current = scan.as_ref().is_some_and(|s| s.is_current());
        let picture = scan.and_then(|s| s.picture).filter(|p| p.root == root);
        let status = *lock(&self.shared.status);
        let completeness = match &picture {
            None => Completeness::Missing,
            Some(p) if p.coverage.is_complete() => Completeness::Complete,
            Some(_) => Completeness::Partial,
        };
        let freshness = Freshness {
            full_scan: picture.as_ref().map(|p| p.finished),
            updated: picture.as_ref().map(|p| p.updated.unwrap_or(p.finished)),
            pending: status.pending,
            state: self.shared.state(),
            picture: completeness,
            watching: status.watching,
            needs_full_scan: status.needs_full_scan || !current,
        };
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
            freshness,
            profile: self.shared.effective(),
        })
    }

    /// One page of the offered candidates of `category` (all when
    /// `None`), largest first; worktrees with their status as far as it
    /// is known.
    pub fn candidates(
        &self,
        category: Option<Category>,
        page: Page,
    ) -> Result<Vec<Listed>, EngineError> {
        let recency = self.recency();
        let (picture, found) = self.read(|snap| {
            let picture = snap.scan()?.and_then(|s| s.picture).map(|p| p.id());
            Ok((picture, snap.candidates(category, page, &recency)?))
        })?;
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
        Ok(listed.collect())
    }

    /// Offered candidates per category; every category is present.
    pub fn category_totals(
        &self,
    ) -> Result<std::collections::BTreeMap<Category, Tally>, EngineError> {
        self.read(|snap| snap.category_tallies())
    }

    /// A directory's contents, largest first: its subdirectories from the
    /// picture and its files listed now (at most [`FILE_LIMIT`]).
    pub fn children(&self, dir: &DirRef, page: Page) -> Result<Folder, EngineError> {
        let found = self.read(|snap| {
            let found = match dir {
                DirRef::Id(id) => (snap.record(*id)?, snap.path_of(*id)?),
                DirRef::Path(path) => (snap.dir(path)?, Some(path.clone())),
            };
            let (Some(row), Some(path)) = found else {
                return Ok(None);
            };
            let children = snap.children(row.id)?;
            Ok(Some((row, path, children)))
        })?;
        let (row, path, children) = found.ok_or(EngineError::NotInPicture)?;
        Ok(folder(row, path, children, page))
    }

    /// Starts a full scan with the `Interactive` profile's threads.
    pub fn start_scan(&self) -> Result<(), EngineError> {
        lock(&self.shared.status).held = false;
        self.send(Command::Scan)
    }

    /// Stops a running full scan. With a picture to fall back on, the
    /// scan is given up; the first scan ends as a partial picture.
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
        if !self.shared.profile().full_scans {
            self.shared.cancel_background_scan();
        }
        self.shared.notify(Changed::Activity);
        self.send(Command::Profile)
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
        if !self.shared.profile().full_scans {
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

fn clean_settings(home: &Path, settings: &Settings) -> CleanSettings {
    CleanSettings {
        worktrees: WorktreeSettings {
            home: home.to_path_buf(),
            stale_after: settings.stale_after,
        },
        recent_age: settings.recent_age,
    }
}

/// A folder's page: subdirectories and files listed now, largest first.
fn folder(row: DirRecord, path: PathBuf, children: Vec<DirRecord>, page: Page) -> Folder {
    let listed = DirHandle::open(&path).and_then(|dir| list_dir(&dir));
    let mut files: Vec<FolderEntry> = listed
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.meta.kind != EntryKind::Dir)
        .map(|e| FolderEntry::File {
            bytes: e.meta.share(),
            name: e.name,
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(size(file)));
    let files_truncated = files.len() > FILE_LIMIT;
    files.truncate(FILE_LIMIT);
    let dirs = children
        .into_iter()
        .map(|row| FolderEntry::Dir(Box::new(row)));
    let mut entries: Vec<FolderEntry> = dirs.collect();
    entries.extend(files);
    entries.sort_by(|a, b| size(b).cmp(&size(a)).then_with(|| name(a).cmp(name(b))));
    let total = entries.len();
    let entries = entries
        .into_iter()
        .skip(page.offset)
        .take(page.limit)
        .collect();
    Folder {
        dir: row,
        path,
        entries,
        total,
        files_truncated,
    }
}

fn size(entry: &FolderEntry) -> crate::domain::Bytes {
    match entry {
        FolderEntry::Dir(row) => row.tree.bytes,
        FolderEntry::File { bytes, .. } => *bytes,
    }
}

fn name(entry: &FolderEntry) -> &OsString {
    match entry {
        FolderEntry::Dir(row) => &row.name,
        FolderEntry::File { name, .. } => name,
    }
}
