using System.CommandLine;
using System.Text.Json;
using AzureFlowLogParser.Services;

namespace AzureFlowLogParser;

class Program
{
    static async Task<int> Main(string[] args)
    {
        // Define CLI options
        var storageAccountOption = new Option<string>(
            name: "--storage-account",
            description: "The Azure storage account name containing flow logs")
        {
            IsRequired = true
        };
        storageAccountOption.AddAlias("-s");

        var containerOption = new Option<string>(
            name: "--container",
            description: "The container name containing flow logs",
            getDefaultValue: () => "insights-logs-flowlogflowevent");
        containerOption.AddAlias("-c");

        var prefixOption = new Option<string?>(
            name: "--prefix",
            description: "Filter blobs by prefix (optional)",
            getDefaultValue: () => null);
        prefixOption.AddAlias("-p");

        var outputOption = new Option<string?>(
            name: "--output",
            description: "Output file path (optional, prints to stdout if not specified)",
            getDefaultValue: () => null);
        outputOption.AddAlias("-o");

        var formatOption = new Option<string>(
            name: "--format",
            description: "Output format: json (default) or jsonl (JSON Lines)",
            getDefaultValue: () => "json");
        formatOption.AddAlias("-f");

        var limitOption = new Option<int?>(
            name: "--limit",
            description: "Limit the number of blobs to process (optional)",
            getDefaultValue: () => null);
        limitOption.AddAlias("-l");

        var verboseOption = new Option<bool>(
            name: "--verbose",
            description: "Enable verbose output",
            getDefaultValue: () => false);
        verboseOption.AddAlias("-v");

        var listOnlyOption = new Option<bool>(
            name: "--list-only",
            description: "Only list available blobs without processing",
            getDefaultValue: () => false);

        // Create root command
        var rootCommand = new RootCommand("Azure VNet Flow Log Parser - Fetches and parses Azure virtual network flow logs using MSI credentials");

        rootCommand.AddOption(storageAccountOption);
        rootCommand.AddOption(containerOption);
        rootCommand.AddOption(prefixOption);
        rootCommand.AddOption(outputOption);
        rootCommand.AddOption(formatOption);
        rootCommand.AddOption(limitOption);
        rootCommand.AddOption(verboseOption);
        rootCommand.AddOption(listOnlyOption);

        rootCommand.SetHandler(async (
            string storageAccount,
            string container,
            string? prefix,
            string? output,
            string format,
            int? limit,
            bool verbose,
            bool listOnly) =>
        {
            await ProcessFlowLogsAsync(
                storageAccount,
                container,
                prefix,
                output,
                format,
                limit,
                verbose,
                listOnly);
        },
        storageAccountOption,
        containerOption,
        prefixOption,
        outputOption,
        formatOption,
        limitOption,
        verboseOption,
        listOnlyOption);

        return await rootCommand.InvokeAsync(args);
    }

    static async Task ProcessFlowLogsAsync(
        string storageAccount,
        string container,
        string? prefix,
        string? outputPath,
        string format,
        int? limit,
        bool verbose,
        bool listOnly)
    {
        try
        {
            if (verbose)
            {
                Console.Error.WriteLine($"Connecting to storage account: {storageAccount}");
                Console.Error.WriteLine($"Container: {container}");
                if (!string.IsNullOrEmpty(prefix))
                    Console.Error.WriteLine($"Prefix filter: {prefix}");
            }

            // Initialize Azure Storage client with MSI
            var storageClient = new AzureStorageClient(storageAccount);

            // List blobs
            if (verbose)
                Console.Error.WriteLine("Fetching blob list...");

            var blobs = await storageClient.ListBlobsAsync(container, prefix);

            if (verbose)
                Console.Error.WriteLine($"Found {blobs.Count} blob(s)");

            if (blobs.Count == 0)
            {
                Console.Error.WriteLine("No blobs found matching the criteria");
                return;
            }

            // Apply limit if specified
            if (limit.HasValue && limit.Value > 0)
            {
                blobs = blobs.Take(limit.Value).ToList();
                if (verbose)
                    Console.Error.WriteLine($"Limited to {blobs.Count} blob(s)");
            }

            // List only mode
            if (listOnly)
            {
                Console.WriteLine("Available blobs:");
                foreach (var blob in blobs)
                {
                    Console.WriteLine($"  - {blob}");
                }
                return;
            }

            // Download and parse blobs
            var allDenormalizedRecords = new List<Models.DenormalizedFlowRecord>();

            foreach (var blobName in blobs)
            {
                if (verbose)
                    Console.Error.WriteLine($"Processing: {blobName}");

                try
                {
                    var content = await storageClient.DownloadBlobAsStringAsync(container, blobName);
                    var records = FlowLogParser.ParseFlowLog(content);

                    if (verbose)
                        Console.Error.WriteLine($"  Parsed {records.Count} flow record(s)");

                    allDenormalizedRecords.AddRange(records);
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine($"Error processing blob {blobName}: {ex.Message}");
                    if (verbose)
                        Console.Error.WriteLine($"  Stack trace: {ex.StackTrace}");
                }
            }

            if (verbose)
                Console.Error.WriteLine($"Total records parsed: {allDenormalizedRecords.Count}");

            // Format and output results
            string outputContent;

            if (format.ToLower() == "jsonl")
            {
                // JSON Lines format - one JSON object per line
                var lines = allDenormalizedRecords.Select(r =>
                    JsonSerializer.Serialize(r, new JsonSerializerOptions
                    {
                        DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull
                    }));
                outputContent = string.Join(Environment.NewLine, lines);
            }
            else
            {
                // Standard JSON array format (matching reference implementation)
                outputContent = FlowLogParser.FormatAsJson(allDenormalizedRecords, indented: true);
            }

            // Write to file or stdout
            if (!string.IsNullOrEmpty(outputPath))
            {
                await File.WriteAllTextAsync(outputPath, outputContent);
                if (verbose)
                    Console.Error.WriteLine($"Output written to: {outputPath}");
            }
            else
            {
                Console.WriteLine(outputContent);
            }
        }
        catch (Azure.RequestFailedException ex)
        {
            Console.Error.WriteLine($"Azure Storage Error: {ex.Message}");
            Console.Error.WriteLine($"Status Code: {ex.Status}");
            Console.Error.WriteLine($"Error Code: {ex.ErrorCode}");

            if (ex.Message.Contains("AuthenticationFailed") || ex.Message.Contains("Authorization"))
            {
                Console.Error.WriteLine();
                Console.Error.WriteLine("Authentication failed. Ensure that:");
                Console.Error.WriteLine("  1. Managed Identity is enabled on your Azure resource");
                Console.Error.WriteLine("  2. The identity has 'Storage Blob Data Reader' role on the storage account");
                Console.Error.WriteLine("  3. For local development, you're authenticated via Azure CLI (az login)");
            }

            Environment.Exit(1);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Error: {ex.Message}");
            if (verbose)
                Console.Error.WriteLine($"Stack trace: {ex.StackTrace}");
            Environment.Exit(1);
        }
    }
}
