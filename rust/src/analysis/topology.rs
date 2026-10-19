use std::collections::HashMap;

use super::ipv4::Ipv4Lookup;
use crate::model::{AzureTopology, ReportScope, scope_kind};

/// Builds the IPv4 scope table from ARM topology (NIC /32, subnet, then VNet). Same rules as TopologyMapper.cs,
/// including disabled configurations as owners. Notes are appended for multi-account targets.
pub fn build_lookup(topology: &AzureTopology, notes: &mut Vec<String>) -> Ipv4Lookup {
    let mut lookup = Ipv4Lookup::new();
    let mut storage_scopes: HashMap<String, ReportScope> = HashMap::new();

    // Disabled configurations still describe which account a network reports to, so they own address space too.
    let nsg_owners = owners_by_target(topology, "NSG", notes);
    let vnet_owners = owners_by_target(topology, "VNet", notes);

    for nic in &topology.nics {
        let Some(owner) = nic.nsg_id.as_deref().and_then(|id| nsg_owners.get(&id.to_ascii_lowercase())) else {
            continue;
        };
        let scope = storage_scope(&mut storage_scopes, topology, owner);
        for ip in &nic.private_ips {
            lookup.try_add(&format!("{ip}/32"), scope.clone());
        }
    }

    for vnet in &topology.vnets {
        for subnet in &vnet.subnets {
            let Some(owner) = subnet.nsg_id.as_deref().and_then(|id| nsg_owners.get(&id.to_ascii_lowercase())) else {
                continue;
            };
            let scope = storage_scope(&mut storage_scopes, topology, owner);
            for prefix in &subnet.address_prefixes {
                lookup.try_add(prefix, scope.clone());
            }
        }

        let vnet_scope = match vnet_owners.get(&vnet.id.to_ascii_lowercase()) {
            Some(owner) => storage_scope(&mut storage_scopes, topology, owner),
            None => ReportScope {
                id: format!("vnet:{}/{}", vnet.resource_group, vnet.name),
                kind: scope_kind::VNET.to_string(),
                label: format!("{} (no flow logs)", vnet.name),
                subscription_id: vnet.subscription_id.clone(),
                resource_group: vnet.resource_group.clone(),
                ..Default::default()
            },
        };
        for prefix in &vnet.address_prefixes {
            lookup.try_add(prefix, vnet_scope.clone());
        }
    }

    lookup
}

/// Scope for a storage account, with subscription and resource group taken from the account inventory.
fn storage_scope(cache: &mut HashMap<String, ReportScope>, topology: &AzureTopology, account: &str) -> ReportScope {
    cache
        .entry(account.to_ascii_lowercase())
        .or_insert_with(|| {
            let info = topology.storage_accounts.iter().find(|info| info.name.eq_ignore_ascii_case(account));
            ReportScope {
                id: account.to_string(),
                kind: scope_kind::STORAGE.to_string(),
                label: account.to_string(),
                subscription_id: info.map(|info| info.subscription_id.clone()).unwrap_or_default(),
                resource_group: info.map(|info| info.resource_group.clone()).unwrap_or_default(),
                storage_account: account.to_string(),
            }
        })
        .clone()
}

/// Owning storage account for each target (keyed by lower-cased ARM id) of one target kind. With several
/// accounts, the lowest name wins and a note says so.
fn owners_by_target(topology: &AzureTopology, target_kind: &str, notes: &mut Vec<String>) -> HashMap<String, String> {
    // Groups keep first-seen order and casing, which is the order and text the .NET GroupBy produces.
    let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();
    for config in topology.flow_logs.iter().filter(|config| config.target_kind == target_kind) {
        let key = config.target.to_ascii_lowercase();
        let index = match group_index.get(&key) {
            Some(&index) => index,
            None => {
                groups.push((config.target.clone(), Vec::new()));
                group_index.insert(key, groups.len() - 1);
                groups.len() - 1
            }
        };
        groups[index].1.push(config.storage_account.as_str());
    }

    let mut owners = HashMap::new();
    for (target, accounts) in groups {
        let mut distinct: Vec<&str> = Vec::new();
        for account in accounts {
            if !distinct.iter().any(|known| known.eq_ignore_ascii_case(account)) {
                distinct.push(account);
            }
        }
        distinct.sort_by_key(|account| account.to_ascii_uppercase());

        owners.insert(target.to_ascii_lowercase(), distinct[0].to_string());
        if distinct.len() > 1 {
            notes.push(format!(
                "{target_kind} {target} has flow logs to {} storage accounts; attributed to {}.",
                distinct.len(),
                distinct[0]
            ));
        }
    }
    owners
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FlowLogConfigInfo, NicInfo, StorageAccountInfo, SubnetInfo, VnetInfo};

    const NSG_APP: &str =
        "/subscriptions/s1/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/nsg-app";
    const VNET: &str = "/subscriptions/s1/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/vnet-main";

    fn config(target: &str, target_kind: &str, account: &str) -> FlowLogConfigInfo {
        FlowLogConfigInfo {
            target: target.to_string(),
            target_kind: target_kind.to_string(),
            storage_account: account.to_string(),
            enabled: true,
            ..Default::default()
        }
    }

    fn topology() -> AzureTopology {
        AzureTopology {
            storage_accounts: vec![StorageAccountInfo {
                subscription_id: "s1".to_string(),
                name: "stflowlogs01".to_string(),
                resource_group: "rg-logs".to_string(),
                ..Default::default()
            }],
            flow_logs: vec![config(NSG_APP, "NSG", "stflowlogs01")],
            vnets: vec![VnetInfo {
                id: VNET.to_string(),
                name: "vnet-main".to_string(),
                resource_group: "rg".to_string(),
                subscription_id: "s1".to_string(),
                address_prefixes: vec!["10.10.0.0/16".to_string()],
                subnets: vec![
                    SubnetInfo {
                        address_prefixes: vec!["10.10.1.0/24".to_string()],
                        nsg_id: Some(NSG_APP.to_uppercase()),
                    },
                    SubnetInfo { address_prefixes: vec!["10.10.2.0/24".to_string()], nsg_id: None },
                ],
            }],
            nics: vec![NicInfo { nsg_id: Some(NSG_APP.to_string()), private_ips: vec!["10.10.1.4".to_string()] }],
            ..Default::default()
        }
    }

    fn lookup_id(lookup: &Ipv4Lookup, address: &str) -> String {
        lookup.find(crate::analysis::ipv4::parse_ipv4(address).unwrap()).unwrap().id.clone()
    }

    #[test]
    fn nic_then_subnet_then_vnet_most_specific_wins() {
        let mut notes = Vec::new();
        let lookup = build_lookup(&topology(), &mut notes);

        assert_eq!(lookup_id(&lookup, "10.10.1.4"), "stflowlogs01", "NIC /32");
        assert_eq!(lookup_id(&lookup, "10.10.1.9"), "stflowlogs01", "subnet with NSG id in other case");
        assert_eq!(lookup_id(&lookup, "10.10.2.9"), "vnet:rg/vnet-main", "subnet without NSG falls to VNet");
        assert!(notes.is_empty());
    }

    #[test]
    fn storage_scope_carries_inventory_details() {
        let lookup = build_lookup(&topology(), &mut Vec::new());
        let scope = lookup.find(crate::analysis::ipv4::parse_ipv4("10.10.1.4").unwrap()).unwrap();
        assert_eq!(scope.kind, scope_kind::STORAGE);
        assert_eq!(scope.subscription_id, "s1");
        assert_eq!(scope.resource_group, "rg-logs");
        assert_eq!(scope.storage_account, "stflowlogs01");
    }

    #[test]
    fn vnet_scope_is_labelled_when_it_has_no_flow_logs() {
        let lookup = build_lookup(&topology(), &mut Vec::new());
        let scope = lookup.find(crate::analysis::ipv4::parse_ipv4("10.10.200.1").unwrap()).unwrap();
        assert_eq!(scope.kind, scope_kind::VNET);
        assert_eq!(scope.label, "vnet-main (no flow logs)");
        assert_eq!(scope.resource_group, "rg");
    }

    #[test]
    fn disabled_configurations_still_own_address_space() {
        let mut topo = topology();
        topo.flow_logs[0].enabled = false;
        let lookup = build_lookup(&topo, &mut Vec::new());
        assert_eq!(lookup_id(&lookup, "10.10.1.4"), "stflowlogs01");
    }

    #[test]
    fn vnet_flow_log_owns_the_address_space_and_multiple_owners_add_a_note() {
        let mut topo = topology();
        topo.flow_logs = vec![
            config(VNET, "VNet", "stlogsb"),
            config(&VNET.to_uppercase(), "VNet", "stLogsA"),
            config(VNET, "VNet", "stlogsa"),
        ];
        topo.nics.clear();
        topo.vnets[0].subnets.clear();
        let mut notes = Vec::new();
        let lookup = build_lookup(&topo, &mut notes);

        // Lowest name, ignoring case, keeps the first-seen spelling of that name.
        assert_eq!(lookup_id(&lookup, "10.10.5.5"), "stLogsA");
        assert_eq!(notes, vec![format!("VNet {} has flow logs to 2 storage accounts; attributed to stLogsA.", VNET)]);
    }
}
