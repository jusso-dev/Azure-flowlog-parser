namespace AzureFlowLogParser.Models;

/// <summary>
/// Why a virtual network, subnet, NIC, or NSG is not writing flow logs to a storage account.
/// Strings because they are part of the JSON output contract.
/// </summary>
public static class DiagnosisReason
{
    public const string NotConfigured = "NotConfigured";
    public const string NetworkWatcherMissing = "NetworkWatcherMissing";
    public const string Disabled = "Disabled";
    public const string ProvisioningFailed = "ProvisioningFailed";
    public const string TargetMissing = "TargetMissing";
    public const string StorageMissing = "StorageMissing";
    public const string RegionMismatch = "RegionMismatch";
    public const string UnsupportedStorage = "UnsupportedStorage";
    public const string StorageFirewall = "StorageFirewall";
    public const string SharedKeyDisabled = "SharedKeyDisabled";
    public const string Error = "Error";
    public const string AccessDenied = "AccessDenied";
    public const string NoData = "NoData";
    public const string Stale = "Stale";

    // Worst first. Used when several enabled flow logs target one source and none is delivering.
    private static readonly string[] Severity =
    {
        ProvisioningFailed, TargetMissing, StorageMissing, RegionMismatch, UnsupportedStorage,
        StorageFirewall, SharedKeyDisabled, Error, AccessDenied, NoData, Stale, Disabled,
        NetworkWatcherMissing, NotConfigured
    };

    public static int Rank(string reason)
    {
        var index = Array.IndexOf(Severity, reason);
        return index < 0 ? Severity.Length : index;
    }
}

/// <summary>One source that should be sending flow logs to storage but is not.</summary>
public sealed class FlowLogFinding
{
    public string SubscriptionId { get; set; } = string.Empty;
    public string ResourceGroup { get; set; } = string.Empty;
    public string Location { get; set; } = string.Empty;
    public string Kind { get; set; } = string.Empty;
    public string Name { get; set; } = string.Empty;
    public string ResourceId { get; set; } = string.Empty;
    public string Reason { get; set; } = string.Empty;
    public string FlowLog { get; set; } = string.Empty;
    public string StorageAccount { get; set; } = string.Empty;
    public string Message { get; set; } = string.Empty;
}

/// <summary>Output of the diagnose command.</summary>
public sealed class DiagnosisResult
{
    public string GeneratedUtc { get; set; } = string.Empty;
    public int StaleAfterHours { get; set; }
    public int SourceCount { get; set; }
    public int FindingCount { get; set; }
    public List<FlowLogFinding> Findings { get; set; } = new();
}
