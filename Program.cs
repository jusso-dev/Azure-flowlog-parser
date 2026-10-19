using System.CommandLine;
using System.Text.Json;
using AzureFlowLogParser.Commands;
using AzureFlowLogParser.Serialization;
using AzureFlowLogParser.Services;

namespace AzureFlowLogParser;

class Program
{
    static async Task<int> Main(string[] args)
    {
        // Define CLI options for storage account sources
        var storageAccountOption = new Option<string?>("--storage-account", "-s")
        {
            Description = "Single Azure storage account name (use this OR one of the multi-account options)"
        };

        var accountsFileOption = new Option<string?>("--accounts-file", "-af")
        {
            Description = "Path to file containing storage account names (one per line or comma-separated)"
        };

        var accountsEnvOption = new Option<string?>("--accounts-env", "-ae")
        {
            Description = "Environment variable name containing comma-separated storage account names"
        };

        var accountsKeyVaultOption = new Option<string?>("--accounts-keyvault", "-akv")
        {
            Description = "Azure Key Vault URL (e.g., https://myvault.vault.azure.net/)"
        };

        var keyVaultSecretOption = new Option<string?>("--keyvault-secret", "-kvs")
        {
            Description = "Key Vault secret name containing comma-separated storage account names"
        };

        // Container and filtering options
        var containerOption = new Option<string>("--container", "-c")
        {
            Description = "The container name containing flow logs",
            DefaultValueFactory = _ => "insights-logs-flowlogflowevent"
        };

        var prefixOption = new Option<string?>("--prefix", "-p")
        {
            Description = "Filter blobs by prefix (optional)"
        };

        // Output options
        var outputOption = new Option<string?>("--output", "-o")
        {
            Description = "Output file path (optional, prints to stdout if not specified)"
        };

        var formatOption = new Option<string>("--format", "-f")
        {
            Description = "Output format: json (default) or jsonl (JSON Lines)",
            DefaultValueFactory = _ => "json"
        };

        var limitOption = new Option<int?>("--limit", "-l")
        {
            Description = "Limit the number of blobs to process per storage account (optional)"
        };

        // Control options
        var verboseOption = new Option<bool>("--verbose", "-v")
        {
            Description = "Enable verbose output"
        };

        var listOnlyOption = new Option<bool>("--list-only")
        {
            Description = "Only list available blobs without processing"
        };

        var mergeOutputOption = new Option<bool>("--merge-output", "-m")
        {
            Description = "Merge results from all storage accounts into a single output (default: true)",
            DefaultValueFactory = _ => true
        };

        // HTTP posting options
        var httpEndpointOption = new Option<string?>("--http-endpoint", "-he")
        {
            Description = "HTTP endpoint URL to POST flow logs to (optional)"
        };

        var httpTokenOption = new Option<string?>("--http-token", "-ht")
        {
            Description = "Bearer token for HTTP endpoint authentication (optional)"
        };

        var httpCompressionOption = new Option<bool>("--http-compression", "-hc")
        {
            Description = "Enable gzip compression for HTTP requests (default: true)",
            DefaultValueFactory = _ => true
        };

        var httpTimeoutOption = new Option<int>("--http-timeout")
        {
            Description = "HTTP request timeout in seconds (default: 300)",
            DefaultValueFactory = _ => 300
        };

        var httpTestOption = new Option<bool>("--http-test")
        {
            Description = "Test HTTP endpoint connectivity without processing logs"
        };

        var httpKeyVaultOption = new Option<string?>("--http-keyvault", "-hkv")
        {
            Description = "Azure Key Vault URL to load HTTP endpoint and token from"
        };

        var httpEndpointSecretOption = new Option<string>("--http-endpoint-secret", "-hes")
        {
            Description = "Key Vault secret name for HTTP endpoint URL",
            DefaultValueFactory = _ => "cortexendpoint"
        };

        var httpTokenSecretOption = new Option<string>("--http-token-secret", "-hts")
        {
            Description = "Key Vault secret name for Bearer token",
            DefaultValueFactory = _ => "cortextoken"
        };

        // State tracking options
        var enableStateTrackingOption = new Option<bool>("--enable-state-tracking", "-st")
        {
            Description = "Enable state tracking using blob metadata to avoid reprocessing (useful for CI/CD pipelines)",
            DefaultValueFactory = _ => true
        };

        var forceReprocessOption = new Option<bool>("--force-reprocess", "-fr")
        {
            Description = "Force reprocessing of all blobs, ignoring metadata state"
        };

        // Create root command
        var rootCommand = new RootCommand("Azure VNet Flow Log Parser - Fetches and parses Azure virtual network flow logs using Entra ID credentials");

        rootCommand.Add(storageAccountOption);
        rootCommand.Add(accountsFileOption);
        rootCommand.Add(accountsEnvOption);
        rootCommand.Add(accountsKeyVaultOption);
        rootCommand.Add(keyVaultSecretOption);
        rootCommand.Add(containerOption);
        rootCommand.Add(prefixOption);
        rootCommand.Add(outputOption);
        rootCommand.Add(formatOption);
        rootCommand.Add(limitOption);
        rootCommand.Add(verboseOption);
        rootCommand.Add(listOnlyOption);
        rootCommand.Add(mergeOutputOption);
        rootCommand.Add(httpEndpointOption);
        rootCommand.Add(httpTokenOption);
        rootCommand.Add(httpCompressionOption);
        rootCommand.Add(httpTimeoutOption);
        rootCommand.Add(httpTestOption);
        rootCommand.Add(httpKeyVaultOption);
        rootCommand.Add(httpEndpointSecretOption);
        rootCommand.Add(httpTokenSecretOption);
        rootCommand.Add(enableStateTrackingOption);
        rootCommand.Add(forceReprocessOption);

        // Discovery, monitoring and traffic report subcommands
        rootCommand.Add(DiscoverCommand.Create());
        rootCommand.Add(ReportCommand.Create());

        rootCommand.SetAction(async (ParseResult parseResult, CancellationToken cancellationToken) =>
        {
            var storageAccount = parseResult.GetValue(storageAccountOption);
            var accountsFile = parseResult.GetValue(accountsFileOption);
            var accountsEnv = parseResult.GetValue(accountsEnvOption);
            var accountsKeyVault = parseResult.GetValue(accountsKeyVaultOption);
            var keyVaultSecret = parseResult.GetValue(keyVaultSecretOption);
            var container = parseResult.GetValue(containerOption);
            var prefix = parseResult.GetValue(prefixOption);
            var output = parseResult.GetValue(outputOption);
            var format = parseResult.GetValue(formatOption);
            var limit = parseResult.GetValue(limitOption);
            var verbose = parseResult.GetValue(verboseOption);
            var listOnly = parseResult.GetValue(listOnlyOption);
            var mergeOutput = parseResult.GetValue(mergeOutputOption);
            var httpEndpoint = parseResult.GetValue(httpEndpointOption);
            var httpToken = parseResult.GetValue(httpTokenOption);
            var httpCompression = parseResult.GetValue(httpCompressionOption);
            var httpTimeout = parseResult.GetValue(httpTimeoutOption);
            var httpTest = parseResult.GetValue(httpTestOption);
            var httpKeyVault = parseResult.GetValue(httpKeyVaultOption);
            var httpEndpointSecret = parseResult.GetValue(httpEndpointSecretOption);
            var httpTokenSecret = parseResult.GetValue(httpTokenSecretOption);
            var enableStateTracking = parseResult.GetValue(enableStateTrackingOption);
            var forceReprocess = parseResult.GetValue(forceReprocessOption);

            await ProcessFlowLogsAsync(
                storageAccount,
                accountsFile,
                accountsEnv,
                accountsKeyVault,
                keyVaultSecret,
                container!,
                prefix,
                output,
                format!,
                limit,
                verbose,
                listOnly,
                mergeOutput,
                httpEndpoint,
                httpToken,
                httpCompression,
                httpTimeout,
                httpTest,
                httpKeyVault,
                httpEndpointSecret!,
                httpTokenSecret!,
                enableStateTracking,
                forceReprocess);

            return 0;
        });

        return await rootCommand.Parse(args).InvokeAsync();
    }

    static async Task ProcessFlowLogsAsync(
        string? storageAccount,
        string? accountsFile,
        string? accountsEnv,
        string? accountsKeyVault,
        string? keyVaultSecret,
        string container,
        string? prefix,
        string? outputPath,
        string format,
        int? limit,
        bool verbose,
        bool listOnly,
        bool mergeOutput,
        string? httpEndpoint,
        string? httpToken,
        bool httpCompression,
        int httpTimeout,
        bool httpTest,
        string? httpKeyVault,
        string httpEndpointSecret,
        string httpTokenSecret,
        bool enableStateTracking,
        bool forceReprocess)
    {
        try
        {
            // Load HTTP endpoint and token from Key Vault if specified
            if (!string.IsNullOrWhiteSpace(httpKeyVault))
            {
                if (verbose)
                    Console.Error.WriteLine($"Loading HTTP credentials from Key Vault: {httpKeyVault}");

                try
                {
                    // Load endpoint if not already provided
                    if (string.IsNullOrWhiteSpace(httpEndpoint))
                    {
                        var endpoints = await StorageAccountConfigLoader.LoadFromKeyVaultAsync(httpKeyVault, httpEndpointSecret);
                        httpEndpoint = endpoints.FirstOrDefault();

                        if (verbose && !string.IsNullOrWhiteSpace(httpEndpoint))
                            Console.Error.WriteLine($"  Loaded endpoint from secret '{httpEndpointSecret}'");
                    }

                    // Load token if not already provided
                    if (string.IsNullOrWhiteSpace(httpToken))
                    {
                        var tokens = await StorageAccountConfigLoader.LoadFromKeyVaultAsync(httpKeyVault, httpTokenSecret);
                        httpToken = tokens.FirstOrDefault();

                        if (verbose && !string.IsNullOrWhiteSpace(httpToken))
                            Console.Error.WriteLine($"  Loaded token from secret '{httpTokenSecret}'");
                    }
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine($"Error loading HTTP credentials from Key Vault: {ex.Message}");
                    if (verbose)
                        Console.Error.WriteLine($"  Stack trace: {ex.StackTrace}");
                    Environment.Exit(1);
                    return;
                }
            }

            // Handle HTTP test mode
            if (httpTest)
            {
                if (string.IsNullOrWhiteSpace(httpEndpoint))
                {
                    Console.Error.WriteLine("Error: --http-endpoint or --http-keyvault is required when using --http-test");
                    Environment.Exit(1);
                    return;
                }

                using var testClient = new FlowLogHttpClient(
                    httpEndpoint,
                    httpToken,
                    httpCompression,
                    httpTimeout,
                    maxRetries: 3,
                    verbose: verbose);

                var success = await testClient.TestConnectivityAsync(verbose);
                Environment.Exit(success ? 0 : 1);
                return;
            }

            // Load storage accounts from the specified source
            List<string> storageAccounts;

            var sourcesProvided = new[]
            {
                !string.IsNullOrEmpty(storageAccount),
                !string.IsNullOrEmpty(accountsFile),
                !string.IsNullOrEmpty(accountsEnv),
                !string.IsNullOrEmpty(accountsKeyVault)
            }.Count(x => x);

            if (sourcesProvided == 0)
            {
                Console.Error.WriteLine("Error: You must specify a storage account source:");
                Console.Error.WriteLine("  --storage-account <name>           Single storage account");
                Console.Error.WriteLine("  --accounts-file <path>             File with account names");
                Console.Error.WriteLine("  --accounts-env <variable>          Environment variable");
                Console.Error.WriteLine("  --accounts-keyvault <url>          Azure Key Vault");
                Environment.Exit(1);
                return;
            }

            if (sourcesProvided > 1)
            {
                Console.Error.WriteLine("Error: You can only specify one storage account source at a time");
                Environment.Exit(1);
                return;
            }

            if (verbose)
                Console.Error.WriteLine("Loading storage account configuration...");

            if (!string.IsNullOrEmpty(storageAccount))
            {
                storageAccounts = new List<string> { storageAccount };
                if (verbose)
                    Console.Error.WriteLine($"Using single storage account: {storageAccount}");
            }
            else if (!string.IsNullOrEmpty(accountsFile))
            {
                storageAccounts = await StorageAccountConfigLoader.LoadFromFileAsync(accountsFile);
                if (verbose)
                    Console.Error.WriteLine($"Loaded {storageAccounts.Count} storage account(s) from file: {accountsFile}");
            }
            else if (!string.IsNullOrEmpty(accountsEnv))
            {
                storageAccounts = StorageAccountConfigLoader.LoadFromEnvironmentVariable(accountsEnv);
                if (verbose)
                    Console.Error.WriteLine($"Loaded {storageAccounts.Count} storage account(s) from environment variable: {accountsEnv}");
            }
            else if (!string.IsNullOrEmpty(accountsKeyVault))
            {
                if (string.IsNullOrEmpty(keyVaultSecret))
                {
                    Console.Error.WriteLine("Error: --keyvault-secret is required when using --accounts-keyvault");
                    Environment.Exit(1);
                    return;
                }

                storageAccounts = await StorageAccountConfigLoader.LoadFromKeyVaultAsync(accountsKeyVault, keyVaultSecret);
                if (verbose)
                    Console.Error.WriteLine($"Loaded {storageAccounts.Count} storage account(s) from Key Vault: {accountsKeyVault}");
            }
            else
            {
                throw new InvalidOperationException("No storage account source specified");
            }

            // Validate storage account names
            storageAccounts = StorageAccountConfigLoader.ValidateStorageAccounts(storageAccounts);

            if (verbose)
            {
                Console.Error.WriteLine($"Processing {storageAccounts.Count} storage account(s):");
                foreach (var account in storageAccounts)
                {
                    Console.Error.WriteLine($"  - {account}");
                }

                if (enableStateTracking)
                {
                    Console.Error.WriteLine("State tracking enabled: Using blob metadata to track processing");
                    if (forceReprocess)
                        Console.Error.WriteLine("Force reprocess enabled - ignoring blob metadata state");
                }
            }

            // Process all storage accounts
            var allDenormalizedRecords = new List<Models.DenormalizedFlowRecord>();

            foreach (var account in storageAccounts)
            {
                if (verbose)
                    Console.Error.WriteLine($"\n=== Processing storage account: {account} ===");

                try
                {
                    var records = await ProcessSingleStorageAccountAsync(
                        account,
                        container,
                        prefix,
                        limit,
                        verbose,
                        listOnly,
                        enableStateTracking,
                        forceReprocess);

                    if (mergeOutput)
                    {
                        allDenormalizedRecords.AddRange(records);
                    }
                    else if (!listOnly)
                    {
                        // Post to HTTP endpoint if configured (per account)
                        if (!string.IsNullOrWhiteSpace(httpEndpoint))
                        {
                            if (verbose)
                                Console.Error.WriteLine($"Posting {records.Count} records from {account} to HTTP endpoint...");

                            using var httpClient = new FlowLogHttpClient(
                                httpEndpoint,
                                httpToken,
                                httpCompression,
                                httpTimeout,
                                maxRetries: 3,
                                verbose: verbose);

                            await httpClient.PostFlowLogsAsync(records, verbose);
                        }

                        // Output results for this account separately (if file output specified or no HTTP endpoint)
                        if (!string.IsNullOrWhiteSpace(outputPath) || string.IsNullOrWhiteSpace(httpEndpoint))
                        {
                            await OutputResultsAsync(
                                records,
                                format,
                                outputPath != null ? $"{Path.GetFileNameWithoutExtension(outputPath)}_{account}{Path.GetExtension(outputPath)}" : null,
                                verbose,
                                account);
                        }
                    }
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine($"Error processing storage account '{account}': {ex.Message}");
                    if (verbose)
                        Console.Error.WriteLine($"  Stack trace: {ex.StackTrace}");
                }
            }

            // Output merged results
            if (mergeOutput && !listOnly)
            {
                if (verbose)
                    Console.Error.WriteLine($"\nTotal records parsed from all accounts: {allDenormalizedRecords.Count}");

                // Post to HTTP endpoint if configured
                if (!string.IsNullOrWhiteSpace(httpEndpoint))
                {
                    if (verbose)
                        Console.Error.WriteLine($"\n=== Posting to HTTP endpoint ===");

                    using var httpClient = new FlowLogHttpClient(
                        httpEndpoint,
                        httpToken,
                        httpCompression,
                        httpTimeout,
                        maxRetries: 3,
                        verbose: verbose);

                    await httpClient.PostFlowLogsAsync(allDenormalizedRecords, verbose);
                }

                // Output to file or stdout if configured
                if (!string.IsNullOrWhiteSpace(outputPath) || string.IsNullOrWhiteSpace(httpEndpoint))
                {
                    await OutputResultsAsync(allDenormalizedRecords, format, outputPath, verbose);
                }
            }
        }
        catch (Azure.RequestFailedException ex)
        {
            Console.Error.WriteLine($"Azure Error: {ex.Message}");
            Console.Error.WriteLine($"Status Code: {ex.Status}");
            Console.Error.WriteLine($"Error Code: {ex.ErrorCode}");

            if (ex.Message.Contains("AuthenticationFailed") || ex.Message.Contains("Authorization"))
            {
                Console.Error.WriteLine();
                Console.Error.WriteLine("Authentication failed. Ensure that:");
                Console.Error.WriteLine("  1. Managed Identity is enabled on your Azure resource");
                Console.Error.WriteLine("  2. The identity has 'Storage Blob Data Reader' role on the storage account(s)");
                Console.Error.WriteLine("  3. For Key Vault: The identity has 'Key Vault Secrets User' role");
                Console.Error.WriteLine("  4. For local development, you're authenticated via Azure CLI (az login)");
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

    static async Task<List<Models.DenormalizedFlowRecord>> ProcessSingleStorageAccountAsync(
        string storageAccount,
        string container,
        string? prefix,
        int? limit,
        bool verbose,
        bool listOnly,
        bool enableStateTracking,
        bool forceReprocess)
    {
        if (verbose)
        {
            Console.Error.WriteLine($"Container: {container}");
            if (!string.IsNullOrEmpty(prefix))
                Console.Error.WriteLine($"Prefix filter: {prefix}");
        }

        // Initialize Azure Storage client with MSI
        var storageClient = new AzureStorageClient(storageAccount, verbose);

        // Test authentication before proceeding
        if (verbose)
            Console.Error.WriteLine("Testing authentication...");

        var authSuccess = await storageClient.TestAuthenticationAsync(verbose);
        if (!authSuccess)
        {
            throw new InvalidOperationException($"Authentication failed for storage account '{storageAccount}'");
        }

        // List blobs with metadata if state tracking is enabled
        if (verbose)
            Console.Error.WriteLine("Fetching blob list...");

        List<string> blobsToProcess;
        Dictionary<string, DateTimeOffset> blobMetadata = new();

        if (enableStateTracking && !listOnly)
        {
            // Get blobs with metadata for state tracking
            var blobsWithMetadata = await storageClient.ListBlobsWithMetadataAsync(container, prefix);

            if (verbose)
                Console.Error.WriteLine($"Found {blobsWithMetadata.Count} blob(s)");

            // Filter blobs based on metadata state
            var filteredBlobs = new List<string>();
            int skippedCount = 0;

            foreach (var blobInfo in blobsWithMetadata)
            {
                blobMetadata[blobInfo.Name] = blobInfo.LastModified;

                if (forceReprocess)
                {
                    filteredBlobs.Add(blobInfo.Name);
                }
                else
                {
                    var shouldProcess = await storageClient.ShouldProcessBlobAsync(container, blobInfo.Name, blobInfo.LastModified);
                    if (shouldProcess)
                    {
                        filteredBlobs.Add(blobInfo.Name);
                    }
                    else
                    {
                        skippedCount++;
                    }
                }
            }

            blobsToProcess = filteredBlobs;

            if (verbose && skippedCount > 0)
                Console.Error.WriteLine($"Skipped {skippedCount} blob(s) based on metadata state");
        }
        else
        {
            // Regular blob listing without metadata
            blobsToProcess = await storageClient.ListBlobsAsync(container, prefix);

            if (verbose)
                Console.Error.WriteLine($"Found {blobsToProcess.Count} blob(s)");
        }

        if (blobsToProcess.Count == 0)
        {
            Console.Error.WriteLine($"No blobs to process in storage account '{storageAccount}'");
            return new List<Models.DenormalizedFlowRecord>();
        }

        // Apply limit if specified
        if (limit.HasValue && limit.Value > 0)
        {
            blobsToProcess = blobsToProcess.Take(limit.Value).ToList();
            if (verbose)
                Console.Error.WriteLine($"Limited to {blobsToProcess.Count} blob(s)");
        }

        // List only mode
        if (listOnly)
        {
            Console.WriteLine($"\nAvailable blobs in {storageAccount}:");
            foreach (var blob in blobsToProcess)
            {
                Console.WriteLine($"  - {blob}");
            }
            return new List<Models.DenormalizedFlowRecord>();
        }

        // Download and parse blobs
        var denormalizedRecords = new List<Models.DenormalizedFlowRecord>();

        foreach (var blobName in blobsToProcess)
        {
            if (verbose)
                Console.Error.WriteLine($"Processing: {blobName}");

            try
            {
                var content = await storageClient.DownloadBlobAsStringAsync(container, blobName);
                var records = FlowLogParser.ParseFlowLog(content);

                if (verbose)
                    Console.Error.WriteLine($"  Parsed {records.Count} flow record(s)");

                denormalizedRecords.AddRange(records);

                // Mark blob as processed if state tracking is enabled
                if (enableStateTracking && blobMetadata.TryGetValue(blobName, out var lastModified))
                {
                    await storageClient.MarkBlobAsProcessedAsync(container, blobName, lastModified, records.Count);

                    if (verbose)
                        Console.Error.WriteLine($"  Marked as processed in blob metadata");
                }
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"Error processing blob {blobName}: {ex.Message}");
                if (verbose)
                    Console.Error.WriteLine($"  Stack trace: {ex.StackTrace}");
            }
        }

        if (verbose)
            Console.Error.WriteLine($"Total records from {storageAccount}: {denormalizedRecords.Count}");

        return denormalizedRecords;
    }

    static async Task OutputResultsAsync(
        List<Models.DenormalizedFlowRecord> records,
        string format,
        string? outputPath,
        bool verbose,
        string? accountName = null)
    {
        if (records.Count == 0)
        {
            if (verbose)
                Console.Error.WriteLine("No records to output");
            return;
        }

        string outputContent;

        if (format.ToLower() == "jsonl")
        {
            // JSON Lines format - one JSON object per line
            var lines = records.Select(r => JsonSerializer.Serialize(r, AppJsonContext.Default.DenormalizedFlowRecord));
            outputContent = string.Join(Environment.NewLine, lines);
        }
        else
        {
            // Standard JSON array format (matching reference implementation)
            outputContent = FlowLogParser.FormatAsJson(records, indented: true);
        }

        // Write to file or stdout
        if (!string.IsNullOrEmpty(outputPath))
        {
            await File.WriteAllTextAsync(outputPath, outputContent);
            if (verbose)
            {
                var accountInfo = accountName != null ? $" (from {accountName})" : "";
                Console.Error.WriteLine($"Output written to: {outputPath}{accountInfo}");
            }
        }
        else
        {
            Console.WriteLine(outputContent);
        }
    }
}
