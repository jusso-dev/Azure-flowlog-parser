# Tuple parsing, filtering, dedupe, aggregation and report model assembly.

function New-FlowLogState {
    # Mutable run state. $Window is $null for unbounded local reads.
    [CmdletBinding()]
    param($Window, [Parameter(Mandatory)]$Filter)

    return @{
        Topology       = $null
        Filter         = $Filter
        Window         = $Window
        Scopes         = @{}
        ScopeCache     = @{}
        ReadAccounts   = @{}
        Rows           = [System.Collections.Generic.Dictionary[string, hashtable]]::new([StringComparer]::Ordinal)
        SourceIps      = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        DestinationIps = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        Flows          = [long]0
        Allowed        = [long]0
        Denied         = [long]0
        Bytes          = [long]0
        Packets        = [long]0
        Skipped        = [long]0
        Malformed      = [long]0
        MinTime        = $null
        MaxTime        = $null
    }
}

function Add-FlowLogBlobJson {
    # Walks records -> flows -> flowGroups -> flowTuples and adds each tuple as one observation.
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable]$State, [string]$Json = '')

    $root = $Json | ConvertFrom-Json
    foreach ($record in @(Get-FlowLogProperty -Object $root -Name 'records')) {
        $versionValue = Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $record -Name 'properties') -Name 'Version'
        $version = if ($null -eq $versionValue) { 1 } else { [int]$versionValue }
        $recordTime = ConvertTo-FlowLogRecordTime -Text ([string](Get-FlowLogProperty -Object $record -Name 'time'))
        $flowRecords = Get-FlowLogProperty -Object $record -Name 'flowRecords'
        foreach ($flow in @(Get-FlowLogProperty -Object $flowRecords -Name 'flows')) {
            $rule = [string](Get-FlowLogProperty -Object $flow -Name 'rule')
            foreach ($group in @(Get-FlowLogProperty -Object $flow -Name 'flowGroups')) {
                foreach ($tuple in @(Get-FlowLogProperty -Object $group -Name 'flowTuples')) {
                    Add-FlowLogObservation -State $State -Tuple ([string]$tuple) -Version $version -Rule $rule -RecordTime $recordTime
                }
            }
        }
    }
}

function Add-FlowLogObservation {
    # One flow tuple = one observation. Time is the tuple start, falling back to the record time.
    # Order: time, window, filters, scope, dedupe, aggregate.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][hashtable]$State,
        [Parameter(Mandatory)][string]$Tuple,
        [Parameter(Mandatory)][int]$Version,
        [string]$Rule = '',
        [AllowNull()][Nullable[datetime]]$RecordTime = $null
    )

    $parts = $Tuple.Split(',')
    if ($parts.Count -lt 8) {
        $State.Malformed++
        return
    }
    $epoch = [long]0
    $time = $null
    if ([long]::TryParse($parts[0], [ref]$epoch) -and $epoch -ge 0 -and $epoch -le 253402300799) {
        $time = [DateTimeOffset]::FromUnixTimeSeconds($epoch).UtcDateTime
    }
    elseif ($null -ne $RecordTime) {
        $time = [datetime]$RecordTime
    }
    if ($null -eq $time) {
        $State.Malformed++
        return
    }
    if ($null -ne $State.Window -and ($time -lt $State.Window.FromUtc -or $time -gt $State.Window.ToUtc)) { return }

    $filter = $State.Filter
    $source = $parts[1]
    $destination = $parts[2]
    $port = 0
    [void][int]::TryParse($parts[4], [ref]$port)
    $proto = switch ($parts[5]) {
        'T' { 'TCP' }
        'U' { 'UDP' }
        default { $parts[5] }
    }
    $direction = $parts[6]
    $action = $parts[7]

    if ($null -ne $filter.ActionCode -and $action -ne $filter.ActionCode) { return }
    if ($null -ne $filter.ProtocolCode -and $proto -ne $filter.ProtocolCode) { return }
    if (-not (Test-FlowLogPort -Filter $filter.Ports -Port $port)) { return }
    $ipFilter = [string]$filter.IpFilter
    if ($ipFilter -ne '' -and $source.IndexOf($ipFilter, [StringComparison]::OrdinalIgnoreCase) -lt 0 -and
        $destination.IndexOf($ipFilter, [StringComparison]::OrdinalIgnoreCase) -lt 0) { return }

    $packets = [long]0
    $bytes = [long]0
    if ($Version -ge 2 -and $parts.Count -gt 8) {
        $packets = (Get-FlowLogCount -Parts $parts -Index 9) + (Get-FlowLogCount -Parts $parts -Index 11)
        $bytes = (Get-FlowLogCount -Parts $parts -Index 10) + (Get-FlowLogCount -Parts $parts -Index 12)
    }
    if ($bytes -lt $filter.MinBytes) { return }

    $srcScope = Resolve-FlowLogScope -State $State -Address $source
    $dstScope = Resolve-FlowLogScope -State $State -Address $destination
    if ($direction -eq 'I' -and $action -eq 'A' -and $srcScope.kind -eq 'storage' -and $State.ReadAccounts.ContainsKey($srcScope.id)) {
        $State.Skipped++
        return
    }

    $State.Flows++
    if ($action -eq 'A') { $State.Allowed++ } else { $State.Denied++ }
    $State.Bytes += $bytes
    $State.Packets += $packets
    [void]$State.SourceIps.Add($source)
    [void]$State.DestinationIps.Add($destination)
    if ($null -eq $State.MinTime -or $time -lt $State.MinTime) { $State.MinTime = $time }
    if ($null -eq $State.MaxTime -or $time -gt $State.MaxTime) { $State.MaxTime = $time }

    $key = "$source|$destination|$($srcScope.id)|$($dstScope.id)|$proto|$port|$action|$direction|$Rule"
    if (-not $State.Rows.ContainsKey($key)) {
        $State.Rows[$key] = @{
            first     = $time
            last      = $time
            src       = $source
            dst       = $destination
            srcScope  = $srcScope.id
            dstScope  = $dstScope.id
            proto     = $proto
            dstPort   = $port
            action    = $action
            direction = $direction
            rule      = $Rule
            flows     = [long]0
            packets   = [long]0
            bytes     = [long]0
        }
    }
    $row = $State.Rows[$key]
    if ($time -lt $row.first) { $row.first = $time }
    if ($time -gt $row.last) { $row.last = $time }
    $row.flows++
    $row.packets += $packets
    $row.bytes += $bytes
}

function Get-FlowLogInputFile {
    param([string[]]$InputPath)

    foreach ($path in $InputPath) {
        if (-not (Test-Path -LiteralPath $path)) { throw "Input path not found: $path" }
        $item = Get-Item -LiteralPath $path
        if ($item.PSIsContainer) {
            Get-ChildItem -LiteralPath $item.FullName -Filter '*.json' -Recurse -File | Sort-Object -Property FullName | ForEach-Object { $_.FullName }
        }
        else {
            $item.FullName
        }
    }
}

function Complete-FlowLogModel {
    # Builds the ReportModel (schema 1). $Window null means local data with no time bounds; use observed range.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][hashtable]$State,
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][datetime]$GeneratedUtc,
        $Window,
        [Parameter(Mandatory)]$Filters,
        $Monitoring,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[string]]$Notes
    )

    $maxRows = 20000
    $rowsAll = @($State.Rows.Values | Sort-Object -Property @(
        @{ Expression = { $_.bytes }; Descending = $true },
        @{ Expression = { $_.flows }; Descending = $true }
    ))
    $rowsTruncated = $rowsAll.Count -gt $maxRows
    $rows = @(foreach ($row in @($rowsAll | Select-Object -First $maxRows)) {
        $first = Format-FlowLogTime -Value $row.first
        $last = Format-FlowLogTime -Value $row.last
        [pscustomobject]@{
            first     = $first
            last      = $last
            src       = $row.src
            dst       = $row.dst
            srcScope  = $row.srcScope
            dstScope  = $row.dstScope
            proto     = $row.proto
            dstPort   = $row.dstPort
            action    = $row.action
            direction = $row.direction
            rule      = $row.rule
            flows     = $row.flows
            packets   = $row.packets
            bytes     = $row.bytes
        }
    })

    if ($null -ne $Window) {
        $fromUtc = $Window.FromUtc
        $toUtc = $Window.ToUtc
    }
    else {
        $fromUtc = if ($null -ne $State.MinTime) { $State.MinTime } else { $GeneratedUtc }
        $toUtc = if ($null -ne $State.MaxTime) { $State.MaxTime } else { $GeneratedUtc }
    }

    $accounts = @()
    $flowLogs = @()
    $subscriptionCount = 0
    if ($null -ne $Monitoring) {
        $accounts = @($Monitoring.Accounts | Where-Object { $_.hasFlowLogs -or $_.status -eq 'AccessDenied' })
        $flowLogs = @($Monitoring.FlowLogs)
        $subscriptionCount = $Monitoring.SubscriptionCount
    }

    $scopes = @(foreach ($scope in @($State.Scopes.Values | Sort-Object -Property { $_.id })) {
        [pscustomobject]@{
            id             = $scope.id
            kind           = $scope.kind
            label          = $scope.label
            subscriptionId = $scope.subscriptionId
            resourceGroup  = $scope.resourceGroup
            storageAccount = $scope.storageAccount
        }
    })

    $allNotes = [System.Collections.Generic.List[string]]::new()
    $allNotes.Add('Counting: each flow-log tuple is one observation, so flows = observations (1 each). Bytes and packets are 0 for version 1 flow logs.')
    $allNotes.Add("Dedupe: an inbound allowed observation whose source is a storage-backed scope is skipped when that storage account was read in this run, because its outbound record is counted at the source NSG. Skipped: $($State.Skipped).")
    if ($State.Malformed -gt 0) { $allNotes.Add("Malformed tuples skipped: $($State.Malformed).") }
    if ($rowsTruncated) {
        $allNotes.Add("Truncated: $($rowsAll.Count) distinct rows; the top $maxRows by bytes are listed. Summary totals cover all observations.")
    }
    if ($Source -eq 'azure') {
        $allNotes.Add('Mapping: IPv4 longest-prefix match over NIC IPs, subnets and VNets of the selected subscriptions. A flow-log target maps to its storage account. Other VNet space maps to a vnet scope. Unmatched public IPv4 is internet; private or non-IPv4 is unmapped.')
    }
    else {
        if ($null -ne $State.Topology) {
            $allNotes.Add('Local files with offline topology: longest IPv4 prefix match. Unmatched public IPv4 is internet; private or non-IPv4 is unmapped.')
        }
        else {
            $allNotes.Add('Local files: no ARM topology. Public IPv4 is internet; private or non-IPv4 is unmapped.')
        }
        if ($null -eq $Window) { $allNotes.Add('No time filter applied to local files. Use -Since or -Until to filter. Window is the observed range.') }
    }
    $allNotes.Add('Port filter matches the destination port. IP filter matches the source or destination address. Min bytes applies per observation.')
    foreach ($note in $Notes) { $allNotes.Add($note) }

    [pscustomobject]@{
        schema       = 1
        generatedUtc = Format-FlowLogTime -Value $GeneratedUtc
        source       = $Source
        window       = [pscustomobject]@{ fromUtc = Format-FlowLogTime -Value $fromUtc; toUtc = Format-FlowLogTime -Value $toUtc }
        filters      = $Filters
        summary      = [pscustomobject]@{
            flows                = $State.Flows
            allowed              = $State.Allowed
            denied               = $State.Denied
            bytes                = $State.Bytes
            packets              = $State.Packets
            rows                 = @($rows).Count
            rowsTotal            = $rowsAll.Count
            rowsTruncated        = $rowsTruncated
            storageAccounts      = $State.ReadAccounts.Count
            scopes               = @($scopes).Count
            subscriptions        = $subscriptionCount
            observationsSkipped  = $State.Skipped
            uniqueSourceIps      = $State.SourceIps.Count
            uniqueDestinationIps = $State.DestinationIps.Count
        }
        scopes       = @($scopes)
        flows        = @($rows)
        accounts     = $accounts
        flowLogs     = $flowLogs
        notes        = $allNotes.ToArray()
    }
}
