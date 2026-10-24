# Azure VNet Flow Log Parser

A .NET 10 CLI tool that fetches Azure virtual network flow logs from Azure Storage accounts using Entra ID credentials (managed identity, Azure CLI, or service principal) and parses them into denormalized records.

It also **discovers** every storage account per subscription that receives flow logs or contains them, **monitors** whether flow logs are still being written, **diagnoses** why a virtual network, subnet, NIC, or attached network security group is not sending flow logs to storage, and produces an interactive **HTML** or static **PDF** **traffic report** showing flows between storage accounts and for each account individually. The same functionality is available as PowerShell modules in [`powershell/`](powershell/README.md), which reach Azure through the Azure CLI (`az`) rather than the Az PowerShell modules.

## Features

- **MSI Authentication**: Uses Azure Managed Identity for secure, credential-less authentication
- **Multiple Storage Accounts**: Process flow logs from multiple storage accounts using file, environment variable, or Azure Key Vault
- **State Tracking**: Avoid reprocessing flow logs using blob metadata - perfect for CI/CD pipelines running every 5 minutes
- **HTTP Posting**: POST flow logs to HTTP endpoints with Bearer authentication and gzip compression (one record per request)
- **Compatible Format**: Parses flow logs in the same format as [PaloAlto Cortex Azure Functions](https://github.com/PaloAltoNetworks/cortex-azure-functions/tree/master/vnet-flow-logs)
- **Flexible Output**: Supports JSON and JSON Lines (JSONL) output formats, with merged or separate output per account
- **Filtering**: Filter blobs by prefix and limit the number of processed files
- **Local Development**: Supports Azure CLI credentials for local testing
- **Discovery and monitoring**: `discover` lists storage accounts with flow log configurations or flow log blobs, and flags flow logs that are disabled, stale, have no data, or cannot be read
- **Traffic reports**: `report` writes an interactive HTML report (filters, scope graph, sortable tables) or a PDF with the same content, between storage accounts and per account
- **Offline reports**: `report --input` runs on downloaded `PT1H.json` files, optionally with a topology file (see [examples](examples/README.md))

## Prerequisites

- .NET 10.0 SDK or runtime (the project targets `net10.0`, the current LTS release)
- Azure Storage account with VNet flow logs
- One of the following for authentication:
  - Managed Identity (when running in Azure)
  - Azure CLI login (for local development: `az login`)
  - Environment variables with service principal credentials

## Required Azure Permissions

The Managed Identity or user account needs the **Storage Blob Data Reader** role on the storage account(s) containing the flow logs.

```bash
# Assign role to a managed identity
az role assignment create \
  --role "Storage Blob Data Reader" \
  --assignee <managed-identity-principal-id> \
  --scope /subscriptions/<subscription-id>/resourceGroups/<resource-group>/providers/Microsoft.Storage/storageAccounts/<storage-account>
```

When using Azure Key Vault, also grant the **Key Vault Secrets User** role:

```bash
# Assign Key Vault role
az role assignment create \
  --role "Key Vault Secrets User" \
  --assignee <managed-identity-principal-id> \
  --scope /subscriptions/<subscription-id>/resourceGroups/<resource-group>/providers/Microsoft.KeyVault/vaults/<keyvault-name>
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

### Standalone binaries (no runtime needed)

Two builds of the same tool are available. Both produce one executable with the same subcommands (`discover`, `diagnose`, `report`, and the root parse command).

**.NET, native AOT** (ahead-of-time compiled, self-contained; about 29 MB on macOS arm64):

```bash
dotnet publish AzureFlowLogParser.csproj -c Release -r linux-x64   -p:PublishAot=true -o ./dist/linux-x64
dotnet publish AzureFlowLogParser.csproj -c Release -r win-x64     -p:PublishAot=true -o ./dist/win-x64
dotnet publish AzureFlowLogParser.csproj -c Release -r osx-arm64   -p:PublishAot=true -o ./dist/osx-arm64
```

Output: `azure-flowlog-parser` (or `azure-flowlog-parser.exe` on Windows). Native AOT needs `clang` and `zlib1g-dev` on Linux. The .NET build uses invariant globalization, so numbers always print with `1,234`-style separators regardless of the machine locale; this also removes the ICU dependency on Linux.

**Rust** (a single native binary; no .NET or Python runtime, TLS by rustls):

```bash
cd rust
cargo build --release                                  # binary: target/release/azure-flowlog-parser
cargo build --release --target x86_64-unknown-linux-musl   # fully static Linux binary (needs musl-tools)
```

Azure credentials for both builds come from the same chain: an environment service principal (`AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`), then managed identity (App Service or IMDS), then the Azure CLI (`az account get-access-token`).

### Continuous integration and releases

- `.github/workflows/ci.yml` runs on every push and pull request to `main`. It builds and tests the .NET tool, the Rust crate and the PowerShell modules on Ubuntu, Windows and macOS. It also builds the .NET AOT binaries for each platform and a static musl build of the Rust binary. Each job uploads its binaries or Pester results as artifacts.
- `.github/workflows/release.yml` runs on tags matching `v*`. It calls the same build jobs, then creates a GitHub release with every binary and a `SHA256SUMS` file.

## Usage

### Basic Usage

```bash
# Parse all flow logs from a single storage account
azure-flowlog-parser --storage-account mystorageaccount

# Or using dotnet run
dotnet run -- --storage-account mystorageaccount
```

### Multiple Storage Accounts

The tool supports processing flow logs from multiple storage accounts using different sources:

#### 1. From a File

Create a file with storage account names (one per line or comma-separated):

```bash
# storage-accounts.txt
storageaccount1
storageaccount2
storageaccount3
```

Then run:
```bash
dotnet run -- --accounts-file storage-accounts.txt --verbose
```

#### 2. From Environment Variable

Set an environment variable with comma-separated storage account names:

```bash
export AZURE_STORAGE_ACCOUNTS="storageaccount1,storageaccount2,storageaccount3"
dotnet run -- --accounts-env AZURE_STORAGE_ACCOUNTS --verbose
```

#### 3. From Azure Key Vault

Store comma-separated storage account names in an Azure Key Vault secret:

```bash
# Create the secret (secret name must be alphanumeric, value contains comma-separated accounts)
az keyvault secret set \
  --vault-name myvault \
  --name storageaccounts \
  --value "storageaccount1,storageaccount2,storageaccount3"

# Use it in the tool
dotnet run -- \
  --accounts-keyvault https://myvault.vault.azure.net/ \
  --keyvault-secret storageaccounts \
  --verbose
```

**Note**: Key Vault secret names should be alphanumeric (hyphens allowed but not recommended). The secret VALUE contains the comma-separated list of storage account names.

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

#### Storage Account Sources (choose one)

| Option | Alias | Description |
|--------|-------|-------------|
| `--storage-account` | `-s` | Single Azure storage account name |
| `--accounts-file` | `-af` | Path to file containing storage account names |
| `--accounts-env` | `-ae` | Environment variable name with storage accounts |
| `--accounts-keyvault` | `-akv` | Azure Key Vault URL |
| `--keyvault-secret` | `-kvs` | Key Vault secret name (required with --accounts-keyvault) |

#### General Options

| Option | Alias | Description | Default |
|--------|-------|-------------|---------|
| `--container` | `-c` | Container name | `insights-logs-flowlogflowevent` |
| `--prefix` | `-p` | Filter blobs by prefix | - |
| `--output` | `-o` | Output file path (stdout if not specified) | - |
| `--format` | `-f` | Output format: `json` or `jsonl` | `json` |
| `--limit` | `-l` | Limit number of blobs per storage account | - |
| `--merge-output` | `-m` | Merge results from all accounts | `true` |
| `--verbose` | `-v` | Enable verbose output | `false` |
| `--list-only` | - | Only list available blobs | `false` |

#### HTTP Posting Options

| Option | Alias | Description | Default |
|--------|-------|-------------|---------|
| `--http-endpoint` | `-he` | HTTP endpoint URL to POST flow logs | - |
| `--http-token` | `-ht` | Bearer token for authentication | - |
| `--http-keyvault` | `-hkv` | Azure Key Vault URL for HTTP credentials | - |
| `--http-endpoint-secret` | `-hes` | Key Vault secret name for endpoint | `cortexendpoint` |
| `--http-token-secret` | `-hts` | Key Vault secret name for token | `cortextoken` |
| `--http-compression` | `-hc` | Enable gzip compression | `true` |
| `--http-timeout` | - | HTTP request timeout (seconds) | `300` |
| `--http-test` | - | Test endpoint connectivity only | `false` |

### Examples

#### Single Storage Account

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

#### Multiple Storage Accounts

```bash
# Process multiple accounts from a file and merge results
dotnet run -- --accounts-file storage-accounts.txt --output merged-results.json

# Process multiple accounts and create separate output files
dotnet run -- --accounts-file storage-accounts.txt --merge-output false --output results.json
# This creates: results_storageaccount1.json, results_storageaccount2.json, etc.

# Use environment variable with limit per account
dotnet run -- --accounts-env AZURE_STORAGE_ACCOUNTS --limit 10 --verbose

# Use Key Vault with filtering
dotnet run -- \
  --accounts-keyvault https://myvault.vault.azure.net/ \
  --keyvault-secret storageaccounts \
  --prefix "resourceId=/SUBSCRIPTIONS/abc123" \
  --output filtered-results.json \
  --verbose

# List blobs from multiple accounts
dotnet run -- --accounts-file storage-accounts.txt --list-only
```

#### HTTP Posting

```bash
# Post flow logs to an HTTP endpoint
dotnet run -- \
  --storage-account mystorageaccount \
  --http-endpoint https://api.example.com/flowlogs \
  --verbose

# Post with Bearer token authentication
dotnet run -- \
  --storage-account mystorageaccount \
  --http-endpoint https://api.example.com/flowlogs \
  --http-token "your-bearer-token" \
  --verbose

# Post without compression (useful for debugging)
dotnet run -- \
  --storage-account mystorageaccount \
  --http-endpoint https://api.example.com/flowlogs \
  --http-compression false \
  --verbose

# Post from multiple accounts to endpoint
dotnet run -- \
  --accounts-file storage-accounts.txt \
  --http-endpoint https://api.example.com/flowlogs \
  --http-token "your-bearer-token" \
  --verbose

# Post to endpoint AND save to file
dotnet run -- \
  --storage-account mystorageaccount \
  --http-endpoint https://api.example.com/flowlogs \
  --output backup.json \
  --verbose

# Test endpoint connectivity
dotnet run -- \
  --http-endpoint https://api.example.com/flowlogs \
  --http-token "your-bearer-token" \
  --http-test

# Load HTTP endpoint and token from Key Vault (using default secret names)
dotnet run -- \
  --storage-account mystorageaccount \
  --http-keyvault https://myvault.vault.azure.net/ \
  --verbose

# Load from Key Vault with custom secret names
dotnet run -- \
  --storage-account mystorageaccount \
  --http-keyvault https://myvault.vault.azure.net/ \
  --http-endpoint-secret myendpoint \
  --http-token-secret mytoken \
  --verbose

# Override endpoint from Key Vault but use CLI token
dotnet run -- \
  --storage-account mystorageaccount \
  --http-keyvault https://myvault.vault.azure.net/ \
  --http-token "override-token" \
  --verbose
```

## Discovery and Traffic Reports

### Discover storage accounts and check flow log health

```bash
# Every enabled subscription the identity can read
azure-flowlog-parser discover

# Specific subscriptions, JSON for pipelines, flag flow logs older than 3 hours
azure-flowlog-parser discover --subscription <sub-id> --subscription <sub-id-2> \
  --stale-hours 3 --output-format json -o inventory.json

# Also list accounts that have neither flow log configurations nor flow log blobs
azure-flowlog-parser discover --all
```

A storage account is listed when it is the destination of a flow log configuration (network watcher flow log) or its `insights-logs-flowlogflowevent` container holds flow log blobs. Each flow log configuration is checked against the blobs it writes (today and yesterday, per target):

| Status | Meaning |
|--------|---------|
| `Healthy` | Newest blob is within `--stale-hours` |
| `Stale` | Blobs exist but the newest is older than `--stale-hours` |
| `NoData` | No blobs in the last two days |
| `Disabled` | Flow log configuration is disabled (not read) |
| `Unmanaged` | Account has flow log blobs but no configuration (shows the source resource it found) |
| `AccessDenied` | Identity cannot list the container or blobs (needs `Storage Blob Data Reader`) |
| `Error` | Any other failure, with the message |

An account's status is the worst status among its configurations. Accounts whose container cannot be read (`AccessDenied`) are listed even without a proven flow log, so the gap is visible; `--all` adds accounts with neither flow logs nor blobs.

### Diagnose sources that are not sending flow logs

```bash
# Every virtual network, subnet, NIC, and attached NSG that is not writing to storage
azure-flowlog-parser diagnose

# JSON, one subscription, flag blobs older than 6 hours
azure-flowlog-parser diagnose --subscription <sub-id> --stale-hours 6 --output-format json -o gaps.json

# Hide gaps for other storage accounts. Sources with no flow log are still listed.
azure-flowlog-parser diagnose --storage-account stflowlogs01
```

The sweep uses the same Azure reads as `discover` (Reader on the subscriptions, plus Storage Blob Data Reader to list blobs). A source is left off the list when a delivering flow log covers it. Delivering means the flow log is enabled, its status is `Healthy`, and a blob timestamp was found. An enabled flow log on a NIC wins over its subnet, which wins over its virtual network. The command exits 0 when the sweep finishes, including when it finds gaps.

| Reason | Meaning |
|--------|---------|
| `NotConfigured` | No flow log targets this source or a covering parent |
| `NetworkWatcherMissing` | No Network Watcher in the source's region |
| `Disabled` | The flow log that would cover this source is disabled |
| `ProvisioningFailed` | Provisioning state is set and is not `Succeeded` or `Updating` |
| `TargetMissing` | The flow log's target is not in the scanned subscriptions |
| `StorageMissing` | The destination storage account is not in the scanned subscriptions |
| `RegionMismatch` | The storage account is in a different region from the source |
| `UnsupportedStorage` | Premium SKU, `FileStorage`, or `BlockBlobStorage` |
| `StorageFirewall` | Trusted Azure services are off, and public access is disabled or the default action is Deny |
| `SharedKeyDisabled` | Shared key access is off and the flow log has no managed identity |
| `AccessDenied` | Blobs could not be listed |
| `Error` | The health check failed |
| `NoData` | Enabled, and the account can accept logs, but no blobs in the last two days |
| `Stale` | Newest blob is older than `--stale-hours` |

`NoData` also says that rotating the storage account keys stops flow logs until the flow log is disabled and enabled again. That is a hint, not proof the keys were rotated. The blob check reads `insights-logs-flowlogflowevent`. NSG flow logs are stored in `insights-logs-networksecuritygroupflowevent`, so an empty result does not prove that container is empty. NSG flow logs retire on 30 Sep 2027 and new ones cannot be created. A storage account with hierarchical namespace is not treated as a blocker. A firewall that allows trusted Microsoft services is not treated as a blocker.

### Generate a traffic report

```bash
# Interactive HTML for the last 24 hours across all discovered accounts
azure-flowlog-parser report --report html

# Static PDF, denied traffic only, for one subscription
azure-flowlog-parser report --report pdf --action deny --subscription <sub-id> -o denied.pdf

# SQL traffic (destination port 1433, TCP) for an explicit window
azure-flowlog-parser report --since 2026-10-18T10:00:00Z --until 2026-10-18T13:00:00Z \
  --port 1433 --protocol tcp --report html
```

| Option | Default | Description |
|--------|---------|-------------|
| `--since` | `24h` (Azure); observed range (`--input`) | Window start: `90m`, `24h`, `7d` or ISO-8601 UTC |
| `--until` | now (Azure); observed range (`--input`) | Window end, same formats |
| `--action` | `all` | `all`, `allow` or `deny` |
| `--protocol` | `all` | `all`, `tcp` or `udp` |
| `--port` | any | Destination ports, e.g. `443,8000-8100` |
| `--ip` | any | Keep observations whose source or destination contains this text |
| `--min-bytes` | `0` | Keep observations with at least this many bytes |
| `--report` | `html` | `html` (interactive) or `pdf` (static) |
| `--input` | - | Offline: local `PT1H.json` files or folders instead of Azure |
| `--topology` | - | Offline only: JSON mapping prefixes to storage accounts or VNets |

**How flows are attributed.** Each flow tuple's source and destination IPv4 address is matched (longest prefix) against NIC private addresses with a flow-logged NSG, then subnets with a flow-logged NSG, then VNet address spaces. A VNet with flow logs belongs to its storage account. A VNet without flow logs appears as `vnet:<rg>/<name>`. Public addresses that match nothing are `Internet`; private ones are `Unmapped`.

**How observations are counted.** Each NSG or VNet flow tuple is one observation. A flow that crosses two logged networks is observed at both. To avoid counting it twice, an *allowed inbound* observation whose source sits in a storage account that is read in the same run is skipped (its outbound twin is counted at the source). Denied inbound observations are kept, because they are a separate decision. The number skipped appears in the report summary.

**What the report contains.** Summary metrics; flow log monitoring (accounts and configurations with status and age); traffic between scopes; an individual view per storage account (inbound and outbound flows, denied counts, top peers and destination ports); top flow rows; notes on the counting and mapping rules. The HTML version adds a scope graph and filters that work in the browser without a server: focus scope, action, protocol, destination ports, IP text and minimum bytes. The HTML file is self-contained and makes no network requests.

Limits: the report embeds at most 20,000 aggregated rows (totals still cover every observation), and filters apply per observation on the command line and per aggregated row in the HTML.

### Examples

Sample reports generated from the offline data in [`examples/`](examples/README.md) are committed for both this tool and the PowerShell modules.

## Rust port: differences from the .NET tool

The Rust binary implements the same report, discovery, diagnose and parse behaviour. Offline reports (`--input`) produce payloads identical to the .NET tool on the committed examples. These differences remain:

- **Short options:** the .NET multi-letter aliases (`-af`, `-ae`, `-akv`, `-kvs`, `-he`, `-ht`, `-hc`, `-hkv`, `-hes`, `-hts`, `-st`, `-fr`) are not available; use the long names. Single-letter shorts (`-s`, `-c`, `-p`, `-o`, `-f`, `-l`, `-v`, `-m`) are the same.
- **JSON keys:** case-sensitive in Rust. The .NET reader accepts any casing for flow log blobs and topology files. Azure writes the standard casing.
- **IPv4:** dotted-quad only. The .NET reader also accepts shorthand such as `10.1`.
- **Window times:** absolute `--since`/`--until` accept RFC 3339 and common naive forms. The .NET parser accepts more formats.
- **Offline `--topology` without `--input`:** both tools warn and then use Azure. The .NET tool prints a multi-line credential-chain error on authentication failure; the Rust tool prints one line.
- **Parse output:** a non-merge run writes `<name>_<account><ext>` into the directory of the requested output file. The .NET tool wrote it into the working directory.
- **Verbose errors:** show the error chain, not a stack trace. Logged HTTP endpoints show scheme, host and path only.
- **Verification status:** offline reports are verified against the .NET tool. The Azure paths (discovery, reads, Key Vault, HTTP posting) are unit-tested against documented REST shapes but have not yet been run against a live subscription.

## State Tracking (CI/CD Pipeline Support)

The tool supports state tracking using Azure Blob metadata to avoid reprocessing the same flow logs. This is particularly useful for CI/CD pipelines that run on a schedule (e.g., every 5 minutes).

### Features

- **Blob Metadata Storage**: Processing state is stored directly in Azure Blob metadata (no external state file needed)
- **Change Detection**: Only processes blobs that have been modified since last processing
- **CI/CD Friendly**: Perfect for scheduled pipeline execution
- **No State File Management**: No need to manage separate state files or databases
- **Force Reprocessing**: Option to ignore state and reprocess all blobs

### State Tracking Options

| Option | Alias | Description | Default |
|--------|-------|-------------|---------|
| `--enable-state-tracking` | `-st` | Enable state tracking using blob metadata | `true` |
| `--force-reprocess` | `-fr` | Force reprocessing, ignoring metadata state | `false` |

### How It Works

When state tracking is enabled:

1. **First Run**: All blobs are processed normally
2. **Metadata Written**: After successful processing, the tool writes metadata to each blob:
   - `lastProcessed`: Timestamp when the blob was processed
   - `processedBlobLastModified`: The blob's LastModified timestamp at processing time
   - `recordCount`: Number of records parsed from the blob
   - `processedBy`: Tool identifier ("AzureFlowLogParser")
3. **Subsequent Runs**: The tool checks each blob's metadata and only processes blobs that:
   - Have no metadata (never processed)
   - Have been modified since last processing (LastModified > processedBlobLastModified)
4. **Skip Processed Blobs**: Blobs that haven't changed are automatically skipped

### Required Permissions

To use state tracking, the Managed Identity needs **Storage Blob Data Contributor** role (not just Reader) to write metadata:

```bash
az role assignment create \
  --role "Storage Blob Data Contributor" \
  --assignee <managed-identity-principal-id> \
  --scope /subscriptions/<subscription-id>/resourceGroups/<resource-group>/providers/Microsoft.Storage/storageAccounts/<storage-account>
```

### Examples

#### Basic State Tracking

```bash
# First run - processes all blobs and writes metadata (state tracking is enabled by default)
dotnet run -- \
  --storage-account mystorageaccount \
  --verbose

# Second run - only processes new or modified blobs
dotnet run -- \
  --storage-account mystorageaccount \
  --verbose

# Disable state tracking if you want to process all blobs every time
dotnet run -- \
  --storage-account mystorageaccount \
  --enable-state-tracking false \
  --verbose
```

#### CI/CD Pipeline Usage

Perfect for scheduled pipelines (e.g., Azure DevOps, GitHub Actions):

```bash
# Run every 5 minutes - only processes new/changed flow logs (state tracking enabled by default)
dotnet run -- \
  --storage-account mystorageaccount \
  --http-endpoint https://api.example.com/flowlogs \
  --http-keyvault https://myvault.vault.azure.net/ \
  --verbose
```

#### Force Reprocessing

```bash
# Ignore metadata and reprocess everything (still writes new metadata)
dotnet run -- \
  --storage-account mystorageaccount \
  --force-reprocess \
  --verbose
```

#### Multiple Storage Accounts with State Tracking

```bash
# State tracking works across multiple storage accounts (enabled by default)
dotnet run -- \
  --accounts-file storage-accounts.txt \
  --http-endpoint https://api.example.com/flowlogs \
  --verbose
```

### Verbose Output

When running with `--verbose` (state tracking is enabled by default):

```
State tracking enabled: Using blob metadata to track processing
Found 100 blob(s)
Skipped 95 blob(s) based on metadata state
Processing: resourceId=.../PT1H.json
  Parsed 1250 flow record(s)
  Marked as processed in blob metadata
```

### Benefits for CI/CD

1. **Efficiency**: Only processes new or changed flow logs, reducing processing time and costs
2. **Idempotent**: Safe to run multiple times - won't reprocess the same data
3. **Reliable**: State is stored with the blob itself, no external state management needed
4. **Simple**: No cleanup or state file rotation required
5. **Scalable**: Works across multiple storage accounts without state conflicts

### Notes

- **State tracking is enabled by default** - perfect for CI/CD pipelines
- To disable state tracking, use `--enable-state-tracking false`
- State tracking requires write permissions to blob metadata (Storage Blob Data Contributor role)
- Blob metadata is independent of blob content and doesn't affect the flow log data
- Force reprocessing (`--force-reprocess`) still writes metadata after processing
- State tracking has no effect when `--list-only` is used

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

## Multiple Storage Accounts Features

### Output Modes

When processing multiple storage accounts, you can choose between two output modes:

1. **Merged Output** (default, `--merge-output true`):
   - All flow records from all storage accounts are combined into a single output
   - Perfect for centralized logging and analysis

2. **Separate Output** (`--merge-output false`):
   - Each storage account gets its own output file
   - Files are named with the pattern: `{basename}_{storageaccount}{extension}`
   - Example: `results.json` → `results_account1.json`, `results_account2.json`, etc.

### Storage Account Name Validation

The tool automatically validates storage account names according to Azure rules:
- Must be 3-24 characters long
- Only lowercase letters and numbers allowed
- Invalid names are skipped with a warning

### Error Handling

- If one storage account fails, processing continues with remaining accounts
- Errors are logged to stderr while results go to stdout or file
- Use `--verbose` to see detailed error information

## HTTP Posting Features

The tool can POST denormalized flow logs to an HTTP endpoint, matching the pattern from the [PaloAlto Cortex Azure Functions reference](https://github.com/PaloAltoNetworks/cortex-azure-functions/tree/master/vnet-flow-logs).

### Features

- **Individual Record Posting**: Each flow log record is POSTed individually to the endpoint
- **Gzip Compression**: Reduces payload size by ~80% (enabled by default)
- **Bearer Token Auth**: Supports Bearer token authentication for secure endpoints
- **Automatic Retry with Polly**: Exponential backoff retry policy for transient failures (5xx errors, timeouts, rate limits)
- **Dual Output**: Can post to HTTP endpoint AND save to file simultaneously
- **Error Resilience**: Failed records are logged but don't stop processing of remaining records
- **Connectivity Testing**: Test endpoint connectivity before processing logs

### HTTP Request Format

The tool POSTs individual denormalized flow records (one record per request):

```http
POST /endpoint HTTP/1.1
Host: api.example.com
Content-Type: application/json
Content-Encoding: gzip
Authorization: Bearer your-token-here

{ "time": "...", "category": "...", "sourceAddress": "...", ... }
```

### Individual Record Processing

Each flow log record is posted individually to the endpoint:
- Records are processed sequentially (one HTTP POST per record)
- Each record is compressed independently with gzip
- Failed records don't prevent processing of subsequent records
- Detailed progress tracking in verbose mode shows `[X/Total]` for each record

### Compression

Gzip compression is enabled by default and significantly reduces payload size:
- Typical compression ratio: 75-85%
- Example: 5MB uncompressed → ~1MB compressed
- Disable with `--http-compression false` for debugging

### Retry Policy

The HTTP client uses [Polly](https://github.com/App-vNext/Polly) for automatic retry with exponential backoff:

**Configuration:**
- Default retry attempts: 3
- Initial delay: 1 second
- Backoff strategy: Exponential with jitter
- Retries on:
  - HTTP 5xx (Server errors)
  - HTTP 408 (Request Timeout)
  - HTTP 429 (Too Many Requests / Rate Limiting)
  - Network errors (HttpRequestException)
  - Timeouts (TaskCanceledException)

**Exponential Backoff:**
- Attempt 1: ~1 second delay
- Attempt 2: ~2 seconds delay
- Attempt 3: ~4 seconds delay
- Jitter is added to prevent thundering herd problems

**Verbose Output:**
When running with `--verbose`, you'll see detailed retry information:
```
HTTP client configured with 3 retry attempts using exponential backoff
  Retry attempt 1 after delay of 1.2s
    Reason: Status=503 ServiceUnavailable, Exception=N/A
  Retry attempt 2 after delay of 2.4s
    Reason: Status=503 ServiceUnavailable, Exception=N/A
```

This ensures reliable delivery even when endpoints experience temporary issues or rate limiting.

### Key Vault Configuration

Store HTTP endpoint URL and Bearer token securely in Azure Key Vault:

**Setup:**
```bash
# Create secrets with default names (recommended for Cortex)
az keyvault secret set \
  --vault-name myvault \
  --name cortexendpoint \
  --value "https://api.example.com/flowlogs"

az keyvault secret set \
  --vault-name myvault \
  --name cortextoken \
  --value "your-bearer-token"
```

**Usage:**
```bash
# Tool will automatically load both endpoint and token
dotnet run -- \
  --storage-account mystorageaccount \
  --http-keyvault https://myvault.vault.azure.net/ \
  --verbose
```

**Custom Secret Names:**
If you use different secret names, specify them:
```bash
dotnet run -- \
  --storage-account mystorageaccount \
  --http-keyvault https://myvault.vault.azure.net/ \
  --http-endpoint-secret myendpoint \
  --http-token-secret mytoken \
  --verbose
```

**Priority:**
- CLI arguments (`--http-endpoint`, `--http-token`) take precedence over Key Vault values
- Key Vault values are only loaded if CLI arguments are not provided
- This allows overriding specific values while keeping others from Key Vault

### Use Cases

1. **Forward to SIEM**: Post flow logs to your SIEM platform's ingestion endpoint
2. **Custom Analytics**: Send to custom analytics platform for processing
3. **Data Lake**: Forward to data lake ingestion API
4. **Monitoring**: Send to monitoring/alerting systems

## Dependencies

- `Azure.Identity` 1.21 - Entra ID authentication (managed identity, Azure CLI, service principal)
- `Azure.ResourceManager` 1.14, `Azure.ResourceManager.Network` 1.17, `Azure.ResourceManager.Storage` 1.8, `Azure.ResourceManager.Resources` 1.12 - subscriptions, storage accounts, network watchers and flow logs, VNets, NICs
- `Azure.Security.KeyVault.Secrets` 4.11 - Azure Key Vault secret retrieval
- `Azure.Storage.Blobs` 12.30 - Azure Blob Storage client
- `Polly` 8.8 - Resilience and transient-fault-handling with retry policies
- `System.CommandLine` 2.0.13 - CLI argument parsing

The PDF writer is built into the project (no PDF library dependency). The HTML template is embedded from `shared/report-template.html`.

Tests (in `tests/AzureFlowLogParser.Tests`, not shipped) use xUnit v3 and the Microsoft Testing Platform. Run them with `dotnet test`; the repository's `global.json` selects the Microsoft Testing Platform runner.

## License

MIT

## References

- [Azure VNet Flow Logs Documentation](https://docs.microsoft.com/azure/network-watcher/network-watcher-nsg-flow-logging-overview)
- [PaloAlto Cortex Azure Functions](https://github.com/PaloAltoNetworks/cortex-azure-functions/tree/master/vnet-flow-logs)
- [Azure Managed Identity](https://docs.microsoft.com/azure/active-directory/managed-identities-azure-resources/)
