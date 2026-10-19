use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::ipv4::Ipv4Lookup;
use crate::model::{ReportScope, ReportSubscription, scope_kind};
use crate::util::file_name;

/// Offline topology: { "networks": [ { "name", "kind": "storage"|"vnet", "prefixes": [...] } ],
/// "subscriptions": [ { "subscriptionId", "displayName", "tenantId" } ] }. The subscriptions key is optional.
#[derive(Debug)]
pub struct TopologyFileResult {
    pub lookup: Ipv4Lookup,
    pub networks: usize,
    pub prefixes: usize,
    pub skipped_prefixes: usize,
    /// Names of "storage" networks: the dedupe read set in offline mode.
    pub storage_accounts: Vec<String>,
    /// Subscriptions in file order. Entries that cannot be used are left out and named in `notes`.
    pub subscriptions: Vec<ReportSubscription>,
    pub notes: Vec<String>,
}

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    networks: Vec<Network>,
    /// Read entry by entry (see `read_subscriptions`), so one bad entry is noted instead of failing the file.
    #[serde(default)]
    subscriptions: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct Network {
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    prefixes: Vec<String>,
}

pub fn load(path: &Path) -> Result<TopologyFileResult> {
    if !path.is_file() {
        bail!("Topology file not found: {}", path.display());
    }
    parse(&std::fs::read_to_string(path)?, &file_name(path))
}

/// `file` names the topology file in a JSON error ("topology file <name>: <message>"), as the .NET tool does.
fn parse(json: &str, file: &str) -> Result<TopologyFileResult> {
    let document: Document = serde_json::from_str(json).with_context(|| format!("topology file {file}"))?;

    let mut lookup = Ipv4Lookup::new();
    let mut prefixes = 0;
    let mut skipped_prefixes = 0;
    let mut storage_accounts = Vec::new();

    for network in &document.networks {
        if network.name.trim().is_empty() {
            bail!("Every topology network needs a name.");
        }
        let kind = network.kind.to_ascii_lowercase();
        if kind == "storage" {
            storage_accounts.push(network.name.clone());
        }

        let scope = match kind.as_str() {
            "storage" => ReportScope {
                id: network.name.clone(),
                kind: scope_kind::STORAGE.to_string(),
                label: network.name.clone(),
                storage_account: network.name.clone(),
                ..Default::default()
            },
            "vnet" => ReportScope {
                id: format!("vnet:{}", network.name),
                kind: scope_kind::VNET.to_string(),
                label: network.name.clone(),
                ..Default::default()
            },
            _ => bail!("Topology network '{}' has kind '{}'. Use storage or vnet.", network.name, network.kind),
        };

        for prefix in &network.prefixes {
            if lookup.try_add(prefix.trim(), scope.clone()) {
                prefixes += 1;
            } else {
                skipped_prefixes += 1;
            }
        }
    }

    let (subscriptions, notes) = read_subscriptions(&document.subscriptions.unwrap_or_default());

    Ok(TopologyFileResult {
        lookup,
        networks: document.networks.len(),
        prefixes,
        skipped_prefixes,
        storage_accounts,
        subscriptions,
        notes,
    })
}

/// Same rules as TopologyFile.cs: an entry that is not an object, or has no subscriptionId string, is left out with
/// a note. Other fields that are not strings count as missing.
fn read_subscriptions(entries: &[serde_json::Value]) -> (Vec<ReportSubscription>, Vec<String>) {
    let mut subscriptions = Vec::new();
    let mut notes = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let position = index + 1;
        let Some(object) = entry.as_object() else {
            notes.push(format!("Topology subscription entry ignored: entry {position} is not an object."));
            continue;
        };
        let text = |name: &str| object.get(name).and_then(serde_json::Value::as_str).map(str::to_string);
        match text("subscriptionId") {
            Some(subscription_id) if !subscription_id.trim().is_empty() => subscriptions.push(ReportSubscription {
                subscription_id,
                display_name: text("displayName").unwrap_or_default(),
                tenant_id: text("tenantId").unwrap_or_default(),
            }),
            _ => notes.push(format!("Topology subscription entry ignored: entry {position} has no subscriptionId.")),
        }
    }
    (subscriptions, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::ipv4::parse_ipv4;

    #[test]
    fn loads_storage_and_vnet_networks_and_counts_prefixes() {
        let loaded = parse(
            r#"{ "networks": [
                { "name": "stflowlogs01", "kind": "Storage", "prefixes": ["10.10.0.0/16", " 10.20.1.0/24 ", "2001:db8::/32"] },
                { "name": "vnet-hub", "kind": "vnet", "prefixes": ["10.99.0.0/16"] }
            ] }"#,
            "topology.json",
        )
        .unwrap();

        assert_eq!(loaded.networks, 2);
        assert_eq!(loaded.prefixes, 3);
        assert_eq!(loaded.skipped_prefixes, 1);
        assert_eq!(loaded.storage_accounts, vec!["stflowlogs01"]);

        let storage = loaded.lookup.find(parse_ipv4("10.20.1.9").unwrap()).unwrap();
        assert_eq!(
            (storage.id.as_str(), storage.kind.as_str(), storage.storage_account.as_str()),
            ("stflowlogs01", "storage", "stflowlogs01")
        );
        let vnet = loaded.lookup.find(parse_ipv4("10.99.4.4").unwrap()).unwrap();
        assert_eq!((vnet.id.as_str(), vnet.kind.as_str(), vnet.label.as_str()), ("vnet:vnet-hub", "vnet", "vnet-hub"));
    }

    #[test]
    fn unknown_kind_and_blank_name_are_errors() {
        let error =
            parse(r#"{ "networks": [{ "name": "x", "kind": "subnet", "prefixes": [] }] }"#, "t.json").unwrap_err();
        assert_eq!(error.to_string(), "Topology network 'x' has kind 'subnet'. Use storage or vnet.");

        let error = parse(r#"{ "networks": [{ "name": "  ", "kind": "storage" }] }"#, "t.json").unwrap_err();
        assert_eq!(error.to_string(), "Every topology network needs a name.");
    }

    #[test]
    fn missing_file_is_reported_by_path() {
        let missing = std::env::temp_dir().join("flowlog-topology-does-not-exist.json");
        let error = load(&missing).unwrap_err().to_string();
        assert_eq!(error, format!("Topology file not found: {}", missing.display()));
    }

    #[test]
    fn json_error_names_the_file() {
        let path = std::env::temp_dir().join("flowlog-topology-bad-subscriptions.json");
        std::fs::write(&path, r#"{ "networks": [], "subscriptions": "sub-1" }"#).unwrap();

        let error = format!("{:#}", load(&path).unwrap_err());

        assert!(
            error.starts_with("topology file flowlog-topology-bad-subscriptions.json: invalid type: string \"sub-1\""),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn subscriptions_load_in_file_order_and_unknown_keys_are_ignored() {
        let loaded = parse(
            r#"{
                "owner": { "team": "net" },
                "networks": [{ "name": "stflowlogs01", "kind": "storage", "prefixes": ["10.10.0.0/16"], "region": "au" }],
                "subscriptions": [
                    { "subscriptionId": "sub-b", "displayName": "Second", "tenantId": "t-1", "state": "Enabled" },
                    { "subscriptionId": "sub-a", "displayName": "First" }
                ]
            }"#,
            "topology.json",
        )
        .unwrap();

        assert_eq!(loaded.networks, 1);
        assert_eq!(
            loaded.subscriptions,
            vec![
                ReportSubscription {
                    subscription_id: "sub-b".into(),
                    display_name: "Second".into(),
                    tenant_id: "t-1".into()
                },
                ReportSubscription {
                    subscription_id: "sub-a".into(),
                    display_name: "First".into(),
                    tenant_id: String::new()
                },
            ]
        );
        assert!(loaded.notes.is_empty());
    }

    #[test]
    fn subscriptions_key_is_optional_and_null_means_none() {
        let loaded = parse(r#"{ "networks": [] }"#, "topology.json").unwrap();
        assert!(loaded.subscriptions.is_empty());
        assert!(loaded.notes.is_empty());

        let loaded = parse(r#"{ "networks": [], "subscriptions": null }"#, "topology.json").unwrap();
        assert!(loaded.subscriptions.is_empty());
        assert!(loaded.notes.is_empty());
    }

    #[test]
    fn malformed_subscription_entries_are_ignored_with_a_note() {
        let loaded = parse(
            r#"{ "networks": [], "subscriptions": [
                { "displayName": "No id" },
                { "subscriptionId": "  ", "displayName": "Blank id" },
                { "subscriptionId": 42 },
                "sub-as-string",
                { "subscriptionId": "sub-ok", "displayName": "Kept", "tenantId": 7 }
            ] }"#,
            "topology.json",
        )
        .unwrap();

        assert_eq!(
            loaded.subscriptions,
            vec![ReportSubscription {
                subscription_id: "sub-ok".into(),
                display_name: "Kept".into(),
                tenant_id: String::new()
            }]
        );
        assert_eq!(
            loaded.notes,
            vec![
                "Topology subscription entry ignored: entry 1 has no subscriptionId.",
                "Topology subscription entry ignored: entry 2 has no subscriptionId.",
                "Topology subscription entry ignored: entry 3 has no subscriptionId.",
                "Topology subscription entry ignored: entry 4 is not an object.",
            ]
        );
    }
}
