//! The last cleanup's report as the engine keeps it in the index (a
//! note), so the app shows it again after a restart.
//!
//! One line for the report (preview, free space before and after) and
//! one per item (candidate, path, rule, found, removed, outcome), fields
//! apart by tabs. A field escapes `\`, tab and newline with a backslash,
//! so a path or a message may hold any byte. Enumerations are written as
//! their place in a fixed table. What does not read back exactly is not
//! a report.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::str::FromStr;

use crate::domain::{
    ActionFailure, Bytes, CleanupReport, DirId, Errno, ItemReport, Leftovers, Manager, Outcome,
    PreviewId, Reason, RuleId, SkipReason, WorktreeStatus,
};

/// The reasons to skip an item that carry nothing, by their code.
const SKIPS: [SkipReason; 13] = [
    SkipReason::NotConfirmed,
    SkipReason::Cancelled,
    SkipReason::Excluded,
    SkipReason::OutsideRoot,
    SkipReason::Symlink,
    SkipReason::NotADirectory,
    SkipReason::Replaced,
    SkipReason::MountPoint,
    SkipReason::AncestorChanged,
    SkipReason::RuleMismatch,
    SkipReason::NotIgnored,
    SkipReason::HasTrackedFiles,
    SkipReason::Unverifiable,
];

/// The reasons a worktree is blocked for, by their code; a failed
/// inspection is written as `!` and Git's message.
const REASONS: [Reason; 10] = [
    Reason::Uncommitted,
    Reason::Untracked,
    Reason::Unpushed,
    Reason::Locked,
    Reason::Prunable,
    Reason::OperationInProgress,
    Reason::InUse,
    Reason::NotListed,
    Reason::Managed(Manager::Codex),
    Reason::Managed(Manager::Claude),
];

/// `report` as the engine keeps it.
pub fn encode_report(report: &CleanupReport) -> Vec<u8> {
    let head = [
        report.preview.0.to_string(),
        report.free_before.0.to_string(),
        report.free_after.0.to_string(),
    ];
    let mut lines = vec![line(head.iter().map(|f| f.as_bytes().to_vec()).collect())];
    lines.extend(report.items.iter().map(|item| line(item_fields(item))));
    lines.join(&b'\n')
}

/// The report `bytes` hold, if they hold exactly one.
pub fn decode_report(bytes: &[u8]) -> Option<CleanupReport> {
    let mut lines = bytes.split(|b| *b == b'\n');
    let mut head = Fields::of(lines.next()?)?;
    let preview = PreviewId(head.number()?);
    let free_before = Bytes(head.number()?);
    let free_after = Bytes(head.number()?);
    head.end()?;
    let items: Option<Vec<ItemReport>> = lines.map(item).collect();
    Some(CleanupReport {
        preview,
        items: items?,
        free_before,
        free_after,
    })
}

fn line(fields: Vec<Vec<u8>>) -> Vec<u8> {
    let escaped: Vec<Vec<u8>> = fields.iter().map(|f| escape(f)).collect();
    escaped.join(&b'\t')
}

fn escape(field: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.len());
    for byte in field {
        match byte {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            other => out.push(*other),
        }
    }
    out
}

fn unescape(field: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(field.len());
    let mut bytes = field.iter();
    while let Some(byte) = bytes.next() {
        let plain = match (byte, bytes.as_slice().first()) {
            (b'\\', Some(b'\\')) => b'\\',
            (b'\\', Some(b't')) => b'\t',
            (b'\\', Some(b'n')) => b'\n',
            (b'\\', _) => return None,
            (other, _) => {
                out.push(*other);
                continue;
            }
        };
        bytes.next();
        out.push(plain);
    }
    Some(out)
}

fn text(value: impl ToString) -> Vec<u8> {
    value.to_string().into_bytes()
}

fn item_fields(item: &ItemReport) -> Vec<Vec<u8>> {
    let mut fields = vec![
        text(item.candidate.0),
        item.path.as_os_str().as_bytes().to_vec(),
        text(item.rule.as_str()),
        text(item.found.0),
        text(item.removed.0),
    ];
    outcome_fields(&item.outcome, &mut fields);
    fields
}

fn outcome_fields(outcome: &Outcome, fields: &mut Vec<Vec<u8>>) {
    match outcome {
        Outcome::Removed => fields.push(text("removed")),
        Outcome::AlreadyGone => fields.push(text("gone")),
        Outcome::MovedToTrash => fields.push(text("trash")),
        Outcome::Failed(failure) => {
            fields.push(text("failed"));
            failure_fields(failure, fields);
        }
        Outcome::Skipped(reason) => {
            fields.push(text("skipped"));
            skip_fields(reason, fields);
        }
        Outcome::Partial(left) => {
            fields.extend([text("partial"), text(left.failures)]);
            fields.extend([text(u8::from(left.cancelled)), text(left.first.len())]);
            for (path, errno) in &left.first {
                fields.extend([path.as_os_str().as_bytes().to_vec(), text(errno.0)]);
            }
            let mounts = left.mounts.iter();
            fields.extend(mounts.map(|m| m.as_os_str().as_bytes().to_vec()));
        }
    }
}

/// A failure's kind, then what it carries: the counts and the Trash's
/// words, or Git's.
fn failure_fields(failure: &ActionFailure, fields: &mut Vec<Vec<u8>>) {
    match failure {
        ActionFailure::Unlisted => fields.push(text("unlisted")),
        ActionFailure::TrashRefused { refused, of, said } => {
            fields.extend([text("trash"), text(refused), text(of), text(said)])
        }
        ActionFailure::GitRefused(said) => fields.extend([text("git"), text(said)]),
    }
}

fn skip_fields(reason: &SkipReason, fields: &mut Vec<Vec<u8>>) {
    match reason {
        SkipReason::Blocked(reasons) => {
            fields.push(text("blocked"));
            fields.extend(reasons.iter().map(reason_field));
        }
        SkipReason::Unreadable(errno) => fields.extend([text("unreadable"), text(errno.0)]),
        simple => {
            let code = SKIPS.iter().position(|s| s == simple).unwrap_or_default();
            fields.extend([text("skip"), text(code)]);
        }
    }
}

fn reason_field(reason: &Reason) -> Vec<u8> {
    match reason {
        Reason::InspectionFailed(why) => text(format!("!{why}")),
        listed => text(REASONS.iter().position(|r| r == listed).unwrap_or_default()),
    }
}

/// A flag written as `0` or `1`.
fn flag(fields: &mut Fields) -> Option<bool> {
    match fields.text()?.as_str() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// A count, then that many paths each with its errno.
fn failed_paths(fields: &mut Fields) -> Option<Vec<(PathBuf, Errno)>> {
    let count: usize = fields.number()?;
    let mut first = Vec::with_capacity(count.min(Leftovers::FIRST));
    for _ in 0..count {
        first.push((fields.path()?, Errno(fields.number()?)));
    }
    Some(first)
}

/// A line's fields, read one after another.
struct Fields(std::vec::IntoIter<Vec<u8>>);

impl Fields {
    fn of(line: &[u8]) -> Option<Fields> {
        let fields: Option<Vec<Vec<u8>>> = line.split(|b| *b == b'\t').map(unescape).collect();
        Some(Fields(fields?.into_iter()))
    }

    fn bytes(&mut self) -> Option<Vec<u8>> {
        self.0.next()
    }

    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }

    fn number<T: FromStr>(&mut self) -> Option<T> {
        self.text()?.parse().ok()
    }

    fn path(&mut self) -> Option<PathBuf> {
        Some(PathBuf::from(OsStr::from_bytes(&self.bytes()?)))
    }

    /// Nothing is left.
    fn end(&mut self) -> Option<()> {
        self.0.next().is_none().then_some(())
    }
}

fn item(line: &[u8]) -> Option<ItemReport> {
    let mut fields = Fields::of(line)?;
    let candidate = DirId(fields.number()?);
    let path = fields.path()?;
    let rule = RuleId::parse(&fields.text()?)?;
    let found = Bytes(fields.number()?);
    let removed = Bytes(fields.number()?);
    let outcome = outcome(&mut fields)?;
    Some(ItemReport {
        candidate,
        path,
        rule,
        found,
        removed,
        outcome,
    })
}

fn outcome(fields: &mut Fields) -> Option<Outcome> {
    let tag = fields.text()?;
    match tag.as_str() {
        "skipped" => skip(fields).map(Outcome::Skipped),
        "partial" => leftovers(fields).map(Outcome::Partial),
        "failed" => failure(fields).map(Outcome::Failed),
        _ => plain(&tag, fields),
    }
}

/// An outcome that carries nothing.
fn plain(tag: &str, fields: &mut Fields) -> Option<Outcome> {
    let outcome = match tag {
        "removed" => Outcome::Removed,
        "gone" => Outcome::AlreadyGone,
        "trash" => Outcome::MovedToTrash,
        _ => return None,
    };
    fields.end().map(|()| outcome)
}

fn failure(fields: &mut Fields) -> Option<ActionFailure> {
    let failure = match fields.text()?.as_str() {
        "unlisted" => ActionFailure::Unlisted,
        "trash" => trash_refused(fields)?,
        "git" => ActionFailure::GitRefused(fields.text()?),
        _ => return None,
    };
    fields.end().map(|()| failure)
}

fn trash_refused(fields: &mut Fields) -> Option<ActionFailure> {
    Some(ActionFailure::TrashRefused {
        refused: fields.number()?,
        of: fields.number()?,
        said: fields.text()?,
    })
}

fn skip(fields: &mut Fields) -> Option<SkipReason> {
    let reason = match fields.text()?.as_str() {
        "skip" => SKIPS.get(fields.number::<usize>()?)?.clone(),
        "unreadable" => SkipReason::Unreadable(Errno(fields.number()?)),
        "blocked" => return blocked(fields),
        _ => return None,
    };
    fields.end().map(|()| reason)
}

fn blocked(fields: &mut Fields) -> Option<SkipReason> {
    let mut reasons = std::collections::BTreeSet::new();
    while let Some(field) = fields.bytes() {
        let field = String::from_utf8(field).ok()?;
        let reason = match field.strip_prefix('!') {
            Some(why) => Reason::InspectionFailed(why.to_owned()),
            None => REASONS.get(field.parse::<usize>().ok()?)?.clone(),
        };
        reasons.insert(reason);
    }
    match WorktreeStatus::new(None, reasons) {
        WorktreeStatus::Blocked { reasons, .. } => Some(SkipReason::Blocked(reasons)),
        WorktreeStatus::Removable { .. } => None,
    }
}

fn leftovers(fields: &mut Fields) -> Option<Leftovers> {
    let failures = fields.number()?;
    let cancelled = flag(fields)?;
    let first = failed_paths(fields)?;
    let mounts = std::iter::from_fn(|| fields.path()).collect();
    Some(Leftovers {
        failures,
        first,
        mounts,
        cancelled,
    })
}
