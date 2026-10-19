#Requires -Version 7.6
# Get-FlowLogRecord and blob state tracking. Only Invoke-FlowLogAzCli is mocked, so no test calls az.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..' 'AzureFlowLogParser' 'AzureFlowLogParser.psd1') -Force
    # Safety net: a call that reaches the Azure CLI without the per-test mock below fails instead of calling az.
    Mock -ModuleName AzureFlowLogParser Invoke-FlowLogAzCli { throw 'Invoke-FlowLogAzCli was not mocked: no test may call az.' }

    # A flow-log blob with the given number of version 2 tuples.
    function New-BlobJson {
        param([int]$Tuples)
        $list = @(for ($i = 0; $i -lt $Tuples; $i++) {
            "1729328400,10.10.1.$($i + 4),13.107.42.14,5000$i,443,T,O,A,B,1,100,1,200"
        })
        $tupleText = ($list | ForEach-Object { '"' + $_ + '"' }) -join ','
        '{"records":[{"time":"2026-10-19T09:00:00Z","category":"NetworkSecurityGroupFlowEvent","operationName":"NetworkSecurityGroupFlowEvents",' +
        '"resourceId":"/SUBSCRIPTIONS/00000000-0000-0000-0000-000000000001/RESOURCEGROUPS/RG-APP/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/WEB-NSG",' +
        '"properties":{"Version":2},"flowRecords":{"flows":[{"rule":"DefaultRule_AllowInternetOutBound","flowGroups":[{"rule":"DefaultRule_AllowInternetOutBound","mac":"001122334455","flowTuples":[' +
        $tupleText + ']}]}]}}]}'
    }

    $script:Culture = [cultureinfo]::InvariantCulture
    $script:BlobText = @{
        'blob-a' = New-BlobJson -Tuples 2
        'blob-b' = New-BlobJson -Tuples 3
        'blob-c' = New-BlobJson -Tuples 1
    }
}

Describe 'Get-FlowLogRecord' {
    BeforeEach {
        $script:ContainerExists = $true
        $script:BlobListing = @(
            [pscustomobject]@{ name = 'blob-a'; properties = [pscustomobject]@{ lastModified = '2026-10-19T09:30:00+00:00'; contentLength = 100 } }
            [pscustomobject]@{ name = 'blob-b'; properties = [pscustomobject]@{ lastModified = '2026-10-19T10:30:00+00:00'; contentLength = 100 } }
        )
        $script:MetadataFor = @{}
        $script:MetadataReadFailure = @()
        $script:DownloadFailure = @()
        $script:WriteFailure = @()
        $script:Downloads = [System.Collections.Generic.List[string]]::new()
        $script:Writes = [System.Collections.Generic.List[object]]::new()

        Mock -ModuleName AzureFlowLogParser Invoke-FlowLogAzCli {
            param([string[]]$Arguments)
            $command = $Arguments -join ' '
            $nameArgument = @($Arguments | Where-Object { $_ -like '--name=*' })
            $name = if ($nameArgument.Count) { $nameArgument[0].Substring(7) } else { '' }
            switch -Wildcard ($command) {
                'storage container exists *' { return [pscustomobject]@{ exists = $script:ContainerExists } }
                'storage blob list *' { return $script:BlobListing }
                'storage blob metadata show *' {
                    if ($script:MetadataReadFailure -contains $name) { throw 'ERROR: metadata read failed' }
                    if ($script:MetadataFor.ContainsKey($name)) { return [pscustomobject]$script:MetadataFor[$name] }
                    return
                }
                'storage blob metadata update *' {
                    if ($script:WriteFailure -contains $name) { throw 'ERROR: metadata write failed' }
                    $script:Writes.Add([pscustomobject]@{ Name = $name; Arguments = $Arguments })
                    return
                }
                'storage blob download *' {
                    $script:Downloads.Add($name)
                    if ($script:DownloadFailure -contains $name) { throw 'ERROR: download failed' }
                    $target = $Arguments[[array]::IndexOf($Arguments, '--file') + 1]
                    [System.IO.File]::WriteAllText($target, $script:BlobText[$name])
                    return
                }
                default { throw "Unexpected az call in a Get-FlowLogRecord test: $command" }
            }
        }
    }

    It 'skips a blob whose metadata already records its last modified time' {
        $script:MetadataFor['blob-a'] = @{
            lastProcessed = '2026-10-19T09:31:00.0000000+00:00'
            processedBlobLastModified = '2026-10-19T09:30:00.0000000+00:00'
            recordCount = '2'
            processedBy = 'AzureFlowLogParser'
        }
        $records = @(Get-FlowLogRecord -StorageAccountName 'stflowlogs01')
        $script:Downloads | Should -Be @('blob-b')
        $records.Count | Should -Be 3
    }

    It 'processes a blob whose last modified time is newer than the processed time' {
        $script:MetadataFor['blob-a'] = @{
            lastProcessed = '2026-10-19T08:01:00.0000000+00:00'
            processedBlobLastModified = '2026-10-19T08:00:00.0000000+00:00'
        }
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' | Out-Null
        $script:Downloads | Should -Contain 'blob-a'
    }

    It 'processes a blob with missing or unreadable metadata' {
        $script:MetadataReadFailure = @('blob-b')
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' | Out-Null
        $script:Downloads | Should -Be @('blob-a', 'blob-b')
    }

    It '-ForceReprocess ignores metadata and reads none' {
        $script:MetadataFor['blob-a'] = @{
            lastProcessed = '2026-10-19T09:31:00.0000000+00:00'
            processedBlobLastModified = '2026-10-19T09:30:00.0000000+00:00'
        }
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -ForceReprocess | Out-Null
        $script:Downloads | Should -Be @('blob-a', 'blob-b')
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -Times 0 -Exactly -ParameterFilter {
            ($Arguments -join ' ') -like 'storage blob metadata show *'
        }
    }

    It 'writes exactly four metadata pairs after a successful parse' {
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' | Out-Null
        $write = @($script:Writes | Where-Object { $_.Name -eq 'blob-b' })[0]
        $write | Should -Not -BeNullOrEmpty
        $start = [array]::IndexOf($write.Arguments, '--metadata') + 1
        $pairs = @($write.Arguments[$start..($write.Arguments.Count - 1)])
        $pairs.Count | Should -Be 4
        ($pairs | ForEach-Object { ($_ -split '=', 2)[0] }) | Should -Be @('lastProcessed', 'processedBlobLastModified', 'recordCount', 'processedBy')
        $pairs[1] | Should -Be 'processedBlobLastModified=2026-10-19T10:30:00.0000000+00:00'
        $pairs[2] | Should -Be 'recordCount=3'
        $pairs[3] | Should -Be 'processedBy=AzureFlowLogParser'
        $pairs[0] | Should -Match '^lastProcessed=\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}\+00:00$'
    }

    It '-Limit applies after the state filter' {
        $script:BlobListing += [pscustomobject]@{ name = 'blob-c'; properties = [pscustomobject]@{ lastModified = '2026-10-19T11:30:00+00:00'; contentLength = 100 } }
        $script:MetadataFor['blob-a'] = @{
            lastProcessed = '2026-10-19T09:31:00.0000000+00:00'
            processedBlobLastModified = '2026-10-19T09:30:00.0000000+00:00'
        }
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -Limit 1 | Out-Null
        $script:Downloads | Should -Be @('blob-b')
    }

    It '-ListOnly writes the blob names and downloads, marks and reads nothing' {
        $names = @(Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -ListOnly)
        $names | Should -Be @('blob-a', 'blob-b')
        $script:Downloads.Count | Should -Be 0
        $script:Writes.Count | Should -Be 0
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -Times 0 -Exactly -ParameterFilter {
            ($Arguments -join ' ') -like 'storage blob metadata *'
        }
    }

    It '-DisableStateTracking reads and writes no metadata' {
        $script:MetadataFor['blob-a'] = @{
            lastProcessed = '2026-10-19T09:31:00.0000000+00:00'
            processedBlobLastModified = '2026-10-19T09:30:00.0000000+00:00'
        }
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -DisableStateTracking | Out-Null
        $script:Downloads | Should -Be @('blob-a', 'blob-b')
        $script:Writes.Count | Should -Be 0
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -Times 0 -Exactly -ParameterFilter {
            ($Arguments -join ' ') -like 'storage blob metadata show *'
        }
    }

    It 'writes records to the pipeline with the .NET field names' {
        $records = @(Get-FlowLogRecord -StorageAccountName 'stflowlogs01')
        $records.Count | Should -Be 5
        $first = $records[0]
        ($first.PSObject.Properties.Name -join ',') | Should -Be ('time,category,operationName,resourceId,version,nsgRuleName,mac,startTime,sourceAddress,destinationAddress,sourcePort,destinationPort,transportProtocol,deviceDirection,deviceAction,flowState,packetsStoD,bytesStoD,packetsDtoS,bytesDtoS')
        $first.nsgRuleName | Should -Be 'DefaultRule_AllowInternetOutBound'
        $first.mac | Should -Be '001122334455'
        $first.sourceAddress | Should -Be '10.10.1.4'
        $first.destinationPort | Should -Be '443'
        $first.transportProtocol | Should -Be 'T'
        $first.bytesStoD | Should -Be '100'
    }

    It 'writes a JSON array to -OutputPath, and one compact object per line for Jsonl' {
        $jsonPath = Join-Path $TestDrive 'records.json'
        $file = Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -OutputPath $jsonPath
        $file.FullName | Should -Be $jsonPath
        $text = [System.IO.File]::ReadAllText($jsonPath)
        $text.TrimStart().StartsWith('[') | Should -BeTrue
        @(ConvertFrom-Json -InputObject $text).Count | Should -Be 5

        $linesPath = Join-Path $TestDrive 'records.jsonl'
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -OutputPath $linesPath -Format Jsonl | Out-Null
        $lines = @([System.IO.File]::ReadAllLines($linesPath) | Where-Object { $_ })
        $lines.Count | Should -Be 5
        $lines[0].StartsWith('{') | Should -BeTrue
        ($lines[0] -split "`n").Count | Should -Be 1
        (ConvertFrom-Json -InputObject $lines[4]).sourceAddress | Should -Not -BeNullOrEmpty
    }

    It 'a download failure warns and processing continues with the next blob' {
        $script:DownloadFailure = @('blob-a')
        $records = @(Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -WarningVariable warnings -WarningAction SilentlyContinue)
        ($warnings -join ' ') | Should -Match 'Could not download blob-a'
        $records.Count | Should -Be 3
        @($script:Writes | ForEach-Object { $_.Name }) | Should -Be @('blob-b')
    }

    It 'a metadata write failure warns and keeps the records' {
        $script:WriteFailure = @('blob-a')
        $records = @(Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -WarningVariable warnings -WarningAction SilentlyContinue)
        ($warnings -join ' ') | Should -Match 'Could not write processing metadata on blob-a'
        $records.Count | Should -Be 5
    }

    It 'warns and reads nothing when the container is missing' {
        $script:ContainerExists = $false
        $records = @(Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -WarningVariable warnings -WarningAction SilentlyContinue)
        ($warnings -join ' ') | Should -Match 'Container insights-logs-flowlogflowevent not found'
        $records.Count | Should -Be 0
        $script:Downloads.Count | Should -Be 0
    }

    It 'uses the flow-log container by default and the given prefix when one is set' {
        Get-FlowLogRecord -StorageAccountName 'stflowlogs01' -Prefix 'resourceId=/SUBSCRIPTIONS/' | Out-Null
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -ParameterFilter {
            ($Arguments -join ' ') -like 'storage blob list * --container-name insights-logs-flowlogflowevent --prefix=resourceId=/SUBSCRIPTIONS/ *'
        }
    }
}

Describe 'Record denormalization and the processing rule' {
    BeforeAll {
        $script:V1Json = '{"records":[{"time":"2026-10-19T09:00:00Z","category":"C","operationName":"O","resourceId":"R","properties":{"Version":1},' +
            '"flowRecords":{"flows":[{"rule":"Rule1","flowGroups":[{"mac":"AA","flowTuples":["1729328400,10.0.0.4,8.8.8.8,1,53,U,O,A"]}]}]}}]}'
    }

    It 'omits the version 2 fields for version 1 tuples' {
        $records = InModuleScope AzureFlowLogParser -Parameters @{ Json = $script:V1Json } {
            param($Json)
            ConvertTo-FlowLogRecord -Json $Json
        }
        $records.Count | Should -Be 1
        ($records[0].PSObject.Properties.Name -contains 'flowState') | Should -BeFalse
        $records[0].version | Should -Be 1
        $records[0].startTime | Should -Be '1729328400'
        $records[0].mac | Should -Be 'AA'
    }

    It 'keeps the time text and the version 2 counts, with empty counts as 0' {
        $json = '{"records":[{"time":"2026-10-19T09:00:00Z","properties":{"Version":2},"flowRecords":{"flows":[{"rule":"R","flowGroups":[{"mac":"AA","flowTuples":["1729328400,10.0.0.4,8.8.8.8,1,53,U,O,A,B,,,,"]}]}]}}]}'
        $record = InModuleScope AzureFlowLogParser -Parameters @{ Json = $json } {
            param($Json)
            ConvertTo-FlowLogRecord -Json $Json
        }
        $record[0].time | Should -Be '2026-10-19T09:00:00Z'
        $record[0].flowState | Should -Be 'B'
        $record[0].packetsStoD | Should -Be '0'
        $record[0].bytesDtoS | Should -Be '0'
    }

    It 'processes when the processed time is not a date' {
        Mock -ModuleName AzureFlowLogParser Invoke-FlowLogAzCli { [pscustomobject]@{ lastProcessed = 'x'; processedBlobLastModified = 'not a date' } }
        $needs = InModuleScope AzureFlowLogParser {
            Test-FlowLogBlobNeedsProcessing -Context ([pscustomobject]@{ Name = 'stflowlogs01' }) -Container 'c' -Name 'b' -LastModifiedUtc ([datetime]'2026-10-19T09:00:00Z')
        }
        $needs | Should -BeTrue
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -Times 1 -Exactly
    }

    It 'skips when the blob is not newer than the processed time' {
        Mock -ModuleName AzureFlowLogParser Invoke-FlowLogAzCli { [pscustomobject]@{ lastProcessed = 'x'; processedBlobLastModified = '2026-10-19T09:00:00.0000000+00:00' } }
        $needs = InModuleScope AzureFlowLogParser {
            Test-FlowLogBlobNeedsProcessing -Context ([pscustomobject]@{ Name = 'stflowlogs01' }) -Container 'c' -Name 'b' -LastModifiedUtc ([datetime]'2026-10-19T09:00:00Z')
        }
        $needs | Should -BeFalse
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -Times 1 -Exactly
    }
}
