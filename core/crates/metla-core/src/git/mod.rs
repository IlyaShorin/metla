//! Asking Git, through the stock `git` binary: whether a linked worktree
//! can be removed, removing it with `git worktree remove` (never forced),
//! and whether project output is ignored and holds nothing tracked.
//! Nothing else here changes a repository.

mod porcelain;
mod run;
mod verify;
mod worktree;

pub use porcelain::{Listed, Status, Unparsable, count, fields, status, worktree_list};
pub use run::{Git, Output, RunError};
pub use verify::{repository_of, verify_outputs};
pub use worktree::{WorktreeSettings, inspect_worktree, remove_worktree};
