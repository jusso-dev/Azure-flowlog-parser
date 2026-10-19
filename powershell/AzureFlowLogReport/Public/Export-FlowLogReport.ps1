function Export-FlowLogReport {
    <#
    .SYNOPSIS
    Writes a traffic report (HTML or PDF) from a ReportModel.

    .DESCRIPTION
    Takes the object returned by Get-FlowLogTraffic (AzureFlowLogParser module), accepts it from the pipeline,
    and writes either an interactive HTML file (the report template with the model embedded as JSON) or a
    static PDF (hand-written PDF 1.4, Helvetica, A4 portrait). The HTML and PDF show the same content;
    HTML adds client-side filtering, an SVG graph and sortable tables.

    .PARAMETER Model
    ReportModel (schema 1) from Get-FlowLogTraffic. Accepts pipeline input.

    .PARAMETER Format
    Html or Pdf.

    .PARAMETER Path
    Output file path. The folder must exist. Relative paths resolve against the current location.

    .OUTPUTS
    System.IO.FileInfo for the written file.

    .EXAMPLE
    Get-FlowLogTraffic -InputPath ./sample-flowlog.json | Export-FlowLogReport -Format Html -Path ./flowlog-report.html

    .EXAMPLE
    $model = Get-FlowLogTraffic -Since 7d -Action Deny
    Export-FlowLogReport -Model $model -Format Pdf -Path ./denied-7d.pdf
    #>
    [CmdletBinding()]
    [OutputType([System.IO.FileInfo])]
    param(
        [Parameter(Mandatory, ValueFromPipeline)][ValidateNotNull()]$Model,
        [Parameter(Mandatory)][ValidateSet('Html', 'Pdf')][string]$Format,
        [Parameter(Mandatory)][string]$Path
    )

    process {
        if ($Model.schema -ne 1) {
            throw 'Unsupported model. Pass the object returned by Get-FlowLogTraffic (schema 1).'
        }
        $fullPath = $PSCmdlet.GetUnresolvedProviderPathFromPSPath($Path)
        $folder = Split-Path -Parent $fullPath
        if (-not (Test-Path -LiteralPath $folder -PathType Container)) {
            throw "Output folder not found: $folder"
        }

        if ($Format -eq 'Html') {
            $html = ConvertTo-FlowLogHtml -Model $Model
            [System.IO.File]::WriteAllText($fullPath, $html, [System.Text.UTF8Encoding]::new($false))
        }
        else {
            $bytes = New-FlowLogPdf -Model $Model
            [System.IO.File]::WriteAllBytes($fullPath, $bytes)
        }
        Get-Item -LiteralPath $fullPath
    }
}
