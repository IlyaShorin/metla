//! Cleaning: a preview of exactly what will be removed, executed once
//! after it is confirmed, with every item revalidated right before it is
//! acted on, and a report that keeps found, removed and freed apart.
//!
//! Removal needs permission: a development build and the CLI act only on a
//! root marked with [`FIXTURE_MARKER`]; only the release app can make the
//! [`Production`] capability that lifts that.

mod act;
mod check;
mod control;
mod preview;

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    Bytes, Candidate, CleanupPreview, CleanupReport, Confirmation, EntryKind, Exclusion,
    ItemReport, Outcome, PreviewId, PreviewItem, QuickCategory, Recency, SkipReason, UnixTime,
};
use crate::fs::{DirHandle, volume};
use crate::git::{Git, WorktreeSettings, inspect_worktree};
use crate::rules::RuleSet;

use act::Actor;
use check::Checker;

pub use control::{CleanControl, CleanProgress};

/// The file that marks a synthetic tree a development build may clean.
pub const FIXTURE_MARKER: &str = ".metla-fixture";

/// Moves things to the Trash. The app implements it with
/// `FileManager.trashItem`, so "Put Back" works; tests and the CLI move
/// into a fixture's own trash directory.
pub trait TrashHost {
    fn trash(&self, path: &Path) -> Result<(), String>;
}

/// Permission to clean a root that is not a fixture. Only a release build
/// with the `production` feature can make one; the CLI never does.
#[derive(Debug)]
pub struct Production(());

impl Production {
    #[cfg(all(feature = "production", not(debug_assertions)))]
    pub fn release_app() -> Production {
        Production(())
    }
}

/// How a cleaner judges what it is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanSettings {
    pub worktrees: WorktreeSettings,
    /// Build output active within this age is flagged in a preview.
    pub recent_age: Duration,
}

/// What holds at the moment a preview is executed.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    pub now: UnixTime,
    /// The exclusions in force now.
    pub exclusions: &'a [Exclusion],
}

/// Why a cleanup did not run at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanError {
    /// The root carries no fixture marker and no production capability
    /// was given.
    NotAFixture(PathBuf),
    /// The root cannot be opened, or is no longer the directory it was.
    Root(String),
    /// No preview with this id was made, or a newer one replaced it.
    UnknownPreview(PreviewId),
    /// The preview was executed already.
    PreviewUsed(PreviewId),
}

impl fmt::Display for CleanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanError::NotAFixture(root) => write!(
                f,
                "{} has no {FIXTURE_MARKER}: development builds clean only fixtures",
                root.display()
            ),
            CleanError::Root(why) => write!(f, "root: {why}"),
            CleanError::UnknownPreview(id) => write!(f, "no preview {}", id.0),
            CleanError::PreviewUsed(id) => write!(f, "preview {} was executed already", id.0),
        }
    }
}

/// The root a cleaner works in: canonical, and checked to be the same
/// directory every time it acts.
struct Root {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Root {
    fn open(path: &Path) -> Result<(Root, DirHandle), CleanError> {
        let failed = |e: std::io::Error| CleanError::Root(format!("{}: {e}", path.display()));
        let path = std::fs::canonicalize(path).map_err(failed)?;
        let dir = DirHandle::open(&path).map_err(|e| CleanError::Root(format!("{e:?}")))?;
        let meta = dir.meta().map_err(|e| CleanError::Root(format!("{e:?}")))?;
        let root = Root {
            path,
            device: meta.device,
            inode: meta.inode,
        };
        Ok((root, dir))
    }

    /// The root opened again, if it is still the same directory.
    fn reopen(&self) -> Result<DirHandle, CleanError> {
        let (now, dir) = Root::open(&self.path)?;
        match (now.device, now.inode) == (self.device, self.inode) {
            true => Ok(dir),
            false => Err(CleanError::Root("it is another directory now".into())),
        }
    }
}

/// Refuses a root without the fixture marker (a regular file), unless the
/// production capability was given.
fn permitted(
    dir: &DirHandle,
    root: &Path,
    production: Option<&Production>,
) -> Result<(), CleanError> {
    let marker = dir.entry(OsStr::new(FIXTURE_MARKER));
    let fixture = marker.is_ok_and(|meta| meta.kind == EntryKind::File);
    match fixture || production.is_some() {
        true => Ok(()),
        false => Err(CleanError::NotAFixture(root.to_path_buf())),
    }
}

/// Makes previews and executes them, each at most once.
pub struct Cleaner {
    root: Root,
    production: Option<Production>,
    rules: Arc<RuleSet>,
    git: Git,
    settings: CleanSettings,
    /// How many previews were made: the next one's id follows.
    made: u64,
    /// The latest preview, `Some` until it is executed. Only it can be
    /// executed, so a cleaner holds one preview however long it lives.
    latest: Option<(PreviewId, Option<CleanupPreview>)>,
}

impl Cleaner {
    /// A cleaner for `root`. Without `production`, `root` must carry the
    /// fixture marker; nothing is touched otherwise.
    pub fn new(
        root: &Path,
        production: Option<Production>,
        rules: Arc<RuleSet>,
        git: Git,
        settings: CleanSettings,
    ) -> Result<Cleaner, CleanError> {
        let (root, dir) = Root::open(root)?;
        permitted(&dir, &root.path, production.as_ref())?;
        Ok(Cleaner {
            root,
            production,
            rules,
            git,
            settings,
            made: 0,
            latest: None,
        })
    }

    /// Judges the previews and executions that follow with `settings`.
    pub fn set_settings(&mut self, settings: CleanSettings) {
        self.settings = settings;
    }

    /// The preview of `candidates`: worktrees that cannot be removed now
    /// and items inside other items are set aside with the reason. It
    /// replaces the previous preview, which can no longer be executed.
    pub fn preview(&mut self, candidates: Vec<Candidate>, now: UnixTime) -> CleanupPreview {
        self.made += 1;
        let id = PreviewId(self.made);
        let recency = Recency::new(now, self.settings.recent_age);
        let inspect = |c: &Candidate| {
            inspect_worktree(
                &self.git,
                &c.path,
                c.activity,
                now,
                &self.settings.worktrees,
            )
        };
        let preview = preview::build(id, candidates, inspect, &recency);
        self.latest = Some((id, Some(preview.clone())));
        preview
    }

    /// Executes the preview `id` as confirmed. Each item is revalidated
    /// right before it is acted on; what fails a check is skipped with the
    /// reason, what is not there any more is already gone.
    pub fn execute(
        &mut self,
        id: PreviewId,
        confirmation: &Confirmation,
        context: &Context<'_>,
        control: &CleanControl,
        trash: &dyn TrashHost,
    ) -> Result<CleanupReport, CleanError> {
        let root_dir = self.root.reopen()?;
        permitted(&root_dir, &self.root.path, self.production.as_ref())?;
        let preview = self.take(id)?;
        let checker = Checker {
            root: &self.root.path,
            root_dir: &root_dir,
            rules: &self.rules,
            git: &self.git,
            exclusions: context.exclusions,
        };
        let actor = Actor {
            rules: &self.rules,
            git: &self.git,
            worktrees: &self.settings.worktrees,
            now: context.now,
            control,
            trash,
        };
        control.start(preview.items.len());
        let free_before = self.available();
        let items = preview.items.iter().map(|item| {
            let (outcome, removed) = clean(item, confirmation, &checker, &actor);
            control.finish_item(removed);
            ItemReport {
                candidate: item.candidate.id,
                path: item.path().to_path_buf(),
                rule: item.rule().clone(),
                found: item.estimated(),
                removed,
                outcome,
            }
        });
        let items = items.collect();
        Ok(CleanupReport {
            preview: id,
            items,
            free_before,
            free_after: self.available(),
        })
    }

    /// Cleans, with no confirmation step, the preselected candidates of
    /// exactly `categories`. There is no quick category for logs,
    /// temporary data or worktrees, and nothing that needs a confirmation
    /// of its own is cleaned.
    pub fn quick_clean(
        &mut self,
        categories: &[QuickCategory],
        candidates: Vec<Candidate>,
        context: &Context<'_>,
        control: &CleanControl,
        trash: &dyn TrashHost,
    ) -> Result<CleanupReport, CleanError> {
        let chosen = candidates.into_iter().filter(|c| {
            let quick = c.category().quick();
            c.preselected && quick.is_some_and(|q| categories.contains(&q))
        });
        let preview = self.preview(chosen.collect(), context.now);
        let nothing_else = Confirmation::default();
        self.execute(preview.id, &nothing_else, context, control, trash)
    }

    /// The preview `id`, which can be taken once, and only while it is
    /// the latest.
    fn take(&mut self, id: PreviewId) -> Result<CleanupPreview, CleanError> {
        let slot = self.latest.as_mut().filter(|(latest, _)| *latest == id);
        let (_, preview) = slot.ok_or(CleanError::UnknownPreview(id))?;
        preview.take().ok_or(CleanError::PreviewUsed(id))
    }

    fn available(&self) -> Bytes {
        volume(&self.root.path)
            .map(|v| v.available)
            .unwrap_or_default()
    }
}

/// One item: confirmed, revalidated, then acted on.
fn clean(
    item: &PreviewItem,
    confirmation: &Confirmation,
    checker: &Checker<'_>,
    actor: &Actor<'_>,
) -> (Outcome, Bytes) {
    if actor.control.is_cancelled() {
        return (Outcome::Skipped(SkipReason::Cancelled), Bytes::default());
    }
    if !confirmation.covers(item) {
        return (Outcome::Skipped(SkipReason::NotConfirmed), Bytes::default());
    }
    match checker.check(item) {
        Ok(opened) => actor.act(item, opened),
        Err(outcome) => (outcome, Bytes::default()),
    }
}
