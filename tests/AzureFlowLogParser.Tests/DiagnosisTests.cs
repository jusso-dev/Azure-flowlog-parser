using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Discovery;
using Xunit;

namespace AzureFlowLogParser.Tests;

public class DiagnosisTests
{
    private const string Sub = "11111111-1111-1111-1111-111111111111";
    private const string VnetId = $"/subscriptions/{Sub}/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/vnet1";
    private const string SubnetId = $"{VnetId}/subnets/app";
    private const string NicId = $"/subscriptions/{Sub}/resourceGroups/rg/providers/Microsoft.Network/networkInterfaces/nic1";
    private const string NsgId = $"/subscriptions/{Sub}/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/nsg1";

    [Fact]
    public void Sources_without_a_flow_log_are_not_configured()
    {
        var result = Diagnose(Network(), Array.Empty<FlowLogConfigInfo>());

        Assert.Equal(4, result.SourceCount);
        Assert.Equal(new[] { "VNet", "Subnet", "NIC", "NSG" }, result.Findings.Select(f => f.Kind).ToArray());
        Assert.All(result.Findings.Where(f => f.Kind != "NSG"), f => Assert.Equal(DiagnosisReason.NotConfigured, f.Reason));
        Assert.Equal(DiagnosisReason.NotConfigured, result.Findings.Single(f => f.Kind == "NSG").Reason);
        Assert.Contains("30 Sep 2027", result.Findings.Single(f => f.Kind == "NSG").Message, StringComparison.Ordinal);
    }

    [Fact]
    public void Missing_network_watcher_is_its_own_reason()
    {
        var topology = Network();
        topology.NetworkWatchers.Clear();

        var result = Diagnose(topology, Array.Empty<FlowLogConfigInfo>());

        Assert.All(result.Findings, finding => Assert.Equal(DiagnosisReason.NetworkWatcherMissing, finding.Reason));
    }

    [Fact]
    public void Delivering_vnet_flow_log_covers_children_and_the_attached_nsg()
    {
        var log = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.Healthy, "2026-10-24T00:00:00Z");

        var result = Diagnose(Network(), new[] { log });

        Assert.Equal(4, result.SourceCount);
        Assert.Empty(result.Findings);
    }

    [Fact]
    public void Disabled_parent_is_reported_for_every_child()
    {
        var log = Config("fl-vnet", VnetId, "VNet", enabled: false, FlowStatus.Disabled, null);

        var result = Diagnose(Network(), new[] { log });

        Assert.Equal(4, result.Findings.Count);
        Assert.All(result.Findings.Where(f => f.Kind != "NSG"), f => Assert.Equal(DiagnosisReason.Disabled, f.Reason));
        Assert.Contains("parent virtual network vnet1", result.Findings.Single(f => f.Kind == "NIC").Message, StringComparison.Ordinal);
    }

    [Fact]
    public void Enabled_nic_flow_log_wins_over_a_healthy_vnet_flow_log()
    {
        var vnet = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.Healthy, "2026-10-24T00:00:00Z");
        var nic = Config("fl-nic", NicId, "NIC", enabled: true, FlowStatus.NoData, null);
        nic.ProvisioningState = "Succeeded";

        var result = Diagnose(Network(), new[] { vnet, nic });

        var finding = Assert.Single(result.Findings, f => f.Kind == "NIC");
        Assert.Equal(DiagnosisReason.NoData, finding.Reason);
        Assert.Equal("fl-nic", finding.FlowLog);
        // The NIC is not covered, so the NSG attached to it is still reported.
        Assert.Equal(DiagnosisReason.NotConfigured, Assert.Single(result.Findings, f => f.Kind == "NSG").Reason);
    }

    [Fact]
    public void Firewall_blocks_before_an_empty_container()
    {
        var topology = Network();
        topology.StorageAccounts[0].NetworkDefaultAction = "Deny";
        topology.StorageAccounts[0].AllowsTrustedServices = false;
        topology.StorageAccounts[0].PublicNetworkAccess = "Enabled";
        var log = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.NoData, null);
        log.ProvisioningState = "Succeeded";

        var result = Diagnose(topology, new[] { log });

        Assert.All(result.Findings.Where(f => f.Kind != "NSG"), f => Assert.Equal(DiagnosisReason.StorageFirewall, f.Reason));
    }

    [Fact]
    public void Trusted_services_exception_is_not_a_firewall_block()
    {
        var topology = Network();
        topology.StorageAccounts[0].NetworkDefaultAction = "Deny";
        topology.StorageAccounts[0].AllowsTrustedServices = true;
        var log = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.NoData, null);
        log.ProvisioningState = "Succeeded";

        var finding = Assert.Single(Diagnose(topology, new[] { log }).Findings, f => f.Kind == "VNet");
        Assert.Equal(DiagnosisReason.NoData, finding.Reason);
    }

    [Fact]
    public void Premium_sku_and_region_mismatch_are_named()
    {
        var premium = Network();
        premium.StorageAccounts[0].SkuName = "Premium_LRS";
        var premiumLog = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.NoData, null);
        Assert.Equal(DiagnosisReason.UnsupportedStorage, Diagnose(premium, new[] { premiumLog }).Findings.Single(f => f.Kind == "VNet").Reason);

        var otherRegion = Network();
        otherRegion.StorageAccounts[0].Location = "eastus";
        Assert.Equal(DiagnosisReason.RegionMismatch, Diagnose(otherRegion, new[] { premiumLog }).Findings.Single(f => f.Kind == "VNet").Reason);

        var spaced = Network();
        spaced.StorageAccounts[0].Location = "Australia East";
        Assert.Equal(DiagnosisReason.NoData, Diagnose(spaced, new[] { premiumLog }).Findings.Single(f => f.Kind == "VNet").Reason);
    }

    [Fact]
    public void Shared_key_off_without_an_identity_is_reported()
    {
        var topology = Network();
        topology.StorageAccounts[0].AllowSharedKeyAccess = false;
        var log = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.NoData, null);
        log.ProvisioningState = "Succeeded";

        Assert.Equal(DiagnosisReason.SharedKeyDisabled, Diagnose(topology, new[] { log }).Findings.Single(f => f.Kind == "VNet").Reason);

        log.HasManagedIdentity = true;
        Assert.Equal(DiagnosisReason.NoData, Diagnose(topology, new[] { log }).Findings.Single(f => f.Kind == "VNet").Reason);
    }

    [Fact]
    public void Deleted_target_and_failed_provisioning_are_reported()
    {
        var gone = Config("fl-old", VnetId + "-deleted", "VNet", enabled: true, FlowStatus.NoData, null);
        gone.ProvisioningState = "Succeeded";
        gone.TargetName = "vnet-deleted";
        var failed = Config("fl-bad", VnetId, "VNet", enabled: true, FlowStatus.NoData, null);
        failed.ProvisioningState = "Failed";

        var result = Diagnose(Network(), new[] { gone, failed });

        Assert.Equal(DiagnosisReason.ProvisioningFailed, result.Findings.Single(f => f.Name == "vnet1").Reason);
        Assert.Equal(DiagnosisReason.TargetMissing, result.Findings.Single(f => f.Name == "vnet-deleted").Reason);
    }

    [Fact]
    public void Stale_blobs_name_the_age_and_the_limit()
    {
        var log = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.Stale, "2026-10-20T00:00:00Z");
        log.AgeHours = 30;
        log.ProvisioningState = "Succeeded";

        var finding = Diagnose(Network(), new[] { log }).Findings.Single(f => f.Kind == "VNet");
        Assert.Equal(DiagnosisReason.Stale, finding.Reason);
        Assert.Contains("30.0 hours", finding.Message, StringComparison.Ordinal);
        Assert.Contains("limit is 6 hours", finding.Message, StringComparison.Ordinal);
    }

    [Fact]
    public void Storage_filter_keeps_unconfigured_sources_and_matching_accounts()
    {
        var other = Config("fl-vnet", VnetId, "VNet", enabled: true, FlowStatus.NoData, null);
        other.StorageAccount = "stother";

        var result = Diagnose(Network(), new[] { other }, new[] { "stlogs" });

        Assert.Contains(result.Findings, finding => finding.Kind == "NSG" && finding.StorageAccount.Length == 0);
        Assert.DoesNotContain(result.Findings, finding => finding.StorageAccount == "stother");
    }

    private static DiagnosisResult Diagnose(AzureTopology topology, IReadOnlyList<FlowLogConfigInfo> logs, IReadOnlyCollection<string>? filter = null)
    {
        return FlowLogDiagnostician.Diagnose(topology, logs, 6, new DateTimeOffset(2026, 10, 24, 0, 0, 0, TimeSpan.Zero), filter);
    }

    private static AzureTopology Network()
    {
        var topology = new AzureTopology();
        topology.NetworkWatchers.Add(new NetworkWatcherRef(Sub, "NetworkWatcher_australiaeast", "australiaeast"));
        topology.StorageAccounts.Add(new StorageAccountInfo
        {
            SubscriptionId = Sub,
            Name = "stlogs",
            ResourceGroup = "rg",
            Location = "australiaeast",
            Kind = "StorageV2",
            SkuName = "Standard_LRS",
            PublicNetworkAccess = "Enabled",
            NetworkDefaultAction = "Allow",
            AllowsTrustedServices = true,
            AllowSharedKeyAccess = true
        });
        topology.Vnets.Add(new VnetInfo(
            VnetId,
            "vnet1",
            "rg",
            Sub,
            new[] { "10.0.0.0/16" },
            new[] { new SubnetInfo(new[] { "10.0.1.0/24" }, NsgId, SubnetId, "app") },
            "australiaeast"));
        topology.Nics.Add(new NicInfo(NsgId, new[] { "10.0.1.4" }, NicId, "nic1", "australiaeast", SubnetId));
        return topology;
    }

    private static FlowLogConfigInfo Config(string name, string target, string kind, bool enabled, string status, string? latest)
    {
        return new FlowLogConfigInfo
        {
            SubscriptionId = Sub,
            Name = name,
            StorageAccount = "stlogs",
            NetworkWatcher = "NetworkWatcher_australiaeast",
            Location = "australiaeast",
            Target = target,
            TargetKind = kind,
            TargetName = target.Split('/', StringSplitOptions.RemoveEmptyEntries)[^1],
            TargetResourceGroup = "rg",
            Enabled = enabled,
            Status = status,
            LatestBlobUtc = latest,
            ProvisioningState = "Succeeded"
        };
    }
}
