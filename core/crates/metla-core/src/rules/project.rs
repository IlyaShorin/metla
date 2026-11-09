//! Project-output rules: a directory name, markers that prove which tool
//! made the directory, and a surrounding Git repository. No marker, no
//! match. Nothing inside an installed package tree is project output: a
//! version manager or a package prefix is often a Git clone that keeps
//! installed packages in an ignored `lib/node_modules`, `site-packages` or
//! Homebrew `Cellar`.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::catalogue::{Marker, Matcher, Rule};
use super::names::NamePattern;
use crate::domain::{DirFacts, Probe};

/// The file a `CACHEDIR.TAG` marker is read from, and how it must start
/// (<https://bford.info/cachedir/>).
pub const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Installed packages live below directories of these names.
const INSTALLED: [&str; 4] = ["node_modules", "site-packages", "dist-packages", "Cellar"];

#[derive(Debug)]
enum Evidence {
    Sibling(NamePattern),
    Inner(NamePattern),
    CacheDirTag,
}

impl Evidence {
    fn of(marker: &Marker) -> Evidence {
        match marker {
            Marker::Sibling(name) => Evidence::Sibling(NamePattern::parse(name)),
            Marker::Inner(name) => Evidence::Inner(NamePattern::parse(name)),
            Marker::CacheDirTag => Evidence::CacheDirTag,
        }
    }

    fn holds(&self, facts: &DirFacts<'_>) -> bool {
        match self {
            Evidence::Sibling(name) => facts
                .parent_markers
                .iter()
                .any(|n| name.matches(n.as_bytes())),
            Evidence::Inner(name) => facts
                .entries
                .iter()
                .any(|e| name.matches(e.name.as_bytes())),
            Evidence::CacheDirTag => facts.probes.iter().any(is_cache_dir_tag),
        }
    }
}

fn is_cache_dir_tag(probe: &Probe) -> bool {
    probe.name == CACHEDIR_TAG && probe.head.starts_with(SIGNATURE)
}

#[derive(Debug)]
struct Proof {
    rule: usize,
    /// Every marker of at least one group must hold.
    groups: Vec<Vec<Evidence>>,
}

impl Proof {
    fn holds(&self, facts: &DirFacts<'_>) -> bool {
        let mut groups = self.groups.iter();
        groups.any(|group| group.iter().all(|evidence| evidence.holds(facts)))
    }
}

#[derive(Debug, Default)]
pub struct Projects {
    proofs: Vec<Proof>,
    by_name: HashMap<Vec<u8>, Vec<usize>>,
    any_name: Vec<usize>,
}

impl Projects {
    pub fn new(rules: &[Rule]) -> Projects {
        let mut projects = Projects::default();
        for (rule, entry) in rules.iter().enumerate() {
            if let Matcher::Project { names, markers } = &entry.matcher {
                projects.add(rule, names, markers);
            }
        }
        projects
    }

    fn add(&mut self, rule: usize, names: &[&str], markers: &[&[Marker]]) {
        let proof = self.proofs.len();
        let groups = markers
            .iter()
            .map(|group| group.iter().map(Evidence::of).collect());
        self.proofs.push(Proof {
            rule,
            groups: groups.collect(),
        });
        for name in names {
            self.by_name
                .entry(name.as_bytes().to_vec())
                .or_default()
                .push(proof);
        }
        if names.is_empty() {
            self.any_name.push(proof);
        }
    }

    /// The names of sibling markers, which the walker must report.
    pub fn siblings(&self) -> impl Iterator<Item = &NamePattern> {
        let evidence = self.proofs.iter().flat_map(|p| p.groups.iter().flatten());
        evidence.filter_map(|e| match e {
            Evidence::Sibling(name) => Some(name),
            _ => None,
        })
    }

    /// The first rule, in catalogue order (named rules before any-name
    /// rules), whose markers prove the directory is its output.
    pub fn find(&self, facts: &DirFacts<'_>) -> Option<usize> {
        let name = facts.path.file_name()?.as_bytes();
        let named = self.by_name.get(name).into_iter().flatten();
        let proofs = named
            .chain(&self.any_name)
            .filter(|_| facts.inside_git_repo);
        let mut proofs = proofs.map(|&p| &self.proofs[p]);
        let found = proofs.find(|proof| proof.holds(facts));
        found
            .filter(|_| !installed(facts.path))
            .map(|proof| proof.rule)
    }
}

/// Whether `path` lies inside an installed package tree.
fn installed(path: &Path) -> bool {
    let above = path.parent().into_iter().flat_map(Path::components);
    let mut names = above.map(|part| part.as_os_str());
    names.any(|name| INSTALLED.iter().any(|installed| name == *installed))
}
