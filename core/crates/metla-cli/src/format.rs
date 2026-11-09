//! Human-readable numbers. The only place the CLI formats sizes and counts.

use std::time::Duration;

use metla_core::domain::Bytes;

const UNITS: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];

/// Decimal units, as Finder shows them: `999 B`, `1.0 kB`, `12.3 GB`.
pub fn bytes(size: Bytes) -> String {
    let mut value = size.0 as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    match unit {
        0 => format!("{} B", size.0),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}

/// A size that may be negative: `-1.2 GB`.
pub fn signed_bytes(size: i128) -> String {
    let magnitude = bytes(Bytes(
        u64::try_from(size.unsigned_abs()).unwrap_or(u64::MAX),
    ));
    match size < 0 {
        true => format!("-{magnitude}"),
        false => magnitude,
    }
}

/// Thousands separated by commas: `1,234,567`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// How many per second.
pub fn per_second(n: u64, elapsed: Duration) -> u64 {
    (n as f64 / elapsed.as_secs_f64()) as u64
}

pub fn seconds(elapsed: Duration) -> String {
    format!("{:.1} s", elapsed.as_secs_f64())
}

/// Roughly how long `seconds` is: `45 s`, `12 min`, `5 h`, `3 d`. A time
/// in the future reads as `0 s`.
pub fn ago(seconds: i64) -> String {
    const UNITS: [(i64, &str); 3] = [(86_400, "d"), (3_600, "h"), (60, "min")];
    let seconds = seconds.max(0);
    match UNITS.iter().find(|(unit, _)| seconds >= *unit) {
        Some((unit, name)) => format!("{} {name}", seconds / unit),
        None => format!("{seconds} s"),
    }
}
