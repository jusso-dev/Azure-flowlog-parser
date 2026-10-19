//! Blob service REST calls: container existence, List Blobs (XML with NextMarker), Get Blob Properties, Get Blob,
//! Set Blob Metadata, and a List Containers access check.
//!
//! Versions and doc URLs (verified on Microsoft Learn):
//! - x-ms-version 2026-04-06, the latest fully deployed version per https://learn.microsoft.com/en-us/rest/api/storageservices/versioning-for-the-azure-storage-services
//! - List Blobs: https://learn.microsoft.com/en-us/rest/api/storageservices/list-blobs
//! - Get Blob Properties (HEAD, x-ms-meta-* headers): https://learn.microsoft.com/en-us/rest/api/storageservices/get-blob-properties
//! - Set Blob Metadata (PUT comp=metadata, replaces all metadata): https://learn.microsoft.com/en-us/rest/api/storageservices/set-blob-metadata
//! - Get Blob: https://learn.microsoft.com/en-us/rest/api/storageservices/get-blob
//! - Get Container Properties (restype=container, 200 when present): https://learn.microsoft.com/en-us/rest/api/storageservices/get-container-properties
//! - List Containers (access check, not fetched separately; standard comp=list form on the account endpoint)

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, LAST_MODIFIED};
use reqwest::{RequestBuilder, StatusCode, Url};
use serde::Deserialize;

use super::auth::RESOURCE_STORAGE;
use super::client::AzureClient;
use super::http::{AzureHttpError, azure_error, describe_failure, ensure_success, send_with_retry, transport_failure};

pub const STORAGE_VERSION: &str = "2026-04-06";

#[derive(Debug, Clone, PartialEq)]
pub struct BlobItem {
    pub name: String,
    pub last_modified: Option<DateTime<Utc>>,
    pub content_length: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlobListing {
    pub blobs: Vec<BlobItem>,
    /// Empty NextMarker means the listing is complete. Otherwise request the next page with this marker.
    pub next_marker: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlobProperties {
    pub last_modified: Option<DateTime<Utc>>,
    /// Metadata with lower-cased names (header names are case-insensitive, so lookups use lower case).
    pub metadata: HashMap<String, String>,
}

// ---------- URLs ----------

fn account_url(account: &str) -> Result<Url> {
    Url::parse(&format!("https://{account}.blob.core.windows.net/")).context("invalid storage account name")
}

fn container_url(account: &str, container: &str) -> Result<Url> {
    let mut url = account_url(account)?;
    url.path_segments_mut().map_err(|_| anyhow!("invalid storage account URL"))?.pop_if_empty().push(container);
    Ok(url)
}

/// Blob URL with each name segment percent-encoded. Names contain "/" as a separator, so split on it first.
fn blob_url(account: &str, container: &str, blob: &str) -> Result<Url> {
    let mut url = container_url(account, container)?;
    url.path_segments_mut().map_err(|_| anyhow!("invalid storage account URL"))?.extend(blob.split('/'));
    Ok(url)
}

fn list_url(
    account: &str,
    container: &str,
    prefix: Option<&str>,
    marker: Option<&str>,
    max_results: Option<u32>,
) -> Result<Url> {
    let mut url = container_url(account, container)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("restype", "container").append_pair("comp", "list");
        if let Some(prefix) = prefix.filter(|value| !value.is_empty()) {
            query.append_pair("prefix", prefix);
        }
        if let Some(marker) = marker.filter(|value| !value.is_empty()) {
            query.append_pair("marker", marker);
        }
        if let Some(max) = max_results {
            query.append_pair("maxresults", &max.to_string());
        }
    }
    Ok(url)
}

// ---------- Requests ----------

fn http_date(now: DateTime<Utc>) -> String {
    now.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn storage_headers(request: RequestBuilder, token: &str) -> RequestBuilder {
    request.bearer_auth(token).header("x-ms-version", STORAGE_VERSION).header("x-ms-date", http_date(Utc::now()))
}

/// Sends a storage request with a fresh storage token and the retry policy. Token failures are returned as is.
async fn send<F>(client: &AzureClient, build: F) -> Result<reqwest::Response>
where
    F: Fn() -> RequestBuilder,
{
    let token = client.token(RESOURCE_STORAGE).await?;
    send_with_retry(client.retry(), || storage_headers(build(), &token))
        .await
        .map_err(|error| anyhow!("Blob request {}", describe_failure(transport_failure(&error))))
}

async fn get_text(client: &AzureClient, url: Url) -> Result<String> {
    let http = client.http();
    let response = ensure_success(send(client, || http.get(url.clone())).await?).await?;
    response.text().await.context("could not read blob service response")
}

// ---------- List Blobs ----------

#[derive(Debug, Deserialize)]
#[serde(rename = "EnumerationResults")]
struct ListResultsXml {
    #[serde(rename = "Blobs", default)]
    blobs: BlobsXml,
    #[serde(rename = "NextMarker", default)]
    next_marker: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BlobsXml {
    #[serde(rename = "Blob", default)]
    items: Vec<BlobXml>,
}

#[derive(Debug, Deserialize)]
struct BlobXml {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties", default)]
    properties: BlobPropertiesXml,
}

#[derive(Debug, Default, Deserialize)]
struct BlobPropertiesXml {
    #[serde(rename = "Last-Modified", default)]
    last_modified: Option<String>,
    #[serde(rename = "Content-Length", default)]
    content_length: Option<u64>,
}

/// Parses a List Blobs response. An empty NextMarker element is treated as no marker.
pub fn parse_list_xml(xml: &str) -> Result<BlobListing> {
    let parsed: ListResultsXml = quick_xml::de::from_str(xml).context("unexpected List Blobs response")?;
    let blobs = parsed
        .blobs
        .items
        .into_iter()
        .map(|blob| BlobItem {
            name: blob.name,
            last_modified: blob.properties.last_modified.as_deref().and_then(parse_last_modified),
            content_length: blob.properties.content_length.unwrap_or(0),
        })
        .collect();
    Ok(BlobListing { blobs, next_marker: parsed.next_marker.filter(|marker| !marker.is_empty()) })
}

/// RFC 1123 (the form storage uses in headers and XML), with ISO 8601 as a fallback.
pub fn parse_last_modified(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(text)
        .or_else(|_| DateTime::parse_from_rfc3339(text))
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

/// One page of blobs under `prefix`. `max_results` limits the page size; the probe for unmanaged accounts uses 1.
pub async fn list_page(
    client: &AzureClient,
    account: &str,
    container: &str,
    prefix: Option<&str>,
    marker: Option<&str>,
    max_results: Option<u32>,
) -> Result<BlobListing> {
    let url = list_url(account, container, prefix, marker, max_results)?;
    parse_list_xml(&get_text(client, url).await?)
}

/// Every blob under `prefix`, following NextMarker. A repeated marker is an error.
pub async fn list_blobs(
    client: &AzureClient,
    account: &str,
    container: &str,
    prefix: Option<&str>,
) -> Result<Vec<BlobItem>> {
    let mut all = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let listing = list_page(client, account, container, prefix, marker.as_deref(), None).await?;
        all.extend(listing.blobs);
        match listing.next_marker {
            Some(next) if Some(&next) == marker.as_ref() => bail!("List Blobs repeated a marker"),
            Some(next) => marker = Some(next),
            None => return Ok(all),
        }
    }
}

// ---------- Container and blob calls ----------

/// Container existence by HEAD: 200 is present, 404 is absent, anything else is an error (403 is access denied).
pub async fn container_exists(client: &AzureClient, account: &str, container: &str) -> Result<bool> {
    let mut url = container_url(account, container)?;
    url.query_pairs_mut().append_pair("restype", "container");
    let http = client.http();
    let response = send(client, || http.head(url.clone())).await?;
    if response.status().is_success() {
        return Ok(true);
    }
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(false);
    }
    Err(anyhow::Error::new(azure_error(response).await))
}

/// Properties of one blob (HEAD). Metadata comes from x-ms-meta-* headers.
pub async fn blob_properties(
    client: &AzureClient,
    account: &str,
    container: &str,
    blob: &str,
) -> Result<BlobProperties> {
    let url = blob_url(account, container, blob)?;
    let http = client.http();
    let response = ensure_success(send(client, || http.head(url.clone())).await?).await?;
    Ok(BlobProperties {
        last_modified: response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_last_modified),
        metadata: metadata_from_headers(response.headers()),
    })
}

/// Lower-cased metadata names from `x-ms-meta-*` headers. Values that are not visible ASCII are skipped.
pub fn metadata_from_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let key = name.as_str().strip_prefix("x-ms-meta-")?;
            Some((key.to_ascii_lowercase(), value.to_str().ok()?.to_string()))
        })
        .collect()
}

/// Blob content as UTF-8 text (Get Blob).
pub async fn download_text(client: &AzureClient, account: &str, container: &str, blob: &str) -> Result<String> {
    let url = blob_url(account, container, blob)?;
    let http = client.http();
    let response = ensure_success(send(client, || http.get(url.clone())).await?).await?;
    let bytes = response.bytes().await.context("could not read blob content")?;
    String::from_utf8(bytes.to_vec()).context("blob content is not UTF-8")
}

/// Replaces all metadata on a blob (Set Blob Metadata). Names are sent as x-ms-meta-<name> headers.
pub async fn set_metadata(
    client: &AzureClient,
    account: &str,
    container: &str,
    blob: &str,
    metadata: &[(&str, String)],
) -> Result<()> {
    let mut url = blob_url(account, container, blob)?;
    url.query_pairs_mut().append_pair("comp", "metadata");
    let http = client.http();
    let response = send(client, || {
        let mut request = http.put(url.clone());
        for (name, value) in metadata {
            request = request.header(format!("x-ms-meta-{name}"), value.as_str());
        }
        request
    })
    .await?;
    ensure_success(response).await.map(|_| ())
}

/// Access check used before processing an account: List Containers with one result.
pub async fn check_access(client: &AzureClient, account: &str) -> Result<()> {
    let mut url = account_url(account)?;
    url.query_pairs_mut().append_pair("comp", "list").append_pair("maxresults", "1");
    get_text(client, url).await.map(|_| ())
}

/// Maps an HTTP error from a blob call to a message the caller can show. Kept as an `AzureHttpError` in anyhow.
pub fn is_access_denied(error: &anyhow::Error) -> bool {
    error.downcast_ref::<AzureHttpError>().is_some_and(AzureHttpError::is_access_denied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    const LIST_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<EnumerationResults ServiceEndpoint="https://acct.blob.core.windows.net/" ContainerName="insights-logs-flowlogflowevent">
  <Prefix>resourceId=/</Prefix>
  <MaxResults>2</MaxResults>
  <Blobs>
    <Blob>
      <Name>resourceId=/SUBSCRIPTIONS/A/PT1H.json</Name>
      <Properties>
        <Creation-Time>Sun, 18 Oct 2026 10:00:00 GMT</Creation-Time>
        <Last-Modified>Sun, 18 Oct 2026 11:05:00 GMT</Last-Modified>
        <Content-Length>2048</Content-Length>
        <BlobType>BlockBlob</BlobType>
      </Properties>
    </Blob>
    <Blob>
      <Name>resourceId=/SUBSCRIPTIONS/B/PT1H.json</Name>
      <Properties>
        <Last-Modified>Sun, 18 Oct 2026 12:05:00 GMT</Last-Modified>
        <Content-Length>4096</Content-Length>
      </Properties>
    </Blob>
  </Blobs>
  <NextMarker>2!80!MDAwMDIy</NextMarker>
</EnumerationResults>"#;

    #[test]
    fn list_xml_maps_names_times_sizes_and_next_marker() {
        let listing = parse_list_xml(LIST_XML).unwrap();
        assert_eq!(listing.blobs.len(), 2);
        assert_eq!(listing.blobs[0].name, "resourceId=/SUBSCRIPTIONS/A/PT1H.json");
        assert_eq!(listing.blobs[0].content_length, 2048);
        assert_eq!(
            listing.blobs[0].last_modified,
            DateTime::parse_from_rfc3339("2026-10-18T11:05:00Z").ok().map(|d| d.with_timezone(&Utc))
        );
        assert_eq!(listing.blobs[1].content_length, 4096);
        assert_eq!(listing.next_marker.as_deref(), Some("2!80!MDAwMDIy"));
    }

    #[test]
    fn empty_next_marker_means_complete() {
        let xml = r#"<EnumerationResults><Blobs/><NextMarker></NextMarker></EnumerationResults>"#;
        let listing = parse_list_xml(xml).unwrap();
        assert!(listing.blobs.is_empty());
        assert_eq!(listing.next_marker, None);
    }

    #[test]
    fn missing_next_marker_element_means_complete() {
        let xml = r#"<EnumerationResults><Blobs><Blob><Name>a</Name><Properties/></Blob></Blobs></EnumerationResults>"#;
        let listing = parse_list_xml(xml).unwrap();
        assert_eq!(listing.blobs.len(), 1);
        assert_eq!(listing.blobs[0].last_modified, None);
        assert_eq!(listing.next_marker, None);
    }

    #[test]
    fn rfc1123_and_iso_times_parse() {
        let expected = DateTime::parse_from_rfc3339("2026-10-19T10:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(parse_last_modified("Mon, 19 Oct 2026 10:00:00 GMT"), Some(expected));
        assert_eq!(parse_last_modified("2026-10-19T10:00:00Z"), Some(expected));
        assert_eq!(parse_last_modified("not a time"), None);
    }

    #[test]
    fn list_url_carries_prefix_marker_and_page_size() {
        let url = list_url("acct", "insights", Some("resourceId=/"), Some("m1"), Some(1)).unwrap();
        let query = url.query().unwrap();
        assert!(query.contains("restype=container"));
        assert!(query.contains("comp=list"));
        assert!(query.contains("prefix=resourceId%3D%2F"));
        assert!(query.contains("marker=m1"));
        assert!(query.contains("maxresults=1"));
        assert_eq!(url.path(), "/insights");
    }

    #[test]
    fn list_url_without_optional_parts_has_no_marker() {
        let url = list_url("acct", "insights", None, None, None).unwrap();
        let query = url.query().unwrap();
        assert!(!query.contains("marker"));
        assert!(!query.contains("maxresults"));
        assert!(!query.contains("prefix"));
    }

    #[test]
    fn blob_url_encodes_each_name_segment() {
        let url = blob_url("acct", "insights", "resourceId=/SUB A/PT1H.json").unwrap();
        assert_eq!(url.as_str(), "https://acct.blob.core.windows.net/insights/resourceId=/SUB%20A/PT1H.json");
    }

    #[test]
    fn metadata_headers_are_read_case_insensitively() {
        let mut headers = HeaderMap::new();
        headers.insert(HeaderName::from_static("x-ms-meta-lastprocessed"), HeaderValue::from_static("2026-10-19"));
        headers
            .insert(HeaderName::from_static("x-ms-meta-processedby"), HeaderValue::from_static("AzureFlowLogParser"));
        headers.insert(HeaderName::from_static("content-length"), HeaderValue::from_static("12"));
        let metadata = metadata_from_headers(&headers);
        assert_eq!(metadata.get("lastprocessed").map(String::as_str), Some("2026-10-19"));
        assert_eq!(metadata.get("processedby").map(String::as_str), Some("AzureFlowLogParser"));
        assert_eq!(metadata.len(), 2);
    }

    #[test]
    fn access_denied_detection_uses_status_403() {
        let denied = anyhow::Error::new(AzureHttpError { status: 403, code: None, message: "no".into() });
        let missing = anyhow::Error::new(AzureHttpError { status: 404, code: None, message: "no".into() });
        assert!(is_access_denied(&denied));
        assert!(!is_access_denied(&missing));
        assert!(!is_access_denied(&anyhow!("network down")));
    }
}
