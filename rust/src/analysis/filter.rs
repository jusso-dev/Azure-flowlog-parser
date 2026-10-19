use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use regex::Regex;

/// Per-observation filters. Port matches destination port; IP matches source or destination (case-insensitive).
#[derive(Debug, Clone)]
pub struct TrafficFilter {
    pub action: String,
    pub protocol: String,
    pub ports: String,
    pub ip: String,
    pub min_bytes: i64,
    pub port_ranges: Vec<(u32, u32)>,
}

impl TrafficFilter {
    /// action: all|allow|deny; protocol: all|tcp|udp. Errors on invalid input.
    pub fn create(
        action: Option<&str>,
        protocol: Option<&str>,
        ports: Option<&str>,
        ip: Option<&str>,
        min_bytes: i64,
    ) -> Result<Self> {
        let action_input = action.unwrap_or("all");
        let action = match action_input.trim().to_ascii_lowercase().as_str() {
            known @ ("all" | "allow" | "deny") => known.to_string(),
            _ => bail!("Invalid --action '{action_input}'. Use all, allow or deny."),
        };

        let protocol_input = protocol.unwrap_or("all");
        let protocol = match protocol_input.trim().to_ascii_lowercase().as_str() {
            "all" => "all",
            "tcp" => "TCP",
            "udp" => "UDP",
            _ => bail!("Invalid --protocol '{protocol_input}'. Use all, tcp or udp."),
        };

        if min_bytes < 0 {
            bail!("--min-bytes cannot be negative.");
        }

        let ports = ports.unwrap_or_default().trim().to_string();
        let port_ranges = parse_port_ranges(&ports)?;
        Ok(Self {
            action,
            protocol: protocol.to_string(),
            ports,
            ip: ip.unwrap_or_default().trim().to_string(),
            min_bytes,
            port_ranges,
        })
    }

    /// `action` is "A" or "D"; `protocol` is "TCP" or "UDP" (or the raw letter).
    pub fn matches(&self, action: &str, protocol: &str, dst_port: u32, src: &str, dst: &str, bytes: i64) -> bool {
        let protocol = match protocol {
            "T" => "TCP",
            "U" => "UDP",
            other => other,
        };
        let action_matches = match self.action.as_str() {
            "allow" => action == "A",
            "deny" => action == "D",
            _ => true,
        };
        let port_matches =
            self.port_ranges.is_empty() || self.port_ranges.iter().any(|&(low, high)| (low..=high).contains(&dst_port));
        let ip_matches =
            self.ip.is_empty() || contains_ignore_case(src, &self.ip) || contains_ignore_case(dst, &self.ip);

        action_matches
            && (self.protocol == "all" || protocol == self.protocol)
            && port_matches
            && ip_matches
            && bytes >= self.min_bytes
    }
}

/// ASCII case-insensitive substring test. Addresses are ASCII, so this matches OrdinalIgnoreCase.
fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    needle.is_empty()
        || haystack.as_bytes().windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// "443,8000-8100" to inclusive ranges. Empty input means no port filter.
pub fn parse_port_ranges(spec: &str) -> Result<Vec<(u32, u32)>> {
    let mut ranges = Vec::new();
    for token in spec.split(',').map(str::trim).filter(|token| !token.is_empty()) {
        let bounds: Vec<&str> = token.split('-').map(str::trim).collect();
        let range = match bounds.as_slice() {
            [single] => parse_port(single).map(|port| (port, port)),
            [low, high] => parse_port(low).zip(parse_port(high)),
            _ => None,
        };
        match range {
            Some((low, high)) if low <= high => ranges.push((low, high)),
            _ => bail!("Invalid port or range '{token}'. Use values like 443 or 8000-8100."),
        }
    }
    Ok(ranges)
}

/// Digits only, at most 65535. Rust's integer parser would also accept a leading '+'.
fn parse_port(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok().filter(|port| *port <= 65_535)
}

static RELATIVE_BOUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^([0-9]+)([mhd])$").expect("valid relative window pattern"));

/// "90m", "24h", "7d" relative to `now`, or an ISO-8601 timestamp (UTC when no offset is given).
pub fn parse_window_bound(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let text = value.trim();
    if let Some(captures) = RELATIVE_BOUND.captures(text) {
        let amount: i64 = captures[1].parse().map_err(|_| invalid_window(value))?;
        let delta = match captures[2].to_ascii_lowercase().as_str() {
            "m" => TimeDelta::try_minutes(amount),
            "h" => TimeDelta::try_hours(amount),
            _ => TimeDelta::try_days(amount),
        };
        return delta.and_then(|delta| now.checked_sub_signed(delta)).ok_or_else(|| invalid_window(value));
    }
    parse_iso_utc(text).ok_or_else(|| invalid_window(value))
}

/// RFC 3339 with an offset, or a naive date or date-time read as UTC. None for anything else.
pub fn parse_iso_utc(text: &str) -> Option<DateTime<Utc>> {
    if let Ok(with_offset) = DateTime::parse_from_rfc3339(text) {
        return Some(with_offset.with_timezone(&Utc));
    }
    const NAIVE_FORMATS: [&str; 4] =
        ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"];
    let naive = NAIVE_FORMATS
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .or_else(|| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0))?;
    Some(naive.and_utc())
}

fn invalid_window(value: &str) -> anyhow::Error {
    anyhow!("Invalid time '{value}'. Use 90m, 24h, 7d or an ISO-8601 timestamp.")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap()
    }

    #[test]
    fn port_ranges_parse_single_values_and_ranges() {
        assert_eq!(parse_port_ranges("443,8000-8100").unwrap(), vec![(443, 443), (8000, 8100)]);
        assert_eq!(parse_port_ranges(" 22 , 0-65535 ,, ").unwrap(), vec![(22, 22), (0, 65535)]);
        assert!(parse_port_ranges("").unwrap().is_empty());
        assert!(parse_port_ranges(" , ").unwrap().is_empty());
    }

    #[test]
    fn port_ranges_reject_invalid_tokens() {
        for bad in ["65536", "-1", "10-", "-10", "5-3", "1-2-3", "abc", "+443", "1 2", "443-x"] {
            let error = parse_port_ranges(bad).unwrap_err().to_string();
            assert_eq!(
                error,
                format!("Invalid port or range '{}'. Use values like 443 or 8000-8100.", bad.trim()),
                "{bad}"
            );
        }
    }

    #[test]
    fn create_normalises_and_rejects_bad_options() {
        let filter = TrafficFilter::create(Some(" DENY "), Some("Tcp"), Some(" 1433 "), Some(" 10.10.1. "), 5).unwrap();
        assert_eq!(filter.action, "deny");
        assert_eq!(filter.protocol, "TCP");
        assert_eq!(filter.ports, "1433");
        assert_eq!(filter.ip, "10.10.1.");
        assert_eq!(filter.min_bytes, 5);
        assert_eq!(filter.port_ranges, vec![(1433, 1433)]);

        let defaults = TrafficFilter::create(None, None, None, None, 0).unwrap();
        assert_eq!((defaults.action.as_str(), defaults.protocol.as_str()), ("all", "all"));

        let action = TrafficFilter::create(Some("block"), None, None, None, 0).unwrap_err();
        assert_eq!(action.to_string(), "Invalid --action 'block'. Use all, allow or deny.");
        let protocol = TrafficFilter::create(None, Some("icmp"), None, None, 0).unwrap_err();
        assert_eq!(protocol.to_string(), "Invalid --protocol 'icmp'. Use all, tcp or udp.");
        let negative = TrafficFilter::create(None, None, None, None, -1).unwrap_err();
        assert_eq!(negative.to_string(), "--min-bytes cannot be negative.");
    }

    #[test]
    fn matches_applies_each_filter_per_observation() {
        let filter =
            TrafficFilter::create(Some("allow"), Some("tcp"), Some("443,8000-8100"), Some("APP"), 100).unwrap();
        assert!(filter.matches("A", "TCP", 8050, "10.0.1.app", "13.1.2.3", 100));
        assert!(filter.matches("A", "T", 443, "10.0.0.1", "10.0.1.APP", 150));
        assert!(!filter.matches("D", "TCP", 443, "10.0.0.1", "10.0.1.4", 150), "action");
        assert!(!filter.matches("A", "UDP", 443, "10.0.0.1", "10.0.1.4", 150), "protocol");
        assert!(!filter.matches("A", "TCP", 80, "10.0.0.1", "10.0.1.4", 150), "port");
        assert!(!filter.matches("A", "TCP", 443, "10.0.0.1", "10.0.2.4", 150), "ip");
        assert!(!filter.matches("A", "TCP", 443, "10.0.0.1", "10.0.1.4", 99), "min bytes");
    }

    #[test]
    fn window_bounds_accept_relative_and_iso_values() {
        assert_eq!(parse_window_bound("90m", now()).unwrap(), Utc.with_ymd_and_hms(2026, 10, 19, 10, 30, 0).unwrap());
        assert_eq!(parse_window_bound("24h", now()).unwrap(), Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap());
        assert_eq!(parse_window_bound("7D", now()).unwrap(), Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap());
        assert_eq!(parse_window_bound("0h", now()).unwrap(), now());

        let zulu = Utc.with_ymd_and_hms(2026, 10, 18, 10, 0, 0).unwrap();
        assert_eq!(parse_window_bound("2026-10-18T10:00:00Z", now()).unwrap(), zulu);
        assert_eq!(parse_window_bound("2026-10-18T20:00:00+10:00", now()).unwrap(), zulu);
        assert_eq!(parse_window_bound("2026-10-18T10:00:00.1234567Z", now()).unwrap().timestamp(), zulu.timestamp());
        assert_eq!(parse_window_bound("2026-10-18T10:00:00", now()).unwrap(), zulu);
        assert_eq!(parse_window_bound("2026-10-18 10:00", now()).unwrap(), zulu);
        assert_eq!(
            parse_window_bound("2026-10-18", now()).unwrap(),
            Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn window_bounds_reject_garbage() {
        for bad in ["", "yesterday", "10x", "2026-13-01", "99999999999999999h"] {
            let error = parse_window_bound(bad, now()).unwrap_err().to_string();
            assert_eq!(error, format!("Invalid time '{bad}'. Use 90m, 24h, 7d or an ISO-8601 timestamp."), "{bad}");
        }
    }
}
