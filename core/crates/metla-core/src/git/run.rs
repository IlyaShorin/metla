//! Running the stock `git` binary: no pager, no prompts, no optional
//! locks, the C locale, a time limit, and everything it prints captured.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// How the `git` binary is run. The binary is given, so tests can put a
/// failing one in its place.
#[derive(Clone, Debug)]
pub struct Git {
    program: PathBuf,
    env: Vec<(OsString, OsString)>,
    timeout: Duration,
}

/// What Git printed and how it exited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Git could not be run to the end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// The binary could not be started.
    Spawn(String),
    /// It ran longer than the time limit and was killed.
    TimedOut,
    /// It was ended by a signal.
    Killed,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Spawn(why) => write!(f, "git could not start: {why}"),
            RunError::TimedOut => write!(f, "git timed out"),
            RunError::Killed => write!(f, "git was killed"),
        }
    }
}

/// Environment every run gets: never wait for a terminal, never page,
/// never take a lock that only speeds Git up, and speak plain English.
const QUIET: [(&str, &str); 5] = [
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    ("LC_ALL", "C"),
];

/// How often a running Git is checked on.
const POLL: Duration = Duration::from_millis(2);

impl Git {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

    /// The `git` on the search path, called directly: on macOS
    /// `/usr/bin/git` is a shim that looks the developer tools up on every
    /// call, which costs more than most commands. Falls back to `git`.
    pub fn system() -> Git {
        let exec = Git::new("git").run(Path::new("/"), &[OsStr::new("--exec-path")], &[]);
        let dir = exec.ok().filter(|output| output.code == 0).map(|output| {
            let dir = output.stdout.trim_ascii_end().to_vec();
            PathBuf::from(OsString::from_vec(dir))
        });
        let real = dir.map(|dir| dir.join("git")).filter(|git| git.is_file());
        Git::new(real.unwrap_or_else(|| PathBuf::from("git")))
    }

    /// The binary this runs.
    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn new(program: impl Into<PathBuf>) -> Git {
        Git {
            program: program.into(),
            env: Vec::new(),
            timeout: Git::DEFAULT_TIMEOUT,
        }
    }

    /// Adds an environment variable to every run.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Git {
        let pair = (key.as_ref().to_owned(), value.as_ref().to_owned());
        self.env.push(pair);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Git {
        self.timeout = timeout;
        self
    }

    /// Runs `git -C <dir> <args>` with `input` on its standard input.
    /// Everything, the output included, must arrive within the time limit:
    /// a process Git started cannot keep the call waiting by holding the
    /// pipes open.
    pub fn run(&self, dir: &Path, args: &[&OsStr], input: &[u8]) -> Result<Output, RunError> {
        let deadline = Instant::now() + self.timeout;
        let mut command = Command::new(&self.program);
        command.arg("--no-pager").arg("-C").arg(dir).args(args);
        command
            .envs(QUIET)
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| RunError::Spawn(e.to_string()))?;
        feed(&mut child, input.to_vec());
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let code = wait(&mut child, deadline)?.code().ok_or(RunError::Killed)?;
        Ok(Output {
            code,
            stdout: collect(&stdout, deadline)?,
            stderr: collect(&stderr, deadline)?,
        })
    }
}

/// Writes `input` to the child's standard input on its own thread, so a
/// child that answers while it reads cannot block on a full pipe.
fn feed(child: &mut Child, input: Vec<u8>) {
    let stdin = child.stdin.take();
    thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(&input);
        }
    });
}

/// Reads a pipe to its end on its own thread.
fn drain(pipe: Option<impl Read + Send + 'static>) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(bytes);
    });
    receiver
}

fn collect(pipe: &Receiver<Vec<u8>>, deadline: Instant) -> Result<Vec<u8>, RunError> {
    let left = deadline.saturating_duration_since(Instant::now());
    pipe.recv_timeout(left).map_err(|_| RunError::TimedOut)
}

/// Waits for the child; kills it when it outlives the deadline.
fn wait(child: &mut Child, deadline: Instant) -> Result<ExitStatus, RunError> {
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| RunError::Spawn(e.to_string()))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::TimedOut);
        }
        thread::sleep(POLL);
    }
}
