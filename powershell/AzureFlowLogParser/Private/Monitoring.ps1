# Discovery and health: storage accounts, flow-log configs, container checks and the read plan.

function Get-FlowLogErrorStatus {
    param([Parameter(Mandatory)][System.Management.Automation.ErrorRecord]$ErrorRecord)

    if ($ErrorRecord.Exception.Message -match '\b403\b|AuthorizationPermissionMismatch') { return 'AccessDenied' }
    return 'Error'
}

function Get-FlowLogWorstStatus {
    param([string[]]$Statuses)

    foreach ($status in 'Error', 'AccessDenied', 'NoData', 'Stale', 'Disabled', 'Healthy') {
        if ($Statuses -contains $status) { return $status }
    }
    return 'Healthy'
}

function Get-FlowLogConfigHealth {
    # Two-day lookback (yesterday and today, UTC) for the config's target prefix.
    [CmdletBinding()]
    param($Config, $Context, [bool]$ContainerPresent, [datetime]$NowUtc, [int]$StaleAfterHours)

    $health = [pscustomobject]@{ Status = 'Healthy'; LatestBlobUtc = $null; AgeHours = $null; Message = '' }
    if (-not $Config.Enabled) {
        $health.Status = 'Disabled'
        $health.Message = 'Flow log is disabled.'
        return $health
    }
    if (-not $ContainerPresent) {
        $health.Status = 'NoData'
        $health.Message = "Container $($script:FlowLogContainerName) not found."
        return $health
    }

    try {
        $latest = $null
        foreach ($day in $NowUtc.Date.AddDays(-1), $NowUtc.Date) {
            $prefix = Get-FlowLogDayPrefix -Target $Config.Target -Day $day
            foreach ($blob in @(Get-FlowLogBlobList -Context $Context -Prefix $prefix)) {
                if ($null -eq $latest -or $blob.LastModifiedUtc -gt $latest) { $latest = $blob.LastModifiedUtc }
            }
        }
        if ($null -eq $latest) {
            $health.Status = 'NoData'
            $health.Message = 'No flow log blobs in the last 2 days.'
            return $health
        }

        $age = $NowUtc - $latest
        $health.LatestBlobUtc = $latest
        $health.AgeHours = [math]::Round($age.TotalHours, 2)
        $health.Status = if ($age.TotalHours -gt $StaleAfterHours) { 'Stale' } else { 'Healthy' }
        return $health
    }
    catch {
        $health.Status = Get-FlowLogErrorStatus -ErrorRecord $_
        $health.Message = $_.Exception.Message
        return $health
    }
}

function Get-FlowLogMonitoring {
    [CmdletBinding()]
    param(
        [string[]]$SubscriptionId,
        [string[]]$StorageAccountName,
        [int]$StaleAfterHours,
        [datetime]$NowUtc
    )

    $notes = [System.Collections.Generic.List[string]]::new()
    $subscriptions = @(Get-FlowLogAzSubscription -SubscriptionId $SubscriptionId)
    if ($subscriptions.Count -eq 0) {
        throw 'No enabled subscriptions are visible to the current identity. Run Connect-AzAccount or pass -SubscriptionId.'
    }

    $armData = [System.Collections.Generic.List[object]]::new()
    foreach ($subscription in $subscriptions) {
        try {
            $armData.Add((Get-FlowLogAzSubscriptionData -SubscriptionId $subscription.Id -TenantId $subscription.TenantId))
        }
        catch {
            $notes.Add("Subscription $($subscription.Id) could not be read: $($_.Exception.Message)")
        }
    }
    $topology = New-FlowLogTopology -ArmData $armData.ToArray() -Notes $notes

    $wanted = @($StorageAccountName | Where-Object { $_ })
    $allAccounts = @($armData | ForEach-Object { $_.StorageAccounts })
    $candidates = @($allAccounts | Where-Object { $wanted.Count -eq 0 -or ($wanted -contains $_.Name) })
    $allConfigs = @($armData | ForEach-Object { $_.FlowLogs })

    if ($wanted.Count -eq 0) {
        $accountNames = @($allAccounts | ForEach-Object { $_.Name })
        foreach ($config in $allConfigs) {
            if ($accountNames -notcontains $config.StorageAccount) {
                $notes.Add("Flow log $($config.Name) writes to storage account $($config.StorageAccount), which is outside the selected subscriptions. It is not checked.")
            }
        }
    }

    $accounts = [System.Collections.Generic.List[object]]::new()
    $flowLogs = [System.Collections.Generic.List[object]]::new()
    $readPlan = [System.Collections.Generic.List[object]]::new()

    foreach ($account in $candidates) {
        $configs = @($allConfigs | Where-Object { $_.StorageAccount -eq $account.Name })

        $containerPresent = $false
        $containerStatus = $null
        $containerMessage = ''
        $context = $null
        try {
            $context = New-FlowLogStorageContext -StorageAccountName $account.Name
            $containerPresent = Test-FlowLogContainer -Context $context
        }
        catch {
            $containerStatus = Get-FlowLogErrorStatus -ErrorRecord $_
            $containerMessage = $_.Exception.Message
        }

        $configResults = [System.Collections.Generic.List[object]]::new()
        $latest = $null
        foreach ($config in $configs) {
            if ($null -ne $containerStatus) {
                $health = [pscustomobject]@{ Status = $containerStatus; LatestBlobUtc = $null; AgeHours = $null; Message = $containerMessage }
            }
            else {
                $health = Get-FlowLogConfigHealth -Config $config -Context $context -ContainerPresent $containerPresent -NowUtc $NowUtc -StaleAfterHours $StaleAfterHours
            }
            if ($null -ne $health.LatestBlobUtc -and ($null -eq $latest -or $health.LatestBlobUtc -gt $latest)) {
                $latest = $health.LatestBlobUtc
            }
            $configResults.Add([pscustomobject]@{
                subscriptionId = $config.SubscriptionId
                name           = $config.Name
                storageAccount = $config.StorageAccount
                networkWatcher = $config.NetworkWatcher
                location       = $config.Location
                target         = $config.Target
                targetKind     = $config.TargetKind
                enabled        = $config.Enabled
                retentionDays  = $config.RetentionDays
                trafficAnalytics = $config.TrafficAnalytics
                status         = $health.Status
                latestBlobUtc  = Format-FlowLogOptionalTime -Value $health.LatestBlobUtc
                ageHours       = $health.AgeHours
                message        = $health.Message
            })
        }

        $status = 'NoFlowLogs'
        $observedTarget = $null
        $hasBlobs = $false
        if ($null -ne $containerStatus) {
            $status = $containerStatus
        }
        elseif ($configs.Count -gt 0) {
            $status = Get-FlowLogWorstStatus -Statuses @($configResults | ForEach-Object { $_.status })
        }
        elseif ($containerPresent) {
            try {
                # First blob only: enough to say the account is unmanaged and name its source. A full listing can be huge.
                $blobs = @(Get-FlowLogBlobList -Context $context -Prefix 'resourceId=/' -MaxCount 1)
                $hasBlobs = $blobs.Count -gt 0
                if ($hasBlobs) {
                    $status = 'Unmanaged'
                    $observedTarget = Get-FlowLogBlobTarget -BlobName $blobs[0].Name
                }
            }
            catch {
                $status = Get-FlowLogErrorStatus -ErrorRecord $_
                $containerMessage = $_.Exception.Message
            }
        }

        $hasFlowLogs = $configs.Count -gt 0 -or $hasBlobs
        $accounts.Add([pscustomobject]@{
            subscriptionId = $account.SubscriptionId
            name           = $account.Name
            resourceGroup  = $account.ResourceGroup
            location       = $account.Location
            kind           = $account.Kind
            skuName        = $account.SkuName
            flowLogConfigs = $configs.Count
            containerPresent = $containerPresent
            hasFlowLogs    = $hasFlowLogs
            status         = $status
            latestBlobUtc  = Format-FlowLogOptionalTime -Value $latest
            observedTarget = $observedTarget
        })
        foreach ($result in $configResults) { $flowLogs.Add($result) }

        if ($null -eq $containerStatus -and $containerPresent -and ($configs.Count -gt 0 -or $hasBlobs)) {
            $mode = if ($configs.Count -gt 0) { 'configured' } else { 'unmanaged' }
            $readPlan.Add([pscustomobject]@{
                Name    = $account.Name
                Context = $context
                Mode    = $mode
                Targets = @($configs | ForEach-Object { $_.Target })
            })
        }
    }

    [pscustomobject]@{
        SubscriptionCount = $armData.Count
        Topology          = $topology
        Accounts          = $accounts.ToArray()
        FlowLogs          = $flowLogs.ToArray()
        ReadPlan          = $readPlan.ToArray()
        Notes             = $notes
    }
}
