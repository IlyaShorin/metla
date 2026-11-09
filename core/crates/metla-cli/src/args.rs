//! Command-line parsing: outside data is checked here, once.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// The default number of rows `top` prints.
pub const TOP_LIMIT: usize = 20;

pub const USAGE: &str = "usage:
  metla-cli scan --root <dir> --index <file> [--threads N] [--json]
  metla-cli top  --index <file> [--under <path>] [--limit N]
  metla-cli ls   --index <file> <path>";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Scan {
        root: PathBuf,
        index: PathBuf,
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
}

const VALUE_FLAGS: [&str; 5] = ["--root", "--index", "--threads", "--under", "--limit"];

/// Flags with values, the `--json` switch and positional arguments.
#[derive(Default)]
struct Parsed {
    values: BTreeMap<&'static str, OsString>,
    json: bool,
    positional: Vec<OsString>,
}

impl Parsed {
    fn take(&mut self, flag: &str) -> Option<OsString> {
        self.values.remove(flag)
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

    /// Fails on anything the command did not use.
    fn done(self) -> Result<(), String> {
        let unused = self.values.keys().next().map(|flag| flag.to_string());
        let unused = unused.or_else(|| self.json.then(|| "--json".to_owned()));
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

fn command(name: &OsStr, parsed: &mut Parsed) -> Result<Command, String> {
    match name.to_str() {
        Some("scan") => scan(parsed),
        Some("top") => top(parsed),
        Some("ls") => ls(parsed),
        _ => Err(format!("unknown command {}", name.to_string_lossy())),
    }
}

fn split(args: &[OsString]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match flag_name(arg) {
            Some("--json") => parsed.json = true,
            Some(flag) => {
                let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
                parsed.values.insert(flag, value.clone());
            }
            None => parsed.positional.push(arg.clone()),
        }
    }
    Ok(parsed)
}

/// The known flag `arg` names, or `None` for a positional argument.
fn flag_name(arg: &OsStr) -> Option<&'static str> {
    if arg == "--json" {
        return Some("--json");
    }
    VALUE_FLAGS.into_iter().find(|flag| arg == *flag)
}

fn scan(parsed: &mut Parsed) -> Result<Command, String> {
    let json = std::mem::take(&mut parsed.json);
    Ok(Command::Scan {
        root: parsed.path("--root")?,
        index: parsed.path("--index")?,
        threads: parsed.number("--threads")?,
        json,
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
