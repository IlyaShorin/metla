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
    /// Someone looks at the picture: changes are followed as
    /// `Interactive` follows them.
    pub attention: bool,
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

    /// The profile of the power state: it decides the engine's own full
    /// scans.
    pub fn power(&self) -> Profile {
        self.profiles.of(self.effective())
    }

    /// The profile changes are followed with: `Interactive` while someone
    /// looks, if anything is followed at all.
    pub fn profile(&self) -> Profile {
        let power = self.power();
        let attention = lock(&self.status).attention;
        match (attention, power.timing) {
            (true, Some(_)) => Profile {
                full_scans: power.full_scans,
                ..self.profiles.interactive
            },
            _ => power,
        }
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
        if status.cleaning {
            return EngineState::Cleaning;
        }
        let scanning = lock(&self.scan).as_ref().map(|scan| EngineState::Scanning {
            progress: scan.control.progress(),
            interactive: scan.interactive,
        });
        scanning.unwrap_or_else(|| self.between_scans(status))
    }

    /// What the engine is doing while it neither scans nor cleans.
    fn between_scans(&self, status: Status) -> EngineState {
        let effective = self.effective();
        let deferred = status.needs_full_scan && !self.power().full_scans;
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
