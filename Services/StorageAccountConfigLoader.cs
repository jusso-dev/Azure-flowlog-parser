using Azure.Identity;
using Azure.Security.KeyVault.Secrets;

namespace AzureFlowLogParser.Services;

/// <summary>
/// Loads storage account configurations from various sources
/// </summary>
public class StorageAccountConfigLoader
{
    /// <summary>
    /// Loads storage accounts from a file (one account per line or comma-separated)
    /// </summary>
    /// <param name="filePath">Path to the file containing storage account names</param>
    /// <returns>List of storage account names</returns>
    public static async Task<List<string>> LoadFromFileAsync(string filePath)
    {
        if (!File.Exists(filePath))
        {
            throw new FileNotFoundException($"Storage accounts file not found: {filePath}");
        }

        var content = await File.ReadAllTextAsync(filePath);
        return ParseStorageAccounts(content);
    }

    /// <summary>
    /// Loads storage accounts from an environment variable (comma-separated)
    /// </summary>
    /// <param name="variableName">Name of the environment variable</param>
    /// <returns>List of storage account names</returns>
    public static List<string> LoadFromEnvironmentVariable(string variableName)
    {
        var value = Environment.GetEnvironmentVariable(variableName);

        if (string.IsNullOrWhiteSpace(value))
        {
            throw new InvalidOperationException($"Environment variable '{variableName}' is not set or is empty");
        }

        return ParseStorageAccounts(value);
    }

    /// <summary>
    /// Loads storage accounts from Azure Key Vault secret (comma-separated)
    /// </summary>
    /// <param name="keyVaultUrl">Key Vault URL (e.g., https://myvault.vault.azure.net/)</param>
    /// <param name="secretName">Name of the secret containing storage account names</param>
    /// <returns>List of storage account names</returns>
    public static async Task<List<string>> LoadFromKeyVaultAsync(string keyVaultUrl, string secretName)
    {
        try
        {
            var keyVaultUri = new Uri(keyVaultUrl);
            var credential = new DefaultAzureCredential();
            var client = new SecretClient(keyVaultUri, credential);

            var secret = await client.GetSecretAsync(secretName);

            if (string.IsNullOrWhiteSpace(secret.Value.Value))
            {
                throw new InvalidOperationException($"Key Vault secret '{secretName}' is empty");
            }

            return ParseStorageAccounts(secret.Value.Value);
        }
        catch (Azure.RequestFailedException ex)
        {
            throw new InvalidOperationException(
                $"Failed to retrieve secret '{secretName}' from Key Vault '{keyVaultUrl}': {ex.Message}", ex);
        }
    }

    /// <summary>
    /// Parses a string containing storage account names
    /// Supports comma-separated values and newline-separated values
    /// Lines starting with # are treated as comments and ignored
    /// </summary>
    /// <param name="content">Content containing storage account names</param>
    /// <returns>List of storage account names</returns>
    private static List<string> ParseStorageAccounts(string content)
    {
        // Split by comma and newline, trim whitespace, filter comments and empty entries
        var accounts = content
            .Split(new[] { ',', '\n', '\r' }, StringSplitOptions.RemoveEmptyEntries)
            .Select(s => s.Trim())
            .Where(s => !string.IsNullOrWhiteSpace(s) && !s.StartsWith("#"))
            .Distinct()
            .ToList();

        if (accounts.Count == 0)
        {
            throw new InvalidOperationException("No storage accounts found in the provided content");
        }

        return accounts;
    }

    /// <summary>
    /// Validates that storage account names are in the correct format
    /// </summary>
    /// <param name="storageAccounts">List of storage account names to validate</param>
    /// <returns>List of valid storage account names</returns>
    public static List<string> ValidateStorageAccounts(List<string> storageAccounts)
    {
        var validAccounts = new List<string>();
        var invalidAccounts = new List<string>();

        foreach (var account in storageAccounts)
        {
            // Azure storage account name rules:
            // - 3-24 characters
            // - lowercase letters and numbers only
            if (account.Length >= 3 &&
                account.Length <= 24 &&
                account.All(c => char.IsLower(c) || char.IsDigit(c)))
            {
                validAccounts.Add(account);
            }
            else
            {
                invalidAccounts.Add(account);
            }
        }

        if (invalidAccounts.Any())
        {
            Console.Error.WriteLine($"Warning: Skipping {invalidAccounts.Count} invalid storage account name(s):");
            foreach (var invalid in invalidAccounts)
            {
                Console.Error.WriteLine($"  - '{invalid}' (must be 3-24 lowercase letters/numbers)");
            }
        }

        if (validAccounts.Count == 0)
        {
            throw new InvalidOperationException("No valid storage account names found");
        }

        return validAccounts;
    }
}
