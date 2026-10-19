using System.CommandLine;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Analysis;
using AzureFlowLogParser.Services.Common;
using AzureFlowLogParser.Services.Discovery;
using AzureFlowLogParser.Services.Reporting;

namespace AzureFlowLogParser.Commands;

/// <summary>
/// "report": aggregates flow observations between storage accounts and writes an interactive HTML or static PDF report.
/// </summary>
public static class ReportCommand
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

        var inputOption = new Option<string[]>("--input")
        {
            Description = "Offline mode: local PT1H.json files or directories instead of Azure. Repeat for several."
        };

        var sinceOption = new Option<string?>("--since")
        {
            Description = "Window start: 90m, 24h, 7d or an ISO-8601 timestamp (UTC). Default: 24h for Azure; the observed range for --input."
        };

        var untilOption = new Option<string?>("--until")
        {
            Description = "Window end: relative like 1h (one hour ago) or an ISO-8601 timestamp. Default: now for Azure; the observed range for --input."
        };

        var actionOption = new Option<string>("--action")
        {
            Description = "all (default), allow or deny.",
            DefaultValueFactory = _ => "all"
        };

        var protocolOption = new Option<string>("--protocol")
        {
            Description = "all (default), tcp or udp.",
            DefaultValueFactory = _ => "all"
        };

        var portOption = new Option<string?>("--port")
        {
            Description = "Destination ports, e.g. 443,8000-8100."
        };

        var ipOption = new Option<string?>("--ip")
        {
            Description = "Keep observations whose source or destination address contains this text."
        };

        var minBytesOption = new Option<long>("--min-bytes")
        {
            Description = "Keep observations with at least this many bytes.",
            DefaultValueFactory = _ => 0
        };

        var staleHoursOption = new Option<int>("--stale-hours")
        {
            Description = "A flow log is Stale when its newest blob is older than this many hours.",
            DefaultValueFactory = _ => 3
        };

        var reportOption = new Option<string>("--report")
        {
            Description = "html (default, interactive) or pdf (static).",
            DefaultValueFactory = _ => "html"
        };

        var outputOption = new Option<string?>("--output", "-o")
        {
            Description = "Report file path. Default: flowlog-report-<yyyyMMddHHmm>.html or .pdf in the current directory."
        };

        var topologyOption = new Option<string?>("--topology")
        {
            Description = "Offline mode only: JSON file mapping address prefixes to storage accounts or VNets (see examples/)."
        };

        var concurrencyOption = new Option<int>("--concurrency")
        {
            Description = "Parallel blob downloads.",
            DefaultValueFactory = _ => 8
        };

        var verboseOption = new Option<bool>("--verbose", "-v")
        {
            Description = "Print progress to stderr."
        };

        var command = new Command("report", "Generate a traffic-flow report between storage accounts (HTML or PDF)");
        command.Add(subscriptionOption);
        command.Add(storageAccountOption);
        command.Add(inputOption);
        command.Add(sinceOption);
        command.Add(untilOption);
        command.Add(actionOption);
        command.Add(protocolOption);
        command.Add(portOption);
        command.Add(ipOption);
        command.Add(minBytesOption);
        command.Add(staleHoursOption);
        command.Add(reportOption);
        command.Add(outputOption);
        command.Add(topologyOption);
        command.Add(concurrencyOption);
        command.Add(verboseOption);

        command.SetAction((ParseResult parseResult, CancellationToken cancellationToken) => CommandRunner.RunAsync(async () =>
        {
            var subscriptions = parseResult.GetValue(subscriptionOption) ?? Array.Empty<string>();
            var storageAccounts = parseResult.GetValue(storageAccountOption) ?? Array.Empty<string>();
            var inputs = parseResult.GetValue(inputOption) ?? Array.Empty<string>();
            var reportFormat = (parseResult.GetValue(reportOption) ?? "html").ToLowerInvariant();
            var verbose = parseResult.GetValue(verboseOption);
            var log = Logger.For(verbose);

            if (reportFormat is not ("html" or "pdf"))
            {
                await Console.Error.WriteLineAsync("Error: --report must be html or pdf.");
                return 1;
            }

            if (inputs.Length > 0 && (subscriptions.Length > 0 || storageAccounts.Length > 0))
            {
                await Console.Error.WriteLineAsync("Error: --input cannot be combined with --subscription or --storage-account.");
                return 1;
            }

            var now = DateTimeOffset.UtcNow;
            // Offline input without explicit bounds uses the observed range; Azure defaults to the last 24 hours.
            DateTimeOffset? since = parseResult.GetValue(sinceOption) is { } sinceText ? TrafficFilter.ParseWindowBound(sinceText, now) : null;
            DateTimeOffset? until = parseResult.GetValue(untilOption) is { } untilText ? TrafficFilter.ParseWindowBound(untilText, now) : null;
            if (inputs.Length == 0)
            {
                since ??= TrafficFilter.ParseWindowBound("24h", now);
                until ??= now;
            }

            if (since is { } windowStart && until is { } windowEnd && windowStart >= windowEnd)
            {
                await Console.Error.WriteLineAsync("Error: --since must be earlier than --until.");
                return 1;
            }

            var filter = TrafficFilter.Create(
                parseResult.GetValue(actionOption),
                parseResult.GetValue(protocolOption),
                parseResult.GetValue(portOption),
                parseResult.GetValue(ipOption),
                parseResult.GetValue(minBytesOption));

            var staleHours = parseResult.GetValue(staleHoursOption);
            var concurrency = Math.Max(1, parseResult.GetValue(concurrencyOption));

            var model = new ReportModel
            {
                GeneratedUtc = IsoTime.Format(now),
                Filters = new ReportFilters
                {
                    Action = filter.Action,
                    Protocol = filter.Protocol,
                    Ports = filter.Ports,
                    Ip = filter.Ip,
                    MinBytes = filter.MinBytes
                }
            };

            if (inputs.Length > 0)
            {
                model.Source = "local";
                var topologyPath = parseResult.GetValue(topologyOption);
                Ipv4Lookup? localLookup = null;
                var localReadAccounts = Array.Empty<string>();
                if (topologyPath is not null)
                {
                    var topology = TopologyFile.Load(topologyPath);
                    localLookup = topology.Lookup;
                    // Assumes the inputs cover every storage network in the topology (same dedupe as Azure mode).
                    localReadAccounts = topology.StorageAccounts.ToArray();
                    model.Notes.Add($"Offline topology from {Path.GetFileName(topologyPath)}: {topology.Networks} network(s), {topology.Prefixes} prefix(es)" +
                        (topology.SkippedPrefixes > 0 ? $", {topology.SkippedPrefixes} non-IPv4 prefix(es) skipped." : "."));
                }
                else
                {
                    model.Notes.Add("Local input: no Azure topology was loaded. Public addresses are 'internet'; private addresses are 'unmapped'.");
                }

                var aggregator = new TrafficAggregator(filter, since, until, localLookup, readAccounts: localReadAccounts);
                var files = TrafficReader.ReadLocal(inputs, aggregator, log);
                log($"Read {files} local file(s).");
                aggregator.FillModel(model, subscriptionCount: 0);
                model.Window = WindowFor(since, until, aggregator.ObservedRange, now);
            }
            else if (parseResult.GetValue(topologyOption) is not null)
            {
                log("Warning: --topology is only used with --input; ignoring it.");
            }
            else
            {
                model.Source = "azure";
                var clients = new AzureClients();
                var load = await new InventoryService(clients)
                    .LoadAsync(new InventoryRequest(subscriptions, storageAccounts, staleHours, IncludeAll: false), log, cancellationToken);

                var notes = new List<string>();
                var lookup = TopologyMapper.Build(load.Topology, notes);
                var readAccounts = load.Result.FlowLogs
                    .Where(f => f.Enabled)
                    .Select(f => f.StorageAccount)
                    .Concat(load.Result.Accounts.Where(a => a.Status == FlowStatus.Unmanaged).Select(a => a.Name))
                    .Distinct(StringComparer.OrdinalIgnoreCase)
                    .ToList();

                var aggregator = new TrafficAggregator(filter, since, until, lookup, readAccounts);
                var read = await TrafficReader.ReadAzureAsync(clients, load.Result, since!.Value, until!.Value, aggregator, concurrency, log, cancellationToken);

                model.Accounts = load.Result.Accounts;
                model.FlowLogs = load.Result.FlowLogs;
                aggregator.FillModel(model, load.Topology.Subscriptions.Count);
                model.Window = WindowFor(since, until, aggregator.ObservedRange, now);

                model.Notes.AddRange(notes);
                if (read.FailedAccounts.Count > 0)
                {
                    model.Notes.Add($"Reads failed for {string.Join(", ", read.FailedAccounts)}: their outbound observations are missing, so inbound allowed duplicates from them were still skipped. Totals are lower than the true traffic.");
                }
                model.Notes.Add("Scope mapping: longest prefix match over NIC, subnet and VNet address ranges. Public addresses outside all known ranges are 'internet'; private ones are 'unmapped'.");
                model.Notes.AddRange(load.Topology.Warnings.Select(w => $"Discovery warning: {w}"));
            }

            AddSummaryNotes(model, filter);

            var outputPath = parseResult.GetValue(outputOption)
                ?? $"flowlog-report-{now:yyyyMMddHHmm}.{reportFormat}";

            if (reportFormat == "pdf")
            {
                PdfReportWriter.Write(model, outputPath);
            }
            else
            {
                HtmlReportWriter.Write(model, outputPath);
            }

            Console.Error.WriteLine($"Report written: {Path.GetFullPath(outputPath)} ({model.Summary.Flows:N0} observations, {model.Summary.RowsTotal:N0} rows)");
            return 0;
        }, parseResult.GetValue(verboseOption)));

        return command;
    }

    /// <summary>Explicit bounds win; open bounds take the observed range, or the generation time when nothing was kept.</summary>
    private static ReportWindow WindowFor(DateTimeOffset? since, DateTimeOffset? until, (DateTimeOffset? First, DateTimeOffset? Last) observed, DateTimeOffset now)
    {
        var from = since ?? observed.First ?? now;
        var to = until ?? observed.Last ?? now;
        return new ReportWindow { FromUtc = IsoTime.Format(from), ToUtc = IsoTime.Format(to) };
    }

    private static void AddSummaryNotes(ReportModel model, TrafficFilter filter)
    {
        model.Notes.Insert(0, "Counts are flow log observations: one per NSG or VNet flow tuple. A flow crossing two logged networks is observed at both; the duplicate allowed inbound observation is skipped.");

        if (model.Summary.RowsTruncated)
        {
            model.Notes.Add($"Flow rows capped at {TrafficAggregator.MaxRows:N0} of {model.Summary.RowsTotal:N0}. Totals still include every observation.");
        }

        if (filter.Action != "all" || filter.Protocol != "all" || filter.Ports.Length > 0 || filter.Ip.Length > 0 || filter.MinBytes > 0)
        {
            model.Notes.Add("Filters apply per observation: port matches destination port; IP matches source or destination text.");
        }
    }
}
