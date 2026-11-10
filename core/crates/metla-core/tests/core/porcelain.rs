//! Parsing what Git prints, from captured output.

use std::path::PathBuf;

use metla_core::git::{Listed, Status, Unparsable, count, fields, status, worktree_list};

#[test]
fn nul_separated_output_splits_into_fields() {
    assert_eq!(fields(b"a\0b c\0\0d\0"), [&b"a"[..], b"b c", b"", b"d"]);
    assert_eq!(fields(b"a"), [&b"a"[..]]);
    assert!(fields(b"").is_empty());
}

#[test]
fn the_worktree_list_names_every_worktree_with_its_lock_and_prune_state() {
    let output = b"worktree /r\0HEAD 1234\0branch refs/heads/main\0\0\
worktree /w 1\0HEAD 1234\0detached\0locked on a\nshared drive\0\0\
worktree /w2\0HEAD 1234\0branch refs/heads/x\0prunable gitdir file points to non-existent location\0\0\
worktree /w3\0HEAD 1234\0locked\0\0";
    let listed = worktree_list(output).unwrap();
    let entry = |path: &str, locked, prunable| Listed {
        path: PathBuf::from(path),
        locked,
        prunable,
    };
    assert_eq!(
        listed,
        [
            entry("/r", false, false),
            entry("/w 1", true, false),
            entry("/w2", false, true),
            entry("/w3", true, false),
        ]
    );
    assert_eq!(worktree_list(b"").unwrap(), []);
}

#[test]
fn a_worktree_list_that_does_not_start_with_a_worktree_is_unparsable() {
    let Unparsable(what) = worktree_list(b"HEAD 1234\0\0").unwrap_err();
    assert_eq!(what, "worktree list: HEAD 1234");
    assert!(
        worktree_list(b"worktree /r\0\0locked\0").is_err(),
        "outside a record"
    );
}

#[test]
fn status_counts_staged_modified_conflicted_and_untracked_entries() {
    let output = b"# branch.oid 9102\0# branch.head feature\0\
# branch.upstream origin/feature\0# branch.ab +2 -5\0\
1 .M N... 100644 100644 100644 a a README\0\
1 A. N... 000000 100644 100644 0 b new.txt\0\
1 MM N... 100644 100644 100644 a b both.txt\0\
2 R. N... 100644 100644 100644 a a R100 moved.txt\x001 .M looks like an entry\0\
u UU N... 100644 100644 100644 100644 a b c conflict.txt\0\
? notes.txt\0? 1 .M named like an entry\0! ignored.log\0";
    assert_eq!(
        status(output).unwrap(),
        Status {
            branch: Some("feature".into()),
            upstream: Some("origin/feature".into()),
            ahead: Some(2),
            staged: 3,
            modified: 2,
            conflicted: 1,
            untracked: 2,
        }
    );
}

#[test]
fn a_detached_head_and_a_branch_without_upstream_are_told_apart() {
    let detached = status(b"# branch.oid 9102\0# branch.head (detached)\0").unwrap();
    assert_eq!(detached, Status::default());
    let gone =
        status(b"# branch.head x\0# branch.upstream origin/x\0# branch.unknown y\0").unwrap();
    assert_eq!(
        (gone.branch.as_deref(), gone.upstream.as_deref(), gone.ahead),
        (Some("x"), Some("origin/x"), None),
        "an upstream that is gone has no ahead count"
    );
}

#[test]
fn status_output_of_an_unknown_shape_is_unparsable() {
    for (output, said) in [
        (&b"x what\0"[..], "status entry: x what"),
        (b"# branch.ab 2 -1\0", "ab: 2 -1"),
        (b"# branch.ab +two -1\0", "ab: +two -1"),
        (b"# branch.ab\0", "ab: "),
    ] {
        assert_eq!(status(output), Err(Unparsable(said.into())));
    }
}

#[test]
fn a_count_is_one_number_on_a_line() {
    assert_eq!(count(b"42\n"), Ok(42));
    assert_eq!(count(b"0"), Ok(0));
    assert_eq!(count(b"x\n"), Err(Unparsable("count: x\n".into())));
}
