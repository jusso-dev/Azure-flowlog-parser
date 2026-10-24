using System.Text.RegularExpressions;

namespace AzureFlowLogParser.Services.Discovery;

/// <summary>
/// Blob layout helpers for Azure VNet flow logs:
/// resourceId=/SUBSCRIPTIONS/.../NETWORKSECURITYGROUPS/NAME/y=YYYY/m=MM/d=DD/h=HH/m=00/macAddress=.../PT1H.json
/// </summary>
public static partial class FlowLogPaths
{
    public const string ContainerName = "insights-logs-flowlogflowevent";

    /// <summary>Blob prefix for one target resource and one UTC day.</summary>
    public static string DayPrefix(string targetResourceId, DateTime utcDay)
    {
        return $"resourceId={targetResourceId.ToUpperInvariant()}/y={utcDay:yyyy}/m={utcDay:MM}/d={utcDay:dd}/";
    }

    /// <summary>Prefix that matches every flow log blob for a container (used for unmanaged accounts).</summary>
    public const string AllResourcesPrefix = "resourceId=/";

    public static string TargetKind(string targetResourceId)
    {
        if (targetResourceId.Contains("/networkSecurityGroups/", StringComparison.OrdinalIgnoreCase))
        {
            return "NSG";
        }

        if (targetResourceId.Contains("/networkInterfaces/", StringComparison.OrdinalIgnoreCase))
        {
            return "NIC";
        }

        // A subnet id also contains /virtualNetworks/, so the subnet segment wins.
        if (targetResourceId.Contains("/subnets/", StringComparison.OrdinalIgnoreCase))
        {
            return "Subnet";
        }

        if (targetResourceId.Contains("/virtualNetworks/", StringComparison.OrdinalIgnoreCase))
        {
            return "VNet";
        }

        return "Other";
    }

    /// <summary>Last segment of the target's ARM id: the NSG or VNet name.</summary>
    public static string TargetName(string targetResourceId)
    {
        var parts = targetResourceId.Split('/', StringSplitOptions.RemoveEmptyEntries);
        return parts.Length == 0 ? string.Empty : parts[^1];
    }

    /// <summary>Resource group of the target's ARM id (case-insensitive match), or empty when the id has none.</summary>
    public static string TargetResourceGroup(string targetResourceId)
    {
        var match = ResourceGroupRegex().Match(targetResourceId);
        return match.Success ? match.Groups["group"].Value : string.Empty;
    }

    /// <summary>Extracts the ARM target id from a flow log blob name, or null when the name does not match.</summary>
    public static string? TargetFromBlobName(string blobName)
    {
        var match = BlobTargetRegex().Match(blobName);
        return match.Success ? match.Groups["target"].Value : null;
    }

    [GeneratedRegex(@"^resourceId=(?<target>/SUBSCRIPTIONS/[^/]+/RESOURCEGROUPS/[^/]+/PROVIDERS/MICROSOFT\.NETWORK/[^/]+/[^/]+)/y=", RegexOptions.IgnoreCase)]
    private static partial Regex BlobTargetRegex();

    [GeneratedRegex(@"/resourceGroups/(?<group>[^/]+)", RegexOptions.IgnoreCase)]
    private static partial Regex ResourceGroupRegex();
}
