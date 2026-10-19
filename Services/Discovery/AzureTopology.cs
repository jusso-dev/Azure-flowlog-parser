using Azure;
using Azure.Core;
using Azure.ResourceManager;
using Azure.ResourceManager.Network;
using Azure.ResourceManager.Resources;
using Azure.ResourceManager.Resources.Models;
using Azure.ResourceManager.Storage;
using AzureFlowLogParser.Models;

namespace AzureFlowLogParser.Services.Discovery;

/// <summary>Raw ARM data for the selected subscriptions. No blob health checks yet.</summary>
public sealed class AzureTopology
{
    public List<SubscriptionRef> Subscriptions { get; } = new();
    public List<StorageAccountInfo> StorageAccounts { get; } = new();
    public List<FlowLogConfigInfo> FlowLogs { get; } = new();
    public List<VnetInfo> Vnets { get; } = new();
    public List<NicInfo> Nics { get; } = new();
    public List<string> Warnings { get; } = new();
}

/// <summary>A scanned subscription. DisplayName and TenantId are empty when ARM does not return them.</summary>
public sealed record SubscriptionRef(string Id, string DisplayName, string TenantId);

public sealed record VnetInfo(
    string Id,
    string Name,
    string ResourceGroup,
    string SubscriptionId,
    IReadOnlyList<string> AddressPrefixes,
    IReadOnlyList<SubnetInfo> Subnets);

public sealed record SubnetInfo(IReadOnlyList<string> AddressPrefixes, string? NsgId);

public sealed record NicInfo(string? NsgId, IReadOnlyList<string> PrivateIps);

/// <summary>
/// Loads subscriptions, storage accounts, flow log configurations, VNets and NICs through the ARM SDK.
/// </summary>
public sealed class ArmTopologyLoader
{
    private readonly ArmClient _armClient;

    public ArmTopologyLoader(ArmClient armClient)
    {
        _armClient = armClient;
    }

    public async Task<AzureTopology> LoadAsync(IReadOnlyCollection<string> subscriptionIds, CancellationToken cancellationToken = default)
    {
        var topology = new AzureTopology();

        foreach (var subscription in await ResolveSubscriptionsAsync(subscriptionIds, cancellationToken))
        {
            topology.Subscriptions.Add(subscription);

            try
            {
                await LoadSubscriptionAsync(subscription, topology, cancellationToken);
            }
            catch (RequestFailedException ex)
            {
                var label = string.IsNullOrEmpty(subscription.DisplayName) ? subscription.Id : $"{subscription.Id} ({subscription.DisplayName})";
                topology.Warnings.Add($"Subscription {label}: {ex.Status} {ex.ErrorCode ?? ex.Message}");
            }
        }

        return topology;
    }

    private async Task<List<SubscriptionRef>> ResolveSubscriptionsAsync(IReadOnlyCollection<string> subscriptionIds, CancellationToken cancellationToken)
    {
        var result = new List<SubscriptionRef>();

        if (subscriptionIds.Count > 0)
        {
            foreach (var id in subscriptionIds.Distinct(StringComparer.OrdinalIgnoreCase))
            {
                var resource = _armClient.GetSubscriptionResource(SubscriptionResource.CreateResourceIdentifier(id));
                var response = await resource.GetAsync(cancellationToken);
                var data = response.Value.Data;
                result.Add(new SubscriptionRef(id, data.DisplayName ?? string.Empty, data.TenantId?.ToString() ?? string.Empty));
            }

            return result;
        }

        await foreach (var subscription in _armClient.GetSubscriptions().GetAllAsync(cancellationToken))
        {
            if (subscription.Data.State == SubscriptionState.Enabled)
            {
                var data = subscription.Data;
                result.Add(new SubscriptionRef(data.SubscriptionId, data.DisplayName ?? string.Empty, data.TenantId?.ToString() ?? string.Empty));
            }
        }

        return result;
    }

    private async Task LoadSubscriptionAsync(SubscriptionRef subscription, AzureTopology topology, CancellationToken cancellationToken)
    {
        var subResource = _armClient.GetSubscriptionResource(SubscriptionResource.CreateResourceIdentifier(subscription.Id));

        await foreach (var account in subResource.GetStorageAccountsAsync(cancellationToken: cancellationToken))
        {
            topology.StorageAccounts.Add(new StorageAccountInfo
            {
                SubscriptionId = subscription.Id,
                Name = account.Data.Name,
                ResourceGroup = account.Id.ResourceGroupName ?? string.Empty,
                Location = account.Data.Location.ToString(),
                Kind = account.Data.Kind?.ToString() ?? string.Empty,
                SkuName = account.Data.Sku?.Name.ToString() ?? string.Empty
            });
        }

        await foreach (var watcher in subResource.GetNetworkWatchersAsync(cancellationToken: cancellationToken))
        {
            await foreach (var flowLog in watcher.GetFlowLogs().GetAllAsync(cancellationToken))
            {
                var data = flowLog.Data;
                if (string.IsNullOrEmpty(data.StorageId) || string.IsNullOrEmpty(data.TargetResourceId))
                {
                    continue;
                }

                string target = data.TargetResourceId!;

                topology.FlowLogs.Add(new FlowLogConfigInfo
                {
                    SubscriptionId = subscription.Id,
                    Name = data.Name,
                    StorageAccount = new ResourceIdentifier(data.StorageId!).Name,
                    NetworkWatcher = watcher.Id.Name,
                    Location = data.Location.ToString() ?? string.Empty,
                    Target = target,
                    TargetName = FlowLogPaths.TargetName(target),
                    TargetResourceGroup = FlowLogPaths.TargetResourceGroup(target),
                    TargetKind = FlowLogPaths.TargetKind(target),
                    Enabled = data.Enabled ?? false,
                    RetentionDays = data.RetentionPolicy?.Days ?? 0,
                    TrafficAnalytics = data.NetworkWatcherFlowAnalyticsConfiguration?.Enabled ?? false
                });
            }
        }

        await foreach (var vnet in subResource.GetVirtualNetworksAsync(cancellationToken: cancellationToken))
        {
            var subnets = vnet.Data.Subnets
                .Select(s => new SubnetInfo(
                    (s.AddressPrefixes.Count > 0 ? s.AddressPrefixes : new[] { s.AddressPrefix })
                        .Where(p => !string.IsNullOrEmpty(p))
                        .Select(p => p!)
                        .ToList(),
                    s.NetworkSecurityGroup?.Id?.ToString()))
                .ToList();

            topology.Vnets.Add(new VnetInfo(
                vnet.Id.ToString(),
                vnet.Data.Name,
                vnet.Id.ResourceGroupName ?? string.Empty,
                subscription.Id,
                vnet.Data.AddressPrefixes.ToList(),
                subnets));
        }

        await foreach (var nic in subResource.GetNetworkInterfacesAsync(cancellationToken: cancellationToken))
        {
            var privateIps = nic.Data.IPConfigurations
                .Select(c => c.PrivateIPAddress)
                .Where(ip => !string.IsNullOrEmpty(ip))
                .Select(ip => ip!)
                .ToList();

            topology.Nics.Add(new NicInfo(nic.Data.NetworkSecurityGroup?.Id?.ToString(), privateIps));
        }
    }
}
