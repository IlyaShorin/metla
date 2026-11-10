//! Classification: which rule, if any, a directory matches, and what kind
//! of data its files are. Pure: the catalogue is data, the walker supplies
//! the facts, and nothing here reads the file system or the environment.
//!
//! Precedence: a location rule, then a linked worktree, then project
//! output; within each, catalogue order.

mod catalogue;
mod kinds;
mod location;
mod names;
mod pattern;
mod project;
mod worktree;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::domain::{DirFacts, FileKind, RuleId};

pub use crate::domain::Area;
pub use catalogue::{CATALOGUE, Marker, Matcher, Removes, Rule};

use kinds::Kinds;
use location::Locations;
use names::{NamePattern, Names};
use project::{CACHEDIR_TAG, Projects};
use worktree::GIT;

/// How many bytes of a probed file the rules look at.
pub const PROBE_LIMIT: usize = 2048;

/// The catalogue compiled for one home directory.
#[derive(Debug)]
pub struct RuleSet {
    home: PathBuf,
    ids: Vec<RuleId>,
    locations: Locations,
    projects: Projects,
    worktrees: Option<usize>,
    markers: Names,
    kinds: Kinds,
}

impl RuleSet {
    /// The rules for the user `user` whose home directory is `home` (an
    /// absolute, canonical path).
    pub fn new(home: &Path, user: &OsStr) -> RuleSet {
        let rules = CATALOGUE;
        let locations = Locations::new(rules, home.as_os_str().as_bytes(), user.as_bytes());
        let projects = Projects::new(rules);
        let required = locations
            .required()
            .map(|name| NamePattern::Exact(name.to_vec()));
        let markers = Names::new(projects.siblings().cloned().chain(required));
        RuleSet {
            home: home.to_path_buf(),
            ids: rules
                .iter()
                .map(|r| RuleId::new(r.category, r.name))
                .collect(),
            worktrees: rules
                .iter()
                .position(|r| matches!(r.matcher, Matcher::LinkedWorktree)),
            locations,
            projects,
            markers,
            kinds: Kinds::new(home),
        }
    }

    /// The home directory the rules were compiled for.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Every rule with its id, in catalogue order.
    pub fn rules(&self) -> impl Iterator<Item = (&RuleId, &'static Rule)> {
        self.ids.iter().zip(CATALOGUE)
    }

    /// The rule with this id, if the catalogue has it.
    pub fn rule(&self, id: &RuleId) -> Option<&'static Rule> {
        self.rules()
            .find(|(known, _)| *known == id)
            .map(|(_, rule)| rule)
    }

    /// Whether the walker must report an entry with this name to the
    /// children's classification (a sibling marker).
    pub fn is_marker(&self, name: &OsStr) -> bool {
        self.markers.matches(name.as_bytes())
    }

    /// The small files whose first [`PROBE_LIMIT`] bytes the rules look at
    /// when a directory contains them as regular files.
    pub fn probes(&self) -> [&'static str; 2] {
        [GIT, CACHEDIR_TAG]
    }

    /// The rule the directory matches, if any.
    pub fn classify(&self, facts: &DirFacts<'_>) -> Option<&RuleId> {
        let worktree = || self.worktrees.filter(|_| worktree::is_linked(facts));
        let rule = self.locations.find(facts);
        let rule = rule.or_else(worktree).or_else(|| self.projects.find(facts));
        rule.map(|index| &self.ids[index])
    }

    /// The area of a directory: its parent's, or a stronger one of its own
    /// (see [`Area`]). `in_repo`: the directory is, or is inside, a Git
    /// repository.
    pub fn area(&self, parent: Area, path: &Path, in_repo: bool, rule: Option<&RuleId>) -> Area {
        self.kinds
            .area(parent, path, in_repo, rule.map(RuleId::category))
    }

    /// The area a scan root inherits from the unread directories above it.
    pub fn area_above(&self, root: &Path) -> Area {
        self.kinds.above(root)
    }

    /// The kind of the file `name` in a directory of `area`.
    pub fn kind(&self, area: Area, name: &OsStr) -> FileKind {
        self.kinds.kind(area, name)
    }
}
