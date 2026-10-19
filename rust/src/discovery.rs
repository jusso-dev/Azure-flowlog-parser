//! Storage account and flow log inventory with health checks. Same rules as InventoryService.cs and
//! FlowLogHealthChecker.cs: lookback of today and yesterday (UTC), Disabled / NoData / Stale / Healthy per config,
//! AccessDenied on HTTP 403, worst-status rollup per account, and Unmanaged detection for accounts without configs.

use anyhow::Result;
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use futures::stream::{self, StreamExt};

use crate::azure::AzureClient;
use crate::azure::arm;
use crate::azure::blob;
use crate::azure::http::AzureHttpError;
use crate::flowlog::paths::{ALL_RESOURCES_PREFIX, CONTAINER_NAME, day_prefix, target_from_blob_name};
use crate::model::{AzureTopology, FlowLogConfigInfo, InventoryResult, StorageAccountInfo, flow_status};
use crate::util::{iso, iso_opt, one_decimal};

/// Parallel health checks, as InventoryService.cs (MaxDegreeOfParallelism = 8).
const HEALTH_CONCURRENCY: usize = 8;

pub struct InventoryRequest {
    pub subscriptions: Vec<String>,
    pub storage_accounts: Vec<String>,
    pub stale_hours: u32,
    pub include_all: bool,
}

pub struct InventoryLoad {
    pub topology: AzureTopology,
    pub result: InventoryResult,
}

/// Progress and warnings for a command. Warnings always go to stderr; progress only with --verbose (Logger.cs).
pub fn progress_logger(verbose: bool) -> impl Fn(&str) + Sync {
    move |message: &str| {
        if verbose || message.starts_with("Warning:") {
            eprintln!("{message}");
        }
    }
}

pub async fn load_inventory(
    client: &AzureClient,
    request: &InventoryRequest,
    log: &(dyn Fn(&str) + Sync),
) -> Result<InventoryLoad> {
    let topology = arm::load_topology(client, &request.subscriptions).await?;
    for warning in &topology.warnings {
        log(&format!("Warning: {warning}"));
    }

    let mut accounts: Vec<StorageAccountInfo> = topology
        .storage_accounts
        .iter()
        .filter(|account| is_selected(&account.name, &request.storage_accounts))
        .cloned()
        .collect();
    sort_accounts(&mut accounts);

    let flow_logs: Vec<FlowLogConfigInfo> = topology
        .flow_logs
        .iter()
        .filter(|config| accounts.iter().any(|account| account.name.eq_ignore_ascii_case(&config.storage_account)))
        .cloned()
        .collect();

    let checker = Checker { client, stale_after: TimeDelta::hours(i64::from(request.stale_hours)), now: Utc::now() };
    let groups: Vec<(StorageAccountInfo, Vec<FlowLogConfigInfo>)> = accounts
        .into_iter()
        .map(|account| {
            let configs = flow_logs
                .iter()
                .filter(|config| config.storage_account.eq_ignore_ascii_case(&account.name))
                .cloned()
                .collect();
            (account, configs)
        })
        .collect();

    // buffered (not buffer_unordered) keeps the sorted account order.
    let checked: Vec<(StorageAccountInfo, Vec<FlowLogConfigInfo>, Option<String>)> = stream::iter(groups)
        .map(|(account, configs)| check_one_account(&checker, account, configs))
        .buffered(HEALTH_CONCURRENCY)
        .collect()
        .await;

    let mut checked_accounts = Vec::with_capacity(checked.len());
    let mut checked_configs = Vec::new();
    for (account, configs, warning) in checked {
        if let Some(warning) = warning {
            log(&format!("Warning: {warning}"));
        }
        checked_configs.extend(configs);
        checked_accounts.push(account);
    }
    sort_flow_logs(&mut checked_configs);
    let flow_logs = checked_configs;

    let included = checked_accounts.into_iter().filter(|account| is_included(account, request.include_all)).collect();

    Ok(InventoryLoad {
        topology,
        result: InventoryResult {
            generated_utc: iso(checker.now),
            stale_after_hours: request.stale_hours,
            accounts: included,
            flow_logs,
        },
    })
}

/// Empty filter selects every account. Names compare case-insensitively.
pub fn is_selected(name: &str, filter: &[String]) -> bool {
    filter.is_empty() || filter.iter().any(|wanted| wanted.eq_ignore_ascii_case(name))
}

/// Subscription, then name, both case-insensitive (InventoryService.cs ordering).
pub fn sort_accounts(accounts: &mut [StorageAccountInfo]) {
    accounts.sort_by(|a, b| {
        a.subscription_id
            .to_ascii_lowercase()
            .cmp(&b.subscription_id.to_ascii_lowercase())
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
}

/// Storage account, then flow log name, both case-insensitive.
pub fn sort_flow_logs(configs: &mut [FlowLogConfigInfo]) {
    configs.sort_by(|a, b| {
        a.storage_account
            .to_ascii_lowercase()
            .cmp(&b.storage_account.to_ascii_lowercase())
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
}

/// Accounts with flow logs or AccessDenied are listed. --all also lists the rest. AccessDenied stays visible
/// because its status says the check could not run.
pub fn is_included(account: &StorageAccountInfo, include_all: bool) -> bool {
    include_all || account.has_flow_logs || account.status == flow_status::ACCESS_DENIED
}

/// Health check for one account and its configurations. Returns the warning text when the check itself failed,
/// and marks the account Error in that case, as InventoryService.cs does.
async fn check_one_account(
    checker: &Checker<'_>,
    mut account: StorageAccountInfo,
    mut configs: Vec<FlowLogConfigInfo>,
) -> (StorageAccountInfo, Vec<FlowLogConfigInfo>, Option<String>) {
    let warning = match checker.check_account(&mut account, &mut configs).await {
        Ok(()) => None,
        Err(error) => {
            account.status = flow_status::ERROR.to_string();
            Some(format!("{}: {error:#}", account.name))
        }
    };
    (account, configs, warning)
}

struct Checker<'a> {
    client: &'a AzureClient,
    stale_after: TimeDelta,
    now: DateTime<Utc>,
}

struct ContainerProbe {
    present: bool,
    first_blob: Option<String>,
}

impl Checker<'_> {
    /// Mirrors FlowLogHealthChecker.CheckAccountAsync. The container check and the unmanaged probe may return 403,
    /// which makes the account AccessDenied. Other container errors fail the whole account.
    async fn check_account(&self, account: &mut StorageAccountInfo, configs: &mut [FlowLogConfigInfo]) -> Result<()> {
        let mut container_denied = false;
        let mut has_unmanaged = false;

        match self.probe_container(&account.name, configs.is_empty()).await {
            Ok(probe) => {
                account.container_present = probe.present;
                account.observed_target = probe.first_blob.as_deref().and_then(target_from_blob_name);
                has_unmanaged = probe.first_blob.is_some();
            }
            Err(error) if blob::is_access_denied(&error) => container_denied = true,
            Err(error) => return Err(error),
        }

        let mut latest_overall: Option<DateTime<Utc>> = None;
        for config in configs.iter_mut() {
            if let Some(latest) = self.check_config(config).await {
                if latest_overall.is_none_or(|current| latest > current) {
                    latest_overall = Some(latest);
                }
            }
        }

        account.flow_log_configs = configs.len() as u32;
        account.has_flow_logs = !configs.is_empty() || has_unmanaged;
        account.latest_blob_utc = iso_opt(latest_overall);
        account.status = account_status(container_denied, configs, has_unmanaged).to_string();
        Ok(())
    }

    async fn probe_container(&self, account: &str, check_unmanaged: bool) -> Result<ContainerProbe> {
        let present = blob::container_exists(self.client, account, CONTAINER_NAME).await?;
        let first_blob = if present && check_unmanaged {
            blob::list_page(self.client, account, CONTAINER_NAME, Some(ALL_RESOURCES_PREFIX), None, Some(1))
                .await?
                .blobs
                .into_iter()
                .next()
                .map(|item| item.name)
        } else {
            None
        };
        Ok(ContainerProbe { present, first_blob })
    }

    /// Updates one configuration. Returns its newest blob time when one was found. Errors never escape: they
    /// become AccessDenied (403) or Error statuses on the configuration.
    async fn check_config(&self, config: &mut FlowLogConfigInfo) -> Option<DateTime<Utc>> {
        if !config.enabled {
            mark_disabled(config);
            return None;
        }
        match self.newest_blob(config).await {
            Ok(latest) => {
                apply_latest(config, latest, self.now, self.stale_after);
                latest
            }
            Err(error) => {
                match error.downcast_ref::<AzureHttpError>() {
                    Some(http) if http.is_access_denied() => {
                        config.status = flow_status::ACCESS_DENIED.to_string();
                        config.message = http.code.clone().unwrap_or_else(|| http.message.clone());
                    }
                    _ => {
                        config.status = flow_status::ERROR.to_string();
                        config.message = format!("{error:#}");
                    }
                }
                None
            }
        }
    }

    /// Newest blob modified time under today's prefix, then yesterday's. Lists the full prefix (with paging).
    async fn newest_blob(&self, config: &FlowLogConfigInfo) -> Result<Option<DateTime<Utc>>> {
        let today = self.now.date_naive();
        for day in lookback_days(today) {
            let prefix = day_prefix(&config.target, day);
            let blobs = blob::list_blobs(self.client, &config.storage_account, CONTAINER_NAME, Some(&prefix)).await?;
            let newest = blobs.iter().filter_map(|item| item.last_modified).max();
            if newest.is_some() {
                return Ok(newest);
            }
        }
        Ok(None)
    }
}

/// Today (UTC) and yesterday, in the order the health check searches them.
pub fn lookback_days(today: NaiveDate) -> [NaiveDate; 2] {
    [today, today.pred_opt().unwrap_or(today)]
}

/// Disabled configurations are not read. The message says so.
pub fn mark_disabled(config: &mut FlowLogConfigInfo) {
    config.status = flow_status::DISABLED.to_string();
    config.message = "Flow log is disabled; no blobs are read.".to_string();
}

/// Sets latest time, age, status and message for an enabled configuration from its newest blob time.
pub fn apply_latest(
    config: &mut FlowLogConfigInfo,
    latest: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    stale_after: TimeDelta,
) {
    config.latest_blob_utc = iso_opt(latest);
    match latest {
        None => {
            config.status = flow_status::NO_DATA.to_string();
            config.message = "No flow log blobs found in the last 2 days.".to_string();
        }
        Some(latest) => {
            let age = now - latest;
            let age_hours = round2((age.num_milliseconds().max(0) as f64) / 3_600_000.0);
            config.age_hours = Some(age_hours);
            let stale = age > stale_after;
            config.status = if stale { flow_status::STALE } else { flow_status::HEALTHY }.to_string();
            config.message =
                if stale { format!("Newest blob is {} hours old.", one_decimal(age_hours)) } else { String::new() };
        }
    }
}

/// Account status: AccessDenied when the container check was denied, otherwise the worst configuration status,
/// otherwise Unmanaged when blobs exist without configuration, otherwise NoFlowLogs.
pub fn account_status(container_denied: bool, configs: &[FlowLogConfigInfo], has_unmanaged: bool) -> &'static str {
    if container_denied {
        flow_status::ACCESS_DENIED
    } else if !configs.is_empty() {
        flow_status::worst(configs.iter().map(|config| config.status.as_str()))
    } else if has_unmanaged {
        flow_status::UNMANAGED
    } else {
        flow_status::NO_FLOW_LOGS
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn config(name: &str, enabled: bool) -> FlowLogConfigInfo {
        FlowLogConfigInfo {
            name: name.to_string(),
            storage_account: "stflowlogs01".to_string(),
            enabled,
            ..Default::default()
        }
    }

    fn account(name: &str, sub: &str) -> StorageAccountInfo {
        StorageAccountInfo { name: name.to_string(), subscription_id: sub.to_string(), ..Default::default() }
    }

    #[test]
    fn enabled_config_with_recent_blob_is_healthy() {
        let now = at("2026-10-19T12:00:00Z");
        let mut cfg = config("a", true);
        apply_latest(&mut cfg, Some(at("2026-10-19T11:00:00Z")), now, TimeDelta::hours(3));
        assert_eq!(cfg.status, "Healthy");
        assert_eq!(cfg.age_hours, Some(1.0));
        assert_eq!(cfg.latest_blob_utc.as_deref(), Some("2026-10-19T11:00:00Z"));
        assert_eq!(cfg.message, "");
    }

    #[test]
    fn blob_older_than_stale_window_is_stale_with_message() {
        let now = at("2026-10-19T12:00:00Z");
        let mut cfg = config("a", true);
        apply_latest(&mut cfg, Some(at("2026-10-19T07:00:00Z")), now, TimeDelta::hours(3));
        assert_eq!(cfg.status, "Stale");
        assert_eq!(cfg.age_hours, Some(5.0));
        assert_eq!(cfg.message, "Newest blob is 5.0 hours old.");
    }

    #[test]
    fn exactly_at_the_stale_limit_is_still_healthy() {
        let now = at("2026-10-19T12:00:00Z");
        let mut cfg = config("a", true);
        apply_latest(&mut cfg, Some(at("2026-10-19T09:00:00Z")), now, TimeDelta::hours(3));
        assert_eq!(cfg.status, "Healthy");
    }

    #[test]
    fn no_blobs_is_no_data() {
        let now = at("2026-10-19T12:00:00Z");
        let mut cfg = config("a", true);
        apply_latest(&mut cfg, None, now, TimeDelta::hours(3));
        assert_eq!(cfg.status, "NoData");
        assert_eq!(cfg.latest_blob_utc, None);
        assert_eq!(cfg.age_hours, None);
    }

    #[test]
    fn disabled_config_is_marked_without_reading() {
        let mut cfg = config("a", false);
        mark_disabled(&mut cfg);
        assert_eq!(cfg.status, "Disabled");
        assert_eq!(cfg.message, "Flow log is disabled; no blobs are read.");
    }

    #[test]
    fn lookback_is_today_then_yesterday() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(
            lookback_days(today),
            [NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()]
        );
    }

    #[test]
    fn account_status_worst_order_error_access_nodata_stale_disabled_healthy() {
        let mut configs = vec![config("a", true), config("b", true)];
        configs[0].status = "Healthy".into();
        configs[1].status = "Stale".into();
        assert_eq!(account_status(false, &configs, false), "Stale");
        configs[1].status = "Disabled".into();
        assert_eq!(account_status(false, &configs, false), "Disabled");
        configs[1].status = "NoData".into();
        assert_eq!(account_status(false, &configs, false), "NoData");
        configs[1].status = "AccessDenied".into();
        assert_eq!(account_status(false, &configs, false), "AccessDenied");
        configs[1].status = "Error".into();
        assert_eq!(account_status(false, &configs, false), "Error");
    }

    #[test]
    fn container_denial_overrides_configuration_statuses() {
        let mut configs = vec![config("a", true)];
        configs[0].status = "Healthy".into();
        assert_eq!(account_status(true, &configs, false), "AccessDenied");
    }

    #[test]
    fn no_configs_is_unmanaged_when_blobs_exist_else_no_flow_logs() {
        assert_eq!(account_status(false, &[], true), "Unmanaged");
        assert_eq!(account_status(false, &[], false), "NoFlowLogs");
    }

    #[test]
    fn inclusion_follows_flow_logs_access_denied_and_include_all() {
        let mut plain = account("plain", "s");
        assert!(!is_included(&plain, false));
        assert!(is_included(&plain, true));
        plain.has_flow_logs = true;
        assert!(is_included(&plain, false));
        let mut denied = account("denied", "s");
        denied.status = "AccessDenied".into();
        assert!(is_included(&denied, false));
    }

    #[test]
    fn accounts_sort_by_subscription_then_name_ignoring_case() {
        let mut accounts = vec![account("beta", "S2"), account("Alpha", "s1"), account("alpha2", "s1")];
        sort_accounts(&mut accounts);
        let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "alpha2", "beta"]);
    }

    #[test]
    fn account_filter_is_case_insensitive_and_empty_selects_all() {
        assert!(is_selected("Acct", &[]));
        assert!(is_selected("Acct", &["acct".to_string()]));
        assert!(!is_selected("other", &["acct".to_string()]));
    }

    #[test]
    fn progress_logger_is_silent_without_verbose_except_warnings() {
        // The closure writes to stderr; this test only checks that it can be built and called.
        let log = progress_logger(false);
        log("progress line");
        log("Warning: example");
    }
}
