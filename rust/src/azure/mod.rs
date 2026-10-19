//! Azure access: Entra ID token chain, ARM and Blob REST calls, Key Vault, and shared HTTP helpers (retry and error
//! mapping). No Azure SDK: requests go through reqwest with rustls.
pub mod arm;
pub mod auth;
pub mod blob;
pub mod client;
pub mod http;
pub mod keyvault;
pub mod reader;

pub use client::AzureClient;
pub use reader::read_flow_blobs;
