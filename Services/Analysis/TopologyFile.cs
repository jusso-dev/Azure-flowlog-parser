using System.Text.Json;
using System.Text.Json.Serialization;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Serialization;

namespace AzureFlowLogParser.Services.Analysis;

/// <summary>
/// Offline topology: maps address prefixes to storage accounts or unlogged VNets for --input runs.
/// Shape: { "networks": [ { "name": "stflowlogs01", "kind": "storage", "prefixes": ["10.10.0.0/16"] } ] }
/// </summary>
public static class TopologyFile
{
    public sealed record Result(Ipv4Lookup Lookup, int Networks, int Prefixes, int SkippedPrefixes, IReadOnlyList<string> StorageAccounts);

    public sealed class Document
    {
        [JsonPropertyName("networks")]
        public List<Network> Networks { get; set; } = new();
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

        var document = JsonSerializer.Deserialize(File.ReadAllText(path), AppJsonContext.Default.Document)
            ?? throw new InvalidDataException($"Topology file is empty: {path}");

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

        return new Result(lookup, document.Networks.Count, prefixes, skipped, storageAccounts);
    }
}
