//! Command-line parsing: outside data is checked here, once.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use metla_core::domain::Category;

/// The default number of rows `top` prints.
pub const TOP_LIMIT: usize = 20;

/// The default number of rows `candidates` prints.
pub const CANDIDATE_LIMIT: usize = 50;

pub const USAGE: &str = "usage:
  metla-cli scan       --root <dir> --index <file> [--home <dir>] [--no-rules] [--threads N] [--json]
  metla-cli top        --index <file> [--under <path>] [--limit N]
  metla-cli ls         --index <file> <path>
  metla-cli candidates --index <file> [--category <c>] [--limit N] [--json]
  metla-cli summary    --index <file> [--json]
  metla-cli exclude    add|remove|list --index <file> [<path>]
categories: app-cache, dev-cache, build, log, worktree, temp";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Scan {
        root: PathBuf,
        index: PathBuf,
        /// `None`: the current user's home directory.
        home: Option<PathBuf>,
        rules: bool,
        threads: Option<usize>,
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
}

/// What `exclude` does.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    Add(PathBuf),
    Remove(PathBuf),
    List,
}

const VALUE_FLAGS: [&str; 7] = [
    "--root",
    "--index",
    "--threads",
    "--under",
    "--limit",
    "--home",
    "--category",
];
const SWITCHES: [&str; 2] = ["--json", "--no-rules"];

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

const COMMANDS: [(&str, Parser); 6] = [
    ("scan", scan),
    ("top", top),
    ("ls", ls),
    ("candidates", candidates),
    ("summary", summary),
    ("exclude", exclude),
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
