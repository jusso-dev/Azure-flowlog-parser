using Azure.Identity;
using Azure.Storage.Blobs;
using Azure.Storage.Blobs.Models;
using Azure.Core.Diagnostics;
using System.Diagnostics.Tracing;

namespace AzureFlowLogParser.Services;

/// <summary>
/// Azure Storage client that uses Managed Service Identity (MSI) for authentication
/// </summary>
public class AzureStorageClient
{
    private readonly BlobServiceClient _blobServiceClient;
    private readonly string _storageAccountName;

    /// <summary>
    /// Initializes the Azure Storage client with MSI authentication
    /// </summary>
    /// <param name="storageAccountName">The name of the storage account</param>
    /// <param name="verbose">Enable verbose logging for authentication diagnostics</param>
    public AzureStorageClient(string storageAccountName, bool verbose = false)
    {
        _storageAccountName = storageAccountName;
        var storageUri = new Uri($"https://{storageAccountName}.blob.core.windows.net");

        if (verbose)
        {
            Console.Error.WriteLine($"Initializing Azure Storage client for: {storageAccountName}");
            Console.Error.WriteLine($"Storage URI: {storageUri}");
            Console.Error.WriteLine("Authentication: Using DefaultAzureCredential");
            Console.Error.WriteLine("  Credential chain will attempt (in order):");
            Console.Error.WriteLine("    1. Environment variables (AZURE_CLIENT_ID, AZURE_CLIENT_SECRET, AZURE_TENANT_ID)");
            Console.Error.WriteLine("    2. Managed Identity (if running in Azure)");
            Console.Error.WriteLine("    3. Azure CLI (az login)");
            Console.Error.WriteLine("    4. Azure PowerShell");
            Console.Error.WriteLine("    5. Visual Studio / VS Code credentials");
        }

        // Use DefaultAzureCredential which supports:
        // - Environment variables (AZURE_CLIENT_ID, AZURE_CLIENT_SECRET, AZURE_TENANT_ID)
        // - Managed Identity (MSI) in Azure
        // - Azure CLI credentials for local development
        // - Visual Studio/VS Code credentials
        var credentialOptions = new DefaultAzureCredentialOptions
        {
            ExcludeVisualStudioCredential = false,
            ExcludeVisualStudioCodeCredential = false,
            ExcludeAzureCliCredential = false,
            ExcludeManagedIdentityCredential = false,
            ExcludeEnvironmentCredential = false,
            ExcludeAzurePowerShellCredential = false,
            // Add retry options
            Retry =
            {
                MaxRetries = 3,
                NetworkTimeout = TimeSpan.FromSeconds(30)
            }
        };

        var credential = new DefaultAzureCredential(credentialOptions);

        _blobServiceClient = new BlobServiceClient(storageUri, credential);

        if (verbose)
        {
            Console.Error.WriteLine("Azure Storage client initialized successfully");
        }
    }

    /// <summary>
    /// Tests authentication by attempting to list containers
    /// </summary>
    /// <returns>True if authentication is successful</returns>
    public async Task<bool> TestAuthenticationAsync(bool verbose = false)
    {
        try
        {
            if (verbose)
                Console.Error.WriteLine($"Testing authentication for storage account: {_storageAccountName}");

            // Attempt to list containers (minimal permission required)
            var containers = _blobServiceClient.GetBlobContainersAsync();
            await foreach (var _ in containers.Take(1))
            {
                // Successfully authenticated and can access storage
                break;
            }

            if (verbose)
                Console.Error.WriteLine("  ✓ Authentication successful");

            return true;
        }
        catch (Azure.RequestFailedException ex)
        {
            Console.Error.WriteLine($"  ✗ Authentication failed for storage account '{_storageAccountName}'");
            Console.Error.WriteLine($"    Error: {ex.Message}");
            Console.Error.WriteLine($"    Status: {ex.Status}");
            Console.Error.WriteLine($"    Error Code: {ex.ErrorCode}");

            PrintAuthenticationHelp();
            return false;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"  ✗ Unexpected error during authentication test: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Prints helpful authentication troubleshooting information
    /// </summary>
    private static void PrintAuthenticationHelp()
    {
        Console.Error.WriteLine();
        Console.Error.WriteLine("Authentication Troubleshooting:");
        Console.Error.WriteLine();
        Console.Error.WriteLine("For LOCAL DEVELOPMENT:");
        Console.Error.WriteLine("  1. Run 'az login' to authenticate with Azure CLI");
        Console.Error.WriteLine("  2. Run 'az account show' to verify you're logged in");
        Console.Error.WriteLine("  3. Ensure you have 'Storage Blob Data Reader' role on the storage account");
        Console.Error.WriteLine();
        Console.Error.WriteLine("For AZURE ENVIRONMENT (VM, Container, App Service, etc.):");
        Console.Error.WriteLine("  1. Enable Managed Identity on your Azure resource");
        Console.Error.WriteLine("  2. Grant the identity 'Storage Blob Data Reader' role:");
        Console.Error.WriteLine("     az role assignment create \\");
        Console.Error.WriteLine("       --role 'Storage Blob Data Reader' \\");
        Console.Error.WriteLine("       --assignee <managed-identity-principal-id> \\");
        Console.Error.WriteLine("       --scope /subscriptions/<sub-id>/resourceGroups/<rg>/providers/Microsoft.Storage/storageAccounts/<account>");
        Console.Error.WriteLine();
        Console.Error.WriteLine("For SERVICE PRINCIPAL:");
        Console.Error.WriteLine("  Set environment variables:");
        Console.Error.WriteLine("    export AZURE_CLIENT_ID='<client-id>'");
        Console.Error.WriteLine("    export AZURE_CLIENT_SECRET='<client-secret>'");
        Console.Error.WriteLine("    export AZURE_TENANT_ID='<tenant-id>'");
        Console.Error.WriteLine();
    }

    /// <summary>
    /// Lists all blobs in a container
    /// </summary>
    /// <param name="containerName">The container name (default: insights-logs-flowlogflowevent)</param>
    /// <param name="prefix">Optional prefix to filter blobs</param>
    /// <returns>List of blob names</returns>
    public async Task<List<string>> ListBlobsAsync(string containerName, string? prefix = null)
    {
        var containerClient = _blobServiceClient.GetBlobContainerClient(containerName);
        var blobs = new List<string>();

        await foreach (var blobItem in containerClient.GetBlobsAsync(prefix: prefix))
        {
            blobs.Add(blobItem.Name);
        }

        return blobs;
    }

    /// <summary>
    /// Lists all blobs in a container with metadata
    /// </summary>
    /// <param name="containerName">The container name (default: insights-logs-flowlogflowevent)</param>
    /// <param name="prefix">Optional prefix to filter blobs</param>
    /// <returns>List of blob information with metadata</returns>
    public async Task<List<BlobInfo>> ListBlobsWithMetadataAsync(string containerName, string? prefix = null)
    {
        var containerClient = _blobServiceClient.GetBlobContainerClient(containerName);
        var blobs = new List<BlobInfo>();

        await foreach (var blobItem in containerClient.GetBlobsAsync(prefix: prefix))
        {
            blobs.Add(new BlobInfo
            {
                Name = blobItem.Name,
                LastModified = blobItem.Properties.LastModified ?? DateTimeOffset.UtcNow,
                ContentLength = blobItem.Properties.ContentLength ?? 0
            });
        }

        return blobs;
    }

    /// <summary>
    /// Downloads a blob's content as a string
    /// </summary>
    /// <param name="containerName">The container name</param>
    /// <param name="blobName">The blob name</param>
    /// <returns>Blob content as string</returns>
    public async Task<string> DownloadBlobAsStringAsync(string containerName, string blobName)
    {
        var containerClient = _blobServiceClient.GetBlobContainerClient(containerName);
        var blobClient = containerClient.GetBlobClient(blobName);

        var response = await blobClient.DownloadContentAsync();
        return response.Value.Content.ToString();
    }

    /// <summary>
    /// Downloads multiple blobs and returns their contents
    /// </summary>
    /// <param name="containerName">The container name</param>
    /// <param name="blobNames">List of blob names to download</param>
    /// <returns>Dictionary mapping blob name to content</returns>
    public async Task<Dictionary<string, string>> DownloadMultipleBlobsAsync(
        string containerName,
        IEnumerable<string> blobNames)
    {
        var results = new Dictionary<string, string>();

        foreach (var blobName in blobNames)
        {
            try
            {
                var content = await DownloadBlobAsStringAsync(containerName, blobName);
                results[blobName] = content;
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"Error downloading blob {blobName}: {ex.Message}");
            }
        }

        return results;
    }

    /// <summary>
    /// Gets blob metadata without downloading the full content
    /// </summary>
    /// <param name="containerName">The container name</param>
    /// <param name="blobName">The blob name</param>
    /// <returns>Blob properties</returns>
    public async Task<BlobProperties> GetBlobPropertiesAsync(string containerName, string blobName)
    {
        var containerClient = _blobServiceClient.GetBlobContainerClient(containerName);
        var blobClient = containerClient.GetBlobClient(blobName);

        var properties = await blobClient.GetPropertiesAsync();
        return properties.Value;
    }

    /// <summary>
    /// Checks if a blob has been processed by looking at its metadata
    /// </summary>
    /// <param name="containerName">The container name</param>
    /// <param name="blobName">The blob name</param>
    /// <param name="blobLastModified">The blob's LastModified timestamp</param>
    /// <returns>True if the blob should be processed</returns>
    public async Task<bool> ShouldProcessBlobAsync(string containerName, string blobName, DateTimeOffset blobLastModified)
    {
        try
        {
            var properties = await GetBlobPropertiesAsync(containerName, blobName);

            // Check if blob has processing metadata
            if (properties.Metadata.TryGetValue("lastProcessed", out var lastProcessedStr) &&
                properties.Metadata.TryGetValue("processedBlobLastModified", out var processedLastModifiedStr))
            {
                // Parse the timestamps
                if (DateTimeOffset.TryParse(processedLastModifiedStr, out var processedLastModified))
                {
                    // Only reprocess if the blob has been modified since last processing
                    return blobLastModified > processedLastModified;
                }
            }

            // No metadata found - should process
            return true;
        }
        catch
        {
            // If we can't read metadata, assume we should process
            return true;
        }
    }

    /// <summary>
    /// Marks a blob as processed by setting metadata
    /// </summary>
    /// <param name="containerName">The container name</param>
    /// <param name="blobName">The blob name</param>
    /// <param name="blobLastModified">The blob's LastModified timestamp</param>
    /// <param name="recordCount">Number of records processed</param>
    public async Task MarkBlobAsProcessedAsync(string containerName, string blobName, DateTimeOffset blobLastModified, int recordCount)
    {
        try
        {
            var containerClient = _blobServiceClient.GetBlobContainerClient(containerName);
            var blobClient = containerClient.GetBlobClient(blobName);

            var metadata = new Dictionary<string, string>
            {
                ["lastProcessed"] = DateTimeOffset.UtcNow.ToString("O"),
                ["processedBlobLastModified"] = blobLastModified.ToString("O"),
                ["recordCount"] = recordCount.ToString(),
                ["processedBy"] = "AzureFlowLogParser"
            };

            await blobClient.SetMetadataAsync(metadata);
        }
        catch (Exception ex)
        {
            // Log but don't fail if we can't set metadata
            Console.Error.WriteLine($"Warning: Failed to set metadata on blob {blobName}: {ex.Message}");
        }
    }
}

/// <summary>
/// Blob information including metadata
/// </summary>
public class BlobInfo
{
    public string Name { get; set; } = string.Empty;
    public DateTimeOffset LastModified { get; set; }
    public long ContentLength { get; set; }
}
