//! The commands: gather data from the core, hand it to `report`.

use std::path::{Path, PathBuf};

use metla_core::domain::{Bytes, DirState, EntryKind};
use metla_core::fs::{DirHandle, list_dir, now, peak_rss, volume};
use metla_core::index::{IndexError, IndexReader, IndexWriter, Snapshot};
use metla_core::scan::{ScanControl, ScanOptions, ScanRoot, ScanScope, run};

use crate::report::{Listed, ScanReport, VolumeUse};

/// How many shallow directories the live tree tracks.
const LIVE_LIMIT: usize = 4096;

/// The `Interactive` power profile: all cores but two, at least one (also
/// when the core count is unknown).
pub fn default_threads(cores: usize) -> usize {
    cores.saturating_sub(2).max(1)
}

/// The number of cores, or 0 when it cannot be known.
fn cores() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or_default()
}

/// Scans `root` into `index`.
pub fn scan(root: &Path, index: &Path, threads: Option<usize>) -> Result<ScanReport, String> {
    let root = ScanRoot::new(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let scope = ScanScope::detect(&root).map_err(|e| e.to_string())?;
    let threads = threads.unwrap_or_else(|| default_threads(cores()));
    let options = ScanOptions::new(root, scope).threads(threads);
    let writer = IndexWriter::start_scan(index, now()).map_err(|e| e.to_string())?;
    let control = ScanControl::new(LIVE_LIMIT);
    let summary = run(&options, &control, &|rows| writer.send(rows)).map_err(|e| e.to_string())?;
    writer.finish(&summary).map_err(|e| e.to_string())?;
    let volume = volume(&summary.root)
        .ok()
        .and_then(|v| VolumeUse::of(&v, &summary.root));
    Ok(ScanReport {
        volume,
        peak_rss: peak_rss(),
        index_size: index_size(index),
        summary,
    })
}

/// The index file and its write-ahead log.
fn index_size(index: &Path) -> Bytes {
    let mut wal = index.as_os_str().to_owned();
    wal.push("-wal");
    [index.to_path_buf(), PathBuf::from(wal)]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| Bytes(m.len()))
        .sum()
}

/// A path as the index stores it.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The largest directories below `under` (default: the root).
pub fn top(
    index: &Path,
    under: Option<&Path>,
    limit: usize,
) -> Result<Vec<(Bytes, PathBuf)>, String> {
    let mut reader = IndexReader::open(index).map_err(|e| e.to_string())?;
    reader
        .read(|snap| largest(snap, under, limit))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not in the index".to_owned())
}

fn largest(
    snap: &Snapshot<'_>,
    under: Option<&Path>,
    limit: usize,
) -> Result<Option<Vec<(Bytes, PathBuf)>>, IndexError> {
    let anchor = match under {
        Some(path) => snap.dir(&canonical(path))?,
        None => snap.root()?,
    };
    let Some(anchor) = anchor else {
        return Ok(None);
    };
    let mut rows = Vec::new();
    for dir in snap.top(anchor.id, limit)? {
        rows.push((dir.tree.bytes, snap.path_of(dir.id)?.unwrap_or_default()));
    }
    Ok(Some(rows))
}

/// The subdirectories of `path` from the index and its files listed now,
/// largest first.
pub fn ls(index: &Path, path: &Path) -> Result<Vec<Listed>, String> {
    let path = canonical(path);
    let mut reader = IndexReader::open(index).map_err(|e| e.to_string())?;
    let dirs = reader
        .read(|snap| match snap.dir(&path)? {
            Some(dir) => Ok(Some(snap.children(dir.id)?)),
            None => Ok(None),
        })
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{} is not in the index", path.display()))?;
    let files = DirHandle::open(&path)
        .and_then(|dir| list_dir(&dir))
        .map_err(|e| format!("cannot list {}: {e:?}", path.display()))?;
    let mut rows: Vec<Listed> = dirs
        .into_iter()
        .map(|d| Listed {
            size: d.tree.bytes,
            name: d.name,
            dir: Some(d.state),
        })
        .chain(
            files
                .into_iter()
                .filter(|e| e.meta.kind != EntryKind::Dir)
                .map(|e| Listed {
                    size: e.meta.share(),
                    name: e.name,
                    dir: None::<DirState>,
                }),
        )
        .collect();
    rows.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    Ok(rows)
}
