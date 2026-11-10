//! Command-line parsing: outside data is checked here, once.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

use metla_core::domain::Category;
use metla_core::engine::PowerProfile;

use crate::live::Place;

/// The default number of rows `top` prints.
pub const TOP_LIMIT: usize = 20;

/// The default number of rows `candidates` prints.
pub const CANDIDATE_LIMIT: usize = 50;

pub const USAGE: &str = "usage:
  metla-cli scan       --root <dir> --index <file> [--home <dir>] [--no-rules] [--threads N] [--classic] [--json]
  metla-cli top        --index <file> [--under <path>] [--limit N]
  metla-cli ls         --index <file> <path>
  metla-cli candidates --index <file> [--category <c>] [--limit N] [--json]
  metla-cli summary    --index <file> [--json]
  metla-cli exclude    add|remove|list --index <file> [<path>]
  metla-cli fixture    create <dir>
  metla-cli clean      --index <file> --root <fixture> [--category <c>] [--yes]
  metla-cli watch      --root <dir> --data-dir <dir> [engine options] [--seconds N]
  metla-cli bench      open|churn|refresh --root <dir> --data-dir <dir> [engine options]
                       [--seconds N] [--files N]
engine options: [--home <dir>] [--profile p] [--latency-ms N] [--settle-ms N]
categories: app-cache, dev-cache, build, log, worktree, temp
profiles: interactive, background, saver, off
clean works only on a root marked .metla-fixture; without --yes it only previews;
bench churn and refresh write only into a root marked .metla-fixture";

/// The FSEvents latency `watch` and `bench` use unless told otherwise.
pub const LATENCY: Duration = Duration::from_secs(1);
/// How long `bench churn` churns, and how many files per burst.
pub const CHURN: Duration = Duration::from_secs(120);
pub const BURST: usize = 400;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Scan {
        root: PathBuf,
        index: PathBuf,
        /// `None`: the current user's home directory.
        home: Option<PathBuf>,
        rules: bool,
        threads: Option<usize>,
        /// List with `readdir` and `fstatat` instead of `getattrlistbulk`.
        classic: bool,
        json: bool,
    },
    Top {
        index: PathBuf,
        under: Option<PathBuf>,
        limit: usize,
    },
    Ls {
        index: PathBuf,
        path: PathBuf,
    },
    Candidates {
        index: PathBuf,
        category: Option<Category>,
        limit: usize,
        json: bool,
    },
    Summary {
        index: PathBuf,
        json: bool,
    },
    Exclude {
        index: PathBuf,
        edit: Edit,
    },
    /// Builds a synthetic fixture in a new or empty directory.
    Fixture {
        dir: PathBuf,
    },
    /// Previews, and with `yes` executes, a cleanup of a fixture.
    Clean {
        index: PathBuf,
        root: PathBuf,
        /// `None`: the default selection (preselected candidates).
        category: Option<Category>,
        yes: bool,
    },
    /// Runs the engine on a root and prints every batch it applies.
    Watch {
        place: Place,
        /// `None`: until the process is stopped.
        limit: Option<Duration>,
    },
    Bench {
        place: Place,
        bench: Bench,
    },
}

/// What `bench` measures.
#[derive(Debug, PartialEq, Eq)]
pub enum Bench {
    /// Opening the engine on a saved picture and the first snapshot.
    Open,
    /// Following a churn of files, and how soon the picture is right.
    Churn { length: Duration, per_burst: usize },
    /// How soon a small change shows.
    Refresh,
}

/// What `exclude` does.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    Add(PathBuf),
    Remove(PathBuf),
    List,
}

const VALUE_FLAGS: [&str; 13] = [
    "--root",
    "--index",
    "--threads",
    "--under",
    "--limit",
    "--home",
    "--category",
    "--data-dir",
    "--seconds",
    "--latency-ms",
    "--profile",
    "--files",
    "--settle-ms",
];

const PROFILES: [(&str, PowerProfile); 4] = [
    ("interactive", PowerProfile::Interactive),
    ("background", PowerProfile::Background),
    ("saver", PowerProfile::Saver),
    ("off", PowerProfile::Off),
];
const SWITCHES: [&str; 4] = ["--json", "--no-rules", "--yes", "--classic"];

/// Flags with values, switches and positional arguments.
#[derive(Default)]
struct Parsed {
    values: BTreeMap<&'static str, OsString>,
    switches: BTreeSet<&'static str>,
    positional: Vec<OsString>,
}

impl Parsed {
    fn take(&mut self, flag: &str) -> Option<OsString> {
        self.values.remove(flag)
    }

    fn switch(&mut self, name: &str) -> bool {
        self.switches.remove(name)
    }

    fn path(&mut self, flag: &str) -> Result<PathBuf, String> {
        self.take(flag)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{flag} is required"))
    }

    fn number(&mut self, flag: &str) -> Result<Option<usize>, String> {
        self.take(flag)
            .map(|v| {
                v.to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("{flag} needs a whole number"))
            })
            .transpose()
    }

    fn category(&mut self) -> Result<Option<Category>, String> {
        self.take("--category")
            .map(|v| {
                v.to_str()
                    .and_then(Category::from_name)
                    .ok_or_else(|| format!("unknown category {}", v.to_string_lossy()))
            })
            .transpose()
    }

    fn profile(&mut self) -> Result<PowerProfile, String> {
        let Some(name) = self.take("--profile") else {
            return Ok(PowerProfile::Background);
        };
        let known = PROFILES.iter().find(|(known, _)| name == *known);
        let profile = known.map(|(_, profile)| *profile);
        profile.ok_or_else(|| format!("unknown profile {}", name.to_string_lossy()))
    }

    fn seconds(&mut self) -> Result<Option<Duration>, String> {
        Ok(self
            .number("--seconds")?
            .map(|s| Duration::from_secs(s as u64)))
    }

    fn millis(&mut self, flag: &str) -> Result<Option<Duration>, String> {
        Ok(self
            .number(flag)?
            .map(|ms| Duration::from_millis(ms as u64)))
    }

    fn place(&mut self) -> Result<Place, String> {
        Ok(Place {
            root: self.path("--root")?,
            data_dir: self.path("--data-dir")?,
            home: self.take("--home").map(PathBuf::from),
            latency: self.millis("--latency-ms")?.unwrap_or(LATENCY),
            settle: self.millis("--settle-ms")?,
            profile: self.profile()?,
        })
    }

    /// The next positional argument, if any.
    fn next(&mut self) -> Option<OsString> {
        (!self.positional.is_empty()).then(|| self.positional.remove(0))
    }

    /// Fails on anything the command did not use.
    fn done(self) -> Result<(), String> {
        let unused = self.values.keys().next().map(|flag| flag.to_string());
        let unused = unused.or_else(|| self.switches.first().map(|s| s.to_string()));
        let unused = unused.or_else(|| {
            self.positional
                .first()
                .map(|p| p.to_string_lossy().into_owned())
        });
        match unused {
            Some(arg) => Err(format!("unexpected argument {arg}")),
            None => Ok(()),
        }
    }
}

pub fn parse(args: &[OsString]) -> Result<Command, String> {
    let (name, rest) = args.split_first().ok_or("missing command")?;
    let mut parsed = split(rest)?;
    let command = command(name, &mut parsed)?;
    parsed.done()?;
    Ok(command)
}

type Parser = fn(&mut Parsed) -> Result<Command, String>;

const COMMANDS: [(&str, Parser); 10] = [
    ("scan", scan),
    ("top", top),
    ("ls", ls),
    ("candidates", candidates),
    ("summary", summary),
    ("exclude", exclude),
    ("fixture", fixture),
    ("clean", clean),
    ("watch", watch),
    ("bench", bench),
];

fn command(name: &OsStr, parsed: &mut Parsed) -> Result<Command, String> {
    let known = COMMANDS.iter().find(|(known, _)| name == *known);
    let (_, parser) = known.ok_or_else(|| format!("unknown command {}", name.to_string_lossy()))?;
    parser(parsed)
}

fn split(args: &[OsString]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let Some(switch) = SWITCHES.into_iter().find(|s| arg == *s) {
            parsed.switches.insert(switch);
        } else if let Some(flag) = VALUE_FLAGS.into_iter().find(|f| arg == *f) {
            let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
            parsed.values.insert(flag, value.clone());
        } else {
            parsed.positional.push(arg.clone());
        }
    }
    Ok(parsed)
}

fn scan(parsed: &mut Parsed) -> Result<Command, String> {
    Ok(Command::Scan {
        root: parsed.path("--root")?,
        index: parsed.path("--index")?,
        home: parsed.take("--home").map(PathBuf::from),
        rules: !parsed.switch("--no-rules"),
        threads: parsed.number("--threads")?,
        classic: parsed.switch("--classic"),
        json: parsed.switch("--json"),
    })
}

fn top(parsed: &mut Parsed) -> Result<Command, String> {
    Ok(Command::Top {
        index: parsed.path("--index")?,
        under: parsed.take("--under").map(PathBuf::from),
        limit: parsed.number("--limit")?.unwrap_or(TOP_LIMIT),
    })
}

fn ls(parsed: &mut Parsed) -> Result<Command, String> {
    let index = parsed.path("--index")?;
    let path = match parsed.positional.len() {
        1 => PathBuf::from(parsed.positional.remove(0)),
        _ => return Err("ls needs exactly one path".into()),
    };
    Ok(Command::Ls { index, path })
}

fn candidates(parsed: &mut Parsed) -> Result<Command, String> {
    Ok(Command::Candidates {
        index: parsed.path("--index")?,
        category: parsed.category()?,
        limit: parsed.number("--limit")?.unwrap_or(CANDIDATE_LIMIT),
        json: parsed.switch("--json"),
    })
}

fn summary(parsed: &mut Parsed) -> Result<Command, String> {
    Ok(Command::Summary {
        index: parsed.path("--index")?,
        json: parsed.switch("--json"),
    })
}

fn exclude(parsed: &mut Parsed) -> Result<Command, String> {
    let index = parsed.path("--index")?;
    let action = parsed.next();
    let edit = match action.as_ref().and_then(|a| a.to_str()) {
        Some("add") => Edit::Add(edited_path(parsed)?),
        Some("remove") => Edit::Remove(edited_path(parsed)?),
        Some("list") => Edit::List,
        _ => return Err("exclude needs add, remove or list".into()),
    };
    Ok(Command::Exclude { index, edit })
}

fn edited_path(parsed: &mut Parsed) -> Result<PathBuf, String> {
    parsed
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "exclude add and remove need a path".to_owned())
}

fn fixture(parsed: &mut Parsed) -> Result<Command, String> {
    let action = parsed.next();
    let dir = parsed.next();
    match (action.as_ref().and_then(|a| a.to_str()), dir) {
        (Some("create"), Some(dir)) => Ok(Command::Fixture {
            dir: PathBuf::from(dir),
        }),
        _ => Err("fixture needs create and a directory".into()),
    }
}

fn clean(parsed: &mut Parsed) -> Result<Command, String> {
    Ok(Command::Clean {
        index: parsed.path("--index")?,
        root: parsed.path("--root")?,
        category: parsed.category()?,
        yes: parsed.switch("--yes"),
    })
}

fn watch(parsed: &mut Parsed) -> Result<Command, String> {
    Ok(Command::Watch {
        place: parsed.place()?,
        limit: parsed.seconds()?,
    })
}

fn bench(parsed: &mut Parsed) -> Result<Command, String> {
    let what = parsed.next();
    let place = parsed.place()?;
    let bench = match what.as_ref().and_then(|w| w.to_str()) {
        Some("open") => Bench::Open,
        Some("churn") => Bench::Churn {
            length: parsed.seconds()?.unwrap_or(CHURN),
            per_burst: parsed.number("--files")?.unwrap_or(BURST),
        },
        Some("refresh") => Bench::Refresh,
        _ => return Err("bench needs open, churn or refresh".into()),
    };
    Ok(Command::Bench { place, bench })
}
