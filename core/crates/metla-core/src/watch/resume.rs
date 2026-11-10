//! Where a restarted stream resumes.

/// The event id to resume after: the stored one, when it was recorded in
/// this volume's event history (the same UUID) and that history has not
/// been reset since (the stored id is not ahead of the current one).
/// `None`: the history is lost, only a full scan is right.
pub fn resume_after(stored: Option<(u64, &str)>, uuid: Option<&str>, current: u64) -> Option<u64> {
    let (id, recorded) = stored?;
    let same = uuid == Some(recorded);
    (same && id <= current).then_some(id)
}
