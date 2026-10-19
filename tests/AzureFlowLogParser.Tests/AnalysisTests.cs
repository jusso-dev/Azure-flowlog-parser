using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services;
using AzureFlowLogParser.Services.Analysis;
using Xunit;

namespace AzureFlowLogParser.Tests;

public class AnalysisTests
{
    private static readonly DateTimeOffset SampleFrom = new(2024, 1, 1, 0, 0, 0, TimeSpan.Zero);
    private static readonly DateTimeOffset SampleTo = new(2024, 2, 1, 0, 0, 0, TimeSpan.Zero);

    private static TrafficFilter AllTraffic() => TrafficFilter.Create(null, null, null, null, 0);

    private static DenormalizedFlowRecord Observation(
        string src, string dst, string action = "A", string direction = "O",
        string protocol = "T", string dstPort = "443", long bytes = 100, long packets = 1, string startTime = "1705318200", string rule = "rule")
    {
        return new DenormalizedFlowRecord
        {
            StartTime = startTime,
            Time = "2024-01-15T10:30:00Z",
            SourceAddress = src,
            DestinationAddress = dst,
            SourcePort = "50000",
            DestinationPort = dstPort,
            TransportProtocol = protocol,
            DeviceDirection = direction,
            DeviceAction = action,
            NsgRuleName = rule,
            Version = 2,
            PacketsStoD = packets.ToString(),
            PacketsDtoS = "0",
            BytesStoD = bytes.ToString(),
            BytesDtoS = "0"
        };
    }

    private static ReportModel Build(TrafficAggregator aggregator)
    {
        var model = new ReportModel();
        aggregator.FillModel(model, subscriptionCount: 0);
        return model;
    }

    [Fact]
    public void Sample_file_aggregates_to_six_observations_in_four_rows()
    {
        var aggregator = new TrafficAggregator(AllTraffic(), SampleFrom, SampleTo, lookup: null, readAccounts: Array.Empty<string>());

        aggregator.Add(FlowLogParser.ParseFlowLog(File.ReadAllText(TestPaths.SampleFlowLog)));
        var model = Build(aggregator);

        Assert.Equal(6, model.Summary.Flows);
        Assert.Equal(5, model.Summary.Allowed);
        Assert.Equal(1, model.Summary.Denied);
        Assert.Equal(4, model.Summary.RowsTotal);
        Assert.False(model.Summary.RowsTruncated);
        // Largest row: 10.0.1.4 -> 13.107.42.14 on 443, three tuples totalling 2400 + 5400 + 700 bytes.
        Assert.Equal(3, model.Flows[0].Flows);
        Assert.Equal(8500, model.Flows[0].Bytes);
    }

    [Fact]
    public void Observations_outside_the_window_are_excluded()
    {
        var aggregator = new TrafficAggregator(AllTraffic(), new DateTimeOffset(2025, 1, 1, 0, 0, 0, TimeSpan.Zero), SampleTo, null, Array.Empty<string>());

        aggregator.Add(FlowLogParser.ParseFlowLog(File.ReadAllText(TestPaths.SampleFlowLog)));

        Assert.Equal(0, Build(aggregator).Summary.Flows);
    }

    [Fact]
    public void Action_and_port_filters_apply_per_observation()
    {
        var deny = new TrafficAggregator(TrafficFilter.Create("deny", null, null, null, 0), SampleFrom, SampleTo, null, Array.Empty<string>());
        var port443 = new TrafficAggregator(TrafficFilter.Create(null, "tcp", "443", null, 0), SampleFrom, SampleTo, null, Array.Empty<string>());
        var rows = FlowLogParser.ParseFlowLog(File.ReadAllText(TestPaths.SampleFlowLog));

        deny.Add(rows);
        port443.Add(rows);

        Assert.Equal(1, Build(deny).Summary.Flows);
        Assert.Equal(3, Build(port443).Summary.Flows);
    }

    [Fact]
    public void Minimum_bytes_and_ip_substring_filters_apply()
    {
        var aggregator = new TrafficAggregator(TrafficFilter.Create(null, null, null, "10.0.2.", 500), SampleFrom, SampleTo, null, Array.Empty<string>());
        aggregator.Add(new[]
        {
            Observation("10.0.2.10", "10.0.1.4", bytes: 1000),
            Observation("10.0.1.4", "13.107.42.14", bytes: 1000),
            Observation("10.0.2.10", "10.0.1.4", bytes: 100)
        });

        Assert.Equal(1, Build(aggregator).Summary.Flows);
    }

    [Fact]
    public void Allowed_inbound_from_a_read_storage_network_is_skipped_as_duplicate()
    {
        var lookup = new Ipv4Lookup();
        lookup.TryAdd("10.0.1.0/24", new ReportScope { Id = "stflowlogs01", Kind = ScopeKind.Storage, Label = "stflowlogs01", StorageAccount = "stflowlogs01" });
        var aggregator = new TrafficAggregator(AllTraffic(), SampleFrom, SampleTo, lookup, readAccounts: new[] { "stflowlogs01" });

        aggregator.Add(new[]
        {
            Observation("10.0.1.4", "13.107.42.14", action: "A", direction: "O"),   // outbound: kept
            Observation("10.0.1.4", "10.0.9.9", action: "A", direction: "I"),       // allowed inbound from read account: skipped
            Observation("10.0.1.4", "10.0.9.9", action: "D", direction: "I")        // denied inbound: kept
        });

        var model = Build(aggregator);
        Assert.Equal(2, model.Summary.Flows);
        Assert.Equal(1, model.Summary.ObservationsSkipped);
    }

    [Fact]
    public void Inbound_from_an_unread_storage_network_is_kept()
    {
        var lookup = new Ipv4Lookup();
        lookup.TryAdd("10.0.1.0/24", new ReportScope { Id = "stflowlogs01", Kind = ScopeKind.Storage, Label = "stflowlogs01", StorageAccount = "stflowlogs01" });
        var aggregator = new TrafficAggregator(AllTraffic(), SampleFrom, SampleTo, lookup, readAccounts: Array.Empty<string>());

        aggregator.Add(new[] { Observation("10.0.1.4", "10.0.9.9", action: "A", direction: "I") });

        Assert.Equal(1, Build(aggregator).Summary.Flows);
    }

    [Fact]
    public void Scopes_come_from_lookup_or_fall_back_to_internet_and_unmapped()
    {
        var lookup = new Ipv4Lookup();
        lookup.TryAdd("10.0.2.0/24", new ReportScope { Id = "vnet:vnet-app", Kind = ScopeKind.Vnet, Label = "vnet-app" });
        var aggregator = new TrafficAggregator(AllTraffic(), SampleFrom, SampleTo, lookup, Array.Empty<string>());

        aggregator.Add(new[]
        {
            Observation("10.0.2.5", "8.8.8.8"),
            Observation("10.9.9.9", "8.8.8.8"),
            Observation("not-an-ip", "8.8.8.8")
        });

        var model = Build(aggregator);
        var scopeIds = model.Scopes.Select(s => s.Id).OrderBy(id => id, StringComparer.Ordinal).ToArray();
        Assert.Equal(new[] { "internet", "unmapped", "vnet:vnet-app" }, scopeIds);
    }

    [Fact]
    public void Rows_are_capped_but_totals_include_every_observation()
    {
        var aggregator = new TrafficAggregator(AllTraffic(), SampleFrom, SampleTo, null, Array.Empty<string>());
        var records = Enumerable.Range(0, TrafficAggregator.MaxRows + 5)
            .Select(i => Observation("10.1.0.1", $"10.2.{i / 250}.{i % 250}", dstPort: "443", bytes: 10))
            .ToList();

        aggregator.Add(records);
        var model = Build(aggregator);

        Assert.True(model.Summary.RowsTruncated);
        Assert.Equal(TrafficAggregator.MaxRows, model.Flows.Count);
        Assert.Equal(TrafficAggregator.MaxRows + 5, model.Summary.RowsTotal);
        Assert.Equal(TrafficAggregator.MaxRows + 5, model.Summary.Flows);
    }

    [Fact]
    public void Ipv4_longest_prefix_wins_and_non_ipv4_is_rejected()
    {
        var wide = new ReportScope { Id = "wide" };
        var narrow = new ReportScope { Id = "narrow" };
        var lookup = new Ipv4Lookup();
        lookup.TryAdd("10.0.0.0/16", wide);
        lookup.TryAdd("10.0.1.0/24", narrow);

        Assert.Equal("narrow", lookup.Find(Ipv4Parse("10.0.1.200"))!.Id);
        Assert.Equal("wide", lookup.Find(Ipv4Parse("10.0.9.1"))!.Id);
        Assert.Null(lookup.Find(Ipv4Parse("11.0.0.1")));
        Assert.False(lookup.TryAdd("fd00::/64", wide));
        Assert.False(lookup.TryAdd("10.0.0.0/33", wide));
    }

    [Theory]
    [InlineData("10.1.2.3", true)]
    [InlineData("172.16.0.1", true)]
    [InlineData("172.32.0.1", false)]
    [InlineData("192.168.1.1", true)]
    [InlineData("100.64.1.1", true)]
    [InlineData("169.254.1.1", true)]
    [InlineData("13.107.42.14", false)]
    public void Private_ranges_are_classified(string address, bool isPrivate)
    {
        Assert.Equal(isPrivate, Ipv4Lookup.IsPrivate(Ipv4Parse(address)));
    }

    [Fact]
    public void Topology_file_builds_storage_and_vnet_scopes()
    {
        var result = TopologyFile.Load(TestPaths.Topology);

        Assert.Equal(3, result.Networks);
        Assert.Equal(3, result.Prefixes);
        Assert.Equal(1, result.SkippedPrefixes);
        Assert.Equal(new[] { "stflowlogs01", "stflowlogs02" }, result.StorageAccounts);
        Assert.Equal("stflowlogs02", result.Lookup.Find(Ipv4Parse("10.0.2.10"))!.Id);
        Assert.Equal("vnet:vnet-app", result.Lookup.Find(Ipv4Parse("10.0.2.20"))!.Id);
        Assert.Equal(ScopeKind.Storage, result.Lookup.Find(Ipv4Parse("10.0.1.4"))!.Kind);
    }

    [Fact]
    public void Topology_file_rejects_unknown_kind()
    {
        var path = Path.Combine(TestPaths.NewTempDirectory(), "bad.json");
        File.WriteAllText(path, """{"networks":[{"name":"x","kind":"lake","prefixes":["10.0.0.0/8"]}]}""");

        Assert.Throws<InvalidDataException>(() => TopologyFile.Load(path));
    }

    [Fact]
    public void Topology_file_without_subscriptions_lists_none()
    {
        var result = TopologyFile.Load(TestPaths.Topology);

        Assert.Empty(result.Subscriptions);
        Assert.Empty(result.SubscriptionNotes);
    }

    [Fact]
    public void Topology_file_reads_subscriptions_in_file_order_and_ignores_unknown_keys()
    {
        var path = Path.Combine(TestPaths.NewTempDirectory(), "subscriptions.json");
        File.WriteAllText(path, """
            {
              "version": 2,
              "networks": [ { "name": "stflowlogs01", "kind": "storage", "prefixes": ["10.0.1.0/24"] } ],
              "subscriptions": [
                { "subscriptionId": "00000000-0000-0000-0000-00000000000b", "displayName": "Second", "tenantId": "tenant-2", "owner": "ignored" },
                { "subscriptionId": "00000000-0000-0000-0000-00000000000a", "displayName": "First", "tenantId": "tenant-1" }
              ]
            }
            """);

        var result = TopologyFile.Load(path);

        Assert.Equal(
            new[] { ("00000000-0000-0000-0000-00000000000b", "Second", "tenant-2"), ("00000000-0000-0000-0000-00000000000a", "First", "tenant-1") },
            result.Subscriptions.Select(s => (s.SubscriptionId, s.DisplayName, s.TenantId)).ToArray());
        Assert.Equal(new[] { "stflowlogs01" }, result.StorageAccounts);
        Assert.Empty(result.SubscriptionNotes);
    }

    [Fact]
    public void Topology_file_skips_malformed_subscription_entries_with_a_note()
    {
        var path = Path.Combine(TestPaths.NewTempDirectory(), "malformed.json");
        File.WriteAllText(path, """
            {
              "networks": [],
              "subscriptions": [
                42,
                { "displayName": "No id" },
                { "subscriptionId": "  ", "displayName": "Blank id" },
                { "subscriptionId": "00000000-0000-0000-0000-00000000000a" }
              ]
            }
            """);

        var result = TopologyFile.Load(path);

        Assert.Equal(
            new[] { ("00000000-0000-0000-0000-00000000000a", string.Empty, string.Empty) },
            result.Subscriptions.Select(s => (s.SubscriptionId, s.DisplayName, s.TenantId)).ToArray());
        Assert.Equal(
            new[]
            {
                "Topology subscription entry ignored: entry 1 is not an object.",
                "Topology subscription entry ignored: entry 2 has no subscriptionId.",
                "Topology subscription entry ignored: entry 3 has no subscriptionId."
            },
            result.SubscriptionNotes);
    }

    [Fact]
    public void Topology_file_with_non_array_subscriptions_names_the_file_in_the_error()
    {
        var path = Path.Combine(TestPaths.NewTempDirectory(), "not-array.json");
        File.WriteAllText(path, """{"networks":[],"subscriptions":{"subscriptionId":"00000000-0000-0000-0000-00000000000a"}}""");

        var error = Assert.Throws<InvalidDataException>(() => TopologyFile.Load(path));

        Assert.StartsWith("topology file not-array.json: ", error.Message, StringComparison.Ordinal);
        Assert.IsType<System.Text.Json.JsonException>(error.InnerException);
    }

    [Theory]
    [InlineData("443", new[] { 443 })]
    [InlineData("443,8000-8001", new[] { 443, 8000, 8001 })]
    public void Port_specs_parse_to_values(string spec, int[] expected)
    {
        var inRange = expected.Where(p => TrafficFilter.Create(null, null, spec, null, 0).Matches("A", "TCP", p, "a", "b", 0));
        Assert.Equal(expected, inRange);
    }

    [Theory]
    [InlineData("abc")]
    [InlineData("8001-8000")]
    [InlineData("70000")]
    public void Invalid_port_specs_are_rejected(string spec)
    {
        Assert.Throws<ArgumentException>(() => TrafficFilter.ParsePortRanges(spec));
    }

    [Fact]
    public void Relative_and_absolute_window_bounds_parse()
    {
        var now = new DateTimeOffset(2026, 10, 19, 12, 0, 0, TimeSpan.Zero);

        Assert.Equal(now.AddHours(-24), TrafficFilter.ParseWindowBound("24h", now));
        Assert.Equal(now.AddDays(-7), TrafficFilter.ParseWindowBound("7d", now));
        Assert.Equal(now.AddMinutes(-90), TrafficFilter.ParseWindowBound("90m", now));
        Assert.Equal(new DateTimeOffset(2026, 10, 1, 0, 0, 0, TimeSpan.Zero), TrafficFilter.ParseWindowBound("2026-10-01T00:00:00Z", now));
        Assert.Throws<ArgumentException>(() => TrafficFilter.ParseWindowBound("yesterday", now));
    }

    [Fact]
    public void Invalid_action_and_protocol_are_rejected()
    {
        Assert.Throws<ArgumentException>(() => TrafficFilter.Create("maybe", null, null, null, 0));
        Assert.Throws<ArgumentException>(() => TrafficFilter.Create(null, "icmp", null, null, 0));
    }

    private static uint Ipv4Parse(string text)
    {
        Assert.True(Ipv4Lookup.TryParseIpv4(text, out var value));
        return value;
    }
}
