# Record output and blob state for Get-FlowLogRecord. Field names and order follow the .NET DenormalizedFlowRecord.

function ConvertTo-FlowLogRecord {
    # Walks records -> flows -> flowGroups -> flowTuples and returns one denormalized object per tuple.
    # DateKind String keeps 'time' as the text in the blob, as the .NET parser keeps it.
    param([Parameter(Mandatory)][string]$Json)

    $root = $Json | ConvertFrom-Json -DateKind String
    $records = [System.Collections.Generic.List[object]]::new()
    foreach ($record in @(Get-FlowLogProperty -Object $root -Name 'records')) {
        $versionValue = Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $record -Name 'properties') -Name 'Version'
        $version = if ($null -eq $versionValue) { 1 } else { [int]$versionValue }
        foreach ($flow in @(Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $record -Name 'flowRecords') -Name 'flows')) {
            $rule = [string](Get-FlowLogProperty -Object $flow -Name 'rule')
            foreach ($group in @(Get-FlowLogProperty -Object $flow -Name 'flowGroups')) {
                $mac = [string](Get-FlowLogProperty -Object $group -Name 'mac')
                foreach ($tuple in @(Get-FlowLogProperty -Object $group -Name 'flowTuples')) {
                    $records.Add((New-FlowLogRecordObject -Record $record -Version $version -Rule $rule -Mac $mac -Tuple ([string]$tuple)))
                }
            }
        }
    }
    return $records.ToArray()
}

function New-FlowLogRecordObject {
    # One flow tuple: startTime,src,dst,srcPort,dstPort,proto,direction,action[,flowState,pktS2D,bytesS2D,pktD2S,bytesD2S].
    # Version 2+ adds the last five fields; an empty count becomes "0". Version 1 has no such fields in the output.
    param($Record, [int]$Version, [string]$Rule, [string]$Mac, [string]$Tuple)

    $parts = $Tuple.Split(',')
    $at = { param([int]$Index) if ($Index -lt $parts.Count) { [string]$parts[$Index] } else { '' } }
    $count = { param([int]$Index) if ($Index -lt $parts.Count -and $parts[$Index]) { [string]$parts[$Index] } else { '0' } }
    $f = @(0..7 | ForEach-Object { & $at $_ })

    $record = [ordered]@{
        time               = [string](Get-FlowLogProperty -Object $Record -Name 'time')
        category           = [string](Get-FlowLogProperty -Object $Record -Name 'category')
        operationName      = [string](Get-FlowLogProperty -Object $Record -Name 'operationName')
        resourceId         = [string](Get-FlowLogProperty -Object $Record -Name 'resourceId')
        version            = $Version
        nsgRuleName        = $Rule
        mac                = $Mac
        startTime          = $f[0]
        sourceAddress      = $f[1]
        destinationAddress = $f[2]
        sourcePort         = $f[3]
        destinationPort    = $f[4]
        transportProtocol  = $f[5]
        deviceDirection    = $f[6]
        deviceAction       = $f[7]
    }
    if ($Version -ge 2 -and $parts.Count -gt 8) {
        $record['flowState'] = & $at 8
        $record['packetsStoD'] = & $count 9
        $record['bytesStoD'] = & $count 10
        $record['packetsDtoS'] = & $count 11
        $record['bytesDtoS'] = & $count 12
    }
    return [pscustomobject]$record
}

function Test-FlowLogBlobNeedsProcessing {
    # True unless the blob's metadata says this version was processed: lastProcessed and processedBlobLastModified are
    # present, the timestamp parses, and the blob is not newer than it. Missing or unreadable metadata means process.
    param($Context, [string]$Container, [string]$Name, [datetime]$LastModifiedUtc)

    try {
        $metadata = Get-FlowLogBlobMetadata -Context $Context -Container $Container -Name $Name
    }
    catch {
        return $true
    }
    $lastProcessed = Get-FlowLogProperty -Object $metadata -Name 'lastProcessed'
    $processedText = Get-FlowLogProperty -Object $metadata -Name 'processedBlobLastModified'
    if ($null -eq $lastProcessed -or $null -eq $processedText) { return $true }
    $processed = [datetimeoffset]::MinValue
    if (-not [datetimeoffset]::TryParse([string]$processedText, [cultureinfo]::InvariantCulture, [System.Globalization.DateTimeStyles]::None, [ref]$processed)) {
        return $true
    }
    return $LastModifiedUtc.ToUniversalTime() -gt $processed.UtcDateTime
}

function Write-FlowLogRecordFile {
    # Json: one indented array, as the .NET file output. Jsonl: one compact object per line. UTF-8 without a BOM.
    param([object[]]$Records, [Parameter(Mandatory)][string]$FullPath, [Parameter(Mandatory)][string]$Format)

    $folder = Split-Path -Parent $FullPath
    if (-not (Test-Path -LiteralPath $folder -PathType Container)) { throw "Output folder not found: $folder" }

    if ($Format -eq 'Jsonl') {
        $lines = @($Records | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress -Depth 5 })
        $text = if ($lines.Count -gt 0) { ($lines -join "`n") + "`n" } else { '' }
    }
    else {
        $text = ConvertTo-Json -InputObject @($Records) -Depth 5
    }
    [System.IO.File]::WriteAllText($FullPath, $text, [System.Text.UTF8Encoding]::new($false))
}
