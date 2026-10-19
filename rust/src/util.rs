//! Small formatting helpers shared by the report writers and the CLI.

use std::path::Path;

use chrono::{DateTime, Utc};

/// File name of a path as text, for messages that name a file. Empty when the path has no file name.
pub fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// ISO-8601 UTC with a Z suffix, e.g. 2026-10-19T10:00:00Z (matches the .NET IsoTime.Format).
pub fn iso(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn iso_opt(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(iso)
}

/// Thousands-separated integer, matching .NET "N0" formatting in the invariant culture.
pub fn thousands(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if value < 0 { format!("-{out}") } else { out }
}

/// Byte count with 1024-based units, matching PdfReportWriter.FormatBytes.
pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{} B", thousands(bytes)) } else { format!("{} {}", one_decimal(value), UNITS[unit]) }
}

/// One decimal place, as .NET "0.0" shows it: halves round away from zero. Rounds at 15 decimal places first, so
/// 1.15 (stored as 1.1499999...) gives 1.2 and 1.25 gives 1.3, which Rust's "{:.1}" would not.
pub fn one_decimal(value: f64) -> String {
    let fixed = format!("{:.15}", value.abs());
    let (whole, fraction) = fixed.split_once('.').unwrap_or((fixed.as_str(), ""));
    let digit = |index: usize| u64::from(fraction.as_bytes().get(index).map_or(0, |byte| byte - b'0'));
    let tenths = digit(0) + u64::from(digit(1) >= 5);
    let whole: u64 = whole.parse().unwrap_or(0);
    let sign = if value < 0.0 { "-" } else { "" };
    format!("{sign}{}.{}", whole + tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_decimal_rounds_halves_away_from_zero_like_dotnet() {
        // Values recorded from .NET ToString("0.0") and PdfReportWriter.FormatBytes.
        assert_eq!(one_decimal(1.25), "1.3");
        assert_eq!(one_decimal(1.15), "1.2");
        assert_eq!(one_decimal(0.35), "0.4");
        assert_eq!(one_decimal(2.75), "2.8");
        assert_eq!(one_decimal(8760.25), "8760.3");
        assert_eq!(one_decimal(1.0 / 3.0), "0.3");
        assert_eq!(one_decimal(0.0), "0.0");
        assert_eq!(one_decimal(-1.25), "-1.3");
    }

    #[test]
    fn format_bytes_matches_dotnet_units_and_rounding() {
        assert_eq!(format_bytes(1000), "1,000 B");
        assert_eq!(format_bytes(1280), "1.3 KB");
        assert_eq!(format_bytes(1408), "1.4 KB");
        assert_eq!(format_bytes(1310720), "1.3 MB");
    }
}
