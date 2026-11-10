//! Watching the root for changes: a pure coalescer that turns file-system
//! events into rescan tasks, and the FSEvents stream that feeds it.

mod coalescer;
mod flags;

pub use coalescer::{Coalescer, Event, Timing};
pub use flags::Flags;
