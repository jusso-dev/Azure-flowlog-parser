# Azure access wrappers. Every Az cmdlet call lives here so tests can mock these functions.

function Assert-FlowLogAzModule {
    # Az modules load on first Azure call, so -InputPath runs need no Az install.
    if ($script:AzModulesLoaded) { return }
    foreach ($name in $script:AzMinimumVersions.Keys) {
        $minimum = $script:AzMinimumVersions[$name]
        if (-not (Get-Module -ListAvailable -Name $name | Where-Object { $_.Version -ge [version]$minimum })) {
            throw "Azure mode needs $name >= $minimum. Run: Install-Module $name -MinimumVersion $minimum -Scope CurrentUser. Or use -InputPath for local flow log files."
        }
        Import-Module -Name $name -MinimumVersion $minimum -ErrorAction Stop
    }
    $script:AzModulesLoaded = $true
}

function Get-FlowLogAzSubscription {
    [CmdletBinding()]
    param([string[]]$SubscriptionId)

    Assert-FlowLogAzModule
    $quiet = @{ WarningAction = 'SilentlyContinue' }
    $subscriptions = if ($SubscriptionId) {
        foreach ($id in $SubscriptionId) { Get-AzSubscription -SubscriptionId $id @quiet }
    }
    else {
        Get-AzSubscription @quiet
    }
    foreach ($subscription in $subscriptions) {
        if ($subscription.State -ne 'Enabled') { continue }
        [pscustomobject]@{
            Id       = $subscription.Id
            Name     = $subscription.Name
            TenantId = $subscription.TenantId
        }
    }
}

function Get-FlowLogAzSubscriptionData {
    # Reads ARM objects for one subscription, then restores the caller's Az context.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$SubscriptionId,
        [string]$TenantId
    )

    Assert-FlowLogAzModule
    $original = Get-AzContext
    $tenant = @{}
    if ($TenantId) { $tenant.TenantId = $TenantId }
    try {
        Set-AzContext -SubscriptionId $SubscriptionId @tenant -ErrorAction Stop | Out-Null

        $storageAccounts = @(foreach ($account in @(Get-AzStorageAccount -ErrorAction Stop)) {
            [pscustomobject]@{
                SubscriptionId = $SubscriptionId
                Name           = $account.StorageAccountName
                ResourceGroup  = $account.ResourceGroupName
                Location       = $account.Location
                Kind           = [string]$account.Kind
                SkuName        = [string](Get-FlowLogProperty -Object $account.Sku -Name 'Name')
            }
        })

        $flowLogs = @(foreach ($watcher in @(Get-AzNetworkWatcher -ErrorAction Stop)) {
            foreach ($flowLog in @(Get-AzNetworkWatcherFlowLog -NetworkWatcher $watcher -ErrorAction Stop)) {
                ConvertTo-FlowLogConfig -FlowLog $flowLog -Watcher $watcher -SubscriptionId $SubscriptionId
            }
        })

        $virtualNetworks = @(foreach ($vnet in @(Get-AzVirtualNetwork -ErrorAction Stop)) {
            [pscustomobject]@{
                SubscriptionId  = $SubscriptionId
                Name            = $vnet.Name
                ResourceGroup   = $vnet.ResourceGroupName
                Id              = $vnet.Id
                Location        = $vnet.Location
                AddressPrefixes = @($vnet.AddressSpace.AddressPrefixes)
                Subnets         = @(foreach ($subnet in @($vnet.Subnets)) {
                    [pscustomobject]@{
                        Name            = $subnet.Name
                        Id              = $subnet.Id
                        AddressPrefixes = @($subnet.AddressPrefix)
                        NsgId           = [string](Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $subnet -Name 'NetworkSecurityGroup') -Name 'Id')
                    }
                })
            }
        })

        $networkInterfaces = @(foreach ($nic in @(Get-AzNetworkInterface -ErrorAction Stop)) {
            [pscustomobject]@{
                Name       = $nic.Name
                Id         = $nic.Id
                NsgId      = [string](Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $nic -Name 'NetworkSecurityGroup') -Name 'Id')
                PrivateIps = @($nic.IpConfigurations | ForEach-Object { $_.PrivateIpAddress })
            }
        })

        [pscustomobject]@{
            SubscriptionId    = $SubscriptionId
            StorageAccounts   = $storageAccounts
            FlowLogs          = $flowLogs
            VirtualNetworks   = $virtualNetworks
            NetworkInterfaces = $networkInterfaces
        }
    }
    finally {
        if ($null -ne $original) { Set-AzContext -Context $original | Out-Null }
    }
}

function ConvertTo-FlowLogConfig {
    param($FlowLog, $Watcher, [string]$SubscriptionId)

    $target = [string]$FlowLog.TargetResourceId
    $storage = ConvertTo-FlowLogStorageRef -StorageId ([string]$FlowLog.StorageId)
    $retention = Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $FlowLog -Name 'RetentionPolicy') -Name 'Days'
    $analytics = Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $FlowLog -Name 'FlowAnalyticsConfiguration') -Name 'NetworkWatcherFlowAnalyticsConfiguration'

    [pscustomobject]@{
        SubscriptionId        = $SubscriptionId
        Name                  = $FlowLog.Name
        NetworkWatcher        = $Watcher.Name
        Location              = $FlowLog.Location
        Target                = $target
        TargetKind            = Get-FlowLogTargetKind -TargetId $target
        Enabled               = [bool]$FlowLog.Enabled
        RetentionDays         = [int]$retention
        TrafficAnalytics      = [bool](Get-FlowLogProperty -Object $analytics -Name 'Enabled')
        StorageAccount        = $storage.Name
        StorageSubscriptionId = $storage.SubscriptionId
        StorageResourceGroup  = $storage.ResourceGroup
    }
}

function ConvertTo-FlowLogStorageRef {
    param([string]$StorageId)

    $segments = @($StorageId.Trim('/').Split('/'))
    $reference = [pscustomobject]@{ Name = $segments[-1]; SubscriptionId = ''; ResourceGroup = '' }
    for ($i = 0; $i -lt $segments.Count - 1; $i++) {
        if ($segments[$i] -eq 'subscriptions') { $reference.SubscriptionId = $segments[$i + 1] }
        if ($segments[$i] -eq 'resourceGroups') { $reference.ResourceGroup = $segments[$i + 1] }
    }
    return $reference
}

function Get-FlowLogTargetKind {
    param([string]$TargetId)

    if ($TargetId -match '/providers/Microsoft\.Network/networkSecurityGroups/[^/]+$') { return 'NSG' }
    if ($TargetId -match '/providers/Microsoft\.Network/virtualNetworks/[^/]+$') { return 'VNet' }
    return 'Other'
}

function New-FlowLogStorageContext {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$StorageAccountName)

    Assert-FlowLogAzModule
    New-AzStorageContext -StorageAccountName $StorageAccountName -UseConnectedAccount -ErrorAction Stop
}

function Test-FlowLogContainer {
    [CmdletBinding()]
    param($Context)

    Assert-FlowLogAzModule
    $match = Get-AzStorageContainer -Context $Context -Prefix $script:FlowLogContainerName -ErrorAction Stop |
        Where-Object { $_.Name -eq $script:FlowLogContainerName }
    return [bool]$match
}

function Get-FlowLogBlobList {
    [CmdletBinding()]
    param($Context, [string]$Prefix, [int]$MaxCount = 0)

    Assert-FlowLogAzModule
    $splat = @{
        Container   = $script:FlowLogContainerName
        Prefix      = $Prefix
        Context     = $Context
        ErrorAction = 'Stop'
    }
    if ($MaxCount -gt 0) { $splat.MaxCount = $MaxCount }
    Get-AzStorageBlob @splat | ForEach-Object {
        [pscustomobject]@{
            Name            = $_.Name
            LastModifiedUtc = $_.LastModified.UtcDateTime
            Length          = $_.Length
        }
    }
}

function Read-FlowLogBlobText {
    [CmdletBinding()]
    param($Context, [Parameter(Mandatory)][string]$Name)

    Assert-FlowLogAzModule
    $destination = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('N') + '.json')
    try {
        Get-AzStorageBlobContent -Container $script:FlowLogContainerName -Blob $Name -Context $Context -Destination $destination -Force -ErrorAction Stop | Out-Null
        return [System.IO.File]::ReadAllText($destination)
    }
    finally {
        Remove-Item -LiteralPath $destination -ErrorAction SilentlyContinue
    }
}
