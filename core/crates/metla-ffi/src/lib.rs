//! The Rust/Swift boundary: one `Engine` object, the records the app
//! shows and two traits the app implements, generated into Swift by
//! UniFFI (proc-macro mode). No logic of its own beyond converting,
//! paging and mapping errors (ARCHITECTURE §10).
//!
//! The domain's types cross as they are (`domain`: UniFFI's remote
//! derives, checked against the core's definitions when this compiles);
//! shapes the boundary cannot carry (maps, sets, tuples, private fields)
//! have a record of their own here (`view`). Every call returns at once
//! and is safe from any thread; lists are pages of a bounded size.
//!
//! Only a release build with this crate's `production` feature (the
//! app's release build) makes the capability to clean a root that is not
//! a fixture; every other build, the tests included, cleans fixtures
//! only.

mod callbacks;
mod domain;
mod engine;
mod error;
mod view;

pub use callbacks::{EngineObserver, Observer, Trash, TrashHost, TrashOutcome};
pub use engine::{Engine, EngineOptions, MAX_PAGE, cleans_beyond_fixtures, page};
pub use error::MetlaError;
pub use view::{
    CandidatePage, CategoryTally, CleanupResult, Confirmations, CoverageSummary, EngineSettings,
    FailedPath, FolderDir, FolderItem, FolderPage, GapCount, GapPlace, KindBytes, Leftover,
    Overview, ReportTotals, RuleTitle, SkipCount, covered, group_of, rule_titles,
};

uniffi::setup_scaffolding!();
