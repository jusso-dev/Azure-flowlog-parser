using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Common;

namespace AzureFlowLogParser.Services.Discovery;

public sealed record InventoryRequest(
    IReadOnlyCollection<string> Subscriptions,
    IReadOnlyCollection<string> StorageAccounts,
    int StaleAfterHours,
    bool IncludeAll);

/// <summary>Inventory result plus the raw topology used for IP-to-scope mapping.</summary>
public sealed class InventoryLoad
{
    public required AzureTopology Topology { get; init; }
    public required InventoryResult Result { get; init; }
}

/// <summary>
/// Discovers storage accounts and flow log configurations per subscription and classifies their health.
/// </summary>
public sealed class InventoryService
{
    private readonly AzureClients _clients;

    public InventoryService(AzureClients clients)
    {
        _clients = clients;
    }

    public async Task<InventoryLoad> LoadAsync(InventoryRequest request, Action<string>? log, CancellationToken cancellationToken)
    {
        var topology = await new ArmTopologyLoader(_clients.Arm).LoadAsync(request.Subscriptions, cancellationToken);
        foreach (var warning in topology.Warnings)
        {
            log?.Invoke($"Warning: {warning}");
        }

        var selectedNames = request.StorageAccounts.Count > 0
            ? request.StorageAccounts.ToHashSet(StringComparer.OrdinalIgnoreCase)
            : null;

        var accounts = topology.StorageAccounts
            .Where(a => selectedNames is null || selectedNames.Contains(a.Name))
            .OrderBy(a => a.SubscriptionId, StringComparer.OrdinalIgnoreCase)
            .ThenBy(a => a.Name, StringComparer.OrdinalIgnoreCase)
            .ToList();

        var accountNames = accounts.Select(a => a.Name).ToHashSet(StringComparer.OrdinalIgnoreCase);
        var flowLogs = topology.FlowLogs
            .Where(f => accountNames.Contains(f.StorageAccount))
            .OrderBy(f => f.StorageAccount, StringComparer.OrdinalIgnoreCase)
            .ThenBy(f => f.Name, StringComparer.OrdinalIgnoreCase)
            .ToList();

        var now = DateTimeOffset.UtcNow;
        var checker = new FlowLogHealthChecker(_clients, request.StaleAfterHours, now);

        await Parallel.ForEachAsync(accounts, new ParallelOptions { MaxDegreeOfParallelism = 8, CancellationToken = cancellationToken }, async (account, token) =>
        {
            var configs = flowLogs.Where(f => string.Equals(f.StorageAccount, account.Name, StringComparison.OrdinalIgnoreCase)).ToList();
            try
            {
                await checker.CheckAccountAsync(account, configs, token);
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                account.Status = FlowStatus.Error;
                log?.Invoke($"Warning: {account.Name}: {ex.Message}");
            }
        });

        var included = accounts
            // AccessDenied accounts are kept even without a proven flow log: their status says the check could not run.
            .Where(a => request.IncludeAll || a.HasFlowLogs || a.Status == FlowStatus.AccessDenied)
            .ToList();

        return new InventoryLoad
        {
            Topology = topology,
            Result = new InventoryResult
            {
                GeneratedUtc = IsoTime.Format(now),
                StaleAfterHours = request.StaleAfterHours,
                Accounts = included,
                FlowLogs = flowLogs
            }
        };
    }
}
