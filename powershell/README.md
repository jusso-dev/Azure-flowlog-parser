# Azure flow log PowerShell modules

PowerShell 7.6 port of the `report` and `discover` commands. Two modules:

- `AzureFlowLogParser`: discovery, health and aggregation. Exports `Get-FlowLogInventory` and `Get-FlowLogTraffic`.
- `AzureFlowLogReport`: renders the traffic model as HTML or PDF. Exports `Export-FlowLogReport`.

The shared rules (parsing, topology, dedupe, health, report contents) are in the spec that both the .NET CLI and these modules follow.

## Install

Azure mode needs the Az modules at these minimum versions. Offline `-InputPath` mode needs none of them.

```powershell
Install-Module Az.Accounts -MinimumVersion 5.5.3 -Scope CurrentUser
Install-Module Az.Storage -MinimumVersion 9.7.2 -Scope CurrentUser
Install-Module Az.Network -MinimumVersion 8.3.0 -Scope CurrentUser

Connect-AzAccount   # Entra ID sign-in; storage reads use your connected account (RBAC on each account)
```

Import from the repository:

```powershell
Import-Module ./powershell/AzureFlowLogParser
Import-Module ./powershell/AzureFlowLogReport
```

Each Az module loads on the first Azure call, not on import, so the offline examples work without Az installed.

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
  ]
}
```

Addresses map by longest IPv4 prefix, as in Azure mode. A `storage` network becomes a storage scope named after the account, and its name also seeds the dedupe read set, so duplicate inbound records are dropped. A `vnet` network becomes scope `vnet:<name>`. Non-IPv4 prefixes are skipped and counted in the topology note. `-TopologyPath` is ignored, with a warning, unless `-InputPath` is also given.

### Report

```powershell
Get-FlowLogTraffic -Since 24h | Export-FlowLogReport -Format Pdf -Path ./flowlog-24h.pdf
Export-FlowLogReport -Model $model -Format Html -Path ./flowlog-7d.html
```

`Export-FlowLogReport` returns a `FileInfo`. HTML embeds the model as JSON in the template from `shared/report-template.html`; the module copy in `AzureFlowLogReport/Templates/` must stay byte-identical to it (the test suite checks this). PDF is a hand-written PDF 1.4 writer with Helvetica and no third-party dependencies.

## Parity with the .NET CLI

| .NET (`azure-flowlog-parser`) | PowerShell |
|---|---|
| `discover --subscription` | `Get-FlowLogInventory -SubscriptionId` |
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

Timestamps in the model are ISO-8601 strings. PowerShell 7.6's `ConvertFrom-Json` converts them to `DateTime` by default, so read saved reports back with `ConvertFrom-Json -DateKind String` if you need the original strings.

## Known limits

- Counts are per flow-log observation (one tuple). Bytes and packets are 0 for version 1 flow logs.
- An observation's time is its tuple start time. If that does not parse, the record's `time` is used. Observations with neither are counted as malformed.
- Dedupe: an inbound allowed observation whose source is a storage-backed scope is skipped when that account is in the read set. The read set is every account the run attempts to read, so a failed read can drop the matching inbound records; a note records the failure.
- Topology only sees NICs, subnets and VNets in the selected subscriptions. Flows from other addresses show as `internet` or `unmapped`.
- Flow-log configs that write to storage in an unselected subscription are not checked and are noted.
- Health uses a 2-day lookback. Reports read only the requested window.
- Accounts with no flow-log configs are listed only if they hold flow-log blobs. An account whose container cannot be read (AccessDenied) is listed by default with that status. `-IncludeAll` also lists accounts with no flow logs (NoFlowLogs).
- Azure mode is covered by mocked tests and by `tests/AzureTypes.Tests.ps1`, which runs the Az wrappers' property reads against the real Az types (Az.Accounts 5.5.3, Az.Network 8.3.0, Az.Storage 9.7.2, as checked against the Microsoft Learn reference). That test skips unless those modules are installed. No live subscription has been called, so run a real `discover` before relying on Azure mode.
- The PDF top-flows table omits the `rule` column to fit the page; the model still carries it.
- The model lists at most 20,000 flow rows (`summary.rowsTruncated`). Summary totals cover every observation. Per-storage-account figures in the report are computed from the listed rows.

## Tests

Pester 5 (5.9.1 tested). Azure is never called: the Az wrappers are mocked. `tests/AzureTypes.Tests.ps1` loads the real Az modules when they are installed and skips otherwise.

```powershell
Install-Module Pester -MaximumVersion 5.99.99 -Scope CurrentUser
pwsh -NoProfile -Command "Invoke-Pester powershell/tests -CI"
```

The PDF tests run `qpdf --check` when `qpdf` is on the path and skip it otherwise.
`-CI` also writes `testResults.xml` to the current folder.
