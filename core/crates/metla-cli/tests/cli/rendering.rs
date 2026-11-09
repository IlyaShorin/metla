use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use metla_cli::{
    Listed, ScanReport, VolumeUse, bytes, count, default_threads, json_string, ls_text, per_second,
    scan_json, scan_text, signed_bytes, top_text,
};
use metla_core::domain::{
    Bytes, Coverage, DirState, Errno, Gap, GapAt, OwnTotals, ScanSummary, Skip, TreeTotals,
    UnixTime,
};
use metla_core::fs::Volume;

#[test]
fn sizes_use_decimal_units() {
    let cases = [
        (0, "0 B"),
        (999, "999 B"),
        (1_000, "1.0 kB"),
        (1_500, "1.5 kB"),
        (999_949, "999.9 kB"),
        (12_345_678, "12.3 MB"),
        (4_200_000_000, "4.2 GB"),
        (7_000_000_000_000, "7.0 TB"),
        (3_000_000_000_000_000, "3.0 PB"),
        (5_000_000_000_000_000_000, "5000.0 PB"),
    ];
    for (size, text) in cases {
        assert_eq!(bytes(Bytes(size)), text);
    }
}

#[test]
fn negative_sizes_keep_their_sign() {
    assert_eq!(signed_bytes(-1_500), "-1.5 kB");
    assert_eq!(signed_bytes(0), "0 B");
    assert_eq!(signed_bytes(2_000), "2.0 kB");
}

#[test]
fn counts_are_grouped_by_thousands() {
    let cases = [
        (0, "0"),
        (999, "999"),
        (1_000, "1,000"),
        (123_456, "123,456"),
        (1_234_567, "1,234,567"),
    ];
    for (n, text) in cases {
        assert_eq!(count(n), text);
    }
}

#[test]
fn rates_are_per_second() {
    assert_eq!(per_second(1_000, Duration::from_secs(4)), 250);
    assert_eq!(per_second(10, Duration::from_millis(100)), 100);
}

#[test]
fn all_but_two_cores_scan_by_default() {
    assert_eq!(default_threads(12), 10);
    assert_eq!(default_threads(2), 1);
    assert_eq!(default_threads(1), 1);
    assert_eq!(default_threads(0), 1);
}

fn volume(mount: &str) -> Volume {
    Volume {
        total: Bytes(10_000),
        available: Bytes(3_000),
        mount_point: PathBuf::from(mount),
        device: 1,
    }
}

#[test]
fn the_volume_is_shown_only_for_its_own_root() {
    let used = VolumeUse::of(&volume("/v"), Path::new("/v")).unwrap();
    assert_eq!(used.used(), Bytes(7_000));
    assert_eq!(used.unaccounted(Bytes(6_500)), 500);
    assert_eq!(used.unaccounted(Bytes(7_200)), -200);
    assert_eq!(VolumeUse::of(&volume("/v"), Path::new("/v/sub")), None);
}

fn report(volume: Option<VolumeUse>) -> ScanReport {
    let mut coverage = Coverage::new(5);
    coverage.record_gap(GapAt {
        depth: 1,
        path: PathBuf::from("/r/locked"),
        gap: Gap::Denied,
    });
    coverage.record_gap(GapAt {
        depth: 2,
        path: PathBuf::from("/r/a/\"odd\""),
        gap: Gap::Failed(Errno(5)),
    });
    coverage.record_skip(Skip::OtherVolume);
    ScanReport {
        summary: ScanSummary {
            root: PathBuf::from("/r"),
            started: UnixTime(1),
            finished: UnixTime(3),
            elapsed: Duration::from_secs(2),
            threads: 4,
            totals: TreeTotals {
                bytes: Bytes(6_500),
                files: 3_000,
                dirs: 1_200,
                newest: None,
            },
            coverage,
        },
        peak_rss: Bytes(25_000_000),
        index_size: Bytes(4_096),
        volume,
    }
}

#[test]
fn the_scan_report_reads_as_text() {
    let used = VolumeUse::of(&volume("/r"), Path::new("/r"));
    let text = scan_text(&report(used));
    let expected = "\
root         /r
directories  1,200
files        3,000
size         6.5 kB allocated
elapsed      2.0 s
throughput   600 dirs/s, 1,500 files/s
coverage     partial
gaps         1 denied, 1 failed, 0 vanished, 0 cancelled
skipped      1 other volume, 0 mount point, 0 dataless, 0 excluded
  gap        denied  /r/locked
  gap        failed (errno 5)  /r/a/\"odd\"
volume       total 10.0 kB, available 3.0 kB, used 7.0 kB, unaccounted 500 B
peak RSS     25.0 MB
threads      4
index        4.1 kB
";
    assert_eq!(text, expected);
    assert!(!scan_text(&report(None)).contains("\nvolume "));
}

#[test]
fn a_complete_scan_says_so() {
    let mut report = report(None);
    report.summary.coverage = Coverage::new(5);
    assert!(scan_text(&report).contains("coverage     complete\n"));
    assert!(scan_json(&report).contains("\"complete\":true"));
}

#[test]
fn the_scan_report_reads_as_json() {
    let used = VolumeUse::of(&volume("/r"), Path::new("/r"));
    let expected = concat!(
        "{\"root\":\"/r\",\"directories\":1200,\"files\":3000,\"bytes\":6500,",
        "\"elapsed_seconds\":2.000,\"dirs_per_second\":600,\"files_per_second\":1500,",
        "\"threads\":4,\"complete\":false,",
        "\"gaps\":{\"denied\":1,\"failed\":1,\"vanished\":0,\"cancelled\":0},",
        "\"skipped\":{\"other volume\":1,\"mount point\":0,\"dataless\":0,\"excluded\":0},",
        "\"top_gaps\":[{\"gap\":\"denied\",\"depth\":1,\"path\":\"/r/locked\"},",
        "{\"gap\":\"failed (errno 5)\",\"depth\":2,\"path\":\"/r/a/\\\"odd\\\"\"}],",
        "\"peak_rss_bytes\":25000000,\"index_bytes\":4096,",
        "\"volume\":{\"total\":10000,\"available\":3000,\"used\":7000,\"unaccounted\":500}}"
    );
    assert_eq!(scan_json(&report(used)), expected);
    assert!(scan_json(&report(None)).ends_with("\"volume\":null}"));
}

#[test]
fn json_strings_are_escaped() {
    assert_eq!(
        json_string("a\"b\\c\nd\u{1}"),
        "\"a\\\"b\\\\c\\u000ad\\u0001\""
    );
}

#[test]
fn top_lists_size_and_path() {
    let rows = [
        (Bytes(2_000_000), PathBuf::from("/r/a")),
        (Bytes(5), PathBuf::from("/r/b")),
    ];
    assert_eq!(top_text(&rows), "    2.0 MB  /r/a\n       5 B  /r/b\n");
}

#[test]
fn ls_marks_directories_and_their_state() {
    let row = |size, name: &str, dir| Listed {
        size: Bytes(size),
        name: OsString::from(name),
        dir,
    };
    let read = DirState::Read(OwnTotals::default());
    let rows = [
        row(3_000, "src", Some(read)),
        row(1_000, "Cargo.toml", None),
        row(0, "locked", Some(DirState::Gap(Gap::Denied))),
        row(0, "cloud", Some(DirState::Skipped(Skip::Dataless))),
        row(0, "gone", Some(DirState::Gap(Gap::Vanished))),
        row(0, "later", Some(DirState::Gap(Gap::Cancelled))),
        row(0, "image", Some(DirState::Skipped(Skip::MountPoint))),
        row(0, "magic", Some(DirState::Skipped(Skip::Excluded))),
    ];
    let expected = concat!(
        "    3.0 kB  dir   src/\n",
        "    1.0 kB  file  Cargo.toml\n",
        "       0 B  dir   locked/  [denied]\n",
        "       0 B  dir   cloud/  [skipped: dataless]\n",
        "       0 B  dir   gone/  [vanished]\n",
        "       0 B  dir   later/  [cancelled]\n",
        "       0 B  dir   image/  [skipped: mount point]\n",
        "       0 B  dir   magic/  [skipped: excluded]\n",
    );
    assert_eq!(ls_text(&rows), expected);
}
