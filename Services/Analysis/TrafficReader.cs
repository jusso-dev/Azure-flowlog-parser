using System.Collections.Concurrent;
using Azure.Storage.Blobs.Models;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Discovery;

namespace AzureFlowLogParser.Services.Analysis;

/// <summary>Reads flow log blobs (Azure) or local JSON files into a <see cref="TrafficAggregator"/>.</summary>
public static class TrafficReader
{
    /// <summary>
    /// Reads every enabled configuration's day prefixes in the window, plus whole containers for unmanaged accounts.
    /// Returns the blob count and the accounts that had listing or download failures.
    /// </summary>
    public static async Task<ReadSummary> ReadAzureAsync(
        AzureClients clients,
        InventoryResult inventory,
        DateTimeOffset from,
        DateTimeOffset to,
        TrafficAggregator aggregator,
        int concurrency,
        Action<string> log,
        CancellationToken cancellationToken)
    {
        var work = new List<(string Account, string Name)>();
        var failedAccounts = new ConcurrentBag<string>();
        var unmanaged = inventory.Accounts.Where(a => a.Status == FlowStatus.Unmanaged).ToList();

        foreach (var config in inventory.FlowLogs.Where(f => f.Enabled && f.Status != FlowStatus.Disabled))
        {
            var container = clients.Blob(config.StorageAccount).GetBlobContainerClient(FlowLogPaths.ContainerName);
            for (var day = from.UtcDateTime.Date; day <= to.UtcDateTime.Date; day = day.AddDays(1))
            {
                try
                {
                    await foreach (var blob in container.GetBlobsAsync(BlobTraits.None, BlobStates.None, FlowLogPaths.DayPrefix(config.Target, day), cancellationToken))
                    {
                        work.Add((config.StorageAccount, blob.Name));
                    }
                }
                catch (Exception ex) when (ex is not OperationCanceledException)
                {
                    failedAccounts.Add(config.StorageAccount);
                    log($"Warning: could not list {config.StorageAccount} for {config.Name}: {ex.Message}");
                }
            }
        }

        foreach (var account in unmanaged)
        {
            var container = clients.Blob(account.Name).GetBlobContainerClient(FlowLogPaths.ContainerName);
            var lowerBound = from.AddHours(-1);
            try
            {
                await foreach (var blob in container.GetBlobsAsync(BlobTraits.None, BlobStates.None, FlowLogPaths.AllResourcesPrefix, cancellationToken))
                {
                    if (blob.Properties.LastModified >= lowerBound)
                    {
                        work.Add((account.Name, blob.Name));
                    }
                }
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                failedAccounts.Add(account.Name);
                log($"Warning: could not list unmanaged flow logs in {account.Name}: {ex.Message}");
            }
        }

        var failures = 0;
        var processed = 0;
        await Parallel.ForEachAsync(work, new ParallelOptions { MaxDegreeOfParallelism = concurrency, CancellationToken = cancellationToken }, async (item, token) =>
        {
            try
            {
                var container = clients.Blob(item.Account).GetBlobContainerClient(FlowLogPaths.ContainerName);
                var download = await container.GetBlobClient(item.Name).DownloadContentAsync(token);
                var records = FlowLogParser.ParseFlowLog(download.Value.Content.ToString());
                aggregator.Add(records);
                Interlocked.Increment(ref processed);
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                Interlocked.Increment(ref failures);
                failedAccounts.Add(item.Account);
                log($"Warning: skipped {item.Account}/{item.Name}: {ex.Message}");
            }
        });

        log($"Read {processed} blob(s) from {work.Select(w => w.Account).Distinct(StringComparer.OrdinalIgnoreCase).Count()} storage account(s); {failures} failed.");
        return new ReadSummary(processed, failedAccounts.Distinct(StringComparer.OrdinalIgnoreCase).OrderBy(a => a, StringComparer.OrdinalIgnoreCase).ToList());
    }

    public sealed record ReadSummary(int Blobs, IReadOnlyList<string> FailedAccounts);

    /// <summary>Reads local PT1H.json files (or every .json below a directory).</summary>
    public static int ReadLocal(IEnumerable<string> inputs, TrafficAggregator aggregator, Action<string> log)
    {
        var files = new List<string>();
        foreach (var input in inputs)
        {
            if (Directory.Exists(input))
            {
                files.AddRange(Directory.EnumerateFiles(input, "*.json", SearchOption.AllDirectories));
            }
            else if (File.Exists(input))
            {
                files.Add(input);
            }
            else
            {
                throw new FileNotFoundException($"Input not found: {input}");
            }
        }

        var processed = 0;
        foreach (var file in files.Order(StringComparer.Ordinal))
        {
            try
            {
                aggregator.Add(FlowLogParser.ParseFlowLog(File.ReadAllText(file)));
                processed++;
            }
            catch (Exception ex) when (ex is System.Text.Json.JsonException or IOException)
            {
                log($"Warning: skipped {file}: {ex.Message}");
            }
        }

        return processed;
    }
}
