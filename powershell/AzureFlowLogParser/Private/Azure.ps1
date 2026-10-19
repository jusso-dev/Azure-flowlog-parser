# Azure access. Every az call goes through Invoke-FlowLogAzCli, so tests mock that one function and never reach Azure.

function Assert-FlowLogAzCli {
    # Azure mode needs the Azure CLI on PATH. -InputPath runs never get here.
    if ($null -eq (Get-Command -Name az -CommandType Application, ExternalScript -ErrorAction SilentlyContinue)) {
        throw 'Azure mode needs the Azure CLI (az) on PATH. Install it from https://learn.microsoft.com/cli/azure/install-azure-cli and run az login. Or use -InputPath for local flow log files.'
    }
}

function Invoke-FlowLogAzCli {
    # Runs az with the arguments as an array. Adds --only-show-errors and -o json, then parses stdout as JSON. A
    # failing call throws with az's own error text; the command line and any tokens are not printed. PowerShell expands
    # a bare wildcard argument against the working directory, so a wildcard value must be one token (--num-results=*).
    [CmdletBinding()]
    param([Parameter(Mandatory)][string[]]$Arguments)

    Assert-FlowLogAzCli
    $stderrPath = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('N') + '.err')
    try {
        $stdout = & az @Arguments --only-show-errors -o json 2> $stderrPath
        $exitCode = $LASTEXITCODE
        $stderr = if (Test-Path -LiteralPath $stderrPath -PathType Leaf) { [System.IO.File]::ReadAllText($stderrPath).Trim() } else { '' }
    }
    finally {
        Remove-Item -LiteralPath $stderrPath -ErrorAction SilentlyContinue
    }

    $text = (@($stdout) -join "`n").Trim()
    if ($exitCode -ne 0) {
        $command = (@($Arguments) | Select-Object -First 3) -join ' '
        $detail = if ($stderr) { $stderr } else { $text }
        throw "az $command failed: $detail"
    }
    # The JSON list is written one item at a time, so callers wrap the call in @(): an empty list gives zero items.
    if ($text -eq '') { return }
    return ConvertFrom-Json -InputObject $text -DateKind String
}

function Get-FlowLogAzSubscription {
    [CmdletBinding()]
    param([string[]]$SubscriptionId)

    $subscriptions = @(
        if ($SubscriptionId) {
            foreach ($id in $SubscriptionId) { Invoke-FlowLogAzCli -Arguments @('account', 'show', '--subscription', $id) }
        }
        else {
            Invoke-FlowLogAzCli -Arguments @('account', 'list', '--all')
        }
    )
    foreach ($subscription in $subscriptions) {
        if ((Get-FlowLogProperty -Object $subscription -Name 'state') -ne 'Enabled') { continue }
        [pscustomobject]@{
            Id       = [string](Get-FlowLogProperty -Object $subscription -Name 'id')
            Name     = [string](Get-FlowLogProperty -Object $subscription -Name 'name')
            TenantId = [string](Get-FlowLogProperty -Object $subscription -Name 'tenantId')
        }
    }
}

function Get-FlowLogSubnetPrefixes {
    # az shows one subnet prefix as addressPrefix and several as addressPrefixes.
    param($Subnet)

    $prefixes = @(Get-FlowLogProperty -Object $Subnet -Name 'addressPrefixes' | Where-Object { $_ })
    if ($prefixes.Count -eq 0) {
        $prefixes = @(Get-FlowLogProperty -Object $Subnet -Name 'addressPrefix' | Where-Object { $_ })
    }
    return $prefixes
}

function Get-FlowLogAzSubscriptionData {
    # Reads the inventory for one subscription. Each az call names the subscription, so the caller's account is untouched.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$SubscriptionId,
        [string]$TenantId,
        [string]$DisplayName
    )

    $scope = @('--subscription', $SubscriptionId)

    $storageAccounts = @(foreach ($account in @(Invoke-FlowLogAzCli -Arguments (@('storage', 'account', 'list') + $scope))) {
        [pscustomobject]@{
            SubscriptionId = $SubscriptionId
            Name           = [string](Get-FlowLogProperty -Object $account -Name 'name')
            ResourceGroup  = [string](Get-FlowLogProperty -Object $account -Name 'resourceGroup')
            Location       = [string](Get-FlowLogProperty -Object $account -Name 'location')
            Kind           = [string](Get-FlowLogProperty -Object $account -Name 'kind')
            SkuName        = [string](Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $account -Name 'sku') -Name 'name')
        }
    })

    # One flow-log list per watcher location. A region holds one Network Watcher per subscription, so the first is the watcher.
    $watchers = @(Invoke-FlowLogAzCli -Arguments (@('network', 'watcher', 'list') + $scope))
    $watcherNames = @{}
    $locations = [System.Collections.Generic.List[string]]::new()
    foreach ($watcher in $watchers) {
        $location = [string](Get-FlowLogProperty -Object $watcher -Name 'location')
        if ([string]::IsNullOrWhiteSpace($location) -or $watcherNames.ContainsKey($location)) { continue }
        $watcherNames[$location] = [string](Get-FlowLogProperty -Object $watcher -Name 'name')
        $locations.Add($location)
    }
    $flowLogs = @(foreach ($location in $locations) {
        $watcher = [pscustomobject]@{ Name = $watcherNames[$location] }
        foreach ($flowLog in @(Invoke-FlowLogAzCli -Arguments (@('network', 'watcher', 'flow-log', 'list', '--location', $location) + $scope))) {
            ConvertTo-FlowLogConfig -FlowLog $flowLog -Watcher $watcher -SubscriptionId $SubscriptionId
        }
    })

    $virtualNetworks = @(foreach ($vnet in @(Invoke-FlowLogAzCli -Arguments (@('network', 'vnet', 'list') + $scope))) {
        [pscustomobject]@{
            SubscriptionId  = $SubscriptionId
            Name            = [string](Get-FlowLogProperty -Object $vnet -Name 'name')
            ResourceGroup   = [string](Get-FlowLogProperty -Object $vnet -Name 'resourceGroup')
            Id              = [string](Get-FlowLogProperty -Object $vnet -Name 'id')
            Location        = [string](Get-FlowLogProperty -Object $vnet -Name 'location')
            AddressPrefixes = @(Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $vnet -Name 'addressSpace') -Name 'addressPrefixes' | Where-Object { $_ })
            Subnets         = @(foreach ($subnet in @(Get-FlowLogProperty -Object $vnet -Name 'subnets')) {
                [pscustomobject]@{
                    Name            = [string](Get-FlowLogProperty -Object $subnet -Name 'name')
                    Id              = [string](Get-FlowLogProperty -Object $subnet -Name 'id')
                    AddressPrefixes = @(Get-FlowLogSubnetPrefixes -Subnet $subnet)
                    NsgId           = [string](Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $subnet -Name 'networkSecurityGroup') -Name 'id')
                }
            })
        }
    })

    $networkInterfaces = @(foreach ($nic in @(Invoke-FlowLogAzCli -Arguments (@('network', 'nic', 'list') + $scope))) {
        [pscustomobject]@{
            Name       = [string](Get-FlowLogProperty -Object $nic -Name 'name')
            Id         = [string](Get-FlowLogProperty -Object $nic -Name 'id')
            NsgId      = [string](Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $nic -Name 'networkSecurityGroup') -Name 'id')
            PrivateIps = @(@(Get-FlowLogProperty -Object $nic -Name 'ipConfigurations') | ForEach-Object { Get-FlowLogProperty -Object $_ -Name 'privateIpAddress' } | Where-Object { $_ })
        }
    })

    [pscustomobject]@{
        SubscriptionId    = $SubscriptionId
        DisplayName       = [string]$DisplayName
        TenantId          = [string]$TenantId
        StorageAccounts   = $storageAccounts
        FlowLogs          = $flowLogs
        VirtualNetworks   = $virtualNetworks
        NetworkInterfaces = $networkInterfaces
    }
}

function ConvertTo-FlowLogConfig {
    # Maps one flat az flow-log object to the config shape the rest of the module uses.
    param($FlowLog, $Watcher, [string]$SubscriptionId)

    $target = [string](Get-FlowLogProperty -Object $FlowLog -Name 'targetResourceId')
    $storage = ConvertTo-FlowLogStorageRef -StorageId ([string](Get-FlowLogProperty -Object $FlowLog -Name 'storageId'))
    $retention = Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $FlowLog -Name 'retentionPolicy') -Name 'days'
    $analytics = Get-FlowLogProperty -Object (Get-FlowLogProperty -Object $FlowLog -Name 'flowAnalyticsConfiguration') -Name 'networkWatcherFlowAnalyticsConfiguration'

    [pscustomobject]@{
        SubscriptionId        = $SubscriptionId
        Name                  = [string](Get-FlowLogProperty -Object $FlowLog -Name 'name')
        NetworkWatcher        = [string](Get-FlowLogProperty -Object $Watcher -Name 'name')
        Location              = [string](Get-FlowLogProperty -Object $FlowLog -Name 'location')
        Target                = $target
        TargetKind            = Get-FlowLogTargetKind -TargetId $target
        TargetName            = Get-FlowLogTargetName -TargetId $target
        TargetResourceGroup   = Get-FlowLogTargetResourceGroup -TargetId $target
        Enabled               = [bool](Get-FlowLogProperty -Object $FlowLog -Name 'enabled')
        RetentionDays         = [int]$retention
        TrafficAnalytics      = [bool](Get-FlowLogProperty -Object $analytics -Name 'enabled')
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

function Get-FlowLogTargetName {
    # Last segment of the target ARM id (the NSG or VNet name), or '' when there is no target.
    param([string]$TargetId)

    if ([string]::IsNullOrWhiteSpace($TargetId)) { return '' }
    return ($TargetId.TrimEnd('/').Split('/'))[-1]
}

function Get-FlowLogTargetResourceGroup {
    # Segment after /resourceGroups/ (case-insensitive), or '' when the id has none.
    param([string]$TargetId)

    if ([string]::IsNullOrEmpty($TargetId)) { return '' }
    $match = [regex]::Match($TargetId, '(?i)/resourceGroups/([^/]+)')
    if ($match.Success) { return $match.Groups[1].Value }
    return ''
}

function New-FlowLogStorageContext {
    # Storage calls use --auth-mode login, so the context is just the account name.
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$StorageAccountName)

    [pscustomobject]@{ Name = $StorageAccountName }
}

function Test-FlowLogContainer {
    [CmdletBinding()]
    param($Context)

    $result = Invoke-FlowLogAzCli -Arguments @('storage', 'container', 'exists', '--account-name', $Context.Name, '--name', $script:FlowLogContainerName, '--auth-mode', 'login')
    return [bool](Get-FlowLogProperty -Object $result -Name 'exists')
}

function Get-FlowLogBlobList {
    # --num-results=* returns every blob, as the Az listing did. A positive MaxCount caps the listing. The '=' form
    # keeps PowerShell from expanding a bare * against the current directory.
    [CmdletBinding()]
    param($Context, [string]$Prefix, [int]$MaxCount = 0)

    $numResults = if ($MaxCount -gt 0) { [string]$MaxCount } else { '*' }
    $blobs = @(Invoke-FlowLogAzCli -Arguments @(
        'storage', 'blob', 'list', '--account-name', $Context.Name, '--container-name', $script:FlowLogContainerName,
        "--prefix=$Prefix", "--num-results=$numResults", '--auth-mode', 'login'
    ))
    foreach ($blob in $blobs) {
        $properties = Get-FlowLogProperty -Object $blob -Name 'properties'
        [pscustomobject]@{
            Name            = [string](Get-FlowLogProperty -Object $blob -Name 'name')
            LastModifiedUtc = [datetimeoffset]::Parse([string](Get-FlowLogProperty -Object $properties -Name 'lastModified'), [cultureinfo]::InvariantCulture).UtcDateTime
            Length          = [long](Get-FlowLogProperty -Object $properties -Name 'contentLength')
        }
    }
}

function Read-FlowLogBlobText {
    [CmdletBinding()]
    param($Context, [Parameter(Mandatory)][string]$Name)

    $destination = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('N') + '.json')
    try {
        Invoke-FlowLogAzCli -Arguments @(
            'storage', 'blob', 'download', '--account-name', $Context.Name, '--container-name', $script:FlowLogContainerName,
            "--name=$Name", '--file', $destination, '--auth-mode', 'login', '--no-progress'
        ) | Out-Null
        return [System.IO.File]::ReadAllText($destination)
    }
    finally {
        Remove-Item -LiteralPath $destination -ErrorAction SilentlyContinue
    }
}
