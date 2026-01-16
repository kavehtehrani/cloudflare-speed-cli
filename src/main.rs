mod cli;
mod engine;
mod metrics;
mod model;
mod network;
mod orchestrator;
mod stats;
mod storage;
mod text_summary;
#[cfg(feature = "tui")]
mod tui;

use anyhow::Result;
use clap::Parser;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Cli::parse();
    let is_silent = args.silent;
    let is_non_tui = args.silent || args.json || args.text;

    match cli::run(args).await {
        Ok(()) => {
            if is_non_tui {
                std::process::exit(0);
            }
            Ok(())
        }
        Err(e) => {
            if is_silent {
                let msg = e.to_string();
                let _ = tokio::task::spawn_blocking(move || {
                    let stderr = std::io::stderr();
                    let mut err = std::io::LineWriter::new(stderr.lock());
                    let _ = writeln!(err, "{}", msg);
                    let _ = err.flush();
                })
                .await;
                std::process::exit(1);
            } else {
                Err(e)
            }
        }
    }
}
