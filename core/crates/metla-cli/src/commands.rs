//! The commands: gather data from the core, hand it to `report`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use metla_core::domain::{
    Bytes, Candidate, Category, DirId, DirState, EntryKind, Exclusion, Recency,
};
use metla_core::fs::{DirHandle, list_dir, now, peak_rss, volume};
use metla_core::git::{Git, verify_outputs};
use metla_core::index::{IndexError, IndexReader, IndexWriter, Page, Snapshot};
use metla_core::rules::RuleSet;
use metla_core::scan::{ScanControl, ScanOptions, ScanRoot, ScanScope, run};

use crate::args::Edit;
use crate::overview::{Edited, Freshness, Overview, Shown};
use crate::report::{Checked, Listed, ScanReport, VolumeUse};

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

/// Which rules a scan uses.
pub struct ScanRules {
    /// `None`: the current user's home directory.
    pub home: Option<PathBuf>,
    pub enabled: bool,
}

/// The rule set for `home` (default: `$HOME`), for the user `$USER` (or
/// the home directory's name).
fn rule_set(home: Option<&Path>) -> Result<RuleSet, String> {
    let home = home
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .ok_or("no home directory: pass --home")?;
    let home = std::fs::canonicalize(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    let user = std::env::var_os("USER")
        .or_else(|| home.file_name().map(OsString::from))
        .unwrap_or_default();
    Ok(RuleSet::new(&home, &user))
}

/// Scans `root` into `index`.
pub fn scan(
    root: &Path,
    index: &Path,
    rules: &ScanRules,
    threads: Option<usize>,
) -> Result<ScanReport, String> {
    let options = options(root, rules, threads)?;
    let mut writer = IndexWriter::open(index).map_err(|e| e.to_string())?;
    let session = writer.start_scan(now()).map_err(|e| e.to_string())?;
    let control = ScanControl::new(LIVE_LIMIT);
    let summary = run(&options, &control, &|rows| session.send(rows)).map_err(|e| e.to_string())?;
    session.finish(&summary).map_err(|e| e.to_string())?;
    let checked = match rules.enabled {
        true => Some(verify(index, &writer)?),
        false => None,
    };
    let volume = volume(&summary.root)
        .ok()
        .and_then(|v| VolumeUse::of(&v, &summary.root));
    Ok(ScanReport {
        volume,
        peak_rss: peak_rss(),
        index_size: index_size(index),
        rules: checked,
        summary,
    })
}

/// What and how to scan.
fn options(root: &Path, rules: &ScanRules, threads: Option<usize>) -> Result<ScanOptions, String> {
    let root = ScanRoot::new(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let scope = ScanScope::detect(&root).map_err(|e| e.to_string())?;
    let threads = threads.unwrap_or_else(|| default_threads(cores()));
    let options = ScanOptions::new(root, scope).threads(threads);
    match rules.enabled {
        true => Ok(options.rules(Arc::new(rule_set(rules.home.as_deref())?))),
        false => Ok(options),
    }
}

/// Asks Git about the project output of the picture just written and
/// stores its verdicts: only verified output is offered.
fn verify(index: &Path, writer: &IndexWriter) -> Result<Checked, String> {
    let clock = Instant::now();
    let (picture, unverified) = read(index, |snap| {
        let picture = snap.scan()?.and_then(|scan| scan.picture);
        Ok((picture.map(|p| p.id()), snap.unverified()?))
    })?;
    let picture = picture.ok_or("the scan left no picture")?;
    let (ids, paths): (Vec<DirId>, Vec<PathBuf>) = unverified.into_iter().unzip();
    let verdicts = verify_outputs(&Git::system(), &paths);
    let stored = ids.into_iter().zip(verdicts.iter().copied()).collect();
    writer
        .set_verdicts(picture, stored)
        .map_err(|e| e.to_string())?;
    Ok(Checked::of(&verdicts, clock.elapsed()))
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

fn read<T>(
    index: &Path,
    query: impl FnOnce(&Snapshot<'_>) -> Result<T, IndexError>,
) -> Result<T, String> {
    let mut reader = IndexReader::open(index).map_err(|e| e.to_string())?;
    reader.read(query).map_err(|e| e.to_string())
}

/// The largest directories below `under` (default: the root).
pub fn top(
    index: &Path,
    under: Option<&Path>,
    limit: usize,
) -> Result<Vec<(Bytes, PathBuf)>, String> {
    read(index, |snap| largest(snap, under, limit))?.ok_or_else(|| "not in the index".to_owned())
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
    let dirs = read(index, |snap| match snap.dir(&path)? {
        Some(dir) => Ok(Some(snap.children(dir.id)?)),
        None => Ok(None),
    })?
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

/// The largest candidates of `category` (of every category when `None`).
pub fn candidates(
    index: &Path,
    category: Option<Category>,
    limit: usize,
) -> Result<Shown<Vec<Candidate>>, String> {
    let now = now();
    let recency = Recency::new(now, Recency::DEFAULT_AGE);
    let page = Page { offset: 0, limit };
    let found = read(index, |snap| snap.candidates(category, page, &recency))?;
    Ok(Shown { now, data: found })
}

/// What can be cleaned, what the disk holds, and how fresh the picture is.
pub fn summary(index: &Path) -> Result<Shown<Overview>, String> {
    let now = now();
    let recency = Recency::new(now, Recency::DEFAULT_AGE);
    let overview = read(index, |snap| {
        let scan = snap.scan()?;
        let freshness = scan.as_ref().and_then(|scan| {
            let picture = scan.picture.clone()?;
            Some(Freshness {
                root: picture.root,
                finished: picture.finished,
                current: scan.is_current(),
                coverage: picture.coverage,
                kinds: picture.kinds,
            })
        });
        Ok(Overview {
            freshness,
            reclaimable: snap.reclaimable(&recency)?,
            categories: snap.category_tallies()?,
            rules: snap.rule_tallies()?,
        })
    })?;
    Ok(Shown {
        now,
        data: overview,
    })
}

/// Adds, removes or lists exclusions.
pub fn exclude(index: &Path, edit: &Edit) -> Result<Edited, String> {
    match edit {
        Edit::List => Ok(Edited::Listed(read(index, |snap| snap.exclusions())?)),
        Edit::Add(path) => {
            change(index, path, IndexWriter::exclude).map(|(e, c)| Edited::Added(e, c))
        }
        Edit::Remove(path) => {
            change(index, path, IndexWriter::include).map(|(e, c)| Edited::Removed(e, c))
        }
    }
}

type Change = fn(&IndexWriter, &Exclusion) -> Result<bool, IndexError>;

/// Applies one exclusion edit through the index's writer.
fn change(index: &Path, path: &Path, change: Change) -> Result<(Exclusion, bool), String> {
    let path = canonical(path);
    let exclusion = Exclusion::new(&path)
        .ok_or_else(|| format!("{} is not an absolute path", path.display()))?;
    let writer = IndexWriter::open(index).map_err(|e| e.to_string())?;
    let changed = change(&writer, &exclusion).map_err(|e| e.to_string())?;
    Ok((exclusion, changed))
}
