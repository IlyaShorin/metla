//! Rendering of `candidates`, `summary` and `exclude` on synthetic data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use metla_cli::{
    Edited, Freshness, Overview, Shown, ago, candidates_json, candidates_text, edited_text,
    summary_json, summary_text,
};
use metla_core::domain::{
    Bytes, Candidate, Category, Coverage, DirId, Exclusion, FileKind, GapAt, KindTotals, RuleId,
    Tally, UnixTime,
};

const NOW: UnixTime = UnixTime(1_000_000);

#[test]
fn ages_are_rough_and_never_negative() {
    let cases = [
        (-5, "0 s"),
        (0, "0 s"),
        (59, "59 s"),
        (60, "1 min"),
        (3_599, "59 min"),
        (3_600, "1 h"),
        (86_399, "23 h"),
        (86_400, "1 d"),
        (400 * 86_400, "400 d"),
    ];
    for (seconds, text) in cases {
        assert_eq!(ago(seconds), text, "{seconds}");
    }
}

fn candidate(
    path: &str,
    rule: &str,
    bytes: u64,
    activity: Option<i64>,
    preselected: bool,
) -> Candidate {
    Candidate {
        id: DirId(7),
        path: PathBuf::from(path),
        rule: RuleId::parse(rule).unwrap(),
        bytes: Bytes(bytes),
        files: 3,
        activity: activity.map(|ago| UnixTime(NOW.0 - ago)),
        device: 1,
        inode: 2,
        managed: None,
        preselected,
    }
}

fn shown_candidates() -> Shown<Vec<Candidate>> {
    Shown {
        now: NOW,
        data: vec![
            candidate(
                "/r/target",
                "build.cargo-target",
                2_000_000,
                Some(3 * 86_400),
                true,
            ),
            candidate("/w", "worktree.linked", 500, Some(7_200), false),
            candidate("/h/.Trash", "temp.trash", 5, None, true),
        ],
    }
}

#[test]
fn candidates_read_as_size_activity_selection_rule_and_path() {
    let expected = concat!(
        "    2.0 MB      3 d  [x]  build.cargo-target              /r/target\n",
        "     500 B      2 h  [ ]  worktree.linked                 /w\n",
        "       5 B        -  [x]  temp.trash                      /h/.Trash\n",
    );
    assert_eq!(candidates_text(&shown_candidates()), expected);
}

#[test]
fn candidates_read_as_json() {
    let expected = concat!(
        "[{\"id\":7,\"path\":\"/r/target\",\"rule\":\"build.cargo-target\",",
        "\"category\":\"build\",\"disposal\":\"permanently\",\"bytes\":2000000,",
        "\"files\":3,\"activity\":740800,\"preselected\":true},",
        "{\"id\":7,\"path\":\"/w\",\"rule\":\"worktree.linked\",\"category\":\"worktree\",",
        "\"disposal\":\"git worktree remove\",\"bytes\":500,\"files\":3,",
        "\"activity\":992800,\"preselected\":false},",
        "{\"id\":7,\"path\":\"/h/.Trash\",\"rule\":\"temp.trash\",\"category\":\"temp\",",
        "\"disposal\":\"permanently\",\"bytes\":5,\"files\":3,\"activity\":null,",
        "\"preselected\":true}]"
    );
    assert_eq!(candidates_json(&shown_candidates()), expected);
    let empty = Shown {
        now: NOW,
        data: Vec::new(),
    };
    assert_eq!(candidates_json(&empty), "[]");
    assert_eq!(candidates_text(&empty), "");
}

fn tally(count: u64, bytes: u64) -> Tally {
    Tally {
        count,
        bytes: Bytes(bytes),
    }
}

fn overview(freshness: Option<Freshness>) -> Shown<Overview> {
    let rules: BTreeMap<RuleId, Tally> = [
        ("app-cache.library", tally(2, 3_000_000)),
        ("log.user-library", tally(1, 40_000)),
        ("temp.trash", tally(1, 5_000)),
    ]
    .into_iter()
    .map(|(id, t)| (RuleId::parse(id).unwrap(), t))
    .collect();
    let mut categories: BTreeMap<Category, Tally> =
        Category::ALL.map(|c| (c, Tally::default())).into();
    for (rule, t) in &rules {
        categories.get_mut(&rule.category()).unwrap().merge(t);
    }
    Shown {
        now: NOW,
        data: Overview {
            freshness,
            reclaimable: tally(3, 3_005_000),
            categories,
            rules,
        },
    }
}

fn freshness(current: bool) -> Freshness {
    let mut coverage = Coverage::new(2);
    coverage.record_gap(GapAt {
        depth: 1,
        path: PathBuf::from("/r/locked"),
        gap: metla_core::domain::Gap::Denied,
    });
    let mut kinds = KindTotals::default();
    kinds.add(FileKind::Media, Bytes(7_000_000));
    kinds.add(FileKind::System, Bytes(12_000));
    Freshness {
        root: PathBuf::from("/r"),
        finished: UnixTime(NOW.0 - 120),
        current,
        coverage,
        kinds,
    }
}

#[test]
fn the_summary_reads_as_text() {
    let expected = "\
picture      /r finished 2 min ago
coverage     partial
gaps         1 denied, 0 failed, 0 vanished, 0 cancelled
skipped      0 other volume, 0 mount point, 0 dataless, 0 excluded
  gap        denied  /r/locked
can be freed 3.0 MB in 3 preselected candidates (estimate)
  app-cache                         3.0 MB        2
    app-cache.library               3.0 MB        2  permanently
  dev-cache                            0 B        0
  build                                0 B        0
  log                              40.0 kB        1
    log.user-library               40.0 kB        1  to Trash
  worktree                             0 B        0
  temp                              5.0 kB        1
    temp.trash                      5.0 kB        1  permanently
data types
  Applications                         0 B
  Developer                            0 B
  Media                             7.0 MB
  Documents                            0 B
  Archives and disk images             0 B
  System                           12.0 kB
  Other                                0 B
";
    assert_eq!(summary_text(&overview(Some(freshness(true)))), expected);
}

#[test]
fn an_unfinished_later_scan_and_a_missing_picture_are_said() {
    let stale = summary_text(&overview(Some(freshness(false))));
    assert!(stale.contains("\n             a later scan started and did not finish\n"));
    let none = summary_text(&overview(None));
    assert!(none.starts_with("picture      none: no scan has finished\ncan be freed "));
    assert!(!none.contains("data types"));
}

#[test]
fn the_summary_reads_as_json() {
    let expected = concat!(
        "{\"picture\":{\"root\":\"/r\",\"finished\":999880,\"age_seconds\":120,",
        "\"current\":true,\"coverage\":\"partial\",",
        "\"gaps\":{\"denied\":1,\"failed\":0,\"vanished\":0,\"cancelled\":0},",
        "\"skipped\":{\"other volume\":0,\"mount point\":0,\"dataless\":0,\"excluded\":0},",
        "\"kinds\":{\"applications\":0,\"developer\":0,\"media\":7000000,\"documents\":0,",
        "\"archives\":0,\"system\":12000,\"other\":0}},",
        "\"reclaimable\":{\"count\":3,\"bytes\":3005000},",
        "\"categories\":[",
        "{\"count\":2,\"bytes\":3000000,\"category\":\"app-cache\",\"rules\":[",
        "{\"count\":2,\"bytes\":3000000,\"rule\":\"app-cache.library\",\"disposal\":\"permanently\"}]},",
        "{\"count\":0,\"bytes\":0,\"category\":\"dev-cache\",\"rules\":[]},",
        "{\"count\":0,\"bytes\":0,\"category\":\"build\",\"rules\":[]},",
        "{\"count\":1,\"bytes\":40000,\"category\":\"log\",\"rules\":[",
        "{\"count\":1,\"bytes\":40000,\"rule\":\"log.user-library\",\"disposal\":\"to Trash\"}]},",
        "{\"count\":0,\"bytes\":0,\"category\":\"worktree\",\"rules\":[]},",
        "{\"count\":1,\"bytes\":5000,\"category\":\"temp\",\"rules\":[",
        "{\"count\":1,\"bytes\":5000,\"rule\":\"temp.trash\",\"disposal\":\"permanently\"}]}]}"
    );
    assert_eq!(summary_json(&overview(Some(freshness(true)))), expected);
    assert!(summary_json(&overview(None)).starts_with("{\"picture\":null,"));
}

fn excluded(path: &str) -> Exclusion {
    Exclusion::new(Path::new(path)).unwrap()
}

#[test]
fn exclusion_edits_say_what_changed() {
    let cases = [
        (Edited::Added(excluded("/a"), true), "excluded /a\n"),
        (
            Edited::Added(excluded("/a"), false),
            "already excluded /a\n",
        ),
        (
            Edited::Removed(excluded("/a"), true),
            "no longer excluded /a\n",
        ),
        (
            Edited::Removed(excluded("/a"), false),
            "was not excluded /a\n",
        ),
        (
            Edited::Listed(vec![excluded("/a"), excluded("/b c")]),
            "/a\n/b c\n",
        ),
        (Edited::Listed(Vec::new()), ""),
    ];
    for (edited, text) in cases {
        assert_eq!(edited_text(&edited), text);
    }
}
