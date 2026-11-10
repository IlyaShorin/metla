//! Commands that run the engine: `watch` follows a root and prints every
//! batch the engine applies; `bench` measures opening, churn and the
//! latency of a small change. The engine they open can never clean: the
//! CLI has no production capability, and these commands ask for no
//! cleanup.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metla_core::clean::{FIXTURE_MARKER, TrashHost};
use metla_core::domain::UnixTime;
use metla_core::engine::{
    Changed, Completeness, Counters, Engine, EngineConfig, EngineObserver, EngineState, Freshness,
    PowerProfile, Profiles, Watching,
};
use metla_core::fs::{Usage, now, usage};
use metla_core::git::Git;

use crate::commands::{cores, home_and_user};
use crate::format::{ago, bytes, count, seconds};

/// Where and how an engine runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub root: PathBuf,
    pub data_dir: PathBuf,
    /// `None`: the current user's home directory.
    pub home: Option<PathBuf>,
    /// The FSEvents latency.
    pub latency: Duration,
    /// Every profile's settle time instead of the standard table's (its
    /// cap then eight times as long, no pause between batches).
    pub settle: Option<Duration>,
    pub profile: PowerProfile,
}

/// The standard profiles, or all of them settling after `settle`.
pub fn profiles(settle: Option<Duration>) -> Profiles {
    let mut profiles = Profiles::standard(cores());
    let Some(settle) = settle else {
        return profiles;
    };
    let rows = [
        &mut profiles.interactive,
        &mut profiles.background,
        &mut profiles.saver,
    ];
    for profile in rows {
        profile.pause = Duration::ZERO;
        if let Some(timing) = &mut profile.timing {
            timing.settle = settle;
            timing.cap = settle * 8;
        }
    }
    profiles
}

/// The CLI never moves anything to the Trash.
struct NoTrash;

impl TrashHost for NoTrash {
    fn trash(&self, _: &Path) -> Result<(), String> {
        Err("the CLI's engine moves nothing to the Trash".into())
    }
}

/// Hands the engine's notifications to the thread that prints.
struct Relay(Mutex<Sender<Changed>>);

impl EngineObserver for Relay {
    fn changed(&self, what: Changed) {
        let _ = self.0.lock().map(|sender| sender.send(what));
    }
}

/// Opens an engine on `place`, its notifications on the receiver;
/// `manual` opens it without an event stream.
pub fn open(place: &Place, manual: bool) -> Result<(Engine, Receiver<Changed>), String> {
    let watching = match manual {
        true => Watching::Manual,
        false => Watching::FsEvents {
            latency: place.latency,
        },
    };
    let (home, user) = home_and_user(place.home.as_deref())?;
    let config = EngineConfig {
        root: place.root.clone(),
        home,
        user,
        data_dir: place.data_dir.clone(),
        git: Git::system(),
        production: None,
        trash: Arc::new(NoTrash),
        watching,
        profiles: profiles(place.settle),
        profile: place.profile,
    };
    let engine = Engine::open(config).map_err(|e| e.to_string())?;
    let (sender, receiver) = mpsc::channel();
    engine.set_observer(Arc::new(Relay(Mutex::new(sender))));
    Ok((engine, receiver))
}

/// The state, in a word or two.
pub fn state_text(state: &EngineState) -> String {
    match state {
        EngineState::Idle => "idle".into(),
        EngineState::Scanning { progress, .. } => {
            format!("scanning, {} directories", count(progress.dirs))
        }
        EngineState::Updating { pending } => format!("updating, {pending} pending"),
        EngineState::Cleaning => "cleaning".into(),
        EngineState::Paused(profile) => format!("paused ({profile:?})"),
        EngineState::Off => "off".into(),
    }
}

/// One line: what the engine has done and how fresh its picture is.
pub fn freshness_line(now: UnixTime, counters: &Counters, freshness: &Freshness) -> String {
    let updated = freshness
        .updated
        .map_or("never".to_owned(), |t| format!("{} ago", ago(now.0 - t.0)));
    let picture = match freshness.picture {
        Completeness::Missing => "no picture",
        Completeness::Partial => "partial",
        Completeness::Complete => "complete",
    };
    format!(
        "batch {}  tasks {}  events {}  pending {}  {}  updated {updated}  {picture}{}",
        counters.batches,
        counters.tasks,
        counters.events,
        freshness.pending,
        state_text(&freshness.state),
        match freshness.is_current() {
            true => "",
            false => "  (may be out of date)",
        }
    )
}

/// What a process spent between two samples.
pub fn spent(before: &Usage, after: &Usage) -> String {
    format!(
        "CPU {} user + {} system, {} context switches, peak RSS {}",
        seconds(after.since(before).user),
        seconds(after.since(before).system),
        count(after.since(before).switches),
        bytes(after.peak_rss)
    )
}

/// Follows `place` for `limit` (forever without one), printing a line
/// after every batch; a root without a picture is scanned first.
pub fn watch(place: &Place, limit: Option<Duration>, out: &mut dyn Write) -> Result<(), String> {
    let (engine, notes) = open(place, false)?;
    let before = usage();
    let started = Instant::now();
    if engine
        .snapshot()
        .map_err(|e| e.to_string())?
        .totals
        .is_none()
    {
        engine.start_scan().map_err(|e| e.to_string())?;
    }
    let _ = writeln!(
        out,
        "watching {} ({:?})",
        place.root.display(),
        place.profile
    );
    while let Some(left) = left(limit, started) {
        if let Ok(Changed::Picture) = notes.recv_timeout(left) {
            let freshness = engine.freshness().map_err(|e| e.to_string())?;
            let line = freshness_line(now(), &engine.counters(), &freshness);
            let _ = writeln!(out, "{line}");
        }
    }
    let counters = engine.counters();
    let _ = writeln!(
        out,
        "watched {}: {} batches, {} tasks, {} full scans, {} events, {} engine wake-ups; {}",
        seconds(started.elapsed()),
        counters.batches,
        counters.tasks,
        counters.full_scans,
        counters.events,
        counters.wakeups,
        spent(&before, &usage())
    );
    Ok(())
}

/// Time left before `limit`; `None` once it is reached. Without a
/// limit, an hour at a time.
fn left(limit: Option<Duration>, started: Instant) -> Option<Duration> {
    let limit = limit.unwrap_or(Duration::MAX);
    let left = limit.saturating_sub(started.elapsed());
    (!left.is_zero()).then(|| left.min(Duration::from_secs(3600)))
}

/// Refuses a root that is not a fixture: churn and refresh write there.
pub fn fixture(root: &Path) -> Result<(), String> {
    match root.join(FIXTURE_MARKER).is_file() {
        true => Ok(()),
        false => Err(format!(
            "{} has no {FIXTURE_MARKER}: benchmarks write only into fixtures",
            root.display()
        )),
    }
}

/// Opening the engine on a saved picture and reading the first snapshot.
pub fn bench_open(place: &Place) -> Result<String, String> {
    let before = usage();
    let started = Instant::now();
    let quiet = Place {
        profile: PowerProfile::Off,
        ..place.clone()
    };
    let (engine, _) = open(&quiet, true)?;
    let overview = engine.snapshot().map_err(|e| e.to_string())?;
    let elapsed = started.elapsed();
    let dirs = overview.totals.map_or(0, |t| t.dirs);
    Ok(format!(
        "open + first snapshot: {} for {} directories; {}\n",
        seconds(elapsed),
        count(dirs),
        spent(&before, &usage())
    ))
}
