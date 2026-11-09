//! Developer CLI for metla-core: `scan`, `top` and `ls`.
//!
//! The index path is always given explicitly; the CLI never touches the
//! app's own index.

mod args;
mod commands;
mod format;
mod report;

use std::ffi::OsString;
use std::io::Write;

pub use args::{Command, TOP_LIMIT, USAGE, parse};
pub use commands::default_threads;
pub use format::{bytes, count, per_second, signed_bytes};
pub use report::{
    Listed, ScanReport, VolumeUse, json_string, ls_text, scan_json, scan_text, top_text,
};

/// Runs one command; returns the process exit code (2: usage, 1: failure).
pub fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let command = match parse(args) {
        Ok(command) => command,
        Err(message) => {
            let _ = writeln!(err, "{message}\n{USAGE}");
            return 2;
        }
    };
    match execute(command) {
        Ok(text) => {
            let _ = write!(out, "{text}");
            0
        }
        Err(message) => {
            let _ = writeln!(err, "error: {message}");
            1
        }
    }
}

fn execute(command: Command) -> Result<String, String> {
    match command {
        Command::Scan {
            root,
            index,
            threads,
            json,
        } => {
            let report = commands::scan(&root, &index, threads)?;
            Ok(match json {
                true => format!("{}\n", scan_json(&report)),
                false => scan_text(&report),
            })
        }
        Command::Top {
            index,
            under,
            limit,
        } => Ok(top_text(&commands::top(&index, under.as_deref(), limit)?)),
        Command::Ls { index, path } => Ok(ls_text(&commands::ls(&index, &path)?)),
    }
}
