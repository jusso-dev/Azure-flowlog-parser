namespace AzureFlowLogParser.Tests;

internal static class TestPaths
{
    public static string Fixture(string name) => Path.Combine(AppContext.BaseDirectory, "Fixtures", name);

    public static string SampleFlowLog => Fixture("sample-flowlog.json");

    public static string Topology => Fixture("topology.json");

    public static string NewTempDirectory()
    {
        var path = Path.Combine(Path.GetTempPath(), "flowlog-tests-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(path);
        return path;
    }
}
