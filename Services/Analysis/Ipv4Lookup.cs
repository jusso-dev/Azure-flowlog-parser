using System.Net;
using System.Net.Sockets;
using AzureFlowLogParser.Models;

namespace AzureFlowLogParser.Services.Analysis;

/// <summary>
/// IPv4 longest-prefix-match table mapping address ranges to report scopes.
/// </summary>
public sealed class Ipv4Lookup
{
    private readonly List<Entry> _entries = new();
    private Entry[]? _sorted;

    private readonly record struct Entry(uint Network, uint Mask, int Length, ReportScope Scope);

    public int Count => _entries.Count;

    /// <summary>
    /// Adds a CIDR (10.0.0.0/16) or a single address (10.0.1.4, treated as /32).
    /// Returns false for anything that is not IPv4 (for example IPv6 subnet prefixes), which is skipped.
    /// </summary>
    public bool TryAdd(string cidr, ReportScope scope)
    {
        var parts = cidr.Split('/');
        if (!TryParseIpv4(parts[0], out var network))
        {
            return false;
        }

        var length = 32;
        if (parts.Length == 2 && (!int.TryParse(parts[1], out length) || length < 0 || length > 32))
        {
            return false;
        }

        var mask = length == 0 ? 0u : uint.MaxValue << (32 - length);
        _entries.Add(new Entry(network & mask, mask, length, scope));
        _sorted = null;
        return true;
    }

    /// <summary>Returns the most specific scope containing the address, or null when no entry matches.</summary>
    public ReportScope? Find(uint address)
    {
        _sorted ??= _entries.OrderByDescending(e => e.Length).ToArray();

        foreach (var entry in _sorted)
        {
            if ((address & entry.Mask) == entry.Network)
            {
                return entry.Scope;
            }
        }

        return null;
    }

    public static bool TryParseIpv4(string text, out uint value)
    {
        value = 0;
        if (!IPAddress.TryParse(text, out var address) || address.AddressFamily != AddressFamily.InterNetwork)
        {
            return false;
        }

        var bytes = address.GetAddressBytes();
        value = (uint)(bytes[0] << 24 | bytes[1] << 16 | bytes[2] << 8 | bytes[3]);
        return true;
    }

    /// <summary>
    /// Private, shared and link-local ranges (10/8, 172.16/12, 192.168/16, 100.64/10, 169.254/16).
    /// Used to split unmatched addresses into "internet" (public) and "unmapped" (private, unknown).
    /// </summary>
    public static bool IsPrivate(uint address)
    {
        var a = (byte)(address >> 24);
        var b = (byte)(address >> 16);

        return a == 10
            || (a == 172 && b >= 16 && b <= 31)
            || (a == 192 && b == 168)
            || (a == 100 && b >= 64 && b <= 127)
            || (a == 169 && b == 254);
    }
}
