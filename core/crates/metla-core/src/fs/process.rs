use std::mem::MaybeUninit;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::sys::check;
use crate::domain::{Bytes, UnixTime};

/// `OPEN_MAX` from `<sys/syslimits.h>`: the largest soft limit macOS
/// accepts for `RLIMIT_NOFILE`.
const OPEN_MAX: u64 = 10_240;

/// The wall clock, in whole seconds.
pub fn now() -> UnixTime {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    UnixTime(i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
}

/// What the process has used so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub user: Duration,
    pub system: Duration,
    /// Context switches, voluntary (the thread waited) and not: a
    /// measure of how often the process woke up. macOS leaves the
    /// voluntary count at 0 and counts every switch as involuntary
    /// (measured); both are summed in case it ever reports them apart.
    pub switches: u64,
    /// The peak resident set size.
    pub peak_rss: Bytes,
}

impl Usage {
    /// CPU time, user and system.
    pub fn cpu(&self) -> Duration {
        self.user + self.system
    }

    /// What was used between `earlier` and now; the peak stays this
    /// one's.
    pub fn since(&self, earlier: &Usage) -> Usage {
        Usage {
            user: self.user.saturating_sub(earlier.user),
            system: self.system.saturating_sub(earlier.system),
            switches: self.switches.saturating_sub(earlier.switches),
            peak_rss: self.peak_rss,
        }
    }
}

fn rusage() -> libc::rusage {
    let mut usage = MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` is writable rusage storage; RUSAGE_SELF cannot fail.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    // SAFETY: zero-initialised and then filled by getrusage.
    unsafe { usage.assume_init() }
}

fn duration(time: libc::timeval) -> Duration {
    let micros = time.tv_usec.unsigned_abs();
    Duration::from_secs(time.tv_sec.unsigned_abs()) + Duration::from_micros(u64::from(micros))
}

/// The process's CPU time, context switches and peak RSS so far.
pub fn usage() -> Usage {
    let usage = rusage();
    Usage {
        user: duration(usage.ru_utime),
        system: duration(usage.ru_stime),
        switches: [usage.ru_nvcsw, usage.ru_nivcsw]
            .into_iter()
            .sum::<libc::c_long>()
            .unsigned_abs(),
        peak_rss: Bytes(usage.ru_maxrss.unsigned_abs()),
    }
}

/// The process's peak resident set size (macOS reports it in bytes).
pub fn peak_rss() -> Bytes {
    usage().peak_rss
}

/// The CPU time the calling thread has used.
pub fn thread_cpu() -> Duration {
    let mut time = MaybeUninit::<libc::timespec>::zeroed();
    // SAFETY: `time` is writable timespec storage.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, time.as_mut_ptr()) };
    // SAFETY: zero-initialised and then filled by clock_gettime.
    let time = unsafe { time.assume_init() };
    Duration::new(
        time.tv_sec.unsigned_abs(),
        time.tv_nsec.unsigned_abs() as u32,
    )
}

/// The soft open-file limit to ask for, if the current one is lower: as
/// high as the hard limit allows, at most `OPEN_MAX`.
pub fn raised_open_file_limit(soft: u64, hard: u64) -> Option<u64> {
    let wanted = hard.min(OPEN_MAX);
    (soft < wanted).then_some(wanted)
}

/// Raises the soft open-file limit per [`raised_open_file_limit`]: a walk
/// holds one descriptor per directory on its path, and the default soft
/// limit of 256 is shallower than real trees. Returns the soft limit now in
/// force.
pub fn raise_open_file_limit() -> u64 {
    let mut limit = MaybeUninit::<libc::rlimit>::zeroed();
    // SAFETY: `limit` is writable rlimit storage.
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) };
    // SAFETY: zero-initialised and then filled by getrlimit.
    let mut limit = unsafe { limit.assume_init() };
    let Some(wanted) = raised_open_file_limit(limit.rlim_cur, limit.rlim_max) else {
        return limit.rlim_cur;
    };
    let soft = limit.rlim_cur;
    limit.rlim_cur = wanted;
    // SAFETY: `limit` is a valid rlimit within the hard limit.
    let raised = check(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) });
    raised.map_or(soft, |_| wanted)
}

/// Moves the calling thread to background priority: its CPU time and its
/// disk I/O yield to everything else (`PRIO_DARWIN_BG`). True when the
/// system accepted it.
pub fn background_thread() -> bool {
    // SAFETY: changes only the calling thread's own priority.
    let set = unsafe { libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG) };
    check(set).is_ok()
}
