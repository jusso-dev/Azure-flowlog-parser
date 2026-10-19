#Requires -Version 7.6
# Contract tests against the real Az types. They skip unless Az.Accounts, Az.Network and Az.Storage are installed,
# so the default run stays offline. Verified against Az.Accounts 5.5.3, Az.Network 8.3.0 and Az.Storage 9.7.2
# and the Microsoft Learn reference pages for the same versions.

BeforeDiscovery {
    $script:AzAvailable = @('Az.Accounts', 'Az.Network', 'Az.Storage' |
        ForEach-Object { Get-Module -ListAvailable -Name $_ }).Count -ge 3
}

BeforeAll {
    $repo = Resolve-Path (Join-Path $PSScriptRoot '../..')
    Import-Module (Join-Path $repo 'powershell/AzureFlowLogParser/AzureFlowLogParser.psd1') -Force
    if ($script:AzAvailable) {
        Import-Module Az.Accounts, Az.Network, Az.Storage -ErrorAction Stop 3>$null
    }

    function Test-AzParameter {
        param([string]$Cmdlet, [string]$Parameter)
        $command = Get-Command $Cmdlet -ErrorAction Stop
        if ($command.Parameters.ContainsKey($Parameter)) { return $true }
        return [bool]($command.Parameters.Values | Where-Object { $_.Aliases -contains $Parameter })
    }
}

Describe 'Az cmdlets and parameters used by the module' -Skip:(-not $script:AzAvailable) {
    It '<Cmdlet> accepts -<Parameter>' -ForEach @(
        @{ Cmdlet = 'Get-AzSubscription'; Parameter = 'SubscriptionId' }
        @{ Cmdlet = 'Set-AzContext'; Parameter = 'SubscriptionId' }
        @{ Cmdlet = 'Set-AzContext'; Parameter = 'TenantId' }
        @{ Cmdlet = 'Set-AzContext'; Parameter = 'Context' }
        @{ Cmdlet = 'Get-AzNetworkWatcherFlowLog'; Parameter = 'NetworkWatcher' }
        @{ Cmdlet = 'New-AzStorageContext'; Parameter = 'StorageAccountName' }
        @{ Cmdlet = 'New-AzStorageContext'; Parameter = 'UseConnectedAccount' }
        @{ Cmdlet = 'Get-AzStorageContainer'; Parameter = 'Prefix' }
        @{ Cmdlet = 'Get-AzStorageBlob'; Parameter = 'Container' }
        @{ Cmdlet = 'Get-AzStorageBlob'; Parameter = 'Prefix' }
        @{ Cmdlet = 'Get-AzStorageBlob'; Parameter = 'MaxCount' }
        @{ Cmdlet = 'Get-AzStorageBlobContent'; Parameter = 'Destination' }
    ) {
        Test-AzParameter -Cmdlet $Cmdlet -Parameter $Parameter | Should -BeTrue
    }
}

Describe 'Flow log configuration from real PSFlowLogResource objects' -Skip:(-not $script:AzAvailable) {
    BeforeAll {
        $script:Watcher = [Microsoft.Azure.Commands.Network.Models.PSNetworkWatcher]::new()
        $script:Watcher.Name = 'NetworkWatcher_eastus'

        $script:FlowLog = [Microsoft.Azure.Commands.Network.Models.PSFlowLogResource]::new()
        $script:FlowLog.Name = 'fl-nsg-app'
        $script:FlowLog.Location = 'eastus'
        $script:FlowLog.Enabled = $true
        $script:FlowLog.StorageId = '/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-logs/providers/Microsoft.Storage/storageAccounts/stflowapp01'
        $script:FlowLog.TargetResourceId = '/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-app'
        $script:FlowLog.RetentionPolicy = [Microsoft.Azure.Commands.Network.Models.PSRetentionPolicyParameters]::new()
        $script:FlowLog.RetentionPolicy.Days = 5
        $script:FlowLog.RetentionPolicy.Enabled = $true
        $analytics = [Microsoft.Azure.Commands.Network.Models.PSTrafficAnalyticsProperties]::new()
        $analytics.NetworkWatcherFlowAnalyticsConfiguration = [Microsoft.Azure.Commands.Network.Models.PSTrafficAnalyticsConfigurationProperties]::new()
        $analytics.NetworkWatcherFlowAnalyticsConfiguration.Enabled = $true
        $script:FlowLog.FlowAnalyticsConfiguration = $analytics
    }

    It 'reads storage account, target, retention and traffic analytics' {
        $config = & (Get-Module AzureFlowLogParser) { param($f, $w) ConvertTo-FlowLogConfig -FlowLog $f -Watcher $w -SubscriptionId '00000000-0000-0000-0000-000000000001' } $script:FlowLog $script:Watcher
        $config.StorageAccount | Should -Be 'stflowapp01'
        $config.StorageResourceGroup | Should -Be 'rg-logs'
        $config.TargetKind | Should -Be 'NSG'
        $config.RetentionDays | Should -Be 5
        $config.Enabled | Should -BeTrue
        $config.TrafficAnalytics | Should -BeTrue
    }

    It 'reports traffic analytics off when the nested flag is false' {
        $script:FlowLog.FlowAnalyticsConfiguration.NetworkWatcherFlowAnalyticsConfiguration.Enabled = $false
        $config = & (Get-Module AzureFlowLogParser) { param($f, $w) ConvertTo-FlowLogConfig -FlowLog $f -Watcher $w -SubscriptionId 's' } $script:FlowLog $script:Watcher
        $config.TrafficAnalytics | Should -BeFalse
    }
}

Describe 'VNet, subnet and NIC topology from real Az objects' -Skip:(-not $script:AzAvailable) {
    BeforeAll {
        $nsg = [Microsoft.Azure.Commands.Network.Models.PSNetworkSecurityGroup]::new()
        $nsg.Id = '/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/networkSecurityGroups/nsg-app'

        $addressSpace = [Microsoft.Azure.Commands.Network.Models.PSAddressSpace]::new()
        $addressSpace.AddressPrefixes = [System.Collections.Generic.List[string]]::new()
        $addressSpace.AddressPrefixes.Add('10.10.0.0/16')

        $subnet = [Microsoft.Azure.Commands.Network.Models.PSSubnet]::new()
        $subnet.Name = 'app'
        $subnet.Id = '/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/virtualNetworks/vnet-app/subnets/app'
        $subnet.AddressPrefix = [System.Collections.Generic.List[string]]::new()
        $subnet.AddressPrefix.Add('10.10.1.0/24')
        $subnet.NetworkSecurityGroup = $nsg

        $script:Vnet = [Microsoft.Azure.Commands.Network.Models.PSVirtualNetwork]::new()
        $script:Vnet.Name = 'vnet-app'
        $script:Vnet.ResourceGroupName = 'rg-net'
        $script:Vnet.Id = '/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/virtualNetworks/vnet-app'
        $script:Vnet.Location = 'eastus'
        $script:Vnet.AddressSpace = $addressSpace
        $script:Vnet.Subnets = [System.Collections.Generic.List[Microsoft.Azure.Commands.Network.Models.PSSubnet]]::new()
        $script:Vnet.Subnets.Add($subnet)

        $ipConfig = [Microsoft.Azure.Commands.Network.Models.PSNetworkInterfaceIpConfiguration]::new()
        $ipConfig.Name = 'ipconfig1'
        $ipConfig.PrivateIpAddress = '10.10.1.4'
        $script:Nic = [Microsoft.Azure.Commands.Network.Models.PSNetworkInterface]::new()
        $script:Nic.Name = 'nic-vm1'
        $script:Nic.Id = '/subscriptions/x/resourceGroups/rg-net/providers/Microsoft.Network/networkInterfaces/nic-vm1'
        $script:Nic.NetworkSecurityGroup = $nsg
        $script:Nic.IpConfigurations = [System.Collections.Generic.List[Microsoft.Azure.Commands.Network.Models.PSNetworkInterfaceIpConfiguration]]::new()
        $script:Nic.IpConfigurations.Add($ipConfig)
    }

    It 'maps address prefixes, subnet NSG and NIC private addresses' {
        Mock -ModuleName AzureFlowLogParser Set-AzContext { }
        Mock -ModuleName AzureFlowLogParser Get-AzContext { $null }
        Mock -ModuleName AzureFlowLogParser Get-AzStorageAccount { }
        Mock -ModuleName AzureFlowLogParser Get-AzNetworkWatcher { }
        Mock -ModuleName AzureFlowLogParser Get-AzVirtualNetwork { $script:Vnet }
        Mock -ModuleName AzureFlowLogParser Get-AzNetworkInterface { $script:Nic }

        $topology = & (Get-Module AzureFlowLogParser) { param($s) Get-FlowLogAzSubscriptionData -SubscriptionId $s } '00000000-0000-0000-0000-000000000001'
        $vnet = $topology.VirtualNetworks | Select-Object -First 1
        $vnet.AddressPrefixes | Should -Contain '10.10.0.0/16'
        $vnet.Subnets[0].AddressPrefixes | Should -Contain '10.10.1.0/24'
        $vnet.Subnets[0].NsgId | Should -Be $nsg.Id
        $topology.NetworkInterfaces[0].NsgId | Should -Be $nsg.Id
        $topology.NetworkInterfaces[0].PrivateIps | Should -Contain '10.10.1.4'
    }
}
