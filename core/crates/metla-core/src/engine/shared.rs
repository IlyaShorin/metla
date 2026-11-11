//! What the API threads and the scheduler share: the observer, what the
//! engine is doing, the settings, the cleaner and the results of work.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use super::profile::{PowerProfile, Profile, Profiles};
use super::settings::Settings;
use super::view::{Changed, Counters, EngineObserver, EngineState, Failure, ScanReason};
use super::worktrees::Statuses;
use crate::clean::{CleanControl, CleanError, Cleaner, TrashHost};
use crate::domain::{Candidate, CleanupReport, Recency};
use crate::fs::now;
use crate::git::{Git, WorktreeSettings};
use crate::index::PictureId;
use crate::rules::RuleSet;
use crate::scan::{DirObserver, ScanControl};

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The observer slot, shared with the worktree worker.
pub type ObserverSlot = Arc<Mutex<Option<Arc<dyn EngineObserver>>>>;

/// A full scan that runs.
pub struct RunningScan {
    pub control: Arc<ScanControl>,
    pub interactive: bool,
    /// A picture exists to fall back on: the scan may give way to a
    /// cleanup.
    pub yields: bool,
}

/// What the last cleanup may have removed, and until when the events
/// below it carry no news (its own rescans read those directories).
#[derive(Clone, Debug, Default)]
pub struct Quiet {
    pub paths: Vec<PathBuf>,
    pub until: Option<Instant>,
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
    /// Why the last full scan or update failed; no full scan starts on
    /// its own until the user asks for one.
    pub failure: Option<Failure>,
    /// Cleanups asked for and not started yet: nothing else starts
    /// meanwhile.
    pub waiting_cleanups: u32,
    /// Why the running or last full scan was started, or why the one
    /// needed is.
    pub reason: Option<ScanReason>,
}

impl Status {
    /// Whether the engine may start the full scan it needs on its own.
    pub fn scan_due(&self) -> bool {
        self.needs_full_scan && !self.held && self.failure.is_none()
    }
}

pub struct Shared {
    pub root: PathBuf,
    pub home: PathBuf,
    pub rules: Arc<RuleSet>,
    pub git: Git,
    pub profiles: Profiles,
    pub trash: Arc<dyn TrashHost + Send + Sync>,
    /// Shown every directory a scan reads (EngineConfig::dirs).
    pub dirs: Option<Arc<dyn DirObserver>>,
    pub observer: ObserverSlot,
    pub status: Mutex<Status>,
    pub scan: Mutex<Option<RunningScan>>,
    pub cleanup: Mutex<Option<Arc<CleanControl>>>,
    pub quiet: Mutex<Quiet>,
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
        let attention = lock(&self.status).attention;
        self.profiles.in_force(self.effective(), attention)
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
        let due = status.pending > 0 || status.scan_due();
        match (effective, deferred, due, status.failure) {
            (PowerProfile::Off, ..) => EngineState::Off,
            (profile, true, ..) => EngineState::Paused(profile),
            (_, false, true, _) => EngineState::Updating {
                pending: status.pending,
            },
            (_, false, false, Some(failure)) => EngineState::Failed(failure),
            (_, false, false, None) => EngineState::Idle,
        }
    }

    /// Cancels a full scan the user did not ask for.
    /// What is recent now, by the settings.
    pub fn recency(&self) -> Recency {
        Recency::new(now(), lock(&self.settings).recent_age)
    }

    /// Computes the statuses of `worktrees` of `picture` in the
    /// background, with the settings and at the priority in force, when
    /// the list of `picture` was asked for.
    pub fn compute_statuses(&self, picture: PictureId, worktrees: Vec<Candidate>) {
        if worktrees.is_empty() {
            return;
        }
        let settings = WorktreeSettings {
            home: self.home.clone(),
            stale_after: lock(&self.settings).stale_after,
        };
        let priority = self.profile().priority();
        (self.statuses).compute(picture, worktrees, now(), settings, priority);
    }

    /// A cleanup was asked for: nothing else starts before it, and a
    /// running full scan with a picture to fall back on gives way to it.
    /// The scheduler checks the count after it registers a scan, so a
    /// scan that starts meanwhile gives way too.
    pub fn want_cleanup(&self) {
        self.update(|s| s.waiting_cleanups += 1);
        if let Some(scan) = lock(&self.scan).as_ref().filter(|scan| scan.yields) {
            scan.control.cancel();
        }
    }

    pub fn cleanup_waiting(&self) -> bool {
        lock(&self.status).waiting_cleanups > 0
    }

    /// The paths whose events carry no news at `now`: the items of the
    /// running cleanup, and those of the last one until its quiet ends.
    pub fn quiet_paths(&self, now: Instant) -> Vec<PathBuf> {
        let running = lock(&self.cleanup).as_ref().map(|c| c.begun());
        let mut paths = running.unwrap_or_default();
        let last = lock(&self.quiet);
        if last.until.is_some_and(|until| now < until) {
            paths.extend(last.paths.iter().cloned());
        }
        paths
    }

    /// Only a full scan will make the picture right, for `reason`; a scan
    /// already needed keeps its first reason.
    pub fn need_full_scan(&self, reason: ScanReason) {
        self.update(|s| {
            if !s.needs_full_scan {
                s.reason = Some(reason);
            }
            s.needs_full_scan = true;
        });
    }

    pub fn cancel_background_scan(&self) {
        let scan = lock(&self.scan);
        if let Some(scan) = scan.as_ref().filter(|scan| !scan.interactive) {
            scan.control.cancel();
        }
    }
}
