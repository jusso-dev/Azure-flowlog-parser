//! "diagnose": every virtual network, subnet, NIC, and attached NSG that is not writing flow logs to storage, with the reason.

use anyhow::Result;
use chrono::Utc;

use crate::azure::AzureClient;
use crate::cli::DiagnoseArgs;
use crate::diagnosis::{apply_checked_health, diagnose};
use crate::discovery::{InventoryRequest, load_inventory, progress_logger};
use crate::model::DiagnosisResult;

pub async fn run(args: &DiagnoseArgs) -> Result<u8> {
    let format = args.output_format.to_ascii_lowercase();
    if format != "table" && format != "json" {
        eprintln!("Error: --output-format must be table or json.");
        return Ok(1);
    }
    if args.stale_hours < 1 {
        eprintln!("Error: --stale-hours must be at least 1.");
        return Ok(1);
    }

    let log = progress_logger(args.verbose);
    let client = AzureClient::new()?;
    // Health-check every account. The storage filter only narrows the printed gaps.
    let request = InventoryRequest {
        subscriptions: args.subscriptions.clone(),
        storage_accounts: Vec::new(),
        stale_hours: args.stale_hours,
        include_all: true,
    };
    let load = load_inventory(&client, &request, &log).await?;
    let mut topology = load.topology;
    apply_checked_health(&mut topology, &load.result.accounts, &load.result.flow_logs);
    let result = diagnose(&topology, &topology.flow_logs, args.stale_hours, Utc::now(), &args.storage_account);

    let text = if format == "json" { serde_json::to_string_pretty(&result)? } else { render_table(&result) };
    match &args.output {
        None => println!("{text}"),
        Some(path) => {
            std::fs::write(path, format!("{text}\n"))?;
            eprintln!("Diagnosis written to: {}", path.display());
        }
    }
    Ok(0)
}

fn render_table(result: &DiagnosisResult) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Sources not sending flow logs ({} of {}) - generated {}, stale after {}h\n\n",
        result.finding_count, result.source_count, result.generated_utc, result.stale_after_hours
    ));
    let rows: Vec<Vec<String>> = result
        .findings
        .iter()
        .map(|finding| {
            vec![
                finding.kind.clone(),
                finding.name.clone(),
                finding.resource_group.clone(),
                dash_if_empty(&finding.location),
                finding.reason.clone(),
                dash_if_empty(&finding.flow_log),
                dash_if_empty(&finding.storage_account),
            ]
        })
        .collect();
    write_table(
        &mut out,
        &["Kind", "Name", "Resource group", "Location", "Reason", "Flow log", "Storage account"],
        &rows,
    );
    for finding in result.findings.iter().filter(|finding| !finding.message.is_empty()) {
        out.push_str(&format!("  {} {}: {}\n", finding.kind, finding.name, finding.message));
    }
    out.trim_end().to_string()
}

fn dash_if_empty(value: &str) -> String {
    if value.is_empty() { "-".to_string() } else { value.to_string() }
}

fn write_table(out: &mut String, headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.chars().count()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate().take(widths.len()) {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let header_cells: Vec<String> = headers.iter().map(|header| header.to_string()).collect();
    write_row(out, &header_cells, &widths);
    let rule: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    out.push_str(&rule.join("  "));
    out.push('\n');
    for row in rows {
        write_row(out, row, &widths);
    }
}

fn write_row(out: &mut String, cells: &[String], widths: &[usize]) {
    let parts: Vec<String> = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            let cell = cells.get(index).map_or("", String::as_str);
            format!("{cell}{}", " ".repeat(width.saturating_sub(cell.chars().count())))
        })
        .collect();
    out.push_str(parts.join("  ").trim_end());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FlowLogFinding;

    #[test]
    fn table_names_the_gap_and_prints_the_message() {
        let result = DiagnosisResult {
            generated_utc: "2026-10-24T00:00:00Z".into(),
            stale_after_hours: 6,
            source_count: 2,
            finding_count: 1,
            findings: vec![FlowLogFinding {
                kind: "VNet".into(),
                name: "vnet1".into(),
                resource_group: "rg".into(),
                location: "australiaeast".into(),
                reason: "NotConfigured".into(),
                message: "No flow log targets this virtual network.".into(),
                ..Default::default()
            }],
        };
        let text = render_table(&result);
        assert!(
            text.starts_with(
                "Sources not sending flow logs (1 of 2) - generated 2026-10-24T00:00:00Z, stale after 6h\n"
            )
        );
        assert!(text.contains("NotConfigured"));
        assert!(text.contains("  VNet vnet1: No flow log targets this virtual network."));
        assert!(text.contains("-"));
    }
}
