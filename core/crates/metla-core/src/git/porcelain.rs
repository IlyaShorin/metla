//! What Git prints, parsed. Pure: bytes in, values out. Every format is
//! the NUL-separated (`-z`) one, so no name needs unquoting.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// Output Git printed that does not have the expected shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unparsable(pub String);

/// One entry of `git worktree list --porcelain -z`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    pub locked: bool,
    pub prunable: bool,
}

/// The fields of a NUL-separated output, without the empty last one.
pub fn fields(output: &[u8]) -> Vec<&[u8]> {
    let mut fields: Vec<&[u8]> = output.split(|b| *b == 0).collect();
    if fields.last().is_some_and(|last| last.is_empty()) {
        fields.pop();
    }
    fields
}

/// The worktrees of a repository, the main one first. Records are
/// separated by an empty field; each starts with `worktree <path>`.
pub fn worktree_list(output: &[u8]) -> Result<Vec<Listed>, Unparsable> {
    let mut listed: Vec<Listed> = Vec::new();
    let mut open = false;
    for field in fields(output) {
        let (key, value) = split_once(field);
        match (open, key) {
            (_, b"") => open = false,
            (false, b"worktree") => {
                listed.push(Listed {
                    path: PathBuf::from(OsString::from_vec(value.to_vec())),
                    ..Listed::default()
                });
                open = true;
            }
            (true, b"locked") => last(&mut listed).locked = true,
            (true, b"prunable") => last(&mut listed).prunable = true,
            (true, _) => {}
            (false, _) => return Err(unparsable("worktree list", field)),
        }
    }
    Ok(listed)
}

fn last(listed: &mut [Listed]) -> &mut Listed {
    let at = listed.len() - 1;
    &mut listed[at]
}

/// What `git status --porcelain=v2 --branch -z` says about a worktree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` for a detached HEAD.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    /// Commits ahead of the upstream; `None` without one (or when it is
    /// gone).
    pub ahead: Option<u64>,
    pub staged: u64,
    pub modified: u64,
    pub conflicted: u64,
    pub untracked: u64,
}

pub fn status(output: &[u8]) -> Result<Status, Unparsable> {
    let mut status = Status::default();
    let mut fields = fields(output).into_iter();
    while let Some(field) = fields.next() {
        // A rename or copy is followed by its original path.
        if entry(&mut status, field)? == Followed::ByOriginalPath {
            fields.next();
        }
    }
    Ok(status)
}

/// Whether a status entry takes the next field with it.
#[derive(PartialEq, Eq)]
enum Followed {
    Alone,
    ByOriginalPath,
}

/// Counts one status entry.
fn entry(status: &mut Status, field: &[u8]) -> Result<Followed, Unparsable> {
    let (kind, rest) = split_once(field);
    match kind {
        b"#" => header(status, rest)?,
        b"1" => changed(status, rest),
        b"2" => {
            changed(status, rest);
            return Ok(Followed::ByOriginalPath);
        }
        b"u" => status.conflicted += 1,
        b"?" => status.untracked += 1,
        b"!" => {}
        _ => return Err(unparsable("status entry", field)),
    }
    Ok(Followed::Alone)
}

fn header(status: &mut Status, header: &[u8]) -> Result<(), Unparsable> {
    let (name, value) = split_once(header);
    let text = String::from_utf8_lossy(value).into_owned();
    match name {
        b"branch.head" => status.branch = (text != "(detached)").then_some(text),
        b"branch.upstream" => status.upstream = Some(text),
        b"branch.ab" => status.ahead = Some(ahead(&text).ok_or_else(|| unparsable("ab", value))?),
        _ => {}
    }
    Ok(())
}

/// `+<ahead> -<behind>`.
fn ahead(ab: &str) -> Option<u64> {
    ab.split(' ').next()?.strip_prefix('+')?.parse().ok()
}

/// An ordinary or renamed entry: `XY` says staged (`X`) and modified
/// (`Y`); `.` means unchanged.
fn changed(status: &mut Status, entry: &[u8]) {
    let xy = entry.get(..2).unwrap_or_default();
    status.staged += u64::from(xy.first().is_some_and(|x| *x != b'.'));
    status.modified += u64::from(xy.get(1).is_some_and(|y| *y != b'.'));
}

/// A number Git printed on one line, such as `rev-list --count`.
pub fn count(output: &[u8]) -> Result<u64, Unparsable> {
    let text = String::from_utf8_lossy(output);
    text.trim().parse().map_err(|_| unparsable("count", output))
}

fn split_once(field: &[u8]) -> (&[u8], &[u8]) {
    match field.iter().position(|b| *b == b' ') {
        Some(at) => (&field[..at], &field[at + 1..]),
        None => (field, &[]),
    }
}

fn unparsable(what: &str, bytes: &[u8]) -> Unparsable {
    Unparsable(format!("{what}: {}", String::from_utf8_lossy(bytes)))
}
