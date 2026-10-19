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
        Credential = CreateCredential();
        Arm = new ArmClient(Credential);
    }

    public TokenCredential Credential { get; }

    public ArmClient Arm { get; }

    public BlobServiceClient Blob(string storageAccountName)
    {
        return _blobClients.GetOrAdd(storageAccountName, name =>
            new BlobServiceClient(new Uri($"https://{name}.blob.core.windows.net"), Credential));
    }

    /// <summary>
    /// The one Entra ID credential for the app and its tests. Managed identity is tried first, so Azure hosts
    /// need no configuration (AZURE_CLIENT_ID selects a user-assigned identity); a laptop falls through to the
    /// Azure CLI session from `az login`.
    /// </summary>
    public static TokenCredential CreateCredential()
    {
        return new DefaultAzureCredential(CreateCredentialOptions());
    }

    /// <summary>
    /// Options behind <see cref="CreateCredential"/>. Environment, managed identity and the Azure CLI stay enabled.
    /// The Visual Studio and VS Code credentials are excluded: they come before the Azure CLI in the chain and can
    /// return a different signed-in account, so `az login` has to win. The Rust tool uses the same order.
    /// </summary>
    public static DefaultAzureCredentialOptions CreateCredentialOptions()
    {
        return new DefaultAzureCredentialOptions
        {
            ExcludeVisualStudioCredential = true,
            ExcludeVisualStudioCodeCredential = true,
            Retry =
            {
                MaxRetries = 3,
                NetworkTimeout = TimeSpan.FromSeconds(30)
            }
        };
    }
}
