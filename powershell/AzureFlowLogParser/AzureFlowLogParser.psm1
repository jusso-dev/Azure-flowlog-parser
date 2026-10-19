#Requires -Version 7.6
Set-StrictMode -Version Latest

$script:FlowLogContainerName = 'insights-logs-flowlogflowevent'
$script:AzMinimumVersions = @{
    'Az.Accounts' = '5.5.3'
    'Az.Storage'  = '9.7.2'
    'Az.Network'  = '8.3.0'
}
$script:AzModulesLoaded = $false

foreach ($folder in 'Private', 'Public') {
    foreach ($file in Get-ChildItem -Path (Join-Path $PSScriptRoot $folder) -Filter '*.ps1') {
        . $file.FullName
    }
}

Export-ModuleMember -Function 'Get-FlowLogInventory', 'Get-FlowLogTraffic'
