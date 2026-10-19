#Requires -Version 7.6
# Azure CLI contract tests. Invoke-FlowLogAzCli is mocked with the az-shaped fixtures in tests/fixtures/az, so no test calls az.
# The wrapper tests at the end put a test double named az first on PATH. It never reaches Azure.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..' 'AzureFlowLogParser' 'AzureFlowLogParser.psd1') -Force

    $script:AzFixtures = @{}
    foreach ($file in Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'fixtures' 'az') -Filter '*.json') {
        $script:AzFixtures[$file.BaseName] = [System.IO.File]::ReadAllText($file.FullName) | ConvertFrom-Json -DateKind String
    }
    # The ARM originals the az fixtures were derived from. Used to check that both describe the same resources.
    $script:ArmFixtures = @{}
    foreach ($file in Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'fixtures' 'arm') -Filter '*.json') {
        $script:ArmFixtures[$file.BaseName] = [System.IO.File]::ReadAllText($file.FullName) | ConvertFrom-Json -DateKind String
    }

    $script:Sub = '00000000-0000-0000-0000-000000000001'

    # Subnet prefixes as one comma-joined string, whichever of addressPrefix or addressPrefixes the object uses.
    function Get-SubnetPrefixes {
        param($Subnet)
        (@(@($Subnet.addressPrefixes) + @($Subnet.addressPrefix)) | Where-Object { $_ }) -join ','
    }

    # Runs the subscription read against the mocked CLI and returns the inventory object.
    function Get-FixtureInventory {
        InModuleScope AzureFlowLogParser -Parameters @{ Sub = $script:Sub } {
            param($Sub)
            Get-FlowLogAzSubscriptionData -SubscriptionId $Sub -TenantId '11111111-1111-1111-1111-111111111111' -DisplayName 'Prod Network'
        }
    }
}

Describe 'Azure CLI output contract (tests/fixtures/az)' {
    BeforeEach {
        # Each az command maps to the fixture that shows its output shape. Anything else fails the test.
        Mock -ModuleName AzureFlowLogParser Invoke-FlowLogAzCli {
            param([string[]]$Arguments)
            $command = $Arguments -join ' '
            switch -Wildcard ($command) {
                'account list --all' { return $script:AzFixtures['account-list'] }
                'account show --subscription *' { return $script:AzFixtures['account-show'] }
                'storage account list --subscription *' { return $script:AzFixtures['storage-account-list'] }
                'network watcher list --subscription *' { return $script:AzFixtures['network-watcher-list'] }
                'network watcher flow-log list --location australiaeast --subscription *' { return $script:AzFixtures['flow-log-list'] }
                'network vnet list --subscription *' { return $script:AzFixtures['virtual-network-list'] }
                'network nic list --subscription *' { return $script:AzFixtures['network-interface-list'] }
                'storage container exists *' { return $script:AzFixtures['container-exists'] }
                'storage blob list *' { return $script:AzFixtures['blob-list'] }
                'storage blob download *' {
                    $target = $Arguments[[array]::IndexOf($Arguments, '--file') + 1]
                    [System.IO.File]::WriteAllText($target, '{"records":[]}')
                    return
                }
                default { throw "Unexpected az call in a contract test: $command" }
            }
        }
    }

    It 'lists only enabled subscriptions from account list' {
        $subscriptions = InModuleScope AzureFlowLogParser { @(Get-FlowLogAzSubscription) }
        @($subscriptions).Count | Should -Be 1
        $subscriptions[0].Id | Should -Be '00000000-0000-0000-0000-000000000001'
        $subscriptions[0].Name | Should -Be 'Prod Network'
        $subscriptions[0].TenantId | Should -Be '11111111-1111-1111-1111-111111111111'
    }

    It 'reads one named subscription with account show' {
        $subscriptions = InModuleScope AzureFlowLogParser { @(Get-FlowLogAzSubscription -SubscriptionId '00000000-0000-0000-0000-000000000001') }
        $subscriptions[0].Name | Should -Be 'Prod Network'
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -Times 1 -Exactly -ParameterFilter {
            ($Arguments -join ' ') -eq 'account show --subscription 00000000-0000-0000-0000-000000000001'
        }
    }

    It 'maps storage accounts with the SKU name from sku.name' {
        $accounts = @((Get-FixtureInventory).StorageAccounts)
        $accounts.Count | Should -Be 2
        $accounts[0].Name | Should -Be 'stflowlogs01'
        $accounts[0].ResourceGroup | Should -Be 'rg-net'
        $accounts[0].Location | Should -Be 'australiaeast'
        $accounts[0].Kind | Should -Be 'StorageV2'
        $accounts[0].SkuName | Should -Be 'Standard_GRS'
        $accounts[1].Kind | Should -Be 'Storage'
        $accounts[1].SkuName | Should -Be 'Standard_LRS'
    }

    It 'lists flow logs once per watcher location and maps the target fields' {
        $data = Get-FixtureInventory
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -Times 1 -Exactly -ParameterFilter {
            ($Arguments -join ' ') -eq 'network watcher flow-log list --location australiaeast --subscription 00000000-0000-0000-0000-000000000001'
        }
        $logs = @($data.FlowLogs)
        $logs.Count | Should -Be 3
        $logs.NetworkWatcher | Should -Be @('NetworkWatcher_australiaeast', 'NetworkWatcher_australiaeast', 'NetworkWatcher_australiaeast')

        $logs[0].Name | Should -Be 'flowlog-web-nsg'
        $logs[0].Target | Should -Be '/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-app/providers/Microsoft.Network/networkSecurityGroups/web-nsg'
        $logs[0].TargetKind | Should -Be 'NSG'
        $logs[0].TargetName | Should -Be 'web-nsg'
        $logs[0].TargetResourceGroup | Should -Be 'rg-app'
        $logs[0].Enabled | Should -BeTrue
        $logs[0].RetentionDays | Should -Be 30
        $logs[0].TrafficAnalytics | Should -BeTrue
        $logs[0].StorageAccount | Should -Be 'stflowlogs01'
        $logs[0].StorageResourceGroup | Should -Be 'rg-net'

        $logs[1].Enabled | Should -BeFalse
        $logs[1].TargetKind | Should -Be 'VNet'
        $logs[1].TargetName | Should -Be 'hub-vnet'
        $logs[1].RetentionDays | Should -Be 0
        $logs[1].TrafficAnalytics | Should -BeFalse

        $logs[2].Target | Should -Be ''
        $logs[2].TargetKind | Should -Be 'Other'
        $logs[2].TargetName | Should -Be ''
        $logs[2].TargetResourceGroup | Should -Be ''
    }

    It 'maps VNets and subnets, with one prefix or several' {
        $data = Get-FixtureInventory
        $vnet = @($data.VirtualNetworks)[0]
        $vnet.Name | Should -Be 'hub-vnet'
        $vnet.ResourceGroup | Should -Be 'rg-net'
        @($vnet.AddressPrefixes) | Should -Be @('10.10.0.0/16')

        $subnets = @($vnet.Subnets)
        $subnets.Count | Should -Be 3
        $subnets[0].Name | Should -Be 'app'
        @($subnets[0].AddressPrefixes) | Should -Be @('10.10.1.0/24')
        $subnets[0].NsgId | Should -Be '/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-app/providers/Microsoft.Network/networkSecurityGroups/web-nsg'
        @($subnets[1].AddressPrefixes) | Should -Be @('10.10.2.0/24', '10.10.3.0/24')
        $subnets[1].NsgId | Should -Be ''
        @($subnets[2].AddressPrefixes).Count | Should -Be 0
    }

    It 'maps NIC private addresses and NSG ids, and a NIC without an address has none' {
        $nics = @((Get-FixtureInventory).NetworkInterfaces)
        $nics.Count | Should -Be 2
        $nics[0].Name | Should -Be 'web-vm-nic'
        @($nics[0].PrivateIps) | Should -Be @('10.10.1.4')
        $nics[0].NsgId | Should -Be '/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-app/providers/Microsoft.Network/networkSecurityGroups/web-nsg'
        @($nics[1].PrivateIps).Count | Should -Be 0
        $nics[1].NsgId | Should -Be ''
    }

    It 'reads the container and the blob listing with the flat az fields' {
        $context = InModuleScope AzureFlowLogParser { New-FlowLogStorageContext -StorageAccountName 'stflowlogs01' }
        $exists = InModuleScope AzureFlowLogParser -Parameters @{ Context = $context } {
            param($Context)
            Test-FlowLogContainer -Context $Context
        }
        $exists | Should -BeTrue

        $blobs = @(InModuleScope AzureFlowLogParser -Parameters @{ Context = $context } {
            param($Context)
            Get-FlowLogBlobList -Context $Context -Prefix 'resourceId=/'
        })
        $blobs.Count | Should -Be 2
        $blobs[0].Name | Should -BeLike 'resourceId=/SUBSCRIPTIONS/*'
        $blobs[0].Length | Should -Be 2048
        $blobs[0].LastModifiedUtc.Kind | Should -Be 'Utc'
        $blobs[0].LastModifiedUtc.ToString('yyyy-MM-ddTHH:mm:ss') | Should -Be '2026-10-19T09:30:00'
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -ParameterFilter {
            ($Arguments -join ' ') -eq 'storage blob list --account-name stflowlogs01 --container-name insights-logs-flowlogflowevent --prefix=resourceId=/ --num-results=* --auth-mode login'
        }
    }

    It 'caps the blob listing when a MaxCount is given' {
        $context = InModuleScope AzureFlowLogParser { New-FlowLogStorageContext -StorageAccountName 'stflowlogs01' }
        InModuleScope AzureFlowLogParser -Parameters @{ Context = $context } {
            param($Context)
            Get-FlowLogBlobList -Context $Context -Prefix 'resourceId=/' -MaxCount 1 | Out-Null
        }
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -ParameterFilter {
            ($Arguments -join ' ') -like '*--num-results=1 --auth-mode login'
        }
    }

    It 'downloads a blob to a temporary file and reads its text' {
        $context = InModuleScope AzureFlowLogParser { New-FlowLogStorageContext -StorageAccountName 'stflowlogs01' }
        $text = InModuleScope AzureFlowLogParser -Parameters @{ Context = $context; Name = 'PT1H.json' } {
            param($Context, $Name)
            Read-FlowLogBlobText -Context $Context -Name $Name
        }
        $text | Should -Be '{"records":[]}'
        Should -Invoke -CommandName Invoke-FlowLogAzCli -ModuleName AzureFlowLogParser -ParameterFilter {
            ($Arguments -join ' ') -like 'storage blob download *--name=PT1H.json *--auth-mode login*'
        }
    }

    It 'agrees with the ARM originals on storage accounts, VNets, subnets, NICs and flow-log targets' {
        $arm = $script:ArmFixtures
        $az = $script:AzFixtures

        (@($az['storage-account-list'].name) -join ',') | Should -Be (@($arm['storage_accounts'].value.name) -join ',')
        (@($az['storage-account-list'].sku.name) -join ',') | Should -Be (@($arm['storage_accounts'].value.sku.name) -join ',')

        $armVnet = $arm['virtual_networks'].value[0]
        $azVnet = $az['virtual-network-list'][0]
        $azVnet.name | Should -Be $armVnet.name
        $azVnet.id | Should -Be $armVnet.id
        (@($azVnet.addressSpace.addressPrefixes) -join ',') | Should -Be (@($armVnet.properties.addressSpace.addressPrefixes) -join ',')
        for ($i = 0; $i -lt @($armVnet.properties.subnets).Count; $i++) {
            $armSubnet = $armVnet.properties.subnets[$i]
            $azSubnet = $azVnet.subnets[$i]
            $azSubnet.name | Should -Be $armSubnet.name
            $azSubnet.id | Should -Be $armSubnet.id
            Get-SubnetPrefixes -Subnet $azSubnet | Should -Be (Get-SubnetPrefixes -Subnet $armSubnet.properties)
        }

        for ($i = 0; $i -lt @($arm['network_interfaces'].value).Count; $i++) {
            $armNic = $arm['network_interfaces'].value[$i]
            $azNic = $az['network-interface-list'][$i]
            $azNic.name | Should -Be $armNic.name
            $azNic.id | Should -Be $armNic.id
            (@($azNic.ipConfigurations.privateIpAddress) -join ',') | Should -Be (@($armNic.properties.ipConfigurations.properties.privateIPAddress) -join ',')
        }

        foreach ($armLog in $arm['flow_logs'].value) {
            $azLog = @($az['flow-log-list'] | Where-Object { $_.name -eq $armLog.name })[0]
            $azLog | Should -Not -BeNullOrEmpty -Because "flow log $($armLog.name) should be in the az fixture"
            $azLog.targetResourceId | Should -Be $armLog.properties.targetResourceId
            $azLog.storageId | Should -Be $armLog.properties.storageId
            $azLog.enabled | Should -Be $armLog.properties.enabled
        }
    }
}

Describe 'Azure CLI wrapper (test double named az on PATH)' -Skip:$IsWindows {
    BeforeAll {
        $script:OriginalPath = $env:PATH
        $script:FakeDir = Join-Path $TestDrive 'fake-bin'
        New-Item -ItemType Directory -Path $script:FakeDir -Force | Out-Null
        $script:FakeScript = @'
#!/bin/sh
# az test double (powershell/tests/AzureCliOutput.Tests.ps1). Records its arguments and answers from the environment.
printf '%s\n' "$@" > "$FAKE_AZ_ARGS"
if [ -n "$FAKE_AZ_STDERR" ]; then printf '%s\n' "$FAKE_AZ_STDERR" 1>&2; fi
if [ -n "$FAKE_AZ_STDOUT" ]; then printf '%s' "$FAKE_AZ_STDOUT"; fi
exit "${FAKE_AZ_EXIT:-0}"
'@
        $fakeAz = Join-Path $script:FakeDir 'az'
        Set-Content -LiteralPath $fakeAz -Value $script:FakeScript -NoNewline
        & chmod +x $fakeAz
        $env:PATH = $script:FakeDir + [System.IO.Path]::PathSeparator + $script:OriginalPath

        # Refuse to run unless az resolves to the test double, so a real az can never be reached from here.
        $resolved = (Get-Command -Name az -CommandType Application | Select-Object -First 1).Source
        if ($null -eq $resolved -or [System.IO.File]::ReadAllText($resolved) -ne $script:FakeScript) {
            throw 'az does not resolve to the test double. Refusing to run the wrapper tests.'
        }
        $script:ArgsFile = Join-Path $TestDrive 'az-args.txt'
    }

    AfterAll {
        $env:PATH = $script:OriginalPath
        Remove-Item Env:FAKE_AZ_ARGS, Env:FAKE_AZ_STDOUT, Env:FAKE_AZ_STDERR, Env:FAKE_AZ_EXIT -ErrorAction SilentlyContinue
    }

    AfterEach {
        Remove-Item Env:FAKE_AZ_STDOUT, Env:FAKE_AZ_STDERR, Env:FAKE_AZ_EXIT -ErrorAction SilentlyContinue
    }

    It 'passes the arguments as an array and adds --only-show-errors and -o json' {
        $env:FAKE_AZ_ARGS = $script:ArgsFile
        $env:FAKE_AZ_STDOUT = '[]'
        InModuleScope AzureFlowLogParser -Parameters @{ Items = @('storage', 'blob', 'list', 'name with spaces', '--num-results=*') } {
            param($Items)
            Invoke-FlowLogAzCli -Arguments $Items | Out-Null
        }
        # The '=*' token must reach az as written: a bare * would be expanded against the working directory.
        (Get-Content -LiteralPath $script:ArgsFile) -join '|' |
            Should -Be 'storage|blob|list|name with spaces|--num-results=*|--only-show-errors|-o|json'
    }

    It 'parses stdout JSON into items, and an empty answer gives no items' {
        $env:FAKE_AZ_ARGS = $script:ArgsFile
        $env:FAKE_AZ_STDOUT = '[{"name":"a"},{"name":"b"}]'
        $items = @(InModuleScope AzureFlowLogParser -Parameters @{ Items = @('account', 'list') } {
            param($Items)
            Invoke-FlowLogAzCli -Arguments $Items
        })
        $items.Count | Should -Be 2
        $items[1].name | Should -Be 'b'

        Remove-Item Env:FAKE_AZ_STDOUT
        $none = @(InModuleScope AzureFlowLogParser -Parameters @{ Items = @('account', 'list') } {
            param($Items)
            Invoke-FlowLogAzCli -Arguments $Items
        })
        $none.Count | Should -Be 0
    }

    It 'throws with az''s own error text when az exits non-zero' {
        $env:FAKE_AZ_ARGS = $script:ArgsFile
        $env:FAKE_AZ_EXIT = '3'
        $env:FAKE_AZ_STDERR = 'ERROR: (AuthorizationPermissionMismatch) This request is not authorized to perform this operation.'
        InModuleScope AzureFlowLogParser {
            { Invoke-FlowLogAzCli -Arguments @('storage', 'blob', 'list') } |
                Should -Throw '*az storage blob list failed: ERROR: (AuthorizationPermissionMismatch)*'
        }
    }

    It 'maps a permission failure from az to AccessDenied' {
        $env:FAKE_AZ_ARGS = $script:ArgsFile
        $env:FAKE_AZ_EXIT = '3'
        $env:FAKE_AZ_STDERR = 'ERROR: (AuthorizationPermissionMismatch) This request is not authorized.'
        $status = InModuleScope AzureFlowLogParser {
            try { Invoke-FlowLogAzCli -Arguments @('storage', 'container', 'exists') }
            catch { Get-FlowLogErrorStatus -ErrorRecord $_ }
        }
        $status | Should -Be 'AccessDenied'
    }
}

Describe 'Azure CLI presence check' {
    It 'fails with a clear message when az is not on PATH' {
        $emptyDir = Join-Path $TestDrive 'no-az'
        New-Item -ItemType Directory -Path $emptyDir -Force | Out-Null
        $originalPath = $env:PATH
        try {
            $env:PATH = $emptyDir
            InModuleScope AzureFlowLogParser {
                { Assert-FlowLogAzCli } | Should -Throw '*Azure CLI (az) on PATH*'
            }
        }
        finally {
            $env:PATH = $originalPath
        }
    }
}

Describe 'Inventory end to end through the real wrapper (test double answers from the az fixtures)' -Skip:$IsWindows {
    BeforeAll {
        $script:OriginalPath = $env:PATH
        $script:InventoryBin = Join-Path $TestDrive 'inventory-bin'
        New-Item -ItemType Directory -Path $script:InventoryBin -Force | Out-Null
        $script:InventoryScript = @'
#!/bin/sh
# az test double (powershell/tests/AzureCliOutput.Tests.ps1). Answers each exact inventory command from the fixtures.
printf '%s\n' "$@" > "$FAKE_AZ_ARGS"
f="$FAKE_AZ_FIXTURES"
s="00000000-0000-0000-0000-000000000001"
case "$*" in
  "account list --all --only-show-errors -o json") cat "$f/account-list.json" ;;
  "account show --subscription $s --only-show-errors -o json") cat "$f/account-show.json" ;;
  "storage account list --subscription $s --only-show-errors -o json") cat "$f/storage-account-list.json" ;;
  "network watcher list --subscription $s --only-show-errors -o json") cat "$f/network-watcher-list.json" ;;
  "network watcher flow-log list --location australiaeast --subscription $s --only-show-errors -o json") cat "$f/flow-log-list.json" ;;
  "network vnet list --subscription $s --only-show-errors -o json") cat "$f/virtual-network-list.json" ;;
  "network nic list --subscription $s --only-show-errors -o json") cat "$f/network-interface-list.json" ;;
  "storage container exists --account-name stflowlogs01 --name insights-logs-flowlogflowevent --auth-mode login --only-show-errors -o json") cat "$f/container-exists.json" ;;
  "storage container exists --account-name stappdata --name insights-logs-flowlogflowevent --auth-mode login --only-show-errors -o json")
    echo "ERROR: (AuthorizationPermissionMismatch) This request is not authorized to perform this operation." 1>&2
    exit 3 ;;
  "storage blob list --account-name stflowlogs01 "*) cat "$f/blob-list.json" ;;
  *) echo "ERROR: unexpected az call in the inventory test: $*" 1>&2; exit 9 ;;
esac
'@
        $fakeAz = Join-Path $script:InventoryBin 'az'
        Set-Content -LiteralPath $fakeAz -Value $script:InventoryScript -NoNewline
        & chmod +x $fakeAz
        $env:PATH = $script:InventoryBin + [System.IO.Path]::PathSeparator + $script:OriginalPath
        $resolved = (Get-Command -Name az -CommandType Application | Select-Object -First 1).Source
        if ($null -eq $resolved -or [System.IO.File]::ReadAllText($resolved) -ne $script:InventoryScript) {
            throw 'az does not resolve to the inventory test double. Refusing to run.'
        }
        $env:FAKE_AZ_FIXTURES = Join-Path $PSScriptRoot 'fixtures' 'az'
        $env:FAKE_AZ_ARGS = Join-Path $TestDrive 'inventory-args.txt'
    }

    AfterAll {
        $env:PATH = $script:OriginalPath
        Remove-Item Env:FAKE_AZ_FIXTURES, Env:FAKE_AZ_ARGS -ErrorAction SilentlyContinue
    }

    It 'reports the inventory, with a 403 on one account shown as AccessDenied' {
        $inventory = Get-FlowLogInventory -SubscriptionId '00000000-0000-0000-0000-000000000001'
        $inventory.subscriptions | Should -Be 1
        ($inventory.accounts | Where-Object { $_.name -eq 'stappdata' }).status | Should -Be 'AccessDenied'
        ($inventory.accounts | Where-Object { $_.name -eq 'stflowlogs01' }).flowLogConfigs | Should -Be 3
        @($inventory.flowLogs).Count | Should -Be 3
        ($inventory.flowLogs | Where-Object { $_.name -eq 'flowlog-web-nsg' }).targetName | Should -Be 'web-nsg'
        ($inventory.flowLogs | Where-Object { $_.name -eq 'flowlog-vnet-off' }).status | Should -Be 'Disabled'
    }
}
