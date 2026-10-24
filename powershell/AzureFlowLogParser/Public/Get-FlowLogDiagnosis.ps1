function Get-FlowLogDiagnosis {
    <#
    .SYNOPSIS
    Finds virtual networks, subnets, NICs, and attached NSGs that are not sending flow logs to storage, and why.

    .DESCRIPTION
    Sweeps every virtual network, subnet, and NIC in the selected subscriptions, plus NSGs attached to them or
    already targeted by an NSG flow log. A delivering flow log (enabled, Healthy, with a blob timestamp) on a
    parent covers its children. An enabled NIC flow log wins over its subnet, which wins over its virtual network.
    Uses the same Azure reads as Get-FlowLogInventory. Requires az on PATH and a signed-in session.

    .PARAMETER SubscriptionId
    Subscription ids to inspect. Defaults to every enabled subscription visible to the identity.

    .PARAMETER StorageAccountName
    Only show gaps for these storage accounts. Sources with no flow log are still listed.

    .PARAMETER StaleAfterHours
    Hours after the newest blob before a config is reported as Stale. Default 3.

    .OUTPUTS
    PSCustomObject with schema, generatedUtc, staleAfterHours, sourceCount, findingCount, findings, and notes.

    .EXAMPLE
    Get-FlowLogDiagnosis -SubscriptionId 00000000-0000-0000-0000-000000000000

    .EXAMPLE
    Get-FlowLogDiagnosis -StaleAfterHours 6 | ConvertTo-Json -Depth 6
    #>
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [string[]]$SubscriptionId,
        [string[]]$StorageAccountName,
        [ValidateRange(1, 8760)][int]$StaleAfterHours = 3
    )

    $nowUtc = [datetime]::UtcNow
    # Health-check every account. StorageAccountName only narrows the printed gaps.
    $monitoring = Get-FlowLogMonitoring -SubscriptionId $SubscriptionId -StorageAccountName @() -StaleAfterHours $StaleAfterHours -NowUtc $nowUtc
    $result = Get-FlowLogSourceFindings -ArmData @($monitoring.ArmData) -CheckedLogs @($monitoring.FlowLogs) -CheckedAccounts @($monitoring.Accounts) -StaleAfterHours $StaleAfterHours -StorageAccountName $StorageAccountName

    [pscustomobject]@{
        schema          = 1
        generatedUtc    = Format-FlowLogTime -Value $nowUtc
        staleAfterHours = $StaleAfterHours
        sourceCount     = $result.SourceCount
        findingCount    = $result.FindingCount
        findings        = @($result.Findings)
        notes           = @($monitoring.Notes | Select-Object -Unique)
    }
}
