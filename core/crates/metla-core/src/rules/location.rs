//! Location rules: exact path patterns compiled into a component trie, so
//! a directory costs one map lookup per component of its path.
//!
//! Matching prefers a literal component to `*` at every level. A `*` that
//! is the last component never claims a name that another pattern spells
//! out at that position: `~/Library/Caches/*` leaves `Homebrew` to its own
//! rule and `Firefox` to the pattern that goes deeper into it.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;

use super::catalogue::{Matcher, Rule};
use super::names::{NamePattern, Names};
use super::pattern::{Step, expand, steps};
use crate::domain::DirFacts;

#[derive(Debug)]
struct Pattern {
    rule: usize,
    steps: Vec<Step>,
    deny: Names,
}

#[derive(Debug, Default)]
struct Node {
    literal: HashMap<Vec<u8>, usize>,
    any: Option<usize>,
    /// Patterns that end here, in catalogue order.
    ends: Vec<usize>,
}

/// A directory being matched: its path components and its facts.
struct Query<'q> {
    parts: Vec<&'q [u8]>,
    facts: &'q DirFacts<'q>,
}

#[derive(Debug)]
pub struct Locations {
    nodes: Vec<Node>,
    patterns: Vec<Pattern>,
    deepest: usize,
}

/// The trie node every path starts from: `/`.
const ROOT: usize = 0;

impl Locations {
    /// Every location pattern of `rules`, with `~` meaning `home` and
    /// `<user>` meaning `user`.
    pub fn new(rules: &[Rule], home: &[u8], user: &[u8]) -> Locations {
        let compiled = rules.iter().enumerate();
        let patterns: Vec<Pattern> = compiled
            .flat_map(|(rule, entry)| compile(rule, entry, home, user))
            .collect();
        let mut nodes = vec![Node::default()];
        for (index, pattern) in patterns.iter().enumerate() {
            let end = pattern.steps.iter().fold(ROOT, |node, step| {
                child(&mut nodes, node, step.name.as_deref())
            });
            nodes[end].ends.push(index);
        }
        let deepest = patterns.iter().map(|p| p.steps.len()).max();
        Locations {
            nodes,
            deepest: deepest.unwrap_or_default(),
            patterns,
        }
    }

    /// The names some pattern requires a directory above a match to hold.
    pub fn required(&self) -> impl Iterator<Item = &[u8]> {
        let steps = self.patterns.iter().flat_map(|p| &p.steps);
        steps.flat_map(|s| &s.requires).map(Vec::as_slice)
    }

    /// The rule whose pattern matches the directory, if any.
    pub fn find(&self, facts: &DirFacts<'_>) -> Option<usize> {
        let bytes = facts.path.as_os_str().as_bytes();
        let parts: Vec<&[u8]> = bytes
            .split(|b| *b == b'/')
            .filter(|part| !part.is_empty())
            .take(self.deepest + 1)
            .collect();
        let query = Query { parts, facts };
        (query.parts.len() <= self.deepest)
            .then(|| self.descend(ROOT, 0, &query))
            .flatten()
    }

    /// Matches the parts from `depth` on below `node`, literal first. A
    /// last part a literal sibling spells is not taken by `*`.
    fn descend(&self, node: usize, depth: usize, query: &Query<'_>) -> Option<usize> {
        let Some(name) = query.parts.get(depth) else {
            return self.ending(node, query);
        };
        let trie = &self.nodes[node];
        let literal = trie.literal.get(*name).copied();
        let next = depth + 1;
        let shadowed = literal.is_some() && next == query.parts.len();
        let any = trie.any.filter(|_| !shadowed);
        literal
            .and_then(|child| self.descend(child, next, query))
            .or_else(|| any.and_then(|child| self.descend(child, next, query)))
    }

    fn ending(&self, node: usize, query: &Query<'_>) -> Option<usize> {
        let mut ends = self.nodes[node].ends.iter().map(|&p| &self.patterns[p]);
        ends.find(|pattern| pattern.accepts(query))
            .map(|pattern| pattern.rule)
    }
}

/// The patterns of a location rule.
fn compile(rule: usize, entry: &Rule, home: &[u8], user: &[u8]) -> Vec<Pattern> {
    let Matcher::Location { patterns, deny } = &entry.matcher else {
        return Vec::new();
    };
    let deny = Names::new(deny.iter().map(|d| NamePattern::parse(d)));
    let expanded = patterns.iter().flat_map(|p| expand(p));
    expanded
        .map(|pattern| Pattern {
            rule,
            steps: steps(&pattern, home, user),
            deny: deny.clone(),
        })
        .collect()
}

/// The child of `node` for `name` (`None`: `*`), created when missing.
fn child(nodes: &mut Vec<Node>, node: usize, name: Option<&[u8]>) -> usize {
    let next = nodes.len();
    let parent = &mut nodes[node];
    let child = match name {
        Some(name) => *parent.literal.entry(name.to_vec()).or_insert(next),
        None => *parent.any.get_or_insert(next),
    };
    if child == next {
        nodes.push(Node::default());
    }
    child
}

impl Pattern {
    /// No `*` took a denied name, and every condition holds.
    fn accepts(&self, query: &Query<'_>) -> bool {
        let last = query.parts.len() - 1;
        let mut steps = self.steps.iter().zip(&query.parts).enumerate();
        steps.all(|(at, (step, name))| {
            self.allows(step, name) && holds(step, last - at, query.facts)
        })
    }

    fn allows(&self, step: &Step, name: &[u8]) -> bool {
        step.name.is_some() || !self.deny.matches(name)
    }
}

/// Whether the directory `level` steps above the matched one contains one
/// of the entries the step requires. The walker reports the markers of the
/// parent and the grandparent only.
fn holds(step: &Step, level: usize, facts: &DirFacts<'_>) -> bool {
    let reported = [facts.parent_markers, facts.grandparent_markers];
    let markers = level.checked_sub(1).and_then(|up| reported.get(up));
    step.requires.is_empty()
        || markers.is_some_and(|markers| {
            let mut names = markers.iter().map(|m| m.as_bytes());
            names.any(|name| step.requires.iter().any(|r| r == name))
        })
}
