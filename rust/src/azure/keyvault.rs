//! Key Vault secret read. Get Secret: GET {vault}/secrets/{name}?api-version=2025-07-01, value in the "value" field.
//! Doc: https://learn.microsoft.com/en-us/rest/api/keyvault/secrets/get-secret/get-secret (the page lists only this
//! api-version and does not label it GA or preview).

use anyhow::{Context, Result, anyhow};
use reqwest::Url;
use serde::Deserialize;

use super::auth::RESOURCE_VAULT;
use super::client::AzureClient;
use super::http::{describe_failure, ensure_success, send_with_retry, transport_failure};

pub const KEY_VAULT_API_VERSION: &str = "2025-07-01";

#[derive(Deserialize)]
struct SecretBundle {
    value: Option<String>,
}

/// Secret URL for a vault base URL such as https://myvault.vault.azure.net/ and a secret name.
pub fn secret_url(vault_url: &str, name: &str) -> Result<Url> {
    let mut url = Url::parse(vault_url).context("invalid Key Vault URL")?;
    url.path_segments_mut().map_err(|_| anyhow!("invalid Key Vault URL"))?.pop_if_empty().push("secrets").push(name);
    url.query_pairs_mut().append_pair("api-version", KEY_VAULT_API_VERSION);
    Ok(url)
}

/// Value of one secret. Returns an error for HTTP failures; an empty value is returned as an empty string and the
/// caller decides what that means.
pub async fn get_secret(client: &AzureClient, vault_url: &str, name: &str) -> Result<String> {
    let url = secret_url(vault_url, name)?;
    let token = client.token(RESOURCE_VAULT).await?;
    let http = client.http();
    let response = send_with_retry(client.retry(), || http.get(url.clone()).bearer_auth(&token))
        .await
        .map_err(|error| anyhow!("Key Vault request {}", describe_failure(transport_failure(&error))))?;
    let response = ensure_success(response).await?;
    let bundle: SecretBundle = response.json().await.context("unexpected Key Vault response")?;
    Ok(bundle.value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_url_joins_vault_secrets_path_and_api_version() {
        let url = secret_url("https://myvault.vault.azure.net/", "storageaccounts").unwrap();
        assert_eq!(url.as_str(), "https://myvault.vault.azure.net/secrets/storageaccounts?api-version=2025-07-01");
    }

    #[test]
    fn secret_url_accepts_vault_without_trailing_slash() {
        let url = secret_url("https://myvault.vault.azure.net", "cortextoken").unwrap();
        assert_eq!(url.path(), "/secrets/cortextoken");
    }

    #[test]
    fn secret_url_encodes_name_segment() {
        let url = secret_url("https://myvault.vault.azure.net", "a/b").unwrap();
        assert_eq!(url.path(), "/secrets/a%2Fb");
    }

    #[test]
    fn invalid_vault_url_is_rejected() {
        assert!(secret_url("not a url", "x").is_err());
    }
}
