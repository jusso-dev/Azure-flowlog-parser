use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::filter::{TrafficFilter, parse_iso_utc};
use super::ipv4::{Ipv4Lookup, is_private, parse_ipv4};
use crate::model::{DenormalizedRecord, MAX_ROWS, ReportFlow, ReportModel, ReportScope, ReportSummary, scope_kind};
use crate::util::iso;

/// Streams observations into aggregated rows. Same rules as TrafficAggregator.cs (window, filters, scope lookup,
/// duplicate-inbound skip, observed range, row cap). Not thread-safe: callers wrap it in a Mutex.
pub struct TrafficAggregator {
    pub filter: TrafficFilter,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub lookup: Option<Ipv4Lookup>,
    pub read_accounts: Vec<String>,
    /// Rows keyed in key order, so ties in the report sort are deterministic.
    rows: BTreeMap<FlowKey, Accumulator>,
    scopes: HashMap<String, ReportScope>,
    ip_scopes: HashMap<String, ReportScope>,
    source_ips: HashSet<String>,
    destination_ips: HashSet<String>,
    flows: i64,
    allowed: i64,
    denied: i64,
    bytes: i64,
    packets: i64,
    skipped: i64,
    observed: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

impl TrafficAggregator {
    pub fn new(
        filter: TrafficFilter,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        lookup: Option<Ipv4Lookup>,
        read_accounts: Vec<String>,
    ) -> Self {
        Self {
            filter,
            from,
            to,
            lookup,
            read_accounts,
            rows: BTreeMap::new(),
            scopes: HashMap::new(),
            ip_scopes: HashMap::new(),
            source_ips: HashSet::new(),
            destination_ips: HashSet::new(),
            flows: 0,
            allowed: 0,
            denied: 0,
            bytes: 0,
            packets: 0,
            skipped: 0,
            observed: None,
        }
    }

    pub fn add_records(&mut self, records: &[DenormalizedRecord]) {
        for record in records {
            self.add_observation(record);
        }
    }

    fn add_observation(&mut self, record: &DenormalizedRecord) {
        let Some(observed) = observation_time(record) else {
            return;
        };
        if self.from.is_some_and(|from| observed < from) || self.to.is_some_and(|to| observed > to) {
            return;
        }

        let action = record.device_action.as_str();
        let protocol = match record.transport_protocol.as_str() {
            "T" => "TCP",
            "U" => "UDP",
            other => other,
        };
        let dst_port: u32 = record.destination_port.parse().unwrap_or(0);
        let bytes = count(&record.bytes_s_to_d) + count(&record.bytes_d_to_s);
        let packets = count(&record.packets_s_to_d) + count(&record.packets_d_to_s);

        if !self.filter.matches(action, protocol, dst_port, &record.source_address, &record.destination_address, bytes)
        {
            return;
        }

        let src_scope = self.scope_for(&record.source_address);
        let dst_scope = self.scope_for(&record.destination_address);

        // An allowed inbound observation has a matching outbound observation at the source NSG, which is already
        // counted when that storage account is read. Denied inbound stays: it is a separate decision.
        if record.device_direction == "I"
            && action == "A"
            && src_scope.kind == scope_kind::STORAGE
            && self.is_read_account(&src_scope.storage_account)
        {
            self.skipped += 1;
            return;
        }

        // The observed range covers kept observations only, so it stands in for the window when none was given.
        self.observed = Some(match self.observed {
            Some((first, last)) => (first.min(observed), last.max(observed)),
            None => (observed, observed),
        });

        self.register_scope(&src_scope);
        self.register_scope(&dst_scope);

        let key = FlowKey {
            src: record.source_address.clone(),
            dst: record.destination_address.clone(),
            src_scope: src_scope.id,
            dst_scope: dst_scope.id,
            proto: protocol.to_string(),
            dst_port,
            action: action.to_string(),
            direction: record.device_direction.clone(),
            rule: record.nsg_rule_name.clone(),
        };
        self.rows.entry(key).or_insert_with(|| Accumulator::new(observed)).add(observed, packets, bytes);

        self.flows += 1;
        match action {
            "A" => self.allowed += 1,
            "D" => self.denied += 1,
            _ => {}
        }
        self.bytes += bytes;
        self.packets += packets;
        self.source_ips.insert(record.source_address.clone());
        self.destination_ips.insert(record.destination_address.clone());
    }

    /// Earliest and latest kept observation, or None when nothing was kept.
    pub fn observed_range(&self) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        self.observed.map_or((None, None), |(first, last)| (Some(first), Some(last)))
    }

    /// Fills summary, flows and scopes of the model. Totals cover every kept observation; rows are capped.
    pub fn fill_model(&self, model: &mut ReportModel, subscription_count: usize) {
        // Sorting a BTreeMap iterator is stable, so ties beyond the spec keys keep key order.
        let mut ordered: Vec<(&FlowKey, &Accumulator)> = self.rows.iter().collect();
        ordered.sort_by(|(key_a, acc_a), (key_b, acc_b)| {
            acc_b
                .bytes
                .cmp(&acc_a.bytes)
                .then(acc_b.flows.cmp(&acc_a.flows))
                .then_with(|| key_a.src.cmp(&key_b.src))
                .then_with(|| key_a.dst.cmp(&key_b.dst))
                .then(key_a.dst_port.cmp(&key_b.dst_port))
        });

        let rows_total = ordered.len();
        let emitted = &ordered[..rows_total.min(MAX_ROWS)];

        model.flows = emitted
            .iter()
            .map(|(key, acc)| ReportFlow {
                first: iso(acc.first),
                last: iso(acc.last),
                src: key.src.clone(),
                dst: key.dst.clone(),
                src_scope: key.src_scope.clone(),
                dst_scope: key.dst_scope.clone(),
                proto: key.proto.clone(),
                dst_port: key.dst_port,
                action: key.action.clone(),
                direction: key.direction.clone(),
                rule: key.rule.clone(),
                flows: acc.flows,
                packets: acc.packets,
                bytes: acc.bytes,
            })
            .collect();

        let used_scope_ids: HashSet<&str> =
            emitted.iter().flat_map(|(key, _)| [key.src_scope.as_str(), key.dst_scope.as_str()]).collect();
        let mut scopes: Vec<ReportScope> =
            self.scopes.values().filter(|scope| used_scope_ids.contains(scope.id.as_str())).cloned().collect();
        scopes.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.id.cmp(&b.id)));
        model.scopes = scopes;

        model.summary = ReportSummary {
            flows: self.flows,
            allowed: self.allowed,
            denied: self.denied,
            bytes: self.bytes,
            packets: self.packets,
            rows: model.flows.len(),
            rows_total,
            rows_truncated: rows_total > MAX_ROWS,
            storage_accounts: self.distinct_read_accounts(),
            scopes: model.scopes.len(),
            subscriptions: subscription_count,
            observations_skipped: self.skipped,
            unique_source_ips: self.source_ips.len(),
            unique_destination_ips: self.destination_ips.len(),
        };
    }

    fn is_read_account(&self, account: &str) -> bool {
        self.read_accounts.iter().any(|read| read.eq_ignore_ascii_case(account))
    }

    fn distinct_read_accounts(&self) -> usize {
        self.read_accounts.iter().map(|account| account.to_ascii_uppercase()).collect::<HashSet<_>>().len()
    }

    fn register_scope(&mut self, scope: &ReportScope) {
        self.scopes.entry(scope.id.clone()).or_insert_with(|| scope.clone());
    }

    fn scope_for(&mut self, address: &str) -> ReportScope {
        if let Some(cached) = self.ip_scopes.get(address) {
            return cached.clone();
        }

        let scope = match parse_ipv4(address) {
            None => unmapped_scope(),
            Some(value) => match self.lookup.as_ref().and_then(|lookup| lookup.find(value)) {
                Some(scope) => scope.clone(),
                None if is_private(value) => unmapped_scope(),
                None => internet_scope(),
            },
        };
        self.ip_scopes.insert(address.to_string(), scope.clone());
        scope
    }
}

/// Tuple start (unix seconds), else the record time. None when neither parses, and the observation is dropped.
fn observation_time(record: &DenormalizedRecord) -> Option<DateTime<Utc>> {
    if let Ok(seconds) = record.start_time.trim().parse::<i64>() {
        return DateTime::<Utc>::from_timestamp(seconds, 0);
    }
    parse_iso_utc(&record.time)
}

fn count(value: &Option<String>) -> i64 {
    value.as_deref().and_then(|text| text.parse().ok()).unwrap_or(0)
}

fn internet_scope() -> ReportScope {
    ReportScope {
        id: "internet".to_string(),
        kind: scope_kind::INTERNET.to_string(),
        label: "Internet".to_string(),
        ..Default::default()
    }
}

fn unmapped_scope() -> ReportScope {
    ReportScope {
        id: "unmapped".to_string(),
        kind: scope_kind::UNMAPPED.to_string(),
        label: "Unmapped".to_string(),
        ..Default::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FlowKey {
    src: String,
    dst: String,
    src_scope: String,
    dst_scope: String,
    proto: String,
    dst_port: u32,
    action: String,
    direction: String,
    rule: String,
}

#[derive(Debug, Clone)]
struct Accumulator {
    first: DateTime<Utc>,
    last: DateTime<Utc>,
    flows: i64,
    packets: i64,
    bytes: i64,
}

impl Accumulator {
    fn new(seen: DateTime<Utc>) -> Self {
        Self { first: seen, last: seen, flows: 0, packets: 0, bytes: 0 }
    }

    fn add(&mut self, seen: DateTime<Utc>, packets: i64, bytes: i64) {
        self.first = self.first.min(seen);
        self.last = self.last.max(seen);
        self.flows += 1;
        self.packets += packets;
        self.bytes += bytes;
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::analysis::ipv4::Ipv4Lookup;

    const START: i64 = 1_760_781_600; // 2026-10-18T10:00:00Z

    fn filter(action: &str) -> TrafficFilter {
        TrafficFilter::create(Some(action), None, None, None, 0).unwrap()
    }

    fn scope(id: &str, kind: &str, account: &str) -> ReportScope {
        ReportScope {
            id: id.to_string(),
            kind: kind.to_string(),
            storage_account: account.to_string(),
            ..Default::default()
        }
    }

    fn lookup() -> Ipv4Lookup {
        let mut lookup = Ipv4Lookup::new();
        lookup.try_add("10.10.0.0/16", scope("stapp", scope_kind::STORAGE, "stapp"));
        lookup.try_add("10.20.0.0/16", scope("stdata", scope_kind::STORAGE, "stdata"));
        lookup
    }

    /// Tuple-level record with explicit fields. Counters are the bytes/packets for S-to-D and D-to-S.
    fn observation(
        start: i64,
        src: &str,
        dst: &str,
        port: &str,
        direction: &str,
        action: &str,
        bytes: i64,
    ) -> DenormalizedRecord {
        DenormalizedRecord {
            start_time: start.to_string(),
            source_address: src.to_string(),
            destination_address: dst.to_string(),
            source_port: "50000".to_string(),
            destination_port: port.to_string(),
            transport_protocol: "T".to_string(),
            device_direction: direction.to_string(),
            device_action: action.to_string(),
            nsg_rule_name: "UserRule".to_string(),
            version: 2,
            packets_s_to_d: Some("2".to_string()),
            bytes_s_to_d: Some(bytes.to_string()),
            packets_d_to_s: Some("1".to_string()),
            bytes_d_to_s: Some("0".to_string()),
            ..Default::default()
        }
    }

    fn model_for(aggregator: &TrafficAggregator) -> ReportModel {
        let mut model = ReportModel::default();
        aggregator.fill_model(&mut model, 2);
        model
    }

    #[test]
    fn inbound_allowed_is_skipped_when_source_account_was_read() {
        let mut aggregator = TrafficAggregator::new(
            filter("all"),
            None,
            None,
            Some(lookup()),
            vec!["stapp".to_string(), "STDATA".to_string()],
        );
        aggregator.add_records(&[
            // Outbound at the app NSG: kept.
            observation(START, "10.10.1.4", "10.20.1.5", "1433", "O", "A", 100),
            // Matching inbound at the data NSG: duplicate, skipped.
            observation(START, "10.10.1.4", "10.20.1.5", "1433", "I", "A", 100),
            // Denied inbound is a separate decision: kept.
            observation(START, "10.10.1.4", "10.20.1.5", "1433", "I", "D", 0),
        ]);

        let model = model_for(&aggregator);
        assert_eq!(model.summary.observations_skipped, 1);
        assert_eq!(model.summary.flows, 2);
        assert_eq!(model.summary.allowed, 1);
        assert_eq!(model.summary.denied, 1);
        assert_eq!(model.summary.storage_accounts, 2);
        assert_eq!(model.summary.rows, 2);
    }

    #[test]
    fn inbound_is_kept_when_source_account_was_not_read() {
        let mut aggregator =
            TrafficAggregator::new(filter("all"), None, None, Some(lookup()), vec!["stdata".to_string()]);
        aggregator.add_records(&[observation(START, "10.10.1.4", "10.20.1.5", "1433", "I", "A", 100)]);

        let model = model_for(&aggregator);
        assert_eq!(model.summary.observations_skipped, 0);
        assert_eq!(model.summary.flows, 1);
    }

    #[test]
    fn window_bounds_are_inclusive_and_observed_range_covers_kept_observations() {
        let from = Utc.timestamp_opt(START, 0).unwrap();
        let to = Utc.timestamp_opt(START + 60, 0).unwrap();
        let mut aggregator = TrafficAggregator::new(filter("all"), Some(from), Some(to), None, vec![]);
        aggregator.add_records(&[
            observation(START - 1, "1.1.1.1", "2.2.2.2", "443", "O", "A", 1),
            observation(START, "1.1.1.1", "2.2.2.2", "443", "O", "A", 1),
            observation(START + 30, "1.1.1.1", "2.2.2.2", "443", "O", "A", 1),
            observation(START + 60, "1.1.1.1", "2.2.2.2", "443", "O", "A", 1),
            observation(START + 61, "1.1.1.1", "2.2.2.2", "443", "O", "A", 1),
        ]);

        assert_eq!(model_for(&aggregator).summary.flows, 3);
        assert_eq!(aggregator.observed_range(), (Some(from), Some(to)));
    }

    #[test]
    fn observed_range_is_empty_when_nothing_is_kept() {
        let mut aggregator = TrafficAggregator::new(filter("deny"), None, None, None, vec![]);
        aggregator.add_records(&[observation(START, "1.1.1.1", "2.2.2.2", "443", "O", "A", 1)]);
        assert_eq!(aggregator.observed_range(), (None, None));
    }

    #[test]
    fn observation_time_falls_back_to_record_time_and_drops_unparseable() {
        let mut aggregator = TrafficAggregator::new(filter("all"), None, None, None, vec![]);
        let mut from_record_time = observation(START, "1.1.1.1", "2.2.2.2", "443", "O", "A", 1);
        from_record_time.start_time = "not-a-number".to_string();
        from_record_time.time = "2026-10-18T11:00:00.0000000Z".to_string();
        let mut unparseable = from_record_time.clone();
        unparseable.time = "yesterday".to_string();
        aggregator.add_records(&[from_record_time, unparseable]);

        let model = model_for(&aggregator);
        assert_eq!(model.summary.flows, 1);
        assert_eq!(model.flows[0].first, "2026-10-18T11:00:00Z");
    }

    #[test]
    fn summary_totals_rows_scopes_and_unique_ips() {
        let mut aggregator = TrafficAggregator::new(filter("all"), None, None, Some(lookup()), vec![]);
        aggregator.add_records(&[
            observation(START, "10.10.1.4", "13.1.2.3", "443", "O", "A", 1000),
            observation(START + 5, "10.10.1.4", "13.1.2.3", "443", "O", "A", 500),
            observation(START, "10.10.1.4", "10.20.1.5", "1433", "O", "D", 0),
            observation(START, "192.168.0.9", "8.8.8.8", "53", "O", "A", 10),
        ]);

        let model = model_for(&aggregator);
        let summary = &model.summary;
        assert_eq!(summary.flows, 4);
        assert_eq!(summary.allowed, 3);
        assert_eq!(summary.denied, 1);
        assert_eq!(summary.bytes, 1000 + 500 + 10);
        assert_eq!(summary.packets, 4 * 3);
        assert_eq!(summary.rows, 3);
        assert_eq!(summary.rows_total, 3);
        assert!(!summary.rows_truncated);
        assert_eq!(summary.subscriptions, 2);
        assert_eq!(summary.unique_source_ips, 2);
        assert_eq!(summary.unique_destination_ips, 3);

        let ids: Vec<&str> = model.scopes.iter().map(|scope| scope.id.as_str()).collect();
        assert_eq!(ids, vec!["internet", "stapp", "stdata", "unmapped"]);
        assert_eq!(model.summary.scopes, 4);
    }

    #[test]
    fn rows_are_sorted_by_bytes_then_flows_then_addresses() {
        let mut aggregator = TrafficAggregator::new(filter("all"), None, None, None, vec![]);
        aggregator.add_records(&[
            observation(START, "1.1.1.2", "2.2.2.2", "80", "O", "A", 10),
            observation(START, "1.1.1.1", "2.2.2.2", "443", "O", "A", 10),
            observation(START, "1.1.1.1", "2.2.2.2", "443", "O", "A", 10),
            observation(START, "9.9.9.9", "2.2.2.2", "22", "O", "A", 500),
        ]);

        let model = model_for(&aggregator);
        let order: Vec<(&str, u32)> = model.flows.iter().map(|flow| (flow.src.as_str(), flow.dst_port)).collect();
        // 500 bytes first; then the two 10-byte rows, the 2-flow row before the 1-flow row.
        assert_eq!(order, vec![("9.9.9.9", 22), ("1.1.1.1", 443), ("1.1.1.2", 80)]);
        assert_eq!(model.flows[1].flows, 2);
        assert_eq!(model.flows[1].bytes, 10 + 10);
    }

    #[test]
    fn row_cap_limits_emitted_rows_but_totals_cover_everything() {
        let mut aggregator = TrafficAggregator::new(filter("all"), None, None, None, vec![]);
        let records: Vec<DenormalizedRecord> = (0..MAX_ROWS + 5)
            .map(|index| {
                let src = format!("10.{}.{}.{}", (index >> 16) & 255, (index >> 8) & 255, index & 255);
                observation(START, &src, "13.1.2.3", "443", "O", "A", index as i64 + 1)
            })
            .collect();
        aggregator.add_records(&records);

        let model = model_for(&aggregator);
        assert_eq!(model.summary.rows, MAX_ROWS);
        assert_eq!(model.summary.rows_total, MAX_ROWS + 5);
        assert!(model.summary.rows_truncated);
        assert_eq!(model.summary.flows, (MAX_ROWS + 5) as i64);
        // The largest flows are kept: the top byte count is the last index.
        assert_eq!(model.flows[0].bytes, (MAX_ROWS + 5) as i64);
        let expected_bytes: i64 = (1..=(MAX_ROWS + 5) as i64).sum();
        assert_eq!(model.summary.bytes, expected_bytes);
    }
}
