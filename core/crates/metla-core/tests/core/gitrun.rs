//! How the `git` binary is run: in the given directory, quietly, with a
//! time limit, and with everything it prints captured.

use std::ffi::OsStr;
use std::time::{Duration, Instant};

use metla_core::git::{Git, Output, RunError};

use crate::repo::script;
use crate::support::Fixture;

fn run(git: &Git, dir: &std::path::Path, args: &[&str], input: &[u8]) -> Result<Output, RunError> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git.run(dir, &args, input)
}

#[test]
fn the_system_git_is_called_directly() {
    let git = Git::system();
    assert_eq!(git.program().file_name(), Some(OsStr::new("git")));
    assert!(git.program().is_absolute(), "{}", git.program().display());
    let output = run(&git, std::path::Path::new("/"), &["--version"], b"").unwrap();
    assert_eq!(output.code, 0);
    assert!(output.stdout.starts_with(b"git version "));
}

#[test]
fn git_runs_in_the_directory_quietly_with_its_input_and_extra_environment() {
    let fx = Fixture::new();
    let body = "echo \"args: $*\"\n\
                echo \"locks=$GIT_OPTIONAL_LOCKS prompt=$GIT_TERMINAL_PROMPT pager=$GIT_PAGER/$PAGER lang=$LC_ALL extra=$EXTRA\"\n\
                cat\n\
                echo oops >&2\n\
                exit 3";
    let fake = Git::new(script(&fx.path("fake-git"), body)).env("EXTRA", "yes");
    let output = run(&fake, fx.root(), &["status", "-z"], b"from stdin").unwrap();
    let printed = String::from_utf8(output.stdout).unwrap();
    let expected = format!(
        "args: --no-pager -C {} status -z\n\
         locks=0 prompt=0 pager=cat/cat lang=C extra=yes\n\
         from stdin",
        fx.root().display()
    );
    assert_eq!(printed, expected);
    assert_eq!(output.stderr, b"oops\n");
    assert_eq!(output.code, 3);
}

#[test]
fn a_large_input_and_output_do_not_block_each_other() {
    let fx = Fixture::new();
    let echo = Git::new(script(&fx.path("cat-git"), "cat"));
    let input = vec![b'x'; 4 << 20];
    let output = run(&echo, fx.root(), &[], &input).unwrap();
    assert_eq!(output.stdout.len(), input.len());
}

#[test]
fn git_that_runs_too_long_is_killed() {
    let fx = Fixture::new();
    let slow =
        Git::new(script(&fx.path("slow-git"), "exec sleep 30")).timeout(Duration::from_millis(200));
    let started = Instant::now();
    assert_eq!(run(&slow, fx.root(), &[], b""), Err(RunError::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(Git::DEFAULT_TIMEOUT, Duration::from_secs(60));
}

#[test]
fn a_process_git_leaves_behind_cannot_hold_the_call_past_its_time_limit() {
    let fx = Fixture::new();
    let lingering = Git::new(script(&fx.path("lingering-git"), "sleep 30 &\necho done"))
        .timeout(Duration::from_millis(300));
    let started = Instant::now();
    assert_eq!(
        run(&lingering, fx.root(), &[], b""),
        Err(RunError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn git_ended_by_a_signal_or_never_started_is_an_error() {
    let fx = Fixture::new();
    let killed = Git::new(script(&fx.path("killed-git"), "kill -9 $$"));
    assert_eq!(run(&killed, fx.root(), &[], b""), Err(RunError::Killed));
    let missing = Git::new(fx.path("no-such-git"));
    let error = run(&missing, fx.root(), &[], b"").unwrap_err();
    assert!(matches!(error, RunError::Spawn(_)));
    assert!(error.to_string().starts_with("git could not start: "));
    assert_eq!(RunError::TimedOut.to_string(), "git timed out");
    assert_eq!(RunError::Killed.to_string(), "git was killed");
}
