using System.Text.Json.Serialization;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Analysis;

namespace AzureFlowLogParser.Serialization;

/// <summary>
/// Compact camelCase JSON metadata for the types read from or written to JSON.
/// Native AOT has no runtime reflection, so every serialized type must be listed here.
/// </summary>
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase, PropertyNameCaseInsensitive = true)]
[JsonSerializable(typeof(FlowLogRoot))]
[JsonSerializable(typeof(DenormalizedFlowRecord))]
[JsonSerializable(typeof(List<DenormalizedFlowRecord>))]
[JsonSerializable(typeof(ReportModel))]
[JsonSerializable(typeof(TopologyFile.Document))]
public partial class AppJsonContext : JsonSerializerContext
{
}

/// <summary>
/// Indented camelCase JSON metadata for files and console output.
/// </summary>
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase, WriteIndented = true)]
[JsonSerializable(typeof(InventoryResult))]
[JsonSerializable(typeof(List<DenormalizedFlowRecord>))]
public partial class IndentedJsonContext : JsonSerializerContext
{
}
