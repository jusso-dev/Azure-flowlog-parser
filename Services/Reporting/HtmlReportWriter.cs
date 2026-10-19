using System.Reflection;
using System.Text;
using System.Text.Json;
using AzureFlowLogParser.Models;
using AzureFlowLogParser.Services.Common;

namespace AzureFlowLogParser.Services.Reporting;

/// <summary>
/// Writes the self-contained interactive HTML report. The template is embedded from shared/report-template.html,
/// which the PowerShell module ships as an identical copy.
/// </summary>
public static class HtmlReportWriter
{
    public const string DataPlaceholder = "__FLOWLOG_DATA__";

    public static void Write(ReportModel model, string path)
    {
        var template = LoadTemplate();
        var json = JsonSerializer.Serialize(model, JsonOptions.Compact).Replace("</", "<\\/", StringComparison.Ordinal);
        var html = template.Replace(DataPlaceholder, json, StringComparison.Ordinal);
        File.WriteAllText(path, html, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
    }

    private static string LoadTemplate()
    {
        using var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream("report-template.html")
            ?? throw new InvalidOperationException("Embedded report template is missing.");
        using var reader = new StreamReader(stream, Encoding.UTF8);
        return reader.ReadToEnd();
    }
}
