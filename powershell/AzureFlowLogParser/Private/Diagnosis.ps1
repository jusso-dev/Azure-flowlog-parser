# Why virtual networks, subnets, NICs, and attached NSGs are not writing flow logs to storage.
# An enabled NIC flow log wins over its subnet, which wins over its virtual network. A delivering parent covers its children.

$script:NsgFlowContainer = 'insights-logs-networksecuritygroupflowevent'

function Get-FlowLogDiagnosisRank {
    param([string]$Reason)

    $order = @(
        'ProvisioningFailed', 'TargetMissing', 'StorageMissing', 'RegionMismatch', 'UnsupportedStorage',
        'StorageFirewall', 'SharedKeyDisabled', 'Error', 'AccessDenied', 'NoData', 'Stale', 'Disabled',
        'NetworkWatcherMissing', 'NotConfigured'
    )
    $index = [array]::IndexOf($order, $Reason)
    if ($index -lt 0) { return $order.Count }
    return $index
}

function Get-FlowLogKindRank {
    param([string]$Kind)

    switch ($Kind) {
        'VNet' { return 0 }
        'Subnet' { return 1 }
        'NIC' { return 2 }
        'NSG' { return 3 }
        default { return 4 }
    }
}

function Get-FlowLogKindLabel {
    param([string]$Kind)

    switch ($Kind) {
        'VNet' { return 'virtual network' }
        'Subnet' { return 'subnet' }
        'NIC' { return 'network interface' }
        'NSG' { return 'network security group' }
        default { return 'resource' }
    }
}

function Get-FlowLogDiagText {
    param($Object, [string]$Pascal, [string]$Camel)

    $value = Get-FlowLogProperty -Object $Object -Name $Pascal
    if ($null -eq $value -and $Camel) { $value = Get-FlowLogProperty -Object $Object -Name $Camel }
    if ($null -eq $value) { return '' }
    return [string]$value
}

function Test-FlowLogDiagBool {
    param($Value)

    if ($null -eq $Value) { return $false }
    if ($Value -is [bool]) { return $Value }
    return "$Value" -eq 'true'
}

function Get-FlowLogDiagList {
    param($Object, [string]$Name)

    $value = Get-FlowLogProperty -Object $Object -Name $Name
    if ($null -eq $value) { return @() }
    return @($value)
}

function Get-FlowLogNormalizedLocation {
    param([string]$Location)

    if ([string]::IsNullOrWhiteSpace($Location)) { return '' }
    return ($Location.Replace(' ', '').Replace('-', '')).ToLowerInvariant()
}

function Get-FlowLogParentVnetId {
    param([string]$Id)

    if ([string]::IsNullOrWhiteSpace($Id)) { return $null }
    $index = $Id.ToLowerInvariant().IndexOf('/subnets/')
    if ($index -lt 1) { return $null }
    return $Id.Substring(0, $index)
}

function Get-FlowLogArmSegment {
    param([string]$Id, [string]$Marker)

    if ([string]::IsNullOrEmpty($Id)) { return '' }
    $parts = @($Id.Split('/', [System.StringSplitOptions]::RemoveEmptyEntries))
    for ($i = 0; $i -lt $parts.Count - 1; $i++) {
        if ($parts[$i] -eq $Marker) { return $parts[$i + 1] }
    }
    return ''
}

function Test-FlowLogDelivering {
    param($Config, [string]$Status, [string]$LatestBlobUtc)

    $enabled = Test-FlowLogDiagBool (Get-FlowLogProperty -Object $Config -Name 'Enabled')
    if (-not $enabled) { $enabled = Test-FlowLogDiagBool (Get-FlowLogProperty -Object $Config -Name 'enabled') }
    return $enabled -and $Status -eq 'Healthy' -and -not [string]::IsNullOrEmpty($LatestBlobUtc)
}

function Get-FlowLogSourceChain {
    param($Source)

    $ids = [System.Collections.Generic.List[string]]::new()
    $ids.Add($Source.Id)
    if ($Source.Kind -eq 'NIC' -and -not [string]::IsNullOrWhiteSpace($Source.SubnetId)) { $ids.Add($Source.SubnetId) }
    if (($Source.Kind -eq 'NIC' -or $Source.Kind -eq 'Subnet') -and -not [string]::IsNullOrWhiteSpace($Source.VnetId)) { $ids.Add($Source.VnetId) }
    return $ids
}

function Add-FlowLogNsgNote {
    param($Config, [string]$Message)

    if ((Get-FlowLogDiagText -Object $Config -Pascal 'TargetKind' -Camel 'targetKind') -ne 'NSG') { return $Message }
    return "$Message NSG flow logs retire on 30 Sep 2027 and new ones cannot be created."
}

function Get-FlowLogScopeText {
    param($Config, $Source)

    $target = Get-FlowLogDiagText -Object $Config -Pascal 'Target' -Camel 'target'
    if ($target -eq $Source.Id) { return "this $(Get-FlowLogKindLabel $Source.Kind)" }
    $name = Get-FlowLogDiagText -Object $Config -Pascal 'TargetName' -Camel 'targetName'
    if ([string]::IsNullOrWhiteSpace($name)) { $name = 'network' }
    $kind = Get-FlowLogDiagText -Object $Config -Pascal 'TargetKind' -Camel 'targetKind'
    return "parent $(Get-FlowLogKindLabel $kind) $name"
}

function Test-FlowLogProvisioningBlocked {
    param([string]$State)

    if ([string]::IsNullOrWhiteSpace($State)) { return $false }
    return $State -ne 'Succeeded' -and $State -ne 'Updating'
}

function Test-FlowLogStorageUnsupported {
    param($Account)

    $sku = Get-FlowLogDiagText -Object $Account -Pascal 'SkuName' -Camel 'skuName'
    if ($sku.StartsWith('Premium', [System.StringComparison]::OrdinalIgnoreCase)) { return $true }
    $kind = Get-FlowLogDiagText -Object $Account -Pascal 'Kind' -Camel 'kind'
    return $kind -eq 'FileStorage' -or $kind -eq 'BlockBlobStorage'
}

function Test-FlowLogFirewallBlocked {
    param($Account)

    $trusted = Get-FlowLogProperty -Object $Account -Name 'AllowsTrustedServices'
    if ($null -eq $trusted) { $trusted = Get-FlowLogProperty -Object $Account -Name 'allowsTrustedServices' }
    if ($trusted -ne $false) { return $false }
    $access = Get-FlowLogDiagText -Object $Account -Pascal 'PublicNetworkAccess' -Camel 'publicNetworkAccess'
    if ($access -eq 'Disabled' -or $access -eq 'SecuredByPerimeter') { return $true }
    $action = Get-FlowLogDiagText -Object $Account -Pascal 'NetworkDefaultAction' -Camel 'networkDefaultAction'
    return $action -eq 'Deny'
}

function Test-FlowLogLocationsDiffer {
    param([string]$StorageLocation, [string]$ResourceLocation)

    $storage = Get-FlowLogNormalizedLocation $StorageLocation
    $resource = Get-FlowLogNormalizedLocation $ResourceLocation
    return $storage.Length -gt 0 -and $resource.Length -gt 0 -and $storage -ne $resource
}

function Get-FlowLogConfigVerdict {
    param($Config, $Source, $Context, [string]$Status, [string]$LatestBlobUtc, $AgeHours, [string]$HealthMessage)

    $scope = Get-FlowLogScopeText -Config $Config -Source $Source
    $name = Get-FlowLogDiagText -Object $Config -Pascal 'Name' -Camel 'name'
    $storageName = Get-FlowLogDiagText -Object $Config -Pascal 'StorageAccount' -Camel 'storageAccount'
    $enabled = Test-FlowLogDiagBool (Get-FlowLogProperty -Object $Config -Name 'Enabled')
    if (-not $enabled) { $enabled = Test-FlowLogDiagBool (Get-FlowLogProperty -Object $Config -Name 'enabled') }
    if (-not $enabled) {
        return @{
            Reason  = 'Disabled'
            Message = (Add-FlowLogNsgNote $Config "Flow log $name targets $scope but is disabled, so nothing is written to $storageName.")
        }
    }

    $state = Get-FlowLogDiagText -Object $Config -Pascal 'ProvisioningState' -Camel 'provisioningState'
    if (Test-FlowLogProvisioningBlocked $state) {
        return @{
            Reason  = 'ProvisioningFailed'
            Message = (Add-FlowLogNsgNote $Config "Flow log $name provisioning state is $state, so Azure is not writing it to $storageName.")
        }
    }

    $target = Get-FlowLogDiagText -Object $Config -Pascal 'Target' -Camel 'target'
    if (-not $Context.Known.ContainsKey($target.ToLowerInvariant())) {
        return @{
            Reason  = 'TargetMissing'
            Message = (Add-FlowLogNsgNote $Config "Flow log $name targets $target, which is not in the scanned subscriptions. A missing virtual network, subnet, network interface, or network security group sends nothing.")
        }
    }

    $accountKey = $storageName.ToLowerInvariant()
    if (-not $Context.Accounts.ContainsKey($accountKey)) {
        return @{
            Reason  = 'StorageMissing'
            Message = (Add-FlowLogNsgNote $Config "Flow log $name sends to storage account $storageName, which is not in the scanned subscriptions. The account may have been deleted, or it lives in a subscription this identity cannot read.")
        }
    }
    $account = $Context.Accounts[$accountKey]
    $accountLocation = Get-FlowLogDiagText -Object $account -Pascal 'Location' -Camel 'location'
    $resourceLocation = if ([string]::IsNullOrWhiteSpace($Source.Location)) { Get-FlowLogDiagText -Object $Config -Pascal 'Location' -Camel 'location' } else { $Source.Location }
    if (Test-FlowLogLocationsDiffer $accountLocation $resourceLocation) {
        $label = Get-FlowLogKindLabel $Source.Kind
        return @{
            Reason  = 'RegionMismatch'
            Message = (Add-FlowLogNsgNote $Config "Storage account $storageName is in $accountLocation. $label $($Source.Name) is in $resourceLocation. Flow logs are written only when the storage account is in the same region as the virtual network or network security group.")
        }
    }
    if (Test-FlowLogStorageUnsupported $account) {
        $kind = Get-FlowLogDiagText -Object $account -Pascal 'Kind' -Camel 'kind'
        $sku = Get-FlowLogDiagText -Object $account -Pascal 'SkuName' -Camel 'skuName'
        return @{
            Reason  = 'UnsupportedStorage'
            Message = (Add-FlowLogNsgNote $Config "Storage account $storageName is $kind $sku. Flow logs need a Standard general-purpose storage account. Premium storage is not supported.")
        }
    }
    if (Test-FlowLogFirewallBlocked $account) {
        $access = Get-FlowLogDiagText -Object $account -Pascal 'PublicNetworkAccess' -Camel 'publicNetworkAccess'
        $action = Get-FlowLogDiagText -Object $account -Pascal 'NetworkDefaultAction' -Camel 'networkDefaultAction'
        if ([string]::IsNullOrWhiteSpace($access)) { $access = 'unknown' }
        if ([string]::IsNullOrWhiteSpace($action)) { $action = 'unknown' }
        return @{
            Reason  = 'StorageFirewall'
            Message = (Add-FlowLogNsgNote $Config "Storage account $storageName does not allow Network Watcher to write blobs. Public network access is $access, the firewall default action is $action, and trusted Azure services are not allowed. Allow Azure services on the trusted services list.")
        }
    }

    $shared = Get-FlowLogProperty -Object $account -Name 'AllowSharedKeyAccess'
    if ($null -eq $shared) { $shared = Get-FlowLogProperty -Object $account -Name 'allowSharedKeyAccess' }
    $hasIdentity = Test-FlowLogDiagBool (Get-FlowLogProperty -Object $Config -Name 'HasManagedIdentity')
    if (-not $hasIdentity) { $hasIdentity = Test-FlowLogDiagBool (Get-FlowLogProperty -Object $Config -Name 'hasManagedIdentity') }
    if ($shared -eq $false -and -not $hasIdentity) {
        return @{
            Reason  = 'SharedKeyDisabled'
            Message = (Add-FlowLogNsgNote $Config "Storage account $storageName disables shared key access, and flow log $name has no managed identity. Network Watcher cannot authenticate to the account. Assign a user-assigned managed identity to the flow log, or allow shared key access, then disable and re-enable the flow log.")
        }
    }

    return Get-FlowLogBlobVerdict -Config $Config -Account $account -Status $Status -LatestBlobUtc $LatestBlobUtc -AgeHours $AgeHours -HealthMessage $HealthMessage -StaleAfterHours $Context.StaleAfterHours
}

function Get-FlowLogBlobVerdict {
    param($Config, $Account, [string]$Status, [string]$LatestBlobUtc, $AgeHours, [string]$HealthMessage, [int]$StaleAfterHours)

    $name = Get-FlowLogDiagText -Object $Config -Pascal 'Name' -Camel 'name'
    $storageName = Get-FlowLogDiagText -Object $Account -Pascal 'Name' -Camel 'name'
    if ($Status -eq 'AccessDenied') {
        return @{
            Reason  = 'AccessDenied'
            Message = (Add-FlowLogNsgNote $Config "Flow log $name is enabled, but blobs in $storageName could not be listed ($HealthMessage). Grant Storage Blob Data Reader on the account to confirm whether logs are arriving.")
        }
    }
    $accountStatus = Get-FlowLogDiagText -Object $Account -Pascal 'Status' -Camel 'status'
    if ($Status -eq 'Error' -or ($Status -eq 'Healthy' -and [string]::IsNullOrEmpty($LatestBlobUtc) -and $accountStatus -eq 'Error')) {
        $detail = if ([string]::IsNullOrWhiteSpace($HealthMessage)) { 'the health check failed' } else { $HealthMessage }
        return @{ Reason = 'Error'; Message = (Add-FlowLogNsgNote $Config "Flow log $name could not be checked: $detail") }
    }
    if ($Status -eq 'Stale') {
        $age = if ($null -eq $AgeHours -or "$AgeHours" -eq '') { 'unknown' } else { ([double]$AgeHours).ToString('0.0', [cultureinfo]::InvariantCulture) }
        return @{
            Reason  = 'Stale'
            Message = (Add-FlowLogNsgNote $Config "Flow log $name last wrote to $storageName $age hours ago. The limit is $StaleAfterHours hours, so new blobs are not arriving.")
        }
    }
    $gap = ''
    if ((Get-FlowLogDiagText -Object $Config -Pascal 'TargetKind' -Camel 'targetKind') -eq 'NSG') {
        $gap = " The blob check reads $($script:FlowLogContainerName). NSG flow logs are stored in $($script:NsgFlowContainer), so this does not prove that container is empty."
    }
    return @{
        Reason  = 'NoData'
        Message = (Add-FlowLogNsgNote $Config "Flow log $name is enabled and $storageName accepts flow logs, but no blobs were written in the last 2 days. Rotating the storage account keys stops flow logs until the flow log is disabled and enabled again. No traffic on the source also leaves the container empty.$gap")
    }
}

function Test-FlowLogHasWatcher {
    param($Source, $Watchers)

    $location = Get-FlowLogNormalizedLocation $Source.Location
    if ($location.Length -eq 0) { return $true }
    foreach ($watcher in $Watchers) {
        $watcherSub = Get-FlowLogDiagText -Object $watcher -Pascal 'SubscriptionId' -Camel 'subscriptionId'
        $watcherLocation = Get-FlowLogNormalizedLocation (Get-FlowLogDiagText -Object $watcher -Pascal 'Location' -Camel 'location')
        $subOk = [string]::IsNullOrEmpty($Source.SubscriptionId) -or $watcherSub -eq $Source.SubscriptionId
        if ($subOk -and $watcherLocation -eq $location) { return $true }
    }
    return $false
}

function Get-FlowLogNotConfiguredMessage {
    param($Source)

    $label = Get-FlowLogKindLabel $Source.Kind
    $target = if ($Source.Kind -eq 'Subnet' -or $Source.Kind -eq 'NIC') {
        "No flow log targets this $label or its virtual network."
    }
    else {
        "No flow log targets this $label."
    }
    $where = if ([string]::IsNullOrWhiteSpace($Source.Location)) { 'a storage account in the same region' } else { "a storage account in $($Source.Location)" }
    if ($Source.Kind -eq 'NSG') {
        return "$target NSG flow logs retire on 30 Sep 2027 and new ones cannot be created. Enable a virtual network flow log to $where."
    }
    return "$target Enable a virtual network flow log to $where."
}

function New-FlowLogBareFinding {
    param($Source, [string]$Reason, [string]$Message)

    [pscustomobject]@{
        subscriptionId = [string]$Source.SubscriptionId
        resourceGroup  = [string]$Source.ResourceGroup
        location       = [string]$Source.Location
        kind           = [string]$Source.Kind
        name           = [string]$Source.Name
        resourceId     = [string]$Source.Id
        reason         = $Reason
        flowLog        = ''
        storageAccount = ''
        message        = $Message
    }
}

function New-FlowLogConfigFinding {
    param($Source, $Config, $Verdict)

    $location = if ([string]::IsNullOrWhiteSpace($Source.Location)) { Get-FlowLogDiagText -Object $Config -Pascal 'Location' -Camel 'location' } else { $Source.Location }
    $name = if ([string]::IsNullOrWhiteSpace($Source.Name)) { Get-FlowLogDiagText -Object $Config -Pascal 'TargetName' -Camel 'targetName' } else { $Source.Name }
    [pscustomobject]@{
        subscriptionId = [string]$Source.SubscriptionId
        resourceGroup  = [string]$Source.ResourceGroup
        location       = [string]$location
        kind           = [string]$Source.Kind
        name           = [string]$name
        resourceId     = [string]$Source.Id
        reason         = [string]$Verdict.Reason
        flowLog        = (Get-FlowLogDiagText -Object $Config -Pascal 'Name' -Camel 'name')
        storageAccount = (Get-FlowLogDiagText -Object $Config -Pascal 'StorageAccount' -Camel 'storageAccount')
        message        = [string]$Verdict.Message
    }
}

function Get-FlowLogCheckedHealth {
    param($Config, $HealthByKey)

    $key = "$(Get-FlowLogDiagText -Object $Config -Pascal 'SubscriptionId' -Camel 'subscriptionId')|$(Get-FlowLogDiagText -Object $Config -Pascal 'NetworkWatcher' -Camel 'networkWatcher')|$(Get-FlowLogDiagText -Object $Config -Pascal 'Name' -Camel 'name')"
    $key = $key.ToLowerInvariant()
    if ($HealthByKey.ContainsKey($key)) { return $HealthByKey[$key] }
    return $null
}

function Get-FlowLogHealthFields {
    param($Config, $Checked)

    if ($null -ne $Checked) {
        $age = Get-FlowLogProperty -Object $Checked -Name 'ageHours'
        return @{
            Status         = (Get-FlowLogDiagText -Object $Checked -Pascal 'status' -Camel 'Status')
            LatestBlobUtc  = (Get-FlowLogDiagText -Object $Checked -Pascal 'latestBlobUtc' -Camel 'LatestBlobUtc')
            AgeHours       = $age
            Message        = (Get-FlowLogDiagText -Object $Checked -Pascal 'message' -Camel 'Message')
        }
    }
    $age = Get-FlowLogProperty -Object $Config -Name 'AgeHours'
    if ($null -eq $age) { $age = Get-FlowLogProperty -Object $Config -Name 'ageHours' }
    return @{
        Status        = (Get-FlowLogDiagText -Object $Config -Pascal 'Status' -Camel 'status')
        LatestBlobUtc = (Get-FlowLogDiagText -Object $Config -Pascal 'LatestBlobUtc' -Camel 'latestBlobUtc')
        AgeHours      = $age
        Message       = (Get-FlowLogDiagText -Object $Config -Pascal 'Message' -Camel 'message')
    }
}

function Get-FlowLogSourceEvaluation {
    param($Source, $Context)

    foreach ($id in @(Get-FlowLogSourceChain $Source)) {
        $lookup = $id.ToLowerInvariant()
        if (-not $Context.ByTarget.ContainsKey($lookup)) { continue }
        $enabled = @($Context.ByTarget[$lookup] | Where-Object {
            $flag = Get-FlowLogProperty -Object $_ -Name 'Enabled'
            if ($null -eq $flag) { $flag = Get-FlowLogProperty -Object $_ -Name 'enabled' }
            Test-FlowLogDiagBool $flag
        })
        if ($enabled.Count -eq 0) { continue }
        $delivering = $false
        foreach ($config in $enabled) {
            $health = Get-FlowLogHealthFields -Config $config -Checked (Get-FlowLogCheckedHealth $config $Context.Health)
            if (Test-FlowLogDelivering $config $health.Status $health.LatestBlobUtc) { $delivering = $true; break }
        }
        if ($delivering) { return $null }
        $ranked = foreach ($config in $enabled) {
            $health = Get-FlowLogHealthFields -Config $config -Checked (Get-FlowLogCheckedHealth $config $Context.Health)
            $verdict = Get-FlowLogConfigVerdict -Config $config -Source $Source -Context $Context -Status $health.Status -LatestBlobUtc $health.LatestBlobUtc -AgeHours $health.AgeHours -HealthMessage $health.Message
            [pscustomobject]@{ Config = $config; Verdict = $verdict; Name = (Get-FlowLogDiagText -Object $config -Pascal 'Name' -Camel 'name') }
        }
        $worst = @($ranked | Sort-Object { Get-FlowLogDiagnosisRank $_.Verdict.Reason }, Name)[0]
        return (New-FlowLogConfigFinding -Source $Source -Config $worst.Config -Verdict $worst.Verdict)
    }

    foreach ($id in @(Get-FlowLogSourceChain $Source)) {
        $lookup = $id.ToLowerInvariant()
        if (-not $Context.ByTarget.ContainsKey($lookup)) { continue }
        $disabled = @($Context.ByTarget[$lookup] | Where-Object {
            $flag = Get-FlowLogProperty -Object $_ -Name 'Enabled'
            if ($null -eq $flag) { $flag = Get-FlowLogProperty -Object $_ -Name 'enabled' }
            -not (Test-FlowLogDiagBool $flag)
        } | Sort-Object { Get-FlowLogDiagText -Object $_ -Pascal 'Name' -Camel 'name' })
        if ($disabled.Count -eq 0) { continue }
        $config = $disabled[0]
        $health = Get-FlowLogHealthFields -Config $config -Checked (Get-FlowLogCheckedHealth $config $Context.Health)
        $verdict = Get-FlowLogConfigVerdict -Config $config -Source $Source -Context $Context -Status $health.Status -LatestBlobUtc $health.LatestBlobUtc -AgeHours $health.AgeHours -HealthMessage $health.Message
        return (New-FlowLogConfigFinding -Source $Source -Config $config -Verdict $verdict)
    }

    if (-not (Test-FlowLogHasWatcher -Source $Source -Watchers $Context.Watchers)) {
        $where = if ([string]::IsNullOrWhiteSpace($Source.Location)) { 'this subscription' } else { $Source.Location }
        return (New-FlowLogBareFinding $Source 'NetworkWatcherMissing' "No Network Watcher in $where. Flow logs are created on the regional Network Watcher, so nothing is sent until one exists.")
    }
    return (New-FlowLogBareFinding $Source 'NotConfigured' (Get-FlowLogNotConfiguredMessage $Source))
}

function Test-FlowLogNsgAttachmentsCovered {
    param([string]$NsgId, $Vnets, $Nics, $Covered)

    $attachments = [System.Collections.Generic.List[string]]::new()
    foreach ($vnet in $Vnets) {
        foreach ($subnet in @(Get-FlowLogDiagList $vnet 'Subnets')) {
            $subnetId = Get-FlowLogDiagText -Object $subnet -Pascal 'Id' -Camel 'id'
            $nsg = Get-FlowLogDiagText -Object $subnet -Pascal 'NsgId' -Camel 'nsgId'
            if (-not [string]::IsNullOrWhiteSpace($subnetId) -and $nsg -eq $NsgId) { $attachments.Add($subnetId.ToLowerInvariant()) }
        }
    }
    foreach ($nic in $Nics) {
        $nicId = Get-FlowLogDiagText -Object $nic -Pascal 'Id' -Camel 'id'
        $nsg = Get-FlowLogDiagText -Object $nic -Pascal 'NsgId' -Camel 'nsgId'
        if (-not [string]::IsNullOrWhiteSpace($nicId) -and $nsg -eq $NsgId) { $attachments.Add($nicId.ToLowerInvariant()) }
    }
    if ($attachments.Count -eq 0) { return $false }
    foreach ($id in $attachments) {
        if (-not $Covered.ContainsKey($id)) { return $false }
    }
    return $true
}

function Get-FlowLogSourceFindings {
    # Pure sweep over ARM-shaped subscriptions. CheckedLogs, when present, supply blob health keyed by subscription, watcher and name.
    param(
        [object[]]$ArmData,
        [object[]]$CheckedLogs,
        [object[]]$CheckedAccounts,
        [int]$StaleAfterHours = 6,
        [string[]]$StorageAccountName
    )

    $health = @{}
    foreach ($log in @($CheckedLogs)) {
        if ($null -eq $log) { continue }
        $key = "$(Get-FlowLogDiagText -Object $log -Pascal 'subscriptionId' -Camel 'SubscriptionId')|$(Get-FlowLogDiagText -Object $log -Pascal 'networkWatcher' -Camel 'NetworkWatcher')|$(Get-FlowLogDiagText -Object $log -Pascal 'name' -Camel 'Name')"
        $health[$key.ToLowerInvariant()] = $log
    }
    $accountStatus = @{}
    foreach ($account in @($CheckedAccounts)) {
        if ($null -eq $account) { continue }
        $key = "$(Get-FlowLogDiagText -Object $account -Pascal 'subscriptionId' -Camel 'SubscriptionId')|$(Get-FlowLogDiagText -Object $account -Pascal 'name' -Camel 'Name')"
        $accountStatus[$key.ToLowerInvariant()] = Get-FlowLogDiagText -Object $account -Pascal 'status' -Camel 'Status'
    }

    $byTarget = @{}
    $accounts = @{}
    $known = @{}
    $sources = [System.Collections.Generic.List[object]]::new()
    $watchers = [System.Collections.Generic.List[object]]::new()
    $vnets = [System.Collections.Generic.List[object]]::new()
    $nics = [System.Collections.Generic.List[object]]::new()
    $logs = [System.Collections.Generic.List[object]]::new()
    $nsgs = @{}

    foreach ($arm in @($ArmData)) {
        if ($null -eq $arm) { continue }
        foreach ($watcher in @(Get-FlowLogDiagList $arm 'NetworkWatchers')) { $watchers.Add($watcher) }
        foreach ($account in @(Get-FlowLogDiagList $arm 'StorageAccounts')) {
            $name = Get-FlowLogDiagText -Object $account -Pascal 'Name' -Camel 'name'
            if ([string]::IsNullOrWhiteSpace($name)) { continue }
            $statusKey = "$(Get-FlowLogDiagText -Object $account -Pascal 'SubscriptionId' -Camel 'subscriptionId')|$name".ToLowerInvariant()
            if ($accountStatus.ContainsKey($statusKey) -and [string]::IsNullOrEmpty((Get-FlowLogDiagText -Object $account -Pascal 'Status' -Camel 'status'))) {
                $account | Add-Member -NotePropertyName Status -NotePropertyValue $accountStatus[$statusKey] -Force
            }
            if (-not $accounts.ContainsKey($name.ToLowerInvariant())) { $accounts[$name.ToLowerInvariant()] = $account }
        }
        foreach ($vnet in @(Get-FlowLogDiagList $arm 'VirtualNetworks')) {
            $vnets.Add($vnet)
            $vnetId = Get-FlowLogDiagText -Object $vnet -Pascal 'Id' -Camel 'id'
            if ([string]::IsNullOrWhiteSpace($vnetId)) { continue }
            $known[$vnetId.ToLowerInvariant()] = $true
            $sources.Add([pscustomobject]@{
                Kind = 'VNet'; Id = $vnetId; Name = (Get-FlowLogDiagText -Object $vnet -Pascal 'Name' -Camel 'name')
                ResourceGroup = (Get-FlowLogDiagText -Object $vnet -Pascal 'ResourceGroup' -Camel 'resourceGroup')
                SubscriptionId = (Get-FlowLogDiagText -Object $vnet -Pascal 'SubscriptionId' -Camel 'subscriptionId')
                Location = (Get-FlowLogDiagText -Object $vnet -Pascal 'Location' -Camel 'location')
                SubnetId = ''; VnetId = ''
            })
            foreach ($subnet in @(Get-FlowLogDiagList $vnet 'Subnets')) {
                $subnetId = Get-FlowLogDiagText -Object $subnet -Pascal 'Id' -Camel 'id'
                $nsgId = Get-FlowLogDiagText -Object $subnet -Pascal 'NsgId' -Camel 'nsgId'
                $location = Get-FlowLogDiagText -Object $vnet -Pascal 'Location' -Camel 'location'
                if (-not [string]::IsNullOrWhiteSpace($nsgId)) { Add-FlowLogDiagnosisNsg $nsgs $nsgId $location }
                if ([string]::IsNullOrWhiteSpace($subnetId)) { continue }
                $known[$subnetId.ToLowerInvariant()] = $true
                $sources.Add([pscustomobject]@{
                    Kind = 'Subnet'; Id = $subnetId; Name = (Get-FlowLogDiagText -Object $subnet -Pascal 'Name' -Camel 'name')
                    ResourceGroup = (Get-FlowLogDiagText -Object $vnet -Pascal 'ResourceGroup' -Camel 'resourceGroup')
                    SubscriptionId = (Get-FlowLogDiagText -Object $vnet -Pascal 'SubscriptionId' -Camel 'subscriptionId')
                    Location = $location; SubnetId = ''; VnetId = $vnetId
                })
            }
        }
        foreach ($nic in @(Get-FlowLogDiagList $arm 'NetworkInterfaces')) {
            $nics.Add($nic)
            $nicId = Get-FlowLogDiagText -Object $nic -Pascal 'Id' -Camel 'id'
            if ([string]::IsNullOrWhiteSpace($nicId)) { continue }
            $known[$nicId.ToLowerInvariant()] = $true
            $subnetId = Get-FlowLogDiagText -Object $nic -Pascal 'SubnetId' -Camel 'subnetId'
            $nsgId = Get-FlowLogDiagText -Object $nic -Pascal 'NsgId' -Camel 'nsgId'
            $location = Get-FlowLogDiagText -Object $nic -Pascal 'Location' -Camel 'location'
            if (-not [string]::IsNullOrWhiteSpace($nsgId)) { Add-FlowLogDiagnosisNsg $nsgs $nsgId $location }
            $sources.Add([pscustomobject]@{
                Kind = 'NIC'; Id = $nicId; Name = (Get-FlowLogDiagText -Object $nic -Pascal 'Name' -Camel 'name')
                ResourceGroup = (Get-FlowLogArmSegment $nicId 'resourceGroups')
                SubscriptionId = (Get-FlowLogArmSegment $nicId 'subscriptions')
                Location = $location; SubnetId = $subnetId; VnetId = (Get-FlowLogParentVnetId $subnetId)
            })
        }
        foreach ($log in @(Get-FlowLogDiagList $arm 'FlowLogs')) {
            $target = Get-FlowLogDiagText -Object $log -Pascal 'Target' -Camel 'target'
            if ([string]::IsNullOrWhiteSpace($target)) { continue }
            $logs.Add($log)
            if (-not $byTarget.ContainsKey($target.ToLowerInvariant())) { $byTarget[$target.ToLowerInvariant()] = [System.Collections.Generic.List[object]]::new() }
            $byTarget[$target.ToLowerInvariant()].Add($log)
            if ((Get-FlowLogDiagText -Object $log -Pascal 'TargetKind' -Camel 'targetKind') -eq 'NSG') {
                Add-FlowLogDiagnosisNsg $nsgs $target (Get-FlowLogDiagText -Object $log -Pascal 'Location' -Camel 'location')
            }
        }
    }

    foreach ($nsg in $nsgs.Values) {
        $known[$nsg.Id.ToLowerInvariant()] = $true
        $sources.Add($nsg)
    }

    $context = @{
        ByTarget        = $byTarget
        Accounts        = $accounts
        Known           = $known
        Watchers        = $watchers
        Health          = $health
        StaleAfterHours = $StaleAfterHours
    }
    $covered = @{}
    $findings = [System.Collections.Generic.List[object]]::new()
    foreach ($source in @($sources | Where-Object { $_.Kind -ne 'NSG' })) {
        $finding = Get-FlowLogSourceEvaluation -Source $source -Context $context
        if ($null -eq $finding) { $covered[$source.Id.ToLowerInvariant()] = $true } else { $findings.Add($finding) }
    }
    foreach ($source in @($sources | Where-Object { $_.Kind -eq 'NSG' })) {
        $finding = Get-FlowLogSourceEvaluation -Source $source -Context $context
        if ($null -eq $finding) { continue }
        if ($finding.reason -in @('NotConfigured', 'NetworkWatcherMissing', 'Disabled') -and (Test-FlowLogNsgAttachmentsCovered $source.Id $vnets $nics $covered)) { continue }
        $findings.Add($finding)
    }

    $orphans = 0
    foreach ($log in $logs) {
        $target = Get-FlowLogDiagText -Object $log -Pascal 'Target' -Camel 'target'
        if ($known.ContainsKey($target.ToLowerInvariant())) { continue }
        $orphans++
        $healthFields = Get-FlowLogHealthFields -Config $log -Checked (Get-FlowLogCheckedHealth $log $health)
        if (Test-FlowLogDelivering $log $healthFields.Status $healthFields.LatestBlobUtc) { continue }
        $kind = Get-FlowLogDiagText -Object $log -Pascal 'TargetKind' -Camel 'targetKind'
        if ([string]::IsNullOrEmpty($kind)) { $kind = 'Other' }
        $subscription = Get-FlowLogArmSegment $target 'subscriptions'
        if ([string]::IsNullOrEmpty($subscription)) { $subscription = Get-FlowLogDiagText -Object $log -Pascal 'SubscriptionId' -Camel 'subscriptionId' }
        $source = [pscustomobject]@{
            Kind = $kind; Id = $target
            Name = (Get-FlowLogDiagText -Object $log -Pascal 'TargetName' -Camel 'targetName')
            ResourceGroup = (Get-FlowLogDiagText -Object $log -Pascal 'TargetResourceGroup' -Camel 'targetResourceGroup')
            SubscriptionId = $subscription
            Location = (Get-FlowLogDiagText -Object $log -Pascal 'Location' -Camel 'location')
            SubnetId = ''; VnetId = (Get-FlowLogParentVnetId $target)
        }
        $verdict = Get-FlowLogConfigVerdict -Config $log -Source $source -Context $context -Status $healthFields.Status -LatestBlobUtc $healthFields.LatestBlobUtc -AgeHours $healthFields.AgeHours -HealthMessage $healthFields.Message
        $findings.Add((New-FlowLogConfigFinding -Source $source -Config $log -Verdict $verdict))
    }

    $filter = @($StorageAccountName | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $visible = [System.Collections.Generic.List[object]]::new()
    foreach ($finding in $findings) {
        if ($filter.Count -eq 0 -or [string]::IsNullOrEmpty($finding.storageAccount) -or ($filter -contains $finding.storageAccount)) {
            $visible.Add($finding)
        }
    }
    $sorted = @($visible | Sort-Object subscriptionId, { Get-FlowLogKindRank $_.kind }, resourceGroup, name)

    [pscustomobject]@{
        SourceCount  = $sources.Count + $orphans
        FindingCount = $sorted.Count
        Findings     = $sorted
    }
}

function Add-FlowLogDiagnosisNsg {
    param($Nsgs, [string]$Id, [string]$Location)

    if ([string]::IsNullOrWhiteSpace($Id)) { return }
    $key = $Id.ToLowerInvariant()
    if (-not $Nsgs.ContainsKey($key)) {
        $Nsgs[$key] = [pscustomobject]@{
            Kind = 'NSG'; Id = $Id; Name = (Get-FlowLogTargetName $Id)
            ResourceGroup = (Get-FlowLogTargetResourceGroup $Id)
            SubscriptionId = (Get-FlowLogArmSegment $Id 'subscriptions')
            Location = $Location; SubnetId = ''; VnetId = ''
        }
        return
    }
    if ([string]::IsNullOrWhiteSpace($Nsgs[$key].Location) -and -not [string]::IsNullOrWhiteSpace($Location)) {
        $Nsgs[$key].Location = $Location
    }
}
