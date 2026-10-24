# Azure flow log PowerShell modules

PowerShell 7.6 port of the `report`, `discover`, and `diagnose` commands. Two modules:

- `AzureFlowLogParser`: discovery, diagnosis, health and aggregation. Exports `Get-FlowLogInventory`, `Get-FlowLogDiagnosis`, and `Get-FlowLogTraffic`.
- `AzureFlowLogReport`: renders the traffic model as HTML or PDF. Exports `Export-FlowLogReport`.

The shared rules (parsing, topology, dedupe, health, report contents) are in the spec that both the .NET CLI and these modules follow.

## Install

Azure mode calls the Azure CLI (`az`), not the Az PowerShell modules. Install `az` (tested with 2.73.0) and make sure it is on `PATH`. Then sign in:

```powershell
az login   # Entra ID sign-in; storage reads use your signed-in identity (RBAC on each account, --auth-mode login)
```

Offline `-InputPath` mode needs none of this.

Import from the repository:

```powershell
Import-Module ./powershell/AzureFlowLogParser
Import-Module ./powershell/AzureFlowLogReport
```

Nothing is checked on import. The first Azure call fails with a clear message if `az` is not on `PATH`, so the offline examples work without it.

## Usage

### Inventory (monitoring only)

```powershell
# Every enabled subscription visible to you
Get-FlowLogInventory

# Specific subscriptions and accounts, 6h staleness threshold, include accounts without flow logs
Get-FlowLogInventory -SubscriptionId 00000000-0000-0000-0000-000000000000 `
    -StorageAccountName stflowlogs01 -StaleAfterHours 6 -IncludeAll |
    ConvertTo-Json -Depth 12
```

Returns `schema`, `generatedUtc`, `source`, `subscriptions`, `accounts`, `flowLogs` and `notes`.

### Diagnosis

```powershell
# Virtual networks, subnets, NICs, and attached NSGs that are not sending flow logs, and why
Get-FlowLogDiagnosis

# One subscription, JSON, 6h staleness. -StorageAccountName hides other accounts; unconfigured sources stay.
Get-FlowLogDiagnosis -SubscriptionId 00000000-0000-0000-0000-000000000000 -StaleAfterHours 6 |
    ConvertTo-Json -Depth 6
```

Same Azure reads and permissions as `Get-FlowLogInventory`. Reason codes match `azure-flowlog-parser diagnose` (`NotConfigured`, `NetworkWatcherMissing`, `Disabled`, `ProvisioningFailed`, `TargetMissing`, `StorageMissing`, `RegionMismatch`, `UnsupportedStorage`, `StorageFirewall`, `SharedKeyDisabled`, `AccessDenied`, `Error`, `NoData`, `Stale`). The blob check reads `insights-logs-flowlogflowevent`. NSG flow logs live in `insights-logs-networksecuritygroupflowevent`, so an empty result does not prove that container is empty. A `NoData` message mentions storage key rotation as a hint, not as proof.

### Traffic report model

```powershell
# Last 24 hours across all visible subscriptions
$model = Get-FlowLogTraffic

# Denied traffic to SSH and RDP over 7 days, at least 1 KB per observation
$model = Get-FlowLogTraffic -Since 7d -Action Deny -Port '22,3389' -MinBytes 1024 `
    -SubscriptionId 00000000-0000-0000-0000-000000000000
```

Filters apply per observation: `-Action All|Allow|Deny`, `-Protocol All|TCP|UDP`, `-Port '443,8000-8100'` (destination port only), `-IpFilter <text>` (case-insensitive; matches source or destination address), `-MinBytes` (bytes of that tuple). `-Since` takes `<n>h`, `<n>d`, `<n>m` or an ISO-8601 UTC datetime. `-Until` defaults to now.

### Offline (local files, no Azure)

```powershell
Get-FlowLogTraffic -InputPath ../sample-flowlog.json | Export-FlowLogReport -Format Html -Path ./flowlog-report.html
```

`-InputPath` takes `PT1H.json` files or directories (searched recursively). Without a topology file every flow maps to `internet` (public IPv4) or `unmapped`. The time window is not applied unless you pass `-Since` or `-Until`; the report window is then the observed range.

To map addresses to storage accounts and VNets offline, pass a topology file:

```powershell
Get-FlowLogTraffic -InputPath ./logs -TopologyPath ./topology.json | Export-FlowLogReport -Format Pdf -Path ./offline.pdf
```

```json
{
  "networks": [
    { "name": "stflowlogs01", "kind": "storage", "prefixes": ["10.10.0.0/16", "10.10.5.4/32"] },
    { "name": "vnet-spoke", "kind": "vnet", "prefixes": ["10.10.5.0/24"] }
  ],
  "subscriptions": [
    { "subscriptionId": "00000000-0000-0000-0000-000000000001", "displayName": "Example Production", "tenantId": "11111111-2222-3333-4444-555555555555" }
  ]
}
```

Addresses map by longest IPv4 prefix, as in Azure mode. A `storage` network becomes a storage scope named after the account, and its name also seeds the dedupe read set, so duplicate inbound records are dropped. A `vnet` network becomes scope `vnet:<name>`. Non-IPv4 prefixes are skipped and counted in the topology note. `-TopologyPath` is ignored, with a warning, unless `-InputPath` is also given.

The optional `subscriptions` array fills the report's Tenant and Subscriptions rows, in file order. Each entry needs a `subscriptionId`; `displayName` and `tenantId` are optional. An entry that is not an object or has no subscription id is skipped with a topology note. Other keys are ignored. Without the array, those rows read "not available (offline input)".

### Report

```powershell
Get-FlowLogTraffic -Since 24h | Export-FlowLogReport -Format Pdf -Path ./flowlog-24h.pdf
Export-FlowLogReport -Model $model -Format Html -Path ./flowlog-7d.html
```

`Export-FlowLogReport` returns a `FileInfo`. HTML embeds the model as JSON in the template from `shared/report-template.html`; the module copy in `AzureFlowLogReport/Templates/` must stay byte-identical to it (the test suite checks this). PDF is a hand-written PDF 1.4 writer with Helvetica and no third-party dependencies.

### Records and blob state (`Get-FlowLogRecord`)

`Get-FlowLogRecord` is the counterpart of the .NET `parse` command's storage reads. It writes one denormalized object per flow tuple, with the .NET field names (`time`, `nsgRuleName`, `startTime`, `sourceAddress`, ... and `flowState`, `packetsStoD`, `bytesStoD`, `packetsDtoS`, `bytesDtoS` for version 2 blobs).

```powershell
Get-FlowLogRecord -StorageAccountName stflowlogs01 -Limit 5 -OutputPath ./records.json
Get-FlowLogRecord -StorageAccountName stflowlogs01, stflowlogs02 -Format Jsonl -OutputPath ./records.jsonl
Get-FlowLogRecord -StorageAccountName stflowlogs01 -ListOnly
```

State tracking is on by default. For each blob the function reads its metadata and processes the blob unless the metadata holds `lastProcessed` and `processedBlobLastModified`, the timestamp parses, and the blob's LastModified is not newer than it. Missing or unreadable metadata means process. After a successful parse the blob's metadata is replaced with four pairs: `lastProcessed` (UTC now, `o` format), `processedBlobLastModified` (the blob's LastModified), `recordCount` and `processedBy=AzureFlowLogParser`. `-Limit` applies after that filter. `-ForceReprocess` skips the metadata reads (and still writes the metadata). A failed download, parse or metadata write warns and moves on to the next blob.

- `-ListOnly` lists the blob names without the state filter, as the .NET `parse` command does. It downloads and writes nothing.
- With `-OutputPath`, every account's records go to one file. The .NET command writes one file per account.
- Empty results still write the file (`[]` or an empty file). The .NET command writes nothing in that case.
- Blob metadata is written with `az storage blob metadata update`, so the signed-in identity needs write access to the blobs (for example Storage Blob Data Contributor).
- HTTP posting (`parse --http-endpoint`) is not ported.

## Parity with the .NET CLI

| .NET (`azure-flowlog-parser`) | PowerShell |
|---|---|
| `discover --subscription` | `Get-FlowLogInventory -SubscriptionId` |
| `diagnose` | `Get-FlowLogDiagnosis` |
| `diagnose --storage-account` | `Get-FlowLogDiagnosis -StorageAccountName` |
| `diagnose --stale-hours` | `Get-FlowLogDiagnosis -StaleAfterHours` |
| `discover --storage-account` | `-StorageAccountName` |
| `discover --stale-hours` | `-StaleAfterHours` |
| `discover --all` | `-IncludeAll` |
| `discover --output-format json` | `\| ConvertTo-Json -Depth 12` (objects are the output) |
| `report --subscription`, `--storage-account` | `Get-FlowLogTraffic -SubscriptionId`, `-StorageAccountName` |
| `report --input` | `Get-FlowLogTraffic -InputPath` |
| `report --since`, `--until` | `-Since`, `-Until` |
| `report --action`, `--protocol`, `--port`, `--ip`, `--min-bytes` | `-Action`, `-Protocol`, `-Port`, `-IpFilter`, `-MinBytes` |
| `report --stale-hours` | `-StaleAfterHours` |
| `report --topology <file>` | `-TopologyPath <file>` (with `-InputPath` only) |
| `report --report html\|pdf` | `Export-FlowLogReport -Format Html\|Pdf` |
| `-o <file>` | `-Path <file>` (required; no default file name) |
| `parse --enable-state-tracking` (on by default here) | `Get-FlowLogRecord` (state tracking is on; `-DisableStateTracking` turns it off) |
| `parse --http-endpoint` and the HTTP posting options | Not ported. `Get-FlowLogRecord` writes records to the pipeline or a file only |

Timestamps in the model are ISO-8601 strings. PowerShell 7.6's `ConvertFrom-Json` converts them to `DateTime` by default, so read saved reports back with `ConvertFrom-Json -DateKind String` if you need the original strings.

## Known limits

- Counts are per flow-log observation (one tuple). Bytes and packets are 0 for version 1 flow logs.
- An observation's time is its tuple start time. If that does not parse, the record's `time` is used. Observations with neither are counted as malformed.
- Dedupe: an inbound allowed observation whose source is a storage-backed scope is skipped when that account is in the read set. The read set is every account the run attempts to read, so a failed read can drop the matching inbound records; a note records the failure.
- Topology only sees NICs, subnets and VNets in the selected subscriptions. Flows from other addresses show as `internet` or `unmapped`.
- Flow-log configs that write to storage in an unselected subscription are not checked and are noted.
- Health uses a 2-day lookback. Reports read only the requested window.
- Accounts with no flow-log configs are listed only if they hold flow-log blobs. An account whose container cannot be read (AccessDenied) is listed by default with that status. `-IncludeAll` also lists accounts with no flow logs (NoFlowLogs).
- Azure mode is covered by mocked tests. `tests/AzureCliOutput.Tests.ps1` checks the parsing against az-shaped fixtures in `tests/fixtures/az/` (cross-checked against the ARM originals in `tests/fixtures/arm/`), and runs the wrapper against a test double named `az`. The az field names and output shapes are unverified against live output: no live Azure call has been made, so run a real `discover` before relying on Azure mode. Multi-tenant access depends on `az login` having a session for each tenant. Blob listings use `--num-results=*`; the `=` form matters because PowerShell expands a bare `*` against the working directory.
- The PDF top-flows table omits the `rule` column to fit the page; the model still carries it.
- The model lists at most 20,000 flow rows (`summary.rowsTruncated`). Summary totals cover every observation. Per-storage-account figures in the report are computed from the listed rows.

## Tests

Pester 5 (5.9.1 tested). Azure is never called: `Invoke-FlowLogAzCli` is mocked, and the wrapper tests use a test double named `az` placed first on `PATH`. A test that reaches the CLI without a mock fails rather than calling `az`.

```powershell
Install-Module Pester -MaximumVersion 5.99.99 -Scope CurrentUser
pwsh -NoProfile -Command "Invoke-Pester powershell/tests -CI"
```

The PDF tests run `qpdf --check` when `qpdf` is on the path and skip it otherwise.
`-CI` also writes `testResults.xml` to the current folder.
