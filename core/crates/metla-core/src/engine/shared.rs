//! What the API threads and the scheduler share: the observer, what the
//! engine is doing, the settings, the cleaner and the results of work.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::profile::{PowerProfile, Profile, Profiles};
use super::settings::Settings;
use super::view::{Changed, Counters, EngineObserver, EngineState};
use super::worktrees::Statuses;
use crate::clean::{CleanControl, CleanError, Cleaner, TrashHost};
use crate::domain::CleanupReport;
use crate::git::Git;
use crate::rules::RuleSet;
use crate::scan::ScanControl;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The observer slot, shared with the worktree worker.
pub type ObserverSlot = Arc<Mutex<Option<Arc<dyn EngineObserver>>>>;

/// A full scan that runs.
pub struct RunningScan {
    pub control: Arc<ScanControl>,
    pub interactive: bool,
}

/// What the engine is doing, as the scheduler last said.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    /// The profile the app asked for.
    pub profile: PowerProfile,
    pub pending: u64,
    pub watching: bool,
    pub needs_full_scan: bool,
    pub cleaning: bool,
    /// A scan was cancelled: no full scan starts on its own until the
    /// user asks for one.
    pub held: bool,
}

pub struct Shared {
    pub root: PathBuf,
    pub home: PathBuf,
    pub rules: Arc<RuleSet>,
    pub git: Git,
    pub profiles: Profiles,
    pub trash: Arc<dyn TrashHost + Send + Sync>,
    pub observer: ObserverSlot,
    pub status: Mutex<Status>,
    pub scan: Mutex<Option<RunningScan>>,
    pub cleanup: Mutex<Option<Arc<CleanControl>>>,
    pub report: Mutex<Option<Result<CleanupReport, CleanError>>>,
    pub settings: Mutex<Settings>,
    /// `Err`: why this root cannot be cleaned.
    pub cleaner: Mutex<Result<Cleaner, CleanError>>,
    pub statuses: Arc<Statuses>,
    pub counters: Mutex<Counters>,
}

impl Shared {
    pub fn notify(&self, what: Changed) {
        let observer = lock(&self.observer).clone();
        if let Some(observer) = observer {
            observer.changed(what);
        }
    }

    /// The profile in force: `Off` while background analysis is off.
    pub fn effective(&self) -> PowerProfile {
        let background = lock(&self.settings).background;
        let asked = lock(&self.status).profile;
        match background {
            true => asked,
            false => PowerProfile::Off,
        }
    }

    pub fn profile(&self) -> Profile {
        self.profiles.of(self.effective())
    }

    pub fn update(&self, change: impl FnOnce(&mut Status)) {
        change(&mut lock(&self.status));
        self.notify(Changed::Activity);
    }

    pub fn count(&self, change: impl FnOnce(&mut Counters)) {
        change(&mut lock(&self.counters));
    }

    /// What the engine is doing now.
    pub fn state(&self) -> EngineState {
        let status = *lock(&self.status);
        let effective = self.effective();
        let scanning = lock(&self.scan).as_ref().map(|scan| EngineState::Scanning {
            progress: scan.control.progress(),
            interactive: scan.interactive,
        });
        if status.cleaning {
            return EngineState::Cleaning;
        }
        if let Some(scanning) = scanning {
            return scanning;
        }
        let deferred = status.needs_full_scan && !self.profiles.of(effective).full_scans;
        let due = status.pending > 0 || status.needs_full_scan && !status.held;
        match (effective, deferred, due) {
            (PowerProfile::Off, _, _) => EngineState::Off,
            (profile, true, _) => EngineState::Paused(profile),
            (_, false, true) => EngineState::Updating {
                pending: status.pending,
            },
            (_, false, false) => EngineState::Idle,
        }
    }

    /// Cancels a full scan the user did not ask for.
    pub fn cancel_background_scan(&self) {
        let scan = lock(&self.scan);
        if let Some(scan) = scan.as_ref().filter(|scan| !scan.interactive) {
            scan.control.cancel();
        }
    }
}
