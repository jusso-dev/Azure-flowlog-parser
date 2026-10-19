//! Entra ID token chain, tried in this order (see `sources_from_env`):
//! 1. Environment service principal: AZURE_TENANT_ID, AZURE_CLIENT_ID and AZURE_CLIENT_SECRET.
//! 2. App Service managed identity endpoint: IDENTITY_ENDPOINT and IDENTITY_HEADER.
//! 3. Instance Metadata Service (IMDS) at 169.254.169.254, with a two second budget so a laptop falls through quickly.
//! 4. Azure CLI session from `az login`.
//!
//! The first source that returns a token wins. Tokens are cached per resource until five minutes before expiry.
//! Secrets and tokens are never printed; failure reasons are short categories.
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
/// Covers connect and response for IMDS. IMDS is absent on a laptop, so it must fail fast.
const IMDS_TIMEOUT: Duration = Duration::from_secs(2);
const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// Azure CLI session from `az login`.
    AzureCli,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ManagedIdentity {
    /// App Service style endpoint: IDENTITY_ENDPOINT and IDENTITY_HEADER are both set.
    Endpoint { url: String, secret: String, client_id: Option<String> },
    /// Instance Metadata Service. `url` is the production token URL; tests point it at a local server.
    Imds { client_id: Option<String>, url: String },
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

    // 1. Environment service principal.
    if let (Some(tenant_id), Some(client_id), Some(client_secret)) =
        (present("AZURE_TENANT_ID"), present("AZURE_CLIENT_ID"), present("AZURE_CLIENT_SECRET"))
    {
        sources.push(Source::ServicePrincipal { tenant_id, client_id, client_secret });
    }

    // 2. App Service endpoint when both of its variables are set, otherwise 3. IMDS.
    let user_assigned = present("AZURE_CLIENT_ID");
    match (present("IDENTITY_ENDPOINT"), present("IDENTITY_HEADER")) {
        (Some(url), Some(secret)) => {
            sources.push(Source::ManagedIdentity(ManagedIdentity::Endpoint { url, secret, client_id: user_assigned }));
        }
        _ => sources.push(Source::ManagedIdentity(ManagedIdentity::Imds {
            client_id: user_assigned,
            url: IMDS_TOKEN_URL.to_string(),
        })),
    }

    // 4. Azure CLI session from `az login`.
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
        bail!("Azure authentication failed (tried {}); run `az login` or use managed identity", failures.join("; "))
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
            ManagedIdentity::Imds { client_id, url } => {
                let url = with_params(url, IMDS_API_VERSION, resource, client_id.as_deref())?;
                self.http.get(url).header("Metadata", "true").timeout(IMDS_TIMEOUT).send().await
            }
            ManagedIdentity::Endpoint { url, secret, client_id } => {
                let url = with_params(url, ENDPOINT_API_VERSION, resource, client_id.as_deref())?;
                self.http.get(url).header("X-IDENTITY-HEADER", secret.as_str()).timeout(ENDPOINT_TIMEOUT).send().await
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
        return Err("not logged in".to_string());
    }
    parse_cli_output(&output.stdout)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    use super::*;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name: &str| map.get(name).cloned()
    }

    fn names(sources: &[Source]) -> Vec<&'static str> {
        sources.iter().map(Source::name).collect()
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().expect("client builds")
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime builds").block_on(future)
    }

    fn token_json(value: &str, expires_on: i64) -> String {
        format!(r#"{{"access_token":"{value}","expires_on":{expires_on},"token_type":"Bearer"}}"#)
    }

    fn in_an_hour() -> i64 {
        Utc::now().timestamp() + 3600
    }

    fn endpoint(url: String) -> Source {
        Source::ManagedIdentity(ManagedIdentity::Endpoint { url, secret: "identity-secret".into(), client_id: None })
    }

    /// Local HTTP server for one test. Answers the first `responses.len()` connections in order, then stops listening,
    /// so an unexpected extra request is refused instead of hanging. Returns the URL and a handle that yields the
    /// request heads received.
    fn serve(responses: Vec<(u16, String)>) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/msi/token", listener.local_addr().expect("address"));
        let handle = thread::spawn(move || {
            let mut heads = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept");
                stream.set_read_timeout(Some(Duration::from_secs(5))).expect("read timeout");
                heads.push(read_head(&mut stream));
                let reply = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).expect("write response");
            }
            heads
        });
        (url, handle)
    }

    fn read_head(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Value of a request header, matched case-insensitively.
    fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    /// Decoded query parameters of the request line.
    fn query(head: &str) -> HashMap<String, String> {
        let target = head.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or_default();
        let url = reqwest::Url::parse(&format!("http://localhost{target}")).expect("request target parses");
        url.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect()
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
            Source::ManagedIdentity(ManagedIdentity::Imds { client_id, .. }) => {
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
    fn cli_output_accepts_string_expires_on() {
        let token = parse_cli_output(br#"{"accessToken":"t","expires_on":"4600"}"#).unwrap();
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

    #[test]
    fn app_service_endpoint_sends_identity_header_and_returns_token() {
        let (url, server) = serve(vec![(200, token_json("app-token", in_an_hour()))]);
        let source = Source::ManagedIdentity(ManagedIdentity::Endpoint {
            url,
            secret: "identity-secret".into(),
            client_id: Some("user-assigned".into()),
        });
        let chain = TokenChain::new(vec![source], http());
        let token = block_on(chain.token(RESOURCE_ARM)).expect("token");
        let heads = server.join().expect("server");
        assert_eq!(token, "app-token");
        assert_eq!(header(&heads[0], "X-IDENTITY-HEADER"), Some("identity-secret"));
        let params = query(&heads[0]);
        assert_eq!(params["api-version"], ENDPOINT_API_VERSION);
        assert_eq!(params["resource"], RESOURCE_ARM);
        assert_eq!(params["client_id"], "user-assigned");
    }

    #[test]
    fn imds_sends_metadata_header_to_injected_url() {
        let (url, server) = serve(vec![(200, token_json("imds-token", in_an_hour()))]);
        let source = Source::ManagedIdentity(ManagedIdentity::Imds { client_id: Some("user-assigned".into()), url });
        let chain = TokenChain::new(vec![source], http());
        let token = block_on(chain.token(RESOURCE_ARM)).expect("token");
        let heads = server.join().expect("server");
        assert_eq!(token, "imds-token");
        assert_eq!(header(&heads[0], "Metadata"), Some("true"));
        let params = query(&heads[0]);
        assert_eq!(params["api-version"], IMDS_API_VERSION);
        assert_eq!(params["resource"], RESOURCE_ARM);
        assert_eq!(params["client_id"], "user-assigned");
    }

    #[test]
    fn imds_gives_up_quickly_when_the_address_does_not_answer() {
        // Accepts connections into the backlog but never replies, which is what an unanswered IMDS looks like.
        let _silent = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/metadata/identity/oauth2/token", _silent.local_addr().expect("address"));
        let source = Source::ManagedIdentity(ManagedIdentity::Imds { client_id: None, url });
        let chain = TokenChain::new(vec![source], http());
        let started = Instant::now();
        let error = block_on(chain.token(RESOURCE_ARM)).expect_err("silent endpoint must fail");
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(4), "IMDS took {elapsed:?}");
        assert!(error.to_string().contains("managed identity (IMDS): timed out"), "{error}");
    }

    #[test]
    fn chain_falls_through_failing_source_to_the_next_one() {
        let (first, first_server) = serve(vec![(401, r#"{"error":"invalid_client"}"#.into())]);
        let (second, second_server) = serve(vec![(200, token_json("second-token", in_an_hour()))]);
        let chain = TokenChain::new(vec![endpoint(first), endpoint(second)], http());
        let token = block_on(chain.token(RESOURCE_ARM)).expect("token");
        assert_eq!(first_server.join().expect("first server").len(), 1);
        assert_eq!(second_server.join().expect("second server").len(), 1);
        assert_eq!(token, "second-token");
    }

    #[test]
    fn first_source_that_succeeds_wins_and_later_sources_are_not_used() {
        let (first, first_server) = serve(vec![(200, token_json("first-token", in_an_hour()))]);
        // No responses: if the chain reached this source, the connection would be refused.
        let (second, _second_server) = serve(vec![]);
        let chain = TokenChain::new(vec![endpoint(first), endpoint(second)], http());
        let token = block_on(chain.token(RESOURCE_ARM)).expect("token");
        assert_eq!(first_server.join().expect("first server").len(), 1);
        assert_eq!(token, "first-token");
    }

    #[test]
    fn fresh_token_is_served_from_cache() {
        let (url, server) = serve(vec![(200, token_json("cached-token", in_an_hour()))]);
        let chain = TokenChain::new(vec![endpoint(url)], http());
        let first = block_on(chain.token(RESOURCE_ARM)).expect("first token");
        let second = block_on(chain.token(RESOURCE_ARM)).expect("second token");
        assert_eq!(server.join().expect("server").len(), 1);
        assert_eq!((first.as_str(), second.as_str()), ("cached-token", "cached-token"));
    }

    #[test]
    fn token_within_five_minutes_of_expiry_is_fetched_again() {
        let soon = Utc::now().timestamp() + 120;
        let (url, server) =
            serve(vec![(200, token_json("old-token", soon)), (200, token_json("new-token", in_an_hour()))]);
        let chain = TokenChain::new(vec![endpoint(url)], http());
        let first = block_on(chain.token(RESOURCE_ARM)).expect("first token");
        let second = block_on(chain.token(RESOURCE_ARM)).expect("second token");
        assert_eq!(server.join().expect("server").len(), 2);
        assert_eq!((first.as_str(), second.as_str()), ("old-token", "new-token"));
    }

    #[test]
    fn total_failure_lists_each_source_and_ends_with_hint_without_response_body() {
        let (url, server) = serve(vec![(401, r#"{"error":"secret-body-text"}"#.into())]);
        let chain = TokenChain::new(vec![endpoint(url)], http());
        let error = block_on(chain.token(RESOURCE_ARM)).expect_err("401 must fail").to_string();
        server.join().expect("server");
        assert!(error.contains("managed identity endpoint: HTTP 401"), "{error}");
        assert!(error.ends_with("run `az login` or use managed identity"), "{error}");
        assert!(!error.contains("secret-body-text"), "{error}");
    }

    #[cfg(unix)]
    mod fake_cli {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::sync::MutexGuard;

        use super::*;

        /// Serialises the tests that change PATH.
        static PATH_LOCK: Mutex<()> = Mutex::new(());

        const SUCCESS: &str = r#"#!/bin/sh
[ "$1" = account ] && [ "$2" = get-access-token ] || exit 2
printf '%s\n' '{"accessToken":"cli-token","expiresOn":"2030-01-01 00:00:00.000000","expires_on":1893456000,"tokenType":"Bearer"}'
"#;

        const NOT_LOGGED_IN: &str = r#"#!/bin/sh
echo "Please run 'az login' to setup account." >&2
exit 1
"#;

        /// Puts a fake `az` first on PATH until dropped. Holds PATH_LOCK for its whole life.
        struct FakeAz {
            dir: PathBuf,
            previous: Option<std::ffi::OsString>,
            _lock: MutexGuard<'static, ()>,
        }

        impl FakeAz {
            fn install(script: &str) -> Self {
                let lock = PATH_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let dir = std::env::temp_dir().join(format!("az-fake-{}-{}", std::process::id(), fastrand::u64(..)));
                fs::create_dir_all(&dir).expect("temp dir");
                let az = dir.join("az");
                fs::write(&az, script).expect("write fake az");
                fs::set_permissions(&az, fs::Permissions::from_mode(0o755)).expect("make fake az executable");
                let previous = std::env::var_os("PATH");
                let mut path = dir.clone().into_os_string();
                path.push(":");
                path.push(previous.clone().unwrap_or_default());
                // SAFETY: PATH_LOCK is held for the life of the guard, so no other test reads or writes PATH meanwhile.
                unsafe { std::env::set_var("PATH", path) };
                Self { dir, previous, _lock: lock }
            }
        }

        impl Drop for FakeAz {
            fn drop(&mut self) {
                // SAFETY: as in `install`, the lock is still held here.
                unsafe {
                    match &self.previous {
                        Some(value) => std::env::set_var("PATH", value),
                        None => std::env::remove_var("PATH"),
                    }
                }
                let _ = fs::remove_dir_all(&self.dir);
            }
        }

        #[test]
        fn azure_cli_token_and_expiry_are_read_from_az_output() {
            let _az = FakeAz::install(SUCCESS);
            let chain = TokenChain::new(Vec::new(), http());
            let token = block_on(chain.fetch(&Source::AzureCli, RESOURCE_ARM)).expect("token");
            assert_eq!(token.value, "cli-token");
            assert_eq!(token.expires_on.timestamp(), 1_893_456_000);
        }

        #[test]
        fn failing_az_is_reported_as_azure_cli_not_logged_in() {
            let _az = FakeAz::install(NOT_LOGGED_IN);
            let chain = TokenChain::new(vec![Source::AzureCli], http());
            let error = block_on(chain.token(RESOURCE_ARM)).expect_err("az exits 1");
            assert!(error.to_string().contains("Azure CLI: not logged in"), "{error}");
        }
    }
}
