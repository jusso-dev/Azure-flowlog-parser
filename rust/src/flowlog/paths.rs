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

/// "NSG", "NIC", "Subnet", "VNet" or "Other". A subnet id also contains `/virtualNetworks/`, so subnet is tested first.
pub fn target_kind(target_resource_id: &str) -> &'static str {
    let lower = target_resource_id.to_ascii_lowercase();
    if lower.contains("/networksecuritygroups/") {
        "NSG"
    } else if lower.contains("/networkinterfaces/") {
        "NIC"
    } else if lower.contains("/subnets/") {
        "Subnet"
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

/// Name of a flow log target: the last segment of its ARM id (the NSG or VNet name).
pub fn target_name(target_resource_id: &str) -> String {
    last_segment(target_resource_id)
}

/// Resource group of a flow log target, or empty when the id has no resourceGroups segment.
pub fn target_resource_group(target_resource_id: &str) -> String {
    resource_group_of(target_resource_id)
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
            target_kind(
                "/subscriptions/0000/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/vnet/subnets/app"
            ),
            "Subnet"
        );
        assert_eq!(
            target_kind("/subscriptions/0000/resourceGroups/rg/providers/Microsoft.Network/networkInterfaces/nic1"),
            "NIC"
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

    #[test]
    fn target_name_and_resource_group_read_nsg_and_vnet_ids() {
        let vnet = "/subscriptions/0000/resourceGroups/RG-Net/providers/Microsoft.Network/virtualNetworks/hub-vnet";
        assert_eq!(target_name(NSG), "nsg-app");
        assert_eq!(target_resource_group(NSG), "rg-net");
        assert_eq!(target_name(vnet), "hub-vnet");
        assert_eq!(target_resource_group(vnet), "RG-Net");
    }

    #[test]
    fn target_without_resource_group_segment_has_empty_group() {
        let id = "/subscriptions/0000/providers/Microsoft.Network/virtualNetworks/hub/";
        assert_eq!(target_name(id), "hub");
        assert_eq!(target_resource_group(id), "");
        assert_eq!(target_name(""), "");
    }

    #[test]
    fn arm_id_helpers_ignore_case_and_trailing_slashes() {
        let id = "/subscriptions/s/RESOURCEGROUPS/My-RG/providers/Microsoft.Storage/storageAccounts/acct/";
        assert_eq!(resource_group_of(id), "My-RG");
        assert_eq!(last_segment(id), "acct");
        assert_eq!(resource_group_of("/subscriptions/s"), "");
    }
}
