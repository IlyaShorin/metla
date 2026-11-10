//! Incremental rescans are exact: for random sequences of changes to a
//! synthetic tree, the rescan tasks the coalescer makes of the changed
//! directories, applied to the picture, leave exactly what a fresh full
//! scan of the changed tree writes — rows, totals, bytes by kind,
//! candidates and coverage. A failure prints its seed.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use metla_core::domain::{Recency, RescanTask, UnixTime};
use metla_core::engine::{Refreshed, refresh};
use metla_core::index::{IndexReader, IndexWriter, Page};
use metla_core::rules::RuleSet;
use metla_core::scan::{ScanControl, ScanOptions, run};
use metla_core::watch::{Coalescer, Event, Flags, Timing};

use crate::scan::options;
use crate::support::{Fixture, Rng};

const START: &[&str] = &[
    "~/Library/Caches/com.example.app/blob=cached",
    "~/Library/Logs/com.example.app/today.log=log",
    "~/.npm/_cacache/index/x=npm",
    "~/Movies/clip.mov=movie",
    "~/Documents/notes.txt=notes",
    "code/app/Cargo.toml",
    "code/app/.git/HEAD=ref",
    "code/app/src/main.rs=fn main() {}",
    "code/app/target/debug/app=binary",
    "code/web/package.json",
    "code/web/node_modules/x/index.js=js",
    "Applications/Tool.app/Contents/Info.plist=plist",
];

/// The tree being changed, and what the test knows about it.
struct Changing {
    fx: Fixture,
    rng: Rng,
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
    denied: BTreeSet<PathBuf>,
    made: u64,
}

impl Changing {
    fn new(seed: u64) -> Changing {
        let fx = Fixture::new();
        fx.build(START);
        let mut changing = Changing {
            fx,
            rng: Rng::new(seed),
            dirs: Vec::new(),
            files: Vec::new(),
            denied: BTreeSet::new(),
            made: 0,
        };
        changing.survey();
        changing
    }

    /// Relearns the directories and files that can be reached.
    fn survey(&mut self) {
        self.dirs = vec![PathBuf::new()];
        self.files.clear();
        let mut pending = vec![PathBuf::new()];
        while let Some(dir) = pending.pop() {
            if self.denied.contains(&dir) {
                continue;
            }
            for entry in fs::read_dir(self.fx.root().join(&dir)).unwrap() {
                let entry = entry.unwrap();
                let relative = dir.join(entry.file_name());
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    self.dirs.push(relative.clone());
                    pending.push(relative);
                } else if kind.is_file() {
                    self.files.push(relative);
                }
            }
        }
        self.dirs.sort();
        self.files.sort();
    }

    fn pick<'a>(&mut self, from: &'a [PathBuf]) -> Option<&'a PathBuf> {
        let open: Vec<&PathBuf> = from.iter().filter(|p| !self.blocked(p)).collect();
        let at = self.rng.below(open.len() as u64) as usize;
        open.get(at).copied()
    }

    /// Inside (or at) a directory the test made unreadable.
    fn blocked(&self, path: &Path) -> bool {
        self.denied.iter().any(|d| path.starts_with(d))
    }

    fn name(&mut self, stem: &str) -> String {
        self.made += 1;
        format!("{stem}{}", self.made)
    }

    fn at(&self, relative: &Path) -> PathBuf {
        self.fx.root().join(relative)
    }

    /// One random change; returns the directories whose events a
    /// directory-level stream reports for it.
    fn change(&mut self) -> Vec<PathBuf> {
        let dirs = self.dirs.clone();
        let files = self.files.clone();
        let Some(dir) = self.pick(&dirs).cloned() else {
            return Vec::new();
        };
        let file = self.pick(&files).cloned();
        let changed = match (self.rng.below(15), file) {
            (0, _) => self.new_file(&dir),
            (1, Some(file)) => self.append(&file),
            (2, Some(file)) => self.delete_file(&file),
            (3, Some(file)) => self.backdate(&file),
            (4, _) => self.new_dir(&dir),
            (5, _) if dir != Path::new("") => self.delete_dir(&dir),
            (6, _) if dir != Path::new("") => self.rename(&dir, &dirs),
            (7, _) => self.cargo_project(&dir),
            (8, _) => self.repository(&dir),
            (9, _) => self.unmark(&dir),
            (10, _) if dir != Path::new("") => self.deny(&dir),
            (11, _) => self.allow(),
            (12, _) => self.link(&dir),
            (13, _) => self.node_project(&dir),
            _ => self.new_file(&dir),
        };
        self.survey();
        changed
    }

    fn new_file(&mut self, dir: &Path) -> Vec<PathBuf> {
        let extension = ["txt", "png", "zip", "rs", "mov", "bin"][self.rng.below(6) as usize];
        let name = format!("{}.{extension}", self.name("f"));
        let len = self.rng.below(9000) as usize;
        fs::write(self.at(&dir.join(name)), vec![1u8; len]).unwrap();
        vec![dir.to_path_buf()]
    }

    fn append(&mut self, file: &Path) -> Vec<PathBuf> {
        let mut bytes = fs::read(self.at(file)).unwrap();
        bytes.extend(vec![2u8; 5000]);
        fs::write(self.at(file), bytes).unwrap();
        vec![parent(file)]
    }

    fn delete_file(&mut self, file: &Path) -> Vec<PathBuf> {
        fs::remove_file(self.at(file)).unwrap();
        vec![parent(file)]
    }

    fn backdate(&mut self, file: &Path) -> Vec<PathBuf> {
        let long_ago = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let opened = fs::File::options().write(true).open(self.at(file)).unwrap();
        opened.set_modified(long_ago).unwrap();
        vec![parent(file)]
    }

    fn new_dir(&mut self, dir: &Path) -> Vec<PathBuf> {
        let made = dir.join(self.name("d"));
        fs::create_dir(self.at(&made)).unwrap();
        fs::write(self.at(&made.join("inside.md")), "text").unwrap();
        vec![dir.to_path_buf(), made]
    }

    fn delete_dir(&mut self, dir: &Path) -> Vec<PathBuf> {
        if self.denied.iter().any(|d| d.starts_with(dir)) {
            return Vec::new();
        }
        fs::remove_dir_all(self.at(dir)).unwrap();
        vec![parent(dir), dir.to_path_buf()]
    }

    fn rename(&mut self, dir: &Path, dirs: &[PathBuf]) -> Vec<PathBuf> {
        let into = self.pick(dirs).cloned().unwrap_or_default();
        let denied_inside = self.denied.iter().any(|d| d.starts_with(dir));
        if into.starts_with(dir) || denied_inside {
            return Vec::new();
        }
        let to = into.join(self.name("moved"));
        fs::rename(self.at(dir), self.at(&to)).unwrap();
        vec![parent(dir), into]
    }

    fn cargo_project(&mut self, dir: &Path) -> Vec<PathBuf> {
        fs::write(self.at(&dir.join("Cargo.toml")), "[package]").unwrap();
        let target = dir.join("target");
        fs::create_dir_all(self.at(&target.join("debug"))).unwrap();
        fs::write(self.at(&target.join("debug/out")), vec![3u8; 3000]).unwrap();
        vec![dir.to_path_buf(), target.clone(), target.join("debug")]
    }

    fn repository(&mut self, dir: &Path) -> Vec<PathBuf> {
        fs::create_dir_all(self.at(&dir.join(".git"))).unwrap();
        vec![dir.to_path_buf()]
    }

    fn unmark(&mut self, dir: &Path) -> Vec<PathBuf> {
        let marker = self.at(&dir.join("Cargo.toml"));
        if fs::remove_file(marker).is_err() {
            return Vec::new();
        }
        vec![dir.to_path_buf()]
    }

    fn deny(&mut self, dir: &Path) -> Vec<PathBuf> {
        fs::set_permissions(self.at(dir), fs::Permissions::from_mode(0o000)).unwrap();
        self.denied.insert(dir.to_path_buf());
        vec![parent(dir)]
    }

    fn allow(&mut self) -> Vec<PathBuf> {
        let Some(dir) = self.denied.pop_first() else {
            return Vec::new();
        };
        fs::set_permissions(self.at(&dir), fs::Permissions::from_mode(0o755)).unwrap();
        vec![parent(&dir)]
    }

    fn link(&mut self, dir: &Path) -> Vec<PathBuf> {
        let name = self.name("link");
        symlink("/nowhere", self.at(&dir.join(name))).unwrap();
        vec![dir.to_path_buf()]
    }

    fn node_project(&mut self, dir: &Path) -> Vec<PathBuf> {
        fs::write(self.at(&dir.join("package.json")), "{}").unwrap();
        let modules = dir.join("node_modules");
        fs::create_dir_all(self.at(&modules.join("pkg"))).unwrap();
        fs::write(self.at(&modules.join("pkg/index.js")), vec![4u8; 2000]).unwrap();
        vec![dir.to_path_buf(), modules.clone(), modules.join("pkg")]
    }

    /// Makes every directory readable again, so the tree can be removed.
    fn restore(&mut self) {
        while self.allow().len() == 1 {}
    }
}

fn parent(path: &Path) -> PathBuf {
    path.parent().unwrap_or(Path::new("")).to_path_buf()
}

fn scan_options(changing: &Changing) -> ScanOptions {
    let rules = RuleSet::new(&changing.fx.home(), OsStr::new("tester"));
    options(changing.fx.root())
        .rules(Arc::new(rules))
        .threads(2)
}

fn full_scan(options: &ScanOptions, writer: &mut IndexWriter) {
    let session = writer.start_scan(UnixTime(1)).unwrap();
    let summary = run(options, &ScanControl::new(0), &|rows| session.send(rows)).unwrap();
    session.finish(&summary).unwrap();
}

/// Everything the index says about the picture, keyed by path instead
/// of by id: ids differ between a picture brought up to date and a fresh
/// one, nothing else may.
fn contents(index: &Path) -> BTreeMap<String, String> {
    let conn = rusqlite::Connection::open(index).unwrap();
    let mut ids: BTreeMap<i64, (Option<i64>, Vec<u8>)> = BTreeMap::new();
    let mut rows: BTreeMap<i64, String> = BTreeMap::new();
    let mut statement = conn
        .prepare(
            "SELECT id, parent_id, name, own_bytes, own_files, own_mtime, tree_bytes, tree_files,
                    tree_dirs, tree_mtime, mtime, changed, flags, device, inode, rule_id,
                    hex(markers), hex(kinds) FROM dirs",
        )
        .unwrap();
    let mut found = statement.query([]).unwrap();
    while let Some(row) = found.next().unwrap() {
        let id: i64 = row.get(0).unwrap();
        ids.insert(id, (row.get(1).unwrap(), row.get(2).unwrap()));
        let values: Vec<String> = (3..18)
            .map(|i| format!("{:?}", row.get::<_, rusqlite::types::Value>(i).unwrap()))
            .collect();
        rows.insert(id, values.join(" "));
    }
    let path = |mut id: i64| {
        let mut names = Vec::new();
        while let Some((parent, name)) = ids.get(&id) {
            names.push(String::from_utf8_lossy(name).into_owned());
            match parent {
                Some(parent) => id = *parent,
                None => break,
            }
        }
        names.reverse();
        names.join("/")
    };
    let mut all: BTreeMap<String, String> = rows
        .into_iter()
        .map(|(id, values)| (format!("row {}", path(id)), values))
        .collect();
    for (table, sql) in [
        (
            "kinds",
            "SELECT code || ':' || bytes FROM kinds ORDER BY code",
        ),
        (
            "coverage",
            "SELECT code || ':' || count FROM coverage_counts ORDER BY code",
        ),
        (
            "gaps",
            "SELECT rank || ':' || depth || ':' || hex(path) || ':' || flags
             FROM coverage_gaps ORDER BY rank",
        ),
    ] {
        let mut statement = conn.prepare(sql).unwrap();
        let values: Vec<String> = statement
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        all.insert(table.to_owned(), values.join(","));
    }
    all.insert("candidates".to_owned(), candidates(index));
    all
}

fn candidates(index: &Path) -> String {
    let recency = Recency::new(UnixTime(2_000_000_000), Recency::DEFAULT_AGE);
    let page = Page {
        offset: 0,
        limit: 10_000,
    };
    let mut reader = IndexReader::open(index).unwrap();
    let (found, tallies, reclaimable) = reader
        .read(|snap| {
            Ok((
                snap.candidates(None, page, &recency)?,
                snap.category_tallies()?,
                snap.reclaimable(&recency)?,
            ))
        })
        .unwrap();
    // Equal sizes are listed in id order, and ids differ.
    let listed: BTreeSet<String> = found
        .iter()
        .map(|c| format!("{} {} {} {}", c.path.display(), c.rule, c.bytes.0, c.files))
        .collect();
    format!("{listed:?} {tallies:?} {reclaimable:?}")
}

/// The coalescer's timing in this test: nothing waits.
fn eager() -> Timing {
    Timing {
        settle: Duration::from_millis(1),
        factor: 2,
        cap: Duration::from_millis(4),
        batch: 1000,
        collapse: 6,
        max_dirs: 64,
    }
}

/// Changes the tree `steps` times, rescanning what the changes touched
/// after every `every` changes; then compares with a fresh scan.
fn equivalent(seed: u64, steps: usize, every: usize) {
    let mut changing = Changing::new(seed);
    let aux = Fixture::new();
    let (kept, fresh) = (aux.path("kept.sqlite"), aux.path("fresh.sqlite"));
    let options = scan_options(&changing);
    let mut writer = IndexWriter::open(&kept).unwrap();
    full_scan(&options, &mut writer);
    let mut reader = IndexReader::open(&kept).unwrap();
    let root = changing.fx.root().to_path_buf();
    let mut coalescer = Coalescer::new(&root, Vec::new(), Some(eager()));
    let mut clock = Duration::ZERO;
    for step in 1..=steps {
        for dir in changing.change() {
            let path = root.join(dir);
            let event = Event {
                path,
                flags: Flags::NONE,
                id: step as u64,
            };
            coalescer.on_event(&event, clock);
        }
        if step % every != 0 && step != steps {
            continue;
        }
        clock += Duration::from_secs(1);
        loop {
            let tasks = coalescer.due(clock);
            if tasks.is_empty() {
                break;
            }
            for task in tasks {
                let control = ScanControl::new(0);
                let done = refresh(&task, &options, &mut reader, &writer, &control, UnixTime(5));
                match done.unwrap_or_else(|e| panic!("seed {seed}: {task:?}: {e:?}")) {
                    Refreshed::Applied => {}
                    Refreshed::NeedsFullScan => full_scan(&options, &mut writer),
                    Refreshed::NoPicture => panic!("seed {seed}: no picture"),
                }
            }
        }
    }
    drop(writer);
    let mut writer = IndexWriter::open(&fresh).unwrap();
    full_scan(&options, &mut writer);
    drop(writer);
    let (got, want) = (contents(&kept), contents(&fresh));
    changing.restore();
    for (key, value) in &want {
        assert_eq!(got.get(key), Some(value), "seed {seed}: {key}");
    }
    assert_eq!(got.len(), want.len(), "seed {seed}: extra rows");
}

#[test]
fn rescanning_what_changed_gives_what_a_full_scan_gives() {
    for seed in 1..=12 {
        equivalent(seed, 25, 5);
    }
}

#[test]
fn rescanning_after_every_change_gives_what_a_full_scan_gives() {
    for seed in 100..=105 {
        equivalent(seed, 12, 1);
    }
}

#[test]
fn a_task_for_the_root_or_a_full_rescan_needs_a_full_scan() {
    let changing = Changing::new(7);
    let aux = Fixture::new();
    let index = aux.path("index.sqlite");
    let options = scan_options(&changing);
    let mut writer = IndexWriter::open(&index).unwrap();
    let mut reader = IndexReader::open(&index).unwrap();
    let control = ScanControl::new(0);
    let root = changing.fx.root().to_path_buf();
    let shallow = RescanTask::Shallow(root.clone());
    let none = refresh(
        &shallow,
        &options,
        &mut reader,
        &writer,
        &control,
        UnixTime(5),
    );
    assert_eq!(none.unwrap(), Refreshed::NoPicture);
    full_scan(&options, &mut writer);
    for task in [RescanTask::Full, RescanTask::Recursive(root.clone())] {
        let done = refresh(&task, &options, &mut reader, &writer, &control, UnixTime(5));
        assert_eq!(done.unwrap(), Refreshed::NeedsFullScan, "{task:?}");
    }
    let applied = refresh(
        &shallow,
        &options,
        &mut reader,
        &writer,
        &control,
        UnixTime(5),
    );
    assert_eq!(applied.unwrap(), Refreshed::Applied);
    let updated = reader.read(|snap| snap.scan()).unwrap().unwrap();
    assert_eq!(updated.picture.unwrap().updated, Some(UnixTime(5)));
}
