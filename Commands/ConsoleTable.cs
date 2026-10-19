namespace AzureFlowLogParser.Commands;

/// <summary>Plain-text table for console output: left-aligned columns sized to the widest cell.</summary>
public static class ConsoleTable
{
    public static void Write(TextWriter writer, string[] headers, IReadOnlyList<string[]> rows)
    {
        var widths = headers.Select(h => h.Length).ToArray();
        foreach (var row in rows)
        {
            for (var i = 0; i < widths.Length && i < row.Length; i++)
            {
                widths[i] = Math.Max(widths[i], row[i].Length);
            }
        }

        WriteRow(writer, headers, widths);
        writer.WriteLine(string.Join("  ", widths.Select(w => new string('-', w))));
        foreach (var row in rows)
        {
            WriteRow(writer, row, widths);
        }
    }

    private static void WriteRow(TextWriter writer, string[] cells, int[] widths)
    {
        var parts = widths.Select((w, i) => (i < cells.Length ? cells[i] : string.Empty).PadRight(w));
        writer.WriteLine(string.Join("  ", parts).TrimEnd());
    }
}
