//! Location patterns as written in the catalogue, turned into components.
//!
//! `~` is the home directory, `<user>` the user's name, `*` exactly one
//! component, `{a,b}` alternatives (not nested; they may contain `/`), and
//! `name[a|b]` a component whose directory must contain `a` or `b`.

/// One component of a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// `None` for `*`.
    pub name: Option<Vec<u8>>,
    /// The directory at this step must contain one of these entries.
    pub requires: Vec<Vec<u8>>,
}

/// Every pattern a pattern with `{a,b}` alternatives stands for, in order.
pub fn expand(pattern: &str) -> Vec<String> {
    let group = pattern
        .find('{')
        .and_then(|open| Some((open, open + pattern[open..].find('}')?)));
    let Some((open, close)) = group else {
        return vec![pattern.to_owned()];
    };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    let alternatives = pattern[open + 1..close].split(',');
    alternatives
        .flat_map(|alternative| expand(&format!("{head}{alternative}{tail}")))
        .collect()
}

/// The components of an expanded pattern from the file-system root.
pub fn steps(pattern: &str, home: &[u8], user: &[u8]) -> Vec<Step> {
    let pattern = replace(pattern.as_bytes(), b"<user>", user);
    let (anchor, rest) = match pattern.strip_prefix(b"~") {
        Some(rest) => (home, rest),
        None => (&b""[..], &pattern[..]),
    };
    components(anchor)
        .chain(components(rest))
        .map(step)
        .collect()
}

fn components(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.split(|b| *b == b'/').filter(|part| !part.is_empty())
}

fn step(component: &[u8]) -> Step {
    let condition = component
        .strip_suffix(b"]")
        .and_then(|c| c.iter().position(|b| *b == b'[').map(|at| c.split_at(at)));
    match condition {
        Some((name, requires)) => Step {
            name: literal(name),
            requires: requires[1..]
                .split(|b| *b == b'|')
                .map(<[u8]>::to_vec)
                .collect(),
        },
        None => Step {
            name: literal(component),
            requires: Vec::new(),
        },
    }
}

fn literal(name: &[u8]) -> Option<Vec<u8>> {
    (name != b"*").then(|| name.to_vec())
}

fn replace(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    match text.windows(from.len()).position(|window| window == from) {
        Some(at) => [
            &text[..at],
            to,
            &replace(&text[at + from.len()..], from, to),
        ]
        .concat(),
        None => text.to_vec(),
    }
}
