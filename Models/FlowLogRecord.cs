using System.Text.Json.Serialization;

namespace AzureFlowLogParser.Models;

/// <summary>
/// Represents the root structure of Azure VNet flow logs
/// </summary>
public class FlowLogRoot
{
    [JsonPropertyName("records")]
    public List<FlowLogRecord> Records { get; set; } = new();
}

/// <summary>
/// Represents a single flow log record from Azure
/// </summary>
public class FlowLogRecord
{
    [JsonPropertyName("time")]
    public string Time { get; set; } = string.Empty;

    [JsonPropertyName("systemId")]
    public string SystemId { get; set; } = string.Empty;

    [JsonPropertyName("macAddress")]
    public string MacAddress { get; set; } = string.Empty;

    [JsonPropertyName("category")]
    public string Category { get; set; } = string.Empty;

    [JsonPropertyName("resourceId")]
    public string ResourceId { get; set; } = string.Empty;

    [JsonPropertyName("operationName")]
    public string OperationName { get; set; } = string.Empty;

    [JsonPropertyName("properties")]
    public FlowLogProperties? Properties { get; set; }

    [JsonPropertyName("flowRecords")]
    public FlowRecords? FlowRecords { get; set; }
}

public class FlowLogProperties
{
    [JsonPropertyName("Version")]
    public int Version { get; set; }
}

public class FlowRecords
{
    [JsonPropertyName("flows")]
    public List<Flow> Flows { get; set; } = new();
}

public class Flow
{
    [JsonPropertyName("rule")]
    public string Rule { get; set; } = string.Empty;

    [JsonPropertyName("flowGroups")]
    public List<FlowGroup> FlowGroups { get; set; } = new();
}

public class FlowGroup
{
    [JsonPropertyName("mac")]
    public string Mac { get; set; } = string.Empty;

    [JsonPropertyName("flowTuples")]
    public List<string> FlowTuples { get; set; } = new();
}

/// <summary>
/// Represents a denormalized flow log entry (output format)
/// Matches the format from PaloAlto Cortex Azure Functions
/// </summary>
public class DenormalizedFlowRecord
{
    [JsonPropertyName("time")]
    public string Time { get; set; } = string.Empty;

    [JsonPropertyName("category")]
    public string Category { get; set; } = string.Empty;

    [JsonPropertyName("operationName")]
    public string OperationName { get; set; } = string.Empty;

    [JsonPropertyName("resourceId")]
    public string ResourceId { get; set; } = string.Empty;

    [JsonPropertyName("version")]
    public int Version { get; set; }

    [JsonPropertyName("nsgRuleName")]
    public string NsgRuleName { get; set; } = string.Empty;

    [JsonPropertyName("mac")]
    public string Mac { get; set; } = string.Empty;

    [JsonPropertyName("startTime")]
    public string StartTime { get; set; } = string.Empty;

    [JsonPropertyName("sourceAddress")]
    public string SourceAddress { get; set; } = string.Empty;

    [JsonPropertyName("destinationAddress")]
    public string DestinationAddress { get; set; } = string.Empty;

    [JsonPropertyName("sourcePort")]
    public string SourcePort { get; set; } = string.Empty;

    [JsonPropertyName("destinationPort")]
    public string DestinationPort { get; set; } = string.Empty;

    [JsonPropertyName("transportProtocol")]
    public string TransportProtocol { get; set; } = string.Empty;

    [JsonPropertyName("deviceDirection")]
    public string DeviceDirection { get; set; } = string.Empty;

    [JsonPropertyName("deviceAction")]
    public string DeviceAction { get; set; } = string.Empty;

    // Version 2+ fields
    [JsonPropertyName("flowState")]
    public string? FlowState { get; set; }

    [JsonPropertyName("packetsStoD")]
    public string? PacketsStoD { get; set; }

    [JsonPropertyName("bytesStoD")]
    public string? BytesStoD { get; set; }

    [JsonPropertyName("packetsDtoS")]
    public string? PacketsDtoS { get; set; }

    [JsonPropertyName("bytesDtoS")]
    public string? BytesDtoS { get; set; }
}
