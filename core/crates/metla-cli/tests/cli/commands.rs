//! End to end: the same entry point `main` calls, on a synthetic tree.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    index: PathBuf,
}

impl Fixture {
    /// `tree/` with `big/` (2 files and `deeper/` with 1), `small/` (1
    /// file), `empty/` and a top-level file: 5 directories, 5 files. The
    /// index lives beside the tree.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let root = base.join("tree");
        for (path, len) in [
            ("big/one", 100_000),
            ("big/two", 50_000),
            ("big/deeper/three", 30_000),
            ("small/four", 5_000),
            ("top", 9_000),
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            File::create(path)
                .unwrap()
                .write_all(&vec![1; len])
                .unwrap();
        }
        fs::create_dir(root.join("empty")).unwrap();
        Fixture {
            _dir: dir,
            index: base.join("index.sqlite"),
            root,
        }
    }

    fn allocated(&self, relative: &str) -> u64 {
        fs::metadata(self.root.join(relative)).unwrap().blocks() * 512
    }
}

/// Runs the CLI; returns exit code, stdout and stderr.
fn cli(args: &[&OsString]) -> (i32, String, String) {
    let args: Vec<OsString> = args.iter().map(|a| (*a).clone()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = metla_cli::run(&args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn os(text: &str) -> OsString {
    OsString::from(text)
}

fn p(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

fn scan(fx: &Fixture) -> String {
    let (code, out, err) = cli(&[
        &os("scan"),
        &os("--root"),
        &p(&fx.root),
        &os("--index"),
        &p(&fx.index),
        &os("--threads"),
        &os("2"),
    ]);
    assert_eq!((code, err.as_str()), (0, ""));
    out
}

fn value<'a>(report: &'a str, label: &str) -> &'a str {
    report
        .lines()
        .find_map(|l| l.strip_prefix(label))
        .unwrap_or_else(|| panic!("no {label} in {report}"))
        .trim()
}

#[test]
fn scan_reports_the_tree_and_writes_the_index() {
    let fx = Fixture::new();
    let report = scan(&fx);
    assert_eq!(value(&report, "root"), fx.root.to_str().unwrap());
    assert_eq!(value(&report, "directories"), "5");
    assert_eq!(value(&report, "files"), "5");
    assert_eq!(value(&report, "coverage"), "complete");
    assert_eq!(value(&report, "threads"), "2");
    assert!(!report.contains("\nvolume "));
    assert!(fx.index.exists());
    let index_size = fs::metadata(&fx.index).unwrap().len();
    assert!(value(&report, "index").ends_with("kB"), "{index_size}");
}

#[test]
fn scan_as_json_has_the_numbers() {
    let fx = Fixture::new();
    let (code, out, _) = cli(&[
        &os("scan"),
        &os("--root"),
        &p(&fx.root),
        &os("--index"),
        &p(&fx.index),
        &os("--json"),
    ]);
    assert_eq!(code, 0);
    assert!(out.starts_with(&format!("{{\"root\":\"{}\",", fx.root.display())));
    assert!(out.contains("\"directories\":5,\"files\":5,"));
    assert!(out.contains("\"complete\":true"));
    assert!(out.ends_with("\"volume\":null}\n"));
}

#[test]
fn top_lists_the_largest_directories_from_the_index() {
    let fx = Fixture::new();
    scan(&fx);
    let (code, out, _) = cli(&[&os("top"), &os("--index"), &p(&fx.index)]);
    assert_eq!(code, 0);
    let paths: Vec<&str> = out.lines().map(|l| l.split("  ").last().unwrap()).collect();
    let expected: Vec<PathBuf> = ["big", "big/deeper", "small", "empty"]
        .iter()
        .map(|d| fx.root.join(d))
        .collect();
    assert_eq!(
        paths,
        expected
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect::<Vec<_>>()
    );
    let (_, under_big, _) = cli(&[
        &os("top"),
        &os("--index"),
        &p(&fx.index),
        &os("--under"),
        &p(&fx.root.join("big")),
        &os("--limit"),
        &os("1"),
    ]);
    assert_eq!(under_big.lines().count(), 1);
    assert!(under_big.trim_end().ends_with("big/deeper"));
}

#[test]
fn ls_shows_subdirectories_from_the_index_and_files_live() {
    let fx = Fixture::new();
    scan(&fx);
    File::create(fx.root.join("big/new"))
        .unwrap()
        .write_all(&[1; 20_000])
        .unwrap();
    let (code, out, _) = cli(&[
        &os("ls"),
        &os("--index"),
        &p(&fx.index),
        &p(&fx.root.join("big")),
    ]);
    assert_eq!(code, 0);
    let names: Vec<&str> = out
        .lines()
        .map(|l| l.split_whitespace().nth(3).unwrap())
        .collect();
    assert_eq!(names, ["one", "two", "deeper/", "new"]);
    let size = metla_cli::bytes(metla_core::domain::Bytes(fx.allocated("big/one")));
    assert_eq!(
        out.lines().next().unwrap(),
        format!("{size:>10}  file  one")
    );
}

#[test]
fn errors_exit_with_one_and_explain() {
    let fx = Fixture::new();
    let missing = fx.root.join("missing");
    let (code, _, err) = cli(&[&os("top"), &os("--index"), &p(&missing)]);
    assert_eq!(code, 1);
    assert!(err.starts_with("error: "));
    let (code, _, err) = cli(&[
        &os("scan"),
        &os("--root"),
        &p(&missing),
        &os("--index"),
        &p(&fx.index),
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("missing"));
    scan(&fx);
    let (code, _, err) = cli(&[&os("ls"), &os("--index"), &p(&fx.index), &p(&missing)]);
    assert_eq!(code, 1);
    assert!(err.contains("is not in the index"));
    let (code, _, err) = cli(&[
        &os("top"),
        &os("--index"),
        &p(&fx.index),
        &os("--under"),
        &p(&missing),
    ]);
    assert_eq!((code, err.as_str()), (1, "error: not in the index\n"));
}

#[test]
fn ls_of_a_directory_that_cannot_be_listed_fails() {
    let fx = Fixture::new();
    scan(&fx);
    let small = fx.root.join("small");
    fs::set_permissions(&small, fs::Permissions::from_mode(0o000)).unwrap();
    let (code, _, err) = cli(&[&os("ls"), &os("--index"), &p(&fx.index), &p(&small)]);
    fs::set_permissions(&small, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code, 1);
    assert!(err.contains("cannot list"), "{err}");
}

#[test]
fn the_binary_runs_the_same_entry_point() {
    let fx = Fixture::new();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_metla-cli"))
        .args([
            os("scan"),
            os("--root"),
            p(&fx.root),
            os("--index"),
            p(&fx.index),
        ])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("directories  5"));
    let usage = std::process::Command::new(env!("CARGO_BIN_EXE_metla-cli"))
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
}
