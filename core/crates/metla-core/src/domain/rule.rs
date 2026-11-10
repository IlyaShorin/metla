use std::fmt;
use std::sync::Arc;

use super::candidate::Recency;
use super::entry::UnixTime;
use super::verdict::Verdict;
use super::worktree::Manager;

/// The kind of a candidate. It decides how the candidate is removed and
/// whether a hotkey may clean it without a confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    AppCache,
    DevCache,
    BuildArtifact,
    Log,
    GitWorktree,
    Temp,
}

/// How a candidate is removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Disposal {
    DeletePermanently,
    MoveToTrash,
    GitWorktreeRemove,
}

/// The only categories a hotkey may clean without showing a confirmation.
/// There is no value for logs, worktrees or temporary data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuickCategory {
    AppCache,
    DevCache,
    BuildArtifact,
}

impl Category {
    /// Every category, in the order of ARCHITECTURE.md §3.
    pub const ALL: [Category; 6] = [
        Category::AppCache,
        Category::DevCache,
        Category::BuildArtifact,
        Category::Log,
        Category::GitWorktree,
        Category::Temp,
    ];

    /// The stable name: the prefix of every rule id of the category.
    pub fn name(self) -> &'static str {
        match self {
            Category::AppCache => "app-cache",
            Category::DevCache => "dev-cache",
            Category::BuildArtifact => "build",
            Category::Log => "log",
            Category::GitWorktree => "worktree",
            Category::Temp => "temp",
        }
    }

    pub fn from_name(name: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.name() == name)
    }

    /// How candidates of the category are removed. The single place this is
    /// decided (one exception for the Trash, see [`RuleId::disposal`]).
    pub fn disposal(self) -> Disposal {
        match self {
            Category::AppCache | Category::DevCache | Category::BuildArtifact => {
                Disposal::DeletePermanently
            }
            Category::Log | Category::Temp => Disposal::MoveToTrash,
            Category::GitWorktree => Disposal::GitWorktreeRemove,
        }
    }

    /// The quick-clean category, for the only categories that have one.
    pub fn quick(self) -> Option<QuickCategory> {
        match self {
            Category::AppCache => Some(QuickCategory::AppCache),
            Category::DevCache => Some(QuickCategory::DevCache),
            Category::BuildArtifact => Some(QuickCategory::BuildArtifact),
            Category::Log | Category::GitWorktree | Category::Temp => None,
        }
    }

    /// Whether a candidate of the category is offered only with Git's
    /// verdict: project output, which Git must ignore and must not track.
    pub fn needs_verdict(self) -> bool {
        matches!(self, Category::BuildArtifact)
    }

    /// Whether a candidate of the category with this verdict (`None`: Git
    /// was not asked) is offered.
    pub fn offered(self, verdict: Option<Verdict>) -> bool {
        !self.needs_verdict() || verdict == Some(Verdict::Verified)
    }

    /// Whether the directories inside a candidate of the category are still
    /// classified. Only a linked worktree lets them be: most worktrees
    /// cannot be removed whole, and their build output is offered on its
    /// own.
    pub fn classifies_within(self) -> bool {
        matches!(self, Category::GitWorktree)
    }

    /// Whether a candidate is checked by default: never inside (or as) a
    /// worktree another tool manages, not for build output that was active
    /// recently, never for a worktree (its status decides).
    pub fn preselected(
        self,
        activity: Option<UnixTime>,
        recency: &Recency,
        managed: Option<Manager>,
    ) -> bool {
        managed.is_none()
            && match self {
                Category::BuildArtifact => !recency.is_recent(activity),
                Category::GitWorktree => false,
                _ => true,
            }
    }
}

/// The stable id of a rule, such as `dev-cache.npm`: its category's name, a
/// dot and the rule's own name. The category is part of the id, so the two
/// cannot disagree.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleId {
    category: Category,
    id: Arc<str>,
}

impl RuleId {
    pub fn new(category: Category, name: &str) -> RuleId {
        RuleId {
            category,
            id: format!("{}.{name}", category.name()).into(),
        }
    }

    /// An id as stored, if it names a known category and a rule.
    pub fn parse(id: &str) -> Option<RuleId> {
        let (category, name) = id.split_once('.')?;
        let category = Category::from_name(category)?;
        (!name.is_empty()).then(|| RuleId::new(category, name))
    }

    /// The rule for the user's Trash.
    pub fn trash() -> RuleId {
        RuleId::new(Category::Temp, "trash")
    }

    pub fn as_str(&self) -> &str {
        &self.id
    }

    pub fn category(&self) -> Category {
        self.category
    }

    /// How a candidate of this rule is removed: as its category says, except
    /// the Trash, whose contents cannot be moved to the Trash and are deleted
    /// ("Empty Trash").
    pub fn disposal(&self) -> Disposal {
        match *self == RuleId::trash() {
            true => Disposal::DeletePermanently,
            false => self.category.disposal(),
        }
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)
    }
}
