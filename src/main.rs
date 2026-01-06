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
    let is_silent = args.silent;
    let is_non_tui = args.silent || args.json || args.text;

    match cli::run(args).await {
        Ok(()) => {
            // Explicitly exit with code 0 on success, especially for non-TUI modes
            if is_non_tui {
                std::process::exit(0);
            }
            Ok(())
        }
        Err(e) => {
            if is_silent {
                println!("{}", e);
                std::process::exit(1);
            } else {
                Err(e)
            }
        }
    }
}
