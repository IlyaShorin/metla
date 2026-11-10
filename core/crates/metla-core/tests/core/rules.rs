//! The rule set as data and its pure parts: the catalogue against
//! docs/RULES.md, markers, probes, file kinds and areas.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metla_core::domain::{Category, DirFacts, Entry, FileKind, Probe, RuleId};
use metla_core::rules::{Area, PROBE_LIMIT, Removes, RuleSet};
use metla_core::scan::ScanControl;

use crate::scan::{options, scan_with};
use crate::support::{Fixture, write_file};

const HOME: &str = "/Users/tester";

fn rules() -> RuleSet {
    RuleSet::new(Path::new(HOME), OsStr::new("tester"))
}

/// Every backticked id in the first column of a table of RULES.md.
fn documented_ids() -> BTreeSet<String> {
    let doc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/RULES.md");
    let text = std::fs::read_to_string(doc).unwrap();
    let ids = text.lines().filter_map(|line| {
        let id = line.strip_prefix("| `")?.split('`').next()?;
        id.contains('.').then(|| id.to_owned())
    });
    ids.collect()
}

#[test]
fn the_catalogue_holds_exactly_the_rules_documented_in_rules_md() {
    let rules = rules();
    let ids: Vec<String> = rules.rules().map(|(id, _)| id.to_string()).collect();
    let unique: BTreeSet<String> = ids.iter().cloned().collect();
    assert_eq!(unique.len(), ids.len(), "a rule id appears twice");
    assert_eq!(unique, documented_ids());
    assert_eq!(ids.len(), 60);
}

/// The rules whose row of RULES.md says they keep their directory.
fn documented_keepers() -> BTreeSet<String> {
    let doc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/RULES.md");
    let text = std::fs::read_to_string(doc).unwrap();
    let rows = text.lines().filter_map(|line| {
        let mut cells = line.strip_prefix("| `")?.split('|');
        let id = cells.next()?.trim().trim_end_matches('`');
        (cells.next()?.trim() == "yes").then(|| id.to_owned())
    });
    rows.collect()
}

#[test]
fn the_rules_that_keep_their_directory_are_the_documented_ones() {
    let rules = rules();
    let keepers: BTreeSet<String> = rules
        .rules()
        .filter(|(_, rule)| rule.removes == Removes::Contents)
        .map(|(id, _)| id.to_string())
        .collect();
    assert_eq!(keepers, documented_keepers());
    assert_eq!(keepers.len(), 30);
    assert!(keepers.contains("temp.trash"));
    assert!(!keepers.iter().any(|id| id.starts_with("build.")));
}

#[test]
fn every_rule_says_what_it_is_and_why_removing_it_is_safe() {
    for (id, rule) in rules().rules() {
        assert!(!rule.title.is_empty(), "{id} has no title");
        assert!(rule.why.ends_with('.'), "{id}: {:?}", rule.why);
        assert_eq!(id.category(), rule.category);
    }
}

#[test]
fn a_rule_is_found_by_its_id() {
    let rules = rules();
    let npm = RuleId::new(Category::DevCache, "npm");
    assert_eq!(rules.rule(&npm).unwrap().title, "npm cache");
    assert!(
        rules
            .rule(&RuleId::new(Category::DevCache, "nope"))
            .is_none()
    );
}

fn facts<'a>(path: &'a Path, probes: &'a [Probe]) -> DirFacts<'a> {
    DirFacts {
        path,
        entries: &[],
        parent_markers: &[],
        grandparent_markers: &[],
        inside_git_repo: false,
        probes,
    }
}

#[test]
fn the_bazel_output_base_is_the_users_own_under_var_tmp() {
    let rules = rules();
    let bazel = |path: &str| rules.classify(&facts(Path::new(path), &[])).cloned();
    assert_eq!(
        bazel("/private/var/tmp/_bazel_tester"),
        Some(RuleId::new(Category::DevCache, "bazel"))
    );
    assert_eq!(bazel("/private/var/tmp/_bazel_other"), None);
    assert_eq!(bazel("/private/var/tmp/_bazel_tester/x"), None);
    assert_eq!(bazel("/var/tmp/_bazel_tester"), None);
    assert_eq!(bazel("/"), None);
}

#[test]
fn a_directory_below_the_deepest_pattern_is_never_matched_by_its_prefix() {
    let rules = rules();
    let profile = "/Users/tester/Library/Application Support/Google/Chrome/Default";
    let markers = [OsString::from("Preferences")];
    let storage = format!("{profile}/Service Worker/CacheStorage");
    let inside = format!("{storage}/x");
    let classify = |path: &str, grandparent: &[OsString]| {
        let facts = DirFacts {
            grandparent_markers: grandparent,
            ..facts(Path::new(path), &[])
        };
        rules.classify(&facts).cloned()
    };
    assert!(classify(&storage, &markers).is_some());
    assert_eq!(classify(&inside, &markers), None);
}

#[test]
fn the_home_directory_is_given_never_read_from_the_environment() {
    let elsewhere = RuleSet::new(Path::new("/Users/someone"), OsStr::new("someone"));
    let path = Path::new("/Users/tester/Library/Caches/Homebrew");
    assert!(rules().classify(&facts(path, &[])).is_some());
    assert!(elsewhere.classify(&facts(path, &[])).is_none());
}

#[test]
fn sibling_markers_and_location_conditions_are_reported_markers() {
    let rules = rules();
    for name in [
        "Cargo.toml",
        "package.json",
        "next.config.mjs",
        "App.xcodeproj",
        "main.tf",
        "x.cabal",
        "Preferences",
        "Local Storage",
    ] {
        assert!(rules.is_marker(OsStr::new(name)), "{name}");
    }
    for name in [
        "README.md",
        "Cargo.lock",
        "CACHEDIR.TAG",
        ".git",
        "next.config",
    ] {
        assert!(!rules.is_marker(OsStr::new(name)), "{name}");
    }
}

#[test]
fn the_rules_probe_git_pointers_and_cache_tags_with_a_bounded_read() {
    assert_eq!(rules().probes(), [".git", "CACHEDIR.TAG"]);
    assert!((256..=4096).contains(&PROBE_LIMIT));
}

#[test]
fn a_probe_of_another_name_is_not_a_worktree_pointer() {
    let probes = [Probe {
        name: OsString::from("CACHEDIR.TAG"),
        head: b"gitdir: /r/.git/worktrees/w".to_vec(),
    }];
    let path = Path::new("/w");
    assert_eq!(rules().classify(&facts(path, &probes)), None);
}

#[test]
fn files_of_the_users_own_are_sorted_by_extension() {
    let rules = rules();
    let kind = |name: &str| rules.kind(Area::User, OsStr::new(name));
    let cases = [
        ("photo.JPG", FileKind::Media),
        ("clip.mov", FileKind::Media),
        ("song.flac", FileKind::Media),
        ("report.pdf", FileKind::Documents),
        ("a.pdf", FileKind::Documents),
        ("notes.md", FileKind::Documents),
        ("backup.tar.gz", FileKind::Archives),
        ("disk.dmg", FileKind::Archives),
        ("big.sparseimage", FileKind::Archives),
        (".jpg", FileKind::Other),
        ("noextension", FileKind::Other),
        ("trailing.", FileKind::Other),
        ("main.rs", FileKind::Other),
        ("x.abcdefghijklmnopq", FileKind::Other),
    ];
    for (name, expected) in cases {
        assert_eq!(kind(name), expected, "{name}");
    }
}

#[test]
fn where_a_file_is_decides_before_its_extension() {
    let rules = rules();
    let photo = OsStr::new("photo.jpg");
    assert_eq!(rules.kind(Area::Developer, photo), FileKind::Developer);
    assert_eq!(
        rules.kind(Area::Applications, photo),
        FileKind::Applications
    );
    assert_eq!(rules.kind(Area::System, photo), FileKind::System);
}

#[test]
fn a_directory_takes_the_strongest_of_its_parents_area_and_its_own() {
    let rules = rules();
    let area = |parent, path: &str, in_repo, rule: Option<RuleId>| {
        rules.area(parent, Path::new(path), in_repo, rule.as_ref())
    };
    let cache = Some(RuleId::new(Category::DevCache, "npm"));
    let build = Some(RuleId::new(Category::BuildArtifact, "node-modules"));
    let app_cache = Some(RuleId::new(Category::AppCache, "library"));
    let cases = [
        (
            area(Area::User, "/Users/tester/Movies", false, None),
            Area::User,
        ),
        (area(Area::User, "/System", false, None), Area::System),
        (area(Area::User, "/private", false, None), Area::System),
        (area(Area::User, "/usr", false, None), Area::System),
        (area(Area::User, "/Users/x/System", false, None), Area::User),
        (
            area(Area::User, "/Applications/X.app", false, None),
            Area::Applications,
        ),
        (
            area(Area::System, "/System/Applications/Y.app", false, None),
            Area::Applications,
        ),
        (
            area(Area::Applications, "/Applications/X.app/lib", false, None),
            Area::Applications,
        ),
        (
            area(Area::Applications, "/X.app/repo", true, None),
            Area::Developer,
        ),
        (
            area(Area::User, "/Users/tester/Library/Developer", false, None),
            Area::Developer,
        ),
        (
            area(Area::User, "/Users/tester/Library", false, None),
            Area::User,
        ),
        (
            area(Area::User, "/Users/tester/.npm/_cacache", false, cache),
            Area::Developer,
        ),
        (
            area(Area::User, "/r/node_modules", false, build),
            Area::Developer,
        ),
        (
            area(
                Area::User,
                "/Users/tester/Library/Caches/a",
                false,
                app_cache,
            ),
            Area::User,
        ),
        (area(Area::Developer, "/usr", false, None), Area::Developer),
        (area(Area::System, "/private/x", false, None), Area::System),
    ];
    for (n, (got, expected)) in cases.into_iter().enumerate() {
        assert_eq!(got, expected, "case {n}");
    }
}

#[test]
fn a_scan_root_inherits_the_area_of_the_directories_above_it() {
    let rules = rules();
    let above = |path: &str| rules.area_above(Path::new(path));
    assert_eq!(above("/"), Area::User);
    assert_eq!(above("/System"), Area::User);
    assert_eq!(above("/System/Library"), Area::System);
    assert_eq!(above("/Applications/X.app/Contents"), Area::Applications);
    assert_eq!(
        above("/Users/tester/Library/Developer/Xcode"),
        Area::Developer
    );
    assert_eq!(above("/Users/tester/Documents"), Area::User);
}

#[test]
fn a_scan_totals_the_bytes_of_every_file_kind() {
    let fx = Fixture::new();
    fx.build(&[
        "repo/.git/",
        "repo/package.json",
        "repo/node_modules/",
        "home/.npm/_cacache/",
        "Tool.app/Contents/",
    ]);
    let sizes = [
        ("plain.jpg", 10_000),
        ("repo/main.c", 20_000),
        ("repo/node_modules/x.js", 30_000),
        ("home/.npm/_cacache/blob", 40_000),
        ("Tool.app/Contents/binary", 50_000),
    ];
    for (file, len) in sizes {
        write_file(&fx.path(file), len);
    }
    let allocated = |file: &str| {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(fx.path(file)).unwrap().blocks() * 512
    };
    let rules = RuleSet::new(&fx.home(), OsStr::new("tester"));
    let options = options(fx.root()).rules(Arc::new(rules));
    let summary = scan_with(&options, &ScanControl::new(64)).summary;
    let kinds = summary.kinds;
    // A photo of the user's own is Media, unless the temporary directory
    // lies in a system area (below /private, the default on macOS).
    let photo = match fx.root().starts_with("/private") {
        true => FileKind::System,
        false => FileKind::Media,
    };
    assert_eq!(kinds.get(photo).0, allocated("plain.jpg"));
    let developer = [
        "repo/main.c",
        "repo/node_modules/x.js",
        "home/.npm/_cacache/blob",
    ];
    let developer: u64 = developer.iter().map(|f| allocated(f)).sum();
    assert_eq!(kinds.get(FileKind::Developer).0, developer);
    assert_eq!(
        kinds.get(FileKind::Applications).0,
        allocated("Tool.app/Contents/binary")
    );
    assert_eq!(kinds.total(), summary.totals.bytes);
}

#[test]
fn a_scan_without_rules_classifies_nothing_and_totals_no_kinds() {
    let fx = Fixture::new();
    fx.build(&[
        "home/Library/Caches/Homebrew/",
        "repo/.git/",
        "repo/Cargo.toml",
        "repo/target/",
    ]);
    write_file(&fx.path("repo/target/x"), 4096);
    let scanned = scan_with(&options(fx.root()), &ScanControl::new(64));
    assert!(scanned.records.values().all(|r| r.rule.is_none()));
    assert_eq!(scanned.summary.kinds.total().0, 0);
}

#[test]
fn records_carry_the_directory_device_and_inode() {
    use std::os::unix::fs::MetadataExt;
    let fx = Fixture::new();
    fx.build(&["a/b/", "c/"]);
    let path = |p: &str| -> PathBuf { fx.path(p) };
    let scanned = scan_with(&options(fx.root()), &ScanControl::new(64));
    for dir in ["", "a", "a/b", "c"] {
        let meta = std::fs::metadata(path(dir)).unwrap();
        let record = scanned.at(&path(dir));
        assert_eq!(
            (record.device, record.inode),
            (meta.dev() as u64, meta.ino()),
            "{dir}"
        );
    }
}

#[test]
fn project_output_is_classified_from_the_facts_alone() {
    let rules = rules();
    let entries: Vec<Entry> = Vec::new();
    let path = Path::new("/r/target");
    let facts = DirFacts {
        entries: &entries,
        inside_git_repo: true,
        parent_markers: &[OsString::from("Cargo.toml")],
        ..facts(path, &[])
    };
    assert_eq!(
        rules.classify(&facts).cloned(),
        Some(RuleId::new(Category::BuildArtifact, "cargo-target"))
    );
    let outside = DirFacts {
        inside_git_repo: false,
        ..facts
    };
    assert_eq!(rules.classify(&outside), None);
}
