using System.Text.Json;

namespace AzureFlowLogParser.Services.Common;

public static class JsonOptions
{
    /// <summary>Indented camelCase JSON for files and console output.</summary>
    public static readonly JsonSerializerOptions Indented = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        WriteIndented = true
    };

    /// <summary>Single-line camelCase JSON for embedding in HTML.</summary>
    public static readonly JsonSerializerOptions Compact = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        WriteIndented = false
    };
}
