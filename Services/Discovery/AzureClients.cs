using System.Collections.Concurrent;
using Azure.Core;
using Azure.Identity;
using Azure.ResourceManager;
using Azure.Storage.Blobs;

namespace AzureFlowLogParser.Services.Discovery;

/// <summary>
/// Shared Entra ID credential with ARM and per-account Blob clients.
/// </summary>
public sealed class AzureClients
{
    private readonly ConcurrentDictionary<string, BlobServiceClient> _blobClients = new(StringComparer.OrdinalIgnoreCase);

    public AzureClients()
    {
        Credential = new DefaultAzureCredential(new DefaultAzureCredentialOptions
        {
            Retry =
            {
                MaxRetries = 3,
                NetworkTimeout = TimeSpan.FromSeconds(30)
            }
        });
        Arm = new ArmClient(Credential);
    }

    public TokenCredential Credential { get; }

    public ArmClient Arm { get; }

    public BlobServiceClient Blob(string storageAccountName)
    {
        return _blobClients.GetOrAdd(storageAccountName, name =>
            new BlobServiceClient(new Uri($"https://{name}.blob.core.windows.net"), Credential));
    }
}
