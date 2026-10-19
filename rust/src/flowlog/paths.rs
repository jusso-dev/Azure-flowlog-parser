use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;

pub const CONTAINER_NAME: &str = "insights-logs-flowlogflowevent";
pub const ALL_RESOURCES_PREFIX: &str = "resourceId=/";

static BLOB_TARGET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^resourceId=(?P<target>/SUBSCRIPTIONS/[^/]+/RESOURCEGROUPS/[^/]+/PROVIDERS/MICROSOFT\.NETWORK/[^/]+/[^/]+)/y=")
        .expect("valid blob target pattern")
});

/// Blob prefix for one flow log target (NSG or VNet ARM id) and one UTC day. The target is upper-cased.
pub fn day_prefix(target_resource_id: &str, day: NaiveDate) -> String {
    format!("resourceId={}/y={}/", target_resource_id.to_uppercase(), day.format("%Y/m=%m/d=%d"))
}

/// "NSG", "VNet" or "Other", from the ARM type segment of the target.
pub fn target_kind(target_resource_id: &str) -> &'static str {
    let lower = target_resource_id.to_ascii_lowercase();
    if lower.contains("/networksecuritygroups/") {
        "NSG"
    } else if lower.contains("/virtualnetworks/") {
        "VNet"
    } else {
        "Other"
    }
}

/// ARM target id parsed from a flow log blob name, or None when the name does not match.
pub fn target_from_blob_name(blob_name: &str) -> Option<String> {
    BLOB_TARGET.captures(blob_name).map(|captures| captures["target"].to_string())
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    const NSG: &str =
        "/subscriptions/0000/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-app";

    #[test]
    fn day_prefix_uppercases_target_and_pads_date() {
        let day = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        assert_eq!(
            day_prefix(NSG, day),
            "resourceId=/SUBSCRIPTIONS/0000/RESOURCEGROUPS/RG-NET/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-APP/y=2026/m=03/d=07/"
        );
    }

    #[test]
    fn target_kind_matches_arm_type_case_insensitively() {
        assert_eq!(target_kind(NSG), "NSG");
        assert_eq!(
            target_kind("/subscriptions/0000/resourceGroups/rg/providers/Microsoft.Network/VirtualNetworks/vnet"),
            "VNet"
        );
        assert_eq!(
            target_kind("/subscriptions/0000/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/st"),
            "Other"
        );
    }

    #[test]
    fn target_from_blob_name_extracts_arm_id() {
        let blob = "resourceId=/SUBSCRIPTIONS/0000/RESOURCEGROUPS/RG-NET/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-APP/y=2026/m=03/d=07/h=10/m=00/macAddress=000D3A/PT1H.json";
        assert_eq!(
            target_from_blob_name(blob).as_deref(),
            Some("/SUBSCRIPTIONS/0000/RESOURCEGROUPS/RG-NET/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-APP")
        );
        assert_eq!(target_from_blob_name("resourceId=/other/y=2026/PT1H.json"), None);
    }
}
