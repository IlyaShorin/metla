//! Reading the file system: directory listings, volume information and
//! process limits. The only module with `unsafe` system calls.
//!
//! Directories are opened relative to their parent's descriptor, so paths
//! longer than `PATH_MAX` work, symlinks are never followed and file
//! contents are never opened.

mod dir;
mod process;
mod sys;
mod volume;

pub use dir::{DirHandle, ListError, list_dir};
pub use process::{now, peak_rss, raise_open_file_limit, raised_open_file_limit};
pub use sys::retry_interrupted;
pub use volume::{Mount, Volume, mounts, volume};
