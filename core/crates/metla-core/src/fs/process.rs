use std::mem::MaybeUninit;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// The process's peak resident set size (macOS reports it in bytes).
pub fn peak_rss() -> Bytes {
    let mut usage = MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` is writable rusage storage; RUSAGE_SELF cannot fail.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    // SAFETY: zero-initialised and then filled by getrusage.
    Bytes(unsafe { usage.assume_init() }.ru_maxrss.unsigned_abs())
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
