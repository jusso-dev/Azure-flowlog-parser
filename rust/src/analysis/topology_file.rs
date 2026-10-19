use std::path::Path;

use anyhow::{Result, bail};
use serde::Deserialize;

use super::ipv4::Ipv4Lookup;
use crate::model::{ReportScope, scope_kind};

/// Offline topology: { "networks": [ { "name", "kind": "storage"|"vnet", "prefixes": [...] } ] }.
#[derive(Debug)]
pub struct TopologyFileResult {
    pub lookup: Ipv4Lookup,
    pub networks: usize,
    pub prefixes: usize,
    pub skipped_prefixes: usize,
    /// Names of "storage" networks: the dedupe read set in offline mode.
    pub storage_accounts: Vec<String>,
}

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    networks: Vec<Network>,
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
    parse(&std::fs::read_to_string(path)?)
}

fn parse(json: &str) -> Result<TopologyFileResult> {
    let document: Document = serde_json::from_str(json)?;

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

    Ok(TopologyFileResult { lookup, networks: document.networks.len(), prefixes, skipped_prefixes, storage_accounts })
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
        let error = parse(r#"{ "networks": [{ "name": "x", "kind": "subnet", "prefixes": [] }] }"#).unwrap_err();
        assert_eq!(error.to_string(), "Topology network 'x' has kind 'subnet'. Use storage or vnet.");

        let error = parse(r#"{ "networks": [{ "name": "  ", "kind": "storage" }] }"#).unwrap_err();
        assert_eq!(error.to_string(), "Every topology network needs a name.");
    }

    #[test]
    fn missing_file_is_reported_by_path() {
        let missing = std::env::temp_dir().join("flowlog-topology-does-not-exist.json");
        let error = load(&missing).unwrap_err().to_string();
        assert_eq!(error, format!("Topology file not found: {}", missing.display()));
    }
}
