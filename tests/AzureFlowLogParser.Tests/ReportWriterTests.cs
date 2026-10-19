using System.Diagnostics;
using System.Text;
using System.Text.Json;
using AzureFlowLogParser.Commands;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Reporting;
using AzureFlowLogParser.Services.Reporting.Pdf;
using Xunit;

namespace AzureFlowLogParser.Tests;

public class ReportWriterTests
{
    private static ReportModel SampleModel()
    {
        return new ReportModel
        {
            GeneratedUtc = "2026-10-19T10:00:00Z",
            Source = "local",
            Window = new ReportWindow { FromUtc = "2024-01-01T00:00:00Z", ToUtc = "2024-02-01T00:00:00Z" },
            Filters = new ReportFilters { Action = "all", Protocol = "all" },
            Summary = new ReportSummary { Flows = 2, Allowed = 1, Denied = 1, Bytes = 2048, Rows = 2, RowsTotal = 2 },
            Scopes =
            {
                new ReportScope { Id = "stflowlogs01", Kind = ScopeKind.Storage, Label = "stflowlogs01", StorageAccount = "stflowlogs01" },
                new ReportScope { Id = "internet", Kind = ScopeKind.Internet, Label = "Internet" }
            },
            Flows =
            {
                new ReportFlow { First = "2024-01-15T10:30:00Z", Last = "2024-01-15T10:31:00Z", Src = "10.0.1.4", Dst = "13.107.42.14", SrcScope = "stflowlogs01", DstScope = "internet", Proto = "TCP", DstPort = 443, Action = "A", Direction = "O", Rule = "DefaultRule_AllowInternetOutBound", Flows = 1, Packets = 5, Bytes = 2048 },
                new ReportFlow { First = "2024-01-15T10:32:00Z", Last = "2024-01-15T10:32:00Z", Src = "192.168.1.100", Dst = "10.0.1.4", SrcScope = "unmapped", DstScope = "stflowlogs01", Proto = "TCP", DstPort = 22, Action = "D", Direction = "I", Rule = "Rule with ( parens ) and \\ backslash", Flows = 1 }
            },
            Notes = { "Counts are flow log observations.", "Unicode check: café → ok" }
        };
    }

    [Fact]
    public void Html_embeds_model_once_and_escapes_script_terminators()
    {
        var model = SampleModel();
        model.Flows[1].Rule = "</script><script>alert(1)</script>";
        var path = Path.Combine(TestPaths.NewTempDirectory(), "report.html");

        HtmlReportWriter.Write(model, path);
        var html = File.ReadAllText(path);

        Assert.DoesNotContain("__FLOWLOG_DATA__", html);
        var start = html.IndexOf("<script id=\"flowlog-data\" type=\"application/json\">", StringComparison.Ordinal);
        Assert.True(start >= 0);
        var dataStart = start + "<script id=\"flowlog-data\" type=\"application/json\">".Length;
        var dataEnd = html.IndexOf("</script>", dataStart, StringComparison.Ordinal);
        var json = html[dataStart..dataEnd];

        using var doc = JsonDocument.Parse(json);
        Assert.Equal(2, doc.RootElement.GetProperty("flows").GetArrayLength());
        Assert.Equal("</script><script>alert(1)</script>", doc.RootElement.GetProperty("flows")[1].GetProperty("rule").GetString());
        Assert.DoesNotContain("</script><script>alert", json);
    }

    [Fact]
    public void Html_template_is_self_contained()
    {
        var path = Path.Combine(TestPaths.NewTempDirectory(), "report.html");
        HtmlReportWriter.Write(SampleModel(), path);
        var html = File.ReadAllText(path);

        Assert.DoesNotContain("<script src=", html);
        Assert.DoesNotContain("<link rel=\"stylesheet\"", html);
        Assert.DoesNotContain("https://", html.Replace("http://www.w3.org/2000/svg", string.Empty));
    }

    [Fact]
    public void Pdf_has_valid_header_trailer_and_xref_offset()
    {
        var path = Path.Combine(TestPaths.NewTempDirectory(), "report.pdf");

        PdfReportWriter.Write(SampleModel(), path);
        var bytes = File.ReadAllBytes(path);
        var text = Encoding.Latin1.GetString(bytes);

        Assert.StartsWith("%PDF-1.4", text, StringComparison.Ordinal);
        Assert.EndsWith("%%EOF\n", text, StringComparison.Ordinal);

        var startxref = text.LastIndexOf("startxref\n", StringComparison.Ordinal);
        var offsetText = text[(startxref + "startxref\n".Length)..].Split('\n')[0];
        var offset = int.Parse(offsetText);
        Assert.Equal("xref", Encoding.Latin1.GetString(bytes, offset, 4));
    }

    [Fact]
    public void Pdf_xref_entries_are_twenty_bytes_and_point_at_objects()
    {
        var document = new PdfDocument();
        var layout = new PdfLayout(document);
        layout.Title("Check");
        layout.Table(["A", "B"], [100, 100], Enumerable.Range(0, 80).Select(i => new[] { new PdfCell($"row {i}"), new PdfCell($"{i}") }).ToList());
        layout.Footers();
        var bytes = document.Build();
        var text = Encoding.Latin1.GetString(bytes);

        Assert.True(document.Pages.Count >= 2, "table should paginate");

        var xrefStart = text.IndexOf("xref\n", StringComparison.Ordinal);
        var lines = text[xrefStart..].Split('\n');
        var entries = lines.Skip(2).TakeWhile(l => !l.StartsWith("trailer", StringComparison.Ordinal)).ToList();

        Assert.Equal(lines[1].Split(' ')[1], entries.Count.ToString());
        for (var i = 1; i < entries.Count; i++)
        {
            var entry = entries[i];
            Assert.Equal(19, entry.Length); // 19 characters plus the newline make 20 bytes
            Assert.EndsWith(" n ", entry, StringComparison.Ordinal);
            var offset = int.Parse(entry[..10]);
            Assert.StartsWith($"{i} 0 obj", text[offset..], StringComparison.Ordinal);
        }
    }

    [Fact]
    public void Pdf_passes_qpdf_check_when_qpdf_is_installed()
    {
        var qpdf = FindOnPath("qpdf");
        if (qpdf is null)
        {
            return; // qpdf is optional; the structural tests above still run.
        }

        var path = Path.Combine(TestPaths.NewTempDirectory(), "qpdf-check.pdf");
        PdfReportWriter.Write(SampleModel(), path);

        var start = new ProcessStartInfo(qpdf, new[] { "--check", path }) { RedirectStandardOutput = true, RedirectStandardError = true };
        using var process = Process.Start(start)!;
        var output = process.StandardOutput.ReadToEnd() + process.StandardError.ReadToEnd();
        process.WaitForExit();

        Assert.Equal(0, process.ExitCode);
        Assert.Contains("No syntax or stream encoding errors found", output);
    }

    [Fact]
    public async Task Report_command_writes_pdf_from_offline_input()
    {
        var output = Path.Combine(TestPaths.NewTempDirectory(), "offline.pdf");
        var command = ReportCommand.Create();

        var exit = await command.Parse(
            ["--input", TestPaths.SampleFlowLog, "--since", "2024-01-01T00:00:00Z", "--until", "2024-02-01T00:00:00Z", "--report", "pdf", "-o", output]).InvokeAsync(configuration: null, cancellationToken: TestContext.Current.CancellationToken);

        Assert.Equal(0, exit);
        Assert.StartsWith("%PDF-1.4", Encoding.Latin1.GetString(File.ReadAllBytes(output), 0, 8), StringComparison.Ordinal);
    }

    [Fact]
    public async Task Report_command_with_topology_attributes_flows_to_storage_accounts()
    {
        var output = Path.Combine(TestPaths.NewTempDirectory(), "topology.html");
        var command = ReportCommand.Create();

        var exit = await command.Parse(
            ["--input", TestPaths.SampleFlowLog, "--topology", TestPaths.Topology, "--since", "2024-01-01T00:00:00Z", "--until", "2024-02-01T00:00:00Z", "--report", "html", "-o", output]).InvokeAsync(configuration: null, cancellationToken: TestContext.Current.CancellationToken);

        Assert.Equal(0, exit);
        var html = File.ReadAllText(output);
        var start = html.IndexOf("type=\"application/json\">", StringComparison.Ordinal) + "type=\"application/json\">".Length;
        using var doc = JsonDocument.Parse(html[start..html.IndexOf("</script>", start, StringComparison.Ordinal)]);
        var scopes = doc.RootElement.GetProperty("scopes").EnumerateArray().Select(s => s.GetProperty("id").GetString()).ToList();
        Assert.Contains("stflowlogs01", scopes);
        // 10.0.2.10 belongs to a read storage account, so its allowed inbound observation is the duplicate and is skipped.
        var summary = doc.RootElement.GetProperty("summary");
        Assert.Equal(5, summary.GetProperty("flows").GetInt64());
        Assert.Equal(1, summary.GetProperty("observationsSkipped").GetInt64());
    }

    [Fact]
    public async Task Offline_input_without_bounds_reports_the_observed_range()
    {
        var output = Path.Combine(TestPaths.NewTempDirectory(), "observed.html");

        var exit = await ReportCommand.Create().Parse(["--input", TestPaths.SampleFlowLog, "--report", "html", "-o", output])
            .InvokeAsync(configuration: null, cancellationToken: TestContext.Current.CancellationToken);

        Assert.Equal(0, exit);
        var html = File.ReadAllText(output);
        var start = html.IndexOf("type=\"application/json\">", StringComparison.Ordinal) + "type=\"application/json\">".Length;
        using var doc = JsonDocument.Parse(html[start..html.IndexOf("</script>", start, StringComparison.Ordinal)]);
        var window = doc.RootElement.GetProperty("window");
        // Sample tuples start at epoch 1705318200 (11:30 UTC) and end at 1705318260 (11:31 UTC).
        Assert.Equal("2024-01-15T11:30:00Z", window.GetProperty("fromUtc").GetString());
        Assert.Equal("2024-01-15T11:31:00Z", window.GetProperty("toUtc").GetString());
    }

    [Fact]
    public async Task Report_command_rejects_bad_window()
    {
        var command = ReportCommand.Create();

        var exit = await command.Parse(["--input", TestPaths.SampleFlowLog, "--since", "2025-01-01T00:00:00Z", "--until", "2024-01-01T00:00:00Z"]).InvokeAsync(configuration: null, cancellationToken: TestContext.Current.CancellationToken);

        Assert.Equal(1, exit);
    }

    private static string? FindOnPath(string tool)
    {
        foreach (var dir in (Environment.GetEnvironmentVariable("PATH") ?? string.Empty).Split(Path.PathSeparator))
        {
            var candidate = Path.Combine(dir, tool);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        return null;
    }
}
