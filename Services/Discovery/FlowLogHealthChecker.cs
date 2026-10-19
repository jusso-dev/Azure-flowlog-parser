using Azure;
using Azure.Storage.Blobs;
using Azure.Storage.Blobs.Models;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Common;

namespace AzureFlowLogParser.Services.Discovery;

/// <summary>
/// Checks flow log configurations and storage accounts against the blobs they write.
/// Latest-blob lookup walks the per-day prefixes of each target (today, then yesterday).
/// </summary>
public sealed class FlowLogHealthChecker
{
    private readonly AzureClients _clients;
    private readonly TimeSpan _staleAfter;
    private readonly DateTimeOffset _now;

    public FlowLogHealthChecker(AzureClients clients, int staleAfterHours, DateTimeOffset now)
    {
        _clients = clients;
        _staleAfter = TimeSpan.FromHours(staleAfterHours);
        _now = now;
    }

    /// <summary>Updates the account and its configurations in place.</summary>
    public async Task CheckAccountAsync(StorageAccountInfo account, List<FlowLogConfigInfo> configs, CancellationToken cancellationToken)
    {
        var container = _clients.Blob(account.Name).GetBlobContainerClient(FlowLogPaths.ContainerName);
        var containerDenied = false;
        var hasUnmanagedBlobs = false;

        try
        {
            account.ContainerPresent = (await container.ExistsAsync(cancellationToken)).Value;

            // Only accounts without configurations need the all-resources probe; it finds unmanaged flow logs.
            if (account.ContainerPresent && configs.Count == 0)
            {
                var first = await FirstBlobAsync(container, FlowLogPaths.AllResourcesPrefix, cancellationToken);
                hasUnmanagedBlobs = first is not null;
                account.ObservedTarget = first is null ? null : FlowLogPaths.TargetFromBlobName(first.Name);
            }
        }
        catch (RequestFailedException ex) when (IsAccessDenied(ex))
        {
            containerDenied = true;
        }

        DateTimeOffset? latest = null;
        foreach (var config in configs)
        {
            var configLatest = await CheckConfigAsync(container, config, cancellationToken);
            if (configLatest is not null && (latest is null || configLatest > latest))
            {
                latest = configLatest;
            }
        }

        account.FlowLogConfigs = configs.Count;
        account.HasFlowLogs = configs.Count > 0 || hasUnmanagedBlobs;
        account.LatestBlobUtc = IsoTime.FormatOrNull(latest);

        if (containerDenied)
        {
            account.Status = FlowStatus.AccessDenied;
        }
        else if (configs.Count > 0)
        {
            account.Status = FlowStatus.Worst(configs.Select(c => c.Status));
        }
        else
        {
            account.Status = hasUnmanagedBlobs ? FlowStatus.Unmanaged : FlowStatus.NoFlowLogs;
        }
    }

    private async Task<DateTimeOffset?> CheckConfigAsync(BlobContainerClient container, FlowLogConfigInfo config, CancellationToken cancellationToken)
    {
        if (!config.Enabled)
        {
            config.Status = FlowStatus.Disabled;
            config.Message = "Flow log is disabled; no blobs are read.";
            return null;
        }

        try
        {
            DateTimeOffset? latest = null;
            var today = _now.UtcDateTime.Date;

            foreach (var day in new[] { today, today.AddDays(-1) })
            {
                latest = await NewestBlobTimeAsync(container, FlowLogPaths.DayPrefix(config.Target, day), cancellationToken);
                if (latest is not null)
                {
                    break;
                }
            }

            config.LatestBlobUtc = IsoTime.FormatOrNull(latest);

            if (latest is null)
            {
                config.Status = FlowStatus.NoData;
                config.Message = "No flow log blobs found in the last 2 days.";
                return null;
            }

            var age = _now - latest.Value;
            config.AgeHours = Math.Round(Math.Max(age.TotalHours, 0), 2);
            config.Status = age > _staleAfter ? FlowStatus.Stale : FlowStatus.Healthy;
            config.Message = config.Status == FlowStatus.Stale
                ? $"Newest blob is {config.AgeHours:0.0} hours old."
                : string.Empty;
            return latest;
        }
        catch (RequestFailedException ex) when (IsAccessDenied(ex))
        {
            config.Status = FlowStatus.AccessDenied;
            config.Message = ex.ErrorCode ?? ex.Message;
            return null;
        }
        catch (Exception ex)
        {
            config.Status = FlowStatus.Error;
            config.Message = ex.Message;
            return null;
        }
    }

    private static async Task<DateTimeOffset?> NewestBlobTimeAsync(BlobContainerClient container, string prefix, CancellationToken cancellationToken)
    {
        DateTimeOffset? newest = null;

        await foreach (var blob in container.GetBlobsAsync(BlobTraits.None, BlobStates.None, prefix, cancellationToken))
        {
            var modified = blob.Properties.LastModified;
            if (modified is not null && (newest is null || modified > newest))
            {
                newest = modified;
            }
        }

        return newest;
    }

    private static async Task<BlobItem?> FirstBlobAsync(BlobContainerClient container, string prefix, CancellationToken cancellationToken)
    {
        await foreach (var blob in container.GetBlobsAsync(BlobTraits.None, BlobStates.None, prefix, cancellationToken))
        {
            return blob;
        }

        return null;
    }

    public static bool IsAccessDenied(RequestFailedException ex)
    {
        return ex.Status == 403;
    }
}
