//! Entra ID token chain in the order of DefaultAzureCredential: environment service principal, managed identity,
//! Azure CLI. Tokens are cached per resource until five minutes before expiry. Secrets and tokens are never
//! printed; failure reasons are short categories.
//!
//! Doc URLs for the contracts used here:
//! - Client credentials: https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-client-creds-grant-flow
//! - Managed identity from a VM (IMDS, api-version 2018-02-01): https://learn.microsoft.com/en-us/entra/identity/managed-identities-azure-resources/how-to-use-vm-token
//! - Managed identity endpoint (IDENTITY_ENDPOINT, api-version 2019-08-01): https://learn.microsoft.com/en-us/azure/app-service/overview-managed-identity
//! - Azure CLI: https://learn.microsoft.com/en-us/cli/azure/account?view=azure-cli-latest

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use super::http::{describe_failure, transport_failure};

pub const RESOURCE_ARM: &str = "https://management.azure.com/";
pub const RESOURCE_STORAGE: &str = "https://storage.azure.com/";
pub const RESOURCE_VAULT: &str = "https://vault.azure.net";

const LOGIN_BASE: &str = "https://login.microsoftonline.com";
const IMDS_TOKEN_URL: &str = "http://169.254.169.254/metadata/identity/oauth2/token";
const IMDS_API_VERSION: &str = "2018-02-01";
const ENDPOINT_API_VERSION: &str = "2019-08-01";
const IMDS_TIMEOUT: Duration = Duration::from_secs(5);

/// One step of the chain, decided from the environment before any network call.
#[derive(Clone, PartialEq, Eq)]
pub enum Source {
    /// Environment service principal: AZURE_TENANT_ID, AZURE_CLIENT_ID and AZURE_CLIENT_SECRET all set.
    ServicePrincipal {
        tenant_id: String,
        client_id: String,
        client_secret: String,
    },
    ManagedIdentity(ManagedIdentity),
    AzureCli,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ManagedIdentity {
    /// App Service style endpoint: IDENTITY_ENDPOINT and IDENTITY_HEADER are both set.
    Endpoint { url: String, secret: String, client_id: Option<String> },
    /// Instance Metadata Service at 169.254.169.254.
    Imds { client_id: Option<String> },
}

impl Source {
    pub fn name(&self) -> &'static str {
        match self {
            Source::ServicePrincipal { .. } => "environment service principal",
            Source::ManagedIdentity(ManagedIdentity::Endpoint { .. }) => "managed identity endpoint",
            Source::ManagedIdentity(ManagedIdentity::Imds { .. }) => "managed identity (IMDS)",
            Source::AzureCli => "Azure CLI",
        }
    }
}

/// Builds the chain from environment variables. `env` is injected so tests can check the ordering without touching
/// the process environment or the network. Partially set service principal variables are ignored.
pub fn sources_from_env(env: &dyn Fn(&str) -> Option<String>) -> Vec<Source> {
    let present = |name: &str| env(name).filter(|value| !value.trim().is_empty());
    let mut sources = Vec::new();

    if let (Some(tenant_id), Some(client_id), Some(client_secret)) =
        (present("AZURE_TENANT_ID"), present("AZURE_CLIENT_ID"), present("AZURE_CLIENT_SECRET"))
    {
        sources.push(Source::ServicePrincipal { tenant_id, client_id, client_secret });
    }

    let user_assigned = present("AZURE_CLIENT_ID");
    match (present("IDENTITY_ENDPOINT"), present("IDENTITY_HEADER")) {
        (Some(url), Some(secret)) => {
            sources.push(Source::ManagedIdentity(ManagedIdentity::Endpoint { url, secret, client_id: user_assigned }));
        }
        _ => sources.push(Source::ManagedIdentity(ManagedIdentity::Imds { client_id: user_assigned })),
    }

    sources.push(Source::AzureCli);
    sources
}

/// A bearer token with its expiry. Debug output is redacted.
#[derive(Clone)]
pub struct AccessToken {
    value: String,
    expires_on: DateTime<Utc>,
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AccessToken(<redacted>, expires {})", self.expires_on)
    }
}

impl AccessToken {
    fn is_fresh(&self, now: DateTime<Utc>) -> bool {
        now + TimeDelta::minutes(5) < self.expires_on
    }
}

pub struct TokenChain {
    sources: Vec<Source>,
    http: reqwest::Client,
    cache: Mutex<HashMap<String, AccessToken>>,
}

impl TokenChain {
    pub fn new(sources: Vec<Source>, http: reqwest::Client) -> Self {
        Self { sources, http, cache: Mutex::new(HashMap::new()) }
    }

    pub fn from_env(http: reqwest::Client) -> Self {
        Self::new(sources_from_env(&|name: &str| std::env::var(name).ok()), http)
    }

    /// Bearer token for `resource`, from the cache when it is still fresh, otherwise from the first source that
    /// succeeds. The error lists each source with its short failure reason and no secret values.
    pub async fn token(&self, resource: &str) -> Result<String> {
        if let Some(value) = self.cached(resource) {
            return Ok(value);
        }

        let mut failures = Vec::new();
        for source in &self.sources {
            match self.fetch(source, resource).await {
                Ok(token) => {
                    let value = token.value.clone();
                    self.store(resource, token);
                    return Ok(value);
                }
                Err(reason) => failures.push(format!("{}: {reason}", source.name())),
            }
        }
        bail!("Azure authentication failed, no credential succeeded ({})", failures.join("; "))
    }

    fn cached(&self, resource: &str) -> Option<String> {
        let cache = self.cache.lock().ok()?;
        cache.get(resource).filter(|token| token.is_fresh(Utc::now())).map(|token| token.value.clone())
    }

    fn store(&self, resource: &str, token: AccessToken) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(resource.to_string(), token);
        }
    }

    async fn fetch(&self, source: &Source, resource: &str) -> std::result::Result<AccessToken, String> {
        match source {
            Source::ServicePrincipal { tenant_id, client_id, client_secret } => {
                self.service_principal(tenant_id, client_id, client_secret, resource).await
            }
            Source::ManagedIdentity(identity) => self.managed_identity(identity, resource).await,
            Source::AzureCli => azure_cli(resource).await,
        }
    }

    async fn service_principal(
        &self,
        tenant_id: &str,
        client_id: &str,
        client_secret: &str,
        resource: &str,
    ) -> std::result::Result<AccessToken, String> {
        let url = format!("{LOGIN_BASE}/{tenant_id}/oauth2/v2.0/token");
        let scope = scope_for(resource);
        let body = form_body(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("scope", scope.as_str()),
        ]);
        let response = self
            .http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|error| describe_failure(transport_failure(&error)).to_string())?;
        read_token_response(response).await
    }

    async fn managed_identity(
        &self,
        identity: &ManagedIdentity,
        resource: &str,
    ) -> std::result::Result<AccessToken, String> {
        let response = match identity {
            ManagedIdentity::Imds { client_id } => {
                let url = with_params(IMDS_TOKEN_URL, IMDS_API_VERSION, resource, client_id.as_deref())?;
                self.http.get(url).header("Metadata", "true").timeout(IMDS_TIMEOUT).send().await
            }
            ManagedIdentity::Endpoint { url, secret, client_id } => {
                let url = with_params(url, ENDPOINT_API_VERSION, resource, client_id.as_deref())?;
                self.http.get(url).header("X-IDENTITY-HEADER", secret.as_str()).timeout(IMDS_TIMEOUT).send().await
            }
        }
        .map_err(|error| describe_failure(transport_failure(&error)).to_string())?;
        read_token_response(response).await
    }
}

/// Scope for the v2 endpoint: the resource without a trailing slash, plus "/.default".
pub fn scope_for(resource: &str) -> String {
    format!("{}/.default", resource.trim_end_matches('/'))
}

/// Managed identity URL with api-version, resource and optional client_id as query parameters. Built with the url
/// crate because reqwest's query helper is not enabled in this crate's feature set.
fn with_params(
    base: &str,
    api_version: &str,
    resource: &str,
    client_id: Option<&str>,
) -> std::result::Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(base).map_err(|_| "invalid identity endpoint".to_string())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("api-version", api_version).append_pair("resource", resource);
        if let Some(id) = client_id {
            query.append_pair("client_id", id);
        }
    }
    Ok(url)
}

/// application/x-www-form-urlencoded body. Encodes through the url crate, so no extra dependency is needed.
pub fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse("https://localhost/").expect("static URL parses");
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
    }
    url.query().unwrap_or_default().to_string()
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Epoch {
    Number(i64),
    Text(String),
}

impl Epoch {
    fn seconds(&self) -> Option<i64> {
        match self {
            Epoch::Number(value) => Some(*value),
            Epoch::Text(text) => text.trim().parse().ok(),
        }
    }
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
    expires_on: Option<Epoch>,
    expires_in: Option<Epoch>,
}

/// Parses a token endpoint body (service principal, IMDS or App Service). Error text never includes the body.
pub fn parse_token_body(status: u16, body: &str, now: DateTime<Utc>) -> std::result::Result<AccessToken, String> {
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    let parsed: TokenBody = serde_json::from_str(body).map_err(|_| "unexpected response".to_string())?;
    let expires_on = match (
        parsed.expires_on.as_ref().and_then(Epoch::seconds),
        parsed.expires_in.as_ref().and_then(Epoch::seconds),
    ) {
        (Some(epoch), _) => DateTime::from_timestamp(epoch, 0).ok_or("invalid expiry")?,
        (None, Some(seconds)) => now + TimeDelta::seconds(seconds),
        (None, None) => return Err("no expiry in response".to_string()),
    };
    Ok(AccessToken { value: parsed.access_token, expires_on })
}

async fn read_token_response(response: reqwest::Response) -> std::result::Result<AccessToken, String> {
    let status = response.status().as_u16();
    let body = response.text().await.map_err(|_| "unreadable response".to_string())?;
    parse_token_body(status, &body, Utc::now())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliToken {
    access_token: String,
    #[serde(rename = "expires_on")]
    expires_on: Epoch,
}

/// Parses `az account get-access-token --output json`. `expires_on` is the POSIX timestamp field.
pub fn parse_cli_output(stdout: &[u8]) -> std::result::Result<AccessToken, String> {
    let parsed: CliToken = serde_json::from_slice(stdout).map_err(|_| "unexpected output".to_string())?;
    let seconds = parsed.expires_on.seconds().ok_or("invalid expiry")?;
    let expires_on = DateTime::from_timestamp(seconds, 0).ok_or("invalid expiry")?;
    Ok(AccessToken { value: parsed.access_token, expires_on })
}

async fn azure_cli(resource: &str) -> std::result::Result<AccessToken, String> {
    let output = tokio::process::Command::new("az")
        .args(["account", "get-access-token", "--resource", resource, "--output", "json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => "az not found on PATH".to_string(),
            _ => "az could not be started".to_string(),
        })?;
    if !output.status.success() {
        return Err("not logged in (run az login)".to_string());
    }
    parse_cli_output(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name: &str| map.get(name).cloned()
    }

    fn names(sources: &[Source]) -> Vec<&'static str> {
        sources.iter().map(Source::name).collect()
    }

    #[test]
    fn empty_environment_uses_imds_then_cli() {
        let sources = sources_from_env(&env_from(&[]));
        assert_eq!(names(&sources), vec!["managed identity (IMDS)", "Azure CLI"]);
    }

    #[test]
    fn service_principal_comes_first_when_all_three_variables_are_set() {
        let env = env_from(&[
            ("AZURE_TENANT_ID", "tenant"),
            ("AZURE_CLIENT_ID", "client"),
            ("AZURE_CLIENT_SECRET", "secret"),
        ]);
        let sources = sources_from_env(&env);
        assert_eq!(names(&sources), vec!["environment service principal", "managed identity (IMDS)", "Azure CLI"]);
    }

    #[test]
    fn partial_service_principal_is_skipped() {
        let env = env_from(&[("AZURE_TENANT_ID", "tenant"), ("AZURE_CLIENT_ID", "client")]);
        let sources = sources_from_env(&env);
        assert_eq!(names(&sources), vec!["managed identity (IMDS)", "Azure CLI"]);
    }

    #[test]
    fn blank_variables_count_as_unset() {
        let env = env_from(&[("AZURE_TENANT_ID", "  "), ("AZURE_CLIENT_ID", "c"), ("AZURE_CLIENT_SECRET", "s")]);
        assert_eq!(names(&sources_from_env(&env)), vec!["managed identity (IMDS)", "Azure CLI"]);
    }

    #[test]
    fn identity_endpoint_replaces_imds_when_header_is_also_set() {
        let env = env_from(&[("IDENTITY_ENDPOINT", "http://127.0.0.1:8081/msi/token"), ("IDENTITY_HEADER", "h")]);
        let sources = sources_from_env(&env);
        assert_eq!(names(&sources), vec!["managed identity endpoint", "Azure CLI"]);
    }

    #[test]
    fn identity_endpoint_without_header_falls_back_to_imds() {
        let env = env_from(&[("IDENTITY_ENDPOINT", "http://127.0.0.1:8081/msi/token")]);
        assert_eq!(names(&sources_from_env(&env)), vec!["managed identity (IMDS)", "Azure CLI"]);
    }

    #[test]
    fn client_id_selects_user_assigned_identity_in_managed_identity_step() {
        let env = env_from(&[("AZURE_CLIENT_ID", "user-assigned")]);
        let sources = sources_from_env(&env);
        match &sources[0] {
            Source::ManagedIdentity(ManagedIdentity::Imds { client_id }) => {
                assert_eq!(client_id.as_deref(), Some("user-assigned"));
            }
            _ => panic!("expected IMDS source first"),
        }
    }

    #[test]
    fn scope_has_no_double_slash() {
        assert_eq!(scope_for("https://management.azure.com/"), "https://management.azure.com/.default");
        assert_eq!(scope_for("https://vault.azure.net"), "https://vault.azure.net/.default");
    }

    #[test]
    fn form_body_encodes_secret_characters() {
        let body = form_body(&[("client_secret", "a+b/c=d&e"), ("grant_type", "client_credentials")]);
        assert_eq!(body, "client_secret=a%2Bb%2Fc%3Dd%26e&grant_type=client_credentials");
    }

    #[test]
    fn token_body_uses_expires_on_epoch_as_string_or_number() {
        let now = DateTime::from_timestamp(1_000, 0).unwrap();
        let as_string = parse_token_body(200, r#"{"access_token":"t1","expires_on":"4600"}"#, now).unwrap();
        assert_eq!(as_string.expires_on.timestamp(), 4600);
        let as_number = parse_token_body(200, r#"{"access_token":"t2","expires_on":4600}"#, now).unwrap();
        assert_eq!(as_number.expires_on.timestamp(), 4600);
    }

    #[test]
    fn token_body_falls_back_to_expires_in() {
        let now = DateTime::from_timestamp(1_000, 0).unwrap();
        let token =
            parse_token_body(200, r#"{"access_token":"t","expires_in":3600,"token_type":"Bearer"}"#, now).unwrap();
        assert_eq!(token.expires_on.timestamp(), 4600);
    }

    #[test]
    fn failed_status_reports_code_without_body() {
        let now = Utc::now();
        let error =
            parse_token_body(401, r#"{"access_token":"secret-looking","error":"invalid_client"}"#, now).unwrap_err();
        assert_eq!(error, "HTTP 401");
    }

    #[test]
    fn missing_expiry_is_an_error() {
        let error = parse_token_body(200, r#"{"access_token":"t"}"#, Utc::now()).unwrap_err();
        assert_eq!(error, "no expiry in response");
    }

    #[test]
    fn cli_output_parses_posix_expiry() {
        let token =
            parse_cli_output(br#"{"accessToken":"t","expiresOn":"2026-10-19 12:00:00.000000","expires_on":4600}"#)
                .unwrap();
        assert_eq!(token.expires_on.timestamp(), 4600);
    }

    #[test]
    fn token_debug_hides_value() {
        let token =
            AccessToken { value: "super-secret".into(), expires_on: DateTime::from_timestamp(4600, 0).unwrap() };
        let text = format!("{token:?}");
        assert!(!text.contains("super-secret"));
        assert!(text.contains("redacted"));
    }

    #[test]
    fn freshness_keeps_five_minute_margin() {
        let expiry = DateTime::from_timestamp(10_000, 0).unwrap();
        let token = AccessToken { value: "t".into(), expires_on: expiry };
        assert!(token.is_fresh(DateTime::from_timestamp(9_699, 0).unwrap()));
        assert!(!token.is_fresh(DateTime::from_timestamp(9_700, 0).unwrap()));
    }
}
