using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Discovery;

namespace AzureFlowLogParser.Services.Analysis;

/// <summary>
/// Builds the IPv4 scope table from ARM topology. Most specific entry wins:
/// NIC /32 (NSG logged) → subnet (NSG logged) → VNet address space (VNet logged, or an unlogged "vnet" scope).
/// </summary>
public static class TopologyMapper
{
    public static Ipv4Lookup Build(AzureTopology topology, List<string> notes)
    {
        var lookup = new Ipv4Lookup();
        var storageScopes = new Dictionary<string, ReportScope>(StringComparer.OrdinalIgnoreCase);

        // Disabled configurations still describe which account a network reports to, so they own address space too.
        var nsgOwners = OwnersByTarget(topology, "NSG", notes);
        var vnetOwners = OwnersByTarget(topology, "VNet", notes);

        ReportScope StorageScope(string account)
        {
            if (!storageScopes.TryGetValue(account, out var scope))
            {
                var info = topology.StorageAccounts.FirstOrDefault(a => string.Equals(a.Name, account, StringComparison.OrdinalIgnoreCase));
                scope = new ReportScope
                {
                    Id = account,
                    Kind = ScopeKind.Storage,
                    Label = account,
                    SubscriptionId = info?.SubscriptionId ?? string.Empty,
                    ResourceGroup = info?.ResourceGroup ?? string.Empty,
                    StorageAccount = account
                };
                storageScopes[account] = scope;
            }

            return scope;
        }

        foreach (var nic in topology.Nics)
        {
            if (nic.NsgId is null || !nsgOwners.TryGetValue(nic.NsgId, out var account))
            {
                continue;
            }

            foreach (var ip in nic.PrivateIps)
            {
                lookup.TryAdd($"{ip}/32", StorageScope(account));
            }
        }

        foreach (var vnet in topology.Vnets)
        {
            foreach (var subnet in vnet.Subnets)
            {
                if (subnet.NsgId is null || !nsgOwners.TryGetValue(subnet.NsgId, out var account))
                {
                    continue;
                }

                foreach (var prefix in subnet.AddressPrefixes)
                {
                    lookup.TryAdd(prefix, StorageScope(account));
                }
            }

            var vnetScope = vnetOwners.TryGetValue(vnet.Id, out var vnetAccount)
                ? StorageScope(vnetAccount)
                : new ReportScope
                {
                    Id = $"vnet:{vnet.ResourceGroup}/{vnet.Name}",
                    Kind = ScopeKind.Vnet,
                    Label = $"{vnet.Name} (no flow logs)",
                    SubscriptionId = vnet.SubscriptionId,
                    ResourceGroup = vnet.ResourceGroup
                };

            foreach (var prefix in vnet.AddressPrefixes)
            {
                lookup.TryAdd(prefix, vnetScope);
            }
        }

        return lookup;
    }

    /// <summary>Maps ARM target id to the storage account name for configurations of one target kind.</summary>
    private static Dictionary<string, string> OwnersByTarget(AzureTopology topology, string targetKind, List<string> notes)
    {
        var owners = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);

        foreach (var group in topology.FlowLogs
                     .Where(f => f.TargetKind == targetKind)
                     .GroupBy(f => f.Target, StringComparer.OrdinalIgnoreCase))
        {
            var accounts = group.Select(f => f.StorageAccount).Distinct(StringComparer.OrdinalIgnoreCase).OrderBy(a => a, StringComparer.OrdinalIgnoreCase).ToList();
            owners[group.Key] = accounts[0];

            if (accounts.Count > 1)
            {
                notes.Add($"{targetKind} {group.Key} has flow logs to {accounts.Count} storage accounts; attributed to {accounts[0]}.");
            }
        }

        return owners;
    }
}
