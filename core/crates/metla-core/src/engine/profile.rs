//! Power profiles: how much the engine may do, as data in one table
//! (ARCHITECTURE §7). The app picks the profile from the power state; the
//! engine owns what each one means.

use std::time::{Duration, Instant};

use super::view::ScanReason;
use crate::scan::Priority;
use crate::watch::Timing;

/// How much CPU and I/O the engine may use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PowerProfile {
    /// The user is waiting: a scan they asked for.
    Interactive,
    /// On AC power.
    #[default]
    Background,
    /// On battery or in Low Power Mode.
    Saver,
    /// Background analysis is switched off: nothing is watched.
    Off,
}

/// What one profile allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Scan threads.
    pub threads: usize,
    /// How long the FSEvents stream holds events back before it hands
    /// them over at once: the longer, the fewer wake-ups.
    pub latency: Duration,
    /// How changes are coalesced; `None` watches nothing.
    pub timing: Option<Timing>,
    /// The pause between two batches of rescans.
    pub pause: Duration,
    /// Whether a full scan the engine needs (no picture, lost history)
    /// may start without the user asking.
    pub full_scans: bool,
    /// Whether the engine's threads run at background priority, with
    /// throttled I/O.
    pub throttled: bool,
    /// The least time from the end of a full scan to the start of a
    /// deferred one (`scan_start`).
    pub scan_gap: Duration,
    /// The quiet a deferred full scan waits for after the last event
    /// that asked for it.
    pub scan_quiet: Duration,
}

/// The table: one row per profile but `Off`, which is fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profiles {
    pub interactive: Profile,
    pub background: Profile,
    pub saver: Profile,
}

/// More scan threads than this gain little and cost much more CPU on the
/// machine measured (PERFORMANCE.md, M4 thread sweep).
pub const INTERACTIVE_THREADS: usize = 8;

/// `Off`: no threads, no watching, no scans of its own.
pub const OFF: Profile = Profile {
    threads: 0,
    latency: Duration::ZERO,
    timing: None,
    pause: Duration::ZERO,
    full_scans: false,
    throttled: true,
    scan_gap: Duration::ZERO,
    scan_quiet: Duration::ZERO,
};

impl Profile {
    /// The priority its scan threads run at.
    pub fn priority(&self) -> Priority {
        match self.throttled {
            true => Priority::Background,
            false => Priority::Normal,
        }
    }
}

impl Profiles {
    /// The profiles for a machine with `cores` cores. Interactive follows
    /// within seconds. Background and Saver apply changes at most once a
    /// minute and every three minutes (the pause between batches), so a
    /// busy desktop wakes the engine rarely while nobody looks
    /// (PERFORMANCE.md, M5 idle); their streams hold events back for
    /// 15 s, the longest macOS was measured to honour.
    pub fn standard(cores: usize) -> Profiles {
        let timing = |settle: u64, cap: u64, batch: usize, max_dirs: usize| Timing {
            settle: Duration::from_secs(settle),
            factor: 2,
            cap: Duration::from_secs(cap),
            batch,
            collapse: 32,
            max_dirs,
        };
        Profiles {
            interactive: Profile {
                threads: cores.saturating_sub(2).clamp(1, INTERACTIVE_THREADS),
                latency: Duration::from_secs(1),
                timing: Some(timing(1, 30, 256, 4096)),
                pause: Duration::ZERO,
                full_scans: true,
                throttled: false,
                scan_gap: Duration::from_secs(600),
                scan_quiet: Duration::from_secs(60),
            },
            background: Profile {
                threads: 2,
                latency: Duration::from_secs(15),
                timing: Some(timing(30, 300, 1024, 4096)),
                pause: Duration::from_secs(60),
                full_scans: true,
                throttled: false,
                scan_gap: Duration::from_secs(1800),
                scan_quiet: Duration::from_secs(60),
            },
            saver: Profile {
                threads: 1,
                latency: Duration::from_secs(15),
                timing: Some(timing(60, 900, 256, 1024)),
                pause: Duration::from_secs(180),
                full_scans: false,
                throttled: true,
                scan_gap: Duration::from_secs(1800),
                scan_quiet: Duration::from_secs(60),
            },
        }
    }

    pub fn of(&self, profile: PowerProfile) -> Profile {
        match profile {
            PowerProfile::Interactive => self.interactive,
            PowerProfile::Background => self.background,
            PowerProfile::Saver => self.saver,
            PowerProfile::Off => OFF,
        }
    }

    /// The profile work follows under the power profile `power`: its
    /// own, or while someone looks (`attention`) and it watches,
    /// `Interactive`'s threads, timing and priority with `power`'s say on
    /// full scans. A pure table: the threads only apply its priority.
    pub fn in_force(&self, power: PowerProfile, attention: bool) -> Profile {
        let power = self.of(power);
        match (attention, power.timing) {
            (true, Some(_)) => Profile {
                full_scans: power.full_scans,
                ..self.interactive
            },
            _ => power,
        }
    }

    /// The profile the engine's own full scan runs with under `power`:
    /// its own, but the first scan of a root (no `picture` yet) under
    /// `Interactive` or `Background` takes `Interactive`'s threads and
    /// priority, since nothing can be shown until it ends. Timing, pause,
    /// latency and the say on full scans stay `power`'s; `Saver` keeps
    /// its one thread and `Off` scans nothing. A pure table.
    pub fn full_scan(&self, power: PowerProfile, picture: bool) -> Profile {
        let own = self.of(power);
        match (power, picture) {
            (PowerProfile::Interactive | PowerProfile::Background, false) => Profile {
                threads: self.interactive.threads,
                throttled: self.interactive.throttled,
                ..own
            },
            _ => own,
        }
    }
}

/// When the engine's own full scan may start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanStart {
    Now,
    NotBefore(Instant),
    /// The profile starts none (`Saver`, `Off`).
    Never,
}

/// When the engine's own full scan for `reason` may start under
/// `profile`. At once without a `picture`, when the user asked, and for
/// a history found lost at opening. Otherwise (events dropped, too many
/// changes, a rescan too large or failed, a stopped scan done again) not
/// before the profile's `scan_gap` after the last full scan `ended` (or
/// the engine opened) and its `scan_quiet` after the last event that
/// asked for it (`trouble`): a busy disk cannot make the engine scan
/// everything again and again. A pure table.
pub fn scan_start(
    reason: ScanReason,
    picture: bool,
    ended: Instant,
    trouble: Option<Instant>,
    now: Instant,
    profile: &Profile,
) -> ScanStart {
    let urgent = matches!(
        reason,
        ScanReason::NoPicture | ScanReason::UserAsked | ScanReason::HistoryLost
    );
    if !profile.full_scans {
        return ScanStart::Never;
    }
    if urgent || !picture {
        return ScanStart::Now;
    }
    let after_gap = ended + profile.scan_gap;
    let after_quiet = trouble.map_or(after_gap, |at| at + profile.scan_quiet);
    let at = after_gap.max(after_quiet);
    match now >= at {
        true => ScanStart::Now,
        false => ScanStart::NotBefore(at),
    }
}
