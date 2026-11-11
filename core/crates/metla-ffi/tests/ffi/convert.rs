//! Conversions, paging and error mapping, without an engine.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metla_core::clean::{self, CleanError};
use metla_core::domain::{
    Bytes, Candidate, Category, CleanupPreview, CleanupReport, Confirmation, Coverage, DirId,
    DirRecord, DirState, Disposal, Errno, FileKind, Gap, GapAt, GapKind, ItemReport, KindTotals,
    Leftovers, Manager, Outcome, OwnTotals, Passed, PreviewId, PreviewItem, Reclaimable, RuleId,
    Separate, Skip, SkipReason, Stamp, Tally, TreeTotals, UnixTime,
};
use metla_core::engine::{
    self, Changed, Completeness, EngineError, EngineObserver as _, EngineState, Folder,
    FolderEntry, Freshness, PowerProfile,
};
use metla_core::index::IndexError;
use metla_ffi::{
    CandidatePage, CleanupResult, Confirmations, CoverageSummary, FailedPath, FolderItem,
    FolderPage, MAX_PAGE, MetlaError, Overview, ReportTotals, TrashHost, TrashOutcome, capped,
    page, rule_titles,
};

use crate::support::Told;

#[test]
fn a_page_is_never_longer_than_the_most_a_page_holds() {
    let asked = page(7, 25);
    assert_eq!((asked.offset, asked.limit), (7, 25));
    assert_eq!(page(0, MAX_PAGE + 1).limit, 1000);
    assert_eq!(page(0, MAX_PAGE).limit, 1000);
    assert_eq!(page(u64::MAX, 1).offset, usize::MAX);
    let limits = [
        (7, 7),
        (MAX_PAGE, 1000),
        (MAX_PAGE + 1, 1000),
        (u32::MAX, 1000),
    ];
    for (asked, given) in limits {
        assert_eq!(capped(asked), given, "{asked}");
    }
}

#[test]
fn engine_errors_become_typed_errors() {
    let cases = [
        (
            EngineError::Setup("no root".into()),
            MetlaError::Setup {
                reason: "no root".into(),
            },
        ),
        (EngineError::Index(IndexError::NoSpace), MetlaError::NoSpace),
        (
            EngineError::Index(IndexError::WriterGone),
            MetlaError::Index {
                reason: "index writer stopped".into(),
            },
        ),
        (
            EngineError::Clean(CleanError::Root("gone".into())),
            MetlaError::Clean {
                reason: "root: gone".into(),
            },
        ),
        (EngineError::NotInPicture, MetlaError::NotInPicture),
        (EngineError::Stopped, MetlaError::Stopped),
    ];
    for (error, expected) in cases {
        assert_eq!(MetlaError::from(error), expected);
    }
    assert_eq!(MetlaError::Stopped.to_string(), "Stopped");
}

fn row(id: i64, name: &str, state: DirState, rule: Option<&str>) -> DirRecord {
    DirRecord {
        id: DirId(id),
        parent: Some(DirId(1)),
        name: OsString::from(name),
        device: 1,
        inode: 2,
        mtime: UnixTime(3),
        changed: Stamp(4),
        state,
        tree: TreeTotals {
            bytes: Bytes(9000),
            files: 3,
            dirs: 2,
            newest: Some(UnixTime(5)),
        },
        rule: rule.and_then(RuleId::parse),
        passed: Passed::default(),
    }
}

#[test]
fn a_folder_page_says_what_each_entry_is() {
    let read = row(
        2,
        "cache",
        DirState::Read(OwnTotals::default()),
        Some("app-cache.library"),
    );
    let denied = row(3, "secret", DirState::Gap(Gap::Failed(Errno(5))), None);
    let skipped = row(4, "volume", DirState::Skipped(Skip::OtherVolume), None);
    let folder = Folder {
        dir: row(1, "top", DirState::Read(OwnTotals::default()), None),
        path: PathBuf::from("/r/top"),
        entries: vec![
            FolderEntry::Dir(Box::new(read)),
            FolderEntry::Dir(Box::new(denied)),
            FolderEntry::Dir(Box::new(skipped)),
            FolderEntry::File {
                name: OsString::from("notes.txt"),
                bytes: Bytes(4096),
            },
            FolderEntry::MoreFiles {
                files: 12,
                bytes: Bytes(77),
            },
        ],
        total: 1005,
    };
    let page = FolderPage::from(folder);
    assert_eq!((page.path, page.total), (PathBuf::from("/r/top"), 1005));
    assert_eq!(page.dir.name, "top");
    let [cache, secret, volume, file, more] = page.items.as_slice() else {
        panic!("{:?}", page.items);
    };
    let FolderItem::Dir { dir } = cache else {
        panic!("{cache:?}")
    };
    assert_eq!((dir.id, dir.name.as_str()), (DirId(2), "cache"));
    assert_eq!(
        dir.rule.as_ref().map(RuleId::as_str),
        Some("app-cache.library")
    );
    assert_eq!((dir.gap, dir.skip, dir.totals.files), (None, None, 3));
    let FolderItem::Dir { dir } = secret else {
        panic!("{secret:?}")
    };
    assert_eq!((dir.gap, dir.skip), (Some(GapKind::Failed), None));
    let FolderItem::Dir { dir } = volume else {
        panic!("{volume:?}")
    };
    assert_eq!((dir.gap, dir.skip), (None, Some(Skip::OtherVolume)));
    assert_eq!(
        *file,
        FolderItem::File {
            name: "notes.txt".into(),
            bytes: Bytes(4096)
        }
    );
    assert_eq!(
        *more,
        FolderItem::MoreFiles {
            files: 12,
            bytes: Bytes(77)
        }
    );
}

fn freshness() -> Freshness {
    Freshness {
        full_scan: Some(UnixTime(10)),
        updated: Some(UnixTime(20)),
        pending: 0,
        state: EngineState::Idle,
        picture: Completeness::Partial,
        watching: true,
        needs_full_scan: false,
        full_scans_ended: 0,
    }
}

#[test]
fn the_overview_lists_every_category_and_kind_in_a_fixed_order() {
    let mut kinds = KindTotals::default();
    kinds.add(FileKind::Media, Bytes(30));
    let mut coverage = Coverage::new(4);
    coverage.record_gap(GapAt {
        depth: 1,
        path: PathBuf::from("/r/denied"),
        gap: Gap::Denied,
    });
    coverage.record_skip(Skip::Dataless);
    let logs = Tally {
        count: 2,
        bytes: Bytes(500),
    };
    let overview = engine::Overview {
        root: PathBuf::from("/r"),
        volume: None,
        totals: None,
        categories: BTreeMap::from([(Category::Log, logs)]),
        reclaimable: Reclaimable::default(),
        reclaimable_by_category: BTreeMap::from([(
            Category::Log,
            Reclaimable {
                freed: Tally::default(),
                trashed: logs,
            },
        )]),
        owned: BTreeMap::from([(Category::Log, Bytes(400))]),
        kinds,
        coverage: Some(coverage),
        freshness: freshness(),
        profile: PowerProfile::Saver,
    };
    let shown = Overview::from(overview);
    assert_eq!(shown.root, "/r");
    let categories: Vec<_> = shown.categories.iter().map(|c| c.category).collect();
    assert_eq!(categories, Category::ALL);
    let log = &shown.categories[3];
    assert_eq!((log.tally, log.disposal), (logs, Disposal::MoveToTrash));
    assert_eq!(shown.categories[0].tally, Tally::default());
    assert_eq!(
        log.reclaimable.trashed, logs,
        "what its preselected ones do"
    );
    assert_eq!(shown.categories[0].reclaimable, Reclaimable::default());
    assert_eq!(log.owned, Bytes(400), "what it holds on the disk bar");
    assert_eq!(shown.categories[0].owned, Bytes(0));
    assert_eq!(shown.categories[4].disposal, Disposal::GitWorktreeRemove);
    let kinds: Vec<_> = shown.kinds.iter().map(|k| (k.kind, k.bytes.0)).collect();
    assert_eq!(kinds.len(), 7);
    assert_eq!(kinds[2], (FileKind::Media, 30));
    assert_eq!(kinds[0], (FileKind::Applications, 0));
    let summary: CoverageSummary = shown.coverage.unwrap();
    let gaps: Vec<_> = summary.gaps.iter().map(|g| (g.kind, g.count)).collect();
    assert_eq!(
        gaps,
        [
            (GapKind::Denied, 1),
            (GapKind::Failed, 0),
            (GapKind::Vanished, 0),
            (GapKind::Cancelled, 0)
        ]
    );
    let skips: Vec<_> = summary.skips.iter().map(|s| (s.skip, s.count)).collect();
    assert_eq!(skips[2], (Skip::Dataless, 1));
    assert_eq!(skips.len(), 4);
    assert_eq!(summary.shallowest.len(), 1);
    assert_eq!(summary.shallowest[0].path, PathBuf::from("/r/denied"));
    assert_eq!(summary.shallowest[0].kind, GapKind::Denied);
    assert_eq!(shown.profile, PowerProfile::Saver);
    assert_eq!(shown.freshness, freshness());
}

#[test]
fn confirmations_and_reports_cross_in_both_shapes() {
    let confirmed = Confirmations {
        groups: vec![Separate::MoveToTrash, Separate::MoveToTrash],
        items: vec![DirId(4), DirId(2)],
    };
    let confirmation = Confirmation::from(confirmed);
    assert_eq!(confirmation.groups.len(), 1);
    assert_eq!(
        confirmation.items.iter().copied().collect::<Vec<_>>(),
        [DirId(2), DirId(4)]
    );
    let mut leftovers = Leftovers::default();
    leftovers.fail(PathBuf::from("/r/a"), Errno(13));
    leftovers.mounts.push(PathBuf::from("/r/m"));
    leftovers.cancelled = true;
    let leftover = metla_ffi_leftover(leftovers);
    assert_eq!(leftover.failures, 1);
    assert_eq!(
        leftover.first,
        [FailedPath {
            path: PathBuf::from("/r/a"),
            errno: Errno(13)
        }]
    );
    assert_eq!(leftover.mounts, [PathBuf::from("/r/m")]);
    assert!(leftover.cancelled);
    let item = |found: u64, removed: u64, outcome: Outcome| ItemReport {
        candidate: DirId(1),
        path: PathBuf::from("/r/x"),
        rule: RuleId::parse("log.user-library").unwrap(),
        found: Bytes(found),
        removed: Bytes(removed),
        outcome,
    };
    let report = CleanupReport {
        preview: PreviewId(3),
        items: vec![
            item(100, 90, Outcome::Removed),
            item(50, 0, Outcome::MovedToTrash),
            item(7, 0, Outcome::Skipped(SkipReason::NotConfirmed)),
        ],
        free_before: Bytes(1000),
        free_after: Bytes(400),
    };
    let totals = ReportTotals {
        found: Bytes(157),
        removed: Bytes(90),
        moved_to_trash: Bytes(50),
        freed: -600,
    };
    assert_eq!(
        CleanupResult::from(Ok(report.clone())),
        CleanupResult::Done { report, totals }
    );
    let huge = CleanupReport {
        preview: PreviewId(4),
        items: Vec::new(),
        free_before: Bytes(0),
        free_after: Bytes(u64::MAX),
    };
    let CleanupResult::Done { totals, .. } = CleanupResult::from(Ok(huge)) else {
        panic!("done");
    };
    assert_eq!(
        totals.freed,
        i64::MAX,
        "measured beyond i64: the most it can say"
    );
    let refused = CleanupResult::from(Err(CleanError::UnknownPreview(PreviewId(9))));
    assert!(matches!(refused, CleanupResult::NotRun { reason } if !reason.is_empty()));
}

fn preview_item(id: i64, rule: &str, managed: Option<Manager>) -> PreviewItem {
    PreviewItem {
        candidate: Candidate {
            id: DirId(id),
            path: PathBuf::from(format!("/r/{id}")),
            rule: RuleId::parse(rule).unwrap(),
            bytes: Bytes(10),
            files: 1,
            activity: None,
            device: 1,
            inode: id as u64,
            managed,
            preselected: managed.is_none(),
        },
        warnings: Vec::new(),
    }
}

/// What executing a preview would act on, as the core decides it: the
/// quick group with the preview, a separate group when confirmed, an item
/// in a managed worktree only when confirmed by itself.
#[test]
fn a_confirmation_covers_what_the_core_says_it_covers() {
    let preview = CleanupPreview {
        id: PreviewId(1),
        items: vec![
            preview_item(1, "app-cache.library", None),
            preview_item(2, "log.user-library", None),
            preview_item(3, "build.cargo-target", Some(Manager::Codex)),
            preview_item(4, "temp.trash", None),
        ],
        rejected: Vec::new(),
    };
    let covered = |groups: Vec<Separate>, items: Vec<DirId>| {
        metla_ffi::covered(preview.clone(), Confirmations { groups, items })
    };
    assert_eq!(covered(vec![], vec![]), [DirId(1)]);
    assert_eq!(
        covered(vec![Separate::MoveToTrash], vec![]),
        [DirId(1), DirId(2)]
    );
    assert_eq!(covered(vec![], vec![DirId(3)]), [DirId(1), DirId(3)]);
    assert_eq!(
        covered(
            vec![Separate::EmptyTrash, Separate::MoveToTrash],
            vec![DirId(3)]
        ),
        [DirId(1), DirId(2), DirId(3), DirId(4)]
    );
}

fn metla_ffi_leftover(leftovers: Leftovers) -> metla_ffi::Leftover {
    metla_ffi::Leftover::from(leftovers)
}

#[test]
fn a_candidate_page_carries_its_total() {
    let page = engine::CandidatePage {
        listed: Vec::new(),
        total: 1234,
    };
    assert_eq!(CandidatePage::from(page).total, 1234);
}

#[test]
fn every_rule_has_its_english_title_by_id() {
    let titles = rule_titles();
    assert_eq!(titles.len(), metla_core::rules::CATALOGUE.len());
    let cargo = titles
        .iter()
        .find(|t| t.rule.as_str() == "build.cargo-target");
    assert_eq!(cargo.unwrap().title, "Cargo build output");
    assert!(titles.iter().all(|t| !t.title.is_empty()));
}

/// A Trash that moves everything, or refuses with a reason.
struct Host(Option<&'static str>);

impl TrashHost for Host {
    fn trash(&self, path: String) -> TrashOutcome {
        assert_eq!(path, "/r/logs");
        match self.0 {
            None => TrashOutcome::Moved,
            Some(reason) => TrashOutcome::Refused {
                reason: reason.into(),
            },
        }
    }
}

#[test]
fn the_apps_trash_and_observer_are_called_as_the_engine_calls_them() {
    use metla_core::clean::TrashHost as _;
    let moved = metla_ffi_trash(Host(None));
    assert_eq!(moved.trash(Path::new("/r/logs")), Ok(()));
    let refused = metla_ffi_trash(Host(Some("full")));
    assert_eq!(refused.trash(Path::new("/r/logs")), Err("full".into()));
    let told = Arc::new(Told::default());
    let observer = metla_ffi_observer(told.clone());
    observer.changed(Changed::Exclusions);
    assert_eq!(*told.0.lock().unwrap(), [Changed::Exclusions]);
}

fn metla_ffi_trash(host: Host) -> impl clean::TrashHost {
    metla_ffi::Trash(Arc::new(host))
}

fn metla_ffi_observer(told: Arc<Told>) -> impl engine::EngineObserver {
    metla_ffi::Observer(told)
}
