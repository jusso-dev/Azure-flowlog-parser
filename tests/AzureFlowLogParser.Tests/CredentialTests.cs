using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Runtime.Versioning;
using Azure.Core;
using Azure.Identity;
using AzureFlowLogParser.Services.Discovery;
using Xunit;

// These tests set process-wide environment variables and PATH. Other classes read PATH (ReportWriterTests looks for
// qpdf), so the whole assembly runs one test at a time rather than only this class.
[assembly: Xunit.v3.ParallelizationAttribute(Mode = Xunit.Sdk.ParallelMode.None)]

namespace AzureFlowLogParser.Tests;

public class CredentialTests
{
    private const string FakeMsiToken = "fake-msi-token";
    private const string FakeIdentityHeader = "test-identity-header";
    private const string FakeCliToken = "fake-az-token";
    private const string UserAssignedClientId = "11111111-2222-3333-4444-555555555555";
    private const string ManagementScope = "https://management.azure.com/.default";

    [Fact]
    public void Credential_options_keep_managed_identity_and_azure_cli_enabled()
    {
        var options = AzureClients.CreateCredentialOptions();

        Assert.False(options.ExcludeManagedIdentityCredential);
        Assert.False(options.ExcludeAzureCliCredential);
    }

    [Fact]
    public async Task Managed_identity_app_service_endpoint_returns_the_token()
    {
        var (token, request) = await RequestAppServiceTokenAsync(clientId: null);

        Assert.Equal(FakeMsiToken, token.Token);
        Assert.Equal(FakeIdentityHeader, request.IdentityHeader);
        Assert.Contains("resource=https://management.azure.com", Uri.UnescapeDataString(request.Url.Query));
        Assert.DoesNotContain("client_id=", request.Url.Query);
    }

    [Fact]
    public async Task Azure_client_id_selects_the_user_assigned_identity()
    {
        var (_, request) = await RequestAppServiceTokenAsync(UserAssignedClientId);

        Assert.Contains($"client_id={UserAssignedClientId}", Uri.UnescapeDataString(request.Url.Query));
    }

    [Fact]
    [UnsupportedOSPlatform("windows")]
    public async Task Azure_cli_session_returns_its_token_when_managed_identity_is_unset()
    {
        if (OperatingSystem.IsWindows())
        {
            Assert.Skip("The az stub is a POSIX shell script; Windows needs a .cmd stub, which this test does not provide.");
        }

        var binDirectory = TestPaths.NewTempDirectory();
        try
        {
            WriteFakeAzCli(binDirectory);
            using var environment = new EnvironmentScope(new Dictionary<string, string?> { ["PATH"] = binDirectory });

            var token = await CreateHermeticCredential().GetTokenAsync(
                new TokenRequestContext([ManagementScope]), CancellationToken.None);

            Assert.Equal(FakeCliToken, token.Token);
        }
        finally
        {
            Directory.Delete(binDirectory, recursive: true);
        }
    }

    [Fact]
    public async Task Failure_lists_the_sources_tried_and_includes_no_token_text()
    {
        var emptyDirectory = TestPaths.NewTempDirectory();
        try
        {
            using var environment = new EnvironmentScope(new Dictionary<string, string?> { ["PATH"] = emptyDirectory });

            // DefaultAzureCredential throws CredentialUnavailableException, a subclass of AuthenticationFailedException.
            var exception = await Assert.ThrowsAnyAsync<AuthenticationFailedException>(() =>
                CreateHermeticCredential().GetTokenAsync(
                    new TokenRequestContext([ManagementScope]), CancellationToken.None).AsTask());

            Assert.Contains("EnvironmentCredential", exception.Message);
            Assert.Contains("AzureCliCredential", exception.Message);
            Assert.DoesNotContain(FakeCliToken, exception.Message);
            Assert.DoesNotContain("Bearer", exception.Message);
        }
        finally
        {
            Directory.Delete(emptyDirectory, recursive: true);
        }
    }

    // Production options, minus two sources that would make these tests depend on the host. Off Azure the managed
    // identity probe can stall for minutes; on Azure it returns the host's own identity. VS Code returns a signed-in
    // user's cached login. The Azure CLI is then the only source that can answer.
    private static DefaultAzureCredential CreateHermeticCredential()
    {
        var options = AzureClients.CreateCredentialOptions();
        options.ExcludeManagedIdentityCredential = true;
        options.ExcludeVisualStudioCodeCredential = true;
        return new DefaultAzureCredential(options);
    }

    private static async Task<(AccessToken Token, CapturedRequest Request)> RequestAppServiceTokenAsync(string? clientId)
    {
        var port = FreeLoopbackPort();
        using var listener = new HttpListener();
        listener.Prefixes.Add($"http://127.0.0.1:{port}/");
        listener.Start();
        var served = ServeOneTokenAsync(listener);

        using var environment = new EnvironmentScope(new Dictionary<string, string?>
        {
            ["IDENTITY_ENDPOINT"] = $"http://127.0.0.1:{port}/msi/token",
            ["IDENTITY_HEADER"] = FakeIdentityHeader,
            ["AZURE_CLIENT_ID"] = clientId
        });

        var token = await AzureClients.CreateCredential().GetTokenAsync(
            new TokenRequestContext([ManagementScope]), CancellationToken.None);

        return (token, await served);
    }

    // Answers the first request the credential makes, then records the identity header and URL it sent.
    private static Task<CapturedRequest> ServeOneTokenAsync(HttpListener listener)
    {
        return Task.Run(async () =>
        {
            var context = await listener.GetContextAsync();
            // Read the request before the response is written: Windows disposes HttpListenerRequest once the
            // response is closed, and reading headers afterwards throws ObjectDisposedException.
            var captured = new CapturedRequest(context.Request.Headers["X-IDENTITY-HEADER"], context.Request.Url!);

            var expiresOn = DateTimeOffset.UtcNow.AddHours(1).ToUnixTimeSeconds();
            var body = $$"""{"access_token":"{{FakeMsiToken}}","expires_on":"{{expiresOn}}","resource":"https://management.azure.com","token_type":"Bearer"}""";

            context.Response.ContentType = "application/json";
            var bytes = System.Text.Encoding.UTF8.GetBytes(body);
            await context.Response.OutputStream.WriteAsync(bytes);
            context.Response.Close();

            return captured;
        });
    }

    // PATH holds only this directory during the test, so the stub uses the shell's printf builtin, not cat.
    [UnsupportedOSPlatform("windows")]
    private static void WriteFakeAzCli(string binDirectory)
    {
        var expiresOn = DateTimeOffset.UtcNow.AddHours(1);
        var expiresOnLocal = expiresOn.LocalDateTime.ToString("yyyy-MM-dd HH:mm:ss.ffffff", CultureInfo.InvariantCulture);
        var json = $$"""{"accessToken":"{{FakeCliToken}}","expires_on":{{expiresOn.ToUnixTimeSeconds()}},"expiresOn":"{{expiresOnLocal}}","tokenType":"Bearer"}""";

        var path = Path.Combine(binDirectory, "az");
        File.WriteAllText(path, $"#!/bin/sh\nprintf '%s\\n' '{json}'\n");
        File.SetUnixFileMode(path, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
    }

    private static int FreeLoopbackPort()
    {
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        var port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        return port;
    }

    private sealed record CapturedRequest(string? IdentityHeader, Uri Url);

    // Sets environment variables for one test and restores the originals on dispose. A null value unsets the variable.
    // Every variable that could make DefaultAzureCredential pick a different source is cleared first.
    private sealed class EnvironmentScope : IDisposable
    {
        private static readonly string[] CredentialVariables =
        [
            "AZURE_TENANT_ID", "AZURE_CLIENT_ID", "AZURE_CLIENT_SECRET", "AZURE_CLIENT_CERTIFICATE_PATH",
            "AZURE_FEDERATED_TOKEN_FILE", "AZURE_AUTHORITY_HOST", "IDENTITY_ENDPOINT", "IDENTITY_HEADER",
            "MSI_ENDPOINT", "MSI_SECRET", "IMDS_ENDPOINT"
        ];

        private readonly Dictionary<string, string?> _original = new();

        public EnvironmentScope(IReadOnlyDictionary<string, string?> overrides)
        {
            foreach (var name in CredentialVariables.Concat(overrides.Keys).Distinct())
            {
                _original[name] = Environment.GetEnvironmentVariable(name);
                Environment.SetEnvironmentVariable(name, overrides.GetValueOrDefault(name));
            }
        }

        public void Dispose()
        {
            foreach (var (name, value) in _original)
            {
                Environment.SetEnvironmentVariable(name, value);
            }
        }
    }
}
