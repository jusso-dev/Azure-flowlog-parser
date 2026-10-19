# Minimal PDF 1.4 writer: base-14 Helvetica fonts, ASCII content, byte-accurate xref.
# Coordinates passed in are top-left based (y grows down); they are converted to PDF space here.

$script:PageWidth = 595
$script:PageHeight = 842
$script:Margin = 40
$script:BottomLimit = 802

function ConvertTo-PdfAscii {
    param([string]$Text)

    $builder = [System.Text.StringBuilder]::new()
    foreach ($char in $Text.ToCharArray()) {
        $code = [int]$char
        if ($code -eq 9 -or $code -eq 10 -or $code -eq 13) { [void]$builder.Append(' ') }
        elseif ($code -lt 32 -or $code -gt 126) { [void]$builder.Append('?') }
        else { [void]$builder.Append($char) }
    }
    return $builder.ToString()
}

function ConvertTo-PdfString {
    param([string]$Text)

    $escaped = (ConvertTo-PdfAscii -Text $Text).Replace('\', '\\').Replace('(', '\(').Replace(')', '\)')
    return "($escaped)"
}

function Format-PdfNumber {
    param([double]$Value)

    return $Value.ToString('0.###', [cultureinfo]::InvariantCulture)
}

function ConvertTo-PdfRgb {
    param([string]$Hex)

    $channels = foreach ($offset in 1, 3, 5) {
        $value = [Convert]::ToInt32($Hex.Substring($offset, 2), 16) / 255
        Format-PdfNumber -Value ([math]::Round($value, 3))
    }
    return $channels -join ' '
}

function Get-PdfTextWidth {
    param([string]$Text, [double]$Size, [switch]$Bold)

    $factor = if ($Bold) { 0.6 } else { 0.55 }
    return $Text.Length * $Size * $factor
}

function Get-PdfFittedText {
    # Truncates to the column width with '...'. Helvetica average glyph width is approximated.
    param([string]$Text, [double]$Width, [double]$Size, [switch]$Bold)

    $factor = if ($Bold) { 0.6 } else { 0.55 }
    $maxChars = [math]::Floor($Width / ($Size * $factor))
    if ($Text.Length -le $maxChars) { return $Text }
    if ($maxChars -le 3) { return $Text.Substring(0, [math]::Max(0, $maxChars)) }
    return $Text.Substring(0, $maxChars - 3) + '...'
}

function Get-PdfWrappedLine {
    param([string]$Text, [double]$Width, [double]$Size)

    $maxChars = [math]::Floor($Width / ($Size * 0.55))
    $lines = [System.Collections.Generic.List[string]]::new()
    $current = ''
    foreach ($word in @($Text -split '\s+' | Where-Object { $_ -ne '' })) {
        if ($current.Length -eq 0) { $current = $word }
        elseif (($current.Length + 1 + $word.Length) -le $maxChars) { $current += " $word" }
        else {
            $lines.Add($current)
            $current = $word
        }
    }
    if ($current.Length -gt 0) { $lines.Add($current) }
    return $lines.ToArray()
}

function New-PdfDocument {
    $doc = @{ Pages = [System.Collections.Generic.List[System.Text.StringBuilder]]::new(); Current = 0; Y = 0 }
    Add-PdfPage -Doc $doc
    return $doc
}

function Add-PdfPage {
    param($Doc)

    $Doc.Pages.Add([System.Text.StringBuilder]::new())
    $Doc.Current = $Doc.Pages.Count - 1
    $Doc.Y = $script:Margin
}

function Test-PdfSpace {
    param($Doc, [double]$Height)

    return ($Doc.Y + $Height) -le $script:BottomLimit
}

function Add-PdfOperation {
    param($Doc, [string]$Operation)

    [void]$Doc.Pages[$Doc.Current].Append($Operation).Append("`n")
}

function Add-PdfText {
    param($Doc, [double]$X, [double]$Baseline, [string]$Text, [double]$Size = 9, [switch]$Bold, [string]$Color = '#111827')

    $font = if ($Bold) { 'F2' } else { 'F1' }
    $pdfY = $script:PageHeight - $Baseline
    $escaped = ConvertTo-PdfString -Text $Text
    Add-PdfOperation -Doc $Doc -Operation ('{0} rg BT /{1} {2} Tf {3} {4} Td {5} Tj ET' -f (ConvertTo-PdfRgb -Hex $Color), $font,
        (Format-PdfNumber -Value $Size), (Format-PdfNumber -Value $X), (Format-PdfNumber -Value $pdfY), $escaped)
}

function Add-PdfRect {
    param($Doc, [double]$X, [double]$Top, [double]$Width, [double]$Height, [string]$Color)

    if ($Width -le 0) { return }
    $pdfY = $script:PageHeight - $Top - $Height
    Add-PdfOperation -Doc $Doc -Operation ('{0} rg {1} {2} {3} {4} re f' -f (ConvertTo-PdfRgb -Hex $Color),
        (Format-PdfNumber -Value $X), (Format-PdfNumber -Value $pdfY), (Format-PdfNumber -Value $Width), (Format-PdfNumber -Value $Height))
}

function Add-PdfLine {
    param($Doc, [double]$X1, [double]$Y1, [double]$X2, [double]$Y2, [string]$Color = '#D1D5DB')

    Add-PdfOperation -Doc $Doc -Operation ('{0} RG 0.5 w {1} {2} m {3} {4} l S' -f (ConvertTo-PdfRgb -Hex $Color),
        (Format-PdfNumber -Value $X1), (Format-PdfNumber -Value ($script:PageHeight - $Y1)),
        (Format-PdfNumber -Value $X2), (Format-PdfNumber -Value ($script:PageHeight - $Y2)))
}

function ConvertTo-PdfByteArray {
    # Assembles objects, xref and trailer. Every body string is ASCII, so Latin-1 bytes equal character counts.
    param([System.Text.StringBuilder[]]$ContentStreams)

    $objects = [System.Collections.Generic.List[string]]::new()
    $pageCount = $ContentStreams.Count
    $kids = (0..($pageCount - 1) | ForEach-Object { '{0} 0 R' -f (6 + 2 * $_) }) -join ' '
    $objects.Add('<< /Type /Catalog /Pages 2 0 R >>')
    $objects.Add("<< /Type /Pages /Kids [ $kids ] /Count $pageCount >>")
    $objects.Add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>')
    $objects.Add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>')
    $objects.Add('<< /Title (Azure VNet Flow Log Report) /Producer (AzureFlowLogReport) >>')
    for ($i = 0; $i -lt $pageCount; $i++) {
        $objects.Add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 $($script:PageWidth) $($script:PageHeight)] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents $(7 + 2 * $i) 0 R >>")
        $content = $ContentStreams[$i].ToString()
        $objects.Add("<< /Length $($content.Length) >>`nstream`n$content`nendstream")
    }

    $builder = [System.Text.StringBuilder]::new()
    [void]$builder.Append("%PDF-1.4`n%")
    foreach ($code in 0xE2, 0xE3, 0xCF, 0xD3) { [void]$builder.Append([char]$code) }
    [void]$builder.Append("`n")

    $offsets = [System.Collections.Generic.List[int]]::new()
    for ($n = 0; $n -lt $objects.Count; $n++) {
        $offsets.Add($builder.Length)
        [void]$builder.Append("$($n + 1) 0 obj`n$($objects[$n])`nendobj`n")
    }

    $xrefOffset = $builder.Length
    [void]$builder.Append("xref`n0 $($objects.Count + 1)`n0000000000 65535 f `n")
    foreach ($offset in $offsets) {
        [void]$builder.Append("{0:D10} 00000 n `n" -f $offset)
    }
    [void]$builder.Append("trailer`n<< /Size $($objects.Count + 1) /Root 1 0 R /Info 5 0 R >>`nstartxref`n$xrefOffset`n%%EOF`n")

    return [System.Text.Encoding]::Latin1.GetBytes($builder.ToString())
}
