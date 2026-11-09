//! The dependency rule of docs/ARCHITECTURE.md §2, checked on the source.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every module of metla-core and the modules it may use. One table; a
/// module that is not listed may not exist.
const ALLOWED: &[(&str, &[&str])] = &[
    ("domain", &[]),
    ("rules", &["domain"]),
    ("fs", &["domain"]),
    ("git", &["domain"]),
    ("index", &["domain"]),
    ("scan", &["domain", "fs", "rules"]),
    ("watch", &["domain"]),
    ("clean", &["domain", "fs", "rules", "git"]),
    (
        "engine",
        &[
            "domain", "rules", "fs", "git", "index", "scan", "watch", "clean",
        ],
    ),
];

fn allowed(module: &str) -> Option<&'static [&'static str]> {
    ALLOWED
        .iter()
        .find(|(name, _)| *name == module)
        .map(|(_, uses)| *uses)
}

/// The top-level modules referred to by `crate::...` or by enough `super::`
/// to climb out of the file's own top-level module. `depth` is how many
/// `super` steps lead from the file to the crate root.
fn referenced_modules(source: &str, depth: usize) -> BTreeSet<String> {
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let mut found = BTreeSet::new();
    for (at, _) in code.match_indices("crate::") {
        found.extend(heads(&code[at + "crate::".len()..]));
    }
    let climb = "super::".repeat(depth);
    if depth > 0 {
        for (at, _) in code.match_indices(&climb) {
            let rest = &code[at + climb.len()..];
            if !rest.starts_with("super::") {
                found.extend(heads(rest));
            }
        }
    }
    found
}

/// The first identifier of a path, or of each item of a `{...}` group.
fn heads(rest: &str) -> Vec<String> {
    match rest.strip_prefix('{') {
        Some(group) => {
            let mut depth = 0usize;
            let mut items = vec![String::new()];
            for c in group.chars() {
                match c {
                    '{' => depth += 1,
                    '}' if depth == 0 => break,
                    '}' => depth -= 1,
                    ',' if depth == 0 => items.push(String::new()),
                    _ => {}
                }
                if depth == 0 && c != ',' {
                    items.last_mut().unwrap().push(c);
                }
            }
            items
                .iter()
                .filter_map(|item| ident(item.trim_start()))
                .collect()
        }
        None => ident(rest).into_iter().collect(),
    }
}

fn ident(text: &str) -> Option<String> {
    let name: String = text
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    files
}

/// `super` steps from a file to the crate root: `src/scan/mod.rs` is the
/// module `scan` (1), `src/scan/walk.rs` is `scan::walk` (2).
fn depth_below_root(src: &Path, file: &Path) -> usize {
    let parts = file.strip_prefix(src).unwrap().components().count();
    match file.file_name().unwrap().to_str().unwrap() {
        "mod.rs" => parts - 1,
        _ => parts,
    }
}

#[test]
fn modules_only_use_the_modules_the_dependency_rule_allows() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in fs::read_dir(&src).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let module = dir.file_name().unwrap().to_str().unwrap().to_owned();
        let uses = allowed(&module)
            .unwrap_or_else(|| panic!("module `{module}` is not in the dependency table"));
        for file in rust_files(&dir) {
            let source = fs::read_to_string(&file).unwrap();
            for used in referenced_modules(&source, depth_below_root(&src, &file)) {
                assert!(
                    used == module || uses.contains(&used.as_str()),
                    "{} uses `{used}`, but `{module}` may only use {uses:?}",
                    file.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 15, "only {checked} source files were checked");
}

#[test]
fn the_reference_parser_sees_every_form_of_module_path() {
    let source = "use crate::domain::Bytes;\n\
                  use crate::{fs::DirHandle, index::{IndexReader, Snapshot}};\n\
                  fn f() { crate::scan::run(); super::super::rules::x(); }\n\
                  // use crate::engine::Hidden;\n";
    let found: Vec<String> = referenced_modules(source, 2).into_iter().collect();
    assert_eq!(found, ["domain", "fs", "index", "rules", "scan"]);
    assert!(referenced_modules("use super::control::X;", 2).is_empty());
}
