using Azure.Identity;
using Azure.Storage.Blobs;
using Azure.Storage.Blobs.Models;

namespace AzureFlowLogParser.Services;

/// <summary>
/// Azure Storage client that uses Managed Service Identity (MSI) for authentication
/// </summary>
public class AzureStorageClient
{
    private readonly BlobServiceClient _blobServiceClient;

    /// <summary>
    /// Initializes the Azure Storage client with MSI authentication
    /// </summary>
    /// <param name="storageAccountName">The name of the storage account</param>
    public AzureStorageClient(string storageAccountName)
    {
        var storageUri = new Uri($"https://{storageAccountName}.blob.core.windows.net");

        // Use DefaultAzureCredential which supports:
        // - Managed Identity (MSI) in Azure
        // - Azure CLI credentials for local development
        // - Environment variables
        // - Visual Studio/VS Code credentials
        var credential = new DefaultAzureCredential();

        _blobServiceClient = new BlobServiceClient(storageUri, credential);
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
}
