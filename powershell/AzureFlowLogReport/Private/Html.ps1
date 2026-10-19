# HTML output: the shared template with the ReportModel embedded as JSON (spec section 7).

function ConvertTo-FlowLogHtml {
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)]$Model)

    if (-not (Test-Path -LiteralPath $script:TemplatePath -PathType Leaf)) {
        throw "HTML template not found: $($script:TemplatePath)"
    }
    $template = [System.IO.File]::ReadAllText($script:TemplatePath)
    $json = ConvertTo-Json -InputObject $Model -Depth 12 -Compress
    # Keeps the JSON from closing the <script> element early.
    $json = $json.Replace('</', '<\/')
    return $template.Replace('__FLOWLOG_DATA__', $json)
}
