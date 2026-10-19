using System.CommandLine;
using System.Text;
using System.Text.Json;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Serialization;
using AzureFlowLogParser.Services.Discovery;

namespace AzureFlowLogParser.Commands;

/// <summary>
/// "discover": lists storage accounts that receive flow logs or contain flow log blobs, with health per configuration.
/// </summary>
public static class DiscoverCommand
{
    public static Command Create()
    {
        var subscriptionOption = new Option<string[]>("--subscription")
        {
            Description = "Subscription id to scan (repeat for several). Default: every enabled subscription the identity can read."
        };

        var storageAccountOption = new Option<string[]>("--storage-account", "-s")
        {
            Description = "Limit to these storage account names (repeat for several)."
        };

        var staleHoursOption = new Option<int>("--stale-hours")
        {
            Description = "A flow log is Stale when its newest blob is older than this many hours.",
            DefaultValueFactory = _ => 3
        };

        var allOption = new Option<bool>("--all")
        {
            Description = "Also list storage accounts with no flow log configuration and no flow log blobs."
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

        var command = new Command("discover", "Discover storage accounts with flow logs per subscription and check that flow logs are being written");
        command.Add(subscriptionOption);
        command.Add(storageAccountOption);
        command.Add(staleHoursOption);
        command.Add(allOption);
        command.Add(outputFormatOption);
        command.Add(outputOption);
        command.Add(verboseOption);

        command.SetAction((ParseResult parseResult, CancellationToken cancellationToken) => CommandRunner.RunAsync(async () =>
        {
            var subscriptions = parseResult.GetValue(subscriptionOption) ?? Array.Empty<string>();
            var storageAccounts = parseResult.GetValue(storageAccountOption) ?? Array.Empty<string>();
            var staleHours = parseResult.GetValue(staleHoursOption);
            var includeAll = parseResult.GetValue(allOption);
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
            var load = await new InventoryService(new AzureClients())
                .LoadAsync(new InventoryRequest(subscriptions, storageAccounts, staleHours, includeAll), log, cancellationToken);

            var text = outputFormat == "json"
                ? JsonSerializer.Serialize(load.Result, IndentedJsonContext.Default.InventoryResult)
                : RenderTable(load.Result);

            if (string.IsNullOrWhiteSpace(output))
            {
                Console.Out.Write(text + Environment.NewLine);
            }
            else
            {
                await File.WriteAllTextAsync(output, text + Environment.NewLine, cancellationToken);
                Console.Error.WriteLine($"Inventory written to: {output}");
            }

            return 0;
        }, parseResult.GetValue(verboseOption)));

        return command;
    }

    private static string RenderTable(InventoryResult result)
    {
        var builder = new StringBuilder();
        var writer = new StringWriter(builder);

        writer.WriteLine($"Storage accounts with flow logs ({result.Accounts.Count}) - generated {result.GeneratedUtc}, stale after {result.StaleAfterHours}h");
        writer.WriteLine();
        ConsoleTable.Write(writer,
            ["Subscription", "Storage account", "Status", "Configs", "Container", "Latest blob (UTC)", "Unmanaged source"],
            result.Accounts.Select(a => new[]
            {
                a.SubscriptionId,
                a.Name,
                a.Status,
                a.FlowLogConfigs.ToString(),
                a.ContainerPresent ? "yes" : "no",
                a.LatestBlobUtc ?? "-",
                a.ObservedTarget is null ? "-" : LastSegment(a.ObservedTarget)
            }).ToList());

        if (result.FlowLogs.Count > 0)
        {
            writer.WriteLine();
            writer.WriteLine($"Flow log configurations ({result.FlowLogs.Count})");
            writer.WriteLine();
            ConsoleTable.Write(writer,
                ["Storage account", "Flow log", "Kind", "Target", "Enabled", "Status", "Latest blob (UTC)", "Age (h)"],
                result.FlowLogs.Select(f => new[]
                {
                    f.StorageAccount,
                    f.Name,
                    f.TargetKind,
                    LastSegment(f.Target),
                    f.Enabled ? "yes" : "no",
                    f.Status,
                    f.LatestBlobUtc ?? "-",
                    f.AgeHours is null ? "-" : f.AgeHours.Value.ToString("0.0")
                }).ToList());

            foreach (var issue in result.FlowLogs.Where(f => f.Message.Length > 0 && f.Status != FlowStatus.Healthy))
            {
                writer.WriteLine($"  {issue.StorageAccount}/{issue.Name}: {issue.Message}");
            }
        }

        return builder.ToString().TrimEnd();
    }

    private static string LastSegment(string armId)
    {
        var parts = armId.Split('/', StringSplitOptions.RemoveEmptyEntries);
        return parts.Length == 0 ? armId : parts[^1];
    }
}
