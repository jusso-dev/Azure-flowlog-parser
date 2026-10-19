#Requires -Version 7.6

# Discovery-time switches: the template check needs the shared file, the PDF validity check needs qpdf.
$script:HasSharedTemplate = Test-Path -LiteralPath (Join-Path $PSScriptRoot '..' '..' 'shared' 'report-template.html') -PathType Leaf
$script:HasQpdf = [bool](Get-Command qpdf -ErrorAction SilentlyContinue)

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..' 'AzureFlowLogParser' 'AzureFlowLogParser.psd1') -Force
    Import-Module (Join-Path $PSScriptRoot '..' 'AzureFlowLogReport' 'AzureFlowLogReport.psd1') -Force
    $script:SamplePath = Join-Path $PSScriptRoot '..' '..' 'sample-flowlog.json'

    # Azure-shaped model: storage scope, monitoring rows, dedupe note and a configurable number of flow rows.
    function New-TestModel {
        param([int]$FlowRows = 3, [string[]]$Notes = @('Counting: test note.'))
        $flows = @(for ($i = 0; $i -lt $FlowRows; $i++) {
            [pscustomobject]@{
                first = '2026-10-19T10:00:00Z'; last = '2026-10-19T10:01:00Z'
                src = "10.0.1.$($i % 250)"; dst = '13.107.42.14'
                srcScope = 'stflowa01'; dstScope = 'internet'
                proto = 'TCP'; dstPort = 443; action = 'A'; direction = 'O'
                rule = 'DefaultRule_AllowInternetOutBound'; flows = 2; packets = 10; bytes = 10000 - $i
            }
        })
        [pscustomobject]@{
            schema = 1
            generatedUtc = '2026-10-19T10:05:00Z'
            source = 'azure'
            subscriptions = @([pscustomobject]@{ subscriptionId = '11111111-1111-1111-1111-111111111111'; displayName = 'Prod (test)'; tenantId = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa' })
            window = [pscustomobject]@{ fromUtc = '2026-10-18T10:05:00Z'; toUtc = '2026-10-19T10:05:00Z' }
            filters = [pscustomobject]@{ action = 'all'; protocol = 'all'; ports = '443,8000-8100'; ip = ''; minBytes = 0 }
            summary = [pscustomobject]@{
                flows = 2 * $FlowRows; allowed = 2 * $FlowRows; denied = 0; bytes = 10000; packets = 10 * $FlowRows
                rows = $FlowRows; rowsTotal = $FlowRows; rowsTruncated = $false
                storageAccounts = 2; scopes = 2; subscriptions = 1
                observationsSkipped = 1; uniqueSourceIps = 4; uniqueDestinationIps = 1
            }
            scopes = @(
                [pscustomobject]@{ id = 'internet'; kind = 'internet'; label = 'Internet'; subscriptionId = ''; resourceGroup = ''; storageAccount = '' }
                [pscustomobject]@{ id = 'stflowa01'; kind = 'storage'; label = 'stflowa01'; subscriptionId = '11111111-1111-1111-1111-111111111111'; resourceGroup = 'rg-net'; storageAccount = 'stflowa01' }
            )
            flows = $flows
            accounts = @(
                [pscustomobject]@{ subscriptionId = '11111111-1111-1111-1111-111111111111'; name = 'stflowa01'; resourceGroup = 'rg-net'; location = 'australiaeast'; kind = 'StorageV2'; skuName = 'Standard_LRS'; flowLogConfigs = 1; containerPresent = $true; hasFlowLogs = $true; status = 'Healthy'; latestBlobUtc = '2026-10-19T09:30:00Z'; observedTarget = $null }
                [pscustomobject]@{ subscriptionId = '11111111-1111-1111-1111-111111111111'; name = 'stflowb01'; resourceGroup = 'rg-net'; location = 'australiaeast'; kind = 'StorageV2'; skuName = 'Standard_LRS'; flowLogConfigs = 1; containerPresent = $true; hasFlowLogs = $true; status = 'Stale'; latestBlobUtc = '2026-10-19T05:00:00Z'; observedTarget = $null }
            )
            flowLogs = @(
                [pscustomobject]@{ subscriptionId = '11111111-1111-1111-1111-111111111111'; name = 'fl-a'; storageAccount = 'stflowa01'; networkWatcher = 'nw'; location = 'australiaeast'; target = '/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-a'; targetKind = 'NSG'; targetName = 'nsg-a'; targetResourceGroup = 'rg-net'; enabled = $true; retentionDays = 7; trafficAnalytics = $false; status = 'Healthy'; latestBlobUtc = '2026-10-19T09:30:00Z'; ageHours = 0.5; message = '' }
                [pscustomobject]@{ subscriptionId = '11111111-1111-1111-1111-111111111111'; name = 'fl-b'; storageAccount = 'stflowb01'; networkWatcher = 'nw'; location = 'australiaeast'; target = '/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/virtualNetworks/vnet1'; targetKind = 'VNet'; targetName = 'vnet1'; targetResourceGroup = 'rg-net'; enabled = $true; retentionDays = 0; trafficAnalytics = $true; status = 'Stale'; latestBlobUtc = '2026-10-19T05:00:00Z'; ageHours = 5.08; message = '' }
            )
            notes = $Notes
        }
    }

    function Get-PdfXrefCheck {
        # Returns the byte offset of 'startxref' target and whether every xref entry points at "N 0 obj".
        param([byte[]]$Bytes)
        $text = [System.Text.Encoding]::Latin1.GetString($Bytes)
        $startxref = [regex]::Match($text, 'startxref\n(\d+)\n%%EOF\n$')
        $xrefOffset = [int]$startxref.Groups[1].Value
        $table = [regex]::Match($text, '(?s)xref\n0 (\d+)\n(.*?)trailer')
        $entries = [regex]::Matches($table.Groups[2].Value, '(\d{10}) (\d{5}) ([fn]) \n')
        $allMatch = $true
        for ($i = 1; $i -lt $entries.Count; $i++) {
            $offset = [int]$entries[$i].Groups[1].Value
            if (-not $text.Substring($offset).StartsWith("$i 0 obj")) { $allMatch = $false }
        }
        [pscustomobject]@{
            XrefKeyword = $text.Substring($xrefOffset, 4)
            EntryCount = [int]$table.Groups[1].Value
            EntriesMatch = $allMatch
            Eof = $text.EndsWith("%%EOF`n")
        }
    }
}

Describe 'Module surface' {
    It 'exports only Export-FlowLogReport' {
        (Get-Command -Module AzureFlowLogReport).Name | Should -Be 'Export-FlowLogReport'
    }

    It 'declares PowerShell 7.6 Core and comment-based help' {
        $manifest = Import-PowerShellDataFile (Join-Path $PSScriptRoot '..' 'AzureFlowLogReport' 'AzureFlowLogReport.psd1')
        $manifest.PowerShellVersion | Should -Be '7.6'
        $manifest.CompatiblePSEditions | Should -Contain 'Core'
        (Get-Help Export-FlowLogReport).Synopsis | Should -Not -BeNullOrEmpty
    }
}

Describe 'HTML template' {
    It 'matches shared/report-template.html byte for byte' -Skip:(-not $script:HasSharedTemplate) {
        $module = Join-Path $PSScriptRoot '..' 'AzureFlowLogReport' 'Templates' 'report-template.html'
        $shared = Join-Path $PSScriptRoot '..' '..' 'shared' 'report-template.html'
        (Get-FileHash -LiteralPath $module -Algorithm SHA256).Hash | Should -Be (Get-FileHash -LiteralPath $shared -Algorithm SHA256).Hash
    }

    It 'contains the single data placeholder' {
        $template = Join-Path $PSScriptRoot '..' 'AzureFlowLogReport' 'Templates' 'report-template.html'
        ([regex]::Matches([System.IO.File]::ReadAllText($template), '__FLOWLOG_DATA__')).Count | Should -Be 1
    }
}

Describe 'HTML output' {
    It 'embeds the model as parseable JSON in the data script tag' {
        $path = Join-Path $TestDrive 'report.html'
        Get-FlowLogTraffic -InputPath $script:SamplePath | Export-FlowLogReport -Format Html -Path $path | Out-Null
        $bytes = [System.IO.File]::ReadAllBytes($path)
        ($bytes[0..2] -join ',') | Should -Not -Be '239,187,191'   # no UTF-8 BOM

        $html = [System.IO.File]::ReadAllText($path)
        $match = [regex]::Match($html, '<script id="flowlog-data" type="application/json">(.*?)</script>', 'Singleline')
        $match.Success | Should -BeTrue
        $html.Contains('__FLOWLOG_DATA__') | Should -BeFalse
        $data = $match.Groups[1].Value.Replace('<\/', '</') | ConvertFrom-Json -DateKind String
        $data.schema | Should -Be 1
        $data.summary.flows | Should -Be 6
    }

    It 'embeds the subscriptions and the flow-log target fields' {
        $path = Join-Path $TestDrive 'fields.html'
        New-TestModel | Export-FlowLogReport -Format Html -Path $path | Out-Null
        $match = [regex]::Match([System.IO.File]::ReadAllText($path), '<script id="flowlog-data" type="application/json">(.*?)</script>', 'Singleline')
        $data = $match.Groups[1].Value.Replace('<\/', '</') | ConvertFrom-Json -DateKind String
        $data.subscriptions[0].subscriptionId | Should -Be '11111111-1111-1111-1111-111111111111'
        $data.subscriptions[0].displayName | Should -Be 'Prod (test)'
        $data.subscriptions[0].tenantId | Should -Be 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
        $data.flowLogs[0].targetName | Should -Be 'nsg-a'
        $data.flowLogs[0].targetResourceGroup | Should -Be 'rg-net'
    }

    It 'escapes </ inside the embedded JSON so the script element cannot close early' {
        $path = Join-Path $TestDrive 'escaped.html'
        New-TestModel -Notes @('payload </script><b>x</b>') | Export-FlowLogReport -Format Html -Path $path | Out-Null
        $html = [System.IO.File]::ReadAllText($path)
        $html.Contains('</script><b>x') | Should -BeFalse
        $html.Contains('<\/script><b>x<\/b>') | Should -BeTrue
    }

    It 'rejects a model that is not schema 1' {
        { [pscustomobject]@{ schema = 2 } | Export-FlowLogReport -Format Html -Path (Join-Path $TestDrive 'x.html') } | Should -Throw '*schema 1*'
    }

    It 'fails clearly when the output folder does not exist' {
        { New-TestModel | Export-FlowLogReport -Format Html -Path (Join-Path $TestDrive 'missing' 'x.html') } | Should -Throw '*Output folder not found*'
    }
}

Describe 'PDF output structure' {
    It 'writes a PDF 1.4 header, an xref table that matches the objects, and %%EOF' {
        $path = Join-Path $TestDrive 'structure.pdf'
        New-TestModel | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        $bytes = [System.IO.File]::ReadAllBytes($path)
        [System.Text.Encoding]::Latin1.GetString($bytes, 0, 8) | Should -Be '%PDF-1.4'

        $check = Get-PdfXrefCheck -Bytes $bytes
        $check.XrefKeyword | Should -Be 'xref'
        $check.EntriesMatch | Should -BeTrue
        $check.Eof | Should -BeTrue
        $check.EntryCount | Should -BeGreaterThan 5
    }

    It 'keeps all text ASCII apart from the binary comment bytes' {
        $path = Join-Path $TestDrive 'ascii.pdf'
        New-TestModel -Notes @('Caf' + [char]0x00E9 + ' ' + [char]0x2013 + ' ' + [char]0x201C + 'quoted' + [char]0x201D + ' (paren) \ slash') | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))
        $body = $text.Substring(16)
        [regex]::IsMatch($body, '[^\x09\x0A\x0D\x20-\x7E]') | Should -BeFalse
        $text.Contains('Caf?') | Should -BeTrue
        $text.Contains('\(paren\)') | Should -BeTrue
        $text.Contains('\\ slash') | Should -BeTrue
    }

    It 'breaks long tables across pages with a repeated header' {
        $path = Join-Path $TestDrive 'long.pdf'
        New-TestModel -FlowRows 200 | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))
        ([regex]::Matches($text, '/Type /Page\b')).Count | Should -BeGreaterThan 2
        ([regex]::Matches($text, '\(Top flows') ).Count | Should -Be 1
        ([regex]::Matches($text, '\(Source\)')).Count | Should -BeGreaterThan 1
    }

    It 'renders a local-mode model with no accounts or flow logs' {
        $path = Join-Path $TestDrive 'local.pdf'
        Get-FlowLogTraffic -InputPath $script:SamplePath | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))
        $text.Contains('No storage accounts with flow logs') | Should -BeTrue
        $text.Contains("Local input: no Azure topology was loaded. Public addresses are 'internet'") | Should -BeTrue
        $text.Contains('(Tenant)') | Should -BeTrue
        $text.Contains('not available \(offline input\)') | Should -BeTrue
    }

    It 'shows the tenant and subscription rows in the summary' {
        $path = Join-Path $TestDrive 'identity.pdf'
        New-TestModel | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))
        $text.Contains('(aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa)') | Should -BeTrue
        $text.Contains('(Subscriptions)') | Should -BeTrue
        $text.Contains('Prod \(test\) \(11111111-1111-1111-1111-111111111111\)') | Should -BeTrue
        $text.IndexOf('(Tenant)') | Should -BeLessThan $text.IndexOf('(Subscriptions)')
        $text.IndexOf('(Subscriptions)') | Should -BeLessThan $text.IndexOf('(Flow observations)')
    }

    It 'writes the .NET summary rows in order, with the same value formats' {
        $path = Join-Path $TestDrive 'summary-rows.pdf'
        New-TestModel | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))
        # New-TestModel: 6 observations, 6 allowed, 0 denied, 10000 bytes, 30 packets, 1 skipped, 4 source and 1 destination IPs.
        $labels = @(
            'Tenant', 'Subscriptions', 'Flow observations', 'Allowed / denied', 'Bytes / packets',
            'Storage accounts read', 'Scopes in report', 'Unique source / destination IPs', 'Allowed inbound duplicates skipped'
        )
        $positions = foreach ($label in $labels) { $text.IndexOf("($label)") }
        $positions | ForEach-Object { $_ | Should -BeGreaterThan -1 }
        for ($i = 1; $i -lt $positions.Count; $i++) { $positions[$i] | Should -BeGreaterThan $positions[$i - 1] }
        $text.Contains('(6 / 0)') | Should -BeTrue
        $text.Contains('(9.8 KB / 30)') | Should -BeTrue
        $text.Contains('(4 / 1)') | Should -BeTrue
        $text.Contains('(Flows \(observations\))') | Should -BeFalse
        $text.Contains('(Flow rows listed') | Should -BeFalse
        $text.Contains('(Subscriptions scanned)') | Should -BeFalse
    }

    It 'formats bytes as the .NET report does' {
        InModuleScope AzureFlowLogReport {
            Format-PdfBytes -Bytes 512 | Should -Be '512 B'
            Format-PdfBytes -Bytes 1536 | Should -Be '1.5 KB'
            Format-PdfBytes -Bytes 1048576 | Should -Be '1.0 MB'
            Format-PdfBytes -Bytes 15889587 | Should -Be '15.2 MB'
        }
    }

    It 'uses the contract headers for the monitoring tables' {
        $path = Join-Path $TestDrive 'monitoring.pdf'
        New-TestModel | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))
        foreach ($header in 'Resource group', 'Latest blob', 'Flow log', 'Target RG', 'Configs', 'Enabled') {
            $text.Contains("($header)") | Should -BeTrue
        }
        # 'Unmanaged source' is wider than its 60 pt column, so it wraps onto two header lines.
        $text.Contains('(Unmanaged)') | Should -BeTrue
        $text.Contains('(source)') | Should -BeTrue
        $text.Contains('(Age)') | Should -BeTrue
        $text.Contains('(Latest blob \(UTC\))') | Should -BeFalse
        # The 16-character timestamp is shown whole, as the .NET report does.
        $text.Contains('(2026-10-19 09:30)') | Should -BeTrue
        $text.Contains('(nsg-a)') | Should -BeTrue
        $text.Contains('(rg-net)') | Should -BeTrue
        $text.Contains('(Prod \(test\))') | Should -BeTrue
    }

    It 'renders an older model without the subscriptions and target fields' {
        $path = Join-Path $TestDrive 'older.pdf'
        $model = New-TestModel
        $model.PSObject.Properties.Remove('subscriptions')
        foreach ($flowLog in $model.flowLogs) {
            $flowLog.PSObject.Properties.Remove('targetName')
            $flowLog.PSObject.Properties.Remove('targetResourceGroup')
        }
        { $model | Export-FlowLogReport -Format Pdf -Path $path } | Should -Not -Throw
        $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))
        $text.Contains('not available \(offline input\)') | Should -BeTrue
        $text.Contains('(vnet1)') | Should -BeTrue
    }
}

Describe 'PDF validity (qpdf)' {
    It 'passes qpdf --check for the sample report' -Skip:(-not $script:HasQpdf) {
        $path = Join-Path $TestDrive 'qpdf-sample.pdf'
        Get-FlowLogTraffic -InputPath $script:SamplePath | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        & qpdf --check $path *> $null
        $LASTEXITCODE | Should -Be 0
    }

    It 'passes qpdf --check for an Azure-shaped multi-page report' -Skip:(-not $script:HasQpdf) {
        $path = Join-Path $TestDrive 'qpdf-azure.pdf'
        New-TestModel -FlowRows 200 | Export-FlowLogReport -Format Pdf -Path $path | Out-Null
        & qpdf --check $path *> $null
        $LASTEXITCODE | Should -Be 0
    }
}
