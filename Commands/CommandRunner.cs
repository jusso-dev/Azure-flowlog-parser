using Azure;

namespace AzureFlowLogParser.Commands;

/// <summary>Maps unexpected failures to a one-line error and exit code 1 (stack trace only with --verbose).</summary>
public static class CommandRunner
{
    public static async Task<int> RunAsync(Func<Task<int>> body, bool verbose)
    {
        try
        {
            return await body();
        }
        catch (OperationCanceledException)
        {
            await Console.Error.WriteLineAsync("Cancelled.");
            return 130;
        }
        catch (RequestFailedException ex)
        {
            var firstLine = ex.Message.Split('\n', 2)[0].Trim();
            await Console.Error.WriteLineAsync($"Error: {firstLine} (HTTP {ex.Status}{(ex.ErrorCode is null ? string.Empty : ", " + ex.ErrorCode)})");
            if (verbose)
            {
                await Console.Error.WriteLineAsync(ex.ToString());
            }

            return 1;
        }
        catch (Exception ex)
        {
            await Console.Error.WriteLineAsync($"Error: {ex.Message}");
            if (verbose)
            {
                await Console.Error.WriteLineAsync(ex.ToString());
            }

            return 1;
        }
    }
}
