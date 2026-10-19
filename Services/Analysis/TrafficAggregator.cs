using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Common;

namespace AzureFlowLogParser.Services.Analysis;

/// <summary>
/// Streams flow observations into aggregated rows, attributing each endpoint to a scope.
/// Records are never retained individually, so large estates stay within bounded memory.
/// </summary>
public sealed class TrafficAggregator
{
    public const int MaxRows = 20000;

    private static readonly ReportScope InternetScope = new() { Id = "internet", Kind = ScopeKind.Internet, Label = "Internet" };
    private static readonly ReportScope UnmappedScope = new() { Id = "unmapped", Kind = ScopeKind.Unmapped, Label = "Unmapped" };

    private readonly TrafficFilter _filter;
    private readonly DateTimeOffset? _from;
    private readonly DateTimeOffset? _to;
    private readonly Ipv4Lookup? _lookup;
    private readonly HashSet<string> _readAccounts;
    private readonly Dictionary<string, ReportScope> _scopesById = new(StringComparer.Ordinal);
    private readonly Dictionary<FlowKey, Accumulator> _rows = new();
    private readonly Dictionary<string, ReportScope> _ipScopeCache = new(StringComparer.Ordinal);
    private readonly HashSet<string> _sourceIps = new(StringComparer.Ordinal);
    private readonly HashSet<string> _destinationIps = new(StringComparer.Ordinal);
    private readonly object _gate = new();

    private long _flows;
    private long _allowed;
    private long _denied;
    private long _bytes;
    private long _packets;
    private long _skipped;
    private DateTimeOffset? _observedFirst;
    private DateTimeOffset? _observedLast;

    /// <param name="from">Window start, or null for no lower bound.</param>
    /// <param name="to">Window end, or null for no upper bound.</param>
    /// <param name="readAccounts">Storage accounts whose flow logs are read in this run (used by the dedupe rule).</param>
    public TrafficAggregator(TrafficFilter filter, DateTimeOffset? from, DateTimeOffset? to, Ipv4Lookup? lookup, IEnumerable<string> readAccounts)
    {
        _filter = filter;
        _from = from;
        _to = to;
        _lookup = lookup;
        _readAccounts = new HashSet<string>(readAccounts, StringComparer.OrdinalIgnoreCase);
    }

    /// <summary>Adds one parsed file's observations. Safe to call from several threads.</summary>
    public void Add(IEnumerable<DenormalizedFlowRecord> records)
    {
        lock (_gate)
        {
            foreach (var record in records)
            {
                AddObservation(record);
            }
        }
    }

    private void AddObservation(DenormalizedFlowRecord record)
    {
        var observed = ObservationTime(record);
        if (observed is null || (_from is { } from && observed < from) || (_to is { } to && observed > to))
        {
            return;
        }

        var action = record.DeviceAction;
        var protocol = record.TransportProtocol switch
        {
            "T" => "TCP",
            "U" => "UDP",
            var other => other
        };
        _ = int.TryParse(record.DestinationPort, out var dstPort);
        var bytes = ParseCount(record.BytesStoD) + ParseCount(record.BytesDtoS);
        var packets = ParseCount(record.PacketsStoD) + ParseCount(record.PacketsDtoS);

        if (!_filter.Matches(action, protocol, dstPort, record.SourceAddress, record.DestinationAddress, bytes))
        {
            return;
        }

        var srcScope = ScopeFor(record.SourceAddress);
        var dstScope = ScopeFor(record.DestinationAddress);

        // An allowed inbound observation has a matching outbound observation at the source NSG, which is
        // already counted when that storage account is read. Denied inbound stays: it is a separate decision.
        if (record.DeviceDirection == "I" && action == "A" &&
            srcScope.Kind == ScopeKind.Storage && _readAccounts.Contains(srcScope.StorageAccount))
        {
            _skipped++;
            return;
        }

        // Observed range covers the observations that are kept, so it is the window when none was given.
        _observedFirst = _observedFirst is { } first && first <= observed ? first : observed;
        _observedLast = _observedLast is { } last && last >= observed ? last : observed;

        RegisterScope(srcScope);
        RegisterScope(dstScope);

        var key = new FlowKey(
            record.SourceAddress,
            record.DestinationAddress,
            srcScope.Id,
            dstScope.Id,
            protocol,
            dstPort,
            action,
            record.DeviceDirection,
            record.NsgRuleName);

        if (!_rows.TryGetValue(key, out var accumulator))
        {
            accumulator = new Accumulator(observed.Value);
            _rows[key] = accumulator;
        }

        accumulator.Add(observed.Value, packets, bytes);

        _flows++;
        if (action == "A") _allowed++;
        if (action == "D") _denied++;
        _bytes += bytes;
        _packets += packets;
        _sourceIps.Add(record.SourceAddress);
        _destinationIps.Add(record.DestinationAddress);
    }

    /// <summary>Earliest and latest kept observation, or null when nothing was kept.</summary>
    public (DateTimeOffset? First, DateTimeOffset? Last) ObservedRange
    {
        get
        {
            lock (_gate)
            {
                return (_observedFirst, _observedLast);
            }
        }
    }

    /// <summary>Fills summary, flows and scopes of the model. Totals cover every kept observation; rows are capped.</summary>
    public void FillModel(ReportModel model, int subscriptionCount)
    {
        lock (_gate)
        {
            var ordered = _rows
                .OrderByDescending(kv => kv.Value.Bytes)
                .ThenByDescending(kv => kv.Value.Flows)
                .ThenBy(kv => kv.Key.Src, StringComparer.Ordinal)
                .ThenBy(kv => kv.Key.Dst, StringComparer.Ordinal)
                .ThenBy(kv => kv.Key.DstPort)
                // Remaining ties follow the key field order, so the output no longer depends on parallel read order.
                .ThenBy(kv => kv.Key.SrcScope, StringComparer.Ordinal)
                .ThenBy(kv => kv.Key.DstScope, StringComparer.Ordinal)
                .ThenBy(kv => kv.Key.Proto, StringComparer.Ordinal)
                .ThenBy(kv => kv.Key.Action, StringComparer.Ordinal)
                .ThenBy(kv => kv.Key.Direction, StringComparer.Ordinal)
                .ThenBy(kv => kv.Key.Rule, StringComparer.Ordinal)
                .ToList();

            var emitted = ordered.Take(MaxRows).ToList();

            model.Flows = emitted.Select(kv => new ReportFlow
            {
                First = IsoTime.Format(kv.Value.First),
                Last = IsoTime.Format(kv.Value.Last),
                Src = kv.Key.Src,
                Dst = kv.Key.Dst,
                SrcScope = kv.Key.SrcScope,
                DstScope = kv.Key.DstScope,
                Proto = kv.Key.Proto,
                DstPort = kv.Key.DstPort,
                Action = kv.Key.Action,
                Direction = kv.Key.Direction,
                Rule = kv.Key.Rule,
                Flows = kv.Value.Flows,
                Packets = kv.Value.Packets,
                Bytes = kv.Value.Bytes
            }).ToList();

            var usedScopeIds = emitted
                .SelectMany(kv => new[] { kv.Key.SrcScope, kv.Key.DstScope })
                .Distinct(StringComparer.Ordinal)
                .ToHashSet(StringComparer.Ordinal);

            model.Scopes = _scopesById.Values
                .Where(s => usedScopeIds.Contains(s.Id))
                .OrderBy(s => s.Kind, StringComparer.Ordinal)
                .ThenBy(s => s.Id, StringComparer.Ordinal)
                .ToList();

            model.Summary = new ReportSummary
            {
                Flows = _flows,
                Allowed = _allowed,
                Denied = _denied,
                Bytes = _bytes,
                Packets = _packets,
                Rows = model.Flows.Count,
                RowsTotal = ordered.Count,
                RowsTruncated = ordered.Count > MaxRows,
                StorageAccounts = _readAccounts.Count,
                Scopes = model.Scopes.Count,
                Subscriptions = subscriptionCount,
                ObservationsSkipped = _skipped,
                UniqueSourceIps = _sourceIps.Count,
                UniqueDestinationIps = _destinationIps.Count
            };
        }
    }

    private ReportScope ScopeFor(string address)
    {
        if (_ipScopeCache.TryGetValue(address, out var cached))
        {
            return cached;
        }

        ReportScope scope;
        if (!Ipv4Lookup.TryParseIpv4(address, out var value))
        {
            scope = UnmappedScope;
        }
        else
        {
            scope = _lookup?.Find(value) ?? (Ipv4Lookup.IsPrivate(value) ? UnmappedScope : InternetScope);
        }

        _ipScopeCache[address] = scope;
        return scope;
    }

    private void RegisterScope(ReportScope scope)
    {
        _scopesById.TryAdd(scope.Id, scope);
    }

    private static DateTimeOffset? ObservationTime(DenormalizedFlowRecord record)
    {
        if (long.TryParse(record.StartTime, out var unixSeconds))
        {
            return DateTimeOffset.FromUnixTimeSeconds(unixSeconds);
        }

        return DateTimeOffset.TryParse(record.Time, out var fromRecord) ? fromRecord : null;
    }

    private static long ParseCount(string? value)
    {
        return long.TryParse(value, out var count) ? count : 0;
    }

    private readonly record struct FlowKey(
        string Src,
        string Dst,
        string SrcScope,
        string DstScope,
        string Proto,
        int DstPort,
        string Action,
        string Direction,
        string Rule);

    private sealed class Accumulator
    {
        public Accumulator(DateTimeOffset seen)
        {
            First = seen;
            Last = seen;
        }

        public DateTimeOffset First { get; private set; }
        public DateTimeOffset Last { get; private set; }
        public long Flows { get; private set; }
        public long Packets { get; private set; }
        public long Bytes { get; private set; }

        public void Add(DateTimeOffset seen, long packets, long bytes)
        {
            if (seen < First) First = seen;
            if (seen > Last) Last = seen;
            Flows++;
            Packets += packets;
            Bytes += bytes;
        }
    }
}
