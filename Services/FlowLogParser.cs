using System.Text.Json;
using AzureFlowLogParser.Models;

namespace AzureFlowLogParser.Services;

/// <summary>
/// Parses Azure VNet flow logs into denormalized records
/// Matches the format from PaloAlto Cortex Azure Functions
/// </summary>
public class FlowLogParser
{
    /// <summary>
    /// Parses a JSON flow log string and returns denormalized records
    /// </summary>
    public static List<DenormalizedFlowRecord> ParseFlowLog(string jsonContent)
    {
        var options = new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true
        };

        var flowLogRoot = JsonSerializer.Deserialize<FlowLogRoot>(jsonContent, options);

        if (flowLogRoot?.Records == null)
        {
            return new List<DenormalizedFlowRecord>();
        }

        return DenormalizeVnetRecords(flowLogRoot);
    }

    /// <summary>
    /// Denormalizes VNet records by flattening the nested structure
    /// This matches the denormalize_vnet_records function from the reference implementation
    /// </summary>
    private static List<DenormalizedFlowRecord> DenormalizeVnetRecords(FlowLogRoot data)
    {
        var result = new List<DenormalizedFlowRecord>();

        foreach (var record in data.Records)
        {
            if (record.FlowRecords?.Flows == null)
                continue;

            foreach (var outerFlow in record.FlowRecords.Flows)
            {
                foreach (var innerFlow in outerFlow.FlowGroups)
                {
                    foreach (var flowTuple in innerFlow.FlowTuples)
                    {
                        var denormalizedRecord = CreateVnetRecord(record, outerFlow, innerFlow, flowTuple);
                        result.Add(denormalizedRecord);
                    }
                }
            }
        }

        return result;
    }

    /// <summary>
    /// Creates a denormalized VNet record from a flow tuple
    /// Flow tuple format: startTime,sourceAddress,destinationAddress,sourcePort,destinationPort,
    ///                    transportProtocol,deviceDirection,deviceAction[,flowState,packetsStoD,bytesStoD,packetsDtoS,bytesDtoS]
    /// </summary>
    private static DenormalizedFlowRecord CreateVnetRecord(
        FlowLogRecord record,
        Flow outerFlow,
        FlowGroup innerFlow,
        string flowTuple)
    {
        var parts = flowTuple.Split(',');
        var version = record.Properties?.Version ?? 1;

        var denormalized = new DenormalizedFlowRecord
        {
            // Metadata from record
            Time = record.Time,
            Category = record.Category,
            OperationName = record.OperationName,
            ResourceId = record.ResourceId,
            Version = version,

            // Rule and MAC from flow
            NsgRuleName = outerFlow.Rule,
            Mac = innerFlow.Mac,

            // Base fields from flow tuple (indices 0-7)
            StartTime = parts.Length > 0 ? parts[0] : string.Empty,
            SourceAddress = parts.Length > 1 ? parts[1] : string.Empty,
            DestinationAddress = parts.Length > 2 ? parts[2] : string.Empty,
            SourcePort = parts.Length > 3 ? parts[3] : string.Empty,
            DestinationPort = parts.Length > 4 ? parts[4] : string.Empty,
            TransportProtocol = parts.Length > 5 ? parts[5] : string.Empty,
            DeviceDirection = parts.Length > 6 ? parts[6] : string.Empty,
            DeviceAction = parts.Length > 7 ? parts[7] : string.Empty
        };

        // Version 2+ fields (indices 8-12)
        if (version >= 2 && parts.Length > 8)
        {
            denormalized.FlowState = parts[8];

            // Convert empty values to "0" for packet/byte counts
            denormalized.PacketsStoD = parts.Length > 9 ? (string.IsNullOrEmpty(parts[9]) ? "0" : parts[9]) : "0";
            denormalized.BytesStoD = parts.Length > 10 ? (string.IsNullOrEmpty(parts[10]) ? "0" : parts[10]) : "0";
            denormalized.PacketsDtoS = parts.Length > 11 ? (string.IsNullOrEmpty(parts[11]) ? "0" : parts[11]) : "0";
            denormalized.BytesDtoS = parts.Length > 12 ? (string.IsNullOrEmpty(parts[12]) ? "0" : parts[12]) : "0";
        }

        return denormalized;
    }

    /// <summary>
    /// Formats denormalized records as JSON for output
    /// </summary>
    public static string FormatAsJson(List<DenormalizedFlowRecord> records, bool indented = true)
    {
        var options = new JsonSerializerOptions
        {
            WriteIndented = indented,
            DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull
        };

        return JsonSerializer.Serialize(records, options);
    }
}
