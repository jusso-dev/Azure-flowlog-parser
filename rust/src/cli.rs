//! Command-line definitions. Long option names match the .NET tool. The .NET multi-letter short aliases
//! (for example -af, -hkv) have no clap equivalent; only single-letter shorts are offered here.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "azure-flowlog-parser",
    version,
    about = "Fetch and parse Azure VNet flow logs; discover storage accounts; build traffic reports"
)]
pub struct Cli {
    #[command(flatten)]
    pub parse: ParseArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List storage accounts that receive flow logs or hold flow log blobs, with health per configuration.
    Discover(DiscoverArgs),
    /// Aggregate flow observations between storage accounts into an HTML or PDF report.
    Report(ReportArgs),
}

/// Root command: parse flow logs to JSON or JSON Lines, optionally post them over HTTP.
#[derive(Debug, Args)]
pub struct ParseArgs {
    /// Single storage account name.
    #[arg(long, short = 's')]
    pub storage_account: Option<String>,
    /// File with storage account names (one per line or comma-separated).
    #[arg(long)]
    pub accounts_file: Option<PathBuf>,
    /// Environment variable holding comma-separated storage account names.
    #[arg(long)]
    pub accounts_env: Option<String>,
    /// Key Vault URL holding a secret with storage account names.
    #[arg(long)]
    pub accounts_keyvault: Option<String>,
    /// Key Vault secret name used with --accounts-keyvault.
    #[arg(long)]
    pub keyvault_secret: Option<String>,
    /// Container that holds flow logs.
    #[arg(long, short = 'c', default_value = "insights-logs-flowlogflowevent")]
    pub container: String,
    /// Blob name prefix filter.
    #[arg(long, short = 'p')]
    pub prefix: Option<String>,
    /// Output file (default: stdout).
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
    /// json (default) or jsonl.
    #[arg(long, short = 'f', default_value = "json")]
    pub format: String,
    /// Process at most this many blobs per storage account.
    #[arg(long, short = 'l')]
    pub limit: Option<usize>,
    /// Print progress to stderr.
    #[arg(long, short = 'v')]
    pub verbose: bool,
    /// Only list blobs, do not download.
    #[arg(long)]
    pub list_only: bool,
    /// Merge results from all accounts into one output (default true).
    #[arg(long, short = 'm', default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub merge_output: bool,
    /// HTTP endpoint to POST flow log records to.
    #[arg(long)]
    pub http_endpoint: Option<String>,
    /// Bearer token for the HTTP endpoint.
    #[arg(long)]
    pub http_token: Option<String>,
    /// Gzip request bodies (default true).
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub http_compression: bool,
    /// HTTP request timeout in seconds.
    #[arg(long, default_value_t = 300)]
    pub http_timeout: u64,
    /// Test HTTP endpoint connectivity and exit.
    #[arg(long)]
    pub http_test: bool,
    /// Key Vault URL holding the endpoint and token secrets.
    #[arg(long)]
    pub http_keyvault: Option<String>,
    /// Key Vault secret name for the endpoint URL.
    #[arg(long, default_value = "cortexendpoint")]
    pub http_endpoint_secret: String,
    /// Key Vault secret name for the bearer token.
    #[arg(long, default_value = "cortextoken")]
    pub http_token_secret: String,
    /// Track processed blobs in blob metadata to skip them next time (default true).
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub enable_state_tracking: bool,
    /// Reprocess every blob, ignoring metadata state.
    #[arg(long)]
    pub force_reprocess: bool,
}

#[derive(Debug, Args)]
pub struct DiscoverArgs {
    /// Subscription id to scan (repeat for several). Default: every enabled subscription the identity can read.
    #[arg(long = "subscription")]
    pub subscriptions: Vec<String>,
    /// Limit to these storage account names (repeat for several).
    #[arg(long, short = 's')]
    pub storage_account: Vec<String>,
    /// A flow log is Stale when its newest blob is older than this many hours.
    #[arg(long, default_value_t = 3)]
    pub stale_hours: u32,
    /// Also list accounts with neither flow log configurations nor flow log blobs.
    #[arg(long)]
    pub all: bool,
    /// table (default) or json.
    #[arg(long, default_value = "table")]
    pub output_format: String,
    /// Write output to this file instead of stdout.
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
    /// Print progress to stderr.
    #[arg(long, short = 'v')]
    pub verbose: bool,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Subscription id to scan (repeat for several). Default: every enabled subscription the identity can read.
    #[arg(long = "subscription")]
    pub subscriptions: Vec<String>,
    /// Limit to these storage account names (repeat for several).
    #[arg(long, short = 's')]
    pub storage_account: Vec<String>,
    /// Offline: local PT1H.json files or folders instead of Azure (repeat for several).
    #[arg(long = "input")]
    pub inputs: Vec<PathBuf>,
    /// Window start: 90m, 24h, 7d or ISO-8601 UTC. Default: 24h (Azure), observed range (--input).
    #[arg(long)]
    pub since: Option<String>,
    /// Window end, same formats. Default: now (Azure), observed range (--input).
    #[arg(long)]
    pub until: Option<String>,
    /// all (default), allow or deny.
    #[arg(long, default_value = "all")]
    pub action: String,
    /// all (default), tcp or udp.
    #[arg(long, default_value = "all")]
    pub protocol: String,
    /// Destination ports, e.g. 443,8000-8100.
    #[arg(long)]
    pub port: Option<String>,
    /// Keep observations whose source or destination contains this text.
    #[arg(long)]
    pub ip: Option<String>,
    /// Keep observations with at least this many bytes.
    #[arg(long, default_value_t = 0)]
    pub min_bytes: i64,
    /// A flow log is Stale when its newest blob is older than this many hours.
    #[arg(long, default_value_t = 3)]
    pub stale_hours: u32,
    /// html (default) or pdf.
    #[arg(long, default_value = "html")]
    pub report: String,
    /// Report file path. Default: flowlog-report-<yyyyMMddHHmm>.<html|pdf> in the current directory.
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
    /// Offline only: JSON file mapping address prefixes to storage accounts or VNets.
    #[arg(long)]
    pub topology: Option<PathBuf>,
    /// Parallel blob downloads.
    #[arg(long, default_value_t = 8)]
    pub concurrency: usize,
    /// Print progress to stderr.
    #[arg(long, short = 'v')]
    pub verbose: bool,
}
