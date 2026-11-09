use std::ffi::OsString;
use std::path::PathBuf;

use metla_cli::{Command, TOP_LIMIT, parse};

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
            threads: Some(3),
            json: true,
        })
    );
    assert_eq!(
        parse(&args("scan --index /i --root /r")),
        Ok(Command::Scan {
            root: PathBuf::from("/r"),
            index: PathBuf::from("/i"),
            threads: None,
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
}

#[test]
fn a_usage_error_exits_with_two_and_explains_on_stderr() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(metla_cli::run(&args("dance"), &mut out, &mut err), 2);
    let err = String::from_utf8(err).unwrap();
    assert!(err.starts_with("unknown command dance\nusage:"));
    assert!(out.is_empty());
}
