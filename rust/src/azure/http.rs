//! HTTP helpers shared by the Azure calls and the parse command's flow log poster: retry with exponential backoff
//! and jitter, and Azure error mapping. Retry rules match FlowLogHttpClient.cs: retry 408, 429 and 5xx, plus
//! connection, timeout and send failures. Other statuses (400, 403, ...) are returned to the caller without retry.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;

/// Retry settings. Default matches the reference: 3 retries, 1 second base delay, exponential growth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_retries: 3, base_delay: Duration::from_secs(1) }
    }
}

impl RetryPolicy {
    /// Delay before retry number `attempt` (1-based): base * 2^(attempt - 1), scaled by `jitter`.
    /// The sender passes a jitter factor in [0.5, 1.5).
    pub fn delay(&self, attempt: u32, jitter: f64) -> Duration {
        let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
        (self.base_delay * factor).mul_f64(jitter)
    }
}

/// Transient statuses that are retried: 408, 429 and every 5xx.
pub fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 429 || (500..=599).contains(&status)
}

/// Transport failure categories, mapped from reqwest errors so the retry rule can be tested without a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    Timeout,
    Connect,
    Request,
    Other,
}

pub fn transport_failure(error: &reqwest::Error) -> TransportFailure {
    if error.is_timeout() {
        TransportFailure::Timeout
    } else if error.is_connect() {
        TransportFailure::Connect
    } else if error.is_request() {
        TransportFailure::Request
    } else {
        TransportFailure::Other
    }
}

pub fn is_retryable_failure(failure: TransportFailure) -> bool {
    matches!(failure, TransportFailure::Timeout | TransportFailure::Connect | TransportFailure::Request)
}

/// Short user-facing reason for a transport failure. Never includes the request URL or any header.
pub fn describe_failure(failure: TransportFailure) -> &'static str {
    match failure {
        TransportFailure::Timeout => "timed out",
        TransportFailure::Connect => "could not connect",
        TransportFailure::Request | TransportFailure::Other => "request failed",
    }
}

/// Sends the request built by `build`, retrying transient failures. When the last attempt still returns a
/// retryable status, that response is returned so the caller reports it like any other status.
pub async fn send_with_retry<F>(policy: RetryPolicy, build: F) -> Result<reqwest::Response, reqwest::Error>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt = 0;
    loop {
        let outcome = build().send().await;
        let retry = match &outcome {
            Ok(response) => is_retryable_status(response.status().as_u16()),
            Err(error) => is_retryable_failure(transport_failure(error)),
        };
        if !retry || attempt >= policy.max_retries {
            return outcome;
        }
        attempt += 1;
        tokio::time::sleep(policy.delay(attempt, 0.5 + fastrand::f64())).await;
    }
}

/// An HTTP error from an Azure service. Callers downcast anyhow errors to this type to read the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureHttpError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl AzureHttpError {
    pub fn is_access_denied(&self) -> bool {
        self.status == 403
    }
}

impl fmt::Display for AzureHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.code {
            Some(code) => write!(f, "HTTP {} {}: {}", self.status, code, self.message),
            None => write!(f, "HTTP {}: {}", self.status, self.message),
        }
    }
}

impl std::error::Error for AzureHttpError {}

/// Returns the response when its status is 2xx, otherwise an `AzureHttpError` wrapped in anyhow.
pub async fn ensure_success(response: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    Err(anyhow::Error::new(azure_error(response).await))
}

/// Reads an Azure error response. ARM returns JSON `{"error": {"code", "message"}}`. Storage returns an XML
/// `<Error><Code/><Message/></Error>` and an `x-ms-error-code` header, which wins when both are present.
pub async fn azure_error(response: reqwest::Response) -> AzureHttpError {
    let status = response.status();
    let header_code =
        response.headers().get("x-ms-error-code").and_then(|value| value.to_str().ok()).map(str::to_owned);
    let reason = status.canonical_reason().unwrap_or("error").to_string();
    let body = response.text().await.unwrap_or_default();
    let (body_code, body_message) = parse_error_body(&body);
    AzureHttpError {
        status: status.as_u16(),
        code: header_code.or(body_code),
        message: single_line(body_message.unwrap_or(reason)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArmEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ErrorBody {
    code: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "Error")]
struct StorageEnvelope {
    #[serde(rename = "Code")]
    code: Option<String>,
    #[serde(rename = "Message")]
    message: Option<String>,
}

/// Code and message from an ARM JSON body or a Storage XML body. Empty or unknown bodies give (None, None).
pub fn parse_error_body(body: &str) -> (Option<String>, Option<String>) {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        if let Ok(envelope) = serde_json::from_str::<ArmEnvelope>(trimmed) {
            if let Some(error) = envelope.error {
                return (error.code, error.message);
            }
        }
    } else if trimmed.starts_with('<') {
        if let Ok(envelope) = quick_xml::de::from_str::<StorageEnvelope>(trimmed) {
            return (envelope.code, envelope.message);
        }
    }
    (None, None)
}

/// Collapses whitespace, including newlines, and caps the length so error lines stay on one line.
pub fn single_line(text: String) -> String {
    const MAX_CHARS: usize = 300;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_CHARS {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(MAX_CHARS).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_408_429_and_5xx_only() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(599));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(600));
    }

    #[test]
    fn retries_timeout_connect_and_request_failures_only() {
        assert!(is_retryable_failure(TransportFailure::Timeout));
        assert!(is_retryable_failure(TransportFailure::Connect));
        assert!(is_retryable_failure(TransportFailure::Request));
        assert!(!is_retryable_failure(TransportFailure::Other));
    }

    #[test]
    fn backoff_doubles_and_applies_jitter() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay(1, 1.0), Duration::from_secs(1));
        assert_eq!(policy.delay(2, 1.0), Duration::from_secs(2));
        assert_eq!(policy.delay(3, 1.0), Duration::from_secs(4));
        assert_eq!(policy.delay(3, 0.5), Duration::from_secs(2));
    }

    #[test]
    fn parses_arm_json_error_body() {
        let body = r#"{"error":{"code":"AuthorizationFailed","message":"The client does not have access."}}"#;
        assert_eq!(
            parse_error_body(body),
            (Some("AuthorizationFailed".into()), Some("The client does not have access.".into()))
        );
    }

    #[test]
    fn parses_storage_xml_error_body() {
        let body = "<?xml version=\"1.0\"?><Error><Code>ContainerNotFound</Code><Message>The specified container does not exist.</Message></Error>";
        assert_eq!(
            parse_error_body(body),
            (Some("ContainerNotFound".into()), Some("The specified container does not exist.".into()))
        );
    }

    #[test]
    fn unknown_body_gives_no_code() {
        assert_eq!(parse_error_body(""), (None, None));
        assert_eq!(parse_error_body("plain text"), (None, None));
    }

    #[test]
    fn single_line_collapses_newlines_and_truncates() {
        assert_eq!(single_line("a\n  b\r\nc".into()), "a b c");
        let long = "x".repeat(400);
        let out = single_line(long);
        assert_eq!(out.chars().count(), 303);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn azure_error_display_is_one_line() {
        let error = AzureHttpError {
            status: 403,
            code: Some("AuthorizationPermissionMismatch".into()),
            message: "denied".into(),
        };
        assert_eq!(error.to_string(), "HTTP 403 AuthorizationPermissionMismatch: denied");
        assert!(error.is_access_denied());
    }
}
