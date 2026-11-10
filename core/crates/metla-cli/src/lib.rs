//! Developer CLI for metla-core: `scan`, `top`, `ls`, `candidates`,
//! `summary`, `exclude`, and `fixture create` and `clean`, which removes
//! only inside a root marked `.metla-fixture`.
//!
//! The index path is always given explicitly; the CLI never touches the
//! app's own index.

mod args;
mod bench;
mod cleaned;
mod commands;
mod fixture;
mod format;
mod live;
mod overview;
mod report;

use std::ffi::OsString;
use std::io::Write;

pub use args::{
    BURST, Bench, CANDIDATE_LIMIT, CHURN, Command, Edit, LATENCY, TOP_LIMIT, USAGE, parse,
};
pub use cleaned::{Cleaned, clean_text, outcome_text};
pub use commands::{ScanHow, default_threads};
pub use format::{ago, bytes, count, per_second, signed_bytes};
pub use live::{Place, freshness_line, profiles, spent, state_text};
pub use overview::{
    Edited, Freshness, Overview, Shown, candidates_json, candidates_text, edited_text,
    summary_json, summary_text,
};
pub use report::{
    Checked, Listed, ScanReport, VolumeUse, json_string, ls_text, scan_json, scan_text, top_text,
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
    match execute(command, out) {
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

/// Text, or one line of JSON.
fn rendered<T>(data: &T, json: bool, text: fn(&T) -> String, as_json: fn(&T) -> String) -> String {
    match json {
        true => format!("{}\n", as_json(data)),
        false => text(data),
    }
}

fn execute(command: Command, out: &mut dyn Write) -> Result<String, String> {
    match command {
        Command::Scan {
            root,
            index,
            home,
            rules,
            threads,
            classic,
            json,
        } => {
            let how = ScanHow {
                home,
                rules,
                threads,
                classic,
            };
            let report = commands::scan(&root, &index, &how);
            report.map(|report| rendered(&report, json, scan_text, scan_json))
        }
        Command::Top {
            index,
            under,
            limit,
        } => commands::top(&index, under.as_deref(), limit).map(|rows| top_text(&rows)),
        Command::Ls { index, path } => commands::ls(&index, &path).map(|rows| ls_text(&rows)),
        Command::Candidates {
            index,
            category,
            limit,
            json,
        } => commands::candidates(&index, category, limit)
            .map(|shown| rendered(&shown, json, candidates_text, candidates_json)),
        Command::Summary { index, json } => commands::summary(&index)
            .map(|shown| rendered(&shown, json, summary_text, summary_json)),
        Command::Exclude { index, edit } => {
            commands::exclude(&index, &edit).map(|edited| edited_text(&edited))
        }
        Command::Fixture { dir } => commands::fixture(&dir),
        Command::Clean {
            index,
            root,
            category,
            yes,
        } => commands::clean(&index, &root, category, yes).map(|done| clean_text(&done)),
        Command::Watch { place, limit } => live::watch(&place, limit, out).map(|()| String::new()),
        Command::Bench { place, bench } => match bench {
            Bench::Open => live::bench_open(&place),
            Bench::Churn { length, per_burst } => bench::bench_churn(&place, length, per_burst),
            Bench::Refresh => bench::bench_refresh(&place),
        },
    }
}
