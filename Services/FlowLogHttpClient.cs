using System.IO.Compression;
using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using AzureFlowLogParser.Models;
using Polly;
using Polly.Retry;

namespace AzureFlowLogParser.Services;

/// <summary>
/// HTTP client service for posting denormalized flow logs to an HTTP endpoint
/// Matches the pattern from PaloAlto Cortex Azure Functions reference implementation
/// </summary>
public class FlowLogHttpClient : IDisposable
{
    private readonly HttpClient _httpClient;
    private readonly string _endpoint;
    private readonly string? _bearerToken;
    private readonly bool _useCompression;
    private readonly int _maxBatchSize;
    private readonly ResiliencePipeline<HttpResponseMessage> _retryPipeline;
    private readonly bool _verbose;

    /// <summary>
    /// Initializes the HTTP client for posting flow logs
    /// </summary>
    /// <param name="endpoint">HTTP endpoint URL to post flow logs to</param>
    /// <param name="bearerToken">Optional Bearer token for authentication</param>
    /// <param name="useCompression">Enable gzip compression (default: true)</param>
    /// <param name="maxBatchSize">Maximum number of records per batch (default: 1000)</param>
    /// <param name="timeoutSeconds">HTTP timeout in seconds (default: 300)</param>
    /// <param name="maxRetries">Maximum number of retry attempts (default: 3)</param>
    /// <param name="verbose">Enable verbose logging (default: false)</param>
    public FlowLogHttpClient(
        string endpoint,
        string? bearerToken = null,
        bool useCompression = true,
        int maxBatchSize = 1000,
        int timeoutSeconds = 300,
        int maxRetries = 3,
        bool verbose = false)
    {
        if (string.IsNullOrWhiteSpace(endpoint))
        {
            throw new ArgumentException("Endpoint URL cannot be empty", nameof(endpoint));
        }

        _endpoint = endpoint;
        _bearerToken = bearerToken;
        _useCompression = useCompression;
        _maxBatchSize = maxBatchSize;
        _verbose = verbose;

        _httpClient = new HttpClient
        {
            Timeout = TimeSpan.FromSeconds(timeoutSeconds)
        };

        // Set default headers
        _httpClient.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/json"));

        if (!string.IsNullOrWhiteSpace(_bearerToken))
        {
            _httpClient.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", _bearerToken);
        }

        // Configure Polly retry policy with exponential backoff
        _retryPipeline = new ResiliencePipelineBuilder<HttpResponseMessage>()
            .AddRetry(new RetryStrategyOptions<HttpResponseMessage>
            {
                MaxRetryAttempts = maxRetries,
                Delay = TimeSpan.FromSeconds(1),
                BackoffType = DelayBackoffType.Exponential,
                UseJitter = true,
                ShouldHandle = new PredicateBuilder<HttpResponseMessage>()
                    .Handle<HttpRequestException>()
                    .Handle<TaskCanceledException>()
                    .HandleResult(response =>
                    {
                        // Retry on transient HTTP errors
                        var statusCode = (int)response.StatusCode;
                        return statusCode >= 500 || // Server errors
                               response.StatusCode == HttpStatusCode.RequestTimeout || // 408
                               response.StatusCode == HttpStatusCode.TooManyRequests; // 429
                    }),
                OnRetry = args =>
                {
                    var statusCode = args.Outcome.Result?.StatusCode.ToString() ?? "N/A";
                    var exception = args.Outcome.Exception?.Message ?? "N/A";

                    if (verbose)
                    {
                        Console.Error.WriteLine($"  Retry attempt {args.AttemptNumber} after delay of {args.RetryDelay.TotalSeconds:F1}s");
                        Console.Error.WriteLine($"    Reason: Status={statusCode}, Exception={exception}");
                    }

                    return ValueTask.CompletedTask;
                }
            })
            .Build();

        if (verbose && maxRetries > 0)
        {
            Console.Error.WriteLine($"HTTP client configured with {maxRetries} retry attempts using exponential backoff");
        }
    }

    /// <summary>
    /// Posts flow log records to the configured HTTP endpoint
    /// </summary>
    /// <param name="records">List of denormalized flow records to post</param>
    /// <param name="verbose">Enable verbose output</param>
    /// <returns>True if successful, false otherwise</returns>
    public async Task<bool> PostFlowLogsAsync(List<DenormalizedFlowRecord> records, bool verbose = false)
    {
        if (records == null || records.Count == 0)
        {
            if (verbose)
                Console.Error.WriteLine("No records to post");
            return true;
        }

        try
        {
            // Split records into batches if needed
            var batches = SplitIntoBatches(records, _maxBatchSize);

            if (verbose)
                Console.Error.WriteLine($"Posting {records.Count} record(s) in {batches.Count} batch(es) to: {_endpoint}");

            int successCount = 0;
            int failureCount = 0;

            foreach (var batch in batches)
            {
                try
                {
                    var success = await PostBatchAsync(batch, verbose);
                    if (success)
                        successCount++;
                    else
                        failureCount++;
                }
                catch (Exception ex)
                {
                    failureCount++;
                    Console.Error.WriteLine($"Error posting batch: {ex.Message}");
                    if (verbose)
                        Console.Error.WriteLine($"  Stack trace: {ex.StackTrace}");
                }
            }

            if (verbose)
            {
                Console.Error.WriteLine($"Posting complete: {successCount} successful, {failureCount} failed");
            }

            return failureCount == 0;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Error posting flow logs: {ex.Message}");
            if (verbose)
                Console.Error.WriteLine($"  Stack trace: {ex.StackTrace}");
            return false;
        }
    }

    /// <summary>
    /// Posts a single batch of records to the endpoint
    /// </summary>
    private async Task<bool> PostBatchAsync(List<DenormalizedFlowRecord> batch, bool verbose)
    {
        // Serialize records to JSON
        var jsonOptions = new JsonSerializerOptions
        {
            DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull
        };

        var jsonContent = JsonSerializer.Serialize(batch, jsonOptions);
        var contentBytes = Encoding.UTF8.GetBytes(jsonContent);

        HttpContent httpContent;

        if (_useCompression)
        {
            // Compress with gzip (matches reference implementation)
            using var memoryStream = new MemoryStream();
            using (var gzipStream = new GZipStream(memoryStream, CompressionMode.Compress, leaveOpen: true))
            {
                await gzipStream.WriteAsync(contentBytes);
            }

            var compressedBytes = memoryStream.ToArray();
            httpContent = new ByteArrayContent(compressedBytes);
            httpContent.Headers.ContentType = new MediaTypeHeaderValue("application/json");
            httpContent.Headers.ContentEncoding.Add("gzip");

            if (verbose)
            {
                Console.Error.WriteLine($"  Batch size: {batch.Count} records, " +
                    $"Uncompressed: {contentBytes.Length} bytes, " +
                    $"Compressed: {compressedBytes.Length} bytes " +
                    $"({(double)compressedBytes.Length / contentBytes.Length * 100:F1}%)");
            }
        }
        else
        {
            httpContent = new StringContent(jsonContent, Encoding.UTF8, "application/json");

            if (verbose)
            {
                Console.Error.WriteLine($"  Batch size: {batch.Count} records, Size: {contentBytes.Length} bytes");
            }
        }

        // Post to endpoint with retry policy
        var response = await _retryPipeline.ExecuteAsync(
            async cancellationToken => await _httpClient.PostAsync(_endpoint, httpContent, cancellationToken),
            CancellationToken.None);

        if (response.IsSuccessStatusCode)
        {
            if (verbose)
            {
                Console.Error.WriteLine($"  ✓ Successfully posted batch (Status: {(int)response.StatusCode} {response.StatusCode})");
            }
            return true;
        }
        else
        {
            var errorContent = await response.Content.ReadAsStringAsync();
            Console.Error.WriteLine($"  ✗ Failed to post batch (Status: {(int)response.StatusCode} {response.StatusCode})");

            if (verbose && !string.IsNullOrWhiteSpace(errorContent))
            {
                Console.Error.WriteLine($"  Response: {errorContent}");
            }

            return false;
        }
    }

    /// <summary>
    /// Splits records into batches
    /// </summary>
    private List<List<DenormalizedFlowRecord>> SplitIntoBatches(List<DenormalizedFlowRecord> records, int batchSize)
    {
        var batches = new List<List<DenormalizedFlowRecord>>();

        for (int i = 0; i < records.Count; i += batchSize)
        {
            var batch = records.Skip(i).Take(batchSize).ToList();
            batches.Add(batch);
        }

        return batches;
    }

    /// <summary>
    /// Tests connectivity to the endpoint
    /// </summary>
    public async Task<bool> TestConnectivityAsync(bool verbose = false)
    {
        try
        {
            if (verbose)
                Console.Error.WriteLine($"Testing connectivity to: {_endpoint}");

            // Try a HEAD or OPTIONS request first (with retry policy)
            var request = new HttpRequestMessage(HttpMethod.Options, _endpoint);
            var response = await _retryPipeline.ExecuteAsync(
                async cancellationToken => await _httpClient.SendAsync(request, cancellationToken),
                CancellationToken.None);

            if (verbose)
            {
                Console.Error.WriteLine($"  Response: {(int)response.StatusCode} {response.StatusCode}");
            }

            return response.IsSuccessStatusCode || response.StatusCode == System.Net.HttpStatusCode.MethodNotAllowed;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Connectivity test failed: {ex.Message}");
            return false;
        }
    }

    public void Dispose()
    {
        _httpClient?.Dispose();
    }
}
