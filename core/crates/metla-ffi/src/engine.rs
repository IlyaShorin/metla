//! The engine as Swift holds it. Every call is synchronous, returns at
//! once and is safe from any thread; long work (scans, cleanups) runs on
//! the engine's threads and is reported through the observer. Nothing
//! here walks the file system on the caller's thread except `children`,
//! which lists the one directory asked for, keeping at most
//! `LISTED_FILES` files.

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metla_core::clean::CleanProgress;
use metla_core::domain::{
    Category, CleanupPreview, DirId, Exclusion, PreviewId, ProgressSnapshot, QuickCategory, Tally,
};
use metla_core::engine::{
    self, DirRef, EngineConfig, Freshness, PowerProfile, Profiles, TopFolder, Watching,
};
use metla_core::git::Git;
use metla_core::index::{Order, Page};

use crate::callbacks::{EngineObserver, Observer, Trash, TrashHost};
use crate::error::MetlaError;
use crate::view::{
    CandidatePage, CleanupResult, Confirmations, EngineSettings, FolderPage, Overview,
};

/// The most entries a page holds, whatever the caller asks for.
pub const MAX_PAGE: u32 = 1000;

/// `limit`, at most [`MAX_PAGE`].
pub fn capped(limit: u32) -> usize {
    limit.min(MAX_PAGE) as usize
}

/// A page starting at `offset`, at most [`MAX_PAGE`] long.
pub fn page(offset: u64, limit: u32) -> Page {
    Page {
        offset: usize::try_from(offset).unwrap_or(usize::MAX),
        limit: capped(limit),
    }
}

/// Where and how an engine runs.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct EngineOptions {
    /// The directory analysed: `/` in the app, a fixture in development.
    pub root: String,
    /// The user's home directory and name, for the rules.
    pub home: String,
    pub user: String,
    /// Where the index lives.
    pub data_dir: String,
    /// The profile at the start (the app picks it from the power state).
    pub profile: PowerProfile,
    /// Follow the root with an FSEvents stream; without, the picture
    /// changes only by scans.
    pub follow: bool,
    /// Someone looks from the start: the app opened with its window, not
    /// in the menu bar only.
    pub attention: bool,
}

#[derive(uniffi::Object)]
pub struct Engine {
    inner: engine::Engine,
}

#[uniffi::export]
impl Engine {
    /// Opens the engine; the saved picture answers at once. Unless this
    /// is the app's release build, it can clean only a root marked as a
    /// fixture.
    #[uniffi::constructor]
    pub fn open(
        options: EngineOptions,
        trash: Arc<dyn TrashHost>,
    ) -> Result<Arc<Engine>, MetlaError> {
        let watching = match options.follow {
            true => Watching::FsEvents,
            false => Watching::Manual,
        };
        let config = EngineConfig {
            root: PathBuf::from(options.root),
            home: PathBuf::from(options.home),
            user: OsString::from(options.user),
            data_dir: PathBuf::from(options.data_dir),
            git: Git::system(),
            production: production(),
            trash: Arc::new(Trash(trash)),
            watching,
            profiles: Profiles::standard(cores()),
            profile: options.profile,
            index_limit: None,
            dirs: None,
            attention: options.attention,
        };
        let inner = engine::Engine::open(config)?;
        Ok(Arc::new(Engine { inner }))
    }

    pub fn set_observer(&self, observer: Arc<dyn EngineObserver>) {
        self.inner.set_observer(Arc::new(Observer(observer)));
    }

    /// Everything the Overview shows.
    pub fn snapshot(&self) -> Result<Overview, MetlaError> {
        Ok(Overview::from(self.inner.snapshot()?))
    }

    /// Only how fresh the picture is: cheaper than a snapshot.
    pub fn freshness(&self) -> Result<Freshness, MetlaError> {
        Ok(self.inner.freshness()?)
    }

    /// The running full scan's counters.
    pub fn progress(&self) -> Option<ProgressSnapshot> {
        self.inner.progress()
    }

    /// The Overview's largest folders, at most [`MAX_PAGE`].
    pub fn largest_folders(&self, limit: u32) -> Result<Vec<TopFolder>, MetlaError> {
        Ok(self.inner.largest_folders(capped(limit))?)
    }

    /// One page of the offered candidates of `category` (all when
    /// `None`) in `order`, and how many there are.
    pub fn candidates(
        &self,
        category: Option<Category>,
        order: Order,
        offset: u64,
        limit: u32,
    ) -> Result<CandidatePage, MetlaError> {
        let found = self
            .inner
            .candidate_page(category, order, page(offset, limit))?;
        Ok(CandidatePage::from(found))
    }

    /// One page of a directory's contents, largest first.
    pub fn children(&self, dir: DirRef, offset: u64, limit: u32) -> Result<FolderPage, MetlaError> {
        Ok(FolderPage::from(
            self.inner.children(&dir, page(offset, limit))?,
        ))
    }

    pub fn start_scan(&self) -> Result<(), MetlaError> {
        Ok(self.inner.start_scan()?)
    }

    pub fn cancel_scan(&self) {
        self.inner.cancel_scan();
    }

    pub fn set_power_profile(&self, profile: PowerProfile) -> Result<(), MetlaError> {
        Ok(self.inner.set_power_profile(profile)?)
    }

    /// Whether the app's window or panel is visible.
    pub fn set_attention(&self, looking: bool) -> Result<(), MetlaError> {
        Ok(self.inner.set_attention(looking)?)
    }

    /// A page of the paths the user keeps, in path order.
    pub fn exclusions(&self, offset: u64, limit: u32) -> Result<Vec<String>, MetlaError> {
        let page = page(offset, limit);
        let kept = self
            .inner
            .exclusions()?
            .into_iter()
            .skip(page.offset)
            .take(page.limit);
        Ok(kept
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect())
    }

    /// The candidates keeping `path` hides, kept already or not.
    pub fn hidden_by(&self, path: String) -> Result<Tally, MetlaError> {
        Ok(self.inner.hidden_by(&exclusion(path)?)?)
    }

    /// The settings the engine keeps.
    pub fn settings(&self) -> EngineSettings {
        EngineSettings::from(self.inner.settings())
    }

    /// Takes and keeps new settings.
    pub fn set_settings(&self, settings: EngineSettings) -> Result<(), MetlaError> {
        Ok(self.inner.set_settings(settings.into())?)
    }

    /// Keeps `path`; false when it was kept already.
    pub fn add_exclusion(&self, path: String) -> Result<bool, MetlaError> {
        Ok(self.inner.add_exclusion(&exclusion(path)?)?)
    }

    /// No longer keeps `path`; false when it was not kept.
    pub fn remove_exclusion(&self, path: String) -> Result<bool, MetlaError> {
        Ok(self.inner.remove_exclusion(&exclusion(path)?)?)
    }

    /// The preview of cleaning the candidates `selection`.
    pub fn preview(&self, selection: Vec<DirId>) -> Result<CleanupPreview, MetlaError> {
        Ok(self.inner.preview(&selection)?)
    }

    /// Executes the preview `preview` with what the user confirmed.
    pub fn execute_confirmed(
        &self,
        preview: PreviewId,
        confirmed: Confirmations,
    ) -> Result<(), MetlaError> {
        Ok(self.inner.execute_confirmed(preview, confirmed.into())?)
    }

    /// Cleans the preselected candidates of exactly `categories`, which
    /// can only be caches and build output.
    pub fn quick_clean(&self, categories: Vec<QuickCategory>) -> Result<(), MetlaError> {
        Ok(self.inner.quick_clean(categories)?)
    }

    pub fn cleanup_progress(&self) -> Option<CleanProgress> {
        self.inner.cleanup_progress()
    }

    pub fn cancel_cleanup(&self) {
        self.inner.cancel_cleanup();
    }

    /// The last cleanup's report, or why it did not run.
    pub fn last_report(&self) -> Option<CleanupResult> {
        self.inner.last_report().map(CleanupResult::from)
    }
}

/// The stamp scripts/build-core.sh compiled in: a hash of the core's
/// sources and its kind, so a build can tell which core it linked;
/// "unstamped" in a cargo build of its own.
#[uniffi::export]
pub fn core_stamp() -> String {
    option_env!("METLA_CORE_STAMP")
        .unwrap_or("unstamped")
        .to_owned()
}

/// Whether this build can clean a root that is not a fixture: only the
/// app's release build can.
#[uniffi::export]
pub fn cleans_beyond_fixtures() -> bool {
    production().is_some()
}

/// The capability to clean a root that is not a fixture: made only in a
/// release build with the `production` feature, the app's release build.
#[cfg(all(feature = "production", not(debug_assertions)))]
fn production() -> Option<metla_core::clean::Production> {
    Some(metla_core::clean::Production::release_app())
}

/// Any other build cleans fixtures only.
#[cfg(not(all(feature = "production", not(debug_assertions))))]
fn production() -> Option<metla_core::clean::Production> {
    None
}

/// `path` as an exclusion, checked where it comes in.
fn exclusion(path: String) -> Result<Exclusion, MetlaError> {
    Exclusion::new(Path::new(&path)).ok_or(MetlaError::InvalidPath { path })
}

/// The cores scans may use: as many as the system says, else one (the
/// profile table never gives a scan fewer than one thread).
fn cores() -> usize {
    std::thread::available_parallelism()
        .unwrap_or(NonZeroUsize::MIN)
        .get()
}
