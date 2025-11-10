# Azure VNet Flow Log Parser

A .NET 9 CLI tool that fetches Azure virtual network flow logs from Azure Storage accounts using Managed Service Identity (MSI) credentials and parses them into denormalized records.

## Features

- **MSI Authentication**: Uses Azure Managed Identity for secure, credential-less authentication
- **Compatible Format**: Parses flow logs in the same format as [PaloAlto Cortex Azure Functions](https://github.com/PaloAltoNetworks/cortex-azure-functions/tree/master/vnet-flow-logs)
- **Flexible Output**: Supports JSON and JSON Lines (JSONL) output formats
- **Filtering**: Filter blobs by prefix and limit the number of processed files
- **Local Development**: Supports Azure CLI credentials for local testing

## Prerequisites

- .NET 9.0 SDK or runtime
- Azure Storage account with VNet flow logs
- One of the following for authentication:
  - Managed Identity (when running in Azure)
  - Azure CLI login (for local development: `az login`)
  - Environment variables with service principal credentials

## Required Azure Permissions

The Managed Identity or user account needs the **Storage Blob Data Reader** role on the storage account containing the flow logs.

```bash
# Assign role to a managed identity
az role assignment create \
  --role "Storage Blob Data Reader" \
  --assignee <managed-identity-principal-id> \
  --scope /subscriptions/<subscription-id>/resourceGroups/<resource-group>/providers/Microsoft.Storage/storageAccounts/<storage-account>
```

## Installation

### Build from Source

```bash
dotnet build -c Release
dotnet publish -c Release -o ./publish
```

### Run without Building

```bash
dotnet run -- --storage-account <account-name> [options]
```

## Usage

### Basic Usage

```bash
# Parse all flow logs from the default container
azure-flowlog-parser --storage-account mystorageaccount

# Or using dotnet run
dotnet run -- --storage-account mystorageaccount
```

### Advanced Options

```bash
azure-flowlog-parser \
  --storage-account mystorageaccount \
  --container insights-logs-flowlogflowevent \
  --prefix "resourceId=/SUBSCRIPTIONS/xxx" \
  --output output.json \
  --format json \
  --limit 10 \
  --verbose
```

### Options

| Option | Alias | Description | Default |
|--------|-------|-------------|---------|
| `--storage-account` | `-s` | Azure storage account name (required) | - |
| `--container` | `-c` | Container name | `insights-logs-flowlogflowevent` |
| `--prefix` | `-p` | Filter blobs by prefix | - |
| `--output` | `-o` | Output file path (stdout if not specified) | - |
| `--format` | `-f` | Output format: `json` or `jsonl` | `json` |
| `--limit` | `-l` | Limit number of blobs to process | - |
| `--verbose` | `-v` | Enable verbose output | `false` |
| `--list-only` | - | Only list available blobs | `false` |

### Examples

```bash
# List available blobs without processing
dotnet run -- -s mystorageaccount --list-only

# Process only the first 5 blobs and save to file
dotnet run -- -s mystorageaccount --limit 5 --output results.json

# Process blobs with a specific prefix
dotnet run -- -s mystorageaccount --prefix "resourceId=/SUBSCRIPTIONS/abc123"

# Output in JSON Lines format for streaming processing
dotnet run -- -s mystorageaccount --format jsonl --output results.jsonl

# Verbose mode for debugging
dotnet run -- -s mystorageaccount --verbose
```

## Output Format

The tool outputs denormalized flow records matching the format from the [PaloAlto Cortex reference implementation](https://github.com/PaloAltoNetworks/cortex-azure-functions/tree/master/vnet-flow-logs).

### Sample Output

```json
[
  {
    "time": "2024-01-15T10:30:00.000Z",
    "category": "FlowLogFlowEvent",
    "operationName": "FlowLogFlowEvent",
    "resourceId": "/SUBSCRIPTIONS/.../NETWORKWATCHERS/.../FLOWLOGS/...",
    "version": 2,
    "nsgRuleName": "DefaultRule_AllowInternetOutBound",
    "mac": "00155D123456",
    "startTime": "1705318200",
    "sourceAddress": "10.0.1.4",
    "destinationAddress": "13.107.42.14",
    "sourcePort": "52044",
    "destinationPort": "443",
    "transportProtocol": "T",
    "deviceDirection": "O",
    "deviceAction": "A",
    "flowState": "B",
    "packetsStoD": "5",
    "bytesStoD": "1500",
    "packetsDtoS": "3",
    "bytesDtoS": "900"
  }
]
```

### Field Descriptions

| Field | Description |
|-------|-------------|
| `time` | Timestamp of the log event |
| `category` | Log category (typically "FlowLogFlowEvent") |
| `operationName` | Operation name |
| `resourceId` | Azure resource ID |
| `version` | Flow log schema version (1 or 2) |
| `nsgRuleName` | Network Security Group rule name |
| `mac` | MAC address |
| `startTime` | Flow start time (Unix timestamp) |
| `sourceAddress` | Source IP address |
| `destinationAddress` | Destination IP address |
| `sourcePort` | Source port |
| `destinationPort` | Destination port |
| `transportProtocol` | Transport protocol (T=TCP, U=UDP) |
| `deviceDirection` | Direction (I=Inbound, O=Outbound) |
| `deviceAction` | Action (A=Allow, D=Deny) |
| `flowState` | Flow state (Version 2+: B=Begin, C=Continue, E=End) |
| `packetsStoD` | Packets source to destination (Version 2+) |
| `bytesStoD` | Bytes source to destination (Version 2+) |
| `packetsDtoS` | Packets destination to source (Version 2+) |
| `bytesDtoS` | Bytes destination to source (Version 2+) |

## Authentication Methods

### 1. Managed Identity (Production)

When running in Azure (VM, App Service, Container Instance, etc.):
1. Enable system-assigned or user-assigned managed identity
2. Grant the identity **Storage Blob Data Reader** role
3. The tool automatically uses the managed identity

### 2. Azure CLI (Local Development)

For local development:
```bash
az login
dotnet run -- --storage-account mystorageaccount
```

### 3. Environment Variables

Set environment variables for service principal authentication:
```bash
export AZURE_CLIENT_ID="<client-id>"
export AZURE_CLIENT_SECRET="<client-secret>"
export AZURE_TENANT_ID="<tenant-id>"
```

## Flow Log Structure

The tool processes Azure VNet flow logs with the following nested structure:

```
FlowLogRoot
└── records[]
    └── flowRecords
        └── flows[]
            └── flowGroups[]
                └── flowTuples[] (comma-separated strings)
```

Each flow tuple is denormalized into a separate record in the output.

## Troubleshooting

### Authentication Errors

If you see authentication errors:
1. Verify managed identity is enabled
2. Check the identity has the correct role assignment
3. For local dev, ensure `az login` is successful
4. Check the storage account name is correct

### No Blobs Found

- Verify the container name (default: `insights-logs-flowlogflowevent`)
- Check if flow logs are enabled on your VNet
- Use `--list-only` to see available blobs
- Try without `--prefix` to see all blobs

### Permission Errors

Ensure the identity has **Storage Blob Data Reader** role (not just Contributor).

## Dependencies

- `Azure.Identity` - Azure authentication
- `Azure.Storage.Blobs` - Azure Blob Storage client
- `System.CommandLine` - CLI argument parsing
- `System.Text.Json` - JSON serialization

## License

MIT

## References

- [Azure VNet Flow Logs Documentation](https://docs.microsoft.com/azure/network-watcher/network-watcher-nsg-flow-logging-overview)
- [PaloAlto Cortex Azure Functions](https://github.com/PaloAltoNetworks/cortex-azure-functions/tree/master/vnet-flow-logs)
- [Azure Managed Identity](https://docs.microsoft.com/azure/active-directory/managed-identities-azure-resources/)
