use std::ffi::OsString;
use std::path::PathBuf;

use metla_cli::{CANDIDATE_LIMIT, Command, Edit, TOP_LIMIT, parse};
use metla_core::domain::Category;

fn args(text: &str) -> Vec<OsString> {
    text.split_whitespace().map(OsString::from).collect()
}

#[test]
fn scan_takes_root_index_threads_and_json() {
    assert_eq!(
        parse(&args("scan --root /r --index /i.sqlite --threads 3 --json")),
        Ok(Command::Scan {
            root: PathBuf::from("/r"),
            index: PathBuf::from("/i.sqlite"),
            home: None,
            rules: true,
            threads: Some(3),
            classic: false,
            json: true,
        })
    );
    assert_eq!(
        parse(&args(
            "scan --index /i --root /r --home /h --no-rules --classic"
        )),
        Ok(Command::Scan {
            root: PathBuf::from("/r"),
            index: PathBuf::from("/i"),
            home: Some(PathBuf::from("/h")),
            rules: false,
            threads: None,
            classic: true,
            json: false,
        })
    );
}

#[test]
fn top_has_a_default_limit_and_an_optional_anchor() {
    assert_eq!(
        parse(&args("top --index /i")),
        Ok(Command::Top {
            index: PathBuf::from("/i"),
            under: None,
            limit: TOP_LIMIT,
        })
    );
    assert_eq!(
        parse(&args("top --index /i --under /r/a --limit 5")),
        Ok(Command::Top {
            index: PathBuf::from("/i"),
            under: Some(PathBuf::from("/r/a")),
            limit: 5,
        })
    );
}

#[test]
fn ls_takes_one_path() {
    assert_eq!(
        parse(&args("ls --index /i /r/a")),
        Ok(Command::Ls {
            index: PathBuf::from("/i"),
            path: PathBuf::from("/r/a"),
        })
    );
    assert!(parse(&args("ls --index /i")).is_err());
    assert!(parse(&args("ls --index /i /a /b")).is_err());
}

#[test]
fn candidates_take_a_category_a_limit_and_json() {
    assert_eq!(
        parse(&args(
            "candidates --index /i --category build --limit 5 --json"
        )),
        Ok(Command::Candidates {
            index: PathBuf::from("/i"),
            category: Some(Category::BuildArtifact),
            limit: 5,
            json: true,
        })
    );
    assert_eq!(
        parse(&args("candidates --index /i")),
        Ok(Command::Candidates {
            index: PathBuf::from("/i"),
            category: None,
            limit: CANDIDATE_LIMIT,
            json: false,
        })
    );
}

#[test]
fn summary_takes_an_index_and_json() {
    assert_eq!(
        parse(&args("summary --index /i --json")),
        Ok(Command::Summary {
            index: PathBuf::from("/i"),
            json: true,
        })
    );
}

#[test]
fn exclude_adds_removes_and_lists() {
    let exclude = |text| match parse(&args(text)) {
        Ok(Command::Exclude { index, edit }) => {
            assert_eq!(index, PathBuf::from("/i"));
            edit
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        exclude("exclude add --index /i /a/b"),
        Edit::Add("/a/b".into())
    );
    assert_eq!(
        exclude("exclude --index /i remove /a"),
        Edit::Remove("/a".into())
    );
    assert_eq!(exclude("exclude list --index /i"), Edit::List);
}

#[test]
fn fixture_create_takes_a_directory() {
    assert_eq!(
        parse(&args("fixture create /tmp/f")),
        Ok(Command::Fixture {
            dir: PathBuf::from("/tmp/f")
        })
    );
    let error = |text| parse(&args(text)).unwrap_err();
    assert_eq!(error("fixture"), "fixture needs create and a directory");
    assert_eq!(
        error("fixture create"),
        "fixture needs create and a directory"
    );
    assert_eq!(
        error("fixture make /f"),
        "fixture needs create and a directory"
    );
    assert_eq!(error("fixture create /f /g"), "unexpected argument /g");
}

#[test]
fn clean_takes_index_root_category_and_yes() {
    assert_eq!(
        parse(&args("clean --index /i --root /f")),
        Ok(Command::Clean {
            index: PathBuf::from("/i"),
            root: PathBuf::from("/f"),
            category: None,
            yes: false,
        })
    );
    assert_eq!(
        parse(&args("clean --yes --root /f --category log --index /i")),
        Ok(Command::Clean {
            index: PathBuf::from("/i"),
            root: PathBuf::from("/f"),
            category: Some(Category::Log),
            yes: true,
        })
    );
    let error = |text| parse(&args(text)).unwrap_err();
    assert_eq!(error("clean --index /i"), "--root is required");
    assert_eq!(error("clean --root /f"), "--index is required");
    assert_eq!(
        error("scan --root /r --index /i --yes"),
        "unexpected argument --yes"
    );
}

#[test]
fn mistakes_are_reported() {
    let error = |text| parse(&args(text)).unwrap_err();
    assert_eq!(error(""), "missing command");
    assert_eq!(error("dance"), "unknown command dance");
    assert_eq!(error("scan --root /r"), "--index is required");
    assert_eq!(error("scan --index /i"), "--root is required");
    assert_eq!(error("top"), "--index is required");
    assert_eq!(error("ls /x"), "--index is required");
    assert_eq!(error("scan --root"), "--root needs a value");
    assert_eq!(
        error("scan --root /r --index /i --threads many"),
        "--threads needs a whole number"
    );
    assert_eq!(error("top --index /i --json"), "unexpected argument --json");
    assert_eq!(
        error("top --index /i --root /r"),
        "unexpected argument --root"
    );
    assert_eq!(error("top --index /i extra"), "unexpected argument extra");
    assert_eq!(
        error("scan --root /r --index /i --bogus"),
        "unexpected argument --bogus"
    );
    assert_eq!(
        error("candidates --index /i --category cache"),
        "unknown category cache"
    );
    assert_eq!(
        error("summary --index /i --no-rules"),
        "unexpected argument --no-rules"
    );
    assert_eq!(
        error("exclude --index /i"),
        "exclude needs add, remove or list"
    );
    assert_eq!(
        error("exclude drop --index /i /a"),
        "exclude needs add, remove or list"
    );
    assert_eq!(
        error("exclude add --index /i"),
        "exclude add and remove need a path"
    );
    assert_eq!(
        error("exclude remove --index /i"),
        "exclude add and remove need a path"
    );
    assert_eq!(
        error("exclude list --index /i /a"),
        "unexpected argument /a"
    );
}

#[test]
fn a_usage_error_exits_with_two_and_explains_on_stderr() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(metla_cli::run(&args("dance"), &mut out, &mut err), 2);
    let err = String::from_utf8(err).unwrap();
    assert!(err.starts_with("unknown command dance\nusage:"));
    assert!(out.is_empty());
}
