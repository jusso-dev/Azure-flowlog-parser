mod analysis;
mod azure;
mod cli;
mod commands;
mod config;
mod diagnosis;
mod discovery;
mod flowlog;
mod model;
mod report;
mod util;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Some(Command::Discover(args)) => commands::discover::run(args).await,
        Some(Command::Diagnose(args)) => commands::diagnose::run(args).await,
        Some(Command::Report(args)) => commands::report::run(args).await,
        None => commands::parse::run(&cli.parse).await,
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::from(1)
        }
    }
}
