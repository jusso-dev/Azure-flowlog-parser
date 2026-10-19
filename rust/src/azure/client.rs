//! Shared HTTP client plus the Entra ID token chain. One instance is created per run and passed to the ARM, Blob
//! and Key Vault calls.

use std::time::Duration;

use super::auth::TokenChain;
use super::http::RetryPolicy;

pub struct AzureClient {
    http: reqwest::Client,
    tokens: TokenChain,
    retry: RetryPolicy,
}

impl AzureClient {
    /// Builds the HTTP client (rustls) and the token chain from the process environment. No network call happens here.
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("azure-flowlog-parser/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self { tokens: TokenChain::from_env(http.clone()), http, retry: RetryPolicy::default() })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn retry(&self) -> RetryPolicy {
        self.retry
    }

    /// Bearer token for `resource` (for example `super::auth::RESOURCE_ARM`). Callers never log it.
    pub async fn token(&self, resource: &str) -> anyhow::Result<String> {
        self.tokens.token(resource).await
    }
}
