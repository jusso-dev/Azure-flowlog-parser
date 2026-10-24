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
    /// Storage firewall and key settings used by diagnose. Omitted from discover JSON.
    #[serde(skip)]
    pub public_network_access: Option<String>,
    #[serde(skip)]
    pub network_default_action: Option<String>,
    #[serde(skip)]
    pub allows_trusted_services: Option<bool>,
    #[serde(skip)]
    pub allow_shared_key_access: Option<bool>,
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
    /// NSG or VNet name (last segment of target). Defaulted so JSON written before this field still loads.
    #[serde(default)]
    pub target_name: String,
    /// Resource group of the target. Defaulted so JSON written before this field still loads.
    #[serde(default)]
    pub target_resource_group: String,
    pub enabled: bool,
    pub retention_days: u32,
    pub traffic_analytics: bool,
    pub status: String,
    pub latest_blob_utc: Option<String>,
    pub age_hours: Option<f64>,
    pub message: String,
    /// Flow log provisioning state and identity, used by diagnose. Omitted from discover JSON.
    #[serde(skip)]
    pub provisioning_state: Option<String>,
    #[serde(skip)]
    pub has_managed_identity: bool,
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

/// A subscription as ARM reports it. The display name and tenant are empty when ARM does not return them.
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionRef {
    pub id: String,
    pub display_name: String,
    pub tenant_id: String,
}

/// Subscription entry in the report JSON (ReportModel.subscriptions). Also the shape of the topology file entries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSubscription {
    pub subscription_id: String,
    pub display_name: String,
    pub tenant_id: String,
}

impl From<&SubscriptionRef> for ReportSubscription {
    fn from(subscription: &SubscriptionRef) -> Self {
        Self {
            subscription_id: subscription.id.clone(),
            display_name: subscription.display_name.clone(),
            tenant_id: subscription.tenant_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VnetInfo {
    pub id: String,
    pub name: String,
    pub resource_group: String,
    pub subscription_id: String,
    pub address_prefixes: Vec<String>,
    pub subnets: Vec<SubnetInfo>,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubnetInfo {
    pub address_prefixes: Vec<String>,
    pub nsg_id: Option<String>,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NicInfo {
    pub nsg_id: Option<String>,
    pub private_ips: Vec<String>,
    pub id: String,
    pub name: String,
    pub location: String,
    pub subnet_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetworkWatcherRef {
    pub subscription_id: String,
    pub name: String,
    pub location: String,
}

#[derive(Debug, Clone, Default)]
pub struct AzureTopology {
    pub subscriptions: Vec<SubscriptionRef>,
    pub storage_accounts: Vec<StorageAccountInfo>,
    pub flow_logs: Vec<FlowLogConfigInfo>,
    pub vnets: Vec<VnetInfo>,
    pub nics: Vec<NicInfo>,
    pub network_watchers: Vec<NetworkWatcherRef>,
    pub warnings: Vec<String>,
}

// ---------- Diagnose ----------

/// Why a virtual network, subnet, NIC, or NSG is not writing flow logs to a storage account.
pub mod diagnosis_reason {
    pub const NOT_CONFIGURED: &str = "NotConfigured";
    pub const NETWORK_WATCHER_MISSING: &str = "NetworkWatcherMissing";
    pub const DISABLED: &str = "Disabled";
    pub const PROVISIONING_FAILED: &str = "ProvisioningFailed";
    pub const TARGET_MISSING: &str = "TargetMissing";
    pub const STORAGE_MISSING: &str = "StorageMissing";
    pub const REGION_MISMATCH: &str = "RegionMismatch";
    pub const UNSUPPORTED_STORAGE: &str = "UnsupportedStorage";
    pub const STORAGE_FIREWALL: &str = "StorageFirewall";
    pub const SHARED_KEY_DISABLED: &str = "SharedKeyDisabled";
    pub const ERROR: &str = "Error";
    pub const ACCESS_DENIED: &str = "AccessDenied";
    pub const NO_DATA: &str = "NoData";
    pub const STALE: &str = "Stale";

    /// Worst first. Used when several enabled flow logs target one source and none is delivering.
    const SEVERITY: [&str; 14] = [
        PROVISIONING_FAILED,
        TARGET_MISSING,
        STORAGE_MISSING,
        REGION_MISMATCH,
        UNSUPPORTED_STORAGE,
        STORAGE_FIREWALL,
        SHARED_KEY_DISABLED,
        ERROR,
        ACCESS_DENIED,
        NO_DATA,
        STALE,
        DISABLED,
        NETWORK_WATCHER_MISSING,
        NOT_CONFIGURED,
    ];

    pub fn rank(reason: &str) -> usize {
        SEVERITY.iter().position(|item| *item == reason).unwrap_or(SEVERITY.len())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowLogFinding {
    pub subscription_id: String,
    pub resource_group: String,
    pub location: String,
    pub kind: String,
    pub name: String,
    pub resource_id: String,
    pub reason: String,
    pub flow_log: String,
    pub storage_account: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisResult {
    pub generated_utc: String,
    pub stale_after_hours: u32,
    pub source_count: usize,
    pub finding_count: usize,
    pub findings: Vec<FlowLogFinding>,
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
    pub subscriptions: Vec<ReportSubscription>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_json_places_subscriptions_after_source_and_carries_target_fields() {
        let model = ReportModel {
            source: "azure".into(),
            subscriptions: vec![ReportSubscription {
                subscription_id: "sub-1".into(),
                display_name: "Prod".into(),
                tenant_id: "tenant-1".into(),
            }],
            flow_logs: vec![FlowLogConfigInfo {
                target_name: "web-nsg".into(),
                target_resource_group: "rg-app".into(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let text = serde_json::to_string(&model).unwrap();
        assert!(text.find("\"source\"") < text.find("\"subscriptions\""));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            json["subscriptions"],
            serde_json::json!([{ "subscriptionId": "sub-1", "displayName": "Prod", "tenantId": "tenant-1" }])
        );
        assert_eq!(json["flowLogs"][0]["targetName"], "web-nsg");
        assert_eq!(json["flowLogs"][0]["targetResourceGroup"], "rg-app");
    }

    #[test]
    fn flow_log_json_written_before_target_fields_still_loads() {
        let json = r#"{ "subscriptionId": "sub-1", "name": "fl", "storageAccount": "st", "networkWatcher": "nw",
            "location": "australiaeast", "target": "/x", "targetKind": "NSG", "enabled": true, "retentionDays": 0,
            "trafficAnalytics": false, "status": "Healthy", "message": "" }"#;

        let config: FlowLogConfigInfo = serde_json::from_str(json).unwrap();

        assert_eq!((config.target_name.as_str(), config.target_resource_group.as_str()), ("", ""));
    }

    #[test]
    fn diagnostic_fields_stay_out_of_inventory_json() {
        let account = StorageAccountInfo {
            allows_trusted_services: Some(false),
            public_network_access: Some("Disabled".into()),
            ..Default::default()
        };
        let account_json = serde_json::to_string(&account).unwrap();
        assert!(!account_json.contains("allowsTrustedServices"));
        assert!(!account_json.contains("publicNetworkAccess"));

        let config = FlowLogConfigInfo {
            provisioning_state: Some("Failed".into()),
            has_managed_identity: true,
            ..Default::default()
        };
        let config_json = serde_json::to_string(&config).unwrap();
        assert!(!config_json.contains("provisioningState"));
        assert!(!config_json.contains("hasManagedIdentity"));
    }
}
