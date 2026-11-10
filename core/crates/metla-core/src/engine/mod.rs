//! The engine: one long-lived object that owns the index, keeps the
//! picture fresh and answers everything the app shows.

mod refresh;

pub use refresh::{MAX_SUBTREE, RefreshError, Refreshed, refresh};
