//! Which virtual networks, subnets, NICs, and attached NSGs are not writing flow logs to storage, and why.
//! An enabled flow log on a NIC wins over its subnet, which wins over its virtual network. A delivering
//! parent covers its children. A delivering flow log is enabled, Healthy, and has a blob timestamp.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::flowlog::paths::{self, CONTAINER_NAME};
use crate::model::diagnosis_reason::{self, rank};
use crate::model::flow_status;
use crate::model::{AzureTopology, DiagnosisResult, FlowLogConfigInfo, FlowLogFinding, StorageAccountInfo};
use crate::util::iso;

const NSG_CONTAINER: &str = "insights-logs-networksecuritygroupflowevent";

struct Source {
    kind: String,
    id: String,
    name: String,
    resource_group: String,
    subscription_id: String,
    location: String,
    subnet_id: Option<String>,
    vnet_id: Option<String>,
}

struct Context<'a> {
    by_target: HashMap<String, Vec<FlowLogConfigInfo>>,
    accounts: HashMap<String, StorageAccountInfo>,
    known_ids: HashSet<String>,
    topology: &'a AzureTopology,
    stale_after_hours: u32,
}

struct Verdict {
    reason: &'static str,
    message: String,
}

/// Copies health-check results onto the topology copies. Discovery clones configs before checking them,
/// so the topology still has provisioning and firewall fields and the result has status and blob times.
pub fn apply_checked_health(
    topology: &mut AzureTopology,
    accounts: &[StorageAccountInfo],
    flow_logs: &[FlowLogConfigInfo],
) {
    for checked in flow_logs {
        if let Some(config) = topology.flow_logs.iter_mut().find(|config| {
            eq_ignore(&config.subscription_id, &checked.subscription_id)
                && eq_ignore(&config.network_watcher, &checked.network_watcher)
                && eq_ignore(&config.name, &checked.name)
        }) {
            config.status = checked.status.clone();
            config.latest_blob_utc = checked.latest_blob_utc.clone();
            config.age_hours = checked.age_hours;
            config.message = checked.message.clone();
        }
    }
    for checked in accounts {
        if let Some(account) = topology.storage_accounts.iter_mut().find(|account| {
            eq_ignore(&account.subscription_id, &checked.subscription_id) && eq_ignore(&account.name, &checked.name)
        }) {
            account.status = checked.status.clone();
        }
    }
}

pub fn diagnose(
    topology: &AzureTopology,
    flow_logs: &[FlowLogConfigInfo],
    stale_after_hours: u32,
    now: DateTime<Utc>,
    storage_accounts: &[String],
) -> DiagnosisResult {
    let logs: Vec<FlowLogConfigInfo> = flow_logs.iter().filter(|log| !log.target.trim().is_empty()).cloned().collect();
    let mut by_target: HashMap<String, Vec<FlowLogConfigInfo>> = HashMap::new();
    for log in &logs {
        by_target.entry(key(&log.target)).or_default().push(log.clone());
    }

    let mut accounts = HashMap::new();
    for account in &topology.storage_accounts {
        accounts.entry(key(&account.name)).or_insert_with(|| account.clone());
    }

    let mut known_ids = HashSet::new();
    let mut sources = Vec::new();
    for vnet in &topology.vnets {
        if vnet.id.trim().is_empty() {
            continue;
        }
        known_ids.insert(key(&vnet.id));
        sources.push(Source {
            kind: "VNet".into(),
            id: vnet.id.clone(),
            name: vnet.name.clone(),
            resource_group: vnet.resource_group.clone(),
            subscription_id: vnet.subscription_id.clone(),
            location: vnet.location.clone(),
            subnet_id: None,
            vnet_id: None,
        });
        for subnet in &vnet.subnets {
            if subnet.id.trim().is_empty() {
                continue;
            }
            known_ids.insert(key(&subnet.id));
            sources.push(Source {
                kind: "Subnet".into(),
                id: subnet.id.clone(),
                name: subnet.name.clone(),
                resource_group: vnet.resource_group.clone(),
                subscription_id: vnet.subscription_id.clone(),
                location: vnet.location.clone(),
                subnet_id: None,
                vnet_id: Some(vnet.id.clone()),
            });
        }
    }

    for nic in &topology.nics {
        if nic.id.trim().is_empty() {
            continue;
        }
        known_ids.insert(key(&nic.id));
        sources.push(Source {
            kind: "NIC".into(),
            id: nic.id.clone(),
            name: nic.name.clone(),
            resource_group: paths::resource_group_of(&nic.id),
            subscription_id: segment_after(&nic.id, "subscriptions"),
            location: nic.location.clone(),
            subnet_id: nic.subnet_id.clone(),
            vnet_id: nic.subnet_id.as_deref().and_then(vnet_id_of),
        });
    }

    let mut nsgs: HashMap<String, Source> = HashMap::new();
    let mut add_nsg = |id: &str, location: &str| {
        if id.trim().is_empty() {
            return;
        }
        let map_key = key(id);
        match nsgs.get_mut(&map_key) {
            None => {
                nsgs.insert(
                    map_key,
                    Source {
                        kind: "NSG".into(),
                        id: id.to_string(),
                        name: paths::last_segment(id),
                        resource_group: paths::resource_group_of(id),
                        subscription_id: segment_after(id, "subscriptions"),
                        location: location.to_string(),
                        subnet_id: None,
                        vnet_id: None,
                    },
                );
            }
            Some(existing) if existing.location.trim().is_empty() && !location.trim().is_empty() => {
                existing.location = location.to_string();
            }
            Some(_) => {}
        }
    };
    for vnet in &topology.vnets {
        for subnet in &vnet.subnets {
            if let Some(nsg_id) = subnet.nsg_id.as_deref() {
                add_nsg(nsg_id, &vnet.location);
            }
        }
    }
    for nic in &topology.nics {
        if let Some(nsg_id) = nic.nsg_id.as_deref() {
            add_nsg(nsg_id, &nic.location);
        }
    }
    for log in logs.iter().filter(|log| log.target_kind == "NSG") {
        add_nsg(&log.target, &log.location);
    }
    for nsg in nsgs.into_values() {
        known_ids.insert(key(&nsg.id));
        sources.push(nsg);
    }

    let context = Context { by_target, accounts, known_ids, topology, stale_after_hours };
    let mut covered = HashSet::new();
    let mut findings = Vec::new();
    for source in sources.iter().filter(|source| source.kind != "NSG") {
        match evaluate(source, &context) {
            None => {
                covered.insert(key(&source.id));
            }
            Some(finding) => findings.push(finding),
        }
    }
    for source in sources.iter().filter(|source| source.kind == "NSG") {
        let Some(finding) = evaluate(source, &context) else { continue };
        if matches!(
            finding.reason.as_str(),
            diagnosis_reason::NOT_CONFIGURED | diagnosis_reason::NETWORK_WATCHER_MISSING | diagnosis_reason::DISABLED
        ) && attachments_covered(&source.id, topology, &covered)
        {
            continue;
        }
        findings.push(finding);
    }

    let mut orphans = 0;
    for log in &logs {
        if context.known_ids.contains(&key(&log.target)) {
            continue;
        }
        orphans += 1;
        if is_delivering(log) {
            continue;
        }
        let from_target = segment_after(&log.target, "subscriptions");
        let source = Source {
            kind: if log.target_kind.is_empty() { "Other".into() } else { log.target_kind.clone() },
            id: log.target.clone(),
            name: log.target_name.clone(),
            resource_group: log.target_resource_group.clone(),
            subscription_id: if from_target.is_empty() { log.subscription_id.clone() } else { from_target },
            location: log.location.clone(),
            subnet_id: None,
            vnet_id: vnet_id_of(&log.target),
        };
        let verdict = diagnose_config(log, &source, &context);
        findings.push(to_finding(&source, log, verdict));
    }

    if !storage_accounts.is_empty() {
        findings.retain(|finding| {
            finding.storage_account.is_empty()
                || storage_accounts.iter().any(|name| eq_ignore(name, &finding.storage_account))
        });
    }
    findings.sort_by(|left, right| {
        left.subscription_id
            .to_ascii_lowercase()
            .cmp(&right.subscription_id.to_ascii_lowercase())
            .then(kind_rank(&left.kind).cmp(&kind_rank(&right.kind)))
            .then(left.resource_group.to_ascii_lowercase().cmp(&right.resource_group.to_ascii_lowercase()))
            .then(left.name.to_ascii_lowercase().cmp(&right.name.to_ascii_lowercase()))
    });

    let finding_count = findings.len();
    DiagnosisResult {
        generated_utc: iso(now),
        stale_after_hours,
        source_count: sources.len() + orphans,
        finding_count,
        findings,
    }
}

/// None when a delivering flow log covers the source.
fn evaluate(source: &Source, context: &Context) -> Option<FlowLogFinding> {
    for id in chain(source) {
        let Some(configs) = context.by_target.get(&key(&id)) else { continue };
        let enabled: Vec<&FlowLogConfigInfo> = configs.iter().filter(|config| config.enabled).collect();
        if enabled.is_empty() {
            continue;
        }
        if enabled.iter().any(|config| is_delivering(config)) {
            return None;
        }
        let mut ranked: Vec<(&FlowLogConfigInfo, Verdict)> =
            enabled.iter().map(|config| (*config, diagnose_config(config, source, context))).collect();
        ranked.sort_by(|left, right| {
            rank(left.1.reason)
                .cmp(&rank(right.1.reason))
                .then(left.0.name.to_ascii_lowercase().cmp(&right.0.name.to_ascii_lowercase()))
        });
        let (config, verdict) = ranked.remove(0);
        return Some(to_finding(source, config, verdict));
    }

    for id in chain(source) {
        let Some(configs) = context.by_target.get(&key(&id)) else { continue };
        let mut disabled: Vec<&FlowLogConfigInfo> = configs.iter().filter(|config| !config.enabled).collect();
        if disabled.is_empty() {
            continue;
        }
        disabled.sort_by(|left, right| left.name.to_ascii_lowercase().cmp(&right.name.to_ascii_lowercase()));
        return Some(to_finding(source, disabled[0], diagnose_config(disabled[0], source, context)));
    }

    if !has_watcher(source, context.topology) {
        let where_text = if source.location.trim().is_empty() { "this subscription" } else { source.location.as_str() };
        return Some(bare(
            source,
            diagnosis_reason::NETWORK_WATCHER_MISSING,
            format!(
                "No Network Watcher in {where_text}. Flow logs are created on the regional Network Watcher, so nothing is sent until one exists."
            ),
        ));
    }
    Some(bare(source, diagnosis_reason::NOT_CONFIGURED, not_configured_message(source)))
}

fn diagnose_config(config: &FlowLogConfigInfo, source: &Source, context: &Context) -> Verdict {
    let scope = scope_text(config, source);
    if !config.enabled {
        return Verdict {
            reason: diagnosis_reason::DISABLED,
            message: nsg_note(
                config,
                format!(
                    "Flow log {} targets {scope} but is disabled, so nothing is written to {}.",
                    config.name, config.storage_account
                ),
            ),
        };
    }
    if provisioning_blocks(config.provisioning_state.as_deref()) {
        return Verdict {
            reason: diagnosis_reason::PROVISIONING_FAILED,
            message: nsg_note(
                config,
                format!(
                    "Flow log {} provisioning state is {}, so Azure is not writing it to {}.",
                    config.name,
                    config.provisioning_state.as_deref().unwrap_or(""),
                    config.storage_account
                ),
            ),
        };
    }
    if !context.known_ids.contains(&key(&config.target)) {
        return Verdict {
            reason: diagnosis_reason::TARGET_MISSING,
            message: nsg_note(
                config,
                format!(
                    "Flow log {} targets {}, which is not in the scanned subscriptions. A missing virtual network, subnet, network interface, or network security group sends nothing.",
                    config.name, config.target
                ),
            ),
        };
    }
    let Some(account) = context.accounts.get(&key(&config.storage_account)) else {
        return Verdict {
            reason: diagnosis_reason::STORAGE_MISSING,
            message: nsg_note(
                config,
                format!(
                    "Flow log {} sends to storage account {}, which is not in the scanned subscriptions. The account may have been deleted, or it lives in a subscription this identity cannot read.",
                    config.name, config.storage_account
                ),
            ),
        };
    };
    let resource_location =
        if source.location.trim().is_empty() { config.location.as_str() } else { source.location.as_str() };
    if locations_differ(&account.location, resource_location) {
        return Verdict {
            reason: diagnosis_reason::REGION_MISMATCH,
            message: nsg_note(
                config,
                format!(
                    "Storage account {} is in {}. {} {} is in {resource_location}. Flow logs are written only when the storage account is in the same region as the virtual network or network security group.",
                    account.name,
                    account.location,
                    label(&source.kind),
                    source.name
                ),
            ),
        };
    }
    if unsupported(account) {
        return Verdict {
            reason: diagnosis_reason::UNSUPPORTED_STORAGE,
            message: nsg_note(
                config,
                format!(
                    "Storage account {} is {} {}. Flow logs need a Standard general-purpose storage account. Premium storage is not supported.",
                    account.name, account.kind, account.sku_name
                ),
            ),
        };
    }
    if firewall_blocks(account) {
        let access = if account.public_network_access.as_deref().unwrap_or("").trim().is_empty() {
            "unknown"
        } else {
            account.public_network_access.as_deref().unwrap_or("unknown")
        };
        let action = if account.network_default_action.as_deref().unwrap_or("").trim().is_empty() {
            "unknown"
        } else {
            account.network_default_action.as_deref().unwrap_or("unknown")
        };
        return Verdict {
            reason: diagnosis_reason::STORAGE_FIREWALL,
            message: nsg_note(
                config,
                format!(
                    "Storage account {} does not allow Network Watcher to write blobs. Public network access is {access}, the firewall default action is {action}, and trusted Azure services are not allowed. Allow Azure services on the trusted services list.",
                    account.name
                ),
            ),
        };
    }
    if account.allow_shared_key_access == Some(false) && !config.has_managed_identity {
        return Verdict {
            reason: diagnosis_reason::SHARED_KEY_DISABLED,
            message: nsg_note(
                config,
                format!(
                    "Storage account {} disables shared key access, and flow log {} has no managed identity. Network Watcher cannot authenticate to the account. Assign a user-assigned managed identity to the flow log, or allow shared key access, then disable and re-enable the flow log.",
                    account.name, config.name
                ),
            ),
        };
    }
    blob_status(config, account, context)
}

fn blob_status(config: &FlowLogConfigInfo, account: &StorageAccountInfo, context: &Context) -> Verdict {
    if config.status == flow_status::ACCESS_DENIED {
        return Verdict {
            reason: diagnosis_reason::ACCESS_DENIED,
            message: nsg_note(
                config,
                format!(
                    "Flow log {} is enabled, but blobs in {} could not be listed ({}). Grant Storage Blob Data Reader on the account to confirm whether logs are arriving.",
                    config.name, account.name, config.message
                ),
            ),
        };
    }
    if config.status == flow_status::ERROR
        || (config.status == flow_status::HEALTHY
            && latest_blob(config).is_empty()
            && account.status == flow_status::ERROR)
    {
        let detail = if config.message.trim().is_empty() { "the health check failed" } else { config.message.as_str() };
        return Verdict {
            reason: diagnosis_reason::ERROR,
            message: nsg_note(config, format!("Flow log {} could not be checked: {detail}", config.name)),
        };
    }
    if config.status == flow_status::STALE {
        let age = config.age_hours.map(|hours| format!("{hours:.1}")).unwrap_or_else(|| "unknown".into());
        return Verdict {
            reason: diagnosis_reason::STALE,
            message: nsg_note(
                config,
                format!(
                    "Flow log {} last wrote to {} {age} hours ago. The limit is {} hours, so new blobs are not arriving.",
                    config.name, account.name, context.stale_after_hours
                ),
            ),
        };
    }
    let nsg_gap = if config.target_kind == "NSG" {
        format!(
            " The blob check reads {CONTAINER_NAME}. NSG flow logs are stored in {NSG_CONTAINER}, so this does not prove that container is empty."
        )
    } else {
        String::new()
    };
    Verdict {
        reason: diagnosis_reason::NO_DATA,
        message: nsg_note(
            config,
            format!(
                "Flow log {} is enabled and {} accepts flow logs, but no blobs were written in the last 2 days. Rotating the storage account keys stops flow logs until the flow log is disabled and enabled again. No traffic on the source also leaves the container empty.{nsg_gap}",
                config.name, account.name
            ),
        ),
    }
}

fn is_delivering(config: &FlowLogConfigInfo) -> bool {
    config.enabled && config.status == flow_status::HEALTHY && !latest_blob(config).is_empty()
}

fn latest_blob(config: &FlowLogConfigInfo) -> &str {
    config.latest_blob_utc.as_deref().unwrap_or("")
}

fn provisioning_blocks(state: Option<&str>) -> bool {
    let Some(state) = state.map(str::trim).filter(|state| !state.is_empty()) else { return false };
    !state.eq_ignore_ascii_case("Succeeded") && !state.eq_ignore_ascii_case("Updating")
}

fn unsupported(account: &StorageAccountInfo) -> bool {
    account.sku_name.to_ascii_lowercase().starts_with("premium")
        || account.kind.eq_ignore_ascii_case("FileStorage")
        || account.kind.eq_ignore_ascii_case("BlockBlobStorage")
}

fn firewall_blocks(account: &StorageAccountInfo) -> bool {
    if account.allows_trusted_services != Some(false) {
        return false;
    }
    if account.public_network_access.as_deref().is_some_and(|access| {
        access.eq_ignore_ascii_case("Disabled") || access.eq_ignore_ascii_case("SecuredByPerimeter")
    }) {
        return true;
    }
    account.network_default_action.as_deref().is_some_and(|action| action.eq_ignore_ascii_case("Deny"))
}

fn locations_differ(storage_location: &str, resource_location: &str) -> bool {
    let storage = normalize_location(storage_location);
    let resource = normalize_location(resource_location);
    !storage.is_empty() && !resource.is_empty() && storage != resource
}

fn normalize_location(location: &str) -> String {
    location.chars().filter(|c| *c != ' ' && *c != '-').flat_map(|c| c.to_lowercase()).collect()
}

fn has_watcher(source: &Source, topology: &AzureTopology) -> bool {
    let location = normalize_location(&source.location);
    if location.is_empty() {
        return true;
    }
    topology.network_watchers.iter().any(|watcher| {
        (source.subscription_id.is_empty() || eq_ignore(&watcher.subscription_id, &source.subscription_id))
            && normalize_location(&watcher.location) == location
    })
}

fn attachments_covered(nsg_id: &str, topology: &AzureTopology, covered: &HashSet<String>) -> bool {
    let mut attachments = Vec::new();
    for vnet in &topology.vnets {
        for subnet in &vnet.subnets {
            if !subnet.id.trim().is_empty() && subnet.nsg_id.as_deref().is_some_and(|id| eq_ignore(id, nsg_id)) {
                attachments.push(key(&subnet.id));
            }
        }
    }
    for nic in &topology.nics {
        if !nic.id.trim().is_empty() && nic.nsg_id.as_deref().is_some_and(|id| eq_ignore(id, nsg_id)) {
            attachments.push(key(&nic.id));
        }
    }
    !attachments.is_empty() && attachments.iter().all(|id| covered.contains(id))
}

fn chain(source: &Source) -> Vec<String> {
    let mut ids = vec![source.id.clone()];
    if source.kind == "NIC" {
        if let Some(subnet_id) = source.subnet_id.as_deref().filter(|id| !id.trim().is_empty()) {
            ids.push(subnet_id.to_string());
        }
    }
    if source.kind == "NIC" || source.kind == "Subnet" {
        if let Some(vnet_id) = source.vnet_id.as_deref().filter(|id| !id.trim().is_empty()) {
            ids.push(vnet_id.to_string());
        }
    }
    ids
}

fn not_configured_message(source: &Source) -> String {
    let kind_label = label(&source.kind);
    let target = if source.kind == "Subnet" || source.kind == "NIC" {
        format!("No flow log targets this {kind_label} or its virtual network.")
    } else {
        format!("No flow log targets this {kind_label}.")
    };
    let where_text = if source.location.trim().is_empty() {
        "a storage account in the same region".to_string()
    } else {
        format!("a storage account in {}", source.location)
    };
    if source.kind == "NSG" {
        format!(
            "{target} NSG flow logs retire on 30 Sep 2027 and new ones cannot be created. Enable a virtual network flow log to {where_text}."
        )
    } else {
        format!("{target} Enable a virtual network flow log to {where_text}.")
    }
}

fn scope_text(config: &FlowLogConfigInfo, source: &Source) -> String {
    if eq_ignore(&config.target, &source.id) {
        return format!("this {}", label(&source.kind));
    }
    let name = if config.target_name.trim().is_empty() { "network" } else { config.target_name.as_str() };
    format!("parent {} {name}", label(&config.target_kind))
}

fn nsg_note(config: &FlowLogConfigInfo, message: String) -> String {
    if config.target_kind != "NSG" {
        return message;
    }
    format!("{message} NSG flow logs retire on 30 Sep 2027 and new ones cannot be created.")
}

fn label(kind: &str) -> &'static str {
    match kind {
        "VNet" => "virtual network",
        "Subnet" => "subnet",
        "NIC" => "network interface",
        "NSG" => "network security group",
        _ => "resource",
    }
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "VNet" => 0,
        "Subnet" => 1,
        "NIC" => 2,
        "NSG" => 3,
        _ => 4,
    }
}

fn to_finding(source: &Source, config: &FlowLogConfigInfo, verdict: Verdict) -> FlowLogFinding {
    FlowLogFinding {
        subscription_id: source.subscription_id.clone(),
        resource_group: source.resource_group.clone(),
        location: if source.location.trim().is_empty() { config.location.clone() } else { source.location.clone() },
        kind: source.kind.clone(),
        name: if source.name.trim().is_empty() { config.target_name.clone() } else { source.name.clone() },
        resource_id: source.id.clone(),
        reason: verdict.reason.to_string(),
        flow_log: config.name.clone(),
        storage_account: config.storage_account.clone(),
        message: verdict.message,
    }
}

fn bare(source: &Source, reason: &'static str, message: String) -> FlowLogFinding {
    FlowLogFinding {
        subscription_id: source.subscription_id.clone(),
        resource_group: source.resource_group.clone(),
        location: source.location.clone(),
        kind: source.kind.clone(),
        name: source.name.clone(),
        resource_id: source.id.clone(),
        reason: reason.to_string(),
        message,
        ..Default::default()
    }
}

fn vnet_id_of(id: &str) -> Option<String> {
    let index = id.to_ascii_lowercase().find("/subnets/")?;
    if index == 0 { None } else { Some(id[..index].to_string()) }
}

fn segment_after(id: &str, marker: &str) -> String {
    let parts: Vec<&str> = id.split('/').filter(|part| !part.is_empty()).collect();
    parts
        .windows(2)
        .find(|pair| pair[0].eq_ignore_ascii_case(marker))
        .map(|pair| pair[1].to_string())
        .unwrap_or_default()
}

fn key(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn eq_ignore(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NetworkWatcherRef, NicInfo, SubnetInfo, VnetInfo};

    const SUB: &str = "11111111-1111-1111-1111-111111111111";

    fn id(kind: &str, name: &str) -> String {
        format!("/subscriptions/{SUB}/resourceGroups/rg/providers/Microsoft.Network/{kind}/{name}")
    }

    fn vnet_id() -> String {
        id("virtualNetworks", "vnet1")
    }

    fn subnet_id() -> String {
        format!("{}/subnets/app", vnet_id())
    }

    fn nic_id() -> String {
        id("networkInterfaces", "nic1")
    }

    fn nsg_id() -> String {
        id("networkSecurityGroups", "nsg1")
    }

    fn network() -> AzureTopology {
        AzureTopology {
            network_watchers: vec![NetworkWatcherRef {
                subscription_id: SUB.into(),
                name: "NetworkWatcher_australiaeast".into(),
                location: "australiaeast".into(),
            }],
            storage_accounts: vec![StorageAccountInfo {
                subscription_id: SUB.into(),
                name: "stlogs".into(),
                resource_group: "rg".into(),
                location: "australiaeast".into(),
                kind: "StorageV2".into(),
                sku_name: "Standard_LRS".into(),
                public_network_access: Some("Enabled".into()),
                network_default_action: Some("Allow".into()),
                allows_trusted_services: Some(true),
                allow_shared_key_access: Some(true),
                ..Default::default()
            }],
            vnets: vec![VnetInfo {
                id: vnet_id(),
                name: "vnet1".into(),
                resource_group: "rg".into(),
                subscription_id: SUB.into(),
                address_prefixes: vec!["10.0.0.0/16".into()],
                subnets: vec![SubnetInfo {
                    address_prefixes: vec!["10.0.1.0/24".into()],
                    nsg_id: Some(nsg_id()),
                    id: subnet_id(),
                    name: "app".into(),
                }],
                location: "australiaeast".into(),
            }],
            nics: vec![NicInfo {
                nsg_id: Some(nsg_id()),
                private_ips: vec!["10.0.1.4".into()],
                id: nic_id(),
                name: "nic1".into(),
                location: "australiaeast".into(),
                subnet_id: Some(subnet_id()),
            }],
            ..Default::default()
        }
    }

    fn config(
        name: &str,
        target: &str,
        kind: &str,
        enabled: bool,
        status: &str,
        latest: Option<&str>,
    ) -> FlowLogConfigInfo {
        FlowLogConfigInfo {
            subscription_id: SUB.into(),
            name: name.into(),
            storage_account: "stlogs".into(),
            network_watcher: "NetworkWatcher_australiaeast".into(),
            location: "australiaeast".into(),
            target: target.into(),
            target_kind: kind.into(),
            target_name: target.rsplit('/').find(|part| !part.is_empty()).unwrap_or(target).into(),
            target_resource_group: "rg".into(),
            enabled,
            status: status.into(),
            latest_blob_utc: latest.map(str::to_string),
            provisioning_state: Some("Succeeded".into()),
            ..Default::default()
        }
    }

    fn run(topology: &AzureTopology, logs: &[FlowLogConfigInfo], filter: &[String]) -> DiagnosisResult {
        let now = DateTime::parse_from_rfc3339("2026-10-24T00:00:00Z").unwrap().with_timezone(&Utc);
        diagnose(topology, logs, 6, now, filter)
    }

    fn reason_of<'a>(result: &'a DiagnosisResult, kind: &str) -> &'a str {
        result.findings.iter().find(|finding| finding.kind == kind).unwrap().reason.as_str()
    }

    #[test]
    fn sources_without_a_flow_log_are_not_configured() {
        let result = run(&network(), &[], &[]);
        assert_eq!(result.source_count, 4);
        assert_eq!(
            result.findings.iter().map(|finding| finding.kind.as_str()).collect::<Vec<_>>(),
            ["VNet", "Subnet", "NIC", "NSG"]
        );
        assert!(
            result
                .findings
                .iter()
                .filter(|finding| finding.kind != "NSG")
                .all(|finding| finding.reason == diagnosis_reason::NOT_CONFIGURED)
        );
        let nsg = result.findings.iter().find(|finding| finding.kind == "NSG").unwrap();
        assert_eq!(nsg.reason, diagnosis_reason::NOT_CONFIGURED);
        assert!(nsg.message.contains("30 Sep 2027"));
    }

    #[test]
    fn missing_network_watcher_is_its_own_reason() {
        let mut topology = network();
        topology.network_watchers.clear();
        let result = run(&topology, &[], &[]);
        assert!(result.findings.iter().all(|finding| finding.reason == diagnosis_reason::NETWORK_WATCHER_MISSING));
    }

    #[test]
    fn delivering_vnet_flow_log_covers_children_and_the_attached_nsg() {
        let log = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::HEALTHY, Some("2026-10-24T00:00:00Z"));
        let result = run(&network(), &[log], &[]);
        assert_eq!(result.source_count, 4);
        assert!(result.findings.is_empty());
    }

    #[test]
    fn disabled_parent_is_reported_for_every_child() {
        let log = config("fl-vnet", &vnet_id(), "VNet", false, flow_status::DISABLED, None);
        let result = run(&network(), &[log], &[]);
        assert_eq!(result.findings.len(), 4);
        assert!(
            result
                .findings
                .iter()
                .filter(|finding| finding.kind != "NSG")
                .all(|finding| finding.reason == diagnosis_reason::DISABLED)
        );
        let nic = result.findings.iter().find(|finding| finding.kind == "NIC").unwrap();
        assert!(nic.message.contains("parent virtual network vnet1"));
    }

    #[test]
    fn enabled_nic_flow_log_wins_over_a_healthy_vnet_flow_log() {
        let vnet = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::HEALTHY, Some("2026-10-24T00:00:00Z"));
        let nic = config("fl-nic", &nic_id(), "NIC", true, flow_status::NO_DATA, None);
        let result = run(&network(), &[vnet, nic], &[]);
        let nic = result.findings.iter().find(|finding| finding.kind == "NIC").unwrap();
        assert_eq!(nic.reason, diagnosis_reason::NO_DATA);
        assert_eq!(nic.flow_log, "fl-nic");
        assert_eq!(reason_of(&result, "NSG"), diagnosis_reason::NOT_CONFIGURED);
    }

    #[test]
    fn firewall_blocks_before_an_empty_container() {
        let mut topology = network();
        topology.storage_accounts[0].network_default_action = Some("Deny".into());
        topology.storage_accounts[0].allows_trusted_services = Some(false);
        topology.storage_accounts[0].public_network_access = Some("Enabled".into());
        let log = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::NO_DATA, None);
        let result = run(&topology, &[log], &[]);
        assert!(
            result
                .findings
                .iter()
                .filter(|finding| finding.kind != "NSG")
                .all(|finding| finding.reason == diagnosis_reason::STORAGE_FIREWALL)
        );
    }

    #[test]
    fn trusted_services_exception_is_not_a_firewall_block() {
        let mut topology = network();
        topology.storage_accounts[0].network_default_action = Some("Deny".into());
        topology.storage_accounts[0].allows_trusted_services = Some(true);
        let log = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::NO_DATA, None);
        assert_eq!(reason_of(&run(&topology, &[log], &[]), "VNet"), diagnosis_reason::NO_DATA);
    }

    #[test]
    fn premium_sku_and_region_mismatch_are_named() {
        let log = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::NO_DATA, None);
        let mut premium = network();
        premium.storage_accounts[0].sku_name = "Premium_LRS".into();
        assert_eq!(reason_of(&run(&premium, &[log.clone()], &[]), "VNet"), diagnosis_reason::UNSUPPORTED_STORAGE);

        let mut other_region = network();
        other_region.storage_accounts[0].location = "eastus".into();
        assert_eq!(reason_of(&run(&other_region, &[log.clone()], &[]), "VNet"), diagnosis_reason::REGION_MISMATCH);

        let mut spaced = network();
        spaced.storage_accounts[0].location = "Australia East".into();
        assert_eq!(reason_of(&run(&spaced, &[log], &[]), "VNet"), diagnosis_reason::NO_DATA);
    }

    #[test]
    fn shared_key_off_without_an_identity_is_reported() {
        let mut topology = network();
        topology.storage_accounts[0].allow_shared_key_access = Some(false);
        let mut log = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::NO_DATA, None);
        assert_eq!(reason_of(&run(&topology, &[log.clone()], &[]), "VNet"), diagnosis_reason::SHARED_KEY_DISABLED);
        log.has_managed_identity = true;
        assert_eq!(reason_of(&run(&topology, &[log], &[]), "VNet"), diagnosis_reason::NO_DATA);
    }

    #[test]
    fn deleted_target_and_failed_provisioning_are_reported() {
        let mut gone = config("fl-old", &format!("{}-deleted", vnet_id()), "VNet", true, flow_status::NO_DATA, None);
        gone.target_name = "vnet-deleted".into();
        let mut failed = config("fl-bad", &vnet_id(), "VNet", true, flow_status::NO_DATA, None);
        failed.provisioning_state = Some("Failed".into());
        let result = run(&network(), &[gone, failed], &[]);
        assert_eq!(
            result.findings.iter().find(|finding| finding.name == "vnet1").unwrap().reason,
            diagnosis_reason::PROVISIONING_FAILED
        );
        assert_eq!(
            result.findings.iter().find(|finding| finding.name == "vnet-deleted").unwrap().reason,
            diagnosis_reason::TARGET_MISSING
        );
    }

    #[test]
    fn stale_blobs_name_the_age_and_the_limit() {
        let mut log = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::STALE, Some("2026-10-20T00:00:00Z"));
        log.age_hours = Some(30.0);
        let finding = run(&network(), &[log], &[]).findings.into_iter().find(|finding| finding.kind == "VNet").unwrap();
        assert_eq!(finding.reason, diagnosis_reason::STALE);
        assert!(finding.message.contains("30.0 hours"));
        assert!(finding.message.contains("limit is 6 hours"));
    }

    #[test]
    fn storage_filter_keeps_unconfigured_sources_and_matching_accounts() {
        let mut other = config("fl-vnet", &vnet_id(), "VNet", true, flow_status::NO_DATA, None);
        other.storage_account = "stother".into();
        let result = run(&network(), &[other], &["stlogs".into()]);
        assert!(result.findings.iter().any(|finding| finding.kind == "NSG" && finding.storage_account.is_empty()));
        assert!(result.findings.iter().all(|finding| finding.storage_account != "stother"));
    }
}
