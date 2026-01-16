use crate::engine::{EngineControl, TestEngine};
use crate::model::{RunConfig, TestEvent};
use anyhow::{Context, Result};
use clap::Parser;
use rand::RngCore;
use std::io::Write;
use std::time::Duration;
use tokio::sync::mpsc;

/// Output line routing for stdout/stderr writer.
enum OutputLine {
    Stdout(String),
    Stderr(String),
}

/// Spawn a blocking writer for stdout/stderr to avoid blocking async tasks.
fn spawn_output_writer() -> (
    mpsc::UnboundedSender<OutputLine>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<OutputLine>();
    let handle = tokio::task::spawn_blocking(move || {
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let mut out = std::io::LineWriter::new(stdout.lock());
        let mut err = std::io::LineWriter::new(stderr.lock());

        while let Some(line) = rx.blocking_recv() {
            match line {
                OutputLine::Stdout(msg) => {
                    let _ = writeln!(out, "{}", msg);
                }
                OutputLine::Stderr(msg) => {
                    let _ = writeln!(err, "{}", msg);
                }
            }
        }

        let _ = out.flush();
        let _ = err.flush();
    });
    (tx, handle)
}

#[derive(Debug, Parser, Clone)]
#[command(
    name = "cloudflare-speed-cli",
    version,
    about = "Cloudflare-based speed test with optional TUI"
)]
pub struct Cli {
    /// Base URL for the Cloudflare speed test service
    #[arg(long, default_value = "https://speed.cloudflare.com")]
    pub base_url: String,

    /// Print JSON result and exit (no TUI)
    #[arg(long)]
    pub json: bool,

    /// Print text summary and exit (no TUI)
    #[arg(long)]
    pub text: bool,

    /// Run silently: suppress all output except errors (for cron usage)
    #[arg(long)]
    pub silent: bool,

    /// Download phase duration
    #[arg(long, default_value = "10s")]
    pub download_duration: humantime::Duration,

    /// Upload phase duration
    #[arg(long, default_value = "10s")]
    pub upload_duration: humantime::Duration,

    /// Idle latency probe duration (pre-test)
    #[arg(long, default_value = "2s")]
    pub idle_latency_duration: humantime::Duration,

    /// Concurrency for download/upload workers
    #[arg(long, default_value_t = 6)]
    pub concurrency: usize,

    /// Bytes per download request
    #[arg(long, default_value_t = 10_000_000)]
    pub download_bytes_per_req: u64,

    /// Bytes per upload request
    #[arg(long, default_value_t = 5_000_000)]
    pub upload_bytes_per_req: u64,

    /// Probe interval in milliseconds
    #[arg(long, default_value_t = 250)]
    pub probe_interval_ms: u64,

    /// Probe timeout in milliseconds
    #[arg(long, default_value_t = 800)]
    pub probe_timeout_ms: u64,

    /// Enable experimental features (TURN fetch + UDP-like loss probe)
    #[arg(long)]
    pub experimental: bool,

    /// Export results as JSON
    #[arg(long)]
    pub export_json: Option<std::path::PathBuf>,

    /// Export results as CSV
    #[arg(long)]
    pub export_csv: Option<std::path::PathBuf>,

    /// Use --auto-save true or --auto-save false to override
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub auto_save: bool,

    /// Bind to a specific network interface (e.g., ens18, eth0)
    #[arg(long)]
    pub interface: Option<String>,

    /// Bind to a specific source IP address (e.g., 192.168.10.0)
    #[arg(long)]
    pub source: Option<String>,

    /// Path to a custom TLS certificate file (PEM or DER format)
    #[arg(long)]
    pub certificate: Option<std::path::PathBuf>,

    /// Automatically start a test when the app launches
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub test_on_launch: bool,

    /// Attach custom comments to this run
    #[arg(long)]
    pub comments: Option<String>,
}

pub async fn run(args: Cli) -> Result<()> {
    // Validate that --silent can only be used with --json
    if args.silent && !args.json {
        return Err(anyhow::anyhow!(
            "--silent can only be used with --json. Use --silent --json together."
        ));
    }

    // Silent mode takes precedence over other output modes
    if args.silent {
        return run_test_engine(args, true).await;
    }

    if !args.json && !args.text {
        #[cfg(feature = "tui")]
        {
            return crate::tui::run(args).await;
        }
        #[cfg(not(feature = "tui"))]
        {
            // Fallback when built without TUI support.
            return run_text(args).await;
        }
    }

    if args.json {
        return run_test_engine(args, false).await;
    }

    run_text(args).await
}

/// Generate a random measurement ID for the speed test.
fn gen_meas_id() -> String {
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    u64::from_le_bytes(b).to_string()
}

/// Build a `RunConfig` from CLI arguments.
pub fn build_config(args: &Cli) -> RunConfig {
    RunConfig {
        base_url: args.base_url.clone(),
        meas_id: gen_meas_id(),
        comments: args.comments.clone(),
        download_bytes_per_req: args.download_bytes_per_req,
        upload_bytes_per_req: args.upload_bytes_per_req,
        concurrency: args.concurrency,
        idle_latency_duration: Duration::from(args.idle_latency_duration),
        download_duration: Duration::from(args.download_duration),
        upload_duration: Duration::from(args.upload_duration),
        probe_interval_ms: args.probe_interval_ms,
        probe_timeout_ms: args.probe_timeout_ms,
        user_agent: format!("cloudflare-speed-cli/{}", env!("CARGO_PKG_VERSION")),
        experimental: args.experimental,
        interface: args.interface.clone(),
        source_ip: args.source.clone(),
        certificate_path: args.certificate.clone(),
    }
}

/// Common function to run the test engine and process results.
/// `silent` controls whether to consume events and suppress output.
async fn run_test_engine(args: Cli, silent: bool) -> Result<()> {
    let cfg = build_config(&args);
    let network_info = crate::network::gather_network_info(&args);
    let (out_tx, out_handle) = if silent {
        (None, None)
    } else {
        let (tx, handle) = spawn_output_writer();
        (Some(tx), Some(handle))
    };
    let enriched = if silent {
        // In silent mode, spawn task and consume events
        let (evt_tx, mut evt_rx) = mpsc::unbounded_channel::<TestEvent>();
        let (_, ctrl_rx) = mpsc::unbounded_channel::<EngineControl>();

        let engine = TestEngine::new(cfg);
        let handle = tokio::spawn(async move { engine.run(evt_tx, ctrl_rx).await });

        // Consume events silently (no output)
        while let Some(_ev) = evt_rx.recv().await {
            // All events are silently consumed - no output
        }

        let result = handle
            .await
            .context("test engine task failed")?
            .context("speed test failed")?;

        crate::network::enrich_result(&result, &network_info)
    } else {
        // In JSON mode, directly await the engine (no need to consume events)
        let (evt_tx, _) = mpsc::unbounded_channel::<TestEvent>();
        let (_, ctrl_rx) = mpsc::unbounded_channel::<EngineControl>();

        let engine = TestEngine::new(cfg);
        let result = engine
            .run(evt_tx, ctrl_rx)
            .await
            .context("speed test failed")?;

        crate::network::enrich_result(&result, &network_info)
    };

    // Handle exports (errors will propagate)
    handle_exports(&args, &enriched)?;

    if let Some(tx) = out_tx.as_ref() {
        // Print JSON output in non-silent mode
        let out = serde_json::to_string_pretty(&enriched)?;
        let _ = tx.send(OutputLine::Stdout(out));
    }

    // Save results if auto_save is enabled
    if args.auto_save {
        if silent {
            crate::storage::save_run(&enriched).context("failed to save run results")?;
        } else if let Some(tx) = out_tx.as_ref() {
            if let Ok(p) = crate::storage::save_run(&enriched) {
                let _ = tx.send(OutputLine::Stderr(format!("Saved: {}", p.display())));
            }
        }
    }

    if let Some(tx) = out_tx {
        drop(tx);
    }
    if let Some(handle) = out_handle {
        let _ = handle.await;
    }

    Ok(())
}

async fn run_text(args: Cli) -> Result<()> {
    let cfg = build_config(&args);
    let (out_tx, out_handle) = spawn_output_writer();
    let (evt_tx, mut evt_rx) = mpsc::unbounded_channel::<TestEvent>();
    let (_, ctrl_rx) = mpsc::unbounded_channel::<EngineControl>();

    let engine = TestEngine::new(cfg);
    let handle = tokio::spawn(async move { engine.run(evt_tx, ctrl_rx).await });

    // Collect raw samples for metric computation (same as TUI)
    let run_start = std::time::Instant::now();
    let mut idle_latency_samples: Vec<f64> = Vec::new();
    let mut loaded_dl_latency_samples: Vec<f64> = Vec::new();
    let mut loaded_ul_latency_samples: Vec<f64> = Vec::new();
    let mut dl_points: Vec<(f64, f64)> = Vec::new();
    let mut ul_points: Vec<(f64, f64)> = Vec::new();

    while let Some(ev) = evt_rx.recv().await {
        match ev {
            TestEvent::PhaseStarted { phase } => {
                let _ = out_tx.send(OutputLine::Stderr(format!("== {phase:?} ==")));
            }
            TestEvent::ThroughputTick {
                phase,
                bps_instant,
                bytes_total: _,
            } => {
                if matches!(
                    phase,
                    crate::model::Phase::Download | crate::model::Phase::Upload
                ) {
                    let elapsed = run_start.elapsed().as_secs_f64();
                    let mbps = (bps_instant * 8.0) / 1_000_000.0;
                    let _ = out_tx.send(OutputLine::Stderr(format!("{phase:?}: {:.2} Mbps", mbps)));

                    // Collect throughput points for metrics
                    match phase {
                        crate::model::Phase::Download => {
                            dl_points.push((elapsed, mbps));
                        }
                        crate::model::Phase::Upload => {
                            ul_points.push((elapsed, mbps));
                        }
                        _ => {}
                    }
                }
            }
            TestEvent::LatencySample {
                phase,
                ok,
                rtt_ms,
                during,
            } => {
                if ok {
                    if let Some(ms) = rtt_ms {
                        match (phase, during) {
                            (crate::model::Phase::IdleLatency, None) => {
                                let _ = out_tx.send(OutputLine::Stderr(format!(
                                    "Idle latency: {:.1} ms",
                                    ms
                                )));
                                idle_latency_samples.push(ms);
                            }
                            (
                                crate::model::Phase::Download,
                                Some(crate::model::Phase::Download),
                            ) => {
                                loaded_dl_latency_samples.push(ms);
                            }
                            (crate::model::Phase::Upload, Some(crate::model::Phase::Upload)) => {
                                loaded_ul_latency_samples.push(ms);
                            }
                            _ => {}
                        }
                    }
                }
            }
            TestEvent::Info(info) => {
                let _ = out_tx.send(OutputLine::Stderr(info.to_message()));
            }
            TestEvent::MetaInfo { .. } => {
                // Meta info is handled in TUI, ignore in text mode
            }
            TestEvent::RunCompleted { .. } => {}
        }
    }

    let result = handle.await??;

    // Gather network information and enrich result
    let network_info = crate::network::gather_network_info(&args);
    let enriched = crate::network::enrich_result(&result, &network_info);

    handle_exports(&args, &enriched)?;
    let summary = crate::text_summary::build_text_summary(
        &enriched,
        &dl_points,
        &ul_points,
        &idle_latency_samples,
        &loaded_dl_latency_samples,
        &loaded_ul_latency_samples,
    )?;
    for line in summary.lines {
        let _ = out_tx.send(OutputLine::Stdout(line));
    }
    if args.auto_save {
        if let Ok(p) = crate::storage::save_run(&enriched) {
            let _ = out_tx.send(OutputLine::Stderr(format!("Saved: {}", p.display())));
        }
    }
    drop(out_tx);
    let _ = out_handle.await;
    Ok(())
}

/// Handle export operations (JSON and CSV) for both text and JSON modes.
fn handle_exports(args: &Cli, result: &crate::model::RunResult) -> Result<()> {
    if let Some(p) = args.export_json.as_deref() {
        crate::storage::export_json(p, result)?;
    }
    if let Some(p) = args.export_csv.as_deref() {
        crate::storage::export_csv(p, result)?;
    }
    Ok(())
}
