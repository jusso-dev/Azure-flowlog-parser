#Requires -Version 7.6

BeforeAll {
    $script:ModulePath = Join-Path $PSScriptRoot '..' 'AzureFlowLogParser' 'AzureFlowLogParser.psd1'
    $script:SamplePath = Join-Path $PSScriptRoot '..' '..' 'sample-flowlog.json'
    Import-Module $script:ModulePath -Force

    $script:Sub = '11111111-1111-1111-1111-111111111111'
    $script:NsgA = "/subscriptions/$($script:Sub)/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-a"
    $script:NsgB = "/subscriptions/$($script:Sub)/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-b"
    $script:VnetId = "/subscriptions/$($script:Sub)/resourceGroups/rg-net/providers/Microsoft.Network/virtualNetworks/vnet1"

    # Two NSGs with flow logs to two storage accounts, one VNet with two subnets and a NIC in each.
    function Get-TestArmData {
        [pscustomobject]@{
            SubscriptionId    = $script:Sub
            StorageAccounts   = @(
                [pscustomobject]@{ SubscriptionId = $script:Sub; Name = 'stflowa01'; ResourceGroup = 'rg-net'; Location = 'australiaeast'; Kind = 'StorageV2'; SkuName = 'Standard_LRS' }
                [pscustomobject]@{ SubscriptionId = $script:Sub; Name = 'stflowb01'; ResourceGroup = 'rg-net'; Location = 'australiaeast'; Kind = 'StorageV2'; SkuName = 'Standard_LRS' }
            )
            FlowLogs          = @(
                [pscustomobject]@{ SubscriptionId = $script:Sub; Name = 'fl-a'; NetworkWatcher = 'NetworkWatcher_australiaeast'; Location = 'australiaeast'; Target = $script:NsgA; TargetKind = 'NSG'; Enabled = $true; RetentionDays = 7; TrafficAnalytics = $false; StorageAccount = 'stflowa01'; StorageSubscriptionId = $script:Sub; StorageResourceGroup = 'rg-net' }
                [pscustomobject]@{ SubscriptionId = $script:Sub; Name = 'fl-b'; NetworkWatcher = 'NetworkWatcher_australiaeast'; Location = 'australiaeast'; Target = $script:NsgB; TargetKind = 'NSG'; Enabled = $true; RetentionDays = 7; TrafficAnalytics = $false; StorageAccount = 'stflowb01'; StorageSubscriptionId = $script:Sub; StorageResourceGroup = 'rg-net' }
            )
            VirtualNetworks   = @(
                [pscustomobject]@{
                    SubscriptionId  = $script:Sub; Name = 'vnet1'; ResourceGroup = 'rg-net'; Id = $script:VnetId; Location = 'australiaeast'
                    AddressPrefixes = @('10.0.0.0/16')
                    Subnets         = @(
                        [pscustomobject]@{ Name = 'snet-a'; Id = "$($script:VnetId)/subnets/snet-a"; AddressPrefixes = @('10.0.1.0/24'); NsgId = $script:NsgA }
                        [pscustomobject]@{ Name = 'snet-b'; Id = "$($script:VnetId)/subnets/snet-b"; AddressPrefixes = @('10.0.2.0/24'); NsgId = $script:NsgB }
                    )
                }
            )
            NetworkInterfaces = @(
                [pscustomobject]@{ Name = 'nic-a'; Id = '/nics/nic-a'; NsgId = $script:NsgA; PrivateIps = @('10.0.1.4') }
                [pscustomobject]@{ Name = 'nic-b'; Id = '/nics/nic-b'; NsgId = $script:NsgB; PrivateIps = @('10.0.2.10') }
            )
        }
    }

    function New-TestBlobEntry {
        param([string]$Name, [datetime]$LastModifiedUtc)
        [pscustomobject]@{ Name = $Name; LastModifiedUtc = $LastModifiedUtc; Length = 100 }
    }
}

Describe 'Resolve-FlowLogWindow' {
    It 'resolves <Since> relative to until' -ForEach @(
        @{ Since = '24h'; Expected = { param($u) $u.AddHours(-24) } }
        @{ Since = '7d'; Expected = { param($u) $u.AddDays(-7) } }
        @{ Since = '90m'; Expected = { param($u) $u.AddMinutes(-90) } }
    ) {
        InModuleScope AzureFlowLogParser -Parameters @{ Since = $Since; Expected = $Expected } {
            param($Since, $Expected)
            $until = [datetime]::new(2026, 10, 19, 10, 0, 0, [System.DateTimeKind]::Utc)
            $window = Resolve-FlowLogWindow -Since $Since -UntilUtc $until
            $window.FromUtc | Should -Be (& $Expected $until)
            $window.ToUtc | Should -Be $until
        }
    }

    It 'accepts an ISO-8601 UTC start' {
        InModuleScope AzureFlowLogParser {
            $until = [datetime]::new(2026, 10, 19, 10, 0, 0, [System.DateTimeKind]::Utc)
            $window = Resolve-FlowLogWindow -Since '2026-10-18T09:30:00Z' -UntilUtc $until
            $window.FromUtc | Should -Be ([datetime]::new(2026, 10, 18, 9, 30, 0, [System.DateTimeKind]::Utc))
        }
    }

    It 'rejects an unknown unit' {
        InModuleScope AzureFlowLogParser {
            { Resolve-FlowLogWindow -Since '3w' -UntilUtc ([datetime]::UtcNow) } | Should -Throw '*Invalid -Since*'
        }
    }

    It 'rejects a start at or after until' {
        InModuleScope AzureFlowLogParser {
            { Resolve-FlowLogWindow -Since '0h' -UntilUtc ([datetime]::UtcNow) } | Should -Throw '*earlier than -Until*'
        }
    }
}

Describe 'Port filter' {
    It 'matches listed ports and inclusive ranges' {
        InModuleScope AzureFlowLogParser {
            $filter = ConvertTo-FlowLogPortFilter -Ports '443,8000-8100'
            Test-FlowLogPort -Filter $filter -Port 443 | Should -BeTrue
            Test-FlowLogPort -Filter $filter -Port 8000 | Should -BeTrue
            Test-FlowLogPort -Filter $filter -Port 8100 | Should -BeTrue
            Test-FlowLogPort -Filter $filter -Port 80 | Should -BeFalse
            Test-FlowLogPort -Filter $filter -Port 8101 | Should -BeFalse
        }
    }

    It 'matches everything when no ports are given' {
        InModuleScope AzureFlowLogParser {
            Test-FlowLogPort -Filter (ConvertTo-FlowLogPortFilter -Ports '') -Port 1 | Should -BeTrue
        }
    }

    It 'rejects <Ports>' -ForEach @(
        @{ Ports = '70000' }
        @{ Ports = '9-3' }
        @{ Ports = 'https' }
    ) {
        InModuleScope AzureFlowLogParser -Parameters @{ Ports = $Ports } {
            param($Ports)
            { ConvertTo-FlowLogPortFilter -Ports $Ports } | Should -Throw
        }
    }
}

Describe 'IPv4 parsing and prefix match' {
    It 'converts dotted quads to big-endian numbers' {
        InModuleScope AzureFlowLogParser {
            ConvertTo-FlowLogIPv4Number -Address '1.2.3.4' | Should -Be 16909060
        }
    }

    It 'returns null for <Address>' -ForEach @(
        @{ Address = '300.1.1.1' }
        @{ Address = '10.1' }
        @{ Address = 'not-an-ip' }
    ) {
        InModuleScope AzureFlowLogParser -Parameters @{ Address = $Address } {
            param($Address)
            ConvertTo-FlowLogIPv4Number -Address $Address | Should -BeNullOrEmpty
        }
    }

    It 'classifies private ranges as private' {
        InModuleScope AzureFlowLogParser {
            foreach ($address in '10.9.9.9', '172.20.1.1', '192.168.1.1', '100.64.0.1', '169.254.1.1') {
                Test-FlowLogPrivateNumber -Number (ConvertTo-FlowLogIPv4Number -Address $address) | Should -BeTrue
            }
            Test-FlowLogPrivateNumber -Number (ConvertTo-FlowLogIPv4Number -Address '13.107.42.14') | Should -BeFalse
        }
    }

    It 'applies the longest matching prefix' {
        InModuleScope AzureFlowLogParser -Parameters @{ Arm = (Get-TestArmData) } {
            param($Arm)
            $notes = [System.Collections.Generic.List[string]]::new()
            $topology = New-FlowLogTopology -ArmData @($Arm) -Notes $notes
            $state = New-FlowLogState -Window $null -Filter ([pscustomobject]@{ ActionCode = $null; ProtocolCode = $null; Ports = (ConvertTo-FlowLogPortFilter -Ports ''); IpFilter = ''; MinBytes = 0 })
            $state.Topology = $topology

            (Resolve-FlowLogScope -State $state -Address '10.0.1.4').id | Should -Be 'stflowa01'   # NIC /32 on NSG A
            (Resolve-FlowLogScope -State $state -Address '10.0.1.9').id | Should -Be 'stflowa01'   # subnet A
            (Resolve-FlowLogScope -State $state -Address '10.0.2.10').id | Should -Be 'stflowb01'  # NIC /32 on NSG B
            (Resolve-FlowLogScope -State $state -Address '10.0.2.99').id | Should -Be 'stflowb01'  # subnet B
            (Resolve-FlowLogScope -State $state -Address '10.0.3.5').id | Should -Be 'vnet:rg-net/vnet1'
            (Resolve-FlowLogScope -State $state -Address '10.0.3.5').kind | Should -Be 'vnet'
        }
    }

    It 'maps unmatched addresses to internet, private-unmapped or unmapped' {
        InModuleScope AzureFlowLogParser {
            $state = New-FlowLogState -Window $null -Filter ([pscustomobject]@{ ActionCode = $null; ProtocolCode = $null; Ports = (ConvertTo-FlowLogPortFilter -Ports ''); IpFilter = ''; MinBytes = 0 })
            (Resolve-FlowLogScope -State $state -Address '13.107.42.14').id | Should -Be 'internet'
            (Resolve-FlowLogScope -State $state -Address '192.168.1.1').id | Should -Be 'unmapped'
            (Resolve-FlowLogScope -State $state -Address 'garbage').id | Should -Be 'unmapped'
        }
    }
}

Describe 'Local tuple parsing (sample-flowlog.json)' {
    BeforeAll {
        # Independent count from the raw JSON, so the expectations do not depend on the parser under test.
        $sample = Get-Content -LiteralPath $script:SamplePath -Raw | ConvertFrom-Json
        $script:SampleTuples = @(foreach ($record in $sample.records) {
            foreach ($flow in $record.flowRecords.flows) {
                foreach ($group in $flow.flowGroups) {
                    foreach ($tuple in $group.flowTuples) { $tuple }
                }
            }
        })
        $script:SampleModel = Get-FlowLogTraffic -InputPath $script:SamplePath
    }

    It 'counts one observation per flow tuple' {
        $script:SampleModel.summary.flows | Should -Be $script:SampleTuples.Count
        $script:SampleTuples.Count | Should -Be 6
    }

    It 'splits allowed and denied by the action field' {
        $allowed = @($script:SampleTuples | Where-Object { $_.Split(',')[7] -eq 'A' }).Count
        $script:SampleModel.summary.allowed | Should -Be $allowed
        $script:SampleModel.summary.denied | Should -Be ($script:SampleTuples.Count - $allowed)
    }

    It 'sums bytes and packets from the version 2 counters' {
        $bytes = 0
        $packets = 0
        foreach ($tuple in $script:SampleTuples) {
            $parts = $tuple.Split(',')
            $bytes += [long]$parts[10] + [long]$parts[12]
            $packets += [long]$parts[9] + [long]$parts[11]
        }
        $script:SampleModel.summary.bytes | Should -Be $bytes
        $script:SampleModel.summary.packets | Should -Be $packets
    }

    It 'aggregates identical keys into one row and sorts by bytes' {
        $script:SampleModel.flows.Count | Should -Be 4
        $script:SampleModel.flows[0].bytes | Should -Be 8500
        $script:SampleModel.flows[0].flows | Should -Be 3
        $script:SampleModel.flows[0].proto | Should -Be 'TCP'
        $script:SampleModel.flows[0].action | Should -Be 'A'
    }

    It 'reports local source with no accounts or flow logs' {
        $script:SampleModel.source | Should -Be 'local'
        $script:SampleModel.summary.storageAccounts | Should -Be 0
        $script:SampleModel.accounts.Count | Should -Be 0
        $script:SampleModel.flowLogs.Count | Should -Be 0
    }

    It 'keeps the ReportModel contract under JSON round trip' {
        # -DateKind String keeps timestamps as the strings the contract defines.
        $json = $script:SampleModel | ConvertTo-Json -Depth 12 | ConvertFrom-Json -DateKind String
        $json.schema | Should -Be 1
        $json.window.fromUtc | Should -Match '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$'
        $json.flows[0].first | Should -Match 'Z$'
        $json.summary.rowsTruncated | Should -BeFalse
    }

    It 'applies action, protocol, port, ip and min-bytes filters' {
        (Get-FlowLogTraffic -InputPath $script:SamplePath -Action Deny).summary.flows | Should -Be 1
        (Get-FlowLogTraffic -InputPath $script:SamplePath -Protocol UDP).summary.flows | Should -Be 0
        (Get-FlowLogTraffic -InputPath $script:SamplePath -Port '22,3389').summary.flows | Should -Be 2
        (Get-FlowLogTraffic -InputPath $script:SamplePath -IpFilter '10.0.2').summary.flows | Should -Be 1
        (Get-FlowLogTraffic -InputPath $script:SamplePath -MinBytes 1000).summary.flows | Should -Be 4
    }

    It 'filters by window only when -Since or -Until is given' {
        $window = Get-FlowLogTraffic -InputPath $script:SamplePath -Until '2024-01-15T11:31:00Z' -Since '2024-01-15T11:30:30Z'
        $window.summary.flows | Should -Be 1
        $window.window.fromUtc | Should -Be '2024-01-15T11:30:30Z'
    }

    It 'treats version 1 tuples as having zero counters' {
        $path = Join-Path $TestDrive 'v1.json'
        '{"records":[{"properties":{"Version":1},"flowRecords":{"flows":[{"rule":"R","flowGroups":[{"flowTuples":["1705318200,10.0.1.4,8.8.8.8,1,53,U,O,A"]}]}]}}]}' | Set-Content -LiteralPath $path
        $model = Get-FlowLogTraffic -InputPath $path
        $model.summary.flows | Should -Be 1
        $model.summary.bytes | Should -Be 0
        $model.flows[0].proto | Should -Be 'UDP'
    }

    It 'skips and counts malformed tuples' {
        $path = Join-Path $TestDrive 'bad.json'
        '{"records":[{"properties":{"Version":2},"flowRecords":{"flows":[{"rule":"R","flowGroups":[{"flowTuples":["garbage","1705318200,10.0.1.4,8.8.8.8,1,53,U,O,A,,,,,"]}]}]}}]}' | Set-Content -LiteralPath $path
        $model = Get-FlowLogTraffic -InputPath $path
        $model.summary.flows | Should -Be 1
        ($model.notes -match 'Malformed tuples skipped: 1').Count | Should -Be 1
    }

    It 'rejects -InputPath combined with Azure selectors' {
        { Get-FlowLogTraffic -InputPath $script:SamplePath -SubscriptionId $script:Sub } | Should -Throw '*cannot be combined*'
    }

    It 'never calls Azure in local mode' {
        Mock -ModuleName AzureFlowLogParser Get-FlowLogAzSubscription { throw 'Azure must not be called' }
        { Get-FlowLogTraffic -InputPath $script:SamplePath | Out-Null } | Should -Not -Throw
    }
}

Describe 'Dedupe rule' {
    It 'skips <Case>' -ForEach @(
        @{ Case = 'inbound allowed from a read storage scope'; Tuple = '1705318200,10.0.1.5,10.0.2.5,1,443,T,I,A,B,1,10,1,10'; Read = @{ stflowa01 = $true }; Skipped = 1; Flows = 0 }
        @{ Case = 'inbound allowed when the storage account was not read'; Tuple = '1705318200,10.0.1.5,10.0.2.5,1,443,T,I,A,B,1,10,1,10'; Read = @{}; Skipped = 0; Flows = 1 }
        @{ Case = 'inbound denied from a read storage scope'; Tuple = '1705318200,10.0.1.5,10.0.2.5,1,443,T,I,D,B,1,10,1,10'; Read = @{ stflowa01 = $true }; Skipped = 0; Flows = 1 }
        @{ Case = 'outbound allowed from a read storage scope'; Tuple = '1705318200,10.0.1.5,10.0.2.5,1,443,T,O,A,B,1,10,1,10'; Read = @{ stflowa01 = $true }; Skipped = 0; Flows = 1 }
    ) {
        InModuleScope AzureFlowLogParser -Parameters @{ Tuple = $Tuple; Read = $Read; Skipped = $Skipped; Flows = $Flows } {
            param($Tuple, $Read, $Skipped, $Flows)
            $filter = [pscustomobject]@{ ActionCode = $null; ProtocolCode = $null; Ports = [pscustomobject]@{ Ranges = @() }; IpFilter = ''; MinBytes = 0 }
            $state = New-FlowLogState -Window $null -Filter $filter
            $state.ScopeCache['10.0.1.5'] = @{ id = 'stflowa01'; kind = 'storage'; label = 'stflowa01'; subscriptionId = ''; resourceGroup = ''; storageAccount = 'stflowa01' }
            $state.ScopeCache['10.0.2.5'] = @{ id = 'internet'; kind = 'internet'; label = 'Internet'; subscriptionId = ''; resourceGroup = ''; storageAccount = '' }
            $state.ReadAccounts = $Read
            Add-FlowLogObservation -State $state -Tuple $Tuple -Version 2 -Rule 'R'
            $state.Skipped | Should -Be $Skipped
            $state.Flows | Should -Be $Flows
        }
    }
}

Describe 'Health classification and inventory (mocked Az wrappers)' {
    BeforeAll {
        $script:HealthSub = '22222222-2222-2222-2222-222222222222'
        function New-HealthAccount {
            param([string]$Name)
            [pscustomobject]@{ SubscriptionId = $script:HealthSub; Name = $Name; ResourceGroup = 'rg-h'; Location = 'australiaeast'; Kind = 'StorageV2'; SkuName = 'Standard_LRS' }
        }
        function New-HealthConfig {
            param([string]$Account, [string]$Nsg, [bool]$Enabled = $true)
            [pscustomobject]@{
                SubscriptionId = $script:HealthSub; Name = "fl-$Account"; NetworkWatcher = 'NetworkWatcher_australiaeast'; Location = 'australiaeast'
                Target = "/subscriptions/$($script:HealthSub)/resourceGroups/rg-h/providers/Microsoft.Network/networkSecurityGroups/$Nsg"
                TargetKind = 'NSG'; Enabled = $Enabled; RetentionDays = 7; TrafficAnalytics = $false
                StorageAccount = $Account; StorageSubscriptionId = $script:HealthSub; StorageResourceGroup = 'rg-h'
            }
        }
        $script:HealthArm = [pscustomobject]@{
            SubscriptionId    = $script:HealthSub
            StorageAccounts   = @('stflowhealthy', 'stflowstale', 'stflowdisabled', 'stflowunmanaged', 'stflownone', 'stflowdenied', 'stflowerror') | ForEach-Object { New-HealthAccount -Name $_ }
            FlowLogs          = @(
                New-HealthConfig -Account 'stflowhealthy' -Nsg 'nsg-h'
                New-HealthConfig -Account 'stflowstale' -Nsg 'nsg-s'
                New-HealthConfig -Account 'stflowdisabled' -Nsg 'nsg-d' -Enabled $false
                New-HealthConfig -Account 'stflowerror' -Nsg 'nsg-e'
            )
            VirtualNetworks   = @()
            NetworkInterfaces = @()
        }
        $script:UnmanagedBlob = "resourceId=/SUBSCRIPTIONS/$($script:HealthSub)/RESOURCEGROUPS/RG-H/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-U/y=2026/m=10/d=19/h=10/m=00/macAddress=001122334455/PT1H.json"
    }

    BeforeEach {
        Mock -ModuleName AzureFlowLogParser Get-FlowLogAzSubscription {
            [pscustomobject]@{ Id = $script:HealthSub; Name = 'health'; TenantId = '33333333-3333-3333-3333-333333333333' }
        }
        Mock -ModuleName AzureFlowLogParser Get-FlowLogAzSubscriptionData { $script:HealthArm }
        Mock -ModuleName AzureFlowLogParser New-FlowLogStorageContext { [pscustomobject]@{ Name = $StorageAccountName } }
        Mock -ModuleName AzureFlowLogParser Test-FlowLogContainer {
            if ($Context.Name -eq 'stflowdenied') { throw '403 AuthorizationPermissionMismatch: not authorised' }
            return $true
        }
        Mock -ModuleName AzureFlowLogParser Get-FlowLogBlobList {
            $now = [datetime]::UtcNow
            $todayPath = 'y=' + $now.ToString('yyyy') + '/m=' + $now.ToString('MM') + '/d=' + $now.ToString('dd') + '/'
            $hourPath = $todayPath + 'h=' + $now.ToString('HH') + '/m=00/macAddress=001122334455/PT1H.json'
            switch ($Context.Name) {
                'stflowerror' { throw 'socket closed while listing blobs' }
                'stflowhealthy' {
                    if ($Prefix -like "*/NSG-H/*$todayPath") { return @(New-TestBlobEntry -Name ($Prefix + $hourPath.Substring($todayPath.Length)) -LastModifiedUtc $now.AddHours(-1)) }
                }
                'stflowstale' {
                    if ($Prefix -like "*/NSG-S/*$todayPath") { return @(New-TestBlobEntry -Name ($Prefix + $hourPath.Substring($todayPath.Length)) -LastModifiedUtc $now.AddHours(-10)) }
                }
                'stflowunmanaged' {
                    if ($Prefix -eq 'resourceId=/') { return @(New-TestBlobEntry -Name $script:UnmanagedBlob -LastModifiedUtc $now.AddHours(-1)) }
                }
            }
            return @()
        }
    }

    It 'classifies configured accounts as Healthy, Stale, Disabled, Error' {
        $inventory = Get-FlowLogInventory -SubscriptionId $script:HealthSub
        ($inventory.accounts | Where-Object name -eq 'stflowhealthy').status | Should -Be 'Healthy'
        ($inventory.accounts | Where-Object name -eq 'stflowstale').status | Should -Be 'Stale'
        ($inventory.accounts | Where-Object name -eq 'stflowdisabled').status | Should -Be 'Disabled'
        ($inventory.accounts | Where-Object name -eq 'stflowerror').status | Should -Be 'Error'
    }

    It 'reports the newest blob age for a healthy config' {
        $inventory = Get-FlowLogInventory -SubscriptionId $script:HealthSub
        $config = $inventory.flowLogs | Where-Object name -eq 'fl-stflowhealthy'
        $config.status | Should -Be 'Healthy'
        $config.ageHours | Should -BeGreaterThan 0.9
        $config.ageHours | Should -BeLessThan 1.2
        $config.latestBlobUtc | Should -Match 'Z$'
    }

    It 'reports a disabled config without a blob check' {
        $inventory = Get-FlowLogInventory -SubscriptionId $script:HealthSub
        ($inventory.flowLogs | Where-Object name -eq 'fl-stflowdisabled').status | Should -Be 'Disabled'
        Should -Invoke -CommandName Get-FlowLogBlobList -ModuleName AzureFlowLogParser -Times 0 -Exactly -ParameterFilter { $Context.Name -eq 'stflowdisabled' }
    }

    It 'reports an account with no configs but blobs as Unmanaged with the observed target' {
        $inventory = Get-FlowLogInventory -SubscriptionId $script:HealthSub
        $account = $inventory.accounts | Where-Object name -eq 'stflowunmanaged'
        $account.status | Should -Be 'Unmanaged'
        $account.hasFlowLogs | Should -BeTrue
        $account.latestBlobUtc | Should -BeNullOrEmpty
        $account.observedTarget | Should -Be "/SUBSCRIPTIONS/$($script:HealthSub)/RESOURCEGROUPS/RG-H/PROVIDERS/MICROSOFT.NETWORK/NETWORKSECURITYGROUPS/NSG-U"
    }

    It 'reports a container access failure as AccessDenied' {
        $inventory = Get-FlowLogInventory -SubscriptionId $script:HealthSub -IncludeAll
        ($inventory.accounts | Where-Object name -eq 'stflowdenied').status | Should -Be 'AccessDenied'
    }

    It 'lists AccessDenied accounts by default and no-flow-log accounts only with -IncludeAll' {
        $default = Get-FlowLogInventory -SubscriptionId $script:HealthSub
        $all = Get-FlowLogInventory -SubscriptionId $script:HealthSub -IncludeAll
        $default.accounts.name | Should -Contain 'stflowdenied'
        ($default.accounts | Where-Object name -eq 'stflowdenied').status | Should -Be 'AccessDenied'
        $default.accounts.name | Should -Not -Contain 'stflownone'
        $all.accounts.name | Should -Contain 'stflownone'
        ($all.accounts | Where-Object name -eq 'stflownone').status | Should -Be 'NoFlowLogs'
    }

    It 'lists one flow-log row per config and no configs for accounts it did not select' {
        $inventory = Get-FlowLogInventory -SubscriptionId $script:HealthSub -StorageAccountName 'stflowstale'
        $inventory.flowLogs.Count | Should -Be 1
        $inventory.flowLogs[0].storageAccount | Should -Be 'stflowstale'
        $inventory.flowLogs[0].status | Should -Be 'Stale'
    }

    It 'derives the account status from the worst config status' {
        $inventory = Get-FlowLogInventory -SubscriptionId $script:HealthSub
        ($inventory.accounts | Where-Object name -eq 'stflowhealthy').flowLogConfigs | Should -Be 1
        ($inventory.accounts | Where-Object name -eq 'stflowhealthy').latestBlobUtc | Should -Match 'Z$'
    }
}

Describe 'Azure-mode traffic with dedupe across two storage accounts' {
    BeforeAll {
        $script:TrafficSub = '44444444-4444-4444-4444-444444444444'
        $script:TrafficNsgA = "/subscriptions/$($script:TrafficSub)/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-a"
        $script:TrafficNsgB = "/subscriptions/$($script:TrafficSub)/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-b"
        $script:TrafficArm = [pscustomobject]@{
            SubscriptionId    = $script:TrafficSub
            StorageAccounts   = @(
                [pscustomobject]@{ SubscriptionId = $script:TrafficSub; Name = 'stflowa01'; ResourceGroup = 'rg-net'; Location = 'australiaeast'; Kind = 'StorageV2'; SkuName = 'Standard_LRS' }
                [pscustomobject]@{ SubscriptionId = $script:TrafficSub; Name = 'stflowb01'; ResourceGroup = 'rg-net'; Location = 'australiaeast'; Kind = 'StorageV2'; SkuName = 'Standard_LRS' }
            )
            FlowLogs          = @(
                [pscustomobject]@{ SubscriptionId = $script:TrafficSub; Name = 'fl-a'; NetworkWatcher = 'nw'; Location = 'australiaeast'; Target = $script:TrafficNsgA; TargetKind = 'NSG'; Enabled = $true; RetentionDays = 7; TrafficAnalytics = $false; StorageAccount = 'stflowa01'; StorageSubscriptionId = $script:TrafficSub; StorageResourceGroup = 'rg-net' }
                [pscustomobject]@{ SubscriptionId = $script:TrafficSub; Name = 'fl-b'; NetworkWatcher = 'nw'; Location = 'australiaeast'; Target = $script:TrafficNsgB; TargetKind = 'NSG'; Enabled = $true; RetentionDays = 7; TrafficAnalytics = $false; StorageAccount = 'stflowb01'; StorageSubscriptionId = $script:TrafficSub; StorageResourceGroup = 'rg-net' }
            )
            VirtualNetworks   = @()
            NetworkInterfaces = @(
                [pscustomobject]@{ Name = 'nic-a'; Id = '/nics/nic-a'; NsgId = $script:TrafficNsgA; PrivateIps = @('10.0.1.4') }
                [pscustomobject]@{ Name = 'nic-b'; Id = '/nics/nic-b'; NsgId = $script:TrafficNsgB; PrivateIps = @('10.0.2.10') }
            )
        }
    }

    BeforeEach {
        # Tuples fall 30 minutes before now, inside the default 24h window. Blobs are only listed for today.
        $epoch = [DateTimeOffset]::UtcNow.AddMinutes(-30).ToUnixTimeSeconds()
        $script:TrafficBlobJsonA = @"
{"records":[
 {"properties":{"Version":2},"flowRecords":{"flows":[
   {"rule":"DefaultRule_AllowVnetOutBound","flowGroups":[{"flowTuples":["$epoch,10.0.1.4,10.0.2.10,50000,3389,T,O,A,B,1,100,1,200"]}]},
   {"rule":"UserRule_DenySSH","flowGroups":[{"flowTuples":["$epoch,10.0.2.10,10.0.1.4,50001,22,T,I,D,B,1,60,0,0"]}]}
 ]}}
]}
"@
        $script:TrafficBlobJsonB = @"
{"records":[
 {"properties":{"Version":2},"flowRecords":{"flows":[
   {"rule":"DefaultRule_AllowVnetInBound","flowGroups":[{"flowTuples":["$epoch,10.0.1.4,10.0.2.10,50000,3389,T,I,A,B,1,100,1,200"]}]},
   {"rule":"DefaultRule_AllowInternetOutBound","flowGroups":[{"flowTuples":["$epoch,10.0.2.10,13.107.42.14,50002,443,T,O,A,B,2,400,2,800"]}]}
 ]}}
]}
"@
        Mock -ModuleName AzureFlowLogParser Get-FlowLogAzSubscription {
            [pscustomobject]@{ Id = $script:TrafficSub; Name = 'traffic'; TenantId = '55555555-5555-5555-5555-555555555555' }
        }
        Mock -ModuleName AzureFlowLogParser Get-FlowLogAzSubscriptionData { $script:TrafficArm }
        Mock -ModuleName AzureFlowLogParser New-FlowLogStorageContext { [pscustomobject]@{ Name = $StorageAccountName } }
        Mock -ModuleName AzureFlowLogParser Test-FlowLogContainer { $true }
        Mock -ModuleName AzureFlowLogParser Get-FlowLogBlobList {
            $now = [datetime]::UtcNow
            $hour = 'h=' + $now.ToString('HH') + '/m=00/macAddress=001122334455/PT1H.json'
            $todayPrefix = 'y=' + $now.ToString('yyyy') + '/m=' + $now.ToString('MM') + '/d=' + $now.ToString('dd') + '/'
            if ($Prefix -notlike "*$todayPrefix") { return @() }
            if ($Prefix -like '*/NSG-A/*') { return @(New-TestBlobEntry -Name ($Prefix + $hour) -LastModifiedUtc $now) }
            if ($Prefix -like '*/NSG-B/*') { return @(New-TestBlobEntry -Name ($Prefix + $hour) -LastModifiedUtc $now) }
            return @()
        }
        Mock -ModuleName AzureFlowLogParser Read-FlowLogBlobText {
            if ($Context.Name -eq 'stflowa01') { return $script:TrafficBlobJsonA }
            return $script:TrafficBlobJsonB
        }
    }

    It 'skips the duplicate inbound allowed record and keeps the others' {
        $model = Get-FlowLogTraffic -SubscriptionId $script:TrafficSub
        $model.source | Should -Be 'azure'
        $model.summary.flows | Should -Be 3
        $model.summary.allowed | Should -Be 2
        $model.summary.denied | Should -Be 1
        $model.summary.bytes | Should -Be 1560
        $model.summary.observationsSkipped | Should -Be 1
        $model.summary.storageAccounts | Should -Be 2
    }

    It 'maps flows to storage scopes via the NIC and flags the dedupe in notes' {
        $model = Get-FlowLogTraffic -SubscriptionId $script:TrafficSub
        $model.flows.srcScope | Should -Contain 'stflowa01'
        $model.flows.dstScope | Should -Contain 'internet'
        ($model.notes -match 'Skipped: 1').Count | Should -Be 1
    }

    It 'reads only the accounts named by -StorageAccountName' {
        $model = Get-FlowLogTraffic -SubscriptionId $script:TrafficSub -StorageAccountName 'stflowa01'
        $model.summary.storageAccounts | Should -Be 1
        $model.summary.observationsSkipped | Should -Be 0
        $model.summary.flows | Should -Be 2
    }
}

Describe 'Offline topology (-TopologyPath with -InputPath)' {
    BeforeAll {
        # Storage stflowlogs01 covers 10.10.0.0/16; vnet-spoke 10.10.5.0/24 sits inside it, so the VNet wins for 10.10.5.x.
        # The fixture also holds an IPv6 prefix (skipped and counted) and a padded IPv4 prefix (trimmed).
        $script:TopologyFile = Join-Path $PSScriptRoot 'fixtures' 'topology.json'
        # t1 is the inbound copy of a flow whose outbound copy is logged at storage stflowlogs01 (dropped when that account is read).
        $script:TopologyTuples = Join-Path $PSScriptRoot 'fixtures' 'topology-tuples.json'
    }

    It 'maps flows by longest prefix and drops the duplicate inbound record' {
        $model = Get-FlowLogTraffic -InputPath $script:TopologyTuples -TopologyPath $script:TopologyFile
        $model.source | Should -Be 'local'
        $model.summary.flows | Should -Be 3
        $model.summary.observationsSkipped | Should -Be 1
        $model.summary.storageAccounts | Should -Be 1
        $model.scopes.id | Should -Contain 'stflowlogs01'
        $model.scopes.id | Should -Contain 'vnet:vnet-spoke'
        ($model.scopes | Where-Object id -eq 'stflowlogs01').kind | Should -Be 'storage'
        ($model.scopes | Where-Object id -eq 'stflowlogs01').storageAccount | Should -Be 'stflowlogs01'
        ($model.scopes | Where-Object id -eq 'vnet:vnet-spoke').kind | Should -Be 'vnet'
        ($model.scopes | Where-Object id -eq 'vnet:vnet-spoke').label | Should -Be 'vnet-spoke'
        ($model.flows | Where-Object src -eq '10.10.5.9' | Select-Object -First 1).srcScope | Should -Be 'vnet:vnet-spoke'
        ($model.flows | Where-Object src -eq '10.10.1.1' | Select-Object -First 1).srcScope | Should -Be 'stflowlogs01'
    }

    It 'writes the topology note with file name, network and prefix counts, and skipped non-IPv4 prefixes' {
        $model = Get-FlowLogTraffic -InputPath $script:TopologyTuples -TopologyPath $script:TopologyFile
        ($model.notes -contains 'Offline topology from topology.json: 2 network(s), 2 prefix(es), 1 non-IPv4 prefix(es) skipped.') | Should -BeTrue
    }

    It 'writes a note without a skipped clause when every prefix is IPv4' {
        $path = Join-Path $TestDrive 'ipv4-only.json'
        '{ "networks": [ { "name": "stflowlogs01", "kind": "storage", "prefixes": ["10.10.0.0/16"] } ] }' | Set-Content -LiteralPath $path
        $model = Get-FlowLogTraffic -InputPath $script:TopologyTuples -TopologyPath $path
        ($model.notes -contains 'Offline topology from ipv4-only.json: 1 network(s), 1 prefix(es).') | Should -BeTrue
    }

    It 'has no dedupe and only internet/unmapped scopes without a topology file' {
        $model = Get-FlowLogTraffic -InputPath $script:TopologyTuples
        $model.summary.flows | Should -Be 4
        $model.summary.observationsSkipped | Should -Be 0
        $model.scopes.id | Should -Not -Contain 'stflowlogs01'
    }

    It 'rejects a topology file with an unknown kind' {
        $path = Join-Path $TestDrive 'bad-kind.json'
        '{ "networks": [ { "name": "x", "kind": "subnet", "prefixes": ["10.0.0.0/8"] } ] }' | Set-Content -LiteralPath $path
        { Get-FlowLogTraffic -InputPath $script:TopologyTuples -TopologyPath $path } | Should -Throw "*kind 'subnet'*"
    }

    It 'rejects a topology file without a networks array' {
        $path = Join-Path $TestDrive 'no-networks.json'
        '{ "subnets": [] }' | Set-Content -LiteralPath $path
        { Get-FlowLogTraffic -InputPath $script:TopologyTuples -TopologyPath $path } | Should -Throw "*'networks' array*"
    }

    It 'fails clearly for a missing topology file' {
        { Get-FlowLogTraffic -InputPath $script:TopologyTuples -TopologyPath (Join-Path $TestDrive 'nope.json') } | Should -Throw '*Topology file not found*'
    }

    It 'ignores -TopologyPath without -InputPath and warns' {
        Mock -ModuleName AzureFlowLogParser Get-FlowLogMonitoring { throw 'stop after the warning' }
        # The mocked monitoring call stops the run after the warning is written.
        try { Get-FlowLogTraffic -TopologyPath $script:TopologyFile -WarningVariable warnings -WarningAction SilentlyContinue | Out-Null } catch { Write-Debug $_.Exception.Message }
        ($warnings -join ' ') | Should -Match 'ignored without -InputPath'
        Should -Invoke -CommandName Get-FlowLogMonitoring -ModuleName AzureFlowLogParser -Times 1 -Exactly
    }
}

Describe 'Observation time fallback' {
    It 'uses the record time when the tuple start time is not numeric' {
        $path = Join-Path $TestDrive 'record-time.json'
        '{"records":[{"time":"2024-01-15T11:30:00.0000000Z","properties":{"Version":2},"flowRecords":{"flows":[{"rule":"R","flowGroups":[{"flowTuples":["not-a-time,10.0.1.4,8.8.8.8,1,53,U,O,A,,,,,"]}]}]}}]}' | Set-Content -LiteralPath $path
        $model = Get-FlowLogTraffic -InputPath $path
        $model.summary.flows | Should -Be 1
        ($model.notes -match 'Malformed').Count | Should -Be 0
        $model.flows[0].first | Should -Be '2024-01-15T11:30:00Z'
    }

    It 'counts a tuple as malformed when neither the tuple nor the record has a time' {
        $path = Join-Path $TestDrive 'no-time.json'
        '{"records":[{"properties":{"Version":2},"flowRecords":{"flows":[{"rule":"R","flowGroups":[{"flowTuples":["not-a-time,10.0.1.4,8.8.8.8,1,53,U,O,A,,,,,"]}]}]}}]}' | Set-Content -LiteralPath $path
        $model = Get-FlowLogTraffic -InputPath $path
        $model.summary.flows | Should -Be 0
        ($model.notes -match 'Malformed tuples skipped: 1').Count | Should -Be 1
    }
}
