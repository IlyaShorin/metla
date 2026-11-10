//! What the commands print. Pure: data in, text out.

use std::ffi::OsString;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use std::time::Duration;

use metla_core::domain::{Bytes, Coverage, DirState, Gap, GapKind, ScanSummary, Skip, Verdict};
use metla_core::fs::Volume;

use crate::format::{bytes, count, per_second, seconds, signed_bytes};

/// Space on the volume whose root was scanned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeUse {
    pub total: Bytes,
    pub available: Bytes,
}

impl VolumeUse {
    /// The volume's figures, when `root` is where it is mounted.
    pub fn of(volume: &Volume, root: &Path) -> Option<VolumeUse> {
        (volume.mount_point == root).then_some(VolumeUse {
            total: volume.total,
            available: volume.available,
        })
    }

    /// Everything not available, as Finder counts used space.
    pub fn used(&self) -> Bytes {
        Bytes(self.total.0.saturating_sub(self.available.0))
    }

    /// Used space the scan did not find; negative when the scan counted
    /// more (clones share blocks).
    pub fn unaccounted(&self, scanned: Bytes) -> i128 {
        i128::from(self.used().0) - i128::from(scanned.0)
    }
}

/// Everything `scan` reports.
pub struct ScanReport {
    pub summary: ScanSummary,
    pub peak_rss: Bytes,
    pub index_size: Bytes,
    pub volume: Option<VolumeUse>,
    /// What Git said about the project output the rules found; `None`
    /// when the scan did not classify.
    pub rules: Option<Checked>,
}

/// What Git said about project output after a scan, and how long asking
/// took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// Directories per verdict, in the order of `Verdict::ALL`.
    pub verdicts: [u64; Verdict::ALL.len()],
    pub elapsed: Duration,
}

impl Checked {
    pub fn of(verdicts: &[Verdict], elapsed: Duration) -> Checked {
        let mut checked = Checked {
            elapsed,
            ..Checked::default()
        };
        for verdict in verdicts {
            checked.verdicts[*verdict as usize] += 1;
        }
        checked
    }
}

const VERDICT_NAMES: [&str; 4] = ["verified", "not ignored", "tracked inside", "unverifiable"];

pub(crate) const GAP_NAMES: [(GapKind, &str); 4] = [
    (GapKind::Denied, "denied"),
    (GapKind::Failed, "failed"),
    (GapKind::Vanished, "vanished"),
    (GapKind::Cancelled, "cancelled"),
];
pub(crate) const SKIP_NAMES: [(Skip, &str); 4] = [
    (Skip::OtherVolume, "other volume"),
    (Skip::MountPoint, "mount point"),
    (Skip::Dataless, "dataless"),
    (Skip::Excluded, "excluded"),
];

pub(crate) fn name_in<T: PartialEq>(names: &[(T, &'static str)], value: T) -> &'static str {
    names
        .iter()
        .find(|(known, _)| *known == value)
        .map_or("unknown", |(_, name)| name)
}

fn gap_name(kind: GapKind) -> &'static str {
    name_in(&GAP_NAMES, kind)
}

fn skip_name(skip: Skip) -> &'static str {
    name_in(&SKIP_NAMES, skip)
}

fn gap_label(gap: Gap) -> String {
    match gap {
        Gap::Failed(errno) => format!("failed (errno {})", errno.0),
        other => gap_name(other.kind()).to_owned(),
    }
}

pub(crate) fn completeness(coverage: &Coverage) -> &'static str {
    either(coverage.is_complete(), "complete", "partial")
}

pub(crate) fn line(out: &mut String, label: &str, value: impl std::fmt::Display) {
    let _ = writeln!(out, "{label:<12} {value}");
}

pub fn scan_text(report: &ScanReport) -> String {
    let s = &report.summary;
    let mut out = String::new();
    line(&mut out, "root", s.root.display());
    line(&mut out, "directories", count(s.totals.dirs));
    line(&mut out, "files", count(s.totals.files));
    line(
        &mut out,
        "size",
        format!("{} allocated", bytes(s.totals.bytes)),
    );
    line(&mut out, "elapsed", seconds(s.elapsed));
    let rate = |n| count(per_second(n, s.elapsed));
    let throughput = format!(
        "{} dirs/s, {} files/s",
        rate(s.totals.dirs),
        rate(s.totals.files)
    );
    line(&mut out, "throughput", throughput);
    coverage_text(&mut out, &s.coverage);
    if let Some(volume) = report.volume {
        let unaccounted = signed_bytes(volume.unaccounted(s.totals.bytes));
        let text = format!(
            "total {}, available {}, used {}, unaccounted {unaccounted}",
            bytes(volume.total),
            bytes(volume.available),
            bytes(volume.used())
        );
        line(&mut out, "volume", text);
    }
    line(&mut out, "peak RSS", bytes(report.peak_rss));
    line(&mut out, "threads", s.threads);
    line(
        &mut out,
        "rules",
        either(report.rules.is_some(), "on", "off"),
    );
    if let Some(checked) = &report.rules {
        line(&mut out, "git check", checked_text(checked));
    }
    line(&mut out, "index", bytes(report.index_size));
    out
}

fn checked_text(checked: &Checked) -> String {
    let counts = VERDICT_NAMES.iter().zip(checked.verdicts);
    let counts: Vec<String> = counts
        .map(|(name, n)| format!("{} {name}", count(n)))
        .collect();
    format!("{} in {}", counts.join(", "), seconds(checked.elapsed))
}

fn checked_json(checked: &Checked) -> String {
    let mut fields: Vec<(&str, String)> = VERDICT_NAMES
        .iter()
        .zip(checked.verdicts)
        .map(|(name, n)| (*name, n.to_string()))
        .collect();
    let elapsed = format!("{:.3}", checked.elapsed.as_secs_f64());
    fields.push(("elapsed_seconds", elapsed));
    json_object(&fields)
}

pub(crate) fn coverage_text(out: &mut String, coverage: &Coverage) {
    line(out, "coverage", completeness(coverage));
    let gaps: Vec<String> = GAP_NAMES
        .iter()
        .map(|(k, name)| format!("{} {name}", count(coverage.gaps(*k))))
        .collect();
    line(out, "gaps", gaps.join(", "));
    let skips: Vec<String> = SKIP_NAMES
        .iter()
        .map(|(k, name)| format!("{} {name}", count(coverage.skips(*k))))
        .collect();
    line(out, "skipped", skips.join(", "));
    for gap in coverage.top_gaps() {
        line(
            out,
            "  gap",
            format!("{}  {}", gap_label(gap.gap), gap.path.display()),
        );
    }
}

/// `yes` or `no`, as `flag` says.
pub(crate) fn either(flag: bool, yes: &'static str, no: &'static str) -> &'static str {
    match flag {
        true => yes,
        false => no,
    }
}

/// A JSON string literal.
pub fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn json_object(fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}:{value}", json_string(key)))
        .collect();
    format!("{{{}}}", body.join(","))
}

pub fn scan_json(report: &ScanReport) -> String {
    let s = &report.summary;
    let volume = report.volume.map_or("null".to_owned(), |v| {
        json_object(&[
            ("total", v.total.0.to_string()),
            ("available", v.available.0.to_string()),
            ("used", v.used().0.to_string()),
            ("unaccounted", v.unaccounted(s.totals.bytes).to_string()),
        ])
    });
    json_object(&[
        ("root", json_string(&s.root.to_string_lossy())),
        ("directories", s.totals.dirs.to_string()),
        ("files", s.totals.files.to_string()),
        ("bytes", s.totals.bytes.0.to_string()),
        ("elapsed_seconds", format!("{:.3}", s.elapsed.as_secs_f64())),
        (
            "dirs_per_second",
            per_second(s.totals.dirs, s.elapsed).to_string(),
        ),
        (
            "files_per_second",
            per_second(s.totals.files, s.elapsed).to_string(),
        ),
        ("threads", s.threads.to_string()),
        ("rules", report.rules.is_some().to_string()),
        (
            "git_check",
            report
                .rules
                .as_ref()
                .map_or("null".to_owned(), checked_json),
        ),
        ("complete", s.is_complete().to_string()),
        ("gaps", counts_json(&GAP_NAMES, |k| s.coverage.gaps(k))),
        ("skipped", counts_json(&SKIP_NAMES, |k| s.coverage.skips(k))),
        ("top_gaps", top_gaps_json(&s.coverage)),
        ("peak_rss_bytes", report.peak_rss.0.to_string()),
        ("index_bytes", report.index_size.0.to_string()),
        ("volume", volume),
    ])
}

pub(crate) fn counts_json<K: Copy>(
    names: &[(K, &'static str)],
    count: impl Fn(K) -> u64,
) -> String {
    let fields: Vec<(&str, String)> = names
        .iter()
        .map(|(k, name)| (*name, count(*k).to_string()))
        .collect();
    json_object(&fields)
}

pub(crate) fn top_gaps_json(coverage: &Coverage) -> String {
    let gaps: Vec<String> = coverage
        .top_gaps()
        .iter()
        .map(|g| {
            json_object(&[
                ("gap", json_string(&gap_label(g.gap))),
                ("depth", g.depth.to_string()),
                ("path", json_string(&g.path.to_string_lossy())),
            ])
        })
        .collect();
    format!("[{}]", gaps.join(","))
}

pub fn top_text(rows: &[(Bytes, PathBuf)]) -> String {
    let mut out = String::new();
    for (size, path) in rows {
        let _ = writeln!(out, "{:>10}  {}", bytes(*size), path.display());
    }
    out
}

/// One line of `ls`: a subdirectory from the index or a file listed live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub size: Bytes,
    pub name: OsString,
    /// `None` for a file.
    pub dir: Option<DirState>,
}

fn badge(state: DirState) -> String {
    match state {
        DirState::Read(_) => String::new(),
        DirState::Gap(gap) => format!("  [{}]", gap_label(gap)),
        DirState::Skipped(skip) => format!("  [skipped: {}]", skip_name(skip)),
    }
}

pub fn ls_text(rows: &[Listed]) -> String {
    let mut out = String::new();
    for row in rows {
        let (kind, slash, badge) = match row.dir {
            Some(state) => ("dir ", "/", badge(state)),
            None => ("file", "", String::new()),
        };
        let name = row.name.to_string_lossy();
        let _ = writeln!(out, "{:>10}  {kind}  {name}{slash}{badge}", bytes(row.size));
    }
    out
}
