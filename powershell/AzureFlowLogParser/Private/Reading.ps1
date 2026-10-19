# Blob selection and reading for the accounts in the read plan.

function Get-FlowLogWindowBlob {
    # Configured targets: day prefixes covering the window, narrowed to blob hours within 1h of the window.
    param($Plan, $Window)

    $blobs = [System.Collections.Generic.List[object]]::new()
    foreach ($target in $Plan.Targets) {
        for ($day = $Window.FromUtc.Date; $day -le $Window.ToUtc; $day = $day.AddDays(1)) {
            $prefix = Get-FlowLogDayPrefix -Target $target -Day $day
            foreach ($blob in @(Get-FlowLogBlobList -Context $Plan.Context -Prefix $prefix)) {
                $hour = Get-FlowLogBlobHour -BlobName $blob.Name
                if ($null -ne $hour -and ($hour -lt $Window.FromUtc.AddHours(-1) -or $hour -gt $Window.ToUtc.AddHours(1))) { continue }
                $blobs.Add($blob)
            }
        }
    }
    return $blobs.ToArray()
}

function Read-FlowLogAccount {
    # Reads every blob for one account in the plan. A failed account is added to $Failed; it stays in the read set.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][hashtable]$State,
        [Parameter(Mandatory)]$Window,
        [Parameter(Mandatory)]$Plan,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[string]]$Failed
    )

    try {
        if ($Plan.Mode -eq 'unmanaged') {
            $earliest = $Window.FromUtc.AddHours(-1)
            $blobs = @(Get-FlowLogBlobList -Context $Plan.Context -Prefix 'resourceId=/' | Where-Object { $_.LastModifiedUtc -ge $earliest })
        }
        else {
            $blobs = Get-FlowLogWindowBlob -Plan $Plan -Window $Window
        }
        foreach ($blob in @($blobs | Sort-Object -Property Name)) {
            Add-FlowLogBlobJson -State $State -Json (Read-FlowLogBlobText -Context $Plan.Context -Name $blob.Name)
        }
    }
    catch {
        $Failed.Add($Plan.Name)
    }
}
