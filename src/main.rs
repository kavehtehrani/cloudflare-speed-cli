mod cli;
mod engine;
mod metrics;
mod model;
mod network;
mod stats;
mod storage;
#[cfg(feature = "tui")]
mod tui;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Cli::parse();
    cli::run(args).await
}
