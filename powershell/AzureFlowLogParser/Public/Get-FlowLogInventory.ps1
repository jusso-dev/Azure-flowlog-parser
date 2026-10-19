function Get-FlowLogInventory {
    <#
    .SYNOPSIS
    Lists storage accounts that receive VNet flow logs, with flow-log config and health status.

    .DESCRIPTION
    Reads subscriptions, storage accounts, network watchers, flow-log configs, VNets and NICs through Az.
    For each storage account it checks the insights-logs-flowlogflowevent container and, for each flow-log
    config, the blobs written in the last two UTC days. Accounts without a config are checked for blobs
    (Unmanaged). Requires Az.Accounts, Az.Storage and Az.Network and an authenticated Az context
    (Connect-AzAccount).

    .PARAMETER SubscriptionId
    Subscription ids to inspect. Defaults to every enabled subscription visible to the identity.

    .PARAMETER StorageAccountName
    Limit the output to these storage account names.

    .PARAMETER StaleAfterHours
    Hours after the newest blob before a config is reported as Stale. Default 3.

    .PARAMETER IncludeAll
    Also include storage accounts with no flow-log configs and no flow-log blobs (NoFlowLogs).
    Accounts whose container cannot be read (AccessDenied) are included by default.

    .OUTPUTS
    PSCustomObject with schema, generatedUtc, source, subscriptions, accounts, flowLogs and notes.

    .EXAMPLE
    Get-FlowLogInventory -SubscriptionId 00000000-0000-0000-0000-000000000000

    .EXAMPLE
    Get-FlowLogInventory -StorageAccountName stflowlogs01 -StaleAfterHours 6 | ConvertTo-Json -Depth 12
    #>
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [string[]]$SubscriptionId,
        [string[]]$StorageAccountName,
        [ValidateRange(1, 8760)][int]$StaleAfterHours = 3,
        [switch]$IncludeAll
    )

    $nowUtc = [datetime]::UtcNow
    $monitoring = Get-FlowLogMonitoring -SubscriptionId $SubscriptionId -StorageAccountName $StorageAccountName -StaleAfterHours $StaleAfterHours -NowUtc $nowUtc
    # AccessDenied accounts stay visible: their status says the check could not run.
    $accounts = @($monitoring.Accounts | Where-Object { $IncludeAll -or $_.hasFlowLogs -or $_.status -eq 'AccessDenied' })

    [pscustomobject]@{
        schema       = 1
        generatedUtc = Format-FlowLogTime -Value $nowUtc
        source       = 'azure'
        subscriptions = $monitoring.SubscriptionCount
        accounts     = $accounts
        flowLogs     = @($monitoring.FlowLogs)
        notes        = @($monitoring.Notes | Select-Object -Unique)
    }
}
