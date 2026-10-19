# IPv4 prefix table built from ARM data, plus scope resolution for flow addresses.
# Precedence: NIC /32 > subnet > VNet flow log > VNet address space. Longest prefix wins in lookups.

function ConvertTo-FlowLogIPv4Number {
    param([string]$Address)

    $match = [regex]::Match($Address, '^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$')
    if (-not $match.Success) { return $null }
    $number = [long]0
    for ($i = 1; $i -le 4; $i++) {
        $octet = [int]$match.Groups[$i].Value
        if ($octet -gt 255) { return $null }
        $number = ($number -shl 8) -bor $octet
    }
    return $number
}

function Get-FlowLogMask {
    param([int]$Length)

    if ($Length -eq 0) { return [long]0 }
    return ([long]0xFFFFFFFF -shl (32 - $Length)) -band [long]0xFFFFFFFF
}

function ConvertTo-FlowLogPrefix {
    param([string]$Cidr)

    $parts = $Cidr.Split('/')
    $length = 32
    if ($parts.Count -eq 2) {
        if (-not [int]::TryParse($parts[1], [ref]$length)) { return $null }
    }
    elseif ($parts.Count -ne 1) {
        return $null
    }
    $address = ConvertTo-FlowLogIPv4Number -Address $parts[0]
    if ($null -eq $address -or $length -lt 0 -or $length -gt 32) { return $null }
    $mask = Get-FlowLogMask -Length $length
    return [pscustomobject]@{ Network = ($address -band $mask); Length = $length }
}

function Add-FlowLogPrefix {
    # First insert wins for an identical prefix, so callers add the highest-precedence entries first.
    param($Topology, [string]$Cidr, [hashtable]$Scope)

    $prefix = ConvertTo-FlowLogPrefix -Cidr $Cidr
    if ($null -eq $prefix) { return }
    $bucket = $Topology.Buckets[$prefix.Length]
    if (-not $bucket.ContainsKey($prefix.Network)) { $bucket[$prefix.Network] = $Scope }
}

function Find-FlowLogPrefixScope {
    param($Topology, [long]$Number)

    for ($length = 32; $length -ge 0; $length--) {
        $bucket = $Topology.Buckets[$length]
        if ($bucket.Count -eq 0) { continue }
        $network = $Number -band (Get-FlowLogMask -Length $length)
        if ($bucket.ContainsKey($network)) { return $bucket[$network] }
    }
    return $null
}

function Test-FlowLogPrivateNumber {
    param([long]$Number)

    foreach ($cidr in '10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '100.64.0.0/10', '169.254.0.0/16') {
        $prefix = ConvertTo-FlowLogPrefix -Cidr $cidr
        if (($Number -band (Get-FlowLogMask -Length $prefix.Length)) -eq $prefix.Network) { return $true }
    }
    return $false
}

function Get-FlowLogTargetOwners {
    # Owning storage account per flow-log target (NSG or VNet id). When several accounts log one target, the
    # first in ordinal order owns it. The note is written once per target, NSG targets first, as in the .NET topology.
    param([System.Collections.Generic.List[string]]$TargetOrder, [hashtable]$ConfigsByTarget, [System.Collections.Generic.List[string]]$Notes)

    $owners = @{}
    foreach ($kind in 'NSG', 'VNet') {
        foreach ($target in $TargetOrder) {
            $configs = @($ConfigsByTarget[$target] | Where-Object { $_.TargetKind -eq $kind })
            if ($configs.Count -eq 0) { continue }
            $accounts = [System.Collections.Generic.List[string]]::new([string[]]@($configs.StorageAccount | Select-Object -Unique))
            $accounts.Sort([System.StringComparer]::OrdinalIgnoreCase)
            $owners[$target] = $accounts[0]
            if ($accounts.Count -gt 1) {
                $Notes.Add("$kind $target has flow logs to $($accounts.Count) storage accounts; attributed to $($accounts[0]).")
            }
        }
    }
    return $owners
}

function Get-FlowLogTargetScope {
    # Storage scope of the owning account of a target (NSG or VNet id), or $null when no flow log covers it.
    param($Topology, [hashtable]$Owners, [hashtable]$ConfigsByTarget, [string]$TargetId)

    if ([string]::IsNullOrEmpty($TargetId) -or -not $Owners.ContainsKey($TargetId)) { return $null }
    $storageName = $Owners[$TargetId]
    if (-not $Topology.Scopes.ContainsKey($storageName)) {
        $config = @($ConfigsByTarget[$TargetId] | Where-Object { $_.StorageAccount -eq $storageName })[0]
        $Topology.Scopes[$storageName] = @{
            id             = $storageName
            kind           = 'storage'
            label          = $storageName
            subscriptionId = $config.StorageSubscriptionId
            resourceGroup  = $config.StorageResourceGroup
            storageAccount = $storageName
        }
    }
    return $Topology.Scopes[$storageName]
}

function Get-FlowLogVnetScope {
    param($Topology, $Vnet)

    $id = "vnet:$($Vnet.ResourceGroup)/$($Vnet.Name)"
    if (-not $Topology.Scopes.ContainsKey($id)) {
        $Topology.Scopes[$id] = @{
            id             = $id
            kind           = 'vnet'
            label          = "$($Vnet.Name) (no flow logs)"
            subscriptionId = $Vnet.SubscriptionId
            resourceGroup  = $Vnet.ResourceGroup
            storageAccount = ''
        }
    }
    return $Topology.Scopes[$id]
}

function New-FlowLogTopologyObject {
    # Empty prefix table: one bucket per prefix length 0..32, plus scope lookup by id.
    [pscustomobject]@{
        Buckets = @(0..32 | ForEach-Object { @{} })
        Scopes  = @{}
    }
}

function New-FlowLogOfflineTopology {
    # Reads { "networks": [ { "name", "kind": "storage"|"vnet", "prefixes": [...] } ] } and builds the same LPM.
    # Non-IPv4 prefixes are skipped and counted. Storage names are returned so the dedupe read set can be seeded.
    # The optional "subscriptions" array ({ subscriptionId, displayName, tenantId }) is returned in file order;
    # malformed entries are skipped with a note. Unknown keys are ignored.
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Topology file not found: $Path" }
    $document = [System.IO.File]::ReadAllText($Path) | ConvertFrom-Json -DateKind String
    # Read straight from the document: a function return would turn an empty array into $null.
    $hasNetworks = $null -ne $document -and $null -ne $document.PSObject.Properties['networks'] -and $null -ne $document.networks
    if (-not $hasNetworks) { throw "Topology file $Path must contain a 'networks' array." }
    $networks = $document.networks

    $topology = New-FlowLogTopologyObject
    $storageNames = [System.Collections.Generic.List[string]]::new()
    $networkCount = 0
    $prefixCount = 0
    $skippedCount = 0
    $subscriptionNotes = [System.Collections.Generic.List[string]]::new()
    $subscriptions = [System.Collections.Generic.List[object]]::new()
    foreach ($network in @($networks)) {
        $name = [string](Get-FlowLogProperty -Object $network -Name 'name')
        $kind = [string](Get-FlowLogProperty -Object $network -Name 'kind')
        if ([string]::IsNullOrWhiteSpace($name)) { throw "Topology network without a name in $Path." }
        $networkCount++
        if ($kind -eq 'storage') {
            $scope = @{ id = $name; kind = 'storage'; label = $name; subscriptionId = ''; resourceGroup = ''; storageAccount = $name }
            $storageNames.Add($name)
        }
        elseif ($kind -eq 'vnet') {
            $scope = @{ id = "vnet:$name"; kind = 'vnet'; label = $name; subscriptionId = ''; resourceGroup = ''; storageAccount = '' }
        }
        else {
            throw "Topology network '$name' has kind '$kind'. Use 'storage' or 'vnet'."
        }
        $topology.Scopes[$scope.id] = $scope
        foreach ($cidr in @(Get-FlowLogProperty -Object $network -Name 'prefixes')) {
            $text = ([string]$cidr).Trim()
            if ($null -eq (ConvertTo-FlowLogPrefix -Cidr $text)) {
                $skippedCount++
                continue
            }
            Add-FlowLogPrefix -Topology $topology -Cidr $text -Scope $scope
            $prefixCount++
        }
    }

    $subscriptionEntries = @()
    if ($null -ne $document -and $null -ne $document.PSObject.Properties['subscriptions'] -and $null -ne $document.subscriptions) {
        $subscriptionEntries = @($document.subscriptions)
    }
    for ($index = 0; $index -lt $subscriptionEntries.Count; $index++) {
        # Positions are 1-based, as the .NET reader writes them. Only string values are read; anything else is blank.
        $position = $index + 1
        $entry = $subscriptionEntries[$index]
        if ($entry -isnot [pscustomobject]) {
            $subscriptionNotes.Add("Topology subscription entry ignored: entry $position is not an object.")
            continue
        }
        $subscriptionId = Get-FlowLogProperty -Object $entry -Name 'subscriptionId'
        if ($subscriptionId -isnot [string] -or [string]::IsNullOrWhiteSpace($subscriptionId)) {
            $subscriptionNotes.Add("Topology subscription entry ignored: entry $position has no subscriptionId.")
            continue
        }
        $displayName = Get-FlowLogProperty -Object $entry -Name 'displayName'
        $tenantId = Get-FlowLogProperty -Object $entry -Name 'tenantId'
        $subscriptions.Add([pscustomobject]@{
            subscriptionId = $subscriptionId
            displayName    = if ($displayName -is [string]) { $displayName } else { '' }
            tenantId       = if ($tenantId -is [string]) { $tenantId } else { '' }
        })
    }

    return [pscustomobject]@{
        Topology     = $topology
        NetworkCount    = $networkCount
        PrefixCount     = $prefixCount
        SkippedPrefixes = $skippedCount
        StorageNames    = $storageNames.ToArray()
        Subscriptions   = $subscriptions.ToArray()
        SubscriptionNotes = $subscriptionNotes.ToArray()
    }
}

function New-FlowLogTopology {
    [CmdletBinding()]
    param([object[]]$ArmData, [System.Collections.Generic.List[string]]$Notes)

    $topology = New-FlowLogTopologyObject

    $configsByTarget = @{}
    $targetOrder = [System.Collections.Generic.List[string]]::new()
    foreach ($arm in $ArmData) {
        foreach ($config in $arm.FlowLogs) {
            if (-not $configsByTarget.ContainsKey($config.Target)) {
                $configsByTarget[$config.Target] = [System.Collections.Generic.List[object]]::new()
                $targetOrder.Add($config.Target)
            }
            $configsByTarget[$config.Target].Add($config)
        }
    }
    $owners = Get-FlowLogTargetOwners -TargetOrder $targetOrder -ConfigsByTarget $configsByTarget -Notes $Notes

    # Rule 1: NIC private IPs whose NIC NSG has a flow log.
    foreach ($arm in $ArmData) {
        foreach ($nic in $arm.NetworkInterfaces) {
            $scope = Get-FlowLogTargetScope -Topology $topology -Owners $owners -ConfigsByTarget $configsByTarget -TargetId $nic.NsgId
            if ($null -eq $scope) { continue }
            foreach ($ip in $nic.PrivateIps) { Add-FlowLogPrefix -Topology $topology -Cidr "$ip/32" -Scope $scope }
        }
    }

    # Rule 2: subnet prefixes whose subnet NSG has a flow log.
    foreach ($arm in $ArmData) {
        foreach ($vnet in $arm.VirtualNetworks) {
            foreach ($subnet in $vnet.Subnets) {
                $scope = Get-FlowLogTargetScope -Topology $topology -Owners $owners -ConfigsByTarget $configsByTarget -TargetId $subnet.NsgId
                if ($null -eq $scope) { continue }
                foreach ($cidr in $subnet.AddressPrefixes) { Add-FlowLogPrefix -Topology $topology -Cidr $cidr -Scope $scope }
            }
        }
    }

    # Rules 3 and 4: VNet address space, storage if the VNet has a flow log, else a vnet scope.
    foreach ($arm in $ArmData) {
        foreach ($vnet in $arm.VirtualNetworks) {
            $scope = Get-FlowLogTargetScope -Topology $topology -Owners $owners -ConfigsByTarget $configsByTarget -TargetId $vnet.Id
            if ($null -eq $scope) { $scope = Get-FlowLogVnetScope -Topology $topology -Vnet $vnet }
            foreach ($cidr in $vnet.AddressPrefixes) { Add-FlowLogPrefix -Topology $topology -Cidr $cidr -Scope $scope }
        }
    }

    return $topology
}

function Resolve-FlowLogScope {
    # Scope for one IPv4 address, cached per run. Unmatched public IPv4 is internet; everything else is unmapped.
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable]$State, [Parameter(Mandatory)][string]$Address)

    if ($State.ScopeCache.ContainsKey($Address)) { return $State.ScopeCache[$Address] }

    $number = ConvertTo-FlowLogIPv4Number -Address $Address
    $scope = $null
    if ($null -ne $number -and $null -ne $State.Topology) {
        $scope = Find-FlowLogPrefixScope -Topology $State.Topology -Number $number
    }
    if ($null -eq $scope) {
        $isPublic = $null -ne $number -and -not (Test-FlowLogPrivateNumber -Number $number)
        $scope = if ($isPublic) { $script:InternetScope } else { $script:UnmappedScope }
    }

    $State.Scopes[$scope.id] = $scope
    $State.ScopeCache[$Address] = $scope
    return $scope
}

$script:InternetScope = @{ id = 'internet'; kind = 'internet'; label = 'Internet'; subscriptionId = ''; resourceGroup = ''; storageAccount = '' }
$script:UnmappedScope = @{ id = 'unmapped'; kind = 'unmapped'; label = 'Unmapped'; subscriptionId = ''; resourceGroup = ''; storageAccount = '' }
