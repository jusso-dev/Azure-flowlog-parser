using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Common;

namespace AzureFlowLogParser.Services.Discovery;

/// <summary>
/// Decides which virtual networks, subnets, NICs, and attached NSGs are not writing flow logs
/// to storage, and why. An enabled flow log on a NIC wins over its subnet, which wins over its
/// virtual network (the Azure enablement order). A delivering parent covers its children.
/// A delivering flow log is enabled, Healthy, and has a blob timestamp.
/// </summary>
public static class FlowLogDiagnostician
{
    private const string NsgContainer = "insights-logs-networksecuritygroupflowevent";

    public static DiagnosisResult Diagnose(
        AzureTopology topology,
        IReadOnlyList<FlowLogConfigInfo> flowLogs,
        int staleAfterHours,
        DateTimeOffset now,
        IReadOnlyCollection<string>? storageAccounts)
    {
        var logs = flowLogs.Where(log => !string.IsNullOrWhiteSpace(log.Target)).ToList();
        var byTarget = new Dictionary<string, List<FlowLogConfigInfo>>(StringComparer.OrdinalIgnoreCase);
        foreach (var log in logs)
        {
            if (!byTarget.TryGetValue(log.Target, out var list))
            {
                list = new List<FlowLogConfigInfo>();
                byTarget[log.Target] = list;
            }

            list.Add(log);
        }

        var accounts = topology.StorageAccounts.ToDictionary(account => account.Name, StringComparer.OrdinalIgnoreCase);
        var knownIds = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var sources = new List<Source>();

        foreach (var vnet in topology.Vnets)
        {
            if (string.IsNullOrWhiteSpace(vnet.Id))
            {
                continue;
            }

            knownIds.Add(vnet.Id);
            sources.Add(new Source("VNet", vnet.Id, vnet.Name, vnet.ResourceGroup, vnet.SubscriptionId, vnet.Location, null, null));
            foreach (var subnet in vnet.Subnets)
            {
                if (string.IsNullOrWhiteSpace(subnet.Id))
                {
                    continue;
                }

                knownIds.Add(subnet.Id);
                sources.Add(new Source("Subnet", subnet.Id, subnet.Name, vnet.ResourceGroup, vnet.SubscriptionId, vnet.Location, null, vnet.Id));
            }
        }

        foreach (var nic in topology.Nics)
        {
            if (string.IsNullOrWhiteSpace(nic.Id))
            {
                continue;
            }

            knownIds.Add(nic.Id);
            var vnetId = VnetIdOf(nic.SubnetId);
            sources.Add(new Source("NIC", nic.Id, nic.Name, ResourceGroupOf(nic.Id), SubscriptionOf(nic.Id), nic.Location, nic.SubnetId, vnetId));
        }

        var nsgs = new Dictionary<string, Source>(StringComparer.OrdinalIgnoreCase);
        void AddNsg(string? id, string location)
        {
            if (string.IsNullOrWhiteSpace(id))
            {
                return;
            }

            if (!nsgs.TryGetValue(id, out var existing))
            {
                nsgs[id] = new Source("NSG", id, LastSegment(id), ResourceGroupOf(id), SubscriptionOf(id), location, null, null);
                return;
            }

            if (string.IsNullOrWhiteSpace(existing.Location) && !string.IsNullOrWhiteSpace(location))
            {
                nsgs[id] = existing with { Location = location };
            }
        }

        foreach (var vnet in topology.Vnets)
        {
            foreach (var subnet in vnet.Subnets)
            {
                AddNsg(subnet.NsgId, vnet.Location);
            }
        }

        foreach (var nic in topology.Nics)
        {
            AddNsg(nic.NsgId, nic.Location);
        }

        foreach (var log in logs.Where(log => log.TargetKind == "NSG"))
        {
            AddNsg(log.Target, log.Location);
        }

        foreach (var nsg in nsgs.Values)
        {
            knownIds.Add(nsg.Id);
            sources.Add(nsg);
        }

        var context = new Context(byTarget, accounts, knownIds, topology, staleAfterHours);
        var covered = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var findings = new List<FlowLogFinding>();

        foreach (var source in sources.Where(source => source.Kind != "NSG"))
        {
            var finding = Evaluate(source, context);
            if (finding is null)
            {
                covered.Add(source.Id);
            }
            else
            {
                findings.Add(finding);
            }
        }

        foreach (var source in sources.Where(source => source.Kind == "NSG"))
        {
            var finding = Evaluate(source, context);
            if (finding is null)
            {
                continue;
            }

            if (finding.Reason is DiagnosisReason.NotConfigured or DiagnosisReason.NetworkWatcherMissing or DiagnosisReason.Disabled
                && AttachmentsCovered(source.Id, topology, covered))
            {
                continue;
            }

            findings.Add(finding);
        }

        var orphans = 0;
        foreach (var log in logs)
        {
            if (knownIds.Contains(log.Target))
            {
                continue;
            }

            orphans++;
            if (IsDelivering(log))
            {
                continue;
            }

            var source = new Source(
                string.IsNullOrEmpty(log.TargetKind) ? "Other" : log.TargetKind,
                log.Target,
                log.TargetName,
                log.TargetResourceGroup,
                SubscriptionOf(log.Target).Length > 0 ? SubscriptionOf(log.Target) : log.SubscriptionId,
                log.Location,
                null,
                VnetIdOf(log.Target));
            findings.Add(ToFinding(source, log, DiagnoseConfig(log, source, context)));
        }

        var filter = storageAccounts is { Count: > 0 }
            ? storageAccounts.ToHashSet(StringComparer.OrdinalIgnoreCase)
            : null;
        if (filter is not null)
        {
            findings = findings
                .Where(finding => finding.StorageAccount.Length == 0 || filter.Contains(finding.StorageAccount))
                .ToList();
        }

        findings = findings
            .OrderBy(finding => finding.SubscriptionId, StringComparer.OrdinalIgnoreCase)
            .ThenBy(finding => KindRank(finding.Kind))
            .ThenBy(finding => finding.ResourceGroup, StringComparer.OrdinalIgnoreCase)
            .ThenBy(finding => finding.Name, StringComparer.OrdinalIgnoreCase)
            .ToList();

        return new DiagnosisResult
        {
            GeneratedUtc = IsoTime.Format(now),
            StaleAfterHours = staleAfterHours,
            SourceCount = sources.Count + orphans,
            FindingCount = findings.Count,
            Findings = findings
        };
    }

    /// <summary>Null when a delivering flow log covers the source.</summary>
    private static FlowLogFinding? Evaluate(Source source, Context context)
    {
        foreach (var id in Chain(source))
        {
            if (!context.ByTarget.TryGetValue(id, out var configs))
            {
                continue;
            }

            var enabled = configs.Where(config => config.Enabled).ToList();
            if (enabled.Count == 0)
            {
                continue;
            }

            if (enabled.Any(IsDelivering))
            {
                return null;
            }

            var worst = enabled
                .Select(config => (Config: config, Diagnosis: DiagnoseConfig(config, source, context)))
                .OrderBy(item => DiagnosisReason.Rank(item.Diagnosis.Reason))
                .ThenBy(item => item.Config.Name, StringComparer.OrdinalIgnoreCase)
                .First();
            return ToFinding(source, worst.Config, worst.Diagnosis);
        }

        foreach (var id in Chain(source))
        {
            if (!context.ByTarget.TryGetValue(id, out var configs))
            {
                continue;
            }

            var disabled = configs.Where(config => !config.Enabled).OrderBy(config => config.Name, StringComparer.OrdinalIgnoreCase).FirstOrDefault();
            if (disabled is not null)
            {
                return ToFinding(source, disabled, DiagnoseConfig(disabled, source, context));
            }
        }

        if (!HasWatcher(source, context.Topology))
        {
            var where = string.IsNullOrWhiteSpace(source.Location) ? "this subscription" : source.Location;
            return Bare(source, DiagnosisReason.NetworkWatcherMissing,
                $"No Network Watcher in {where}. Flow logs are created on the regional Network Watcher, so nothing is sent until one exists.");
        }

        return Bare(source, DiagnosisReason.NotConfigured, NotConfiguredMessage(source));
    }

    private static (string Reason, string Message) DiagnoseConfig(FlowLogConfigInfo config, Source source, Context context)
    {
        var scope = ScopeText(config, source);
        if (!config.Enabled)
        {
            return (DiagnosisReason.Disabled,
                NsgNote(config, $"Flow log {config.Name} targets {scope} but is disabled, so nothing is written to {config.StorageAccount}."));
        }

        if (ProvisioningBlocks(config.ProvisioningState))
        {
            return (DiagnosisReason.ProvisioningFailed,
                NsgNote(config, $"Flow log {config.Name} provisioning state is {config.ProvisioningState}, so Azure is not writing it to {config.StorageAccount}."));
        }

        if (!context.KnownIds.Contains(config.Target))
        {
            return (DiagnosisReason.TargetMissing,
                NsgNote(config, $"Flow log {config.Name} targets {config.Target}, which is not in the scanned subscriptions. A missing virtual network, subnet, network interface, or network security group sends nothing."));
        }

        if (!context.Accounts.TryGetValue(config.StorageAccount, out var account))
        {
            return (DiagnosisReason.StorageMissing,
                NsgNote(config, $"Flow log {config.Name} sends to storage account {config.StorageAccount}, which is not in the scanned subscriptions. The account may have been deleted, or it lives in a subscription this identity cannot read."));
        }

        var resourceLocation = string.IsNullOrWhiteSpace(source.Location) ? config.Location : source.Location;
        if (LocationsDiffer(account.Location, resourceLocation))
        {
            return (DiagnosisReason.RegionMismatch,
                NsgNote(config, $"Storage account {account.Name} is in {account.Location}. {Label(source.Kind)} {source.Name} is in {resourceLocation}. Flow logs are written only when the storage account is in the same region as the virtual network or network security group."));
        }

        if (Unsupported(account))
        {
            return (DiagnosisReason.UnsupportedStorage,
                NsgNote(config, $"Storage account {account.Name} is {account.Kind} {account.SkuName}. Flow logs need a Standard general-purpose storage account. Premium storage is not supported."));
        }

        if (FirewallBlocks(account))
        {
            var access = string.IsNullOrWhiteSpace(account.PublicNetworkAccess) ? "unknown" : account.PublicNetworkAccess;
            var action = string.IsNullOrWhiteSpace(account.NetworkDefaultAction) ? "unknown" : account.NetworkDefaultAction;
            return (DiagnosisReason.StorageFirewall,
                NsgNote(config, $"Storage account {account.Name} does not allow Network Watcher to write blobs. Public network access is {access}, the firewall default action is {action}, and trusted Azure services are not allowed. Allow Azure services on the trusted services list."));
        }

        if (account.AllowSharedKeyAccess == false && !config.HasManagedIdentity)
        {
            return (DiagnosisReason.SharedKeyDisabled,
                NsgNote(config, $"Storage account {account.Name} disables shared key access, and flow log {config.Name} has no managed identity. Network Watcher cannot authenticate to the account. Assign a user-assigned managed identity to the flow log, or allow shared key access, then disable and re-enable the flow log."));
        }

        return BlobStatus(config, account, context);
    }

    private static (string Reason, string Message) BlobStatus(FlowLogConfigInfo config, StorageAccountInfo account, Context context)
    {
        if (config.Status == FlowStatus.AccessDenied)
        {
            return (DiagnosisReason.AccessDenied,
                NsgNote(config, $"Flow log {config.Name} is enabled, but blobs in {account.Name} could not be listed ({config.Message}). Grant Storage Blob Data Reader on the account to confirm whether logs are arriving."));
        }

        if (config.Status == FlowStatus.Error || (config.Status == FlowStatus.Healthy && string.IsNullOrEmpty(config.LatestBlobUtc) && account.Status == FlowStatus.Error))
        {
            var detail = string.IsNullOrWhiteSpace(config.Message) ? "the health check failed" : config.Message;
            return (DiagnosisReason.Error, NsgNote(config, $"Flow log {config.Name} could not be checked: {detail}"));
        }

        if (config.Status == FlowStatus.Stale)
        {
            var age = config.AgeHours?.ToString("0.0") ?? "unknown";
            return (DiagnosisReason.Stale,
                NsgNote(config, $"Flow log {config.Name} last wrote to {account.Name} {age} hours ago. The limit is {context.StaleAfterHours} hours, so new blobs are not arriving."));
        }

        var nsgGap = config.TargetKind == "NSG"
            ? $" The blob check reads {FlowLogPaths.ContainerName}. NSG flow logs are stored in {NsgContainer}, so this does not prove that container is empty."
            : string.Empty;
        return (DiagnosisReason.NoData,
            NsgNote(config, $"Flow log {config.Name} is enabled and {account.Name} accepts flow logs, but no blobs were written in the last 2 days. Rotating the storage account keys stops flow logs until the flow log is disabled and enabled again. No traffic on the source also leaves the container empty.{nsgGap}"));
    }

    private static bool IsDelivering(FlowLogConfigInfo config)
    {
        return config.Enabled && config.Status == FlowStatus.Healthy && !string.IsNullOrEmpty(config.LatestBlobUtc);
    }

    private static bool ProvisioningBlocks(string? state)
    {
        if (string.IsNullOrWhiteSpace(state))
        {
            return false;
        }

        return !state.Equals("Succeeded", StringComparison.OrdinalIgnoreCase)
            && !state.Equals("Updating", StringComparison.OrdinalIgnoreCase);
    }

    private static bool Unsupported(StorageAccountInfo account)
    {
        if (account.SkuName.StartsWith("Premium", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        return account.Kind.Equals("FileStorage", StringComparison.OrdinalIgnoreCase)
            || account.Kind.Equals("BlockBlobStorage", StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>
    /// Network Watcher writes as a trusted Azure service. A firewall blocks it when that exception
    /// is off and public access is disabled or the default action is Deny.
    /// </summary>
    private static bool FirewallBlocks(StorageAccountInfo account)
    {
        if (account.AllowsTrustedServices != false)
        {
            return false;
        }

        if (account.PublicNetworkAccess?.Equals("Disabled", StringComparison.OrdinalIgnoreCase) == true
            || account.PublicNetworkAccess?.Equals("SecuredByPerimeter", StringComparison.OrdinalIgnoreCase) == true)
        {
            return true;
        }

        return account.NetworkDefaultAction?.Equals("Deny", StringComparison.OrdinalIgnoreCase) == true;
    }

    private static bool LocationsDiffer(string storageLocation, string resourceLocation)
    {
        var storage = NormalizeLocation(storageLocation);
        var resource = NormalizeLocation(resourceLocation);
        return storage.Length > 0 && resource.Length > 0 && !string.Equals(storage, resource, StringComparison.Ordinal);
    }

    private static string NormalizeLocation(string? location)
    {
        if (string.IsNullOrWhiteSpace(location))
        {
            return string.Empty;
        }

        return location.Replace(" ", "", StringComparison.Ordinal).Replace("-", "", StringComparison.Ordinal).ToLowerInvariant();
    }

    private static bool HasWatcher(Source source, AzureTopology topology)
    {
        var location = NormalizeLocation(source.Location);
        if (location.Length == 0)
        {
            // No region on the source, so a missing watcher cannot be pinned to it.
            return true;
        }

        return topology.NetworkWatchers.Any(watcher =>
            (string.IsNullOrEmpty(source.SubscriptionId)
                || string.Equals(watcher.SubscriptionId, source.SubscriptionId, StringComparison.OrdinalIgnoreCase))
            && NormalizeLocation(watcher.Location) == location);
    }

    private static bool AttachmentsCovered(string nsgId, AzureTopology topology, HashSet<string> covered)
    {
        var attachments = new List<string>();
        foreach (var vnet in topology.Vnets)
        {
            foreach (var subnet in vnet.Subnets)
            {
                if (!string.IsNullOrWhiteSpace(subnet.Id) && string.Equals(subnet.NsgId, nsgId, StringComparison.OrdinalIgnoreCase))
                {
                    attachments.Add(subnet.Id);
                }
            }
        }

        foreach (var nic in topology.Nics)
        {
            if (!string.IsNullOrWhiteSpace(nic.Id) && string.Equals(nic.NsgId, nsgId, StringComparison.OrdinalIgnoreCase))
            {
                attachments.Add(nic.Id);
            }
        }

        return attachments.Count > 0 && attachments.All(covered.Contains);
    }

    private static IEnumerable<string> Chain(Source source)
    {
        yield return source.Id;
        if (source.Kind == "NIC" && !string.IsNullOrWhiteSpace(source.SubnetId))
        {
            yield return source.SubnetId;
        }

        if ((source.Kind is "NIC" or "Subnet") && !string.IsNullOrWhiteSpace(source.VnetId))
        {
            yield return source.VnetId;
        }
    }

    private static string NotConfiguredMessage(Source source)
    {
        var label = Label(source.Kind);
        var target = source.Kind is "Subnet" or "NIC"
            ? $"No flow log targets this {label} or its virtual network."
            : $"No flow log targets this {label}.";
        var where = string.IsNullOrWhiteSpace(source.Location)
            ? "a storage account in the same region"
            : $"a storage account in {source.Location}";
        if (source.Kind == "NSG")
        {
            return $"{target} NSG flow logs retire on 30 Sep 2027 and new ones cannot be created. Enable a virtual network flow log to {where}.";
        }

        return $"{target} Enable a virtual network flow log to {where}.";
    }

    private static string ScopeText(FlowLogConfigInfo config, Source source)
    {
        if (string.Equals(config.Target, source.Id, StringComparison.OrdinalIgnoreCase))
        {
            return $"this {Label(source.Kind)}";
        }

        var name = string.IsNullOrWhiteSpace(config.TargetName) ? "network" : config.TargetName;
        return $"parent {Label(config.TargetKind)} {name}";
    }

    private static string NsgNote(FlowLogConfigInfo config, string message)
    {
        if (config.TargetKind != "NSG")
        {
            return message;
        }

        return message + " NSG flow logs retire on 30 Sep 2027 and new ones cannot be created.";
    }

    private static string Label(string kind)
    {
        return kind switch
        {
            "VNet" => "virtual network",
            "Subnet" => "subnet",
            "NIC" => "network interface",
            "NSG" => "network security group",
            _ => "resource"
        };
    }

    private static int KindRank(string kind)
    {
        return kind switch
        {
            "VNet" => 0,
            "Subnet" => 1,
            "NIC" => 2,
            "NSG" => 3,
            _ => 4
        };
    }

    private static FlowLogFinding ToFinding(Source source, FlowLogConfigInfo config, (string Reason, string Message) diagnosis)
    {
        return new FlowLogFinding
        {
            SubscriptionId = source.SubscriptionId,
            ResourceGroup = source.ResourceGroup,
            Location = string.IsNullOrWhiteSpace(source.Location) ? config.Location : source.Location,
            Kind = source.Kind,
            Name = string.IsNullOrWhiteSpace(source.Name) ? config.TargetName : source.Name,
            ResourceId = source.Id,
            Reason = diagnosis.Reason,
            FlowLog = config.Name,
            StorageAccount = config.StorageAccount,
            Message = diagnosis.Message
        };
    }

    private static FlowLogFinding Bare(Source source, string reason, string message)
    {
        return new FlowLogFinding
        {
            SubscriptionId = source.SubscriptionId,
            ResourceGroup = source.ResourceGroup,
            Location = source.Location,
            Kind = source.Kind,
            Name = source.Name,
            ResourceId = source.Id,
            Reason = reason,
            Message = message
        };
    }

    private static string? VnetIdOf(string? id)
    {
        if (string.IsNullOrWhiteSpace(id))
        {
            return null;
        }

        var marker = "/subnets/";
        var index = id.IndexOf(marker, StringComparison.OrdinalIgnoreCase);
        return index < 0 ? null : id[..index];
    }

    private static string SubscriptionOf(string armId)
    {
        return SegmentAfter(armId, "subscriptions");
    }

    private static string ResourceGroupOf(string armId)
    {
        return SegmentAfter(armId, "resourceGroups");
    }

    private static string SegmentAfter(string armId, string marker)
    {
        var parts = armId.Split('/', StringSplitOptions.RemoveEmptyEntries);
        for (var i = 0; i < parts.Length - 1; i++)
        {
            if (parts[i].Equals(marker, StringComparison.OrdinalIgnoreCase))
            {
                return parts[i + 1];
            }
        }

        return string.Empty;
    }

    private static string LastSegment(string armId)
    {
        var parts = armId.Split('/', StringSplitOptions.RemoveEmptyEntries);
        return parts.Length == 0 ? armId : parts[^1];
    }

    private sealed record Source(
        string Kind,
        string Id,
        string Name,
        string ResourceGroup,
        string SubscriptionId,
        string Location,
        string? SubnetId,
        string? VnetId);

    private sealed record Context(
        Dictionary<string, List<FlowLogConfigInfo>> ByTarget,
        Dictionary<string, StorageAccountInfo> Accounts,
        HashSet<string> KnownIds,
        AzureTopology Topology,
        int StaleAfterHours);
}
