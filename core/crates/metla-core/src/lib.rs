//! Metla core: scanning, indexing and (later) classifying and cleaning.
//!
//! Module dependencies follow docs/ARCHITECTURE.md §2; the rule is checked
//! by `tests/core/architecture.rs`.

pub mod clean;
pub mod domain;
pub mod engine;
pub mod fs;
pub mod git;
pub mod index;
pub mod rules;
pub mod scan;
pub mod watch;
