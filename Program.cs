using System.CommandLine;
using System.Text.Json;
using AzureFlowLogParser.Services;

namespace AzureFlowLogParser;

class Program
{
    static async Task<int> Main(string[] args)
    {
        // Define CLI options for storage account sources
        var storageAccountOption = new Option<string?>(
            name: "--storage-account",
            description: "Single Azure storage account name (use this OR one of the multi-account options)")
        {
            IsRequired = false
        };
        storageAccountOption.AddAlias("-s");

        var accountsFileOption = new Option<string?>(
            name: "--accounts-file",
            description: "Path to file containing storage account names (one per line or comma-separated)",
            getDefaultValue: () => null);
        accountsFileOption.AddAlias("-af");

        var accountsEnvOption = new Option<string?>(
            name: "--accounts-env",
            description: "Environment variable name containing comma-separated storage account names",
            getDefaultValue: () => null);
        accountsEnvOption.AddAlias("-ae");

        var accountsKeyVaultOption = new Option<string?>(
            name: "--accounts-keyvault",
            description: "Azure Key Vault URL (e.g., https://myvault.vault.azure.net/)",
            getDefaultValue: () => null);
        accountsKeyVaultOption.AddAlias("-akv");

        var keyVaultSecretOption = new Option<string?>(
            name: "--keyvault-secret",
            description: "Key Vault secret name containing comma-separated storage account names",
            getDefaultValue: () => null);
        keyVaultSecretOption.AddAlias("-kvs");

        // Container and filtering options
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

        // Output options
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
            description: "Limit the number of blobs to process per storage account (optional)",
            getDefaultValue: () => null);
        limitOption.AddAlias("-l");

        // Control options
        var verboseOption = new Option<bool>(
            name: "--verbose",
            description: "Enable verbose output",
            getDefaultValue: () => false);
        verboseOption.AddAlias("-v");

        var listOnlyOption = new Option<bool>(
            name: "--list-only",
            description: "Only list available blobs without processing",
            getDefaultValue: () => false);

        var mergeOutputOption = new Option<bool>(
            name: "--merge-output",
            description: "Merge results from all storage accounts into a single output (default: true)",
            getDefaultValue: () => true);
        mergeOutputOption.AddAlias("-m");

        // HTTP posting options
        var httpEndpointOption = new Option<string?>(
            name: "--http-endpoint",
            description: "HTTP endpoint URL to POST flow logs to (optional)",
            getDefaultValue: () => null);
        httpEndpointOption.AddAlias("-he");

        var httpTokenOption = new Option<string?>(
            name: "--http-token",
            description: "Bearer token for HTTP endpoint authentication (optional)",
            getDefaultValue: () => null);
        httpTokenOption.AddAlias("-ht");

        var httpCompressionOption = new Option<bool>(
            name: "--http-compression",
            description: "Enable gzip compression for HTTP requests (default: true)",
            getDefaultValue: () => true);
        httpCompressionOption.AddAlias("-hc");

        var httpTimeoutOption = new Option<int>(
            name: "--http-timeout",
            description: "HTTP request timeout in seconds (default: 300)",
            getDefaultValue: () => 300);

        var httpTestOption = new Option<bool>(
            name: "--http-test",
            description: "Test HTTP endpoint connectivity without processing logs",
            getDefaultValue: () => false);

        var httpKeyVaultOption = new Option<string?>(
            name: "--http-keyvault",
            description: "Azure Key Vault URL to load HTTP endpoint and token from",
            getDefaultValue: () => null);
        httpKeyVaultOption.AddAlias("-hkv");

        var httpEndpointSecretOption = new Option<string>(
            name: "--http-endpoint-secret",
            description: "Key Vault secret name for HTTP endpoint URL",
            getDefaultValue: () => "cortexendpoint");
        httpEndpointSecretOption.AddAlias("-hes");

        var httpTokenSecretOption = new Option<string>(
            name: "--http-token-secret",
            description: "Key Vault secret name for Bearer token",
            getDefaultValue: () => "cortextoken");
        httpTokenSecretOption.AddAlias("-hts");

        // State tracking options
        var enableStateTrackingOption = new Option<bool>(
            name: "--enable-state-tracking",
            description: "Enable state tracking using blob metadata to avoid reprocessing (useful for CI/CD pipelines)",
            getDefaultValue: () => false);
        enableStateTrackingOption.AddAlias("-st");

        var forceReprocessOption = new Option<bool>(
            name: "--force-reprocess",
            description: "Force reprocessing of all blobs, ignoring metadata state",
            getDefaultValue: () => false);
        forceReprocessOption.AddAlias("-fr");

        // Create root command
        var rootCommand = new RootCommand("Azure VNet Flow Log Parser - Fetches and parses Azure virtual network flow logs using MSI credentials");

        rootCommand.AddOption(storageAccountOption);
        rootCommand.AddOption(accountsFileOption);
        rootCommand.AddOption(accountsEnvOption);
        rootCommand.AddOption(accountsKeyVaultOption);
        rootCommand.AddOption(keyVaultSecretOption);
        rootCommand.AddOption(containerOption);
        rootCommand.AddOption(prefixOption);
        rootCommand.AddOption(outputOption);
        rootCommand.AddOption(formatOption);
        rootCommand.AddOption(limitOption);
        rootCommand.AddOption(verboseOption);
        rootCommand.AddOption(listOnlyOption);
        rootCommand.AddOption(mergeOutputOption);
        rootCommand.AddOption(httpEndpointOption);
        rootCommand.AddOption(httpTokenOption);
        rootCommand.AddOption(httpCompressionOption);
        rootCommand.AddOption(httpTimeoutOption);
        rootCommand.AddOption(httpTestOption);
        rootCommand.AddOption(httpKeyVaultOption);
        rootCommand.AddOption(httpEndpointSecretOption);
        rootCommand.AddOption(httpTokenSecretOption);
        rootCommand.AddOption(enableStateTrackingOption);
        rootCommand.AddOption(forceReprocessOption);

        rootCommand.SetHandler(async (
            string? storageAccount,
            string? accountsFile,
            string? accountsEnv,
            string? accountsKeyVault,
            string? keyVaultSecret,
            string container,
            string? prefix,
            string? output,
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
            bool forceReprocess) =>
        {
            await ProcessFlowLogsAsync(
                storageAccount,
                accountsFile,
                accountsEnv,
                accountsKeyVault,
                keyVaultSecret,
                container,
                prefix,
                output,
                format,
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
                httpEndpointSecret,
                httpTokenSecret,
                enableStateTracking,
                forceReprocess);
        },
        storageAccountOption,
        accountsFileOption,
        accountsEnvOption,
        accountsKeyVaultOption,
        keyVaultSecretOption,
        containerOption,
        prefixOption,
        outputOption,
        formatOption,
        limitOption,
        verboseOption,
        listOnlyOption,
        mergeOutputOption,
        httpEndpointOption,
        httpTokenOption,
        httpCompressionOption,
        httpTimeoutOption,
        httpTestOption,
        httpKeyVaultOption,
        httpEndpointSecretOption,
        httpTokenSecretOption,
        enableStateTrackingOption,
        forceReprocessOption);

        return await rootCommand.InvokeAsync(args);
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
            var lines = records.Select(r =>
                JsonSerializer.Serialize(r, new JsonSerializerOptions
                {
                    DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull
                }));
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
