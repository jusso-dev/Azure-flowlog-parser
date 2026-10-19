//! Storage account source loaders for the parse command: file, environment variable, Key Vault secret, and
//! name validation. Same rules as StorageAccountConfigLoader.cs.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::azure::AzureClient;
use crate::azure::keyvault;

/// Splits on commas and line breaks, trims, drops empty entries and "#" comments, and de-duplicates in order.
pub fn parse_accounts(content: &str) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut accounts = Vec::new();
    for part in content.split([',', '\n', '\r']) {
        let name = part.trim();
        if name.is_empty() || name.starts_with('#') {
            continue;
        }
        if seen.insert(name.to_string()) {
            accounts.push(name.to_string());
        }
    }
    if accounts.is_empty() {
        bail!("No storage accounts found in the provided content");
    }
    Ok(accounts)
}

pub fn load_from_file(path: &Path) -> Result<Vec<String>> {
    if !path.is_file() {
        bail!("Storage accounts file not found: {}", path.display());
    }
    let content = std::fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    parse_accounts(&content)
}

/// Accounts from an environment variable whose value has already been read. A blank or missing value is an error.
pub fn accounts_from_env_value(name: &str, value: Option<String>) -> Result<Vec<String>> {
    match value.filter(|text| !text.trim().is_empty()) {
        Some(text) => parse_accounts(&text),
        None => bail!("Environment variable '{name}' is not set or is empty"),
    }
}

pub async fn load_from_key_vault(client: &AzureClient, vault_url: &str, secret: &str) -> Result<Vec<String>> {
    let value = keyvault::get_secret(client, vault_url, secret)
        .await
        .with_context(|| format!("Failed to retrieve secret '{secret}' from Key Vault '{vault_url}'"))?;
    if value.trim().is_empty() {
        bail!("Key Vault secret '{secret}' is empty");
    }
    parse_accounts(&value)
}

/// 3 to 24 characters, lowercase ASCII letters or digits (Azure storage account naming rules).
pub fn is_valid_account_name(name: &str) -> bool {
    let length = name.chars().count();
    (3..=24).contains(&length) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Keeps valid names and warns on stderr about the rest. Fails when nothing valid is left.
pub fn validate_accounts(names: Vec<String>) -> Result<Vec<String>> {
    let (valid, invalid): (Vec<String>, Vec<String>) = names.into_iter().partition(|name| is_valid_account_name(name));
    if !invalid.is_empty() {
        eprintln!("Warning: Skipping {} invalid storage account name(s):", invalid.len());
        for name in &invalid {
            eprintln!("  - '{name}' (must be 3-24 lowercase letters/numbers)");
        }
    }
    if valid.is_empty() {
        bail!("No valid storage account names found");
    }
    Ok(valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_commas_and_newlines_including_crlf() {
        let accounts = parse_accounts("alpha, beta\r\ngamma\n delta ,epsilon").unwrap();
        assert_eq!(accounts, vec!["alpha", "beta", "gamma", "delta", "epsilon"]);
    }

    #[test]
    fn hash_lines_and_blank_entries_are_ignored() {
        let accounts = parse_accounts("# header line\n\nalpha\n   \n# beta\ngamma").unwrap();
        assert_eq!(accounts, vec!["alpha", "gamma"]);
    }

    #[test]
    fn comma_inside_a_comment_leaks_as_in_the_reference() {
        // The reference splits on commas before it checks for "#", so the text after a comma is kept.
        let accounts = parse_accounts("# header, with comma\nalpha").unwrap();
        assert_eq!(accounts, vec!["with comma", "alpha"]);
    }

    #[test]
    fn duplicates_are_removed_keeping_first_order() {
        let accounts = parse_accounts("beta,alpha,beta\nalpha").unwrap();
        assert_eq!(accounts, vec!["beta", "alpha"]);
    }

    #[test]
    fn content_without_accounts_is_an_error() {
        let error = parse_accounts("# only comments\n,,\n").unwrap_err();
        assert_eq!(error.to_string(), "No storage accounts found in the provided content");
    }

    #[test]
    fn env_value_must_be_set_and_not_blank() {
        assert_eq!(accounts_from_env_value("ACCTS", Some("a1x,b2y".into())).unwrap(), vec!["a1x", "b2y"]);
        let missing = accounts_from_env_value("ACCTS", None).unwrap_err();
        assert_eq!(missing.to_string(), "Environment variable 'ACCTS' is not set or is empty");
        assert!(accounts_from_env_value("ACCTS", Some("   ".into())).is_err());
    }

    #[test]
    fn missing_file_uses_the_reference_message() {
        let error = load_from_file(Path::new("/nonexistent/flowlog-accounts.txt")).unwrap_err();
        assert_eq!(error.to_string(), "Storage accounts file not found: /nonexistent/flowlog-accounts.txt");
    }

    #[test]
    fn name_rules_length_and_characters() {
        assert!(is_valid_account_name("abc"));
        assert!(is_valid_account_name(&"a".repeat(24)));
        assert!(is_valid_account_name("stflowlogs01"));
        assert!(!is_valid_account_name("ab"));
        assert!(!is_valid_account_name(&"a".repeat(25)));
        assert!(!is_valid_account_name("Upper"));
        assert!(!is_valid_account_name("has-dash"));
        assert!(!is_valid_account_name("has space"));
    }

    #[test]
    fn validation_keeps_valid_names_and_fails_when_none_remain() {
        let valid = validate_accounts(vec!["good1".into(), "x".into(), "Bad".into()]).unwrap();
        assert_eq!(valid, vec!["good1"]);
        let error = validate_accounts(vec!["x".into()]).unwrap_err();
        assert_eq!(error.to_string(), "No valid storage account names found");
    }
}
