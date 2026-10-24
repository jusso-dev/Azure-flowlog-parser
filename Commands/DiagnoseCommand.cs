using System.CommandLine;
using System.Text;
using System.Text.Json;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Serialization;
using AzureFlowLogParser.Services.Discovery;

namespace AzureFlowLogParser.Commands;

/// <summary>
/// "diagnose": every virtual network, subnet, NIC, and attached NSG that is not writing flow logs to storage, with the reason.
/// </summary>
public static class DiagnoseCommand
{
    public static Command Create()
    {
        var subscriptionOption = new Option<string[]>("--subscription")
        {
            Description = "Subscription id to scan (repeat for several). Default: every enabled subscription the identity can read."
        };

        var storageAccountOption = new Option<string[]>("--storage-account", "-s")
        {
            Description = "Only show gaps for these storage accounts. Sources with no flow log are still listed."
        };

        var staleHoursOption = new Option<int>("--stale-hours")
        {
            Description = "A flow log is Stale when its newest blob is older than this many hours.",
            DefaultValueFactory = _ => 3
        };

        var outputFormatOption = new Option<string>("--output-format")
        {
            Description = "table (default) or json.",
            DefaultValueFactory = _ => "table"
        };

        var outputOption = new Option<string?>("--output", "-o")
        {
            Description = "Write output to this file instead of stdout."
        };

        var verboseOption = new Option<bool>("--verbose", "-v")
        {
            Description = "Print progress to stderr."
        };

        var command = new Command("diagnose", "Find virtual networks, subnets, NICs, and NSGs that are not sending flow logs to storage, and why");
        command.Add(subscriptionOption);
        command.Add(storageAccountOption);
        command.Add(staleHoursOption);
        command.Add(outputFormatOption);
        command.Add(outputOption);
        command.Add(verboseOption);

        command.SetAction((ParseResult parseResult, CancellationToken cancellationToken) => CommandRunner.RunAsync(async () =>
        {
            var subscriptions = parseResult.GetValue(subscriptionOption) ?? Array.Empty<string>();
            var storageAccounts = parseResult.GetValue(storageAccountOption) ?? Array.Empty<string>();
            var staleHours = parseResult.GetValue(staleHoursOption);
            var outputFormat = (parseResult.GetValue(outputFormatOption) ?? "table").ToLowerInvariant();
            var output = parseResult.GetValue(outputOption);
            var verbose = parseResult.GetValue(verboseOption);

            if (outputFormat is not ("table" or "json"))
            {
                await Console.Error.WriteLineAsync("Error: --output-format must be table or json.");
                return 1;
            }

            if (staleHours < 1)
            {
                await Console.Error.WriteLineAsync("Error: --stale-hours must be at least 1.");
                return 1;
            }

            var log = Logger.For(verbose);
            // Health-check every account. The storage filter only narrows the printed gaps.
            var load = await new InventoryService(new AzureClients())
                .LoadAsync(new InventoryRequest(subscriptions, Array.Empty<string>(), staleHours, true), log, cancellationToken);
            var result = FlowLogDiagnostician.Diagnose(load.Topology, load.Topology.FlowLogs, staleHours, DateTimeOffset.UtcNow, storageAccounts);

            var text = outputFormat == "json"
                ? JsonSerializer.Serialize(result, IndentedJsonContext.Default.DiagnosisResult)
                : RenderTable(result);

            if (string.IsNullOrWhiteSpace(output))
            {
                Console.Out.Write(text + Environment.NewLine);
            }
            else
            {
                await File.WriteAllTextAsync(output, text + Environment.NewLine, cancellationToken);
                Console.Error.WriteLine($"Diagnosis written to: {output}");
            }

            return 0;
        }, parseResult.GetValue(verboseOption)));

        return command;
    }

    private static string RenderTable(DiagnosisResult result)
    {
        var builder = new StringBuilder();
        var writer = new StringWriter(builder);
        writer.WriteLine($"Sources not sending flow logs ({result.FindingCount} of {result.SourceCount}) - generated {result.GeneratedUtc}, stale after {result.StaleAfterHours}h");
        writer.WriteLine();
        ConsoleTable.Write(writer,
            ["Kind", "Name", "Resource group", "Location", "Reason", "Flow log", "Storage account"],
            result.Findings.Select(finding => new[]
            {
                finding.Kind,
                finding.Name,
                finding.ResourceGroup,
                string.IsNullOrEmpty(finding.Location) ? "-" : finding.Location,
                finding.Reason,
                string.IsNullOrEmpty(finding.FlowLog) ? "-" : finding.FlowLog,
                string.IsNullOrEmpty(finding.StorageAccount) ? "-" : finding.StorageAccount
            }).ToList());

        foreach (var finding in result.Findings.Where(finding => finding.Message.Length > 0))
        {
            writer.WriteLine($"  {finding.Kind} {finding.Name}: {finding.Message}");
        }

        return builder.ToString().TrimEnd();
    }
}
