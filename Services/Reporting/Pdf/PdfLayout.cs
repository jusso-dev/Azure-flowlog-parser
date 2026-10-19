namespace AzureFlowLogParser.Services.Reporting.Pdf;

/// <summary>A table cell. Bar &gt;= 0 draws a proportional bar (0..1) instead of text.</summary>
public readonly record struct PdfCell(string Text, double Bar = -1);

/// <summary>Top-down flow layout over <see cref="PdfDocument"/> with automatic page breaks.</summary>
public sealed class PdfLayout
{
    public const double Margin = 40;
    public const double ContentWidth = PdfDocument.PageWidth - 2 * Margin;
    private const double BottomLimit = PdfDocument.PageHeight - 56;
    private const double RowHeight = 14;
    private const double HeaderHeight = 16;

    private static readonly PdfColor Ink = PdfColor.Hex("#111827");
    private static readonly PdfColor Muted = PdfColor.Hex("#4B5563");
    private static readonly PdfColor HeaderFill = PdfColor.Hex("#1F2937");
    private static readonly PdfColor Zebra = PdfColor.Hex("#F3F4F6");
    private static readonly PdfColor Border = PdfColor.Hex("#D1D5DB");
    private static readonly PdfColor BarFill = PdfColor.Hex("#2563EB");
    private static readonly PdfColor Accent = PdfColor.Hex("#1D4ED8");

    private readonly PdfDocument _document;
    private PdfPage _page;
    private double _top;

    public PdfLayout(PdfDocument document)
    {
        _document = document;
        _page = document.AddPage();
        _top = Margin;
    }

    public void Title(string text)
    {
        Ensure(30);
        _page.Text(text, Margin, _top, 20, Ink, bold: true);
        _top += 30;
    }

    public void Heading(string text)
    {
        Ensure(40);
        _top += 8;
        _page.Text(text, Margin, _top, 13, Accent, bold: true);
        _top += 18;
        _page.Line(Margin, _top, Margin + ContentWidth, _top, Border, 0.8);
        _top += 6;
    }

    public void Paragraph(string text, double size = 9, bool muted = false, bool bold = false)
    {
        foreach (var line in PdfText.Wrap(text, ContentWidth, size, bold))
        {
            Ensure(size + 4);
            _page.Text(line, Margin, _top, size, muted ? Muted : Ink, bold);
            _top += size + 4;
        }
    }

    public void Bullet(string text)
    {
        var lines = PdfText.Wrap(text, ContentWidth - 12, 9).ToList();
        for (var i = 0; i < lines.Count; i++)
        {
            Ensure(13);
            if (i == 0)
            {
                _page.Text("-", Margin, _top, 9, Ink);
            }

            _page.Text(lines[i], Margin + 12, _top, 9, Ink);
            _top += 13;
        }
    }

    public void Space(double points)
    {
        _top += points;
    }

    /// <summary>Draws a table. Widths must add up to at most <see cref="ContentWidth"/>.</summary>
    public void Table(string[] headers, double[] widths, IReadOnlyList<PdfCell[]> rows, double fontSize = 8)
    {
        Ensure(HeaderHeight + RowHeight);
        DrawHeader(headers, widths, fontSize);

        for (var r = 0; r < rows.Count; r++)
        {
            if (_top + RowHeight > BottomLimit)
            {
                NewPage();
                DrawHeader(headers, widths, fontSize);
            }

            var row = rows[r];
            if (r % 2 == 1)
            {
                _page.FillRect(Margin, _top, ContentWidth, RowHeight, Zebra);
            }

            var x = Margin;
            for (var c = 0; c < widths.Length; c++)
            {
                var cell = c < row.Length ? row[c] : new PdfCell(string.Empty);
                if (cell.Bar >= 0)
                {
                    var barWidth = Math.Max(0, (widths[c] - 8) * Math.Min(1, cell.Bar));
                    if (barWidth > 0)
                    {
                        _page.FillRect(x + 4, _top + 3, barWidth, RowHeight - 6, BarFill);
                    }
                }
                else
                {
                    _page.Text(PdfText.Fit(cell.Text, widths[c] - 8, fontSize), x + 4, _top + 3, fontSize, Ink);
                }

                x += widths[c];
            }

            _top += RowHeight;
            _page.Line(Margin, _top, Margin + ContentWidth, _top, Border, 0.4);
        }

        _top += 6;
    }

    /// <summary>Writes "Page N" centred in the footer of every page. Call once, after all content.</summary>
    public void Footers()
    {
        for (var i = 0; i < _document.Pages.Count; i++)
        {
            var text = $"Page {i + 1} of {_document.Pages.Count}";
            _document.Pages[i].Text(text, PdfDocument.PageWidth / 2 - PdfText.Width(text, 8) / 2, PdfDocument.PageHeight - 34, 8, Muted);
        }
    }

    /// <summary>Draws the header row. Header text wraps onto extra lines when a narrow column cannot hold it.</summary>
    private void DrawHeader(string[] headers, double[] widths, double fontSize)
    {
        var lines = headers.Select((header, c) => PdfText.Wrap(header, widths[c] - 8, fontSize, bold: true).ToList()).ToList();
        var lineHeight = fontSize + 2;
        var height = Math.Max(HeaderHeight, lines.Max(l => l.Count) * lineHeight + 6);

        _page.FillRect(Margin, _top, ContentWidth, height, HeaderFill);
        var x = Margin;
        for (var c = 0; c < headers.Length; c++)
        {
            for (var line = 0; line < lines[c].Count; line++)
            {
                _page.Text(lines[c][line], x + 4, _top + 4 + line * lineHeight, fontSize, PdfColor.White, bold: true);
            }

            x += widths[c];
        }

        _top += height;
    }

    private void Ensure(double height)
    {
        if (_top + height > BottomLimit)
        {
            NewPage();
        }
    }

    private void NewPage()
    {
        _page = _document.AddPage();
        _top = Margin;
    }
}
