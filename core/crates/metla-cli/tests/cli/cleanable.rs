//! End to end: scan a synthetic home and repository with the rules, then
//! ask what can be cleaned, keep something, and ask again.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    _dir: tempfile::TempDir,
    tree: PathBuf,
    index: PathBuf,
}

impl Fixture {
    /// `tree/home` with an app cache (100 kB) and an npm cache (50 kB),
    /// `tree/repo` with Cargo build output (200 kB, just written).
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let tree = base.join("tree");
        for (path, len) in [
            ("home/Library/Caches/com.example.app/blob", 100_000),
            ("home/.npm/_cacache/x", 50_000),
            ("repo/target/out", 200_000),
            ("repo/Cargo.toml", 10),
        ] {
            let path = tree.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![1; len]).unwrap();
        }
        fs::create_dir(tree.join("repo/.git")).unwrap();
        Fixture {
            _dir: dir,
            index: base.join("index.sqlite"),
            tree,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.tree.join(relative)
    }

    fn scan(&self, extra: &[&str]) {
        let mut args = vec![
            "scan".into(),
            "--root".into(),
            p(&self.tree),
            "--index".into(),
            p(&self.index),
        ];
        args.extend(extra.iter().map(OsString::from));
        let (code, _, err) = cli(&args);
        assert_eq!((code, err.as_str()), (0, ""));
    }

    fn ask(&self, command: &str, extra: &[&str]) -> String {
        let mut args = vec![command.into(), "--index".into(), p(&self.index)];
        args.extend(extra.iter().map(OsString::from));
        let (code, out, err) = cli(&args);
        assert_eq!((code, err.as_str()), (0, ""), "{command} {extra:?}");
        out
    }
}

fn p(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

fn cli(args: &[OsString]) -> (i32, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = metla_cli::run(args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

/// The rule and the path of every candidate line.
fn listed(out: &str) -> Vec<(String, String)> {
    out.lines()
        .map(|line| {
            let mut fields = line.rsplitn(2, "  ").map(str::trim);
            let path = fields.next().unwrap().to_owned();
            let rule = fields
                .next()
                .unwrap()
                .rsplit("  ")
                .next()
                .unwrap()
                .trim()
                .to_owned();
            (rule, path)
        })
        .collect()
}

fn home_flag(fx: &Fixture) -> String {
    fx.path("home").to_string_lossy().into_owned()
}

#[test]
fn candidates_are_listed_largest_first_by_rule_and_path() {
    let fx = Fixture::new();
    fx.scan(&["--home", &home_flag(&fx)]);
    let out = fx.ask("candidates", &[]);
    let expected = [
        ("build.cargo-target", fx.path("repo/target")),
        (
            "app-cache.library",
            fx.path("home/Library/Caches/com.example.app"),
        ),
        ("dev-cache.npm", fx.path("home/.npm/_cacache")),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(rule, path)| (rule.to_string(), path.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(listed(&out), expected);
    let first = out.lines().next().unwrap();
    assert!(
        first.contains("  [ ]  "),
        "fresh build output is not preselected: {first}"
    );
    assert!(out.lines().nth(1).unwrap().contains("  [x]  "));
    let only = fx.ask("candidates", &["--category", "dev-cache"]);
    assert_eq!(listed(&only).len(), 1);
    assert_eq!(fx.ask("candidates", &["--limit", "1"]).lines().count(), 1);
    let json = fx.ask("candidates", &["--json"]);
    assert!(json.starts_with("[{\"id\":"));
    assert!(json.contains("\"rule\":\"dev-cache.npm\",\"category\":\"dev-cache\""));
}

#[test]
fn the_summary_totals_categories_rules_and_kinds() {
    let fx = Fixture::new();
    fx.scan(&["--home", &home_flag(&fx)]);
    let out = fx.ask("summary", &[]);
    assert!(out.contains("in 3 candidates (estimate)\n"), "{out}");
    assert!(out.contains("\n    build.cargo-target "));
    assert!(out.contains("\n    dev-cache.npm "));
    assert!(out.contains("\ndata types\n"));
    assert!(out.contains("coverage     complete\n"));
    let json = fx.ask("summary", &["--json"]);
    assert!(json.contains("\"total\":{\"count\":3,"));
    assert!(json.contains("\"current\":true,\"coverage\":\"complete\""));
}

#[test]
fn an_excluded_path_stops_being_a_candidate_without_a_rescan() {
    let fx = Fixture::new();
    fx.scan(&["--home", &home_flag(&fx)]);
    let repo = fx.path("repo").to_string_lossy().into_owned();
    let added = fx.ask("exclude", &["add", &repo]);
    assert_eq!(added, format!("excluded {repo}\n"));
    assert_eq!(
        fx.ask("exclude", &["add", &repo]),
        format!("already excluded {repo}\n")
    );
    let rules: Vec<String> = listed(&fx.ask("candidates", &[]))
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    assert_eq!(rules, ["app-cache.library", "dev-cache.npm"]);
    assert_eq!(fx.ask("exclude", &["list"]), format!("{repo}\n"));
    let removed = fx.ask("exclude", &["remove", &repo]);
    assert_eq!(removed, format!("no longer excluded {repo}\n"));
    assert_eq!(
        fx.ask("exclude", &["remove", &repo]),
        format!("was not excluded {repo}\n")
    );
    assert_eq!(listed(&fx.ask("candidates", &[])).len(), 3);
}

#[test]
fn an_exclusion_must_be_an_absolute_path() {
    let fx = Fixture::new();
    let args: Vec<OsString> = ["exclude", "add", "--index"]
        .into_iter()
        .map(OsString::from)
        .chain([p(&fx.index), "no/such/relative".into()])
        .collect();
    let (code, _, err) = cli(&args);
    assert_eq!(code, 1);
    assert_eq!(err, "error: no/such/relative is not an absolute path\n");
}

#[test]
fn a_scan_without_rules_finds_no_candidates() {
    let fx = Fixture::new();
    fx.scan(&["--home", &home_flag(&fx), "--no-rules"]);
    assert_eq!(fx.ask("candidates", &[]), "");
}

#[test]
fn a_missing_home_directory_is_an_error() {
    let fx = Fixture::new();
    let missing = fx.path("nobody").to_string_lossy().into_owned();
    let args: Vec<OsString> = ["scan", "--home", &missing, "--root"]
        .into_iter()
        .map(OsString::from)
        .chain([p(&fx.tree), "--index".into(), p(&fx.index)])
        .collect();
    let (code, _, err) = cli(&args);
    assert_eq!(code, 1);
    assert!(err.starts_with(&format!("error: {missing}: ")), "{err}");
}

#[test]
fn the_binary_takes_the_home_directory_from_the_environment() {
    let fx = Fixture::new();
    let bin = env!("CARGO_BIN_EXE_metla-cli");
    let scan = |home: Option<&Path>| {
        let mut command = std::process::Command::new(bin);
        command.args([
            OsString::from("scan"),
            "--root".into(),
            p(&fx.tree),
            "--index".into(),
            p(&fx.index),
        ]);
        command.env_remove("HOME").env("USER", "tester");
        if let Some(home) = home {
            command.env("HOME", home);
        }
        command.output().unwrap()
    };
    let homeless = scan(None);
    assert_eq!(homeless.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&homeless.stderr),
        "error: no home directory: pass --home\n"
    );
    assert!(scan(Some(&fx.path("home"))).status.success());
    assert_eq!(listed(&fx.ask("candidates", &[])).len(), 3);
}
