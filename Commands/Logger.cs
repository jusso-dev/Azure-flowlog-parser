namespace AzureFlowLogParser.Commands;

/// <summary>Warnings always go to stderr; progress only with --verbose.</summary>
public static class Logger
{
    public static Action<string> For(bool verbose)
    {
        return message =>
        {
            if (verbose || message.StartsWith("Warning:", StringComparison.Ordinal))
            {
                Console.Error.WriteLine(message);
            }
        };
    }
}
