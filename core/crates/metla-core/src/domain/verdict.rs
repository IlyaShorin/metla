/// What Git says about a directory of project output. Only `Verified`
/// output is offered; Git is asked again right before removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verdict {
    /// Ignored by Git, and nothing inside is tracked.
    Verified,
    /// Git does not ignore it.
    NotIgnored,
    /// Ignored, but Git tracks files inside it.
    HasTrackedFiles,
    /// Git could not answer: no repository, or Git failed.
    Unverifiable,
}

impl Verdict {
    /// Every verdict, in a fixed order (also the stored order).
    pub const ALL: [Verdict; 4] = [
        Verdict::Verified,
        Verdict::NotIgnored,
        Verdict::HasTrackedFiles,
        Verdict::Unverifiable,
    ];
}
