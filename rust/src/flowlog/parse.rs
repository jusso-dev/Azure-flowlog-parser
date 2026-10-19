use serde::Deserialize;

use crate::model::DenormalizedRecord;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowLogRoot {
    records: Option<Vec<FlowLogRecord>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowLogRecord {
    #[serde(default)]
    time: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    operation_name: String,
    #[serde(default)]
    resource_id: String,
    properties: Option<FlowLogProperties>,
    flow_records: Option<FlowRecords>,
}

#[derive(Deserialize)]
struct FlowLogProperties {
    #[serde(rename = "Version", alias = "version", default)]
    version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowRecords {
    flows: Option<Vec<Flow>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Flow {
    #[serde(default)]
    rule: String,
    flow_groups: Option<Vec<FlowGroup>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowGroup {
    #[serde(default)]
    mac: String,
    flow_tuples: Option<Vec<String>>,
}

/// Flatten one PT1H.json blob into one record per flow tuple. Same semantics as FlowLogParser.cs.
pub fn parse_flow_log(json: &str) -> anyhow::Result<Vec<DenormalizedRecord>> {
    let root: FlowLogRoot = serde_json::from_str(json)?;
    let mut observations = Vec::new();
    for record in root.records.iter().flatten() {
        // A record without a properties block is version 1, as in the .NET reader.
        let version = record.properties.as_ref().map_or(1, |properties| properties.version);
        let flows = record.flow_records.as_ref().and_then(|records| records.flows.as_deref()).unwrap_or_default();
        for flow in flows {
            for group in flow.flow_groups.iter().flatten() {
                for tuple in group.flow_tuples.iter().flatten() {
                    observations.push(denormalize(record, version, flow, group, tuple));
                }
            }
        }
    }
    Ok(observations)
}

/// Tuple: start,srcIp,dstIp,srcPort,dstPort,proto,dir,action[,state,pktStoD,bytesStoD,pktDtoS,bytesDtoS].
fn denormalize(
    source: &FlowLogRecord,
    version: u32,
    flow: &Flow,
    group: &FlowGroup,
    tuple: &str,
) -> DenormalizedRecord {
    let parts: Vec<&str> = tuple.split(',').collect();
    let part = |index: usize| parts.get(index).copied().unwrap_or_default().to_string();
    // Counters exist from version 2. Empty or missing counters read as "0".
    let counter = |index: usize| match parts.get(index) {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => "0".to_string(),
    };

    let mut record = DenormalizedRecord {
        time: source.time.clone(),
        category: source.category.clone(),
        operation_name: source.operation_name.clone(),
        resource_id: source.resource_id.clone(),
        version,
        nsg_rule_name: flow.rule.clone(),
        mac: group.mac.clone(),
        start_time: part(0),
        source_address: part(1),
        destination_address: part(2),
        source_port: part(3),
        destination_port: part(4),
        transport_protocol: part(5),
        device_direction: part(6),
        device_action: part(7),
        ..Default::default()
    };

    if version >= 2 && parts.len() > 8 {
        record.flow_state = Some(part(8));
        record.packets_s_to_d = Some(counter(9));
        record.bytes_s_to_d = Some(counter(10));
        record.packets_d_to_s = Some(counter(11));
        record.bytes_d_to_s = Some(counter(12));
    }
    record
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn version_1_blob() -> String {
        json!({
            "records": [{
                "time": "2026-10-18T10:00:00.0000000Z",
                "category": "NetworkSecurityGroupFlowEvent",
                "operationName": "NetworkSecurityGroupFlowEvents",
                "resourceId": "/SUBSCRIPTIONS/0000/RESOURCEGROUPS/RG/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG",
                "properties": { "Version": 1 },
                "flowRecords": { "flows": [{
                    "rule": "DefaultRule_AllowInternetOutBound",
                    "flowGroups": [{
                        "mac": "000D3A000001",
                        "flowTuples": [
                            "1760781600,10.0.1.4,13.1.2.3,51000,443,T,O,A",
                            "1760781601,10.0.1.4,10.0.2.5,51001,1433,T,I,D"
                        ]
                    }]
                }]}
            }]
        })
        .to_string()
    }

    fn version_2_blob() -> String {
        json!({
            "records": [{
                "time": "2026-10-18T10:00:00.0000000Z",
                "properties": { "Version": 2 },
                "flowRecords": { "flows": [{
                    "rule": "UserRule_AllowSql",
                    "flowGroups": [{
                        "mac": "000D3A000002",
                        "flowTuples": [
                            "1760781600,10.0.2.5,10.0.1.4,1433,51000,T,I,A,B,3,1500,2,900",
                            "1760781601,10.0.2.5,10.0.1.4,1433,51000,U,I,A,C,,,,"
                        ]
                    }]
                }]}
            }]
        })
        .to_string()
    }

    #[test]
    fn version_1_tuples_have_no_counters() {
        let records = parse_flow_log(&version_1_blob()).unwrap();
        assert_eq!(records.len(), 2);

        let first = &records[0];
        assert_eq!(first.version, 1);
        assert_eq!(first.nsg_rule_name, "DefaultRule_AllowInternetOutBound");
        assert_eq!(first.mac, "000D3A000001");
        assert_eq!(first.start_time, "1760781600");
        assert_eq!(first.source_address, "10.0.1.4");
        assert_eq!(first.destination_address, "13.1.2.3");
        assert_eq!(first.destination_port, "443");
        assert_eq!(first.transport_protocol, "T");
        assert_eq!(first.device_direction, "O");
        assert_eq!(first.device_action, "A");
        assert_eq!(first.flow_state, None);
        assert_eq!(first.bytes_s_to_d, None);
        assert_eq!(first.bytes_d_to_s, None);
        assert_eq!(first.operation_name, "NetworkSecurityGroupFlowEvents");
    }

    #[test]
    fn version_2_tuples_read_counters_and_empty_counters_as_zero() {
        let records = parse_flow_log(&version_2_blob()).unwrap();
        assert_eq!(records.len(), 2);

        let full = &records[0];
        assert_eq!(full.version, 2);
        assert_eq!(full.flow_state.as_deref(), Some("B"));
        assert_eq!(full.packets_s_to_d.as_deref(), Some("3"));
        assert_eq!(full.bytes_s_to_d.as_deref(), Some("1500"));
        assert_eq!(full.packets_d_to_s.as_deref(), Some("2"));
        assert_eq!(full.bytes_d_to_s.as_deref(), Some("900"));

        let empty = &records[1];
        assert_eq!(empty.flow_state.as_deref(), Some("C"));
        assert_eq!(empty.packets_s_to_d.as_deref(), Some("0"));
        assert_eq!(empty.bytes_s_to_d.as_deref(), Some("0"));
        assert_eq!(empty.packets_d_to_s.as_deref(), Some("0"));
        assert_eq!(empty.bytes_d_to_s.as_deref(), Some("0"));
    }

    #[test]
    fn records_without_flow_records_produce_nothing() {
        let json = r#"{ "records": [{ "time": "t" }, { "flowRecords": { "flows": null } }] }"#;
        assert!(parse_flow_log(json).unwrap().is_empty());
        assert!(parse_flow_log(r#"{ "records": null }"#).unwrap().is_empty());
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_flow_log("{ not json").is_err());
    }
}
