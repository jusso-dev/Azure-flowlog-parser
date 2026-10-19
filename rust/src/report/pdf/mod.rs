//! Owner: pdf agent. Hand-written PDF 1.4 writer for the traffic report, with no dependencies.
//! Port of PdfReportWriter.cs: same sections in the same order, with the same headings, columns and footers.
mod document;
mod layout;
mod text;

use std::collections::HashMap;
use std::hash::Hash;
use std::path::Path;

use anyhow::Context;

use crate::model::{ReportFlow, ReportModel, ReportScope, scope_kind};
use crate::util::{format_bytes, one_decimal, thousands};
use layout::{Cell, PdfLayout};

const TOP_EDGES: usize = 50;
const TOP_FLOWS: usize = 150;
/// Body text size: the default of the .NET Paragraph call.
const BODY: f64 = 9.0;
/// Tenant and Subscriptions rows when the run has no subscription data (offline input without a topology).
const NOT_AVAILABLE: &str = "not available (offline input)";
/// Monitoring table columns. Each width list adds up to 515 points, the text width of the page.
const ACCOUNT_HEADERS: [&str; 7] =
    ["Storage account", "Resource group", "Subscription", "Status", "Configs", "Latest blob", "Unmanaged source"];
const ACCOUNT_WIDTHS: [f64; 7] = [88.0, 84.0, 110.0, 60.0, 38.0, 75.0, 60.0];
const FLOW_LOG_HEADERS: [&str; 8] =
    ["Storage account", "Flow log", "Target", "Target RG", "Kind", "Enabled", "Status", "Age"];
const FLOW_LOG_WIDTHS: [f64; 8] = [78.0, 92.0, 88.0, 92.0, 34.0, 38.0, 58.0, 35.0];

pub fn write(model: &ReportModel, path: &Path) -> anyhow::Result<()> {
    let mut layout = PdfLayout::new();

    layout.title("Azure VNet Flow Traffic Report");
    layout.paragraph(
        &format!(
            "Generated {} | Source {} | Window {} to {}",
            model.generated_utc, model.source, model.window.from_utc, model.window.to_utc
        ),
        8.5,
        true,
        false,
    );
    let filters = &model.filters;
    layout.paragraph(
        &format!(
            "Filters: action {}; protocol {}; destination ports {}; IP contains {}; min bytes {}",
            filters.action,
            filters.protocol,
            or_any(&filters.ports),
            or_any(&filters.ip),
            filters.min_bytes
        ),
        8.5,
        true,
        false,
    );

    write_summary(&mut layout, model);
    write_monitoring(&mut layout, model);
    write_edges(&mut layout, model);
    write_storage_accounts(&mut layout, model);
    write_top_flows(&mut layout, model);
    write_notes(&mut layout, model);

    layout.footers();
    std::fs::write(path, layout.into_bytes()).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn write_summary(layout: &mut PdfLayout, model: &ReportModel) {
    layout.heading("Summary");
    layout.table(&["Measure", "Value"], &[170.0, 345.0], &summary_rows(model), 8.5);
}

fn summary_rows(model: &ReportModel) -> Vec<Vec<Cell>> {
    let s = &model.summary;
    vec![
        pair("Tenant", tenant_summary(model)),
        pair("Subscriptions", subscription_summary(model)),
        pair("Flow observations", thousands(s.flows)),
        pair("Allowed / denied", format!("{} / {}", thousands(s.allowed), thousands(s.denied))),
        pair("Bytes / packets", format!("{} / {}", format_bytes(s.bytes), thousands(s.packets))),
        pair("Storage accounts read", thousands(s.storage_accounts as i64)),
        pair("Scopes in report", thousands(s.scopes as i64)),
        pair(
            "Unique source / destination IPs",
            format!("{} / {}", thousands(s.unique_source_ips as i64), thousands(s.unique_destination_ips as i64)),
        ),
        pair("Allowed inbound duplicates skipped", thousands(s.observations_skipped)),
    ]
}

/// Distinct tenant ids in first-seen order, joined with ", ".
fn tenant_summary(model: &ReportModel) -> String {
    let mut tenants: Vec<&str> = Vec::new();
    for tenant in model.subscriptions.iter().map(|s| s.tenant_id.as_str()) {
        if !tenant.is_empty() && !tenants.contains(&tenant) {
            tenants.push(tenant);
        }
    }
    if tenants.is_empty() { NOT_AVAILABLE.to_string() } else { tenants.join(", ") }
}

/// "displayName (subscriptionId)" per subscription, joined with "; ". The id alone when there is no display name.
fn subscription_summary(model: &ReportModel) -> String {
    if model.subscriptions.is_empty() {
        return NOT_AVAILABLE.to_string();
    }
    let labels: Vec<String> = model
        .subscriptions
        .iter()
        .map(|s| {
            if s.display_name.is_empty() {
                s.subscription_id.clone()
            } else {
                format!("{} ({})", s.display_name, s.subscription_id)
            }
        })
        .collect();
    labels.join("; ")
}

/// Display name of a subscription in the model, or its id when no name is known.
fn subscription_name<'a>(model: &'a ReportModel, subscription_id: &'a str) -> &'a str {
    model
        .subscriptions
        .iter()
        .find(|s| s.subscription_id.eq_ignore_ascii_case(subscription_id) && !s.display_name.is_empty())
        .map_or(subscription_id, |s| s.display_name.as_str())
}

fn write_monitoring(layout: &mut PdfLayout, model: &ReportModel) {
    layout.heading("Flow log monitoring");
    if model.accounts.is_empty() && model.flow_logs.is_empty() {
        layout.paragraph(
            "No storage account inventory in this run (local input or no accounts discovered).",
            BODY,
            true,
            false,
        );
        return;
    }

    layout.table(&ACCOUNT_HEADERS, &ACCOUNT_WIDTHS, &account_rows(model), 8.0);

    let log_rows = flow_log_rows(model);
    if !log_rows.is_empty() {
        layout.space(4.0);
        layout.table(&FLOW_LOG_HEADERS, &FLOW_LOG_WIDTHS, &log_rows, 8.0);
    }
}

fn account_rows(model: &ReportModel) -> Vec<Vec<Cell>> {
    model
        .accounts
        .iter()
        .map(|a| {
            vec![
                cell(&a.name),
                cell(or_dash(&a.resource_group)),
                cell(subscription_name(model, &a.subscription_id)),
                cell(&a.status),
                cell(a.flow_log_configs.to_string()),
                cell(a.latest_blob_utc.as_deref().map_or_else(|| "-".to_string(), short_time)),
                cell(a.observed_target.as_deref().map_or("-", last_segment)),
            ]
        })
        .collect()
}

fn flow_log_rows(model: &ReportModel) -> Vec<Vec<Cell>> {
    model
        .flow_logs
        .iter()
        .map(|f| {
            // Without a target name, show the last segment of the target id.
            let target = if f.target_name.is_empty() { last_segment(&f.target) } else { f.target_name.as_str() };
            vec![
                cell(&f.storage_account),
                cell(or_dash(&f.name)),
                cell(target),
                cell(or_dash(&f.target_resource_group)),
                cell(&f.target_kind),
                cell(if f.enabled { "yes" } else { "no" }),
                cell(&f.status),
                cell(f.age_hours.map_or("-".to_string(), |hours| format!("{} h", one_decimal(hours)))),
            ]
        })
        .collect()
}

fn write_edges(layout: &mut PdfLayout, model: &ReportModel) {
    layout.heading(&format!("Traffic between scopes (top {TOP_EDGES} by bytes)"));
    let edges = top_edges(model);
    if edges.is_empty() {
        layout.paragraph("No traffic matched the filters.", BODY, true, false);
        return;
    }

    let max = edges.iter().map(|e| e.bytes).max().unwrap_or(0).max(1);
    let rows: Vec<Vec<Cell>> = edges
        .iter()
        .map(|e| {
            vec![
                cell(label_of(model, e.src)),
                cell(label_of(model, e.dst)),
                cell(thousands(e.flows)),
                cell(thousands(e.allowed)),
                cell(thousands(e.denied)),
                cell(format_bytes(e.bytes)),
                Cell::Bar(e.bytes as f64 / max as f64),
            ]
        })
        .collect();
    layout.table(
        &["From", "To", "Flows", "Allowed", "Denied", "Bytes", "Share of bytes"],
        &[110.0, 110.0, 46.0, 50.0, 46.0, 60.0, 93.0],
        &rows,
        8.0,
    );
}

fn write_storage_accounts(layout: &mut PdfLayout, model: &ReportModel) {
    layout.heading("Individual view per storage account");
    let mut scopes: Vec<&ReportScope> = model.scopes.iter().filter(|s| s.kind == scope_kind::STORAGE).collect();
    if scopes.is_empty() {
        layout.paragraph("No storage account scopes appear in the matched traffic.", BODY, true, false);
        return;
    }
    scopes.sort_by(|a, b| a.id.cmp(&b.id));

    for scope in scopes {
        let id = scope.id.as_str();
        let inbound: Vec<&ReportFlow> = model.flows.iter().filter(|f| f.dst_scope == id).collect();
        let outbound: Vec<&ReportFlow> = model.flows.iter().filter(|f| f.src_scope == id).collect();
        let denied: i64 = model
            .flows
            .iter()
            .filter(|f| (f.src_scope == id || f.dst_scope == id) && f.action == "D")
            .map(|f| f.flows)
            .sum();
        let in_flows: i64 = inbound.iter().map(|f| f.flows).sum();
        let in_bytes: i64 = inbound.iter().map(|f| f.bytes).sum();
        let out_flows: i64 = outbound.iter().map(|f| f.flows).sum();
        let out_bytes: i64 = outbound.iter().map(|f| f.bytes).sum();

        layout.paragraph(id, 10.0, false, true);
        layout.paragraph(
            &format!(
                "In: {} flows, {} | Out: {} flows, {} | Denied observations: {}",
                thousands(in_flows),
                format_bytes(in_bytes),
                thousands(out_flows),
                format_bytes(out_bytes),
                thousands(denied)
            ),
            8.5,
            false,
            false,
        );

        let peer_items = inbound
            .iter()
            .map(|f| (label_of(model, &f.src_scope), f.bytes))
            .chain(outbound.iter().map(|f| (label_of(model, &f.dst_scope), f.bytes)));
        let peers: Vec<String> = top_by_bytes(peer_items, 5)
            .into_iter()
            .map(|(label, bytes)| format!("{label} ({})", format_bytes(bytes)))
            .collect();

        let port_items = model
            .flows
            .iter()
            .filter(|f| f.src_scope == id || f.dst_scope == id)
            .map(|f| (format!("{}/{}", f.dst_port, f.proto), f.bytes));
        let ports: Vec<String> = top_by_bytes(port_items, 5)
            .into_iter()
            .map(|(key, bytes)| format!("{key} ({})", format_bytes(bytes)))
            .collect();

        layout.paragraph(&format!("Top peers: {}", join_or_none(&peers)), 8.5, false, false);
        layout.paragraph(&format!("Top destination ports: {}", join_or_none(&ports)), 8.5, false, false);
        layout.space(6.0);
    }
}

fn write_top_flows(layout: &mut PdfLayout, model: &ReportModel) {
    let s = &model.summary;
    let shown = model.flows.len().min(TOP_FLOWS);
    layout.heading(&format!("Top flows (top {shown} of {} rows by bytes)", thousands(s.rows_total as i64)));
    if model.flows.is_empty() {
        layout.paragraph("No flow rows matched the filters.", BODY, true, false);
        return;
    }

    let rows: Vec<Vec<Cell>> = model
        .flows
        .iter()
        .take(TOP_FLOWS)
        .map(|f| {
            vec![
                cell(short_time(&f.first)),
                cell(&f.src),
                cell(&f.dst),
                cell(f.dst_port.to_string()),
                cell(&f.proto),
                cell(if f.action == "A" { "allow" } else { "deny" }),
                cell(if f.direction == "I" { "in" } else { "out" }),
                cell(thousands(f.flows)),
                cell(format_bytes(f.bytes)),
                cell(&f.rule),
            ]
        })
        .collect();
    layout.table(
        &["First seen", "Source", "Destination", "Port", "Proto", "Action", "Dir", "Flows", "Bytes", "Rule"],
        &[76.0, 70.0, 70.0, 32.0, 32.0, 36.0, 24.0, 40.0, 52.0, 83.0],
        &rows,
        7.0,
    );
}

fn write_notes(layout: &mut PdfLayout, model: &ReportModel) {
    layout.heading("Notes");
    for note in &model.notes {
        layout.bullet(note);
    }
}

/// Traffic between one source scope and one destination scope.
struct Edge<'a> {
    src: &'a str,
    dst: &'a str,
    flows: i64,
    allowed: i64,
    denied: i64,
    bytes: i64,
}

/// Scope pairs by bytes, then flows, largest first. Ties keep first-seen order, as the .NET LINQ query does.
fn top_edges(model: &ReportModel) -> Vec<Edge<'_>> {
    let mut edges: Vec<Edge<'_>> = Vec::new();
    let mut index: HashMap<(&str, &str), usize> = HashMap::new();
    for f in &model.flows {
        let key = (f.src_scope.as_str(), f.dst_scope.as_str());
        let i = *index.entry(key).or_insert_with(|| {
            edges.push(Edge { src: key.0, dst: key.1, flows: 0, allowed: 0, denied: 0, bytes: 0 });
            edges.len() - 1
        });
        let edge = &mut edges[i];
        edge.flows += f.flows;
        if f.action == "A" {
            edge.allowed += f.flows;
        }
        if f.action == "D" {
            edge.denied += f.flows;
        }
        edge.bytes += f.bytes;
    }

    edges.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(b.flows.cmp(&a.flows)));
    edges.truncate(TOP_EDGES);
    edges
}

/// Groups by key in first-seen order, sums the bytes, and returns the `take` largest. Ties keep first-seen order.
fn top_by_bytes<K: Eq + Hash + Clone>(items: impl IntoIterator<Item = (K, i64)>, take: usize) -> Vec<(K, i64)> {
    let mut groups: Vec<(K, i64)> = Vec::new();
    let mut index: HashMap<K, usize> = HashMap::new();
    for (key, bytes) in items {
        match index.get(&key) {
            Some(&i) => groups[i].1 += bytes,
            None => {
                index.insert(key.clone(), groups.len());
                groups.push((key, bytes));
            }
        }
    }

    groups.sort_by_key(|group| std::cmp::Reverse(group.1));
    groups.truncate(take);
    groups
}

/// Display label for a scope id, or the id itself when the scope is not in the model.
fn label_of<'a>(model: &'a ReportModel, scope_id: &'a str) -> &'a str {
    model.scopes.iter().find(|s| s.id == scope_id).map_or(scope_id, |s| s.label.as_str())
}

/// Last non-empty segment of an ARM id (the resource name), or the whole value when it has none.
fn last_segment(arm_id: &str) -> &str {
    arm_id.split('/').rev().find(|part| !part.is_empty()).unwrap_or(arm_id)
}

/// "2026-10-18T10:00:00Z" becomes "2026-10-18 10:00". Shorter values are kept as they are.
fn short_time(iso: &str) -> String {
    if iso.chars().count() >= 16 { iso.chars().take(16).collect::<String>().replace('T', " ") } else { iso.to_string() }
}

fn or_any(value: &str) -> &str {
    if value.is_empty() { "any" } else { value }
}

/// "-" for an empty table cell, as PdfReportWriter.cs shows missing values.
fn or_dash(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

fn join_or_none(parts: &[String]) -> String {
    if parts.is_empty() { "none".to_string() } else { parts.join(", ") }
}

fn cell(text: impl Into<String>) -> Cell {
    Cell::Text(text.into())
}

fn pair(label: &str, value: String) -> Vec<Cell> {
    vec![cell(label), cell(value)]
}

#[cfg(test)]
mod tests {
    use super::document::test_support::{assert_qpdf_clean, assert_well_formed};
    use super::*;
    use crate::model::{FlowLogConfigInfo, ReportSubscription, ReportSummary, ReportWindow, StorageAccountInfo};

    /// The content-stream form of a line drawn on the page, parentheses escaped.
    fn drawn(line: &str) -> String {
        format!("({}) Tj", text::escape(line))
    }

    fn scope(id: &str, kind: &str, label: &str) -> ReportScope {
        ReportScope { id: id.into(), kind: kind.into(), label: label.into(), ..Default::default() }
    }

    fn flow(src_scope: &str, dst_scope: &str, action: &str, flows: i64, bytes: i64) -> ReportFlow {
        ReportFlow {
            first: "2026-10-18T10:00:00Z".into(),
            src: "10.0.0.4".into(),
            dst: "10.1.0.5".into(),
            src_scope: src_scope.into(),
            dst_scope: dst_scope.into(),
            proto: "T".into(),
            dst_port: 443,
            action: action.into(),
            direction: "I".into(),
            rule: "DefaultRule_AllowVnetInBound".into(),
            flows,
            bytes,
            ..Default::default()
        }
    }

    fn populated_model() -> ReportModel {
        ReportModel {
            generated_utc: "2026-10-19T00:00:00Z".into(),
            source: "local".into(),
            window: ReportWindow { from_utc: "2026-10-18T10:00:00Z".into(), to_utc: "2026-10-18T13:00:00Z".into() },
            summary: ReportSummary { flows: 9, allowed: 7, denied: 2, bytes: 90_000, rows_total: 9, ..Default::default() },
            scopes: vec![
                scope("storage:acct1", scope_kind::STORAGE, "acct1"),
                scope("storage:acct2", scope_kind::STORAGE, "acct2"),
                scope("vnet:v1", scope_kind::VNET, "vnet-1"),
            ],
            flows: vec![
                flow("vnet:v1", "storage:acct1", "A", 3, 40_000),
                flow("storage:acct1", "vnet:v1", "D", 2, 2_000),
                flow("vnet:v1", "storage:acct2", "A", 4, 48_000),
            ],
            accounts: vec![StorageAccountInfo {
                name: "acct1".into(),
                subscription_id: "sub-1".into(),
                status: "Healthy".into(),
                flow_log_configs: 1,
                latest_blob_utc: Some("2026-10-18T12:00:00Z".into()),
                observed_target: Some("/subscriptions/sub-1/resourceGroups/rg/providers/Microsoft.Network/networkWatchers/nw/flowLogs/fl1".into()),
                ..Default::default()
            }],
            flow_logs: vec![FlowLogConfigInfo {
                storage_account: "acct1".into(),
                target: "/subscriptions/sub-1/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/vnet-1".into(),
                target_kind: "VirtualNetwork".into(),
                enabled: true,
                status: "Healthy".into(),
                age_hours: Some(1.25),
                ..Default::default()
            }],
            notes: vec!["Notes are wrapped onto several lines when they are long enough to need it. ".repeat(3)],
            ..Default::default()
        }
    }

    fn write_to_temp(name: &str, model: &ReportModel) -> Vec<u8> {
        let dir = std::env::temp_dir().join("azure-flowlog-parser-pdf-tests");
        std::fs::create_dir_all(&dir).expect("create test dir");
        let path = dir.join(format!("{name}.pdf"));
        write(model, &path).expect("write pdf");
        std::fs::read(&path).expect("read pdf back")
    }

    #[test]
    fn empty_model_writes_valid_file_with_every_no_data_line() {
        let pdf = write_to_temp("report-empty", &ReportModel::default());

        assert_well_formed(&pdf);
        let content = String::from_utf8_lossy(&pdf);
        for line in [
            "No storage account inventory in this run (local input or no accounts discovered).",
            "No traffic matched the filters.",
            "No storage account scopes appear in the matched traffic.",
            "No flow rows matched the filters.",
            "Top flows (top 0 of 0 rows by bytes)",
        ] {
            assert!(content.contains(&drawn(line)), "missing: {line}");
        }
        assert_qpdf_clean("report-empty", &pdf);
    }

    #[test]
    fn populated_model_writes_every_section_in_order() {
        let pdf = write_to_temp("report-populated", &populated_model());

        assert_well_formed(&pdf);
        let text = String::from_utf8_lossy(&pdf);
        let headings = [
            "Summary",
            "Flow log monitoring",
            "Traffic between scopes (top 50 by bytes)",
            "Individual view per storage account",
            "Top flows (top 3 of 9 rows by bytes)",
            "Notes",
        ];
        let positions: Vec<usize> = headings
            .iter()
            .map(|heading| text.find(&drawn(heading)).unwrap_or_else(|| panic!("missing heading {heading}")))
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "sections out of order");
        // acct1 talks to vnet-1 in both directions: 40,000 + 2,000 bytes.
        assert!(text.contains(&drawn("Top peers: vnet-1 (41.0 KB)")), "peer line");
        assert_qpdf_clean("report-populated", &pdf);
    }

    #[test]
    fn large_report_paginates_with_repeated_headers_and_footers() {
        let mut model = populated_model();
        model.flows = (0..200).map(|i| flow("vnet:v1", "storage:acct1", "A", 1, 1_000 + i)).collect();
        model.summary.rows_total = 200;

        let pdf = write_to_temp("report-long", &model);

        assert_well_formed(&pdf);
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("Page 2 of "), "report should span several pages");
        assert!(text.matches(&drawn("First seen")).count() > 1, "table header repeats on a new page");
        assert_qpdf_clean("report-long", &pdf);
    }

    #[test]
    fn top_edges_orders_by_bytes_then_flows_and_keeps_first_seen_on_ties() {
        let mut model = ReportModel::default();
        model.flows = vec![
            flow("a", "b", "A", 1, 10),
            flow("c", "d", "A", 1, 10),
            flow("e", "f", "D", 5, 10),
            flow("g", "h", "A", 1, 99),
        ];

        let order: Vec<(&str, &str)> = top_edges(&model).iter().map(|e| (e.src, e.dst)).collect();

        assert_eq!(order, vec![("g", "h"), ("e", "f"), ("a", "b"), ("c", "d")]);
    }

    #[test]
    fn top_by_bytes_sums_per_key_and_keeps_first_seen_on_ties() {
        assert_eq!(top_by_bytes(vec![("x", 5), ("y", 5)], 2), vec![("x", 5), ("y", 5)]);
        assert_eq!(top_by_bytes(vec![("a", 5), ("b", 5), ("c", 9), ("b", 1)], 2), vec![("c", 9), ("b", 6)]);
    }

    #[test]
    fn helpers_shorten_times_and_ids() {
        assert_eq!(short_time("2026-10-18T10:00:00Z"), "2026-10-18 10:00");
        assert_eq!(short_time("bad"), "bad");
        assert_eq!(last_segment("/subscriptions/s/flowLogs/fl1"), "fl1");
        assert_eq!(last_segment("/subscriptions/s/flowLogs/fl1/"), "fl1");
        assert_eq!(or_any(""), "any");
    }

    fn subscription(id: &str, name: &str, tenant: &str) -> ReportSubscription {
        ReportSubscription { subscription_id: id.into(), display_name: name.into(), tenant_id: tenant.into() }
    }

    #[test]
    fn summary_rows_list_distinct_tenants_and_subscription_names_first() {
        let model = ReportModel {
            subscriptions: vec![
                subscription("sub-1", "Prod", "tenant-1"),
                subscription("sub-2", "", "tenant-2"),
                subscription("sub-3", "Dev", "tenant-1"),
            ],
            ..Default::default()
        };

        let rows = summary_rows(&model);

        assert_eq!(rows[0], pair("Tenant", "tenant-1, tenant-2".into()));
        assert_eq!(rows[1], pair("Subscriptions", "Prod (sub-1); sub-2; Dev (sub-3)".into()));
        assert_eq!(rows[2][0], cell("Flow observations"));
    }

    #[test]
    fn summary_rows_say_when_subscription_data_is_missing() {
        let rows = summary_rows(&ReportModel::default());

        assert_eq!(rows[0], pair("Tenant", NOT_AVAILABLE.into()));
        assert_eq!(rows[1], pair("Subscriptions", NOT_AVAILABLE.into()));
    }

    #[test]
    fn monitoring_tables_total_the_text_width() {
        assert_eq!(ACCOUNT_WIDTHS.iter().sum::<f64>(), 515.0);
        assert_eq!(FLOW_LOG_WIDTHS.iter().sum::<f64>(), 515.0);
    }

    #[test]
    fn account_row_shows_resource_group_and_subscription_name_or_id() {
        let model = ReportModel {
            accounts: vec![
                StorageAccountInfo {
                    name: "acct1".into(),
                    resource_group: "rg-net".into(),
                    subscription_id: "sub-1".into(),
                    status: "Healthy".into(),
                    ..Default::default()
                },
                StorageAccountInfo { name: "acct2".into(), subscription_id: "sub-9".into(), ..Default::default() },
            ],
            subscriptions: vec![subscription("sub-1", "Prod", "tenant-1")],
            ..Default::default()
        };

        let rows = account_rows(&model);

        assert_eq!(rows[0][1], cell("rg-net"));
        assert_eq!(rows[0][2], cell("Prod"));
        assert_eq!(rows[1][1], cell("-"), "empty resource group");
        assert_eq!(rows[1][2], cell("sub-9"));
        assert_eq!(rows[1][5], cell("-"), "no latest blob");
    }

    #[test]
    fn account_row_shortens_latest_blob_like_short_time() {
        let model = ReportModel {
            accounts: vec![StorageAccountInfo {
                name: "acct1".into(),
                latest_blob_utc: Some("2026-10-18T12:00:00Z".into()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let rows = account_rows(&model);

        assert_eq!(rows[0][5], cell("2026-10-18 12:00"));
    }

    #[test]
    fn flow_log_row_uses_target_name_and_resource_group() {
        let model = ReportModel {
            flow_logs: vec![
                FlowLogConfigInfo {
                    storage_account: "acct1".into(),
                    name: "fl-web".into(),
                    target: "/subscriptions/s/resourceGroups/rg-app/providers/Microsoft.Network/networkSecurityGroups/web-nsg".into(),
                    target_name: "web-nsg".into(),
                    target_resource_group: "rg-app".into(),
                    target_kind: "NSG".into(),
                    enabled: true,
                    status: "Healthy".into(),
                    age_hours: Some(1.5),
                    ..Default::default()
                },
                FlowLogConfigInfo {
                    target: "/subscriptions/s/resourceGroups/rg-net/providers/Microsoft.Network/virtualNetworks/hub-vnet".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let rows = flow_log_rows(&model);

        assert_eq!(rows[0][1], cell("fl-web"));
        assert_eq!(rows[0][2], cell("web-nsg"));
        assert_eq!(rows[0][3], cell("rg-app"));
        assert_eq!(rows[0][7], cell("1.5 h"));
        assert_eq!(rows[1][1], cell("-"), "empty flow log name");
        assert_eq!(rows[1][2], cell("hub-vnet"));
        assert_eq!(rows[1][3], cell("-"), "empty target resource group");
    }
}
