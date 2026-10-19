namespace AzureFlowLogParser.Models;

/// <summary>
/// Health states for flow log configurations and storage accounts.
/// Strings (not enums) because they are part of the JSON output contract.
/// </summary>
public static class FlowStatus
{
    public const string Healthy = "Healthy";
    public const string Stale = "Stale";
    public const string NoData = "NoData";
    public const string Disabled = "Disabled";
    public const string Unmanaged = "Unmanaged";
    public const string NoFlowLogs = "NoFlowLogs";
    public const string AccessDenied = "AccessDenied";
    public const string Error = "Error";

    // Worst first. Unmanaged and NoFlowLogs only apply to accounts without flow log configurations.
    private static readonly string[] Severity = { Error, AccessDenied, NoData, Stale, Disabled, Healthy };

    public static string Worst(IEnumerable<string> statuses)
    {
        var list = statuses.ToList();
        return Severity.FirstOrDefault(s => list.Contains(s)) ?? Healthy;
    }
}

/// <summary>
/// A storage account in a selected subscription, with its flow log status.
/// </summary>
public sealed class StorageAccountInfo
{
    public string SubscriptionId { get; set; } = string.Empty;
    public string Name { get; set; } = string.Empty;
    public string ResourceGroup { get; set; } = string.Empty;
    public string Location { get; set; } = string.Empty;
    public string Kind { get; set; } = string.Empty;
    public string SkuName { get; set; } = string.Empty;
    public int FlowLogConfigs { get; set; }
    public bool ContainerPresent { get; set; }
    public bool HasFlowLogs { get; set; }
    public string Status { get; set; } = FlowStatus.NoFlowLogs;
    public string? LatestBlobUtc { get; set; }
    public string? ObservedTarget { get; set; }
}

/// <summary>
/// A network security group or virtual network flow log configuration.
/// </summary>
public sealed class FlowLogConfigInfo
{
    public string SubscriptionId { get; set; } = string.Empty;
    public string Name { get; set; } = string.Empty;
    public string StorageAccount { get; set; } = string.Empty;
    public string NetworkWatcher { get; set; } = string.Empty;
    public string Location { get; set; } = string.Empty;
    public string Target { get; set; } = string.Empty;
    public string TargetKind { get; set; } = "Other";
    public bool Enabled { get; set; }
    public int RetentionDays { get; set; }
    public bool TrafficAnalytics { get; set; }
    public string Status { get; set; } = FlowStatus.Healthy;
    public string? LatestBlobUtc { get; set; }
    public double? AgeHours { get; set; }
    public string Message { get; set; } = string.Empty;
}

/// <summary>
/// Output of the discover command: accounts plus flow log configurations.
/// </summary>
public sealed class InventoryResult
{
    public string GeneratedUtc { get; set; } = string.Empty;
    public int StaleAfterHours { get; set; }
    public List<StorageAccountInfo> Accounts { get; set; } = new();
    public List<FlowLogConfigInfo> FlowLogs { get; set; } = new();
}
