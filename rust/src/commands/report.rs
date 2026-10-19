//! Owner: core agent. Orchestrates the report command for both Azure and offline input.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeDelta, Utc};

use crate::analysis::aggregate::TrafficAggregator;
use crate::analysis::filter::{TrafficFilter, parse_window_bound};
use crate::analysis::{topology, topology_file};
use crate::azure::{AzureClient, read_flow_blobs};
use crate::cli::ReportArgs;
use crate::discovery::{InventoryRequest, load_inventory};
use crate::flowlog::parse_flow_log;
use crate::model::{
    InventoryResult, MAX_ROWS, ReportFilters, ReportModel, ReportSubscription, ReportWindow, SCHEMA_VERSION,
    flow_status,
};
use crate::report::{html, pdf};
use crate::util::{file_name, iso, thousands};

pub async fn run(args: &ReportArgs) -> Result<u8> {
    let report_format = args.report.to_ascii_lowercase();
    if report_format != "html" && report_format != "pdf" {
        bail!("--report must be html or pdf.");
    }
    let offline = !args.inputs.is_empty();
    if offline && (!args.subscriptions.is_empty() || !args.storage_account.is_empty()) {
        bail!("--input cannot be combined with --subscription or --storage-account.");
    }

    let now = Utc::now();
    let default_since = now - TimeDelta::hours(24);
    let since = args.since.as_deref().map(|value| parse_window_bound(value, now)).transpose()?;
    let until = args.until.as_deref().map(|value| parse_window_bound(value, now)).transpose()?;
    // Azure reads default to the last 24 hours, ending now. Offline input defaults to the observed range.
    let (since, until) =
        if offline { (since, until) } else { (Some(since.unwrap_or(default_since)), Some(until.unwrap_or(now))) };
    if let (Some(from), Some(to)) = (since, until) {
        if from >= to {
            bail!("--since must be earlier than --until.");
        }
    }

    let filter = TrafficFilter::create(
        Some(args.action.as_str()),
        Some(args.protocol.as_str()),
        args.port.as_deref(),
        args.ip.as_deref(),
        args.min_bytes,
    )?;
    let concurrency = args.concurrency.max(1);
    let verbose = args.verbose;
    let log = move |message: &str| {
        // Warnings always print; progress only with --verbose.
        if verbose || message.starts_with("Warning:") {
            eprintln!("{message}");
        }
    };

    let mut model = ReportModel {
        schema: SCHEMA_VERSION,
        generated_utc: iso(now),
        filters: ReportFilters {
            action: filter.action.clone(),
            protocol: filter.protocol.clone(),
            ports: filter.ports.clone(),
            ip: filter.ip.clone(),
            min_bytes: filter.min_bytes,
        },
        ..Default::default()
    };

    if offline {
        model.source = "local".to_string();
        let (lookup, storage_accounts) = match &args.topology {
            Some(path) => {
                let loaded = topology_file::load(path)?;
                let skipped = if loaded.skipped_prefixes > 0 {
                    format!(", {} non-IPv4 prefix(es) skipped.", loaded.skipped_prefixes)
                } else {
                    ".".to_string()
                };
                model.notes.push(format!(
                    "Offline topology from {}: {} network(s), {} prefix(es){skipped}",
                    file_name(path),
                    loaded.networks,
                    loaded.prefixes
                ));
                model.subscriptions = loaded.subscriptions;
                model.notes.extend(loaded.notes);
                // Assumes the inputs cover every storage network in the topology (same dedupe as Azure mode).
                (Some(loaded.lookup), loaded.storage_accounts)
            }
            None => {
                model.notes.push(
                    "Local input: no Azure topology was loaded. Public addresses are 'internet'; private addresses are 'unmapped'."
                        .to_string(),
                );
                (None, Vec::new())
            }
        };

        let mut aggregator = TrafficAggregator::new(filter, since, until, lookup, storage_accounts);
        let files = read_local(&args.inputs, &mut aggregator, &log)?;
        log(&format!("Read {files} local file(s)."));
        aggregator.fill_model(&mut model, 0);
        model.window = window_for(since, until, aggregator.observed_range(), now);
    } else {
        model.source = "azure".to_string();
        if args.topology.is_some() {
            log("Warning: --topology is only used with --input; ignoring it.");
        }
        let from = since.unwrap_or(default_since);
        let to = until.unwrap_or(now);

        let client = AzureClient::new()?;
        let request = InventoryRequest {
            subscriptions: args.subscriptions.clone(),
            storage_accounts: args.storage_account.clone(),
            stale_hours: args.stale_hours,
            include_all: false,
        };
        let load = load_inventory(&client, &request, &log).await?;

        let mut notes = Vec::new();
        let lookup = topology::build_lookup(&load.topology, &mut notes);
        let aggregator =
            Mutex::new(TrafficAggregator::new(filter, Some(from), Some(to), Some(lookup), read_accounts(&load.result)));
        let read = read_flow_blobs(&client, &load.result, from, to, &aggregator, concurrency, &log).await?;
        log(&format!("Read {} blob(s).", read.blobs));
        let aggregator = aggregator.into_inner().unwrap_or_else(PoisonError::into_inner);

        model.subscriptions = load.topology.subscriptions.iter().map(ReportSubscription::from).collect();
        model.accounts = load.result.accounts;
        model.flow_logs = load.result.flow_logs;
        aggregator.fill_model(&mut model, load.topology.subscriptions.len());
        model.window = window_for(Some(from), Some(to), aggregator.observed_range(), now);

        model.notes.extend(notes);
        if !read.failed_accounts.is_empty() {
            model.notes.push(format!(
                "Reads failed for {}: their outbound observations are missing, so inbound allowed duplicates from them were still skipped. Totals are lower than the true traffic.",
                read.failed_accounts.join(", ")
            ));
        }
        model.notes.push(
            "Scope mapping: longest prefix match over NIC, subnet and VNet address ranges. Public addresses outside all known ranges are 'internet'; private ones are 'unmapped'."
                .to_string(),
        );
        model.notes.extend(load.topology.warnings.iter().map(|warning| format!("Discovery warning: {warning}")));
    }

    add_summary_notes(&mut model);

    let output = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("flowlog-report-{}.{report_format}", now.format("%Y%m%d%H%M"))));
    if report_format == "pdf" {
        pdf::write(&model, &output)?;
    } else {
        html::write(&model, &output)?;
    }

    eprintln!(
        "Report written: {} ({} observations, {} rows)",
        std::path::absolute(&output)?.display(),
        thousands(model.summary.flows),
        thousands(model.summary.rows_total as i64)
    );
    Ok(0)
}

/// Reads local PT1H.json files (or every .json below a directory). Unreadable or malformed files are skipped with a
/// warning, as in TrafficReader.ReadLocal.
fn read_local(inputs: &[PathBuf], aggregator: &mut TrafficAggregator, log: &(dyn Fn(&str) + Sync)) -> Result<usize> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_dir() {
            collect_json(input, &mut files)?;
        } else if input.is_file() {
            files.push(input.clone());
        } else {
            bail!("Input not found: {}", input.display());
        }
    }
    files.sort_by_key(|file| file.to_string_lossy().into_owned());

    let mut processed = 0;
    for file in &files {
        let parsed = std::fs::read_to_string(file).map_err(anyhow::Error::from).and_then(|text| parse_flow_log(&text));
        match parsed {
            Ok(records) => {
                aggregator.add_records(&records);
                processed += 1;
            }
            Err(error) => log(&format!("Warning: skipped {}: {error}", file.display())),
        }
    }
    Ok(processed)
}

fn collect_json(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_json(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "json") {
            files.push(path);
        }
    }
    Ok(())
}

/// Storage accounts whose flow logs this run reads: enabled configurations plus unmanaged accounts, distinct
/// ignoring case.
fn read_accounts(inventory: &InventoryResult) -> Vec<String> {
    let candidates =
        inventory.flow_logs.iter().filter(|config| config.enabled).map(|config| &config.storage_account).chain(
            inventory
                .accounts
                .iter()
                .filter(|account| account.status == flow_status::UNMANAGED)
                .map(|account| &account.name),
        );

    let mut accounts: Vec<String> = Vec::new();
    for name in candidates {
        if !accounts.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            accounts.push(name.clone());
        }
    }
    accounts
}

/// Explicit bounds win; open bounds take the observed range, or the generation time when nothing was kept.
fn window_for(
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    observed: (Option<DateTime<Utc>>, Option<DateTime<Utc>>),
    now: DateTime<Utc>,
) -> ReportWindow {
    let from = since.or(observed.0).unwrap_or(now);
    let to = until.or(observed.1).unwrap_or(now);
    ReportWindow { from_utc: iso(from), to_utc: iso(to) }
}

fn add_summary_notes(model: &mut ReportModel) {
    model.notes.insert(
        0,
        "Counts are flow log observations: one per NSG or VNet flow tuple. A flow crossing two logged networks is observed at both; the duplicate allowed inbound observation is skipped."
            .to_string(),
    );

    if model.summary.rows_truncated {
        model.notes.push(format!(
            "Flow rows capped at {} of {}. Totals still include every observation.",
            thousands(MAX_ROWS as i64),
            thousands(model.summary.rows_total as i64)
        ));
    }

    let filters = &model.filters;
    if filters.action != "all"
        || filters.protocol != "all"
        || !filters.ports.is_empty()
        || !filters.ip.is_empty()
        || filters.min_bytes > 0
    {
        model.notes.push(
            "Filters apply per observation: port matches destination port; IP matches source or destination text."
                .to_string(),
        );
    }
}
