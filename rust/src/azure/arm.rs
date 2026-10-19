//! ARM list calls (management.azure.com) with nextLink paging, mapped to the shared model types. Raw JSON mapping
//! is kept in pure functions so the tests can run against fixtures in `azure/fixtures/`.
//!
//! Versions and doc URLs (verified on Microsoft Learn):
//! - Subscriptions List, api-version 2022-12-01: https://learn.microsoft.com/en-us/rest/api/resources/subscriptions/list
//! - Storage Accounts List, api-version 2026-09-01: https://learn.microsoft.com/en-us/rest/api/storagerp/storage-accounts/list
//! - Network Watchers List All, api-version 2026-01-01: https://learn.microsoft.com/en-us/rest/api/network-watcher/network-watchers/list-all
//! - Flow Logs List (per watcher), api-version 2026-01-01: https://learn.microsoft.com/en-us/rest/api/network-watcher/flow-logs/list
//! - Virtual Networks List All, api-version 2026-01-01: https://learn.microsoft.com/en-us/rest/api/virtualnetwork/virtual-networks/list-all
//! - Network Interfaces List All, api-version 2026-01-01: https://learn.microsoft.com/en-us/rest/api/virtualnetwork/network-interfaces/list-all
//! - Paging through nextLink: https://learn.microsoft.com/en-us/rest/api/azure/
//!
//! The Get Subscription call by id reuses the 2022-12-01 version of the subscriptions API (not fetched separately).

use std::collections::HashSet;
use std::future::Future;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::auth::RESOURCE_ARM;
use super::client::AzureClient;
use super::http::{describe_failure, ensure_success, send_with_retry, transport_failure};
use crate::flowlog::paths::target_kind;
use crate::model::{
    AzureTopology, FlowLogConfigInfo, NicInfo, StorageAccountInfo, SubnetInfo, SubscriptionRef, VnetInfo,
};

const ARM: &str = "https://management.azure.com";
const SUBSCRIPTIONS_API: &str = "2022-12-01";
const STORAGE_API: &str = "2026-09-01";
const NETWORK_API: &str = "2026-01-01";

/// One page of an ARM list response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    #[serde(default = "Vec::new")]
    pub value: Vec<T>,
    pub next_link: Option<String>,
}

/// Follows nextLink until it is absent. `fetch` takes the full URL of a page (the first URL includes api-version;
/// nextLink URLs already carry it). A repeated link is an error so a misbehaving service cannot loop forever.
pub async fn collect_pages<T, F, Fut>(first_url: String, mut fetch: F) -> Result<Vec<T>>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Page<T>>>,
{
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut next = Some(first_url);
    while let Some(url) = next.take() {
        if !seen.insert(url.clone()) {
            bail!("ARM paging returned a link that was already followed");
        }
        let page = fetch(url).await?;
        items.extend(page.value);
        next = page.next_link.filter(|link| !link.is_empty());
    }
    Ok(items)
}

// ---------- Raw ARM shapes (only the fields the tool reads) ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionItem {
    pub subscription_id: String,
    pub display_name: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StorageAccountItem {
    pub id: String,
    pub name: String,
    pub location: String,
    pub kind: Option<String>,
    pub sku: Option<Sku>,
}

#[derive(Debug, Deserialize)]
pub struct Sku {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WatcherItem {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct FlowLogItem {
    pub name: String,
    pub location: Option<String>,
    #[serde(default)]
    pub properties: FlowLogProperties,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowLogProperties {
    pub storage_id: Option<String>,
    pub target_resource_id: Option<String>,
    pub enabled: Option<bool>,
    pub retention_policy: Option<RetentionPolicy>,
    pub flow_analytics_configuration: Option<TrafficAnalyticsProperties>,
}

#[derive(Debug, Deserialize)]
pub struct RetentionPolicy {
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficAnalyticsProperties {
    pub network_watcher_flow_analytics_configuration: Option<TrafficAnalyticsConfiguration>,
}

#[derive(Debug, Deserialize)]
pub struct TrafficAnalyticsConfiguration {
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct VnetItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub properties: VnetProperties,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VnetProperties {
    pub address_space: Option<AddressSpace>,
    #[serde(default)]
    pub subnets: Vec<SubnetItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressSpace {
    #[serde(default)]
    pub address_prefixes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SubnetItem {
    #[serde(default)]
    pub properties: SubnetProperties,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubnetProperties {
    pub address_prefix: Option<String>,
    #[serde(default)]
    pub address_prefixes: Vec<String>,
    pub network_security_group: Option<IdReference>,
}

#[derive(Debug, Deserialize)]
pub struct IdReference {
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NicItem {
    #[serde(default)]
    pub properties: NicProperties,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NicProperties {
    pub network_security_group: Option<IdReference>,
    #[serde(default)]
    pub ip_configurations: Vec<IpConfiguration>,
}

#[derive(Debug, Deserialize)]
pub struct IpConfiguration {
    pub properties: Option<IpConfigurationProperties>,
}

#[derive(Debug, Deserialize)]
pub struct IpConfigurationProperties {
    #[serde(rename = "privateIPAddress")]
    pub private_ip_address: Option<String>,
}

// ---------- Mapping (pure) ----------

/// Enabled subscriptions only, as the reference does when no explicit list is given.
pub fn enabled_subscriptions(items: Vec<SubscriptionItem>) -> Vec<SubscriptionRef> {
    items
        .into_iter()
        .filter(|item| item.state.as_deref() == Some("Enabled"))
        .map(|item| SubscriptionRef {
            display_name: item.display_name.unwrap_or_else(|| item.subscription_id.clone()),
            id: item.subscription_id,
        })
        .collect()
}

pub fn storage_account(subscription_id: &str, item: &StorageAccountItem) -> StorageAccountInfo {
    StorageAccountInfo {
        subscription_id: subscription_id.to_string(),
        name: item.name.clone(),
        resource_group: resource_group_of(&item.id),
        location: item.location.clone(),
        kind: item.kind.clone().unwrap_or_default(),
        sku_name: item.sku.as_ref().and_then(|sku| sku.name.clone()).unwrap_or_default(),
        ..Default::default()
    }
}

/// Flow log configuration with its storage account (last segment of storageId). Entries without a storage id or a
/// target are dropped, as in the reference. Health fields are filled later by the health check.
pub fn flow_log_config(subscription_id: &str, watcher_name: &str, item: FlowLogItem) -> Option<FlowLogConfigInfo> {
    let properties = item.properties;
    let storage_id = properties.storage_id.filter(|value| !value.is_empty())?;
    let target = properties.target_resource_id.filter(|value| !value.is_empty())?;
    let kind = target_kind(&target).to_string();
    let traffic_analytics = properties
        .flow_analytics_configuration
        .and_then(|config| config.network_watcher_flow_analytics_configuration)
        .and_then(|config| config.enabled)
        .unwrap_or(false);
    Some(FlowLogConfigInfo {
        subscription_id: subscription_id.to_string(),
        name: item.name,
        storage_account: last_segment(&storage_id),
        network_watcher: watcher_name.to_string(),
        location: item.location.unwrap_or_default(),
        target,
        target_kind: kind,
        enabled: properties.enabled.unwrap_or(false),
        retention_days: properties.retention_policy.and_then(|policy| policy.days).unwrap_or(0),
        traffic_analytics,
        ..Default::default()
    })
}

pub fn vnet(subscription_id: &str, item: VnetItem) -> VnetInfo {
    let address_prefixes = item.properties.address_space.map(|space| space.address_prefixes).unwrap_or_default();
    let subnets = item
        .properties
        .subnets
        .into_iter()
        .map(|subnet| {
            let properties = subnet.properties;
            // Same rule as the reference: the list form wins when present, otherwise the single prefix.
            let prefixes = if properties.address_prefixes.is_empty() {
                properties.address_prefix.into_iter().collect()
            } else {
                properties.address_prefixes
            };
            SubnetInfo {
                address_prefixes: prefixes.into_iter().filter(|prefix| !prefix.is_empty()).collect(),
                nsg_id: properties.network_security_group.and_then(|nsg| nsg.id),
            }
        })
        .collect();
    VnetInfo {
        resource_group: resource_group_of(&item.id),
        id: item.id,
        name: item.name,
        subscription_id: subscription_id.to_string(),
        address_prefixes,
        subnets,
    }
}

pub fn nic(item: NicItem) -> NicInfo {
    NicInfo {
        nsg_id: item.properties.network_security_group.and_then(|nsg| nsg.id),
        private_ips: item
            .properties
            .ip_configurations
            .into_iter()
            .filter_map(|config| config.properties.and_then(|properties| properties.private_ip_address))
            .filter(|ip| !ip.is_empty())
            .collect(),
    }
}

/// Resource group from an ARM id: the segment after "resourceGroups" (case-insensitive). Empty when absent.
pub fn resource_group_of(id: &str) -> String {
    let parts: Vec<&str> = id.split('/').filter(|part| !part.is_empty()).collect();
    parts
        .windows(2)
        .find(|pair| pair[0].eq_ignore_ascii_case("resourceGroups"))
        .map(|pair| pair[1].to_string())
        .unwrap_or_default()
}

/// Last segment of an ARM id, or the input when it has no segments.
pub fn last_segment(id: &str) -> String {
    id.rsplit('/').find(|part| !part.is_empty()).unwrap_or(id).to_string()
}

fn is_guid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    groups.len() == 5
        && groups
            .iter()
            .zip(lengths)
            .all(|(group, length)| group.len() == length && group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Explicit subscription ids, de-duplicated case-insensitively in first-seen order.
fn distinct_ids(requested: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    requested.iter().filter(|id| seen.insert(id.to_ascii_lowercase())).cloned().collect()
}

// ---------- ARM calls ----------

fn transport_error(error: &reqwest::Error) -> anyhow::Error {
    anyhow!("Azure request {}", describe_failure(transport_failure(error)))
}

async fn get_json<T: DeserializeOwned>(client: &AzureClient, url: &str) -> Result<T> {
    let token = client.token(RESOURCE_ARM).await?;
    let response = send_with_retry(client.retry(), || client.http().get(url).bearer_auth(&token))
        .await
        .map_err(|error| transport_error(&error))?;
    let response = ensure_success(response).await?;
    response.json::<T>().await.context("unexpected ARM response")
}

async fn list_all<T: DeserializeOwned>(client: &AzureClient, url: String) -> Result<Vec<T>> {
    collect_pages(url, |page_url| async move { get_json::<Page<T>>(client, &page_url).await }).await
}

async fn resolve_subscriptions(client: &AzureClient, requested: &[String]) -> Result<Vec<SubscriptionRef>> {
    if requested.is_empty() {
        let items: Vec<SubscriptionItem> =
            list_all(client, format!("{ARM}/subscriptions?api-version={SUBSCRIPTIONS_API}")).await?;
        return Ok(enabled_subscriptions(items));
    }

    let mut result = Vec::new();
    for id in distinct_ids(requested) {
        if !is_guid(&id) {
            bail!("Invalid subscription id '{id}': expected a GUID");
        }
        let item: SubscriptionItem =
            get_json(client, &format!("{ARM}/subscriptions/{id}?api-version={SUBSCRIPTIONS_API}")).await?;
        result.push(SubscriptionRef { display_name: item.display_name.unwrap_or_else(|| id.clone()), id });
    }
    Ok(result)
}

/// Loads one subscription into `topology`. Data loaded before a failure is kept, as in the reference.
async fn load_subscription(
    client: &AzureClient,
    subscription: &SubscriptionRef,
    topology: &mut AzureTopology,
) -> Result<()> {
    let sub = &subscription.id;
    let providers = format!("{ARM}/subscriptions/{sub}/providers");

    let accounts: Vec<StorageAccountItem> =
        list_all(client, format!("{providers}/Microsoft.Storage/storageAccounts?api-version={STORAGE_API}")).await?;
    topology.storage_accounts.extend(accounts.iter().map(|account| storage_account(sub, account)));

    let watchers: Vec<WatcherItem> =
        list_all(client, format!("{providers}/Microsoft.Network/networkWatchers?api-version={NETWORK_API}")).await?;
    for watcher in &watchers {
        let watcher_name = last_segment(&watcher.id);
        let url = format!(
            "{ARM}/subscriptions/{sub}/resourceGroups/{}/providers/Microsoft.Network/networkWatchers/{watcher_name}/flowLogs?api-version={NETWORK_API}",
            resource_group_of(&watcher.id)
        );
        let logs: Vec<FlowLogItem> = list_all(client, url).await?;
        topology.flow_logs.extend(logs.into_iter().filter_map(|log| flow_log_config(sub, &watcher_name, log)));
    }

    let vnets: Vec<VnetItem> =
        list_all(client, format!("{providers}/Microsoft.Network/virtualNetworks?api-version={NETWORK_API}")).await?;
    topology.vnets.extend(vnets.into_iter().map(|vnet_item| vnet(sub, vnet_item)));

    let nics: Vec<NicItem> =
        list_all(client, format!("{providers}/Microsoft.Network/networkInterfaces?api-version={NETWORK_API}")).await?;
    topology.nics.extend(nics.into_iter().map(nic));
    Ok(())
}

/// Subscriptions, storage accounts, flow log configurations, VNets and NICs. Per-subscription HTTP errors become
/// warnings; other errors (credentials, invalid input, explicit subscription lookups) stop the run.
pub async fn load_topology(client: &AzureClient, requested: &[String]) -> Result<AzureTopology> {
    let mut topology = AzureTopology::default();
    for subscription in resolve_subscriptions(client, requested).await? {
        topology.subscriptions.push(subscription.clone());
        if let Err(error) = load_subscription(client, &subscription, &mut topology).await {
            let Some(http) = error.downcast_ref::<super::http::AzureHttpError>() else {
                return Err(error);
            };
            let detail = http.code.clone().unwrap_or_else(|| http.message.clone());
            topology.warnings.push(format!(
                "Subscription {} ({}): {} {}",
                subscription.id, subscription.display_name, http.status, detail
            ));
        }
    }
    Ok(topology)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const SUBSCRIPTIONS: &str = include_str!("fixtures/subscriptions.json");
    const STORAGE_ACCOUNTS: &str = include_str!("fixtures/storage_accounts.json");
    const FLOW_LOGS: &str = include_str!("fixtures/flow_logs.json");
    const VNETS: &str = include_str!("fixtures/virtual_networks.json");
    const NICS: &str = include_str!("fixtures/network_interfaces.json");

    fn parse<T: DeserializeOwned>(json: &str) -> Vec<T> {
        serde_json::from_str::<Page<T>>(json).expect("fixture parses").value
    }

    #[test]
    fn enabled_subscriptions_filter_out_disabled() {
        let subs = enabled_subscriptions(parse::<SubscriptionItem>(SUBSCRIPTIONS));
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(subs[0].display_name, "Prod Network");
    }

    #[test]
    fn storage_accounts_map_name_group_kind_and_sku() {
        let accounts = parse::<StorageAccountItem>(STORAGE_ACCOUNTS);
        let mapped: Vec<StorageAccountInfo> = accounts.iter().map(|a| storage_account("sub-1", a)).collect();
        assert_eq!(mapped[0].name, "stflowlogs01");
        assert_eq!(mapped[0].resource_group, "rg-net");
        assert_eq!(mapped[0].kind, "StorageV2");
        assert_eq!(mapped[0].sku_name, "Standard_GRS");
        assert_eq!(mapped[0].location, "australiaeast");
        assert_eq!(mapped[0].subscription_id, "sub-1");
        assert_eq!(mapped[1].sku_name, "Standard_LRS");
    }

    #[test]
    fn flow_logs_map_storage_account_retention_and_analytics() {
        let items = parse::<FlowLogItem>(FLOW_LOGS);
        let mapped: Vec<FlowLogConfigInfo> = items
            .into_iter()
            .filter_map(|item| flow_log_config("sub-1", "NetworkWatcher_australiaeast", item))
            .collect();
        // The entry without a target is dropped, so two configurations remain.
        assert_eq!(mapped.len(), 2);
        let first = &mapped[0];
        assert_eq!(first.name, "flowlog-web-nsg");
        assert_eq!(first.storage_account, "stflowlogs01");
        assert_eq!(first.network_watcher, "NetworkWatcher_australiaeast");
        assert_eq!(first.target_kind, "NSG");
        assert!(first.enabled);
        assert_eq!(first.retention_days, 30);
        assert!(first.traffic_analytics);
        let second = &mapped[1];
        assert_eq!(second.target_kind, "VNet");
        assert!(!second.enabled);
        assert!(!second.traffic_analytics);
        assert_eq!(second.retention_days, 0);
    }

    #[test]
    fn vnet_subnets_use_list_prefixes_when_present() {
        let vnet_items = parse::<VnetItem>(VNETS);
        let mapped = vnet("sub-1", vnet_items.into_iter().next().unwrap());
        assert_eq!(mapped.name, "hub-vnet");
        assert_eq!(mapped.resource_group, "rg-net");
        assert_eq!(mapped.address_prefixes, vec!["10.10.0.0/16"]);
        assert_eq!(mapped.subnets.len(), 3);
        assert_eq!(mapped.subnets[0].address_prefixes, vec!["10.10.1.0/24"]);
        assert_eq!(
            mapped.subnets[0].nsg_id.as_deref(),
            Some(
                "/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-app/providers/Microsoft.Network/networkSecurityGroups/web-nsg"
            )
        );
        assert_eq!(mapped.subnets[1].address_prefixes, vec!["10.10.2.0/24", "10.10.3.0/24"]);
        assert_eq!(mapped.subnets[1].nsg_id, None);
        assert!(mapped.subnets[2].address_prefixes.is_empty());
    }

    #[test]
    fn nics_map_private_ips_and_nsg() {
        let mapped: Vec<NicInfo> = parse::<NicItem>(NICS).into_iter().map(nic).collect();
        assert_eq!(mapped[0].private_ips, vec!["10.10.1.4"]);
        assert!(mapped[0].nsg_id.as_deref().unwrap().ends_with("/web-nsg"));
        assert!(mapped[1].private_ips.is_empty());
        assert_eq!(mapped[1].nsg_id, None);
    }

    #[test]
    fn resource_group_and_last_segment_parse_arm_ids() {
        let id = "/subscriptions/s/resourceGroups/My-RG/providers/Microsoft.Storage/storageAccounts/acct";
        assert_eq!(resource_group_of(id), "My-RG");
        assert_eq!(last_segment(id), "acct");
        assert_eq!(resource_group_of("/subscriptions/s"), "");
    }

    #[test]
    fn subscription_ids_must_be_guids() {
        assert!(is_guid("00000000-0000-0000-0000-000000000001"));
        assert!(!is_guid("00000000-0000-0000-0000-00000000000"));
        assert!(!is_guid("sub/../../x"));
    }

    #[test]
    fn explicit_subscriptions_dedupe_ignoring_case() {
        let requested = vec!["ABC".to_string(), "abc".to_string(), "def".to_string()];
        assert_eq!(distinct_ids(&requested), vec!["ABC", "def"]);
    }

    /// Fake page source: each URL maps to a page of integers and an optional next link.
    fn fake_pages(pages: Vec<(&str, Vec<i32>, Option<&str>)>) -> HashMap<String, (Vec<i32>, Option<String>)> {
        pages.into_iter().map(|(url, value, next)| (url.to_string(), (value, next.map(str::to_string)))).collect()
    }

    #[test]
    fn paging_follows_next_link_until_absent() {
        let pages = fake_pages(vec![
            ("first", vec![1, 2], Some("second")),
            ("second", vec![3], Some("third")),
            ("third", vec![4, 5], None),
        ]);
        let fetched: Vec<String> = Vec::new();
        let fetched = std::cell::RefCell::new(fetched);
        let result = futures::executor::block_on(collect_pages("first".to_string(), |url| {
            fetched.borrow_mut().push(url.clone());
            let (value, next_link) = pages.get(&url).cloned().expect("known page");
            std::future::ready(Ok(Page { value, next_link }))
        }))
        .unwrap();
        assert_eq!(result, vec![1, 2, 3, 4, 5]);
        assert_eq!(fetched.into_inner(), vec!["first", "second", "third"]);
    }

    #[test]
    fn paging_stops_on_empty_next_link() {
        let result = futures::executor::block_on(collect_pages("only".to_string(), |_url| {
            std::future::ready(Ok(Page { value: vec![7], next_link: Some(String::new()) }))
        }))
        .unwrap();
        assert_eq!(result, vec![7]);
    }

    #[test]
    fn paging_rejects_a_repeated_link() {
        let result = futures::executor::block_on(collect_pages("loop".to_string(), |_url| {
            std::future::ready(Ok(Page { value: vec![1], next_link: Some("loop".to_string()) }))
        }));
        assert!(result.is_err());
    }
}
