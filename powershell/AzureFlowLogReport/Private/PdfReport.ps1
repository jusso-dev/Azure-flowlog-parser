# PDF layout for the traffic report. Section order and contents follow the shared spec (section 6).

function Format-PdfCount {
    param($Value)

    return ([long]$Value).ToString('N0', [cultureinfo]::InvariantCulture)
}

function Get-PdfTimeAgeText {
    # Hours between a UTC ISO timestamp and the report time, or '-' when there is no blob.
    param($Latest, [string]$Generated)

    if ($null -eq $Latest) { return '-' }
    $styles = [System.Globalization.DateTimeStyles]::AssumeUniversal -bor [System.Globalization.DateTimeStyles]::AdjustToUniversal
    $latestUtc = [datetime]::Parse([string]$Latest, [cultureinfo]::InvariantCulture, $styles)
    $generatedUtc = [datetime]::Parse($Generated, [cultureinfo]::InvariantCulture, $styles)
    return [math]::Round(($generatedUtc - $latestUtc).TotalHours, 2).ToString('0.00', [cultureinfo]::InvariantCulture)
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

function Write-PdfTableHeader {
    param($Doc, [object[]]$Columns, [double]$TableWidth)

    $top = $Doc.Y
    Add-PdfRect -Doc $Doc -X 40 -Top $top -Width $TableWidth -Height 14 -Color '#1F2937'
    $cursor = [double]40
    foreach ($column in $Columns) {
        $text = Get-PdfFittedText -Text (ConvertTo-PdfAscii -Text $column.Header) -Width ($column.Width - 8) -Size 8 -Bold
        $x = Get-PdfCellX -CellStart $cursor -Column $column -Text $text -Size 8 -Bold
        Add-PdfText -Doc $Doc -X $x -Baseline ($top + 10) -Text $text -Size 8 -Bold -Color '#FFFFFF'
        $cursor += $column.Width
    }
    $Doc.Y += 14
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
            $text = Get-PdfFittedText -Text (ConvertTo-PdfAscii -Text $cell) -Width ($column.Width - 8) -Size 8
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

    Write-PdfHeading -Doc $doc -Text 'Summary'
    $metrics = @(
        @('Flows (observations)', (Format-PdfCount $summary.flows)),
        @('Allowed', (Format-PdfCount $summary.allowed)),
        @('Denied', (Format-PdfCount $summary.denied)),
        @('Bytes', (Format-PdfCount $summary.bytes)),
        @('Packets', (Format-PdfCount $summary.packets)),
        @('Storage accounts', (Format-PdfCount $summary.storageAccounts)),
        @('Scopes', (Format-PdfCount $summary.scopes)),
        @('Subscriptions', (Format-PdfCount $summary.subscriptions)),
        @('Unique source IPs', (Format-PdfCount $summary.uniqueSourceIps)),
        @('Unique destination IPs', (Format-PdfCount $summary.uniqueDestinationIps)),
        @('Skipped (dedupe)', (Format-PdfCount $summary.observationsSkipped)),
        @('Flow rows listed / total', "$(Format-PdfCount $summary.rows) / $(Format-PdfCount $summary.rowsTotal)")
    )
    $summaryRows = for ($i = 0; $i -lt $metrics.Count; $i += 2) {
        @{ Cells = @($metrics[$i][0], $metrics[$i][1], $metrics[$i + 1][0], $metrics[$i + 1][1]); Bars = @() }
    }
    $summaryColumns = @(
        @{ Header = 'Metric'; Width = 130; Align = 'L' },
        @{ Header = 'Value'; Width = 127; Align = 'R' },
        @{ Header = 'Metric'; Width = 130; Align = 'L' },
        @{ Header = 'Value'; Width = 128; Align = 'R' }
    )
    Write-PdfTable -Doc $doc -Columns $summaryColumns -Rows @($summaryRows)

    Write-PdfHeading -Doc $doc -Text 'Monitoring: storage accounts'
    if ($Model.accounts.Count -eq 0) {
        Write-PdfParagraph -Doc $doc -Text 'No storage accounts with flow logs or flow-log blobs in this run.'
    }
    else {
        $accountRows = foreach ($account in $Model.accounts) {
            @{ Cells = @($account.name, $account.subscriptionId, $account.status, $account.flowLogConfigs,
                    (Format-PdfOptionalText -Value $account.latestBlobUtc), (Get-PdfTimeAgeText -Latest $account.latestBlobUtc -Generated $Model.generatedUtc)); Bars = @() }
        }
        $accountColumns = @(
            @{ Header = 'Storage account'; Width = 110; Align = 'L' },
            @{ Header = 'Subscription'; Width = 105; Align = 'L' },
            @{ Header = 'Status'; Width = 70; Align = 'L' },
            @{ Header = 'Configs'; Width = 50; Align = 'R' },
            @{ Header = 'Latest blob (UTC)'; Width = 110; Align = 'L' },
            @{ Header = 'Age (h)'; Width = 70; Align = 'R' }
        )
        Write-PdfTable -Doc $doc -Columns $accountColumns -Rows @($accountRows)
    }

    Write-PdfHeading -Doc $doc -Text 'Monitoring: flow logs'
    if ($Model.flowLogs.Count -eq 0) {
        Write-PdfParagraph -Doc $doc -Text 'No flow-log configs in this run.'
    }
    else {
        $flowLogRows = foreach ($flowLog in $Model.flowLogs) {
            @{ Cells = @($flowLog.storageAccount, $flowLog.targetKind, (Get-PdfShortName -Id $flowLog.target), $(if ($flowLog.enabled) { 'yes' } else { 'no' }),
                    $flowLog.status, (Format-PdfOptionalText -Value $flowLog.latestBlobUtc), (Format-PdfAgeValue -Value $flowLog.ageHours)); Bars = @() }
        }
        $flowLogColumns = @(
            @{ Header = 'Storage account'; Width = 95; Align = 'L' },
            @{ Header = 'Kind'; Width = 40; Align = 'L' },
            @{ Header = 'Target'; Width = 140; Align = 'L' },
            @{ Header = 'Enabled'; Width = 40; Align = 'L' },
            @{ Header = 'Status'; Width = 65; Align = 'L' },
            @{ Header = 'Latest blob (UTC)'; Width = 95; Align = 'L' },
            @{ Header = 'Age (h)'; Width = 40; Align = 'R' }
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

function Format-PdfOptionalText {
    param($Value)

    if ($null -eq $Value) { return '-' }
    return [string]$Value
}

function Format-PdfAgeValue {
    param($Value)

    if ($null -eq $Value) { return '-' }
    return ([double]$Value).ToString('0.00', [cultureinfo]::InvariantCulture)
}
