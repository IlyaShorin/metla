//! `bench churn` and `bench refresh`: what following changes costs, on a
//! fixture only.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use metla_core::domain::{DirRecord, UnixTime};
use metla_core::engine::{Engine, EngineState};
use metla_core::fs::{thread_cpu, usage};
use metla_core::index::{IndexReader, IndexWriter};
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
    let freshness = engine.snapshot().map(|o| o.freshness);
    freshness.is_ok_and(|f| f.state == EngineState::Idle && f.pending == 0 && !f.needs_full_scan)
}

/// What a churn wrote.
struct Churned {
    files: u64,
    cpu: Duration,
}

/// Creates and deletes files in bursts below `dir` until `stop`.
fn churn(dir: PathBuf, per_burst: usize, stop: Arc<AtomicBool>) -> Churned {
    let start = thread_cpu();
    let mut files = 0;
    let mut burst = 0;
    while !stop.load(Ordering::Acquire) {
        let here = dir.join(format!("b{}", burst % 16));
        let _ = fs::remove_dir_all(&here);
        let _ = fs::create_dir_all(&here);
        for i in 0..per_burst {
            let _ = fs::write(here.join(format!("f{i}")), b"churn");
        }
        files += per_burst as u64;
        burst += 1;
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(&dir);
    Churned {
        files,
        cpu: thread_cpu() - start,
    }
}

/// Every row of a picture keyed by path, without the ids, which differ
/// between a picture kept up to date and a fresh one.
fn rows(index: &Path) -> Result<Vec<(PathBuf, DirRecord)>, String> {
    let mut reader = IndexReader::open(index).map_err(|e| e.to_string())?;
    let mut rows = reader.read(|snap| snap.rows()).map_err(|e| e.to_string())?;
    for (_, row) in &mut rows {
        row.id = metla_core::domain::DirId(0);
        row.parent = None;
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(rows)
}

/// Whether the engine's picture is what a fresh scan of the root writes.
fn exactly_right(place: &Place) -> Result<bool, String> {
    let (home, user) = home_and_user(place.home.as_deref())?;
    let root = ScanRoot::new(&place.root).map_err(|e| e.to_string())?;
    let scope = ScanScope::detect(&root).map_err(|e| e.to_string())?;
    let options = ScanOptions::new(root, scope)
        .rules(Arc::new(RuleSet::new(&home, &user)))
        .threads(2);
    let fresh = place.data_dir.join("fresh.sqlite");
    let mut writer = IndexWriter::open(&fresh).map_err(|e| e.to_string())?;
    let session = writer.start_scan(UnixTime(0)).map_err(|e| e.to_string())?;
    let summary = run(&options, &ScanControl::new(0), &|r| session.send(r));
    session
        .finish(&summary.map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    drop(writer);
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
    fixture(&place.root)?;
    if place.data_dir.starts_with(&place.root) {
        return Err("the data directory must lie outside the root".into());
    }
    let (engine, _) = open(place, false)?;
    engine.start_scan().map_err(|e| e.to_string())?;
    waited(Duration::from_secs(600), || idle(&engine)).ok_or("the first scan did not end")?;
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
    let engine_cpu = (after.1.user + after.1.system) - (before.1.user + before.1.system);
    let (tasks, batches) = (
        after.0.tasks - before.0.tasks,
        after.0.batches - before.0.batches,
    );
    Ok(format!(
        "churn: {} files created and deleted in {}, churn CPU {}\n\
         engine: {} batches, {} rescans, {} full scans, {} events; process {}\n\
         engine CPU without the churn thread: {}\n\
         after the churn: idle after {}; picture {} (checked against a full scan {} after the churn ended)\n",
        count(churned.files * 2),
        seconds(length),
        seconds(churned.cpu),
        batches,
        tasks,
        after.0.full_scans - before.0.full_scans,
        after.0.events - before.0.events,
        spent(&before.1, &after.1),
        seconds(engine_cpu.saturating_sub(churned.cpu)),
        seconds(quiet),
        match right {
            true => "exactly right",
            false => "NOT the same as a full scan",
        },
        seconds(to_right),
    ))
}

/// How long a small write takes to show in the engine's totals, three
/// times, with `profile`.
pub fn bench_refresh(place: &Place) -> Result<String, String> {
    fixture(&place.root)?;
    let (engine, _) = open(place, false)?;
    engine.start_scan().map_err(|e| e.to_string())?;
    waited(Duration::from_secs(600), || idle(&engine)).ok_or("the first scan did not end")?;
    let probe = place.root.join("refresh-probe");
    fs::create_dir_all(&probe).map_err(|e| e.to_string())?;
    waited(Duration::from_secs(600), || idle(&engine)).ok_or("never idle")?;
    let mut lines = format!("refresh with {:?}:", place.profile);
    for round in 0..3 {
        let total = || {
            engine
                .snapshot()
                .ok()
                .and_then(|o| o.totals)
                .map(|t| t.bytes)
        };
        let before = total();
        fs::write(probe.join(format!("w{round}")), vec![9u8; 1 << 20])
            .map_err(|e| e.to_string())?;
        let shown = waited(Duration::from_secs(900), || total() != before);
        let shown = shown.map_or("not within 15 min".to_owned(), seconds);
        lines.push_str(&format!(" {shown}"));
        waited(Duration::from_secs(900), || idle(&engine)).ok_or("never idle")?;
    }
    let _ = fs::remove_dir_all(&probe);
    lines.push('\n');
    Ok(lines)
}
