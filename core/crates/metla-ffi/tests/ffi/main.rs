//! Tests of the boundary's own logic: conversions, paging, error
//! mapping, the callbacks, and that it cannot clean a real root. One
//! binary, so every mutant needs a single link step.

mod convert;
mod engine;
mod safety;
mod support;
