//! The engine through the boundary, on a synthetic fixture.

use std::fs;
use std::sync::{Arc, Mutex, Weak};

use metla_core::domain::{Category, Disposal, Outcome, QuickCategory, Separate};
use metla_core::engine::{Changed, Completeness, DirRef, EngineState, PowerProfile};
use metla_core::index::{Order, SortKey};
use metla_ffi::{
    CleanupResult, Confirmations, Engine, EngineObserver, FolderItem, MAX_PAGE, MetlaError,
};

use crate::support::{HOME, Place, Told, text, until};

#[test]
fn the_overview_comes_through_whole() {
    let place = Place::new(HOME);
    let engine = place.open();
    let overview = engine.snapshot().unwrap();
    assert_eq!(overview.root, text(&place.root));
    assert_eq!(overview.freshness.picture, Completeness::Complete);
    assert!(overview.totals.unwrap().files >= 4);
    let caches = &overview.categories[0];
    assert_eq!(caches.category, Category::AppCache);
    assert_eq!(
        (caches.tally.count, caches.disposal),
        (2, Disposal::DeletePermanently)
    );
    assert_eq!(
        caches.reclaimable.freed, caches.tally,
        "both caches preselected"
    );
    let freed: u64 = overview
        .categories
        .iter()
        .map(|c| c.reclaimable.freed.bytes.0)
        .sum();
    assert_eq!(freed, overview.reclaimable.freed.bytes.0, "the rows add up");
    assert_eq!(overview.kinds.len(), 7);
    assert_eq!(overview.coverage.unwrap().gaps.len(), 4);
    assert!(overview.volume.is_some());
    assert_eq!(overview.profile, PowerProfile::Background);
    assert_eq!(engine.progress(), None);
    assert_eq!(engine.freshness().unwrap().state, EngineState::Idle);
}

#[test]
fn candidates_come_in_pages_in_the_order_asked_for() {
    let place = Place::new(HOME);
    fs::write(
        place.path("~/Library/Caches/org.example.other/more"),
        "y".repeat(9000),
    )
    .unwrap();
    let engine = Place::open(&place);
    engine.start_scan().unwrap();
    until(|| {
        engine
            .candidates(None, Order::default(), 0, 10)
            .unwrap()
            .listed[0]
            .candidate
            .bytes
            .0
            > 10_000
    });
    let by_path = Order {
        key: SortKey::Path,
        descending: false,
    };
    let page = engine
        .candidates(Some(Category::AppCache), by_path, 0, 1)
        .unwrap();
    assert_eq!(page.total, 2);
    assert!(page.listed[0].candidate.path.ends_with("com.example.app"));
    let largest = engine
        .candidates(Some(Category::AppCache), Order::default(), 0, 5)
        .unwrap();
    assert!(
        largest.listed[0]
            .candidate
            .path
            .ends_with("org.example.other")
    );
    let second = engine
        .candidates(Some(Category::AppCache), by_path, 1, 5)
        .unwrap();
    assert_eq!(second.listed.len(), 1);
    let all = engine
        .candidates(None, Order::default(), 0, MAX_PAGE + 5)
        .unwrap();
    assert_eq!(all.total as usize, all.listed.len());
    assert!(all.listed.iter().all(|l| l.removability.is_none()));
}

#[test]
fn a_folder_is_listed_by_path_and_by_id() {
    let place = Place::new(HOME);
    let engine = place.open();
    let home = engine
        .children(DirRef::Path(place.path("~")), 0, 10)
        .unwrap();
    assert_eq!(home.path, place.path("~"));
    let FolderItem::Dir { dir } = &home.items[0] else {
        panic!("{:?}", home.items);
    };
    assert_eq!(dir.name, "Library");
    let library = engine.children(DirRef::Id(dir.id), 0, 10).unwrap();
    assert_eq!(library.path, place.path("~/Library"));
    assert_eq!(library.total, 2);
    let missing = engine.children(DirRef::Path(place.path("nowhere")), 0, 10);
    assert_eq!(missing.unwrap_err(), MetlaError::NotInPicture);
    assert!(engine.largest_folders(MAX_PAGE + 1).unwrap().len() <= 1000);
    assert_eq!(engine.largest_folders(1).unwrap().len(), 1);
}

#[test]
fn exclusions_round_trip_and_are_checked_where_they_come_in() {
    let place = Place::new(HOME);
    let engine = place.open();
    let caches = text(&place.path("~/Library/Caches"));
    assert!(engine.add_exclusion(caches.clone()).unwrap());
    assert!(!engine.add_exclusion(caches.clone()).unwrap());
    assert_eq!(
        engine.exclusions(0, 10).unwrap(),
        std::slice::from_ref(&caches)
    );
    assert!(engine.exclusions(1, 10).unwrap().is_empty());
    let offered = engine.candidates(Some(Category::AppCache), Order::default(), 0, 10);
    assert_eq!(offered.unwrap().total, 0);
    assert!(engine.remove_exclusion(caches.clone()).unwrap());
    assert!(!engine.remove_exclusion(caches).unwrap());
    let relative = engine.add_exclusion("Library/../x".into());
    assert_eq!(
        relative.unwrap_err(),
        MetlaError::InvalidPath {
            path: "Library/../x".into()
        }
    );
    assert!(engine.remove_exclusion(String::new()).is_err());
}

/// An observer that reads the engine from inside the notification.
struct Reentrant {
    engine: Mutex<Weak<Engine>>,
    read: Mutex<u32>,
}

impl EngineObserver for Reentrant {
    fn changed(&self, _: Changed) {
        let engine = self.engine.lock().unwrap().upgrade();
        if let Some(engine) = engine {
            engine.freshness().unwrap();
            engine.snapshot().unwrap();
            *self.read.lock().unwrap() += 1;
        }
    }
}

#[test]
fn the_observer_may_call_back_into_the_engine() {
    let place = Place::new(HOME);
    let engine = place.open();
    let observer = Arc::new(Reentrant {
        engine: Mutex::new(Arc::downgrade(&engine)),
        read: Mutex::new(0),
    });
    engine.set_observer(observer.clone());
    engine.set_attention(true).unwrap();
    engine.set_power_profile(PowerProfile::Saver).unwrap();
    engine.start_scan().unwrap();
    until(|| *observer.read.lock().unwrap() >= 3);
    engine.cancel_scan();
    engine.set_attention(false).unwrap();
}

#[test]
fn a_quick_clean_takes_caches_only_and_reports() {
    let place = Place::new(HOME);
    let engine = place.open();
    let told = Arc::new(Told::default());
    engine.set_observer(told.clone());
    assert_eq!(engine.cleanup_progress(), None);
    assert_eq!(engine.last_report(), None);
    engine.quick_clean(vec![QuickCategory::AppCache]).unwrap();
    until(|| engine.last_report().is_some());
    let Some(CleanupResult::Done { report }) = engine.last_report() else {
        panic!("{:?}", engine.last_report());
    };
    assert_eq!(report.items.len(), 2);
    assert!(report.items.iter().all(|i| i.outcome == Outcome::Removed));
    assert!(
        place
            .path("~/Library/Logs/com.example.app/today.log")
            .exists()
    );
    assert!(told.0.lock().unwrap().contains(&Changed::Cleanup));
    engine.cancel_cleanup();
}

#[test]
fn a_confirmed_preview_moves_logs_through_the_apps_trash() {
    let place = Place::new(HOME);
    let engine = place.open();
    let logs = engine
        .candidates(Some(Category::Log), Order::default(), 0, 10)
        .unwrap();
    let ids = logs.listed.iter().map(|l| l.candidate.id).collect();
    let preview = engine.preview(ids).unwrap();
    assert_eq!(preview.items.len(), 1);
    let confirmed = Confirmations {
        groups: vec![Separate::MoveToTrash],
        items: Vec::new(),
    };
    engine.execute_confirmed(preview.id, confirmed).unwrap();
    until(|| engine.last_report().is_some());
    let Some(CleanupResult::Done { report }) = engine.last_report() else {
        panic!("{:?}", engine.last_report());
    };
    let refused = &report.items[0].outcome;
    assert!(
        matches!(refused, Outcome::Failed(why) if why.ends_with("no Trash here")),
        "{refused:?}"
    );
    assert!(
        place
            .path("~/Library/Logs/com.example.app/today.log")
            .exists()
    );
    engine
        .execute_confirmed(preview.id, Confirmations::default())
        .unwrap();
    until(|| matches!(engine.last_report(), Some(CleanupResult::NotRun { .. })));
}
