#Requires -Version 7.6
Set-StrictMode -Version Latest

$script:FlowLogContainerName = 'insights-logs-flowlogflowevent'

foreach ($folder in 'Private', 'Public') {
    foreach ($file in Get-ChildItem -Path (Join-Path $PSScriptRoot $folder) -Filter '*.ps1') {
        . $file.FullName
    }
}

Export-ModuleMember -Function 'Get-FlowLogInventory', 'Get-FlowLogTraffic', 'Get-FlowLogRecord', 'Get-FlowLogDiagnosis'
