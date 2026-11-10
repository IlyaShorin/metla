//! Nothing built from this crate can clean a root that is not a
//! fixture.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use metla_ffi::{Engine, MetlaError};

use crate::support::{Place, Refusing, until};

#[test]
fn the_boundary_never_holds_the_production_capability() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
    assert!(!manifest.contains("features = [\"production\"]"));
    let mut sources = vec![crate_dir.join("src/bin/uniffi-bindgen-swift.rs")];
    for entry in fs::read_dir(crate_dir.join("src")).unwrap() {
        sources.push(entry.unwrap().path());
    }
    for path in sources.iter().filter(|p| p.is_file()) {
        let source = fs::read_to_string(path).unwrap();
        assert!(!source.contains("Production"), "{}", path.display());
        assert!(!source.contains("release_app"), "{}", path.display());
    }
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
