//! Reads flow log blobs for the report command. Mirrors TrafficReader.ReadAzureAsync: day prefixes of enabled
//! configurations over the window (inclusive UTC days), whole containers for Unmanaged accounts (blobs modified at or
//! after the window start minus one hour), bounded concurrent downloads, then parse and aggregate.

use std::sync::Mutex;

use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use futures::stream::{self, StreamExt};

use super::blob;
use super::client::AzureClient;
use crate::analysis::aggregate::TrafficAggregator;
use crate::flowlog::parse_flow_log;
use crate::flowlog::paths::{ALL_RESOURCES_PREFIX, CONTAINER_NAME, day_prefix};
use crate::model::{InventoryResult, flow_status};

#[derive(Debug, Default)]
pub struct ReadSummary {
    pub blobs: usize,
    /// Accounts with listing or download failures (dedupe may then undercount the source side).
    pub failed_accounts: Vec<String>,
}

/// Lists day prefixes of enabled configurations over [from, to], lists whole containers for Unmanaged accounts,
/// downloads with bounded concurrency, parses, and feeds the aggregator.
pub async fn read_flow_blobs(
    client: &AzureClient,
    inventory: &InventoryResult,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    aggregator: &Mutex<TrafficAggregator>,
    concurrency: usize,
    log: &(dyn Fn(&str) + Sync),
) -> Result<ReadSummary> {
    let mut failed: Vec<String> = Vec::new();
    let mut work: Vec<(String, String)> = Vec::new();

    for config in inventory.flow_logs.iter().filter(|f| f.enabled && f.status != flow_status::DISABLED) {
        for day in days_in_window(from, to) {
            let prefix = day_prefix(&config.target, day);
            match blob::list_blobs(client, &config.storage_account, CONTAINER_NAME, Some(&prefix)).await {
                Ok(blobs) => work.extend(blobs.into_iter().map(|item| (config.storage_account.clone(), item.name))),
                Err(error) => {
                    failed.push(config.storage_account.clone());
                    log(&format!("Warning: could not list {} for {}: {error:#}", config.storage_account, config.name));
                }
            }
        }
    }

    let lower_bound = from - TimeDelta::hours(1);
    for account in inventory.accounts.iter().filter(|a| a.status == flow_status::UNMANAGED) {
        match blob::list_blobs(client, &account.name, CONTAINER_NAME, Some(ALL_RESOURCES_PREFIX)).await {
            Ok(blobs) => work.extend(
                blobs
                    .into_iter()
                    .filter(|item| item.last_modified.is_some_and(|modified| modified >= lower_bound))
                    .map(|item| (account.name.clone(), item.name)),
            ),
            Err(error) => {
                failed.push(account.name.clone());
                log(&format!("Warning: could not list unmanaged flow logs in {}: {error:#}", account.name));
            }
        }
    }

    let accounts_read = {
        let mut names: Vec<&str> = work.iter().map(|(account, _)| account.as_str()).collect();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        names.len()
    };

    let outcomes: Vec<(String, Result<()>)> = stream::iter(work)
        .map(|(account, name)| async move {
            let result = read_one(client, &account, &name, aggregator).await;
            if let Err(error) = &result {
                log(&format!("Warning: skipped {account}/{name}: {error:#}"));
            }
            (account, result)
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    let mut processed = 0;
    let mut failures = 0;
    for (account, result) in outcomes {
        match result {
            Ok(()) => processed += 1,
            Err(_) => {
                failures += 1;
                failed.push(account);
            }
        }
    }

    log(&format!("Read {processed} blob(s) from {accounts_read} storage account(s); {failures} failed."));

    failed.sort_by_key(|name| name.to_ascii_lowercase());
    failed.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(ReadSummary { blobs: processed, failed_accounts: failed })
}

async fn read_one(
    client: &AzureClient,
    account: &str,
    name: &str,
    aggregator: &Mutex<TrafficAggregator>,
) -> Result<()> {
    let text = blob::download_text(client, account, CONTAINER_NAME, name).await?;
    let records = parse_flow_log(&text)?;
    let mut guard = aggregator.lock().map_err(|_| anyhow!("aggregator lock poisoned"))?;
    guard.add_records(&records);
    Ok(())
}

/// Every UTC calendar day from the day of `from` to the day of `to`, inclusive.
pub fn days_in_window(from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<NaiveDate> {
    let end = to.date_naive();
    let mut days = Vec::new();
    let mut day = from.date_naive();
    while day <= end {
        days.push(day);
        match day.succ_opt() {
            Some(next) => day = next,
            None => break,
        }
    }
    days
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn window_covers_each_utc_day_inclusively() {
        let days = days_in_window(at("2026-10-18T23:30:00Z"), at("2026-10-20T00:10:00Z"));
        assert_eq!(
            days,
            vec![
                NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
                NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(),
                NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
            ]
        );
    }

    #[test]
    fn same_day_window_has_one_day() {
        let days = days_in_window(at("2026-10-19T01:00:00Z"), at("2026-10-19T02:00:00Z"));
        assert_eq!(days, vec![NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()]);
    }

    #[test]
    fn inverted_window_has_no_days() {
        assert!(days_in_window(at("2026-10-20T00:00:00Z"), at("2026-10-19T00:00:00Z")).is_empty());
    }
}
