//! What `candidates`, `summary` and `exclude` print. Pure: data in, text
//! out.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;

use metla_core::domain::{
    Candidate, Category, Coverage, Disposal, Exclusion, FileKind, KindTotals, Reclaimable, RuleId,
    Tally, UnixTime,
};

use crate::format::{ago, bytes, count};
use crate::report::{
    GAP_NAMES, SKIP_NAMES, completeness, counts_json, coverage_text, either, json_object,
    json_string, line, name_in,
};

/// Data shown at a moment, for relative times.
pub struct Shown<T> {
    pub now: UnixTime,
    pub data: T,
}

/// How fresh the picture is and what it saw.
pub struct Freshness {
    pub root: PathBuf,
    pub finished: UnixTime,
    /// No later scan was started and left unfinished.
    pub current: bool,
    pub coverage: Coverage,
    pub kinds: KindTotals,
}

/// Everything `summary` reports.
pub struct Overview {
    /// `None` until a scan has finished.
    pub freshness: Option<Freshness>,
    /// What cleaning the preselected candidates would free, and what it
    /// would only move to the Trash.
    pub reclaimable: Reclaimable,
    pub categories: BTreeMap<Category, Tally>,
    pub rules: BTreeMap<RuleId, Tally>,
}

/// What `exclude` did: the exclusions, or the one it changed and whether
/// anything changed.
pub enum Edited {
    Listed(Vec<Exclusion>),
    Added(Exclusion, bool),
    Removed(Exclusion, bool),
}

const DISPOSAL_NAMES: [(Disposal, &str); 3] = [
    (Disposal::DeletePermanently, "permanently"),
    (Disposal::MoveToTrash, "to Trash"),
    (Disposal::GitWorktreeRemove, "git worktree remove"),
];

const KIND_NAMES: [(FileKind, &str); 7] = [
    (FileKind::Applications, "Applications"),
    (FileKind::Developer, "Developer"),
    (FileKind::Media, "Media"),
    (FileKind::Documents, "Documents"),
    (FileKind::Archives, "Archives and disk images"),
    (FileKind::System, "System"),
    (FileKind::Other, "Other"),
];

const KIND_KEYS: [(FileKind, &str); 7] = [
    (FileKind::Applications, "applications"),
    (FileKind::Developer, "developer"),
    (FileKind::Media, "media"),
    (FileKind::Documents, "documents"),
    (FileKind::Archives, "archives"),
    (FileKind::System, "system"),
    (FileKind::Other, "other"),
];

fn disposal_name(disposal: Disposal) -> &'static str {
    name_in(&DISPOSAL_NAMES, disposal)
}

fn activity(now: UnixTime, activity: Option<UnixTime>) -> String {
    activity.map_or("-".to_owned(), |time| ago(now.0 - time.0))
}

/// One line per candidate: size, activity, preselection, rule, path.
pub fn candidates_text(shown: &Shown<Vec<Candidate>>) -> String {
    let mut out = String::new();
    for c in &shown.data {
        let _ = writeln!(
            out,
            "{:>10}  {:>7}  {}  {:<30}  {}",
            bytes(c.bytes),
            activity(shown.now, c.activity),
            either(c.preselected, "[x]", "[ ]"),
            c.rule.as_str(),
            c.path.display()
        );
    }
    out
}

pub fn candidates_json(shown: &Shown<Vec<Candidate>>) -> String {
    let rows: Vec<String> = shown.data.iter().map(candidate_json).collect();
    format!("[{}]", rows.join(","))
}

fn candidate_json(c: &Candidate) -> String {
    json_object(&[
        ("id", c.id.0.to_string()),
        ("path", json_string(&c.path.to_string_lossy())),
        ("rule", json_string(c.rule.as_str())),
        ("category", json_string(c.category().name())),
        ("disposal", json_string(disposal_name(c.disposal()))),
        ("bytes", c.bytes.0.to_string()),
        ("files", c.files.to_string()),
        (
            "activity",
            c.activity.map_or("null".to_owned(), |t| t.0.to_string()),
        ),
        ("preselected", c.preselected.to_string()),
    ])
}

fn rules_of(
    rules: &BTreeMap<RuleId, Tally>,
    category: Category,
) -> impl Iterator<Item = (&RuleId, &Tally)> {
    rules
        .iter()
        .filter(move |(rule, _)| rule.category() == category)
}

pub fn summary_text(shown: &Shown<Overview>) -> String {
    let mut out = String::new();
    let overview = &shown.data;
    match &overview.freshness {
        Some(freshness) => freshness_text(&mut out, shown.now, freshness),
        None => line(&mut out, "picture", "none: no scan has finished"),
    }
    let Reclaimable { freed, trashed } = &overview.reclaimable;
    let freed = format!(
        "{} in {} preselected candidates (estimate)",
        bytes(freed.bytes),
        count(freed.count)
    );
    line(&mut out, "can be freed", freed);
    if trashed.count > 0 {
        let trashed = format!(
            "{} in {} preselected candidates, not freed until the Trash is emptied",
            bytes(trashed.bytes),
            count(trashed.count)
        );
        line(&mut out, "to the Trash", trashed);
    }
    for (category, tally) in &overview.categories {
        let _ = writeln!(
            out,
            "  {:<30}{:>10}  {:>7}",
            category.name(),
            bytes(tally.bytes),
            count(tally.count)
        );
        for (rule, tally) in rules_of(&overview.rules, *category) {
            let name = rule.as_str();
            let disposal = disposal_name(rule.disposal());
            let _ = writeln!(
                out,
                "    {name:<28}{:>10}  {:>7}  {disposal}",
                bytes(tally.bytes),
                count(tally.count)
            );
        }
    }
    if let Some(freshness) = &overview.freshness {
        out.push_str("data types\n");
        for (kind, size) in freshness.kinds.iter() {
            let _ = writeln!(
                out,
                "  {:<30}{:>10}",
                name_in(&KIND_NAMES, kind),
                bytes(size)
            );
        }
    }
    out
}

fn freshness_text(out: &mut String, now: UnixTime, freshness: &Freshness) {
    let finished = ago(now.0 - freshness.finished.0);
    line(
        out,
        "picture",
        format!("{} finished {finished} ago", freshness.root.display()),
    );
    if !freshness.current {
        line(out, "", "a later scan started and did not finish");
    }
    coverage_text(out, &freshness.coverage);
}

pub fn summary_json(shown: &Shown<Overview>) -> String {
    let overview = &shown.data;
    let picture = overview
        .freshness
        .as_ref()
        .map_or("null".to_owned(), |f| freshness_json(shown.now, f));
    let categories: Vec<String> = overview
        .categories
        .iter()
        .map(|(category, tally)| category_json(*category, tally, &overview.rules))
        .collect();
    json_object(&[
        ("picture", picture),
        ("freed", tally_json(&overview.reclaimable.freed, &[])),
        ("trashed", tally_json(&overview.reclaimable.trashed, &[])),
        ("categories", format!("[{}]", categories.join(","))),
    ])
}

fn freshness_json(now: UnixTime, f: &Freshness) -> String {
    let coverage = &f.coverage;
    json_object(&[
        ("root", json_string(&f.root.to_string_lossy())),
        ("finished", f.finished.0.to_string()),
        ("age_seconds", (now.0 - f.finished.0).to_string()),
        ("current", f.current.to_string()),
        ("coverage", json_string(completeness(coverage))),
        ("gaps", counts_json(&GAP_NAMES, |k| coverage.gaps(k))),
        ("skipped", counts_json(&SKIP_NAMES, |k| coverage.skips(k))),
        ("kinds", counts_json(&KIND_KEYS, |k| f.kinds.get(k).0)),
    ])
}

fn tally_json(tally: &Tally, more: &[(&str, String)]) -> String {
    let mut fields = vec![
        ("count", tally.count.to_string()),
        ("bytes", tally.bytes.0.to_string()),
    ];
    fields.extend(more.iter().cloned());
    json_object(&fields)
}

fn category_json(category: Category, tally: &Tally, rules: &BTreeMap<RuleId, Tally>) -> String {
    let rules: Vec<String> = rules_of(rules, category)
        .map(|(rule, tally)| {
            let disposal = json_string(disposal_name(rule.disposal()));
            tally_json(
                tally,
                &[("rule", json_string(rule.as_str())), ("disposal", disposal)],
            )
        })
        .collect();
    tally_json(
        tally,
        &[
            ("category", json_string(category.name())),
            ("rules", format!("[{}]", rules.join(","))),
        ],
    )
}

pub fn edited_text(edited: &Edited) -> String {
    let (said, exclusions) = match edited {
        Edited::Listed(all) => ("", all.as_slice()),
        Edited::Added(one, true) => ("excluded ", std::slice::from_ref(one)),
        Edited::Added(one, false) => ("already excluded ", std::slice::from_ref(one)),
        Edited::Removed(one, true) => ("no longer excluded ", std::slice::from_ref(one)),
        Edited::Removed(one, false) => ("was not excluded ", std::slice::from_ref(one)),
    };
    let mut out = String::new();
    for exclusion in exclusions {
        let _ = writeln!(out, "{said}{}", exclusion.path().display());
    }
    out
}
