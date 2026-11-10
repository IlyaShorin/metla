//! A synthetic fixture to clean: a tree marked `.metla-fixture`, scanned
//! with the rules into an index outside it, its project output verified by
//! an isolated Git, and a trash directory outside it for the fake Trash.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metla_core::clean::{CleanControl, CleanSettings, Cleaner, Context, FIXTURE_MARKER, TrashHost};
use metla_core::domain::{
    Candidate, CleanupPreview, CleanupReport, Confirmation, Exclusion, Outcome, Recency, Separate,
};
use metla_core::git::{Git, WorktreeSettings, verify_outputs};
use metla_core::index::{IndexReader, IndexWriter, Page};
use metla_core::rules::RuleSet;
use metla_core::scan::ScanControl;

use crate::repo::{git, isolated, next_year};
use crate::scan::{options, scan_with};
use crate::support::Fixture;

/// Moves into a directory outside the tree, as the Trash would; can be
/// told to refuse.
pub struct FakeTrash {
    pub dir: PathBuf,
    pub refuse: bool,
    pub moved: RefCell<Vec<PathBuf>>,
}

impl TrashHost for FakeTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        if self.refuse {
            return Err("the Trash said no".into());
        }
        let n = self.moved.borrow().len();
        let name = format!("{n}-{}", path.file_name().unwrap().to_string_lossy());
        fs::rename(path, self.dir.join(name)).map_err(|e| e.to_string())?;
        self.moved.borrow_mut().push(path.to_path_buf());
        Ok(())
    }
}

pub struct World {
    pub fx: Fixture,
    pub git: Git,
    aux: Fixture,
    pub trash: FakeTrash,
}

impl World {
    /// A fixture tree built from `lines` (see `Fixture::build`; `~` is the
    /// tree's home), with the marker.
    pub fn new(lines: &[&str]) -> World {
        let world = World::unmarked(lines);
        fs::write(world.fx.path(FIXTURE_MARKER), "").unwrap();
        world
    }

    pub fn unmarked(lines: &[&str]) -> World {
        let fx = Fixture::new();
        fx.build(lines);
        let aux = Fixture::new();
        let trash = FakeTrash {
            dir: aux.dir("trash"),
            refuse: false,
            moved: RefCell::default(),
        };
        World {
            git: isolated(&aux),
            fx,
            aux,
            trash,
        }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.fx.path(&relative.replacen('~', "home", 1))
    }

    /// Makes `relative` a Git repository ignoring `ignored`.
    pub fn repo(&self, relative: &str, ignored: &str) -> PathBuf {
        let repo = self.fx.dir(relative);
        git(&self.git, &repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join(".gitignore"), ignored).unwrap();
        repo
    }

    fn index(&self) -> PathBuf {
        self.aux.path("index.sqlite")
    }

    pub fn rules(&self) -> RuleSet {
        RuleSet::new(&self.fx.home(), OsStr::new("tester"))
    }

    /// Scans the tree, asks Git about its project output, and returns
    /// every candidate offered, by path relative to the tree.
    pub fn scan(&self) -> BTreeMap<String, Candidate> {
        let options = options(self.fx.root()).rules(Arc::new(self.rules()));
        let scanned = scan_with(&options, &ScanControl::new(64));
        let mut writer = IndexWriter::open(&self.index()).unwrap();
        let session = writer.start_scan(scanned.summary.started).unwrap();
        session.send(scanned.records.into_values().collect());
        session.finish(&scanned.summary).unwrap();
        let mut reader = IndexReader::open(&self.index()).unwrap();
        let (picture, unverified) = reader
            .read(|snap| {
                let picture = snap.scan()?.and_then(|s| s.picture).unwrap().id();
                Ok((picture, snap.unverified()?))
            })
            .unwrap();
        let paths: Vec<PathBuf> = unverified.iter().map(|(_, p)| p.clone()).collect();
        let verdicts = verify_outputs(&self.git, &paths);
        let stored = unverified.iter().map(|(id, _)| *id).zip(verdicts).collect();
        writer.set_verdicts(picture, stored).unwrap();
        let recency = Recency::new(next_year(), Recency::DEFAULT_AGE);
        let page = Page {
            offset: 0,
            limit: 10_000,
        };
        let found = reader
            .read(|snap| snap.candidates(None, page, &recency))
            .unwrap();
        let by_path = found.into_iter().map(|c| {
            let relative = c.path.strip_prefix(self.fx.root()).unwrap();
            (relative.display().to_string(), c)
        });
        by_path.collect()
    }

    pub fn settings(&self) -> CleanSettings {
        CleanSettings {
            worktrees: WorktreeSettings {
                home: self.fx.home(),
                stale_after: WorktreeSettings::DEFAULT_STALE_AFTER,
            },
            recent_age: Recency::DEFAULT_AGE,
        }
    }

    pub fn cleaner(&self) -> Cleaner {
        let rules = Arc::new(self.rules());
        Cleaner::new(
            self.fx.root(),
            None,
            rules,
            self.git.clone(),
            self.settings(),
        )
        .unwrap()
    }

    /// The candidates at these relative paths.
    pub fn picked(&self, found: &BTreeMap<String, Candidate>, paths: &[&str]) -> Vec<Candidate> {
        let pick = |p: &&str| {
            found
                .get(*p)
                .unwrap_or_else(|| panic!("no candidate {p}: {found:?}"))
        };
        paths.iter().map(pick).cloned().collect()
    }

    /// Previews `paths` and executes the preview with `confirmation`.
    pub fn clean(&self, paths: &[&str], confirmation: &Confirmation) -> CleanupReport {
        let found = self.scan();
        let mut cleaner = self.cleaner();
        let preview = cleaner.preview(self.picked(&found, paths), next_year());
        self.execute(&mut cleaner, &preview, confirmation, &[])
    }

    pub fn execute(
        &self,
        cleaner: &mut Cleaner,
        preview: &CleanupPreview,
        confirmation: &Confirmation,
        exclusions: &[Exclusion],
    ) -> CleanupReport {
        let context = Context {
            now: next_year(),
            exclusions,
        };
        let control = CleanControl::new();
        cleaner
            .execute(preview.id, confirmation, &context, &control, &self.trash)
            .unwrap()
    }
}

/// Every group confirmed, no item confirmed on its own.
pub fn all_groups() -> Confirmation {
    Confirmation {
        groups: [
            Separate::MoveToTrash,
            Separate::EmptyTrash,
            Separate::RemoveWorktrees,
        ]
        .into(),
        items: Default::default(),
    }
}

/// The outcome of every item, by path relative to `root`.
pub fn outcomes(report: &CleanupReport, root: &Path) -> BTreeMap<String, Outcome> {
    let each = report.items.iter().map(|item| {
        let relative = item.path.strip_prefix(root).unwrap();
        (relative.display().to_string(), item.outcome.clone())
    });
    each.collect()
}
