using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Common;
using AzureFlowLogParser.Services.Reporting.Pdf;

namespace AzureFlowLogParser.Services.Reporting;

/// <summary>Static PDF version of the traffic report: same sections as the HTML report, filters applied at generation.</summary>
public static class PdfReportWriter
{
    private const int TopEdges = 50;
    private const int TopFlows = 150;

    public static void Write(ReportModel model, string path)
    {
        var document = new PdfDocument();
        var layout = new PdfLayout(document);

        layout.Title("Azure VNet Flow Traffic Report");
        layout.Paragraph($"Generated {model.GeneratedUtc} | Source {model.Source} | Window {model.Window.FromUtc} to {model.Window.ToUtc}", 8.5, muted: true);
        layout.Paragraph($"Filters: action {model.Filters.Action}; protocol {model.Filters.Protocol}; destination ports {Or(model.Filters.Ports)}; IP contains {Or(model.Filters.Ip)}; min bytes {model.Filters.MinBytes}", 8.5, muted: true);

        WriteSummary(layout, model);
        WriteMonitoring(layout, model);
        WriteEdges(layout, model);
        WriteStorageAccounts(layout, model);
        WriteTopFlows(layout, model);
        WriteNotes(layout, model);

        layout.Footers();
        File.WriteAllBytes(path, document.Build());
    }

    private static void WriteSummary(PdfLayout layout, ReportModel model)
    {
        var s = model.Summary;
        layout.Heading("Summary");
        var rows = new List<PdfCell[]>
        {
            Row("Tenant", TenantText(model)),
            Row("Subscriptions", SubscriptionText(model)),
            Row("Flow observations", s.Flows.ToString("N0")),
            Row("Allowed / denied", $"{s.Allowed:N0} / {s.Denied:N0}"),
            Row("Bytes / packets", $"{FormatBytes(s.Bytes)} / {s.Packets:N0}"),
            Row("Storage accounts read", s.StorageAccounts.ToString("N0")),
            Row("Scopes in report", s.Scopes.ToString("N0")),
            Row("Unique source / destination IPs", $"{s.UniqueSourceIps:N0} / {s.UniqueDestinationIps:N0}"),
            Row("Allowed inbound duplicates skipped", s.ObservationsSkipped.ToString("N0"))
        };
        layout.Table(["Measure", "Value"], [170, 345], rows, fontSize: 8.5);
    }

    private static void WriteMonitoring(PdfLayout layout, ReportModel model)
    {
        layout.Heading("Flow log monitoring");
        if (model.Accounts.Count == 0 && model.FlowLogs.Count == 0)
        {
            layout.Paragraph("No storage account inventory in this run (local input or no accounts discovered).", muted: true);
            return;
        }

        var accountRows = model.Accounts
            .Select(a => new[]
            {
                Cell(a.Name), Cell(a.ResourceGroup.Length > 0 ? a.ResourceGroup : "-"), Cell(SubscriptionName(model, a.SubscriptionId)),
                Cell(a.Status), Cell(a.FlowLogConfigs.ToString()), Cell(a.LatestBlobUtc is null ? "-" : ShortTime(a.LatestBlobUtc)),
                Cell(a.ObservedTarget is null ? "-" : LastSegment(a.ObservedTarget))
            })
            .ToList();
        layout.Table(["Storage account", "Resource group", "Subscription", "Status", "Configs", "Latest blob", "Unmanaged source"],
            [88, 84, 110, 60, 38, 75, 60], accountRows);

        var logRows = model.FlowLogs
            .Select(f => new[]
            {
                Cell(f.StorageAccount), Cell(f.Name.Length > 0 ? f.Name : "-"),
                Cell(f.TargetName.Length > 0 ? f.TargetName : LastSegment(f.Target)),
                Cell(f.TargetResourceGroup.Length > 0 ? f.TargetResourceGroup : "-"),
                Cell(f.TargetKind), Cell(f.Enabled ? "yes" : "no"), Cell(f.Status),
                Cell(f.AgeHours is null ? "-" : $"{f.AgeHours:0.0} h")
            })
            .ToList();
        if (logRows.Count > 0)
        {
            layout.Space(4);
            layout.Table(["Storage account", "Flow log", "Target", "Target RG", "Kind", "Enabled", "Status", "Age"],
                [78, 92, 88, 92, 34, 38, 58, 35], logRows);
        }
    }

    private static void WriteEdges(PdfLayout layout, ReportModel model)
    {
        layout.Heading($"Traffic between scopes (top {TopEdges} by bytes)");
        var edges = Edges(model).OrderByDescending(e => e.Bytes).ThenByDescending(e => e.Flows).Take(TopEdges).ToList();
        if (edges.Count == 0)
        {
            layout.Paragraph("No traffic matched the filters.", muted: true);
            return;
        }

        var max = Math.Max(1, edges.Max(e => e.Bytes));
        var rows = edges
            .Select(e => new[]
            {
                Cell(LabelOf(model, e.Src)), Cell(LabelOf(model, e.Dst)), Cell(e.Flows.ToString("N0")),
                Cell(e.Allowed.ToString("N0")), Cell(e.Denied.ToString("N0")), Cell(FormatBytes(e.Bytes)),
                new PdfCell(string.Empty, (double)e.Bytes / max)
            })
            .ToList();
        layout.Table(["From", "To", "Flows", "Allowed", "Denied", "Bytes", "Share of bytes"],
            [110, 110, 46, 50, 46, 60, 93], rows);
    }

    private static void WriteStorageAccounts(PdfLayout layout, ReportModel model)
    {
        layout.Heading("Individual view per storage account");
        var storageScopes = model.Scopes.Where(s => s.Kind == ScopeKind.Storage).ToList();
        if (storageScopes.Count == 0)
        {
            layout.Paragraph("No storage account scopes appear in the matched traffic.", muted: true);
            return;
        }

        var flows = model.Flows;
        foreach (var scope in storageScopes.OrderBy(s => s.Id, StringComparer.Ordinal))
        {
            var inbound = flows.Where(f => f.DstScope == scope.Id).ToList();
            var outbound = flows.Where(f => f.SrcScope == scope.Id).ToList();
            var denied = flows.Where(f => (f.SrcScope == scope.Id || f.DstScope == scope.Id) && f.Action == "D").Sum(f => f.Flows);

            layout.Paragraph($"{scope.Id}", 10, bold: true);
            layout.Paragraph(
                $"In: {inbound.Sum(f => f.Flows):N0} flows, {FormatBytes(inbound.Sum(f => f.Bytes))} | " +
                $"Out: {outbound.Sum(f => f.Flows):N0} flows, {FormatBytes(outbound.Sum(f => f.Bytes))} | " +
                $"Denied observations: {denied:N0}", 8.5);

            var peers = inbound.Select(f => (Peer: LabelOf(model, f.SrcScope), f.Bytes))
                .Concat(outbound.Select(f => (Peer: LabelOf(model, f.DstScope), f.Bytes)))
                .GroupBy(p => p.Peer, StringComparer.Ordinal)
                .OrderByDescending(g => g.Sum(p => p.Bytes))
                .Take(5)
                .Select(g => $"{g.Key} ({FormatBytes(g.Sum(p => p.Bytes))})");

            var ports = flows.Where(f => f.SrcScope == scope.Id || f.DstScope == scope.Id)
                .GroupBy(f => $"{f.DstPort}/{f.Proto}", StringComparer.Ordinal)
                .OrderByDescending(g => g.Sum(f => f.Bytes))
                .Take(5)
                .Select(g => $"{g.Key} ({FormatBytes(g.Sum(f => f.Bytes))})");

            layout.Paragraph($"Top peers: {string.Join(", ", peers.DefaultIfEmpty("none"))}", 8.5);
            layout.Paragraph($"Top destination ports: {string.Join(", ", ports.DefaultIfEmpty("none"))}", 8.5);
            layout.Space(6);
        }
    }

    private static void WriteTopFlows(PdfLayout layout, ReportModel model)
    {
        var s = model.Summary;
        layout.Heading($"Top flows (top {Math.Min(TopFlows, model.Flows.Count)} of {s.RowsTotal:N0} rows by bytes)");
        if (model.Flows.Count == 0)
        {
            layout.Paragraph("No flow rows matched the filters.", muted: true);
            return;
        }

        var rows = model.Flows.Take(TopFlows)
            .Select(f => new[]
            {
                Cell(ShortTime(f.First)), Cell(f.Src), Cell(f.Dst), Cell(f.DstPort.ToString()), Cell(f.Proto),
                Cell(f.Action == "A" ? "allow" : "deny"), Cell(f.Direction == "I" ? "in" : "out"),
                Cell(f.Flows.ToString("N0")), Cell(FormatBytes(f.Bytes)), Cell(f.Rule)
            })
            .ToList();
        layout.Table(["First seen", "Source", "Destination", "Port", "Proto", "Action", "Dir", "Flows", "Bytes", "Rule"],
            [76, 70, 70, 32, 32, 36, 24, 40, 52, 83], rows, fontSize: 7);
    }

    private static void WriteNotes(PdfLayout layout, ReportModel model)
    {
        layout.Heading("Notes");
        foreach (var note in model.Notes)
        {
            layout.Bullet(note);
        }
    }

    private static IEnumerable<(string Src, string Dst, long Flows, long Allowed, long Denied, long Bytes)> Edges(ReportModel model)
    {
        return model.Flows
            .GroupBy(f => (f.SrcScope, f.DstScope))
            .Select(g => (g.Key.SrcScope, g.Key.DstScope, g.Sum(f => f.Flows),
                g.Where(f => f.Action == "A").Sum(f => f.Flows),
                g.Where(f => f.Action == "D").Sum(f => f.Flows),
                g.Sum(f => f.Bytes)));
    }

    private static string LabelOf(ReportModel model, string scopeId)
    {
        var scope = model.Scopes.FirstOrDefault(s => s.Id == scopeId);
        return scope?.Label ?? scopeId;
    }

    private const string NotAvailable = "not available (offline input)";

    /// <summary>Distinct tenant ids in report order, or the offline fallback when none is known.</summary>
    private static string TenantText(ReportModel model)
    {
        var tenants = model.Subscriptions.Select(s => s.TenantId).Where(t => t.Length > 0).Distinct().ToList();
        return tenants.Count > 0 ? string.Join(", ", tenants) : NotAvailable;
    }

    /// <summary>"displayName (subscriptionId)" per subscription, or the id alone when it has no display name.</summary>
    private static string SubscriptionText(ReportModel model)
    {
        return model.Subscriptions.Count > 0
            ? string.Join("; ", model.Subscriptions.Select(s => s.DisplayName.Length > 0 ? $"{s.DisplayName} ({s.SubscriptionId})" : s.SubscriptionId))
            : NotAvailable;
    }

    /// <summary>Display name of a subscription in the report, falling back to its id.</summary>
    private static string SubscriptionName(ReportModel model, string subscriptionId)
    {
        var subscription = model.Subscriptions.FirstOrDefault(s => string.Equals(s.SubscriptionId, subscriptionId, StringComparison.OrdinalIgnoreCase));
        return subscription is not null && subscription.DisplayName.Length > 0 ? subscription.DisplayName : subscriptionId;
    }

    private static string LastSegment(string armId)
    {
        var parts = armId.Split('/', StringSplitOptions.RemoveEmptyEntries);
        return parts.Length == 0 ? armId : parts[^1];
    }

    private static string ShortTime(string iso)
    {
        return iso.Length >= 16 ? iso[..16].Replace('T', ' ') : iso;
    }

    private static string Or(string value) => string.IsNullOrEmpty(value) ? "any" : value;

    private static PdfCell Cell(string text) => new(text);

    private static PdfCell[] Row(string label, string value) => [new PdfCell(label), new PdfCell(value)];

    public static string FormatBytes(long bytes)
    {
        string[] units = ["B", "KB", "MB", "GB", "TB"];
        double value = bytes;
        var unit = 0;
        while (value >= 1024 && unit < units.Length - 1)
        {
            value /= 1024;
            unit++;
        }

        return unit == 0 ? $"{bytes:N0} B" : $"{value:0.0} {units[unit]}";
    }
}
