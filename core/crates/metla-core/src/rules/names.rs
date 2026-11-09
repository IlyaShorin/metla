//! Entry names and name patterns: `name`, `prefix*` and `*suffix`.

/// A name, or every name with a given prefix or suffix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamePattern {
    Exact(Vec<u8>),
    Prefix(Vec<u8>),
    Suffix(Vec<u8>),
}

impl NamePattern {
    pub fn parse(text: &str) -> NamePattern {
        let bytes = text.as_bytes();
        match (bytes.strip_prefix(b"*"), bytes.strip_suffix(b"*")) {
            (Some(suffix), _) => NamePattern::Suffix(suffix.to_vec()),
            (None, Some(prefix)) => NamePattern::Prefix(prefix.to_vec()),
            (None, None) => NamePattern::Exact(bytes.to_vec()),
        }
    }

    pub fn matches(&self, name: &[u8]) -> bool {
        match self {
            NamePattern::Exact(exact) => name == exact.as_slice(),
            NamePattern::Prefix(prefix) => name.starts_with(prefix),
            NamePattern::Suffix(suffix) => name.ends_with(suffix),
        }
    }
}

/// A set of name patterns. Exact names are looked up by binary search,
/// because the walker asks about every entry it lists.
#[derive(Clone, Debug, Default)]
pub struct Names {
    exact: Vec<Vec<u8>>,
    globs: Vec<NamePattern>,
}

impl Names {
    pub fn new(patterns: impl IntoIterator<Item = NamePattern>) -> Names {
        let mut names = Names::default();
        for pattern in patterns {
            match pattern {
                NamePattern::Exact(name) => names.exact.push(name),
                glob => names.globs.push(glob),
            }
        }
        names.exact.sort();
        names.exact.dedup();
        names
    }

    pub fn matches(&self, name: &[u8]) -> bool {
        self.exact
            .binary_search_by(|n| n.as_slice().cmp(name))
            .is_ok()
            || self.globs.iter().any(|glob| glob.matches(name))
    }
}
