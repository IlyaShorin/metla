//! Behaviour tests for metla-core, compiled as one binary so that every
//! mutant needs a single link step.

mod architecture;
mod candidates;
mod category;
mod classify;
mod cleaning;
mod cleanworktrees;
mod coalescer;
mod domain;
mod engine;
mod fs;
mod fsevents;
mod gitrun;
mod incremental;
mod index;
mod kept;
mod offered;
mod porcelain;
mod preempt;
mod preview;
mod remove;
mod repo;
mod rescan;
mod revalidate;
mod rules;
mod scan;
mod scope;
mod stream;
mod support;
mod verify;
mod worktrees;
mod world;
