//! "discover": lists storage accounts that receive flow logs or contain flow log blobs, with health per configuration.
//! Table and JSON layouts follow DiscoverCommand.cs and ConsoleTable.cs.

use anyhow::Result;

use crate::azure::AzureClient;
use crate::cli::DiscoverArgs;
use crate::discovery::{InventoryRequest, load_inventory, progress_logger};
use crate::flowlog::paths::last_segment;
use crate::model::{InventoryResult, flow_status};

pub async fn run(args: &DiscoverArgs) -> Result<u8> {
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
    let request = InventoryRequest {
        subscriptions: args.subscriptions.clone(),
        storage_accounts: args.storage_account.clone(),
        stale_hours: args.stale_hours,
        include_all: args.all,
    };
    let load = load_inventory(&client, &request, &log).await?;

    let text = if format == "json" { serde_json::to_string_pretty(&load.result)? } else { render_table(&load.result) };

    match &args.output {
        None => println!("{text}"),
        Some(path) => {
            std::fs::write(path, format!("{text}\n"))?;
            eprintln!("Inventory written to: {}", path.display());
        }
    }
    Ok(0)
}

fn render_table(result: &InventoryResult) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Storage accounts with flow logs ({}) - generated {}, stale after {}h\n\n",
        result.accounts.len(),
        result.generated_utc,
        result.stale_after_hours
    ));

    let account_rows: Vec<Vec<String>> = result
        .accounts
        .iter()
        .map(|account| {
            vec![
                account.subscription_id.clone(),
                account.name.clone(),
                account.status.clone(),
                account.flow_log_configs.to_string(),
                yes_no(account.container_present).to_string(),
                account.latest_blob_utc.clone().unwrap_or_else(|| "-".to_string()),
                account.observed_target.as_deref().map_or_else(|| "-".to_string(), last_segment),
            ]
        })
        .collect();
    write_table(
        &mut out,
        &["Subscription", "Storage account", "Status", "Configs", "Container", "Latest blob (UTC)", "Unmanaged source"],
        &account_rows,
    );

    if !result.flow_logs.is_empty() {
        out.push_str(&format!("\nFlow log configurations ({})\n\n", result.flow_logs.len()));
        let config_rows: Vec<Vec<String>> = result
            .flow_logs
            .iter()
            .map(|config| {
                vec![
                    config.storage_account.clone(),
                    config.name.clone(),
                    config.target_kind.clone(),
                    last_segment(&config.target),
                    yes_no(config.enabled).to_string(),
                    config.status.clone(),
                    config.latest_blob_utc.clone().unwrap_or_else(|| "-".to_string()),
                    config.age_hours.map_or_else(|| "-".to_string(), crate::util::one_decimal),
                ]
            })
            .collect();
        write_table(
            &mut out,
            &["Storage account", "Flow log", "Kind", "Target", "Enabled", "Status", "Latest blob (UTC)", "Age (h)"],
            &config_rows,
        );

        for issue in
            result.flow_logs.iter().filter(|config| !config.message.is_empty() && config.status != flow_status::HEALTHY)
        {
            out.push_str(&format!("  {}/{}: {}\n", issue.storage_account, issue.name, issue.message));
        }
    }

    out.trim_end().to_string()
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Left-aligned columns sized to the widest cell, two spaces apart (ConsoleTable.cs).
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
    use crate::model::{FlowLogConfigInfo, StorageAccountInfo};

    #[test]
    fn table_pads_columns_and_trims_line_ends() {
        let mut out = String::new();
        write_table(
            &mut out,
            &["Name", "Status"],
            &[vec!["a".to_string(), "Healthy".to_string()], vec!["longer".to_string(), "".to_string()]],
        );
        assert_eq!(out, "Name    Status\n------  -------\na       Healthy\nlonger\n");
    }

    #[test]
    fn render_lists_accounts_configs_and_issues() {
        let result = InventoryResult {
            generated_utc: "2026-10-19T12:00:00Z".into(),
            stale_after_hours: 3,
            accounts: vec![StorageAccountInfo {
                subscription_id: "sub".into(),
                name: "stflowlogs01".into(),
                flow_log_configs: 1,
                container_present: true,
                status: "Stale".into(),
                latest_blob_utc: Some("2026-10-19T07:00:00Z".into()),
                ..Default::default()
            }],
            flow_logs: vec![FlowLogConfigInfo {
                storage_account: "stflowlogs01".into(),
                name: "flowlog-web-nsg".into(),
                target: "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/web-nsg"
                    .into(),
                target_kind: "NSG".into(),
                enabled: true,
                status: "Stale".into(),
                latest_blob_utc: Some("2026-10-19T07:00:00Z".into()),
                age_hours: Some(5.0),
                message: "Newest blob is 5.0 hours old.".into(),
                ..Default::default()
            }],
        };
        let text = render_table(&result);
        assert!(
            text.starts_with("Storage accounts with flow logs (1) - generated 2026-10-19T12:00:00Z, stale after 3h\n")
        );
        assert!(text.contains("Flow log configurations (1)"));
        assert!(text.contains("web-nsg"));
        assert!(text.ends_with("  stflowlogs01/flowlog-web-nsg: Newest blob is 5.0 hours old."));
    }
}
