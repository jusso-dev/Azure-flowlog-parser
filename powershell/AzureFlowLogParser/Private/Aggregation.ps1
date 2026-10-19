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

function Sort-FlowLogRows {
    # Same order as the .NET aggregator: bytes and flows descending, then the row key in ordinal order.
    param([object[]]$Rows)

    $list = [System.Collections.Generic.List[object]]::new()
    $list.AddRange([object[]]@($Rows))
    $comparison = [System.Comparison[object]]{
        param($left, $right)
        $result = ([long]$right.bytes).CompareTo([long]$left.bytes)
        if ($result -eq 0) { $result = ([long]$right.flows).CompareTo([long]$left.flows) }
        if ($result -eq 0) { $result = [string]::CompareOrdinal([string]$left.src, [string]$right.src) }
        if ($result -eq 0) { $result = [string]::CompareOrdinal([string]$left.dst, [string]$right.dst) }
        if ($result -eq 0) { $result = ([int]$left.dstPort).CompareTo([int]$right.dstPort) }
        foreach ($name in 'srcScope', 'dstScope', 'proto', 'action', 'direction', 'rule') {
            if ($result -eq 0) { $result = [string]::CompareOrdinal([string]$left.$name, [string]$right.$name) }
        }
        return $result
    }
    $list.Sort($comparison)
    return $list.ToArray()
}

function Sort-FlowLogScopes {
    # Scopes sort by kind, then id, both ordinal, as in the .NET report.
    param([object[]]$Scopes)

    $list = [System.Collections.Generic.List[object]]::new()
    $list.AddRange([object[]]@($Scopes))
    $comparison = [System.Comparison[object]]{
        param($left, $right)
        $result = [string]::CompareOrdinal([string]$left.kind, [string]$right.kind)
        if ($result -eq 0) { $result = [string]::CompareOrdinal([string]$left.id, [string]$right.id) }
        return $result
    }
    $list.Sort($comparison)
    return $list.ToArray()
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
        [object[]]$Subscriptions = @(),
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[string]]$Notes
    )

    $maxRows = 20000
    $rowsAll = @(Sort-FlowLogRows -Rows @($State.Rows.Values))
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

    $scopes = @(foreach ($scope in (Sort-FlowLogScopes -Scopes @($State.Scopes.Values))) {
        [pscustomobject]@{
            id             = $scope.id
            kind           = $scope.kind
            label          = $scope.label
            subscriptionId = $scope.subscriptionId
            resourceGroup  = $scope.resourceGroup
            storageAccount = $scope.storageAccount
        }
    })

    # Note text and order follow the .NET report: the counts note first, the caller's notes, then truncation and filters.
    $allNotes = [System.Collections.Generic.List[string]]::new()
    $allNotes.Add('Counts are flow log observations: one per NSG or VNet flow tuple. A flow crossing two logged networks is observed at both; the duplicate allowed inbound observation is skipped.')
    foreach ($note in $Notes) { $allNotes.Add($note) }
    if ($rowsTruncated) {
        $allNotes.Add("Flow rows capped at $($maxRows.ToString('N0', [cultureinfo]::InvariantCulture)) of $($rowsAll.Count.ToString('N0', [cultureinfo]::InvariantCulture)). Totals still include every observation.")
    }
    $filtered = $Filters.action -ne 'all' -or $Filters.protocol -ne 'all' -or [bool]$Filters.ports -or [bool]$Filters.ip -or $Filters.minBytes -gt 0
    if ($filtered) {
        $allNotes.Add('Filters apply per observation: port matches destination port; IP matches source or destination text.')
    }

    [pscustomobject]@{
        schema       = 1
        generatedUtc = Format-FlowLogTime -Value $GeneratedUtc
        source       = $Source
        subscriptions = @($Subscriptions)
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
