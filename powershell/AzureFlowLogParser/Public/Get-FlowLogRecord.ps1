function Get-FlowLogRecord {
    <#
    .SYNOPSIS
    Reads flow-log blobs from storage accounts and writes one denormalized record per flow tuple.

    .DESCRIPTION
    The PowerShell counterpart of the .NET parse command's reads. For each storage account it checks that the
    container exists, lists the blobs (under -Prefix when given), and skips blobs that were already processed.
    A blob is processed unless its metadata holds lastProcessed and processedBlobLastModified, the timestamp
    parses, and the blob's LastModified is not newer than it. Missing or unreadable metadata means process.
    -Limit applies after that filter.

    Each processed blob is parsed and then marked, with state tracking on, by replacing its metadata with four
    pairs: lastProcessed (UTC now), processedBlobLastModified (the blob's LastModified), recordCount and processedBy
    (AzureFlowLogParser). A failed download or parse, or a failed metadata write, warns and moves on.

    Storage calls go through the Azure CLI (az) with --auth-mode login. This function does not post records over HTTP;
    the .NET parse command's --http-endpoint is not ported.

    .PARAMETER StorageAccountName
    One or more storage account names to read.

    .PARAMETER Container
    Container that holds the flow-log blobs. Default: insights-logs-flowlogflowevent.

    .PARAMETER Prefix
    Only blobs whose names start with this text are listed.

    .PARAMETER Limit
    Process at most this many blobs per account, after the state filter. 0 means no limit.

    .PARAMETER ListOnly
    Writes the blob names that would be processed. Downloads nothing and writes no metadata. As in the .NET parse
    command, the state filter is not applied: every listed blob is shown (and -Limit still applies).

    .PARAMETER DisableStateTracking
    Turns state tracking off: no metadata is read or written.

    .PARAMETER ForceReprocess
    Processes every blob and does not read metadata. Metadata is still written after each parse when tracking is on.

    .PARAMETER OutputPath
    Writes the records to this file instead of the pipeline. Json writes one indented JSON array; Jsonl writes one
    compact object per line. The file is replaced.

    .PARAMETER Format
    Json or Jsonl, for -OutputPath. Default: Json.

    .OUTPUTS
    PSCustomObject, one per flow tuple, when the records go to the pipeline. The FileInfo of the written file with
    -OutputPath. The blob names (strings) with -ListOnly.

    .EXAMPLE
    Get-FlowLogRecord -StorageAccountName stflowlogs01 -Limit 5 -OutputPath ./records.json

    .EXAMPLE
    Get-FlowLogRecord -StorageAccountName stflowlogs01, stflowlogs02 -Format Jsonl -OutputPath ./records.jsonl

    .EXAMPLE
    Get-FlowLogRecord -StorageAccountName stflowlogs01 -ListOnly
    #>
    [CmdletBinding()]
    [OutputType([pscustomobject], [System.IO.FileInfo], [string])]
    param(
        [Parameter(Mandatory)][string[]]$StorageAccountName,
        [string]$Container = 'insights-logs-flowlogflowevent',
        [string]$Prefix = '',
        [ValidateRange(0, [int]::MaxValue)][int]$Limit = 0,
        [switch]$ListOnly,
        [switch]$DisableStateTracking,
        [switch]$ForceReprocess,
        [string]$OutputPath,
        [ValidateSet('Json', 'Jsonl')][string]$Format = 'Json'
    )

    $tracking = -not $DisableStateTracking
    $records = [System.Collections.Generic.List[object]]::new()
    foreach ($account in $StorageAccountName) {
        try {
            $context = New-FlowLogStorageContext -StorageAccountName $account
            if (-not (Test-FlowLogContainer -Context $context -Container $Container)) {
                Write-Warning "Container $Container not found in storage account $account. Nothing to read."
                continue
            }

            $pending = [System.Collections.Generic.List[object]]::new()
            foreach ($blob in @(Get-FlowLogBlobList -Context $context -Prefix $Prefix -Container $Container)) {
                if ($tracking -and -not $ForceReprocess -and -not $ListOnly) {
                    if (-not (Test-FlowLogBlobNeedsProcessing -Context $context -Container $Container -Name $blob.Name -LastModifiedUtc $blob.LastModifiedUtc)) { continue }
                }
                $pending.Add($blob)
            }
            if ($Limit -gt 0 -and $pending.Count -gt $Limit) { $pending.RemoveRange($Limit, $pending.Count - $Limit) }

            if ($ListOnly) {
                foreach ($blob in $pending) { $blob.Name }
                continue
            }

            foreach ($blob in $pending) {
                try {
                    $text = Read-FlowLogBlobText -Context $context -Container $Container -Name $blob.Name
                }
                catch {
                    Write-Warning "Could not download $($blob.Name) from ${account}: $($_.Exception.Message)"
                    continue
                }

                try {
                    $parsed = @(ConvertTo-FlowLogRecord -Json $text)
                }
                catch {
                    Write-Warning "Could not parse $($blob.Name) from ${account}: $($_.Exception.Message)"
                    continue
                }
                foreach ($record in $parsed) { $records.Add($record) }

                if ($tracking) {
                    $culture = [cultureinfo]::InvariantCulture
                    $state = [ordered]@{
                        lastProcessed             = ([datetimeoffset]::UtcNow).ToString('o', $culture)
                        processedBlobLastModified = ([datetimeoffset]$blob.LastModifiedUtc).ToString('o', $culture)
                        recordCount               = [string]$parsed.Count
                        processedBy               = 'AzureFlowLogParser'
                    }
                    try {
                        Set-FlowLogBlobMetadata -Context $context -Container $Container -Name $blob.Name -Metadata $state
                    }
                    catch {
                        Write-Warning "Could not write processing metadata on $($blob.Name) in ${account}: $($_.Exception.Message)"
                    }
                }
            }
        }
        catch {
            Write-Warning "Error processing storage account '$account': $($_.Exception.Message)"
        }
    }

    if ($ListOnly) { return }
    if (-not $OutputPath) {
        foreach ($record in $records) { $record }
        return
    }
    $fullPath = $PSCmdlet.GetUnresolvedProviderPathFromPSPath($OutputPath)
    Write-FlowLogRecordFile -Records $records.ToArray() -FullPath $fullPath -Format $Format
    Get-Item -LiteralPath $fullPath
}
