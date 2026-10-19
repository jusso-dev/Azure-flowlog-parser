# Blob path, tuple and JSON helpers.

function Get-FlowLogProperty {
    # Strict-mode safe property read for parsed JSON and ARM objects.
    param($Object, [string]$Name)

    if ($null -eq $Object) { return $null }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

function ConvertTo-FlowLogRecordTime {
    # Record-level 'time' (ISO-8601, UTC) used when a tuple start time cannot be parsed. Returns $null if not a date.
    param([string]$Text)

    $parsed = [datetime]::MinValue
    $styles = [System.Globalization.DateTimeStyles]::AssumeUniversal -bor [System.Globalization.DateTimeStyles]::AdjustToUniversal
    if ([datetime]::TryParse($Text, [cultureinfo]::InvariantCulture, $styles, [ref]$parsed)) { return $parsed }
    return $null
}

function Get-FlowLogDayPrefix {
    # Blob prefix for one flow-log target on one UTC day.
    param([Parameter(Mandatory)][string]$Target, [Parameter(Mandatory)][datetime]$Day)

    $culture = [cultureinfo]::InvariantCulture
    return 'resourceId={0}/y={1}/m={2}/d={3}/' -f $Target.ToUpperInvariant(),
        $Day.ToString('yyyy', $culture), $Day.ToString('MM', $culture), $Day.ToString('dd', $culture)
}

function Get-FlowLogBlobTarget {
    # ARM id of the NSG or VNet encoded in a blob name, or $null.
    param([string]$BlobName)

    if ($BlobName -match '^resourceId=(/SUBSCRIPTIONS/[^/]+/RESOURCEGROUPS/[^/]+/PROVIDERS/MICROSOFT\.NETWORK/[^/]+/[^/]+)/y=') {
        return $Matches[1]
    }
    return $null
}

function Get-FlowLogBlobHour {
    # UTC hour bucket from the y=/m=/d=/h= segments of a blob name, or $null.
    param([string]$BlobName)

    if ($BlobName -match '/y=(\d{4})/m=(\d{2})/d=(\d{2})/h=(\d{2})/') {
        return [datetime]::new([int]$Matches[1], [int]$Matches[2], [int]$Matches[3], [int]$Matches[4], 0, 0, [System.DateTimeKind]::Utc)
    }
    return $null
}

function Get-FlowLogCount {
    # Byte or packet counter from a tuple field. Empty, missing or non-numeric counts as 0.
    param([string[]]$Parts, [int]$Index)

    if ($Index -ge $Parts.Count) { return [long]0 }
    $value = [long]0
    if ([long]::TryParse($Parts[$Index], [ref]$value)) { return $value }
    return [long]0
}
