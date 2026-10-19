//! Legacy root command: parse flow logs to a JSON array or JSON Lines, state tracking through blob metadata, and HTTP
//! posting. Ports Program.cs ProcessFlowLogsAsync and Services/FlowLogHttpClient.cs. Messages, output rules and exit
//! codes follow the reference; the deviations are noted where they occur.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use reqwest::header::{ACCEPT, CONTENT_ENCODING, CONTENT_TYPE};
use reqwest::{Method, StatusCode, Url};

use crate::azure::AzureClient;
use crate::azure::blob;
use crate::azure::http::{RetryPolicy, describe_failure, send_with_retry, transport_failure};
use crate::cli::ParseArgs;
use crate::config;
use crate::flowlog::parse_flow_log;
use crate::model::DenormalizedRecord;

pub async fn run(args: &ParseArgs) -> Result<u8> {
    let verbose = args.verbose;
    let client = AzureClient::new()?;

    let mut endpoint = non_blank(args.http_endpoint.as_deref()).map(str::to_string);
    let mut token = non_blank(args.http_token.as_deref()).map(str::to_string);

    if let Some(vault) = non_blank(args.http_keyvault.as_deref()) {
        if verbose {
            eprintln!("Loading HTTP credentials from Key Vault: {vault}");
        }
        if let Err(error) = load_http_credentials(&client, vault, args, &mut endpoint, &mut token).await {
            eprintln!("Error loading HTTP credentials from Key Vault: {error:#}");
            return Ok(1);
        }
    }

    if args.http_test {
        let Some(url) = endpoint.as_deref() else {
            eprintln!("Error: --http-endpoint or --http-keyvault is required when using --http-test");
            return Ok(1);
        };
        let poster = FlowLogPoster::new(url, token.as_deref(), args.http_compression, args.http_timeout, verbose)?;
        return Ok(if poster.test_connectivity().await { 0 } else { 1 });
    }

    let sources = [
        non_blank(args.storage_account.as_deref()).is_some(),
        args.accounts_file.as_ref().is_some_and(|path| !path.as_os_str().is_empty()),
        non_blank(args.accounts_env.as_deref()).is_some(),
        non_blank(args.accounts_keyvault.as_deref()).is_some(),
    ]
    .iter()
    .filter(|present| **present)
    .count();
    if sources == 0 {
        eprintln!("Error: You must specify a storage account source:");
        eprintln!("  --storage-account <name>           Single storage account");
        eprintln!("  --accounts-file <path>             File with account names");
        eprintln!("  --accounts-env <variable>          Environment variable");
        eprintln!("  --accounts-keyvault <url>          Azure Key Vault");
        return Ok(1);
    }
    if sources > 1 {
        eprintln!("Error: You can only specify one storage account source at a time");
        return Ok(1);
    }

    if verbose {
        eprintln!("Loading storage account configuration...");
    }
    let loaded = if let Some(name) = non_blank(args.storage_account.as_deref()) {
        if verbose {
            eprintln!("Using single storage account: {name}");
        }
        vec![name.to_string()]
    } else if let Some(path) = args.accounts_file.as_deref().filter(|path| !path.as_os_str().is_empty()) {
        let accounts = config::load_from_file(path)?;
        if verbose {
            eprintln!("Loaded {} storage account(s) from file: {}", accounts.len(), path.display());
        }
        accounts
    } else if let Some(name) = non_blank(args.accounts_env.as_deref()) {
        let accounts = config::accounts_from_env_value(name, std::env::var(name).ok())?;
        if verbose {
            eprintln!("Loaded {} storage account(s) from environment variable: {name}", accounts.len());
        }
        accounts
    } else {
        let vault = non_blank(args.accounts_keyvault.as_deref()).unwrap_or_default();
        let Some(secret) = non_blank(args.keyvault_secret.as_deref()) else {
            eprintln!("Error: --keyvault-secret is required when using --accounts-keyvault");
            return Ok(1);
        };
        let accounts = config::load_from_key_vault(&client, vault, secret).await?;
        if verbose {
            eprintln!("Loaded {} storage account(s) from Key Vault: {vault}", accounts.len());
        }
        accounts
    };

    let accounts = config::validate_accounts(loaded)?;

    if verbose {
        eprintln!("Processing {} storage account(s):", accounts.len());
        for account in &accounts {
            eprintln!("  - {account}");
        }
        if args.enable_state_tracking {
            eprintln!("State tracking enabled: Using blob metadata to track processing");
            if args.force_reprocess {
                eprintln!("Force reprocess enabled - ignoring blob metadata state");
            }
        }
    }

    let uploader = match endpoint.as_deref() {
        Some(url) => {
            Some(FlowLogPoster::new(url, token.as_deref(), args.http_compression, args.http_timeout, verbose)?)
        }
        None => None,
    };

    let mut merged = Vec::new();
    for account in &accounts {
        if verbose {
            eprintln!("\n=== Processing storage account: {account} ===");
        }
        match process_and_emit(&client, account, args, uploader.as_ref()).await {
            Ok(records) => merged.extend(records),
            Err(error) => {
                eprintln!("Error processing storage account '{account}': {error:#}");
                if verbose {
                    eprintln!("  Details: {error:?}");
                }
            }
        }
    }

    if args.merge_output && !args.list_only {
        if verbose {
            eprintln!("\nTotal records parsed from all accounts: {}", merged.len());
        }
        if let Some(poster) = &uploader {
            if verbose {
                eprintln!("\n=== Posting to HTTP endpoint ===");
            }
            poster.post_records(&merged).await;
        }
        if output_path(args).is_some() || uploader.is_none() {
            output_records(&merged, &args.format, output_path(args), verbose, None)?;
        }
    }

    Ok(0)
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.trim().is_empty())
}

fn output_path(args: &ParseArgs) -> Option<&Path> {
    args.output.as_deref().filter(|path| !path.as_os_str().is_empty())
}

async fn load_http_credentials(
    client: &AzureClient,
    vault: &str,
    args: &ParseArgs,
    endpoint: &mut Option<String>,
    token: &mut Option<String>,
) -> Result<()> {
    if endpoint.is_none() {
        let values = config::load_from_key_vault(client, vault, &args.http_endpoint_secret).await?;
        *endpoint = values.into_iter().next();
        if args.verbose && endpoint.is_some() {
            eprintln!("  Loaded endpoint from secret '{}'", args.http_endpoint_secret);
        }
    }
    if token.is_none() {
        let values = config::load_from_key_vault(client, vault, &args.http_token_secret).await?;
        *token = values.into_iter().next();
        if args.verbose && token.is_some() {
            eprintln!("  Loaded token from secret '{}'", args.http_token_secret);
        }
    }
    Ok(())
}

/// One account: process it, then either return its records (merge mode) or post and write them for that account.
async fn process_and_emit(
    client: &AzureClient,
    account: &str,
    args: &ParseArgs,
    uploader: Option<&FlowLogPoster>,
) -> Result<Vec<DenormalizedRecord>> {
    let records = process_account(client, account, args).await?;
    if args.merge_output {
        return Ok(records);
    }
    if !args.list_only {
        if let Some(poster) = uploader {
            if args.verbose {
                eprintln!("Posting {} records from {account} to HTTP endpoint...", records.len());
            }
            poster.post_records(&records).await;
        }
        if output_path(args).is_some() || uploader.is_none() {
            let path = output_path(args).map(|output| account_output_path(output, account));
            output_records(&records, &args.format, path.as_deref(), args.verbose, Some(account))?;
        }
    }
    Ok(Vec::new())
}

/// Per-account output file: "<stem>_<account><ext>" in the directory of the requested output. The reference
/// passed the bare file name, which wrote into the working directory; the requested directory is kept here.
pub fn account_output_path(output: &Path, account: &str) -> PathBuf {
    let stem = output.file_stem().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
    let file_name = match output.extension() {
        Some(extension) => format!("{stem}_{account}.{}", extension.to_string_lossy()),
        None => format!("{stem}_{account}"),
    };
    match output.parent() {
        Some(directory) if !directory.as_os_str().is_empty() => directory.join(file_name),
        _ => PathBuf::from(file_name),
    }
}

/// Writes records as a JSON array (indented) or JSON Lines. Nothing is written for an empty list. Stdout gets a
/// trailing newline; files do not, as in the reference.
fn output_records(
    records: &[DenormalizedRecord],
    format: &str,
    path: Option<&Path>,
    verbose: bool,
    account: Option<&str>,
) -> Result<()> {
    if records.is_empty() {
        if verbose {
            eprintln!("No records to output");
        }
        return Ok(());
    }
    let content = if format.eq_ignore_ascii_case("jsonl") {
        records.iter().map(serde_json::to_string).collect::<std::result::Result<Vec<_>, _>>()?.join("\n")
    } else {
        serde_json::to_string_pretty(records)?
    };
    match path {
        Some(path) => {
            std::fs::write(path, &content)?;
            if verbose {
                let info = account.map(|name| format!(" (from {name})")).unwrap_or_default();
                eprintln!("Output written to: {}{info}", path.display());
            }
        }
        None => println!("{content}"),
    }
    Ok(())
}

/// Lists, filters, downloads and parses the blobs of one account (ProcessSingleStorageAccountAsync).
async fn process_account(client: &AzureClient, account: &str, args: &ParseArgs) -> Result<Vec<DenormalizedRecord>> {
    let verbose = args.verbose;
    let container = args.container.as_str();
    let prefix = non_blank(args.prefix.as_deref());
    if verbose {
        eprintln!("Container: {container}");
        if let Some(prefix) = prefix {
            eprintln!("Prefix filter: {prefix}");
        }
        eprintln!("Testing authentication...");
    }

    if let Err(error) = blob::check_access(client, account).await {
        eprintln!("  ✗ Authentication failed for storage account '{account}'");
        eprintln!("    Error: {error:#}");
        print_authentication_help();
        return Err(anyhow!("Authentication failed for storage account '{account}'"));
    }

    if verbose {
        eprintln!("Fetching blob list...");
    }

    let mut blob_times: HashMap<String, DateTime<Utc>> = HashMap::new();
    let mut names: Vec<String> = if args.enable_state_tracking && !args.list_only {
        let listing = blob::list_blobs(client, account, container, prefix).await?;
        if verbose {
            eprintln!("Found {} blob(s)", listing.len());
        }
        let mut selected = Vec::new();
        let mut skipped = 0;
        for item in listing {
            let modified = item.last_modified.unwrap_or_else(Utc::now);
            blob_times.insert(item.name.clone(), modified);
            if args.force_reprocess || should_process(client, account, container, &item.name, modified).await {
                selected.push(item.name);
            } else {
                skipped += 1;
            }
        }
        if verbose && skipped > 0 {
            eprintln!("Skipped {skipped} blob(s) based on metadata state");
        }
        selected
    } else {
        let listing = blob::list_blobs(client, account, container, prefix).await?;
        if verbose {
            eprintln!("Found {} blob(s)", listing.len());
        }
        listing.into_iter().map(|item| item.name).collect()
    };

    if names.is_empty() {
        eprintln!("No blobs to process in storage account '{account}'");
        return Ok(Vec::new());
    }

    if let Some(limit) = args.limit.filter(|limit| *limit > 0) {
        names.truncate(limit);
        if verbose {
            eprintln!("Limited to {} blob(s)", names.len());
        }
    }

    if args.list_only {
        println!("\nAvailable blobs in {account}:");
        for name in &names {
            println!("  - {name}");
        }
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    for name in &names {
        if verbose {
            eprintln!("Processing: {name}");
        }
        match download_and_parse(client, account, container, name).await {
            Ok(parsed) => {
                if verbose {
                    eprintln!("  Parsed {} flow record(s)", parsed.len());
                }
                let count = parsed.len();
                records.extend(parsed);
                if args.enable_state_tracking {
                    if let Some(modified) = blob_times.get(name) {
                        mark_processed(client, account, container, name, *modified, count, verbose).await;
                    }
                }
            }
            Err(error) => {
                eprintln!("Error processing blob {name}: {error:#}");
                if verbose {
                    eprintln!("  Details: {error:?}");
                }
            }
        }
    }

    if verbose {
        eprintln!("Total records from {account}: {}", records.len());
    }
    Ok(records)
}

async fn download_and_parse(
    client: &AzureClient,
    account: &str,
    container: &str,
    name: &str,
) -> Result<Vec<DenormalizedRecord>> {
    let text = blob::download_text(client, account, container, name).await?;
    parse_flow_log(&text)
}

/// A blob is skipped only when its metadata shows it was processed and it has not changed since. Any read failure
/// means process it (ShouldProcessBlobAsync).
async fn should_process(
    client: &AzureClient,
    account: &str,
    container: &str,
    name: &str,
    modified: DateTime<Utc>,
) -> bool {
    let Ok(properties) = blob::blob_properties(client, account, container, name).await else {
        return true;
    };
    let (Some(_), Some(processed)) =
        (properties.metadata.get("lastprocessed"), properties.metadata.get("processedbloblastmodified"))
    else {
        return true;
    };
    match DateTime::parse_from_rfc3339(processed) {
        Ok(processed) => modified > processed.with_timezone(&Utc),
        Err(_) => true,
    }
}

/// Writes the four state keys the reference writes. A failure is a warning, not an error.
async fn mark_processed(
    client: &AzureClient,
    account: &str,
    container: &str,
    name: &str,
    modified: DateTime<Utc>,
    count: usize,
    verbose: bool,
) {
    let metadata = [
        ("lastProcessed", iso_o(Utc::now())),
        ("processedBlobLastModified", iso_o(modified)),
        ("recordCount", count.to_string()),
        ("processedBy", "AzureFlowLogParser".to_string()),
    ];
    match blob::set_metadata(client, account, container, name, &metadata).await {
        Ok(()) => {
            if verbose {
                eprintln!("  Marked as processed in blob metadata");
            }
        }
        Err(error) => eprintln!("Warning: Failed to set metadata on blob {name}: {error:#}"),
    }
}

/// Round-trip "O" format of .NET DateTimeOffset: seven fractional digits (100 ns ticks) and an explicit offset.
fn iso_o(value: DateTime<Utc>) -> String {
    let ticks = value.timestamp_subsec_nanos() / 100;
    format!("{}.{ticks:07}+00:00", value.format("%Y-%m-%dT%H:%M:%S"))
}

fn print_authentication_help() {
    eprintln!();
    eprintln!("Authentication Troubleshooting:");
    eprintln!(
        "  Local development: run 'az login', then 'az account show', and check the Storage Blob Data Reader role."
    );
    eprintln!("  Azure resource: enable a managed identity and grant it Storage Blob Data Reader on the account.");
    eprintln!(
        "  Service principal: set AZURE_TENANT_ID, AZURE_CLIENT_ID and AZURE_CLIENT_SECRET (values are not printed)."
    );
    eprintln!();
}

/// Endpoint for log lines: scheme, host, port and path only. Query strings and user info can carry secrets.
pub fn redacted_endpoint(endpoint: &str) -> String {
    match Url::parse(endpoint) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "<invalid endpoint>".to_string(),
    }
}

fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

/// Posts flow log records to an HTTP endpoint one record at a time (FlowLogHttpClient.cs). Gzip request bodies and
/// bearer auth are optional. Transient failures are retried with the shared policy.
pub struct FlowLogPoster {
    http: reqwest::Client,
    endpoint: String,
    label: String,
    token: Option<String>,
    compression: bool,
    retry: RetryPolicy,
    verbose: bool,
}

impl FlowLogPoster {
    pub fn new(
        endpoint: &str,
        token: Option<&str>,
        compression: bool,
        timeout_seconds: u64,
        verbose: bool,
    ) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(timeout_seconds)).build()?;
        let retry = RetryPolicy::default();
        if verbose && retry.max_retries > 0 {
            eprintln!("HTTP client configured with {} retry attempts using exponential backoff", retry.max_retries);
        }
        Ok(Self {
            http,
            endpoint: endpoint.to_string(),
            label: redacted_endpoint(endpoint),
            token: token.map(str::to_string),
            compression,
            retry,
            verbose,
        })
    }

    /// Posts every record. Returns true when all posts succeeded. Per-record failures are reported and skipped.
    pub async fn post_records(&self, records: &[DenormalizedRecord]) -> bool {
        if records.is_empty() {
            if self.verbose {
                eprintln!("No records to post");
            }
            return true;
        }
        if self.verbose {
            eprintln!("Posting {} record(s) individually to: {}", records.len(), self.label);
        }
        let mut successes = 0;
        let mut failures = 0;
        for (index, record) in records.iter().enumerate() {
            if self.verbose {
                eprintln!("  [{}/{}] Posting record...", index + 1, records.len());
            }
            match self.post_one(record).await {
                Ok(true) => successes += 1,
                Ok(false) => failures += 1,
                Err(error) => {
                    failures += 1;
                    eprintln!("  Error posting record {}: {error:#}", index + 1);
                }
            }
        }
        if self.verbose {
            eprintln!("Posting complete: {successes} successful, {failures} failed");
        }
        failures == 0
    }

    async fn post_one(&self, record: &DenormalizedRecord) -> Result<bool> {
        let json = serde_json::to_vec(record)?;
        let (body, encoding) = if self.compression {
            let compressed = gzip(&json)?;
            if self.verbose {
                eprintln!(
                    "    Uncompressed: {} bytes, Compressed: {} bytes ({:.1}%)",
                    json.len(),
                    compressed.len(),
                    compressed.len() as f64 / json.len() as f64 * 100.0
                );
            }
            (compressed, Some("gzip"))
        } else {
            if self.verbose {
                eprintln!("    Size: {} bytes", json.len());
            }
            (json, None)
        };

        let response = send_with_retry(self.retry, || {
            let mut request = self
                .http
                .post(self.endpoint.as_str())
                .header(ACCEPT, "application/json")
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
            if let Some(encoding) = encoding {
                request = request.header(CONTENT_ENCODING, encoding);
            }
            if let Some(token) = &self.token {
                request = request.bearer_auth(token);
            }
            request
        })
        .await
        .map_err(|error| anyhow!("request {}", describe_failure(transport_failure(&error))))?;

        let status = response.status();
        if status.is_success() {
            if self.verbose {
                eprintln!("    ✓ Successfully posted (Status: {} {})", status.as_u16(), reason(status));
            }
            return Ok(true);
        }
        let text = response.text().await.unwrap_or_default();
        eprintln!("    ✗ Failed to post (Status: {} {})", status.as_u16(), reason(status));
        if self.verbose && !text.trim().is_empty() {
            eprintln!("    Response: {text}");
        }
        Ok(false)
    }

    /// OPTIONS against the endpoint. 2xx or 405 means reachable (FlowLogHttpClient.TestConnectivityAsync).
    pub async fn test_connectivity(&self) -> bool {
        if self.verbose {
            eprintln!("Testing connectivity to: {}", self.label);
        }
        let result = send_with_retry(self.retry, || {
            let mut request = self.http.request(Method::OPTIONS, self.endpoint.as_str());
            if let Some(token) = &self.token {
                request = request.bearer_auth(token);
            }
            request
        })
        .await;
        match result {
            Ok(response) => {
                let status = response.status();
                if self.verbose {
                    eprintln!("  Response: {} {}", status.as_u16(), reason(status));
                }
                status.is_success() || status == StatusCode::METHOD_NOT_ALLOWED
            }
            Err(error) => {
                eprintln!("Connectivity test failed: {}", describe_failure(transport_failure(&error)));
                false
            }
        }
    }
}

fn reason(status: StatusCode) -> &'static str {
    status.canonical_reason().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_merge_output_name_keeps_directory_and_inserts_account() {
        assert_eq!(account_output_path(Path::new("out/flows.json"), "acct1"), PathBuf::from("out/flows_acct1.json"));
        assert_eq!(account_output_path(Path::new("flows.jsonl"), "acct1"), PathBuf::from("flows_acct1.jsonl"));
        assert_eq!(account_output_path(Path::new("flows"), "acct1"), PathBuf::from("flows_acct1"));
    }

    #[test]
    fn metadata_timestamps_use_seven_digit_round_trip_format() {
        let value = DateTime::parse_from_rfc3339("2026-10-19T10:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(iso_o(value), "2026-10-19T10:00:00.0000000+00:00");
        let parsed = DateTime::parse_from_rfc3339(&iso_o(value)).unwrap().with_timezone(&Utc);
        assert_eq!(parsed, value);
    }

    #[test]
    fn endpoint_label_drops_query_and_user_info() {
        assert_eq!(
            redacted_endpoint("https://user:pw@ingest.example.com:8443/v1/logs?sig=abc#frag"),
            "https://ingest.example.com:8443/v1/logs"
        );
        assert_eq!(redacted_endpoint("not a url"), "<invalid endpoint>");
    }

    #[test]
    fn gzip_output_round_trips() {
        use std::io::Read;
        let data = br#"{"time":"2026-10-19T10:00:00Z"}"#;
        let compressed = gzip(data).unwrap();
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(compressed.as_slice()).read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn non_blank_treats_whitespace_as_absent() {
        assert_eq!(non_blank(Some("  ")), None);
        assert_eq!(non_blank(Some("x")), Some("x"));
        assert_eq!(non_blank(None), None);
    }
}
