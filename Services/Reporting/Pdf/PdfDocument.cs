using System.Globalization;
using System.Text;

namespace AzureFlowLogParser.Services.Reporting.Pdf;

/// <summary>
/// Minimal PDF 1.4 writer: base-14 Helvetica fonts, filled rectangles, lines and text.
/// Coordinates passed to the page API are top-down points on an A4 portrait page.
/// No third-party dependency; output is validated with qpdf in the test suite.
/// </summary>
public sealed class PdfDocument
{
    public const double PageWidth = 595;
    public const double PageHeight = 842;

    private readonly List<PdfPage> _pages = new();

    public IReadOnlyList<PdfPage> Pages => _pages;

    public PdfPage AddPage()
    {
        var page = new PdfPage();
        _pages.Add(page);
        return page;
    }

    public byte[] Build()
    {
        if (_pages.Count == 0)
        {
            AddPage();
        }

        // Every character is ASCII or Latin-1 so string offsets equal byte offsets.
        var pdf = new StringBuilder();
        var offsets = new List<int>();

        void BeginObject(int number)
        {
            while (offsets.Count < number) offsets.Add(0);
            offsets[number - 1] = pdf.Length;
            pdf.Append(number).Append(" 0 obj\n");
        }

        pdf.Append("%PDF-1.4\n%âãÏÓ\n");

        var pageIds = Enumerable.Range(0, _pages.Count).Select(i => 5 + 2 * i).ToList();

        BeginObject(1);
        pdf.Append("<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        BeginObject(2);
        pdf.Append("<< /Type /Pages /Kids [").Append(string.Join(" ", pageIds.Select(id => $"{id} 0 R"))).Append($"] /Count {_pages.Count} >>\nendobj\n");

        BeginObject(3);
        pdf.Append("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n");

        BeginObject(4);
        pdf.Append("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>\nendobj\n");

        for (var i = 0; i < _pages.Count; i++)
        {
            var pageId = 5 + 2 * i;
            var contentId = pageId + 1;
            var content = _pages[i].Content;

            BeginObject(pageId);
            pdf.Append("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents ")
               .Append(contentId).Append(" 0 R >>\nendobj\n");

            BeginObject(contentId);
            pdf.Append("<< /Length ").Append(content.Length).Append(" >>\nstream\n").Append(content).Append("\nendstream\nendobj\n");
        }

        var xrefOffset = pdf.Length;
        var size = offsets.Count + 1;
        pdf.Append("xref\n0 ").Append(size).Append('\n');
        pdf.Append("0000000000 65535 f \n");
        foreach (var offset in offsets)
        {
            // Each entry is exactly 20 bytes, as the xref format requires.
            pdf.Append(offset.ToString("D10", CultureInfo.InvariantCulture)).Append(" 00000 n \n");
        }

        pdf.Append("trailer\n<< /Size ").Append(size).Append(" /Root 1 0 R >>\n");
        pdf.Append("startxref\n").Append(xrefOffset).Append("\n%%EOF\n");

        return Encoding.Latin1.GetBytes(pdf.ToString());
    }
}

/// <summary>One page's content stream.</summary>
public sealed class PdfPage
{
    private readonly StringBuilder _content = new();

    internal string Content => _content.ToString();

    /// <summary>Draws text with its top edge at <paramref name="top"/>.</summary>
    public void Text(string text, double x, double top, double size, PdfColor color, bool bold = false)
    {
        var baseline = PdfDocument.PageHeight - top - size;
        _content.Append("BT /").Append(bold ? "F2" : "F1").Append(' ')
            .Append(Num(size)).Append(" Tf ")
            .Append(color.Components()).Append(" rg ")
            .Append("1 0 0 1 ").Append(Num(x)).Append(' ').Append(Num(baseline)).Append(" Tm ")
            .Append('(').Append(PdfText.Escape(text)).Append(") Tj ET\n");
    }

    public void FillRect(double x, double top, double width, double height, PdfColor color)
    {
        var bottom = PdfDocument.PageHeight - top - height;
        _content.Append("q ").Append(color.Components()).Append(" rg ")
            .Append(Num(x)).Append(' ').Append(Num(bottom)).Append(' ')
            .Append(Num(width)).Append(' ').Append(Num(height)).Append(" re f Q\n");
    }

    public void Line(double x1, double top1, double x2, double top2, PdfColor color, double width = 0.5)
    {
        var y1 = PdfDocument.PageHeight - top1;
        var y2 = PdfDocument.PageHeight - top2;
        _content.Append("q ").Append(color.Components()).Append(" RG ")
            .Append(Num(width)).Append(" w ")
            .Append(Num(x1)).Append(' ').Append(Num(y1)).Append(" m ")
            .Append(Num(x2)).Append(' ').Append(Num(y2)).Append(" l S Q\n");
    }

    private static string Num(double value)
    {
        return value.ToString("0.###", CultureInfo.InvariantCulture);
    }
}

public readonly record struct PdfColor(double R, double G, double B)
{
    public static readonly PdfColor Black = new(0, 0, 0);
    public static readonly PdfColor White = new(1, 1, 1);

    public static PdfColor Hex(string hex)
    {
        var value = Convert.ToInt32(hex.TrimStart('#'), 16);
        return new PdfColor(((value >> 16) & 0xFF) / 255.0, ((value >> 8) & 0xFF) / 255.0, (value & 0xFF) / 255.0);
    }

    /// <summary>Operands only ("r g b"); callers append the rg (fill) or RG (stroke) operator.</summary>
    public string Components()
    {
        return $"{R.ToString("0.###", CultureInfo.InvariantCulture)} {G.ToString("0.###", CultureInfo.InvariantCulture)} {B.ToString("0.###", CultureInfo.InvariantCulture)}";
    }
}

/// <summary>Text helpers for Helvetica metrics (approximate) and WinAnsi-safe output.</summary>
public static class PdfText
{
    /// <summary>Replaces characters outside printable ASCII with '?'.</summary>
    public static string Sanitize(string text)
    {
        var builder = new StringBuilder(text.Length);
        foreach (var c in text)
        {
            builder.Append(c >= 32 && c <= 126 ? c : '?');
        }

        return builder.ToString();
    }

    public static string Escape(string text)
    {
        return Sanitize(text).Replace("\\", "\\\\").Replace("(", "\\(").Replace(")", "\\)");
    }

    /// <summary>Approximate Helvetica advance width in points (average glyph width ~0.5em, bold ~0.6em).</summary>
    public static double Width(string text, double size, bool bold = false)
    {
        return text.Length * size * (bold ? 0.6 : 0.52);
    }

    /// <summary>Truncates to fit the width, adding an ellipsis when needed.</summary>
    public static string Fit(string text, double maxWidth, double size, bool bold = false)
    {
        text = Sanitize(text);
        if (Width(text, size, bold) <= maxWidth)
        {
            return text;
        }

        var maxChars = Math.Max(0, (int)Math.Floor(maxWidth / (size * (bold ? 0.6 : 0.52))) - 2);
        return text[..Math.Min(maxChars, text.Length)] + "..";
    }

    public static IEnumerable<string> Wrap(string text, double maxWidth, double size, bool bold = false)
    {
        var words = Sanitize(text).Split(' ', StringSplitOptions.RemoveEmptyEntries);
        var line = new StringBuilder();

        foreach (var word in words)
        {
            var candidate = line.Length == 0 ? word : line + " " + word;
            if (Width(candidate, size, bold) > maxWidth && line.Length > 0)
            {
                yield return line.ToString();
                line.Clear().Append(word);
            }
            else
            {
                line.Clear().Append(candidate);
            }
        }

        if (line.Length > 0)
        {
            yield return line.ToString();
        }
    }
}
