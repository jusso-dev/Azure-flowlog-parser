using System.Text.Json;
using System.Text.Json.Serialization;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Serialization;

namespace AzureFlowLogParser.Services.Analysis;

/// <summary>
/// Offline topology: maps address prefixes to storage accounts or unlogged VNets for --input runs.
/// Shape: { "networks": [ { "name": "stflowlogs01", "kind": "storage", "prefixes": ["10.10.0.0/16"] } ],
///          "subscriptions": [ { "subscriptionId": "...", "displayName": "...", "tenantId": "..." } ] }
/// "subscriptions" is optional. Entries are read leniently so one malformed entry is noted, not fatal.
/// </summary>
public static class TopologyFile
{
    public sealed record Result(
        Ipv4Lookup Lookup,
        int Networks,
        int Prefixes,
        int SkippedPrefixes,
        IReadOnlyList<string> StorageAccounts,
        IReadOnlyList<ReportSubscription> Subscriptions,
        IReadOnlyList<string> SubscriptionNotes);

    public sealed class Document
    {
        [JsonPropertyName("networks")]
        public List<Network> Networks { get; set; } = new();

        [JsonPropertyName("subscriptions")]
        public List<JsonElement>? Subscriptions { get; set; }
    }

    public sealed class Network
    {
        [JsonPropertyName("name")]
        public string Name { get; set; } = string.Empty;

        [JsonPropertyName("kind")]
        public string Kind { get; set; } = string.Empty;

        [JsonPropertyName("prefixes")]
        public List<string> Prefixes { get; set; } = new();
    }

    public static Result Load(string path)
    {
        if (!File.Exists(path))
        {
            throw new FileNotFoundException($"Topology file not found: {path}");
        }

        Document document;
        try
        {
            document = JsonSerializer.Deserialize(File.ReadAllText(path), AppJsonContext.Default.Document)
                ?? throw new InvalidDataException($"Topology file is empty: {path}");
        }
        catch (JsonException ex)
        {
            // The JSON message does not name the file, so the error does.
            throw new InvalidDataException($"topology file {Path.GetFileName(path)}: {ex.Message}", ex);
        }

        var lookup = new Ipv4Lookup();
        var prefixes = 0;
        var skipped = 0;
        var storageAccounts = new List<string>();

        foreach (var network in document.Networks)
        {
            if (string.IsNullOrWhiteSpace(network.Name))
            {
                throw new InvalidDataException("Every topology network needs a name.");
            }

            if (network.Kind.Equals("storage", StringComparison.OrdinalIgnoreCase))
            {
                storageAccounts.Add(network.Name);
            }

            var scope = network.Kind.ToLowerInvariant() switch
            {
                "storage" => new ReportScope
                {
                    Id = network.Name,
                    Kind = ScopeKind.Storage,
                    Label = network.Name,
                    StorageAccount = network.Name
                },
                "vnet" => new ReportScope
                {
                    Id = $"vnet:{network.Name}",
                    Kind = ScopeKind.Vnet,
                    Label = network.Name
                },
                _ => throw new InvalidDataException($"Topology network '{network.Name}' has kind '{network.Kind}'. Use storage or vnet.")
            };

            foreach (var prefix in network.Prefixes)
            {
                if (lookup.TryAdd(prefix.Trim(), scope))
                {
                    prefixes++;
                }
                else
                {
                    skipped++;
                }
            }
        }

        var (subscriptions, subscriptionNotes) = ReadSubscriptions(document.Subscriptions ?? []);

        return new Result(lookup, document.Networks.Count, prefixes, skipped, storageAccounts, subscriptions, subscriptionNotes);
    }

    private static (List<ReportSubscription> Subscriptions, List<string> Notes) ReadSubscriptions(List<JsonElement> entries)
    {
        var subscriptions = new List<ReportSubscription>();
        var notes = new List<string>();

        for (var i = 0; i < entries.Count; i++)
        {
            var entry = entries[i];
            var position = i + 1;
            if (entry.ValueKind != JsonValueKind.Object)
            {
                notes.Add($"Topology subscription entry ignored: entry {position} is not an object.");
                continue;
            }

            var subscriptionId = TextProperty(entry, "subscriptionId");
            if (string.IsNullOrWhiteSpace(subscriptionId))
            {
                notes.Add($"Topology subscription entry ignored: entry {position} has no subscriptionId.");
                continue;
            }

            subscriptions.Add(new ReportSubscription
            {
                SubscriptionId = subscriptionId,
                DisplayName = TextProperty(entry, "displayName") ?? string.Empty,
                TenantId = TextProperty(entry, "tenantId") ?? string.Empty
            });
        }

        return (subscriptions, notes);
    }

    /// <summary>A string property, or null when it is missing or not a string.</summary>
    private static string? TextProperty(JsonElement entry, string name)
    {
        return entry.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String ? value.GetString() : null;
    }
}
