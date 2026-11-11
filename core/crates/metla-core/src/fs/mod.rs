//! The file system: directory listings, volume information, process
//! limits, and removing what a cleanup removes. The only module with
//! `unsafe` system calls.
//!
//! Directories are opened relative to their parent's descriptor, so paths
//! longer than `PATH_MAX` work, symlinks are never followed and file
//! contents are never opened.

mod bulk;
mod dir;
mod process;
mod remove;
mod sys;
mod volume;

pub use bulk::{Bulk, BulkCall, decode_bulk, getattrlistbulk};
pub use dir::{
    DirHandle, ListError, entry_at, list_dir, list_dir_classic, list_dir_with, visit_dir,
};
pub use process::{
    Usage, background_thread, now, peak_rss, raise_open_file_limit, raised_open_file_limit,
    thread_cpu, usage,
};
pub use remove::{PARALLEL_DEPTH, Removal, Remover, Survey, remove_contents, survey};
pub use sys::retry_interrupted;
pub use volume::{Mount, Volume, mounts, volume};
