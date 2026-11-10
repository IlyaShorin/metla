//! Nothing built from this crate can clean a root that is not a
//! fixture.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use metla_core::domain::{PreviewId, QuickCategory, Separate};
use metla_core::engine::Changed;
use metla_ffi::{CleanupResult, Confirmations, Engine, MetlaError};

use crate::support::{Place, Refusing, Told, until};

/// The cfg under which the boundary makes the production capability:
/// a release build with the `production` feature, which only the app's
/// release build asks for (scripts/build-core.sh).
const RELEASE_APP: &str = "#[cfg(all(feature = \"production\", not(debug_assertions)))]";

#[test]
fn only_the_release_apps_build_can_hold_the_production_capability() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("production = [\"metla-core/production\"]"));
    assert!(!manifest.contains("default ="), "never on by default");
    assert!(!manifest.contains("features = [\"production\"]"));
    let mut sources = vec![crate_dir.join("src/bin/uniffi-bindgen-swift.rs")];
    for entry in fs::read_dir(crate_dir.join("src")).unwrap() {
        sources.push(entry.unwrap().path());
    }
    let mut made = 0;
    for path in sources.iter().filter(|p| p.is_file()) {
        let source = fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = source.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            if line.contains("release_app") {
                made += 1;
                let function = lines[..at].iter().rev().find(|l| l.contains("fn "));
                let gate = lines[..at].iter().rev().find(|l| l.contains("#[cfg"));
                assert_eq!(
                    gate.map(|l| l.trim()),
                    Some(RELEASE_APP),
                    "{}",
                    path.display()
                );
                assert!(function.is_some_and(|f| f.contains("fn production()")));
            }
        }
    }
    assert_eq!(made, 1, "made in exactly one place");
}

/// What a build of the tests makes: no capability, whatever the
/// features.
#[test]
fn a_test_build_has_no_production_capability() {
    assert!(!metla_ffi::cleans_beyond_fixtures());
}

#[test]
fn a_root_without_the_fixture_marker_cannot_be_cleaned() {
    let place = Place::new(&["~/Library/Caches/com.example.app/blob"]);
    fs::remove_file(place.root.join(metla_core::clean::FIXTURE_MARKER)).unwrap();
    let engine = Engine::open(place.options(), Arc::new(Refusing)).unwrap();
    until(|| engine.freshness().unwrap().full_scan.is_some());
    let caches = engine.candidates(None, Default::default(), 0, 10).unwrap();
    let ids = caches.listed.iter().map(|l| l.candidate.id).collect();
    assert!(matches!(engine.preview(ids), Err(MetlaError::Clean { .. })));
    let confirmed = Confirmations {
        groups: vec![Separate::MoveToTrash],
        items: Vec::new(),
    };
    let told = Arc::new(Told::default());
    engine.set_observer(told.clone());
    let cleanups = || {
        let topics = told.0.lock().unwrap();
        topics.iter().filter(|t| **t == Changed::Cleanup).count()
    };
    engine.execute_confirmed(PreviewId(1), confirmed).unwrap();
    until(|| cleanups() == 1);
    assert!(matches!(
        engine.last_report(),
        Some(CleanupResult::NotRun { .. })
    ));
    engine.quick_clean(vec![QuickCategory::AppCache]).unwrap();
    until(|| cleanups() == 2);
    assert!(matches!(
        engine.last_report(),
        Some(CleanupResult::NotRun { .. })
    ));
    assert!(place.path("~/Library/Caches/com.example.app/blob").exists());
}

#[test]
fn an_engine_that_cannot_open_says_why() {
    let place = Place::new(&[]);
    let mut options = place.options();
    options.root = place.root.join("missing").to_string_lossy().into_owned();
    let opened = Engine::open(options, Arc::new(Refusing));
    assert!(matches!(opened, Err(MetlaError::Setup { .. })));
}
