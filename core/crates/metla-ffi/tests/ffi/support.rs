//! A synthetic root marked as a fixture, a data directory beside it,
//! and waiting with a deadline of its own.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metla_core::clean::FIXTURE_MARKER;
use metla_core::engine::{Changed, EngineState, PowerProfile};
use metla_ffi::{Engine, EngineObserver, EngineOptions, TrashHost, TrashOutcome};

pub struct Place {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub data: PathBuf,
}

impl Place {
    /// A fixture holding `files` (paths below the root, `~` for home).
    pub fn new(files: &[&str]) -> Place {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let root = base.join("root");
        for file in files {
            let path = root.join(file.replacen('~', "home", 1));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "x".repeat(5000)).unwrap();
        }
        fs::create_dir_all(root.join("home")).unwrap();
        fs::write(root.join(FIXTURE_MARKER), "").unwrap();
        Place {
            _dir: dir,
            data: base.join("data"),
            root,
        }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative.replacen('~', "home", 1))
    }

    pub fn options(&self) -> EngineOptions {
        EngineOptions {
            root: text(&self.root),
            home: text(&self.root.join("home")),
            user: "tester".into(),
            data_dir: text(&self.data),
            profile: PowerProfile::Background,
            follow: false,
            attention: false,
        }
    }

    /// The engine, its first scan done.
    pub fn open(&self) -> Arc<Engine> {
        let engine = Engine::open(self.options(), Arc::new(Refusing)).unwrap();
        until(|| {
            let freshness = engine.freshness().unwrap();
            freshness.full_scan.is_some() && freshness.state == EngineState::Idle
        });
        engine
    }
}

pub fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

/// Waits until `done` holds, at most ten seconds.
pub fn until(done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(10), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A Trash that refuses everything.
pub struct Refusing;

impl TrashHost for Refusing {
    fn trash(&self, _: String) -> TrashOutcome {
        TrashOutcome::Refused {
            reason: "no Trash here".into(),
        }
    }
}

/// Everything the observer was told.
#[derive(Default)]
pub struct Told(pub Mutex<Vec<Changed>>);

impl EngineObserver for Told {
    fn changed(&self, topic: Changed) {
        self.0.lock().unwrap().push(topic);
    }
}

pub const HOME: &[&str] = &[
    "~/Library/Caches/com.example.app/blob",
    "~/Library/Caches/org.example.other/blob",
    "~/Library/Logs/com.example.app/today.log",
    "~/Documents/notes.txt",
];
