//! Watching the root for changes: a pure coalescer that turns file-system
//! events into rescan tasks, and the FSEvents stream that feeds it
//! (`sys`, the only part with foreign calls).

mod coalescer;
mod flags;
mod resume;
mod sys;

pub use coalescer::{Coalescer, Event, FullCause, Timing};
pub use flags::Flags;
pub use resume::resume_after;
pub use sys::{MAX_EXCLUDED, Stream, current_event_id, events_of, volume_uuid};
