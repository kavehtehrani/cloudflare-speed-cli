use crate::engine::{EngineControl, TestEngine};
use crate::model::{RunConfig, TestEvent};
use anyhow::{Context, Result};
use clap::Parser;
use rand::RngCore;
use std::time::Duration;
use tokio::sync::mpsc;

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

    /// Use --auto-save true or --auto-save false to override (default: true)
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
}

pub async fn run(args: Cli) -> Result<()> {
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
        return run_json(args).await;
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

async fn run_json(args: Cli) -> Result<()> {
    let cfg = build_config(&args);
    let (evt_tx, _evt_rx) = mpsc::channel::<TestEvent>(1024);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<EngineControl>(16);
    drop(ctrl_tx);
    drop(_evt_rx); // Not used in JSON mode

    let engine = TestEngine::new(cfg);
    let result = engine
        .run(evt_tx, ctrl_rx)
        .await
        .context("speed test failed")?;

    // Gather network information and enrich result
    let network_info = crate::network::gather_network_info(&args);
    let enriched = crate::network::enrich_result(&result, &network_info);

    handle_exports(&args, &enriched)?;

    println!("{}", serde_json::to_string_pretty(&enriched)?);
    if args.auto_save {
        if let Ok(p) = crate::storage::save_run(&enriched) {
            eprintln!("Saved: {}", p.display());
        }
    }
    Ok(())
}

async fn run_text(args: Cli) -> Result<()> {
    let cfg = build_config(&args);
    let (evt_tx, mut evt_rx) = mpsc::channel::<TestEvent>(2048);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<EngineControl>(16);
    drop(ctrl_tx);

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
                eprintln!("== {phase:?} ==");
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
                    eprintln!("{phase:?}: {:.2} Mbps", mbps);

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
                                eprintln!("Idle latency: {:.1} ms", ms);
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
            TestEvent::Info { message } => eprintln!("{message}"),
            TestEvent::MetaInfo { .. } => {
                // Meta info is handled in TUI, ignore in text mode
            }
        }
    }

    let result = handle.await??;

    // Gather network information and enrich result
    let network_info = crate::network::gather_network_info(&args);
    let enriched = crate::network::enrich_result(&result, &network_info);

    handle_exports(&args, &enriched)?;
    if let Some(meta) = enriched.meta.as_ref() {
        let ip = meta.get("clientIp").and_then(|v| v.as_str()).unwrap_or("-");
        let colo = meta.get("colo").and_then(|v| v.as_str()).unwrap_or("-");
        let asn = meta
            .get("asn")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        let org = meta
            .get("asOrganization")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!("IP/Colo/ASN: {ip} / {colo} / {asn} ({org})");
    }
    if let Some(server) = enriched.server.as_deref() {
        println!("Server: {server}");
    }

    // Compute and display throughput metrics (mean, median, p25, p75)
    let (dl_mean, dl_median, dl_p25, dl_p75) =
        crate::metrics::compute_throughput_metrics(&dl_points)
            .context("insufficient download throughput data to compute metrics")?;
    println!(
        "Download: avg {:.2} med {:.2} p25 {:.2} p75 {:.2}",
        dl_mean, dl_median, dl_p25, dl_p75
    );

    let (ul_mean, ul_median, ul_p25, ul_p75) =
        crate::metrics::compute_throughput_metrics(&ul_points)
            .context("insufficient upload throughput data to compute metrics")?;
    println!(
        "Upload:   avg {:.2} med {:.2} p25 {:.2} p75 {:.2}",
        ul_mean, ul_median, ul_p25, ul_p75
    );

    // Compute and display latency metrics (mean, median, p25, p75)
    let (idle_mean, idle_median, idle_p25, idle_p75) =
        crate::metrics::compute_latency_metrics(&idle_latency_samples)
            .context("insufficient idle latency data to compute metrics")?;
    println!(
        "Idle latency: avg {:.1} med {:.1} p25 {:.1} p75 {:.1} ms (loss {:.1}%, jitter {:.1} ms)",
        idle_mean,
        idle_median,
        idle_p25,
        idle_p75,
        enriched.idle_latency.loss * 100.0,
        enriched.idle_latency.jitter_ms.unwrap_or(f64::NAN)
    );

    let (dl_lat_mean, dl_lat_median, dl_lat_p25, dl_lat_p75) =
        crate::metrics::compute_latency_metrics(&loaded_dl_latency_samples)
            .context("insufficient loaded download latency data to compute metrics")?;
    println!(
        "Loaded latency (download): avg {:.1} med {:.1} p25 {:.1} p75 {:.1} ms (loss {:.1}%, jitter {:.1} ms)",
        dl_lat_mean,
        dl_lat_median,
        dl_lat_p25,
        dl_lat_p75,
        enriched.loaded_latency_download.loss * 100.0,
        enriched.loaded_latency_download.jitter_ms.unwrap_or(f64::NAN)
    );

    let (ul_lat_mean, ul_lat_median, ul_lat_p25, ul_lat_p75) =
        crate::metrics::compute_latency_metrics(&loaded_ul_latency_samples)
            .context("insufficient loaded upload latency data to compute metrics")?;
    println!(
        "Loaded latency (upload): avg {:.1} med {:.1} p25 {:.1} p75 {:.1} ms (loss {:.1}%, jitter {:.1} ms)",
        ul_lat_mean,
        ul_lat_median,
        ul_lat_p25,
        ul_lat_p75,
        enriched.loaded_latency_upload.loss * 100.0,
        enriched.loaded_latency_upload.jitter_ms.unwrap_or(f64::NAN)
    );
    if let Some(ref exp) = enriched.experimental_udp {
        println!(
            "Experimental UDP-like loss probe: loss {:.1}% p50 {} ms (target {:?})",
            exp.latency.loss * 100.0,
            exp.latency.p50_ms.unwrap_or(f64::NAN),
            exp.target
        );
    }
    if args.auto_save {
        if let Ok(p) = crate::storage::save_run(&enriched) {
            eprintln!("Saved: {}", p.display());
        }
    }
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
