# PDF layout for the traffic report. Section order and contents follow the shared spec (section 6).

function Format-PdfCount {
    param($Value)

    return ([long]$Value).ToString('N0', [cultureinfo]::InvariantCulture)
}

function Format-PdfBytes {
    # Same as the .NET report: whole bytes below 1 KB, otherwise one decimal in KB, MB, GB or TB.
    param([long]$Bytes)

    $units = 'B', 'KB', 'MB', 'GB', 'TB'
    $value = [double]$Bytes
    $unit = 0
    while ($value -ge 1024 -and $unit -lt $units.Count - 1) {
        $value /= 1024
        $unit++
    }
    if ($unit -eq 0) { return "$(Format-PdfCount $Bytes) B" }
    return '{0} {1}' -f $value.ToString('0.0', [cultureinfo]::InvariantCulture), $units[$unit]
}

function Get-PdfShortName {
    # Last segment of an ARM id, or the value itself.
    param([string]$Id)

    if ([string]::IsNullOrEmpty($Id)) { return '-' }
    return ($Id.TrimEnd('/').Split('/'))[-1]
}

function Write-PdfTitle {
    param($Doc, [string]$Text)

    Add-PdfText -Doc $Doc -X 40 -Baseline ($Doc.Y + 20) -Text $Text -Size 20 -Bold
    $Doc.Y += 28
}

function Write-PdfHeading {
    param($Doc, [string]$Text)

    if (-not (Test-PdfSpace -Doc $Doc -Height 60)) { Add-PdfPage -Doc $Doc }
    $Doc.Y += 6
    Add-PdfText -Doc $Doc -X 40 -Baseline ($Doc.Y + 13) -Text (ConvertTo-PdfAscii -Text $Text) -Size 13 -Bold
    $Doc.Y += 22
}

function Write-PdfParagraph {
    param($Doc, [string]$Text, [double]$Size = 9, [switch]$Bold)

    $lines = Get-PdfWrappedLine -Text (ConvertTo-PdfAscii -Text $Text) -Width 515 -Size $Size
    foreach ($line in $lines) {
        if (-not (Test-PdfSpace -Doc $Doc -Height 12)) { Add-PdfPage -Doc $Doc }
        $fitted = Get-PdfFittedText -Text $line -Width 515 -Size $Size -Bold:$Bold
        Add-PdfText -Doc $Doc -X 40 -Baseline ($Doc.Y + 9) -Text $fitted -Size $Size -Bold:$Bold
        $Doc.Y += 12
    }
    $Doc.Y += 4
}

function Get-PdfCellX {
    param([double]$CellStart, $Column, [string]$Text, [double]$Size, [switch]$Bold)

    if ($Column.Align -eq 'R') {
        return $CellStart + $Column.Width - 4 - (Get-PdfTextWidth -Text $Text -Size $Size -Bold:$Bold)
    }
    return $CellStart + 4
}

function Get-PdfHeaderLines {
    # A header too wide for its column wraps once at a space. The second line is truncated if it is still too wide.
    param([string]$Text, [double]$Width)

    $maxChars = [math]::Floor($Width / (8 * 0.6))
    if ($Text.Length -le $maxChars) { return [pscustomobject]@{ First = $Text; Second = '' } }
    $cut = $Text.LastIndexOf(' ', [math]::Min($maxChars, $Text.Length - 1))
    if ($cut -le 0) { return [pscustomobject]@{ First = (Get-PdfFittedText -Text $Text -Width $Width -Size 8 -Bold); Second = '' } }
    return [pscustomobject]@{
        First  = $Text.Substring(0, $cut)
        Second = Get-PdfFittedText -Text $Text.Substring($cut + 1) -Width $Width -Size 8 -Bold
    }
}

function Write-PdfTableHeader {
    param($Doc, [object[]]$Columns, [double]$TableWidth)

    # Header text may use the column less 4 pt, so the contract widths hold one line for every short header.
    $headers =@(foreach ($column in $Columns) { Get-PdfHeaderLines -Text (ConvertTo-PdfAscii -Text $column.Header) -Width ($column.Width - 4) })
    $height = if (@($headers | Where-Object { $_.Second }).Count -gt 0) { 24 } else { 14 }
    $top = $Doc.Y
    Add-PdfRect -Doc $Doc -X 40 -Top $top -Width $TableWidth -Height $height -Color '#1F2937'
    $cursor = [double]40
    for ($c = 0; $c -lt $Columns.Count; $c++) {
        $column = $Columns[$c]
        $header = $headers[$c]
        $firstBaseline = if ($header.Second) { $top + 10 } else { $top + $height / 2 + 3 }
        $x = Get-PdfCellX -CellStart $cursor -Column $column -Text $header.First -Size 8 -Bold
        Add-PdfText -Doc $Doc -X $x -Baseline $firstBaseline -Text $header.First -Size 8 -Bold -Color '#FFFFFF'
        if ($header.Second) {
            $x = Get-PdfCellX -CellStart $cursor -Column $column -Text $header.Second -Size 8 -Bold
            Add-PdfText -Doc $Doc -X $x -Baseline ($top + 20) -Text $header.Second -Size 8 -Bold -Color '#FFFFFF'
        }
        $cursor += $column.Width
    }
    $Doc.Y += $height
}

function Write-PdfTable {
    # $Rows: hashtables with Cells (string[]) and Bars (@{ Column; Fraction; Color; Lane = 'top'|'bottom' }).
    param($Doc, [object[]]$Columns, [object[]]$Rows)

    $rowHeight = 14
    $tableWidth = [double]0
    $starts = [System.Collections.Generic.List[double]]::new()
    foreach ($column in $Columns) {
        $starts.Add(40 + $tableWidth)
        $tableWidth += $column.Width
    }

    if (-not (Test-PdfSpace -Doc $Doc -Height ($rowHeight * 2))) { Add-PdfPage -Doc $Doc }
    Write-PdfTableHeader -Doc $Doc -Columns $Columns -TableWidth $tableWidth

    for ($index = 0; $index -lt $Rows.Count; $index++) {
        if (-not (Test-PdfSpace -Doc $Doc -Height $rowHeight)) {
            Add-PdfPage -Doc $Doc
            Write-PdfTableHeader -Doc $Doc -Columns $Columns -TableWidth $tableWidth
        }
        $top = $Doc.Y
        $row = $Rows[$index]
        if ($index % 2 -eq 1) {
            Add-PdfRect -Doc $Doc -X 40 -Top $top -Width $tableWidth -Height $rowHeight -Color '#F3F4F6'
        }

        for ($c = 0; $c -lt $Columns.Count; $c++) {
            $column = $Columns[$c]
            $cell = if ($c -lt $row.Cells.Count) { [string]$row.Cells[$c] } else { '' }
            # Body text gets the column less 4 pt, the same allowance as headers, so a 16-character timestamp fits 75 pt.
            $text = Get-PdfFittedText -Text (ConvertTo-PdfAscii -Text $cell) -Width ($column.Width - 4) -Size 8
            $x = Get-PdfCellX -CellStart $starts[$c] -Column $column -Text $text -Size 8
            Add-PdfText -Doc $Doc -X $x -Baseline ($top + 10) -Text $text -Size 8
        }

        foreach ($bar in $row.Bars) {
            $column = $Columns[$bar.Column]
            # Casts keep [math] on the double overloads; an int 0 would truncate fractions to 0.
            $fraction = [math]::Min([double]1, [math]::Max([double]0, [double]$bar.Fraction))
            $barWidth = $fraction * ($column.Width - 8)
            $barTop = if ($bar.Lane -eq 'top') { $top + 2 } else { $top + 8 }
            Add-PdfRect -Doc $Doc -X ($starts[$bar.Column] + 4) -Top $barTop -Width $barWidth -Height 4 -Color $bar.Color
        }

        Add-PdfLine -Doc $Doc -X1 40 -Y1 ($top + $rowHeight) -X2 (40 + $tableWidth) -Y2 ($top + $rowHeight)
        foreach ($start in $starts) {
            Add-PdfLine -Doc $Doc -X1 $start -Y1 $top -X2 $start -Y2 ($top + $rowHeight)
        }
        Add-PdfLine -Doc $Doc -X1 (40 + $tableWidth) -Y1 $top -X2 (40 + $tableWidth) -Y2 ($top + $rowHeight)
        $Doc.Y += $rowHeight
    }
    $Doc.Y += 8
}

function Get-PdfEdge {
    # Aggregates flow rows per (srcScope, dstScope), sorted by bytes.
    param($Model, [int]$Top = 50)

    $edges = @{}
    foreach ($flow in $Model.flows) {
        $key = "$($flow.srcScope)|$($flow.dstScope)"
        if (-not $edges.ContainsKey($key)) {
            $edges[$key] = [pscustomobject]@{ Source = $flow.srcScope; Destination = $flow.dstScope; Flows = [long]0; Allowed = [long]0; Denied = [long]0; Bytes = [long]0 }
        }
        $edge = $edges[$key]
        $edge.Flows += $flow.flows
        if ($flow.action -eq 'A') { $edge.Allowed += $flow.flows } else { $edge.Denied += $flow.flows }
        $edge.Bytes += $flow.bytes
    }
    return @($edges.Values | Sort-Object -Property Bytes -Descending | Select-Object -First $Top)
}

function Measure-PdfSum {
    # Sum of one property; an empty input sums to 0 (Measure-Object returns no object for empty input).
    param($Items, [string]$Property)

    $total = [long]0
    foreach ($item in @($Items)) {
        if ($null -ne $item) { $total += [long]$item.$Property }
    }
    return $total
}

function Get-PdfStorageView {
    # Per storage scope: in/out totals, denied flows, top 5 peers and top 5 destination ports (from listed rows).
    param($Model, [int]$Top = 5)

    foreach ($scope in @($Model.scopes | Where-Object { $_.kind -eq 'storage' })) {
        $id = $scope.id
        $inbound = @($Model.flows | Where-Object { $_.dstScope -eq $id })
        $outbound = @($Model.flows | Where-Object { $_.srcScope -eq $id })
        $touching = @($Model.flows | Where-Object { $_.srcScope -eq $id -or $_.dstScope -eq $id })
        $peers = @{}
        foreach ($flow in $inbound) { $peers[$flow.srcScope] = [long]$peers[$flow.srcScope] + $flow.bytes }
        foreach ($flow in $outbound) { $peers[$flow.dstScope] = [long]$peers[$flow.dstScope] + $flow.bytes }
        $ports = @{}
        foreach ($flow in $touching) { $ports[[string]$flow.dstPort] = [long]$ports[[string]$flow.dstPort] + $flow.flows }

        [pscustomobject]@{
            Label        = $scope.label
            InFlows      = Measure-PdfSum -Items $inbound -Property 'flows'
            InBytes      = Measure-PdfSum -Items $inbound -Property 'bytes'
            OutFlows     = Measure-PdfSum -Items $outbound -Property 'flows'
            OutBytes     = Measure-PdfSum -Items $outbound -Property 'bytes'
            DeniedFlows  = Measure-PdfSum -Items @($touching | Where-Object { $_.action -eq 'D' }) -Property 'flows'
            Peers        = @($peers.GetEnumerator() | Sort-Object -Property Value -Descending | Select-Object -First $Top)
            Ports        = @($ports.GetEnumerator() | Sort-Object -Property Value -Descending | Select-Object -First $Top)
        }
    }
}

function New-FlowLogPdf {
    [CmdletBinding()]
    [OutputType([byte[]])]
    param([Parameter(Mandatory)]$Model)

    $labels = @{}
    foreach ($scope in $Model.scopes) { $labels[$scope.id] = $scope.label }
    $labelOf = { param($Id) if ($labels.ContainsKey($Id)) { [string]$labels[$Id] } else { [string]$Id } }
    $summary = $Model.summary
    $doc = New-PdfDocument

    Write-PdfTitle -Doc $doc -Text 'Azure VNet Flow Log Report'
    Write-PdfParagraph -Doc $doc -Text "Generated $($Model.generatedUtc). Source: $($Model.source). Window: $($Model.window.fromUtc) to $($Model.window.toUtc)."
    $portText = if ($Model.filters.ports) { $Model.filters.ports } else { '-' }
    $ipText = if ($Model.filters.ip) { $Model.filters.ip } else { '-' }
    Write-PdfParagraph -Doc $doc -Text "Filters: action $($Model.filters.action), protocol $($Model.filters.protocol), ports $portText, ip $ipText, min bytes $($Model.filters.minBytes)."

    $subscriptions = @(Get-PdfOptionalValue -Object $Model -Name 'subscriptions' | Where-Object { $null -ne $_ })
    $tenantIds = @($subscriptions | ForEach-Object { $_.tenantId } | Where-Object { $_ } | Select-Object -Unique)
    $tenantText = if ($tenantIds.Count -gt 0) { $tenantIds -join ', ' } else { 'not available (offline input)' }
    $subscriptionText = if ($subscriptions.Count -gt 0) {
        (@($subscriptions | ForEach-Object { if ($_.displayName) { "$($_.displayName) ($($_.subscriptionId))" } else { [string]$_.subscriptionId } })) -join '; '
    }
    else { 'not available (offline input)' }

    Write-PdfHeading -Doc $doc -Text 'Summary'
    # Same rows, order, labels and formats as the .NET report's summary.
    $metrics = @(
        @('Tenant', $tenantText),
        @('Subscriptions', $subscriptionText),
        @('Flow observations', (Format-PdfCount $summary.flows)),
        @('Allowed / denied', "$(Format-PdfCount $summary.allowed) / $(Format-PdfCount $summary.denied)"),
        @('Bytes / packets', "$(Format-PdfBytes $summary.bytes) / $(Format-PdfCount $summary.packets)"),
        @('Storage accounts read', (Format-PdfCount $summary.storageAccounts)),
        @('Scopes in report', (Format-PdfCount $summary.scopes)),
        @('Unique source / destination IPs', "$(Format-PdfCount $summary.uniqueSourceIps) / $(Format-PdfCount $summary.uniqueDestinationIps)"),
        @('Allowed inbound duplicates skipped', (Format-PdfCount $summary.observationsSkipped))
    )
    $summaryRows = foreach ($metric in $metrics) {
        @{ Cells = @($metric[0], $metric[1]); Bars = @() }
    }
    $summaryColumns = @(
        @{ Header = 'Measure'; Width = 170; Align = 'L' },
        @{ Header = 'Value'; Width = 345; Align = 'L' }
    )
    Write-PdfTable -Doc $doc -Columns $summaryColumns -Rows @($summaryRows)

    Write-PdfHeading -Doc $doc -Text 'Monitoring: storage accounts'
    if ($Model.accounts.Count -eq 0) {
        Write-PdfParagraph -Doc $doc -Text 'No storage accounts with flow logs or flow-log blobs in this run.'
    }
    else {
        $accountRows = foreach ($account in $Model.accounts) {
            $resourceGroup = if ($account.resourceGroup) { [string]$account.resourceGroup } else { '-' }
            @{ Cells = @($account.name, $resourceGroup, (Get-PdfSubscriptionName -Subscriptions $subscriptions -SubscriptionId $account.subscriptionId),
                    $account.status, $account.flowLogConfigs, (Format-PdfShortTime -Value $account.latestBlobUtc), (Get-PdfShortName -Id $account.observedTarget)); Bars = @() }
        }
        $accountColumns = @(
            @{ Header = 'Storage account'; Width = 88; Align = 'L' },
            @{ Header = 'Resource group'; Width = 84; Align = 'L' },
            @{ Header = 'Subscription'; Width = 110; Align = 'L' },
            @{ Header = 'Status'; Width = 60; Align = 'L' },
            @{ Header = 'Configs'; Width = 38; Align = 'R' },
            @{ Header = 'Latest blob'; Width = 75; Align = 'L' },
            @{ Header = 'Unmanaged source'; Width = 60; Align = 'L' }
        )
        Write-PdfTable -Doc $doc -Columns $accountColumns -Rows @($accountRows)
    }

    Write-PdfHeading -Doc $doc -Text 'Monitoring: flow logs'
    if ($Model.flowLogs.Count -eq 0) {
        Write-PdfParagraph -Doc $doc -Text 'No flow-log configs in this run.'
    }
    else {
        $flowLogRows = foreach ($flowLog in $Model.flowLogs) {
            # Models from before targetName and targetResourceGroup existed fall back to the target id.
            $targetName = [string](Get-PdfOptionalValue -Object $flowLog -Name 'targetName')
            if (-not $targetName) { $targetName = Get-PdfShortName -Id $flowLog.target }
            $targetResourceGroup = [string](Get-PdfOptionalValue -Object $flowLog -Name 'targetResourceGroup')
            if (-not $targetResourceGroup) { $targetResourceGroup = '-' }
            $flowLogName = if ($flowLog.name) { [string]$flowLog.name } else { '-' }
            @{ Cells = @($flowLog.storageAccount, $flowLogName, $targetName, $targetResourceGroup, $flowLog.targetKind,
                    $(if ($flowLog.enabled) { 'yes' } else { 'no' }), $flowLog.status, (Format-PdfAgeValue -Value $flowLog.ageHours)); Bars = @() }
        }
        $flowLogColumns = @(
            @{ Header = 'Storage account'; Width = 78; Align = 'L' },
            @{ Header = 'Flow log'; Width = 92; Align = 'L' },
            @{ Header = 'Target'; Width = 88; Align = 'L' },
            @{ Header = 'Target RG'; Width = 92; Align = 'L' },
            @{ Header = 'Kind'; Width = 34; Align = 'L' },
            @{ Header = 'Enabled'; Width = 38; Align = 'L' },
            @{ Header = 'Status'; Width = 58; Align = 'L' },
            @{ Header = 'Age'; Width = 35; Align = 'R' }
        )
        Write-PdfTable -Doc $doc -Columns $flowLogColumns -Rows @($flowLogRows)
    }

    Write-PdfHeading -Doc $doc -Text 'Traffic between scopes (top 50 by bytes)'
    $edges = Get-PdfEdge -Model $Model
    if ($edges.Count -eq 0) {
        Write-PdfParagraph -Doc $doc -Text 'No flows matched the filters.'
    }
    else {
        $maxBytes = [double]($edges[0].Bytes)
        $edgeRows = foreach ($edge in $edges) {
            $maxBytesValue = if ($maxBytes -gt 0) { [double]$edge.Bytes / $maxBytes } else { 0 }
            $deniedShare = if ($edge.Flows -gt 0) { [double]$edge.Denied / [double]$edge.Flows } else { 0 }
            @{
                Cells = @((& $labelOf $edge.Source), (& $labelOf $edge.Destination), (Format-PdfCount $edge.Flows),
                    (Format-PdfCount $edge.Allowed), (Format-PdfCount $edge.Denied), (Format-PdfCount $edge.Bytes), '')
                Bars  = @(
                    @{ Column = 6; Fraction = $maxBytesValue; Color = '#2563EB'; Lane = 'top' },
                    @{ Column = 6; Fraction = $deniedShare; Color = '#DC2626'; Lane = 'bottom' }
                )
            }
        }
        $edgeColumns = @(
            @{ Header = 'Source scope'; Width = 110; Align = 'L' },
            @{ Header = 'Destination scope'; Width = 110; Align = 'L' },
            @{ Header = 'Flows'; Width = 45; Align = 'R' },
            @{ Header = 'Allowed'; Width = 45; Align = 'R' },
            @{ Header = 'Denied'; Width = 45; Align = 'R' },
            @{ Header = 'Bytes'; Width = 60; Align = 'R' },
            @{ Header = 'Bytes | denied'; Width = 100; Align = 'L' }
        )
        Write-PdfTable -Doc $doc -Columns $edgeColumns -Rows @($edgeRows)
    }

    Write-PdfHeading -Doc $doc -Text 'Per storage account'
    $views = @(Get-PdfStorageView -Model $Model)
    if ($views.Count -eq 0) {
        Write-PdfParagraph -Doc $doc -Text 'No storage-backed scopes in this run.'
    }
    foreach ($view in $views) {
        Write-PdfParagraph -Doc $doc -Text $view.Label -Bold
        Write-PdfParagraph -Doc $doc -Text ("In: {0} flows, {1} bytes. Out: {2} flows, {3} bytes. Denied: {4} flows." -f
            (Format-PdfCount $view.InFlows), (Format-PdfCount $view.InBytes), (Format-PdfCount $view.OutFlows),
            (Format-PdfCount $view.OutBytes), (Format-PdfCount $view.DeniedFlows))
        $peerText = @($view.Peers | ForEach-Object { '{0} ({1} bytes)' -f (& $labelOf $_.Key), (Format-PdfCount $_.Value) })
        Write-PdfParagraph -Doc $doc -Text ("Top peers: {0}." -f $(if ($peerText.Count) { $peerText -join ', ' } else { 'none' }))
        $portList = @($view.Ports | ForEach-Object { '{0} ({1} flows)' -f $_.Key, (Format-PdfCount $_.Value) })
        Write-PdfParagraph -Doc $doc -Text ("Top destination ports: {0}." -f $(if ($portList.Count) { $portList -join ', ' } else { 'none' }))
    }

    Write-PdfHeading -Doc $doc -Text 'Top flows (top 150 by bytes)'
    $topFlows = @($Model.flows | Select-Object -First 150)
    if ($topFlows.Count -eq 0) {
        Write-PdfParagraph -Doc $doc -Text 'No flows matched the filters.'
    }
    else {
        $flowRows = foreach ($flow in $topFlows) {
            $first = if ($flow.first.Length -ge 16) { $flow.first.Substring(0, 16).Replace('T', ' ') } else { $flow.first }
            @{ Cells = @($first, $flow.src, $flow.dst, $flow.dstPort, $flow.proto,
                    $(if ($flow.action -eq 'A') { 'allow' } else { 'deny' }), $(if ($flow.direction -eq 'I') { 'in' } else { 'out' }),
                    (Format-PdfCount $flow.flows), (Format-PdfCount $flow.bytes), $flow.rule); Bars = @() }
        }
        $flowColumns = @(
            @{ Header = 'First seen'; Width = 84; Align = 'L' },
            @{ Header = 'Source'; Width = 68; Align = 'L' },
            @{ Header = 'Destination'; Width = 68; Align = 'L' },
            @{ Header = 'Port'; Width = 32; Align = 'R' },
            @{ Header = 'Proto'; Width = 32; Align = 'L' },
            @{ Header = 'Action'; Width = 40; Align = 'L' },
            @{ Header = 'Dir'; Width = 24; Align = 'L' },
            @{ Header = 'Flows'; Width = 36; Align = 'R' },
            @{ Header = 'Bytes'; Width = 48; Align = 'R' },
            @{ Header = 'Rule'; Width = 83; Align = 'L' }
        )
        Write-PdfTable -Doc $doc -Columns $flowColumns -Rows @($flowRows)
    }

    Write-PdfHeading -Doc $doc -Text 'Notes'
    foreach ($note in $Model.notes) {
        Write-PdfParagraph -Doc $doc -Text "- $note"
    }

    for ($page = 0; $page -lt $doc.Pages.Count; $page++) {
        $doc.Current = $page
        $label = "Page $($page + 1)"
        $x = 297.5 - (Get-PdfTextWidth -Text $label -Size 8) / 2
        Add-PdfText -Doc $doc -X $x -Baseline ($script:PageHeight - 20) -Text $label -Size 8
    }

    return ConvertTo-PdfByteArray -ContentStreams $doc.Pages.ToArray()
}

function Get-PdfOptionalValue {
    # Property read that yields $null when the object lacks it, so older JSON still renders under strict mode.
    param($Object, [string]$Name)

    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

function Get-PdfSubscriptionName {
    # Display name for a subscription id, or the id when the report has no name for it.
    param($Subscriptions, [string]$SubscriptionId)

    $found = $Subscriptions | Where-Object { $_.subscriptionId -eq $SubscriptionId } | Select-Object -First 1
    if ($null -ne $found -and $found.displayName) { return [string]$found.displayName }
    return $SubscriptionId
}

function Format-PdfShortTime {
    # 'yyyy-MM-dd HH:mm' from a UTC ISO timestamp, as the .NET report writes it. '-' when there is no blob.
    param($Value)

    if ($null -eq $Value) { return '-' }
    $text = [string]$Value
    if ($text.Length -lt 16) { return $text }
    return $text.Substring(0, 16).Replace('T', ' ')
}

function Format-PdfAgeValue {
    param($Value)

    if ($null -eq $Value) { return '-' }
    return ([double]$Value).ToString('0.00', [cultureinfo]::InvariantCulture)
}
