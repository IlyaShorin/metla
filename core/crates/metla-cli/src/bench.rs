//! `bench churn` and `bench refresh`: what following changes costs, on a
//! fixture only.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use metla_core::domain::{DirId, DirRecord};
use metla_core::engine::{DirRef, Engine};
use metla_core::fs::{now, thread_cpu, usage};
use metla_core::index::{IndexReader, IndexWriter, Page};
use metla_core::rules::RuleSet;
use metla_core::scan::{ScanControl, ScanOptions, ScanRoot, ScanScope, run};

use crate::commands::home_and_user;
use crate::format::{count, seconds};
use crate::live::{Place, fixture, open, spent};

/// Waits until `done` holds or `limit` passes; how long it took.
fn waited(limit: Duration, done: impl Fn() -> bool) -> Option<Duration> {
    let started = Instant::now();
    while !done() {
        if started.elapsed() > limit {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
    Some(started.elapsed())
}

/// The engine has nothing left to do and its picture is complete.
fn idle(engine: &Engine) -> bool {
    engine.freshness().is_ok_and(|f| f.is_current())
}

/// What a churn did: its bursts, the files it wrote, and its CPU time.
#[derive(Default)]
struct Churned {
    bursts: u64,
    files: u64,
    cpu: Duration,
}

/// Writes files in bursts below `dir` until `stop`: each burst empties one
/// of 16 directories and writes `per_burst` files into it. Runs on a
/// thread of its own, whose CPU time is therefore the churn's.
fn churn(dir: PathBuf, per_burst: usize, stop: Arc<AtomicBool>) -> Churned {
    let mut churned = Churned::default();
    while !stop.load(Ordering::Acquire) {
        let here = dir.join(format!("b{}", churned.bursts % 16));
        let _ = fs::remove_dir_all(&here);
        let _ = fs::create_dir_all(&here);
        for i in 0..per_burst {
            let written = fs::write(here.join(format!("f{i}")), b"churn");
            churned.files += u64::from(written.is_ok());
        }
        churned.bursts += 1;
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(&dir);
    Churned {
        cpu: thread_cpu(),
        ..churned
    }
}

/// Every row of a picture keyed by path, without the ids, which differ
/// between a picture kept up to date and a fresh one.
fn rows(index: &Path) -> Result<Vec<(PathBuf, DirRecord)>, String> {
    let mut reader = IndexReader::open(index).map_err(|e| e.to_string())?;
    let mut rows = reader.read(|snap| snap.rows()).map_err(|e| e.to_string())?;
    for (_, row) in &mut rows {
        // Any one id: the paths tell the rows apart.
        row.id = DirId(i64::MIN);
        row.parent = None;
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(rows)
}

/// How the engine scans the root, for a scan of its own.
fn options(place: &Place) -> Result<ScanOptions, String> {
    let (home, user) = home_and_user(place.home.as_deref())?;
    let root = ScanRoot::new(&place.root).map_err(|e| e.to_string())?;
    let scope = ScanScope::detect(&root).map_err(|e| e.to_string())?;
    Ok(ScanOptions::new(root, scope)
        .rules(Arc::new(RuleSet::new(&home, &user)))
        .threads(2))
}

/// Scans the root into a new index at `fresh`.
fn scan_afresh(place: &Place, fresh: &Path) -> Result<(), String> {
    let options = options(place)?;
    let mut writer = IndexWriter::open(fresh).map_err(|e| e.to_string())?;
    let session = writer.start_scan(now()).map_err(|e| e.to_string())?;
    // No live list of the largest entries: nobody watches this scan.
    let summary = run(&options, &ScanControl::new(usize::MIN), &|r| {
        session.send(r)
    });
    session
        .finish(&summary.map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// Whether the engine's picture is what a fresh scan of the root writes.
fn exactly_right(place: &Place) -> Result<bool, String> {
    let fresh = place.data_dir.join("fresh.sqlite");
    scan_afresh(place, &fresh)?;
    let kept = rows(&place.data_dir.join(metla_core::engine::INDEX_FILE))?;
    let same = kept == rows(&fresh)?;
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", fresh.display()));
    }
    Ok(same)
}

/// Churns below `<root>/churn` for `length` while the engine follows the
/// root; then measures how long until its picture is exactly right.
pub fn bench_churn(place: &Place, length: Duration, per_burst: usize) -> Result<String, String> {
    if place.data_dir.starts_with(&place.root) {
        return Err("the data directory must lie outside the root".into());
    }
    let engine = scanned(place)?;
    let before = (engine.counters(), usage());
    let stop = Arc::new(AtomicBool::new(false));
    let dir = place.root.join("churn");
    let churner = thread::spawn({
        let stop = Arc::clone(&stop);
        move || churn(dir, per_burst, stop)
    });
    thread::sleep(length);
    stop.store(true, Ordering::Release);
    let churned = churner.join().map_err(|_| "the churn failed")?;
    let ended = Instant::now();
    let quiet = waited(Duration::from_secs(600), || idle(&engine)).ok_or("never idle")?;
    let after = (engine.counters(), usage());
    let right = exactly_right(place)?;
    let to_right = ended.elapsed();
    let did = after.0.since(&before.0);
    let engine_cpu = after.1.since(&before.1).cpu();
    let picture = if right {
        "exactly right"
    } else {
        "NOT the same as a full scan"
    };
    Ok(format!(
        "churn: {} files created and deleted in {} bursts in {}, churn CPU {}\n\
         engine: {} batches, {} rescans, {} full scans, {} events; process {}\n\
         engine CPU without the churn thread: {}\n\
         after the churn: idle after {}; picture {} (checked against a full scan {} after the churn ended)\n",
        count(churned.files * 2),
        churned.bursts,
        seconds(length),
        seconds(churned.cpu),
        did.batches,
        did.tasks,
        did.full_scans,
        did.events,
        spent(&before.1, &after.1),
        seconds(engine_cpu.saturating_sub(churned.cpu)),
        seconds(quiet),
        picture,
        seconds(to_right),
    ))
}

/// How long a small write takes to show in the engine's totals, three
/// times, with `profile`.
pub fn bench_refresh(place: &Place) -> Result<String, String> {
    let engine = scanned(place)?;
    let probe = place.root.join("refresh-probe");
    fs::create_dir_all(&probe).map_err(|e| e.to_string())?;
    waited(Duration::from_secs(600), || idle(&engine)).ok_or("never idle")?;
    let mut lines = format!("refresh with {:?}:", place.profile);
    for round in 0..3 {
        let file = probe.join(format!("w{round}"));
        let shown = shown_after_write(&engine, &place.root, &file)?;
        lines.push_str(&format!(" {shown}"));
    }
    let _ = fs::remove_dir_all(&probe);
    lines.push('\n');
    Ok(lines)
}

/// The engine on a fixture root, once its first full scan has ended.
fn scanned(place: &Place) -> Result<Engine, String> {
    fixture(&place.root)?;
    let (engine, _) = open(place, false)?;
    engine.start_scan().map_err(|e| e.to_string())?;
    waited(Duration::from_secs(600), || idle(&engine)).ok_or("the first scan did not end")?;
    Ok(engine)
}

/// Writes 1 MiB to `file`; how long until the root's total shows it.
/// Returns once the engine is idle again.
fn shown_after_write(engine: &Engine, root: &Path, file: &Path) -> Result<String, String> {
    let root = DirRef::Path(root.to_path_buf());
    // No entries: only the directory's own totals are read.
    let none = Page {
        offset: usize::MIN,
        limit: usize::MIN,
    };
    let total = || engine.children(&root, none).ok().map(|f| f.dir.tree.bytes);
    let before = total();
    fs::write(file, vec![9u8; 1 << 20]).map_err(|e| e.to_string())?;
    let shown = waited(Duration::from_secs(900), || total() != before);
    waited(Duration::from_secs(900), || idle(engine)).ok_or("never idle")?;
    let precise = |shown: Duration| format!("{:.3} s", shown.as_secs_f64());
    Ok(shown.map_or("not within 15 min".to_owned(), precise))
}
