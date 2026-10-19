//! Shared data types and the cross-tool JSON contract.
//!
//! Report and inventory output use camelCase (see the SPEC in the repo history). DenormalizedRecord keeps the
//! legacy field names of the .NET DenormalizedFlowRecord, because the parse command writes them.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
/// Maximum aggregated rows embedded in a report. Totals always cover every observation.
pub const MAX_ROWS: usize = 20_000;

/// Health states. Strings because they appear in JSON output.
pub mod flow_status {
    pub const HEALTHY: &str = "Healthy";
    pub const STALE: &str = "Stale";
    pub const NO_DATA: &str = "NoData";
    pub const DISABLED: &str = "Disabled";
    pub const UNMANAGED: &str = "Unmanaged";
    pub const NO_FLOW_LOGS: &str = "NoFlowLogs";
    pub const ACCESS_DENIED: &str = "AccessDenied";
    pub const ERROR: &str = "Error";

    /// Worst first. Unmanaged and NoFlowLogs only apply to accounts without flow log configurations.
    const SEVERITY: [&str; 6] = [ERROR, ACCESS_DENIED, NO_DATA, STALE, DISABLED, HEALTHY];

    /// Most severe status in the list, or Healthy when the list is empty.
    pub fn worst<'a, I: IntoIterator<Item = &'a str>>(statuses: I) -> &'static str {
        let list: Vec<&str> = statuses.into_iter().collect();
        SEVERITY.iter().copied().find(|s| list.contains(s)).unwrap_or(HEALTHY)
    }
}

/// Scope kinds used in reports.
pub mod scope_kind {
    pub const STORAGE: &str = "storage";
    pub const VNET: &str = "vnet";
    pub const INTERNET: &str = "internet";
    pub const UNMAPPED: &str = "unmapped";
}

// ---------- Inventory (discover) ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageAccountInfo {
    pub subscription_id: String,
    pub name: String,
    pub resource_group: String,
    pub location: String,
    pub kind: String,
    pub sku_name: String,
    pub flow_log_configs: u32,
    pub container_present: bool,
    pub has_flow_logs: bool,
    pub status: String,
    pub latest_blob_utc: Option<String>,
    pub observed_target: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowLogConfigInfo {
    pub subscription_id: String,
    pub name: String,
    pub storage_account: String,
    pub network_watcher: String,
    pub location: String,
    pub target: String,
    pub target_kind: String,
    pub enabled: bool,
    pub retention_days: u32,
    pub traffic_analytics: bool,
    pub status: String,
    pub latest_blob_utc: Option<String>,
    pub age_hours: Option<f64>,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryResult {
    pub generated_utc: String,
    pub stale_after_hours: u32,
    pub accounts: Vec<StorageAccountInfo>,
    pub flow_logs: Vec<FlowLogConfigInfo>,
}

// ---------- Azure topology (raw ARM data, no health) ----------

#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionRef {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VnetInfo {
    pub id: String,
    pub name: String,
    pub resource_group: String,
    pub subscription_id: String,
    pub address_prefixes: Vec<String>,
    pub subnets: Vec<SubnetInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubnetInfo {
    pub address_prefixes: Vec<String>,
    pub nsg_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NicInfo {
    pub nsg_id: Option<String>,
    pub private_ips: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AzureTopology {
    pub subscriptions: Vec<SubscriptionRef>,
    pub storage_accounts: Vec<StorageAccountInfo>,
    pub flow_logs: Vec<FlowLogConfigInfo>,
    pub vnets: Vec<VnetInfo>,
    pub nics: Vec<NicInfo>,
    pub warnings: Vec<String>,
}

// ---------- Flow log observations ----------

/// One flow tuple from a PT1H.json record. Field names match the .NET DenormalizedFlowRecord JSON output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DenormalizedRecord {
    pub time: String,
    pub category: String,
    pub operation_name: String,
    pub resource_id: String,
    pub version: u32,
    pub nsg_rule_name: String,
    pub mac: String,
    pub start_time: String,
    pub source_address: String,
    pub destination_address: String,
    pub source_port: String,
    pub destination_port: String,
    pub transport_protocol: String,
    pub device_direction: String,
    pub device_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow_state: Option<String>,
    #[serde(rename = "packetsStoD", skip_serializing_if = "Option::is_none")]
    pub packets_s_to_d: Option<String>,
    #[serde(rename = "bytesStoD", skip_serializing_if = "Option::is_none")]
    pub bytes_s_to_d: Option<String>,
    #[serde(rename = "packetsDtoS", skip_serializing_if = "Option::is_none")]
    pub packets_d_to_s: Option<String>,
    #[serde(rename = "bytesDtoS", skip_serializing_if = "Option::is_none")]
    pub bytes_d_to_s: Option<String>,
}

// ---------- Traffic report (contract shared with .NET and PowerShell) ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportModel {
    pub schema: u32,
    pub generated_utc: String,
    pub source: String,
    pub window: ReportWindow,
    pub filters: ReportFilters,
    pub summary: ReportSummary,
    pub scopes: Vec<ReportScope>,
    pub flows: Vec<ReportFlow>,
    pub accounts: Vec<StorageAccountInfo>,
    pub flow_logs: Vec<FlowLogConfigInfo>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportWindow {
    pub from_utc: String,
    pub to_utc: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportFilters {
    pub action: String,
    pub protocol: String,
    pub ports: String,
    pub ip: String,
    pub min_bytes: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub flows: i64,
    pub allowed: i64,
    pub denied: i64,
    pub bytes: i64,
    pub packets: i64,
    pub rows: usize,
    pub rows_total: usize,
    pub rows_truncated: bool,
    pub storage_accounts: usize,
    pub scopes: usize,
    pub subscriptions: usize,
    pub observations_skipped: i64,
    pub unique_source_ips: usize,
    pub unique_destination_ips: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportScope {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub subscription_id: String,
    pub resource_group: String,
    pub storage_account: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportFlow {
    pub first: String,
    pub last: String,
    pub src: String,
    pub dst: String,
    pub src_scope: String,
    pub dst_scope: String,
    pub proto: String,
    pub dst_port: u32,
    pub action: String,
    pub direction: String,
    pub rule: String,
    pub flows: i64,
    pub packets: i64,
    pub bytes: i64,
}
