#Requires -Version 7.6
Set-StrictMode -Version Latest

$script:TemplatePath = Join-Path $PSScriptRoot 'Templates' 'report-template.html'

foreach ($folder in 'Private', 'Public') {
    foreach ($file in Get-ChildItem -Path (Join-Path $PSScriptRoot $folder) -Filter '*.ps1') {
        . $file.FullName
    }
}

Export-ModuleMember -Function 'Export-FlowLogReport'
