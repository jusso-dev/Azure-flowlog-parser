using System.Globalization;
using System.Text.RegularExpressions;

namespace AzureFlowLogParser.Services.Analysis;

/// <summary>
/// Per-observation filters: action, protocol, destination port list/ranges, IP substring and minimum bytes.
/// </summary>
public sealed class TrafficFilter
{
    public string Action { get; }
    public string Protocol { get; }
    public string Ports { get; }
    public string Ip { get; }
    public long MinBytes { get; }

    private readonly List<(int Low, int High)> _portRanges;

    private TrafficFilter(string action, string protocol, string ports, string ip, long minBytes)
    {
        Action = action;
        Protocol = protocol;
        Ports = ports;
        Ip = ip;
        MinBytes = minBytes;
        _portRanges = ParsePortRanges(ports);
    }

    /// <summary>Validates and builds the filter. action: all|allow|deny; protocol: all|tcp|udp.</summary>
    public static TrafficFilter Create(string? action, string? protocol, string? ports, string? ip, long minBytes)
    {
        var normalizedAction = (action ?? "all").Trim().ToLowerInvariant();
        if (normalizedAction is not ("all" or "allow" or "deny"))
        {
            throw new ArgumentException($"Invalid --action '{action}'. Use all, allow or deny.");
        }

        var normalizedProtocol = (protocol ?? "all").Trim().ToLowerInvariant();
        normalizedProtocol = normalizedProtocol switch
        {
            "all" => "all",
            "tcp" => "TCP",
            "udp" => "UDP",
            _ => throw new ArgumentException($"Invalid --protocol '{protocol}'. Use all, tcp or udp.")
        };

        if (minBytes < 0)
        {
            throw new ArgumentException("--min-bytes cannot be negative.");
        }

        return new TrafficFilter(normalizedAction, normalizedProtocol, ports?.Trim() ?? string.Empty, ip?.Trim() ?? string.Empty, minBytes);
    }

    /// <summary>
    /// Matches one observation. Port filter checks the destination port only; IP filter is a
    /// case-insensitive substring of the source or destination address.
    /// </summary>
    public bool Matches(string action, string protocol, int dstPort, string src, string dst, long bytes)
    {
        if (Action == "allow" && action != "A") return false;
        if (Action == "deny" && action != "D") return false;
        if (Protocol != "all" && !string.Equals(protocol, Protocol, StringComparison.Ordinal)) return false;
        if (_portRanges.Count > 0 && !_portRanges.Any(r => dstPort >= r.Low && dstPort <= r.High)) return false;
        if (Ip.Length > 0 &&
            src.IndexOf(Ip, StringComparison.OrdinalIgnoreCase) < 0 &&
            dst.IndexOf(Ip, StringComparison.OrdinalIgnoreCase) < 0) return false;
        if (bytes < MinBytes) return false;
        return true;
    }

    /// <summary>Parses "443,8000-8100" into ranges. Empty input means no port filter.</summary>
    public static List<(int Low, int High)> ParsePortRanges(string? spec)
    {
        var ranges = new List<(int Low, int High)>();
        if (string.IsNullOrWhiteSpace(spec))
        {
            return ranges;
        }

        foreach (var token in spec.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            var bounds = token.Split('-', StringSplitOptions.TrimEntries);
            if (bounds.Length > 2 ||
                !int.TryParse(bounds[0], NumberStyles.None, CultureInfo.InvariantCulture, out var low) ||
                !int.TryParse(bounds[^1], NumberStyles.None, CultureInfo.InvariantCulture, out var high) ||
                low < 0 || high > 65535 || low > high)
            {
                throw new ArgumentException($"Invalid port or range '{token}'. Use values like 443 or 8000-8100.");
            }

            ranges.Add((low, high));
        }

        return ranges;
    }

    /// <summary>
    /// Parses a window bound: "90m", "24h", "7d" relative to now, or an ISO-8601 timestamp (UTC when no offset).
    /// </summary>
    public static DateTimeOffset ParseWindowBound(string value, DateTimeOffset now)
    {
        var relative = Regex.Match(value.Trim(), @"^(?<n>\d+)(?<u>[mhd])$", RegexOptions.IgnoreCase);
        if (relative.Success)
        {
            var amount = int.Parse(relative.Groups["n"].Value, CultureInfo.InvariantCulture);
            return relative.Groups["u"].Value.ToLowerInvariant() switch
            {
                "m" => now.AddMinutes(-amount),
                "h" => now.AddHours(-amount),
                _ => now.AddDays(-amount)
            };
        }

        if (DateTimeOffset.TryParse(value, CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal, out var absolute))
        {
            return absolute;
        }

        throw new ArgumentException($"Invalid time '{value}'. Use 90m, 24h, 7d or an ISO-8601 timestamp.");
    }
}
