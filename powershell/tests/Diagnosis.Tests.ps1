#Requires -Version 7.6

BeforeAll {
    $script:ModulePath = Join-Path $PSScriptRoot '..' 'AzureFlowLogParser' 'AzureFlowLogParser.psd1'
    Import-Module $script:ModulePath -Force
    Mock -ModuleName AzureFlowLogParser Invoke-FlowLogAzCli { throw 'Invoke-FlowLogAzCli was not mocked: no test may call az.' }

    $script:Sub = '11111111-1111-1111-1111-111111111111'
    $script:VnetId = "/subscriptions/$($script:Sub)/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/vnet1"
    $script:SubnetId = "$($script:VnetId)/subnets/app"
    $script:NicId = "/subscriptions/$($script:Sub)/resourceGroups/rg/providers/Microsoft.Network/networkInterfaces/nic1"
    $script:NsgId = "/subscriptions/$($script:Sub)/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/nsg1"

    function script:New-DiagnosisLog {
        param([string]$Name, [string]$Target, [string]$Kind, [bool]$Enabled, [string]$Status, [string]$Latest)
        [pscustomobject]@{
            SubscriptionId = $script:Sub; Name = $Name; StorageAccount = 'stlogs'
            NetworkWatcher = 'NetworkWatcher_australiaeast'; Location = 'australiaeast'
            Target = $Target; TargetKind = $Kind
            TargetName = ($Target.Trim('/').Split('/'))[-1]; TargetResourceGroup = 'rg'
            Enabled = $Enabled; Status = $Status; LatestBlobUtc = $Latest
            ProvisioningState = 'Succeeded'; HasManagedIdentity = $false; Message = ''; AgeHours = $null
        }
    }

    function script:New-DiagnosisArm {
        param([object[]]$Logs = @())
        [pscustomobject]@{
            SubscriptionId    = $script:Sub
            NetworkWatchers   = @([pscustomobject]@{ SubscriptionId = $script:Sub; Name = 'NetworkWatcher_australiaeast'; Location = 'australiaeast' })
            StorageAccounts   = @([pscustomobject]@{
                SubscriptionId = $script:Sub; Name = 'stlogs'; ResourceGroup = 'rg'; Location = 'australiaeast'
                Kind = 'StorageV2'; SkuName = 'Standard_LRS'; PublicNetworkAccess = 'Enabled'
                NetworkDefaultAction = 'Allow'; AllowsTrustedServices = $true; AllowSharedKeyAccess = $true; Status = ''
            })
            FlowLogs          = @($Logs)
            VirtualNetworks   = @([pscustomobject]@{
                SubscriptionId = $script:Sub; Name = 'vnet1'; ResourceGroup = 'rg'; Id = $script:VnetId; Location = 'australiaeast'
                Subnets = @([pscustomobject]@{ Name = 'app'; Id = $script:SubnetId; NsgId = $script:NsgId })
            })
            NetworkInterfaces = @([pscustomobject]@{
                Name = 'nic1'; Id = $script:NicId; Location = 'australiaeast'; SubnetId = $script:SubnetId; NsgId = $script:NsgId
            })
        }
    }

    function script:Invoke-Diagnosis {
        param($Arm, [string[]]$Filter)
        InModuleScope AzureFlowLogParser -Parameters @{ ArmData = @($Arm); Filter = $Filter } {
            Get-FlowLogSourceFindings -ArmData $ArmData -StaleAfterHours 6 -StorageAccountName $Filter
        }
    }
}

Describe 'Flow log diagnosis' {
    It 'classifies subnet and NIC targets ahead of the virtual network' {
        $kinds = InModuleScope AzureFlowLogParser -Parameters @{ Vnet = $script:VnetId; Subnet = $script:SubnetId; Nic = $script:NicId } {
            @(
                (Get-FlowLogTargetKind $Vnet),
                (Get-FlowLogTargetKind $Subnet),
                (Get-FlowLogTargetKind $Nic),
                (Get-FlowLogTargetKind '')
            )
        }
        $kinds | Should -Be @('VNet', 'Subnet', 'NIC', 'Other')
    }

    It 'reports sources without a flow log as not configured' {
        $result = Invoke-Diagnosis (New-DiagnosisArm)
        $result.SourceCount | Should -Be 4
        @($result.Findings.kind) | Should -Be @('VNet', 'Subnet', 'NIC', 'NSG')
        @($result.Findings | Where-Object kind -ne 'NSG').reason | Should -Be @('NotConfigured', 'NotConfigured', 'NotConfigured')
        ($result.Findings | Where-Object kind -eq 'NSG').reason | Should -Be 'NotConfigured'
        ($result.Findings | Where-Object kind -eq 'NSG').message | Should -Match '30 Sep 2027'
    }

    It 'names a missing Network Watcher' {
        $arm = New-DiagnosisArm
        $arm.NetworkWatchers = @()
        $result = Invoke-Diagnosis $arm
        @($result.Findings.reason) | Should -Be @('NetworkWatcherMissing', 'NetworkWatcherMissing', 'NetworkWatcherMissing', 'NetworkWatcherMissing')
    }

    It 'lets a delivering virtual network flow log cover children and the attached NSG' {
        $log = New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'Healthy' '2026-10-24T00:00:00Z'
        $result = Invoke-Diagnosis (New-DiagnosisArm $log)
        $result.SourceCount | Should -Be 4
        @($result.Findings).Count | Should -Be 0
    }

    It 'reports a disabled parent for every child' {
        $log = New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $false 'Disabled' $null
        $result = Invoke-Diagnosis (New-DiagnosisArm $log)
        @($result.Findings).Count | Should -Be 4
        @($result.Findings | Where-Object kind -ne 'NSG').reason | Should -Be @('Disabled', 'Disabled', 'Disabled')
        ($result.Findings | Where-Object kind -eq 'NIC').message | Should -Match 'parent virtual network vnet1'
    }

    It 'lets an enabled NIC flow log win over a healthy virtual network flow log' {
        $vnet = New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'Healthy' '2026-10-24T00:00:00Z'
        $nic = New-DiagnosisLog 'fl-nic' $script:NicId 'NIC' $true 'NoData' $null
        $result = Invoke-Diagnosis (New-DiagnosisArm @($vnet, $nic))
        ($result.Findings | Where-Object kind -eq 'NIC').reason | Should -Be 'NoData'
        ($result.Findings | Where-Object kind -eq 'NIC').flowLog | Should -Be 'fl-nic'
        ($result.Findings | Where-Object kind -eq 'NSG').reason | Should -Be 'NotConfigured'
    }

    It 'reports a firewall block before an empty container' {
        $arm = New-DiagnosisArm (New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'NoData' $null)
        $arm.StorageAccounts[0].NetworkDefaultAction = 'Deny'
        $arm.StorageAccounts[0].AllowsTrustedServices = $false
        $result = Invoke-Diagnosis $arm
        @($result.Findings | Where-Object kind -ne 'NSG').reason | Should -Be @('StorageFirewall', 'StorageFirewall', 'StorageFirewall')
    }

    It 'does not treat a trusted-services exception as a firewall block' {
        $arm = New-DiagnosisArm (New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'NoData' $null)
        $arm.StorageAccounts[0].NetworkDefaultAction = 'Deny'
        $arm.StorageAccounts[0].AllowsTrustedServices = $true
        $result = Invoke-Diagnosis $arm
        ($result.Findings | Where-Object kind -eq 'VNet').reason | Should -Be 'NoData'
    }

    It 'names premium storage and a region mismatch' {
        $log = New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'NoData' $null
        $premium = New-DiagnosisArm $log
        $premium.StorageAccounts[0].SkuName = 'Premium_LRS'
        (Invoke-Diagnosis $premium).Findings | Where-Object kind -eq 'VNet' | ForEach-Object reason | Should -Be 'UnsupportedStorage'

        $other = New-DiagnosisArm $log
        $other.StorageAccounts[0].Location = 'eastus'
        (Invoke-Diagnosis $other).Findings | Where-Object kind -eq 'VNet' | ForEach-Object reason | Should -Be 'RegionMismatch'

        $spaced = New-DiagnosisArm $log
        $spaced.StorageAccounts[0].Location = 'Australia East'
        (Invoke-Diagnosis $spaced).Findings | Where-Object kind -eq 'VNet' | ForEach-Object reason | Should -Be 'NoData'
    }

    It 'reports shared key off when the flow log has no identity' {
        $arm = New-DiagnosisArm (New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'NoData' $null)
        $arm.StorageAccounts[0].AllowSharedKeyAccess = $false
        (Invoke-Diagnosis $arm).Findings | Where-Object kind -eq 'VNet' | ForEach-Object reason | Should -Be 'SharedKeyDisabled'

        $arm.FlowLogs[0].HasManagedIdentity = $true
        (Invoke-Diagnosis $arm).Findings | Where-Object kind -eq 'VNet' | ForEach-Object reason | Should -Be 'NoData'
    }

    It 'reports a deleted target and failed provisioning' {
        $gone = New-DiagnosisLog 'fl-old' "$($script:VnetId)-deleted" 'VNet' $true 'NoData' $null
        $gone.TargetName = 'vnet-deleted'
        $failed = New-DiagnosisLog 'fl-bad' $script:VnetId 'VNet' $true 'NoData' $null
        $failed.ProvisioningState = 'Failed'
        $result = Invoke-Diagnosis (New-DiagnosisArm @($gone, $failed))
        ($result.Findings | Where-Object name -eq 'vnet1').reason | Should -Be 'ProvisioningFailed'
        ($result.Findings | Where-Object name -eq 'vnet-deleted').reason | Should -Be 'TargetMissing'
    }

    It 'names the stale age and the limit' {
        $log = New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'Stale' '2026-10-20T00:00:00Z'
        $log.AgeHours = 30
        $finding = (Invoke-Diagnosis (New-DiagnosisArm $log)).Findings | Where-Object kind -eq 'VNet'
        $finding.reason | Should -Be 'Stale'
        $finding.message | Should -Match '30.0 hours'
        $finding.message | Should -Match 'limit is 6 hours'
    }

    It 'keeps unconfigured sources when a storage account filter is set' {
        $log = New-DiagnosisLog 'fl-vnet' $script:VnetId 'VNet' $true 'NoData' $null
        $log.StorageAccount = 'stother'
        $result = Invoke-Diagnosis (New-DiagnosisArm $log) @('stlogs')
        @($result.Findings | Where-Object { $_.kind -eq 'NSG' -and $_.storageAccount -eq '' }).Count | Should -Be 1
        @($result.Findings | Where-Object storageAccount -eq 'stother').Count | Should -Be 0
    }
}
