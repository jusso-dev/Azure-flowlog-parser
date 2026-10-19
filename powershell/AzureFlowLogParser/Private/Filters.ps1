# Time window, port filter and time formatting helpers.

function ConvertTo-FlowLogUtc {
    param([Parameter(Mandatory)][datetime]$Value)

    if ($Value.Kind -eq [System.DateTimeKind]::Local) { return $Value.ToUniversalTime() }
    return [datetime]::SpecifyKind($Value, [System.DateTimeKind]::Utc)
}

function Format-FlowLogTime {
    param([Parameter(Mandatory)][datetime]$Value)

    return $Value.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", [cultureinfo]::InvariantCulture)
}

function Format-FlowLogOptionalTime {
    param($Value)

    if ($null -eq $Value) { return $null }
    return Format-FlowLogTime -Value $Value
}

function Resolve-FlowLogWindow {
    # -Since accepts <n>h, <n>d, <n>m or an ISO-8601 UTC datetime.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Since,
        [Parameter(Mandatory)][datetime]$UntilUtc
    )

    if ($Since -match '^\s*(\d+)\s*([hHdDmM])\s*$') {
        $amount = [int]$Matches[1]
        $from = switch ($Matches[2].ToLowerInvariant()) {
            'h' { $UntilUtc.AddHours(-$amount) }
            'd' { $UntilUtc.AddDays(-$amount) }
            'm' { $UntilUtc.AddMinutes(-$amount) }
        }
    }
    else {
        $parsed = [datetime]::MinValue
        $styles = [System.Globalization.DateTimeStyles]::AssumeUniversal -bor [System.Globalization.DateTimeStyles]::AdjustToUniversal
        if (-not [datetime]::TryParse($Since, [cultureinfo]::InvariantCulture, $styles, [ref]$parsed)) {
            throw "Invalid -Since value '$Since'. Use <n>h, <n>d, <n>m or an ISO-8601 UTC datetime."
        }
        $from = $parsed
    }
    if ($from -ge $UntilUtc) { throw '-Since must be earlier than -Until.' }
    return [pscustomobject]@{ FromUtc = $from; ToUtc = $UntilUtc }
}

function ConvertTo-FlowLogPortFilter {
    # Parses '443,8000-8100' into inclusive ranges. Empty means no port filter.
    [CmdletBinding()]
    param([string]$Ports)

    $ranges = [System.Collections.Generic.List[object]]::new()
    foreach ($token in @($Ports -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
        if ($token -match '^(\d{1,5})(?:-(\d{1,5}))?$') {
            $low = [int]$Matches[1]
            $high = if ($Matches[2]) { [int]$Matches[2] } else { $low }
            if ($low -gt $high -or $high -gt 65535) { throw "Invalid port range '$token'." }
            $ranges.Add([pscustomobject]@{ Low = $low; High = $high })
        }
        else {
            throw "Invalid port filter '$token'. Use a list such as 443,8000-8100."
        }
    }
    return [pscustomobject]@{ Ranges = $ranges.ToArray() }
}

function Test-FlowLogPort {
    param([Parameter(Mandatory)]$Filter, [int]$Port)

    if ($Filter.Ranges.Count -eq 0) { return $true }
    foreach ($range in $Filter.Ranges) {
        if ($Port -ge $range.Low -and $Port -le $range.High) { return $true }
    }
    return $false
}
