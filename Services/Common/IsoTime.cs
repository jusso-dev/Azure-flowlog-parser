using System.Globalization;

namespace AzureFlowLogParser.Services.Common;

public static class IsoTime
{
    /// <summary>Formats a timestamp as ISO-8601 UTC with a Z suffix (e.g. 2026-10-19T10:00:00Z).</summary>
    public static string Format(DateTimeOffset value)
    {
        return value.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", CultureInfo.InvariantCulture);
    }

    public static string? FormatOrNull(DateTimeOffset? value)
    {
        return value.HasValue ? Format(value.Value) : null;
    }
}
