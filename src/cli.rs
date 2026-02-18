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

    /// Reserved for future experimental features
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

    /// Route traffic through a proxy (HTTP, HTTPS, or SOCKS5)
    #[arg(long)]
    pub proxy: Option<String>,

    /// Path to a custom TLS certificate file (PEM or DER format)
    #[arg(long)]
    pub certificate: Option<std::path::PathBuf>,

    /// Automatically start a test when the app launches
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub test_on_launch: bool,

    /// Attach custom comments to this run
    #[arg(long)]
    pub comments: Option<String>,

    /// Compare IPv4 vs IPv6 performance
    #[arg(long)]
    pub compare_ip_versions: bool,

    /// Run traceroute to Cloudflare edge
    #[arg(long)]
    pub traceroute: bool,

    /// Maximum number of hops for traceroute
    #[arg(long, default_value_t = 30)]
    pub traceroute_max_hops: u8,

    /// Force IPv4 only (no IPv6)
    #[arg(long)]
    pub ipv4_only: bool,

    /// Force IPv6 only (no IPv4)
    #[arg(long)]
    pub ipv6_only: bool,

    /// Skip default diagnostic measurements (DNS, TLS)
    #[arg(long)]
    pub skip_diagnostics: bool,

    /// Number of UDP packets to send for packet loss measurement
    #[arg(long, default_value_t = 50)]
    pub udp_packets: u64,
}

pub async fn run(args: Cli) -> Result<()> {
    // Validate that --silent can only be used with --json
    if args.silent && !args.json {
        return Err(anyhow::anyhow!(
            "--silent can only be used with --json. Use --silent --json together."
        ));
    }

    // Warn when using a proxy
    if let Some(ref proxy_url) = args.proxy {
        eprintln!(
            "Warning: using proxy {}. Speed results reflect performance through the proxy, not your direct connection.",
            proxy_url
        );
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
    // DNS and TLS run by default unless --skip-diagnostics is set
    let skip = args.skip_diagnostics;
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
        proxy: args.proxy.clone(),
        certificate_path: args.certificate.clone(),
        // Diagnostic options: DNS and TLS run by default unless --skip-diagnostics
        measure_dns: !skip,
        measure_tls: !skip,
        compare_ip_versions: args.compare_ip_versions,
        traceroute: args.traceroute,
        traceroute_max_hops: args.traceroute_max_hops,
        ipv4_only: args.ipv4_only,
        ipv6_only: args.ipv6_only,
        udp_packets: args.udp_packets,
    }
}

/// Common function to run the test engine and process results.
/// `silent` controls whether to consume events and suppress output.
async fn run_test_engine(args: Cli, silent: bool) -> Result<()> {
    let cfg = build_config(&args);
    let network_info = crate::network::gather_network_info(&args);
    let enriched = if silent {
        // In silent mode, spawn task and consume events
        let (evt_tx, mut evt_rx) = mpsc::channel::<TestEvent>(2048);
        let (_, ctrl_rx) = mpsc::channel::<EngineControl>(16);

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
        let (evt_tx, _) = mpsc::channel::<TestEvent>(1024);
        let (_, ctrl_rx) = mpsc::channel::<EngineControl>(16);

        let engine = TestEngine::new(cfg);
        let result = engine
            .run(evt_tx, ctrl_rx)
            .await
            .context("speed test failed")?;

        crate::network::enrich_result(&result, &network_info)
    };

    // Handle exports (errors will propagate)
    handle_exports(&args, &enriched)?;

    if !silent {
        // Print JSON output in non-silent mode
        println!("{}", serde_json::to_string_pretty(&enriched)?);
    }

    // Save results if auto_save is enabled
    if args.auto_save {
        if silent {
            crate::storage::save_run(&enriched).context("failed to save run results")?;
        } else {
            if let Ok(p) = crate::storage::save_run(&enriched) {
                eprintln!("Saved: {}", p.display());
            }
        }
    }

    Ok(())
}

async fn run_text(args: Cli) -> Result<()> {
    let cfg = build_config(&args);
    let (evt_tx, mut evt_rx) = mpsc::channel::<TestEvent>(2048);
    let (_, ctrl_rx) = mpsc::channel::<EngineControl>(16);

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
            TestEvent::UdpLossProgress {
                sent,
                received,
                total,
                rtt_ms,
            } => {
                let loss_pct = if sent == 0 {
                    0.0
                } else {
                    ((sent.saturating_sub(received)) as f64) * 100.0 / sent as f64
                };
                let rtt_display = rtt_ms
                    .map(|v| format!("{:.1}ms", v))
                    .unwrap_or_else(|| "timeout".to_string());
                eprintln!(
                    "Packet loss probe: {}/{} recv {} loss {:.1}% ({})",
                    sent, total, received, loss_pct, rtt_display
                );
            }
            TestEvent::MetaInfo { .. } => {
                // Meta info is handled in TUI, ignore in text mode
            }
            // Diagnostic events
            TestEvent::DiagnosticDns { summary } => {
                eprintln!("DNS: {:.2}ms", summary.resolution_time_ms);
            }
            TestEvent::DiagnosticTls { summary } => {
                eprintln!(
                    "TLS: handshake {:.2}ms, {} {}",
                    summary.handshake_time_ms,
                    summary.protocol_version.as_deref().unwrap_or("-"),
                    summary.cipher_suite.as_deref().unwrap_or("-")
                );
            }
            TestEvent::DiagnosticIpComparison { comparison } => {
                if let Some(ref v4) = comparison.ipv4_result {
                    if v4.available {
                        eprintln!(
                            "IPv4: {} - DL {:.2} Mbps, UL {:.2} Mbps, latency {:.1}ms",
                            v4.ip_address, v4.download_mbps, v4.upload_mbps, v4.latency_ms
                        );
                    } else {
                        eprintln!("IPv4: unavailable - {:?}", v4.error);
                    }
                }
                if let Some(ref v6) = comparison.ipv6_result {
                    if v6.available {
                        eprintln!(
                            "IPv6: {} - DL {:.2} Mbps, UL {:.2} Mbps, latency {:.1}ms",
                            v6.ip_address, v6.download_mbps, v6.upload_mbps, v6.latency_ms
                        );
                    } else {
                        eprintln!("IPv6: unavailable - {:?}", v6.error);
                    }
                }
            }
            TestEvent::TracerouteHop { hop_number, hop } => {
                let addr = hop.ip_address.as_deref().unwrap_or("*");
                let rtts: Vec<String> = hop.rtt_ms.iter().map(|r| format!("{:.1}ms", r)).collect();
                let rtt_str = if rtts.is_empty() {
                    "*".to_string()
                } else {
                    rtts.join(" ")
                };
                eprintln!("{:>2}  {} {}", hop_number, addr, rtt_str);
            }
            TestEvent::TracerouteComplete { summary } => {
                eprintln!(
                    "Traceroute to {} {} ({} hops)",
                    summary.destination,
                    if summary.completed {
                        "completed"
                    } else {
                        "incomplete"
                    },
                    summary.hops.len()
                );
            }
            TestEvent::ExternalIps { ipv4, ipv6 } => {
                let v4 = ipv4.as_deref().unwrap_or("-");
                let v6 = ipv6.as_deref().unwrap_or("-");
                eprintln!("External IPs: v4={} v6={}", v4, v6);
            }
        }
    }

    let result = handle.await??;

    // Gather network information and enrich result
    let network_info = crate::network::gather_network_info(&args);
    let enriched = crate::network::enrich_result(&result, &network_info);

    handle_exports(&args, &enriched)?;
    if let Some(meta) = enriched.meta.as_ref() {
        let extracted = crate::network::extract_metadata(meta);
        let ip = extracted.ip.as_deref().unwrap_or("-");
        let colo = extracted.colo.as_deref().unwrap_or("-");
        let asn = extracted.asn.as_deref().unwrap_or("-");
        let org = extracted.as_org.as_deref().unwrap_or("-");
        println!("IP/Colo/ASN: {ip} / {colo} / {asn} ({org})");
    }
    if let Some(server) = enriched.server.as_deref() {
        println!("Server: {server}");
    }
    if let Some(comments) = enriched.comments.as_deref() {
        if !comments.trim().is_empty() {
            println!("Comments: {}", comments);
        }
    }

    // Compute and display throughput metrics (mean, median, p25, p75)
    let dl_values: Vec<f64> = dl_points.iter().map(|(_, y)| *y).collect();
    let (dl_mean, dl_median, dl_p25, dl_p75) = crate::metrics::compute_metrics(&dl_values)
        .context("insufficient download throughput data to compute metrics")?;
    println!(
        "Download: avg {:.2} med {:.2} p25 {:.2} p75 {:.2}",
        dl_mean, dl_median, dl_p25, dl_p75
    );

    let ul_values: Vec<f64> = ul_points.iter().map(|(_, y)| *y).collect();
    let (ul_mean, ul_median, ul_p25, ul_p75) = crate::metrics::compute_metrics(&ul_values)
        .context("insufficient upload throughput data to compute metrics")?;
    println!(
        "Upload:   avg {:.2} med {:.2} p25 {:.2} p75 {:.2}",
        ul_mean, ul_median, ul_p25, ul_p75
    );

    // Compute and display latency metrics (mean, median, p25, p75)
    let (idle_mean, idle_median, idle_p25, idle_p75) =
        crate::metrics::compute_metrics(&idle_latency_samples)
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
        crate::metrics::compute_metrics(&loaded_dl_latency_samples)
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
        crate::metrics::compute_metrics(&loaded_ul_latency_samples)
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
        let mos_str = exp.mos.map(|m| format!("MOS {:.1}", m)).unwrap_or_else(|| "N/A".to_string());
        let jitter_str = exp.latency.jitter_ms.map(|j| format!("{:.1}ms", j)).unwrap_or_else(|| "-".to_string());
        println!(
            "UDP quality: {} ({}) | loss {:.1}% jitter {} reorder {:.1}% rtt {}ms",
            exp.quality_label,
            mos_str,
            exp.latency.loss * 100.0,
            jitter_str,
            exp.out_of_order_pct,
            exp.latency.median_ms.unwrap_or(f64::NAN)
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
