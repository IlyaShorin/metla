//! Power profiles: how much the engine may do, as data in one table
//! (ARCHITECTURE §7). The app picks the profile from the power state; the
//! engine owns what each one means.

use std::time::Duration;

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
    timing: None,
    pause: Duration::ZERO,
    full_scans: false,
    throttled: true,
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
    /// The profiles for a machine with `cores` cores.
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
                timing: Some(timing(1, 30, 256, 4096)),
                pause: Duration::ZERO,
                full_scans: true,
                throttled: false,
            },
            background: Profile {
                threads: 2,
                timing: Some(timing(2, 60, 64, 4096)),
                pause: Duration::from_millis(100),
                full_scans: true,
                throttled: false,
            },
            saver: Profile {
                threads: 1,
                timing: Some(timing(10, 300, 16, 1024)),
                pause: Duration::from_secs(1),
                full_scans: false,
                throttled: true,
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
}
