namespace AzureFlowLogParser.Models;

/// <summary>
/// Traffic report model. JSON shape is the cross-language contract shared with the PowerShell modules
/// (see SPEC section 3); it is also embedded in the HTML report.
/// </summary>
public sealed class ReportModel
{
    public int Schema { get; set; } = 1;
    public string GeneratedUtc { get; set; } = string.Empty;
    public string Source { get; set; } = "azure";
    public ReportWindow Window { get; set; } = new();
    public ReportFilters Filters { get; set; } = new();
    public ReportSummary Summary { get; set; } = new();
    public List<ReportScope> Scopes { get; set; } = new();
    public List<ReportFlow> Flows { get; set; } = new();
    public List<StorageAccountInfo> Accounts { get; set; } = new();
    public List<FlowLogConfigInfo> FlowLogs { get; set; } = new();
    public List<string> Notes { get; set; } = new();
}

public sealed class ReportWindow
{
    public string FromUtc { get; set; } = string.Empty;
    public string ToUtc { get; set; } = string.Empty;
}

public sealed class ReportFilters
{
    public string Action { get; set; } = "all";
    public string Protocol { get; set; } = "all";
    public string Ports { get; set; } = string.Empty;
    public string Ip { get; set; } = string.Empty;
    public long MinBytes { get; set; }
}

public sealed class ReportSummary
{
    public long Flows { get; set; }
    public long Allowed { get; set; }
    public long Denied { get; set; }
    public long Bytes { get; set; }
    public long Packets { get; set; }
    public int Rows { get; set; }
    public int RowsTotal { get; set; }
    public bool RowsTruncated { get; set; }
    public int StorageAccounts { get; set; }
    public int Scopes { get; set; }
    public int Subscriptions { get; set; }
    public long ObservationsSkipped { get; set; }
    public int UniqueSourceIps { get; set; }
    public int UniqueDestinationIps { get; set; }
}

/// <summary>
/// A network scope that flows are attributed to: a storage account, a VNet without flow logs,
/// the internet or unmapped private addresses.
/// </summary>
public sealed class ReportScope
{
    public string Id { get; set; } = string.Empty;
    public string Kind { get; set; } = string.Empty;
    public string Label { get; set; } = string.Empty;
    public string SubscriptionId { get; set; } = string.Empty;
    public string ResourceGroup { get; set; } = string.Empty;
    public string StorageAccount { get; set; } = string.Empty;
}

public static class ScopeKind
{
    public const string Storage = "storage";
    public const string Vnet = "vnet";
    public const string Internet = "internet";
    public const string Unmapped = "unmapped";
}

/// <summary>
/// One aggregated row: observations sharing the same key (see SPEC section 3).
/// </summary>
public sealed class ReportFlow
{
    public string First { get; set; } = string.Empty;
    public string Last { get; set; } = string.Empty;
    public string Src { get; set; } = string.Empty;
    public string Dst { get; set; } = string.Empty;
    public string SrcScope { get; set; } = string.Empty;
    public string DstScope { get; set; } = string.Empty;
    public string Proto { get; set; } = string.Empty;
    public int DstPort { get; set; }
    public string Action { get; set; } = string.Empty;
    public string Direction { get; set; } = string.Empty;
    public string Rule { get; set; } = string.Empty;
    public long Flows { get; set; }
    public long Packets { get; set; }
    public long Bytes { get; set; }
}
