//! Small formatting helpers shared by the report writers and the CLI.

use chrono::{DateTime, Utc};

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
    if unit == 0 { format!("{} B", thousands(bytes)) } else { format!("{:.1} {}", value, UNITS[unit]) }
}
