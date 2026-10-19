function Get-FlowLogTraffic {
    <#
    .SYNOPSIS
    Reads VNet flow log tuples for a time window and returns the aggregated traffic report model.

    .DESCRIPTION
    Two modes. Azure mode (default) discovers flow logs and reads blobs for the window, mapping each flow
    address to a scope (storage-backed NSG or VNet, VNet without flow logs, internet or unmapped).
    Local mode (-InputPath) reads PT1H.json files or directories offline with no Azure access and no topology.
    The result is the ReportModel (schema 1). Pipe it to Export-FlowLogReport (AzureFlowLogReport module).

    Counting is per flow-log observation (one tuple). Inbound allowed observations whose source is a storage
    scope that was read in this run are skipped as duplicates of the outbound record at the source NSG.

    .PARAMETER SubscriptionId
    Subscription ids to inspect in Azure mode. Defaults to every enabled subscription visible to the identity.

    .PARAMETER StorageAccountName
    Limit Azure mode to these storage accounts.

    .PARAMETER InputPath
    Local PT1H.json files or directories. Cannot be combined with -SubscriptionId or -StorageAccountName.

    .PARAMETER TopologyPath
    Offline topology JSON for -InputPath runs: { "networks": [ { "name", "kind": "storage" | "vnet", "prefixes": [...] } ] }.
    Flows map to scopes by longest IPv4 prefix. Storage networks also seed the dedupe read set. Ignored
    without -InputPath (a warning is written).

    .PARAMETER Since
    Window start: <n>h, <n>d, <n>m (minutes) or an ISO-8601 UTC datetime. Default 24h. In local mode the
    window applies only when -Since or -Until is given.

    .PARAMETER Until
    Window end. Default now (UTC).

    .PARAMETER Action
    All, Allow or Deny. Filters observations by flow action.

    .PARAMETER Protocol
    All, TCP or UDP.

    .PARAMETER Port
    Destination port list and ranges, for example '443,8000-8100'.

    .PARAMETER IpFilter
    Keep observations whose source or destination address contains this text.

    .PARAMETER MinBytes
    Keep observations with at least this many bytes (bytes = bytesStoD + bytesDtoS).

    .PARAMETER StaleAfterHours
    Hours after the newest blob before a flow-log config is reported as Stale. Default 3.

    .OUTPUTS
    PSCustomObject: the ReportModel (schema 1, camelCase properties).

    .EXAMPLE
    Get-FlowLogTraffic -InputPath ./sample-flowlog.json

    .EXAMPLE
    Get-FlowLogTraffic -Since 7d -Action Deny -Port '22,3389' | Export-FlowLogReport -Format Pdf -Path ./denied.pdf
    #>
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [string[]]$SubscriptionId,
        [string[]]$StorageAccountName,
        [string[]]$InputPath,
        [string]$TopologyPath,
        [string]$Since = '24h',
        [datetime]$Until,
        [ValidateSet('All', 'Allow', 'Deny')][string]$Action = 'All',
        [ValidateSet('All', 'TCP', 'UDP')][string]$Protocol = 'All',
        [string]$Port = '',
        [string]$IpFilter = '',
        [ValidateScript({ $_ -ge 0 })][long]$MinBytes = 0,
        [ValidateRange(1, 8760)][int]$StaleAfterHours = 3
    )

    $nowUtc = [datetime]::UtcNow
    $untilUtc = if ($PSBoundParameters.ContainsKey('Until')) { ConvertTo-FlowLogUtc -Value $Until } else { $nowUtc }
    $portFilter = ConvertTo-FlowLogPortFilter -Ports $Port
    $actionCode = $null
    if ($Action -eq 'Allow') { $actionCode = 'A' }
    if ($Action -eq 'Deny') { $actionCode = 'D' }
    $protocolCode = $null
    if ($Protocol -ne 'All') { $protocolCode = $Protocol.ToUpperInvariant() }

    $filter = [pscustomobject]@{
        ActionCode   = $actionCode
        ProtocolCode = $protocolCode
        Ports        = $portFilter
        IpFilter     = $IpFilter
        MinBytes     = $MinBytes
    }
    $filters = [pscustomobject]@{
        action   = $Action.ToLowerInvariant()
        protocol = if ($Protocol -eq 'All') { 'all' } else { $Protocol.ToUpperInvariant() }
        ports    = $Port
        ip       = $IpFilter
        minBytes = $MinBytes
    }
    $notes = [System.Collections.Generic.List[string]]::new()

    if ($PSBoundParameters.ContainsKey('InputPath')) {
        if ($PSBoundParameters.ContainsKey('SubscriptionId') -or $PSBoundParameters.ContainsKey('StorageAccountName')) {
            throw '-InputPath cannot be combined with -SubscriptionId or -StorageAccountName.'
        }
        $window = $null
        if ($PSBoundParameters.ContainsKey('Since') -or $PSBoundParameters.ContainsKey('Until')) {
            $window = Resolve-FlowLogWindow -Since $Since -UntilUtc $untilUtc
        }
        $state = New-FlowLogState -Window $window -Filter $filter
        if ($PSBoundParameters.ContainsKey('TopologyPath')) {
            $offline = New-FlowLogOfflineTopology -Path $TopologyPath
            $state.Topology = $offline.Topology
            foreach ($name in $offline.StorageNames) { $state.ReadAccounts[$name] = $true }
            $skipped = if ($offline.SkippedPrefixes -gt 0) { ", $($offline.SkippedPrefixes) non-IPv4 prefix(es) skipped." } else { '.' }
            $notes.Add("Offline topology from $(Split-Path -Leaf $TopologyPath): $($offline.NetworkCount) network(s), $($offline.PrefixCount) prefix(es)$skipped")
        }
        foreach ($file in @(Get-FlowLogInputFile -InputPath $InputPath)) {
            Add-FlowLogBlobJson -State $state -Json ([System.IO.File]::ReadAllText($file))
        }
        return Complete-FlowLogModel -State $state -Source 'local' -GeneratedUtc $nowUtc -Window $window -Filters $filters -Notes $notes
    }

    if ($PSBoundParameters.ContainsKey('TopologyPath')) {
        Write-Warning '-TopologyPath is ignored without -InputPath.'
    }
    $window = Resolve-FlowLogWindow -Since $Since -UntilUtc $untilUtc
    $monitoring = Get-FlowLogMonitoring -SubscriptionId $SubscriptionId -StorageAccountName $StorageAccountName -StaleAfterHours $StaleAfterHours -NowUtc $nowUtc
    foreach ($note in $monitoring.Notes) { $notes.Add($note) }

    $state = New-FlowLogState -Window $window -Filter $filter
    $state.Topology = $monitoring.Topology
    # Accounts are marked as read before any blob is parsed, so the dedupe rule applies the same way to every observation.
    foreach ($plan in $monitoring.ReadPlan) { $state.ReadAccounts[$plan.Name] = $true }
    foreach ($plan in $monitoring.ReadPlan) {
        Read-FlowLogAccount -State $state -Window $window -Plan $plan -Notes $notes
    }

    return Complete-FlowLogModel -State $state -Source 'azure' -GeneratedUtc $nowUtc -Window $window -Filters $filters -Monitoring $monitoring -Notes $notes
}
