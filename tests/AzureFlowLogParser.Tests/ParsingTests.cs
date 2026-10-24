using System.Text.Json;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services;
using AzureFlowLogParser.Services.Discovery;
using Xunit;

namespace AzureFlowLogParser.Tests;

public class ParsingTests
{
    [Fact]
    public void Sample_file_expands_to_one_row_per_flow_tuple()
    {
        var json = File.ReadAllText(TestPaths.SampleFlowLog);

        var rows = FlowLogParser.ParseFlowLog(json);

        // Two records: 3 + 1 tuples under the first rule group, 1 under the second, 1 in the second record.
        Assert.Equal(6, rows.Count);
    }

    [Fact]
    public void Version2_tuple_fields_are_mapped_by_position()
    {
        var rows = FlowLogParser.ParseFlowLog(File.ReadAllText(TestPaths.SampleFlowLog));

        var first = rows[0];
        Assert.Equal("DefaultRule_AllowInternetOutBound", first.NsgRuleName);
        Assert.Equal("1705318200", first.StartTime);
        Assert.Equal("10.0.1.4", first.SourceAddress);
        Assert.Equal("13.107.42.14", first.DestinationAddress);
        Assert.Equal("52044", first.SourcePort);
        Assert.Equal("443", first.DestinationPort);
        Assert.Equal("T", first.TransportProtocol);
        Assert.Equal("O", first.DeviceDirection);
        Assert.Equal("A", first.DeviceAction);
        Assert.Equal("B", first.FlowState);
        Assert.Equal("5", first.PacketsStoD);
        Assert.Equal("1500", first.BytesStoD);
        Assert.Equal("3", first.PacketsDtoS);
        Assert.Equal("900", first.BytesDtoS);
    }

    [Fact]
    public void Version1_tuples_have_no_counters()
    {
        const string json = """
        {"records":[{"time":"2024-01-15T10:30:00Z","resourceId":"/x","properties":{"Version":1},
          "flowRecords":{"flows":[{"rule":"r","flowGroups":[{"mac":"m","flowTuples":["1705318200,10.0.1.4,10.0.1.5,1,22,T,I,D"]}]}]}}]}
        """;

        var row = Assert.Single(FlowLogParser.ParseFlowLog(json));

        Assert.Equal("D", row.DeviceAction);
        Assert.Null(row.FlowState);
        Assert.Null(row.BytesStoD);
    }

    [Fact]
    public void Blob_names_map_to_targets_and_day_prefixes()
    {
        var target = "/subscriptions/abc/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/nsg-app";

        var prefix = FlowLogPaths.DayPrefix(target, new DateTime(2026, 10, 19));
        var blobName = "resourceId=/SUBSCRIPTIONS/ABC/RESOURCEGROUPS/RG/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-APP/y=2026/m=10/d=19/h=10/m=00/macAddress=00155D123456/PT1H.json";

        Assert.Equal("resourceId=/SUBSCRIPTIONS/ABC/RESOURCEGROUPS/RG/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-APP/y=2026/m=10/d=19/", prefix);
        Assert.StartsWith(prefix, blobName, StringComparison.Ordinal);
        Assert.Equal("/SUBSCRIPTIONS/ABC/RESOURCEGROUPS/RG/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-APP", FlowLogPaths.TargetFromBlobName(blobName));
        Assert.Null(FlowLogPaths.TargetFromBlobName("random/blob.json"));
    }

    [Theory]
    [InlineData("/subscriptions/x/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/n", "NSG")]
    [InlineData("/subscriptions/x/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/v", "VNet")]
    [InlineData("/subscriptions/x/resourceGroups/rg/providers/Microsoft.Network/networkInterfaces/nic", "NIC")]
    [InlineData("/subscriptions/x/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/v/subnets/app", "Subnet")]
    public void Target_kind_is_derived_from_resource_type(string target, string expected)
    {
        Assert.Equal(expected, FlowLogPaths.TargetKind(target));
    }

    [Theory]
    [InlineData("/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-app", "nsg-app", "rg-net")]
    [InlineData("/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/virtualNetworks/vnet-hub", "vnet-hub", "rg-net")]
    [InlineData("/SUBSCRIPTIONS/X/RESOURCEGROUPS/RG-NET/PROVIDERS/MICROSOFT.NETWORK/VIRTUALNETWORKS/VNET-HUB", "VNET-HUB", "RG-NET")]
    [InlineData("/subscriptions/x/providers/Microsoft.Network/virtualNetworks/vnet-hub", "vnet-hub", "")]
    public void Target_name_and_resource_group_come_from_the_arm_id(string target, string expectedName, string expectedGroup)
    {
        Assert.Equal(expectedName, FlowLogPaths.TargetName(target));
        Assert.Equal(expectedGroup, FlowLogPaths.TargetResourceGroup(target));
    }

    [Fact]
    public void Status_worst_picks_most_severe()
    {
        Assert.Equal(FlowStatus.Error, FlowStatus.Worst(new[] { FlowStatus.Healthy, FlowStatus.Error, FlowStatus.Stale }));
        Assert.Equal(FlowStatus.Stale, FlowStatus.Worst(new[] { FlowStatus.Healthy, FlowStatus.Stale, FlowStatus.Disabled }));
        Assert.Equal(FlowStatus.Healthy, FlowStatus.Worst(Array.Empty<string>()));
    }

    [Fact]
    public void Report_model_serializes_to_camel_case_contract()
    {
        var model = new ReportModel
        {
            GeneratedUtc = "2026-10-19T10:00:00Z",
            Summary = new ReportSummary { Flows = 6, RowsTotal = 4 }
        };

        var json = JsonSerializer.Serialize(model, Serialization.AppJsonContext.Default.ReportModel);
        using var doc = JsonDocument.Parse(json);

        Assert.Equal(1, doc.RootElement.GetProperty("schema").GetInt32());
        Assert.Equal("2026-10-19T10:00:00Z", doc.RootElement.GetProperty("generatedUtc").GetString());
        Assert.Equal(6, doc.RootElement.GetProperty("summary").GetProperty("flows").GetInt64());
        Assert.True(doc.RootElement.TryGetProperty("flows", out _));
        Assert.True(doc.RootElement.TryGetProperty("scopes", out _));
    }
}
