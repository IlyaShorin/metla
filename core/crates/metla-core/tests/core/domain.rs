use std::path::PathBuf;

use metla_core::domain::{
    Bytes, Coverage, EntryKind, EntryMeta, Errno, Gap, GapAt, GapKind, OwnTotals, Skip, TreeTotals,
    UnixTime,
};

fn meta(kind: EntryKind, blocks: u64, links: u64) -> EntryMeta {
    EntryMeta {
        kind,
        blocks,
        links: std::num::NonZeroU64::new(links).unwrap(),
        device: 1,
        inode: 2,
        mtime: UnixTime(100),
        dataless: false,
    }
}

#[test]
fn a_file_contributes_its_allocated_blocks() {
    assert_eq!(meta(EntryKind::File, 8, 1).share(), Bytes(4096));
}

#[test]
fn a_hard_linked_file_contributes_its_share_per_link() {
    assert_eq!(meta(EntryKind::File, 8, 2).share(), Bytes(2048));
}

#[test]
fn a_symlink_counts_as_itself() {
    assert_eq!(meta(EntryKind::Symlink, 1, 1).share(), Bytes(512));
}

#[test]
fn a_directory_inode_contributes_nothing_itself() {
    assert_eq!(meta(EntryKind::Dir, 8, 3).share(), Bytes(0));
}

#[test]
fn own_totals_count_every_non_directory_entry() {
    let mut own = OwnTotals::default();
    own.add(&meta(EntryKind::File, 8, 1));
    own.add(&meta(EntryKind::Other, 2, 1));
    assert_eq!(
        own,
        OwnTotals {
            bytes: Bytes(5120),
            files: 2
        }
    );
}

#[test]
fn a_directory_tree_starts_with_itself() {
    let tree = TreeTotals::directory(UnixTime(7));
    assert_eq!(
        tree,
        TreeTotals {
            bytes: Bytes(0),
            files: 0,
            dirs: 1,
            newest: Some(UnixTime(7))
        }
    );
}

#[test]
fn tree_totals_add_own_totals_and_keep_the_newest_time() {
    let mut tree = TreeTotals::directory(UnixTime(7));
    let own = OwnTotals {
        bytes: Bytes(10),
        files: 3,
    };
    tree.add_own(&own, Some(UnixTime(9)));
    tree.add_own(&own, Some(UnixTime(8)));
    assert_eq!(
        tree,
        TreeTotals {
            bytes: Bytes(20),
            files: 6,
            dirs: 1,
            newest: Some(UnixTime(9))
        }
    );
}

#[test]
fn tree_totals_merge_a_subtree() {
    let mut tree = TreeTotals::directory(UnixTime(9));
    let child = TreeTotals {
        bytes: Bytes(5),
        files: 2,
        dirs: 3,
        newest: Some(UnixTime(4)),
    };
    tree.add_tree(&child);
    tree.add_tree(&child);
    assert_eq!(
        tree,
        TreeTotals {
            bytes: Bytes(10),
            files: 4,
            dirs: 7,
            newest: Some(UnixTime(9))
        }
    );
}

#[test]
fn bytes_add_up() {
    let mut total = Bytes(1) + Bytes(2);
    total += Bytes(4);
    assert_eq!(total, Bytes(7));
    assert_eq!([Bytes(3), Bytes(5)].into_iter().sum::<Bytes>(), Bytes(8));
}

#[test]
fn each_gap_has_its_kind() {
    assert_eq!(Gap::Denied.kind(), GapKind::Denied);
    assert_eq!(Gap::Failed(Errno(5)).kind(), GapKind::Failed);
    assert_eq!(Gap::Vanished.kind(), GapKind::Vanished);
    assert_eq!(Gap::Cancelled.kind(), GapKind::Cancelled);
}

fn gap_at(path: &str, depth: u32, gap: Gap) -> GapAt {
    GapAt {
        path: PathBuf::from(path),
        depth,
        gap,
    }
}

#[test]
fn coverage_without_gaps_is_complete_even_with_skips() {
    let mut coverage = Coverage::new(4);
    coverage.record_skip(Skip::OtherVolume);
    coverage.record_skip(Skip::OtherVolume);
    coverage.record_skip(Skip::Dataless);
    assert!(coverage.is_complete());
    assert_eq!(coverage.skips(Skip::OtherVolume), 2);
    assert_eq!(coverage.skips(Skip::Dataless), 1);
    assert_eq!(coverage.skips(Skip::Excluded), 0);
    assert_eq!(coverage.skip_total(), 3);
    assert_eq!(coverage.gap_total(), 0);
}

#[test]
fn any_gap_makes_coverage_partial_and_is_counted_by_kind() {
    let mut coverage = Coverage::new(4);
    coverage.record_gap(gap_at("/a", 1, Gap::Denied));
    coverage.record_gap(gap_at("/b", 1, Gap::Denied));
    coverage.record_gap(gap_at("/c", 1, Gap::Failed(Errno(5))));
    assert!(!coverage.is_complete());
    assert_eq!(coverage.gaps(GapKind::Denied), 2);
    assert_eq!(coverage.gaps(GapKind::Failed), 1);
    assert_eq!(coverage.gaps(GapKind::Vanished), 0);
    assert_eq!(coverage.gap_total(), 3);
}

#[test]
fn coverage_keeps_only_the_shallowest_gaps_in_path_order() {
    let mut coverage = Coverage::new(2);
    coverage.record_gap(gap_at("/z/deep", 3, Gap::Vanished));
    coverage.record_gap(gap_at("/b", 1, Gap::Denied));
    coverage.record_gap(gap_at("/a", 1, Gap::Cancelled));
    let paths: Vec<_> = coverage.top_gaps().iter().map(|g| g.path.clone()).collect();
    assert_eq!(paths, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(coverage.gap_total(), 3);
}

#[test]
fn coverage_can_be_restored_from_its_parts() {
    let mut original = Coverage::new(3);
    original.record_gap(gap_at("/a", 1, Gap::Denied));
    original.record_skip(Skip::MountPoint);
    let restored = Coverage::restore(
        3,
        original.gap_counts().collect(),
        original.skip_counts().collect(),
        original.top_gaps().to_vec(),
    );
    assert_eq!(restored, original);
}
