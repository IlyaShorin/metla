//! Records for the shapes the boundary cannot carry as the core holds
//! them: maps become lists in a fixed order, private fields become
//! counts, tuples become records, sizes become `u64`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use metla_core::clean::CleanError;
use metla_core::domain::{
    Bytes, Category, CleanupReport, Confirmation, Coverage, DirId, DirRecord, DirState, Disposal,
    Errno, FileKind, GapKind, Group, KindTotals, Leftovers, QuickCategory, Reclaimable, RuleId,
    Separate, Skip, Tally, TreeTotals,
};
use metla_core::engine::{
    self, CandidatePage as Page, Folder, FolderEntry, Freshness, HotkeyMode, Listed, PowerProfile,
    Settings, VolumeSpace,
};
use metla_core::rules::CATALOGUE;

/// Everything the Overview shows, from one read of the picture.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Overview {
    pub root: String,
    pub volume: Option<VolumeSpace>,
    /// The root's totals; `None` without a picture.
    pub totals: Option<TreeTotals>,
    /// Every category, in the order of `Category::ALL`.
    pub categories: Vec<CategoryTally>,
    pub reclaimable: Reclaimable,
    /// Every file kind, in the order of `FileKind::ALL`.
    pub kinds: Vec<KindBytes>,
    pub coverage: Option<CoverageSummary>,
    pub freshness: Freshness,
    pub profile: PowerProfile,
}

/// A category's offered candidates, how the category is removed, and
/// what cleaning its preselected candidates would do (by their rules'
/// disposal: the Trash's contents are deleted, not moved).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CategoryTally {
    pub category: Category,
    pub disposal: Disposal,
    /// Every offered candidate.
    pub tally: Tally,
    /// The preselected ones; the categories' add up to the Overview's.
    pub reclaimable: Reclaimable,
    /// What the category holds on the disk bar: a candidate inside
    /// another (build output in a worktree) counts with the outer one,
    /// so the categories add up to no more than the analysed size.
    pub owned: Bytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct KindBytes {
    pub kind: FileKind,
    pub bytes: Bytes,
}

/// What the picture could not see: gaps and skips by kind (every kind,
/// in a fixed order) and the shallowest gaps (at most a scan's limit).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CoverageSummary {
    pub gaps: Vec<GapCount>,
    pub skips: Vec<SkipCount>,
    pub shallowest: Vec<GapPlace>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct GapCount {
    pub kind: GapKind,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SkipCount {
    pub skip: Skip,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct GapPlace {
    pub path: PathBuf,
    pub kind: GapKind,
}

impl From<engine::Overview> for Overview {
    fn from(overview: engine::Overview) -> Overview {
        Overview {
            root: overview.root.to_string_lossy().into_owned(),
            volume: overview.volume,
            totals: overview.totals,
            categories: Category::ALL
                .map(|category| CategoryTally {
                    category,
                    disposal: category.disposal(),
                    tally: overview
                        .categories
                        .get(&category)
                        .copied()
                        .unwrap_or_default(),
                    reclaimable: overview
                        .reclaimable_by_category
                        .get(&category)
                        .copied()
                        .unwrap_or_default(),
                    owned: overview.owned.get(&category).copied().unwrap_or_default(),
                })
                .into(),
            reclaimable: overview.reclaimable,
            kinds: kinds(&overview.kinds),
            coverage: overview.coverage.as_ref().map(CoverageSummary::from),
            freshness: overview.freshness,
            profile: overview.profile,
        }
    }
}

fn kinds(totals: &KindTotals) -> Vec<KindBytes> {
    let kinds = FileKind::ALL.map(|kind| KindBytes {
        kind,
        bytes: totals.get(kind),
    });
    kinds.into()
}

impl From<&Coverage> for CoverageSummary {
    fn from(coverage: &Coverage) -> CoverageSummary {
        let gaps = GapKind::ALL.map(|kind| GapCount {
            kind,
            count: coverage.gaps(kind),
        });
        let skips = Skip::ALL.map(|skip| SkipCount {
            skip,
            count: coverage.skips(skip),
        });
        let places = coverage.top_gaps().iter().map(|at| GapPlace {
            path: at.path.clone(),
            kind: at.gap.kind(),
        });
        CoverageSummary {
            gaps: gaps.into(),
            skips: skips.into(),
            shallowest: places.collect(),
        }
    }
}

/// One page of candidates and how many are offered in all.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CandidatePage {
    pub listed: Vec<Listed>,
    pub total: u64,
}

impl From<Page> for CandidatePage {
    fn from(page: Page) -> CandidatePage {
        CandidatePage {
            listed: page.listed,
            total: page.total as u64,
        }
    }
}

/// A directory of the picture as a folder listing shows it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FolderDir {
    pub id: DirId,
    pub name: String,
    pub totals: TreeTotals,
    /// Its rule, when it is a candidate (offered or not).
    pub rule: Option<RuleId>,
    /// Why it was not read, if it was not.
    pub gap: Option<GapKind>,
    pub skip: Option<Skip>,
}

impl From<DirRecord> for FolderDir {
    fn from(row: DirRecord) -> FolderDir {
        let (gap, skip) = match row.state {
            DirState::Read(_) => (None, None),
            DirState::Gap(gap) => (Some(gap.kind()), None),
            DirState::Skipped(skip) => (None, Some(skip)),
        };
        FolderDir {
            id: row.id,
            name: row.name.to_string_lossy().into_owned(),
            totals: row.tree,
            rule: row.rule,
            gap,
            skip,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FolderItem {
    Dir {
        dir: FolderDir,
    },
    File {
        name: String,
        bytes: Bytes,
    },
    /// The files past the most a listing keeps; always last.
    MoreFiles {
        files: u64,
        bytes: Bytes,
    },
}

impl From<FolderEntry> for FolderItem {
    fn from(entry: FolderEntry) -> FolderItem {
        match entry {
            FolderEntry::Dir(row) => FolderItem::Dir {
                dir: FolderDir::from(*row),
            },
            FolderEntry::File { name, bytes } => FolderItem::File {
                name: name.to_string_lossy().into_owned(),
                bytes,
            },
            FolderEntry::MoreFiles { files, bytes } => FolderItem::MoreFiles { files, bytes },
        }
    }
}

/// One page of a directory's contents, largest first.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FolderPage {
    pub dir: FolderDir,
    pub path: PathBuf,
    pub items: Vec<FolderItem>,
    /// Entries on every page.
    pub total: u64,
}

impl From<Folder> for FolderPage {
    fn from(folder: Folder) -> FolderPage {
        FolderPage {
            dir: FolderDir::from(folder.dir),
            path: folder.path,
            items: folder.entries.into_iter().map(FolderItem::from).collect(),
            total: folder.total as u64,
        }
    }
}

/// What the user confirmed on top of a preview: the separate groups, and
/// the items that need a confirmation of their own.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct Confirmations {
    pub groups: Vec<Separate>,
    pub items: Vec<DirId>,
}

impl From<Confirmations> for Confirmation {
    fn from(confirmed: Confirmations) -> Confirmation {
        Confirmation {
            groups: confirmed.groups.into_iter().collect::<BTreeSet<_>>(),
            items: confirmed.items.into_iter().collect::<BTreeSet<_>>(),
        }
    }
}

/// A path a cleanup could not remove, and why.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FailedPath {
    pub path: PathBuf,
    pub errno: Errno,
}

/// What a partial removal left behind.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Leftover {
    pub failures: u64,
    pub first: Vec<FailedPath>,
    pub mounts: Vec<PathBuf>,
    pub cancelled: bool,
}

impl From<Leftovers> for Leftover {
    fn from(leftovers: Leftovers) -> Leftover {
        let first = leftovers
            .first
            .into_iter()
            .map(|(path, errno)| FailedPath { path, errno });
        Leftover {
            failures: leftovers.failures,
            first: first.collect(),
            mounts: leftovers.mounts,
            cancelled: leftovers.cancelled,
        }
    }
}

/// The last cleanup: its report, or why it did not run.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CleanupResult {
    Done { report: CleanupReport },
    NotRun { reason: String },
}

impl From<Result<CleanupReport, CleanError>> for CleanupResult {
    fn from(result: Result<CleanupReport, CleanError>) -> CleanupResult {
        match result {
            Ok(report) => CleanupResult::Done { report },
            Err(error) => CleanupResult::NotRun {
                reason: error.to_string(),
            },
        }
    }
}

/// A rule's id and its English title; the app localises by id.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuleTitle {
    pub rule: RuleId,
    pub title: String,
}

/// Every rule of the catalogue with its English title.
#[uniffi::export]
pub fn rule_titles() -> Vec<RuleTitle> {
    let titles = CATALOGUE.iter().map(|rule| RuleTitle {
        rule: RuleId::new(rule.category, rule.name),
        title: rule.title.to_owned(),
    });
    titles.collect()
}

/// The settings the engine keeps, as they cross: the quick categories as
/// a list (once each, in a fixed order on the way out).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct EngineSettings {
    /// Watch the disk and keep the picture fresh.
    pub background: bool,
    /// Use the Saver profile on battery (the app decides).
    pub reduce_on_battery: bool,
    /// Build output active within this age is not preselected.
    pub recent_age: Duration,
    /// A worktree changed within this age is in use.
    pub stale_after: Duration,
    pub hotkey: HotkeyMode,
    /// What the hotkey cleans in `Immediate` mode: there is no value for
    /// logs, worktrees or temporary data.
    pub quick: Vec<QuickCategory>,
}

impl From<Settings> for EngineSettings {
    fn from(settings: Settings) -> EngineSettings {
        EngineSettings {
            background: settings.background,
            reduce_on_battery: settings.reduce_on_battery,
            recent_age: settings.recent_age,
            stale_after: settings.stale_after,
            hotkey: settings.hotkey,
            quick: settings.quick.into_iter().collect(),
        }
    }
}

impl From<EngineSettings> for Settings {
    fn from(settings: EngineSettings) -> Settings {
        Settings {
            background: settings.background,
            reduce_on_battery: settings.reduce_on_battery,
            recent_age: settings.recent_age,
            stale_after: settings.stale_after,
            hotkey: settings.hotkey,
            quick: settings.quick.into_iter().collect(),
        }
    }
}

/// How a preview item with the rule `rule` is confirmed: with the
/// preview (`Quick`) or in a group of its own.
#[uniffi::export]
pub fn group_of(rule: RuleId) -> Group {
    Group::of(&rule)
}
