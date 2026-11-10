use std::collections::BTreeSet;
use std::path::Path;

/// A tool that keeps linked worktrees of its own. Metla never removes such
/// a worktree, and nothing inside one is preselected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Manager {
    /// Codex Desktop: below `<home>/.codex/worktrees`.
    Codex,
    /// Claude Code: below a `.claude/worktrees` directory.
    Claude,
}

impl Manager {
    /// The tool managing the worktree that is, or holds, `path`.
    pub fn of(path: &Path, home: &Path) -> Option<Manager> {
        let codex = path.strip_prefix(home.join(".codex/worktrees"));
        if codex.is_ok_and(|inside| inside.components().next().is_some()) {
            return Some(Manager::Codex);
        }
        let names: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
        let mut triples = names.windows(3);
        triples
            .any(|w| w[0] == ".claude" && w[1] == "worktrees")
            .then_some(Manager::Claude)
    }
}

/// Why a linked worktree cannot be removed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// Tracked files are modified or staged, or a merge left conflicts.
    Uncommitted,
    /// Files Git does not track and does not ignore.
    Untracked,
    /// Commits that no remote has.
    Unpushed,
    /// `git worktree lock`.
    Locked,
    /// Git itself considers the worktree prunable.
    Prunable,
    /// A rebase, merge, cherry-pick, revert or bisect is in progress.
    OperationInProgress,
    /// Changed recently, or Git's index is locked.
    InUse,
    /// Another tool owns it.
    Managed(Manager),
    /// Not exactly one linked worktree of its repository (or the main one).
    NotListed,
    /// Git failed or said something Metla does not understand.
    InspectionFailed(String),
}

/// Why a worktree is blocked: never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reasons(BTreeSet<Reason>);

impl Reasons {
    pub fn contains(&self, reason: &Reason) -> bool {
        self.0.contains(reason)
    }

    /// In a fixed order.
    pub fn iter(&self) -> impl Iterator<Item = &Reason> {
        self.0.iter()
    }
}

/// Whether a linked worktree may be removed: removable, or blocked for at
/// least one reason. Nothing can be both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeStatus {
    /// Clean, pushed, unlocked, idle and nobody else's.
    Removable {
        /// `None` for a detached HEAD.
        branch: Option<String>,
    },
    Blocked(Reasons),
}

impl WorktreeStatus {
    /// Removable exactly when there is no reason against it.
    pub fn new(branch: Option<String>, reasons: BTreeSet<Reason>) -> WorktreeStatus {
        match reasons.is_empty() {
            true => WorktreeStatus::Removable { branch },
            false => WorktreeStatus::Blocked(Reasons(reasons)),
        }
    }

    pub fn blocked(reason: Reason) -> WorktreeStatus {
        WorktreeStatus::Blocked(Reasons(BTreeSet::from([reason])))
    }
}
