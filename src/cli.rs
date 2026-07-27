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
    #[arg(long, default_value_t = 2000)]
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

    /// Bind all test traffic to a specific network interface (e.g. ens18, eth0).
    /// On Linux/macOS this uses device binding (SO_BINDTODEVICE / IP_BOUND_IF),
    /// so both IPv4 and IPv6 destinations stay reachable. On platforms without
    /// device binding (Windows, the BSDs) it instead binds one of the
    /// interface's source IPs — a single address family, preferring IPv6; pass
    /// -4 / -6 to pick the family.
    #[arg(long)]
    pub interface: Option<String>,

    /// Bind all test traffic to a specific source IP address (e.g. 192.168.10.5).
    /// Constrains the test to that address's family (IPv4 or IPv6). Mutually
    /// exclusive with --interface.
    #[arg(long, conflicts_with = "interface")]
    pub source: Option<String>,

    /// Route traffic through a proxy (HTTP, HTTPS, or SOCKS5)
    #[arg(long)]
    pub proxy: Option<String>,

    /// Path to a custom TLS certificate file (PEM or DER format). Not needed if the CA is already trusted by your OS truststore.
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
    #[arg(short = '4', long, conflicts_with = "ipv6_only")]
    pub ipv4_only: bool,

    /// Force IPv6 only (no IPv4)
    #[arg(short = '6', long)]
    pub ipv6_only: bool,

    /// Skip default diagnostic measurements (DNS, TLS)
    #[arg(long)]
    pub skip_diagnostics: bool,

    /// Number of UDP packets to send for packet loss measurement
    #[arg(long, default_value_t = 50)]
    pub udp_packets: u64,

    /// Redact identifying network info (IP, MAC, SSID, ISP, server location) in the TUI display.
    /// Useful for sharing screenshots or recording demos. Toggle at runtime with Shift+H.
    #[arg(long)]
    pub hide_network_info: bool,

    /// Export all saved runs as a single CSV file (no test is run unless combined with a run mode).
    #[arg(long)]
    pub export_all_csv: Option<std::path::PathBuf>,
}

pub async fn run(args: Cli) -> Result<()> {
    if let Some(p) = args.export_all_csv.as_deref() {
        let (all, skipped) = crate::storage::load_all_with_skipped()?;
        crate::storage::export_all_csv(p, &all)?;
        eprintln!("Exported {} run(s) to {}", all.len(), p.display());
        for s in &skipped {
            eprintln!("Warning: skipped unreadable run file {}", s.display());
        }
        // Documented as "no test is run unless combined with a run mode":
        // exit here instead of falling through to the TUI (which would
        // auto-start a test).
        if !(args.json || args.text || args.silent) {
            return Ok(());
        }
    }

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
pub fn build_config(args: &Cli) -> Result<RunConfig> {
    use crate::engine::network_bind;

    // DNS and TLS run by default unless --skip-diagnostics is set
    let skip = args.skip_diagnostics;

    // The requested IP-version restriction (from --ipv4-only / --ipv6-only),
    // validated up front. `None` = unrestricted.
    let family = network_bind::resolve_ip_family(args.ipv4_only, args.ipv6_only, None)?;

    // Bind resolution (--source and --interface are mutually exclusive at the
    // CLI). --source pins a specific local IP. --interface uses device binding
    // (SO_BINDTODEVICE / IP_BOUND_IF) where available, which keeps the run
    // dual-stack with no pinned IP; on platforms without it we instead pin the
    // interface's own source IP for the requested family (preferring IPv6 when
    // unrestricted). Either way the result is a single `resolved_bind_ip` (or
    // None for device binding) consumed by the existing source-IP/family
    // machinery. The user-facing notice is emitted by the caller (see
    // `bind_notice`), NOT here: build_config runs inside the TUI's alternate
    // screen, where stray stderr writes corrupt the display.
    let resolved_bind_ip = if let Some(src) = args.source.as_deref() {
        Some(src.parse().context("Invalid source IP address format")?)
    } else if let Some(iface) = args.interface.as_deref() {
        if !network_bind::interface_exists(iface) {
            return Err(anyhow::anyhow!(
                "Interface '{}' not found or has no addresses",
                iface
            ));
        }
        if network_bind::device_binding_supported() {
            None
        } else {
            Some(
                network_bind::interface_source_ip(iface, family).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Interface '{}' has no usable {} address",
                        iface,
                        family.map(|f| f.label()).unwrap_or("IP")
                    )
                })?,
            )
        }
    } else {
        None
    };

    // Re-validate now that --source's family is known (a v4 source with
    // --ipv6-only, etc.). For --interface we already selected a matching family.
    network_bind::resolve_ip_family(args.ipv4_only, args.ipv6_only, resolved_bind_ip)?;

    Ok(RunConfig {
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
        resolved_bind_ip,
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
    })
}

/// One-line summary of the active interface/source binding, or `None` when
/// neither was given. Emitted on stderr in text/json modes (the TUI shows the
/// interface in its Network Information panel instead).
pub fn bind_notice(cfg: &RunConfig) -> Option<String> {
    // --source and --interface are mutually exclusive, so at most one applies.
    match (cfg.interface.as_deref(), cfg.resolved_bind_ip) {
        // Device binding: no pinned IP, the OS sources per family.
        (Some(iface), None) => Some(format!(
            "Binding sockets to interface {} (device binding; dual-stack preserved)",
            iface
        )),
        // No device binding: the interface was resolved to a single source IP.
        (Some(iface), Some(ip)) => Some(format!(
            "Binding sockets to interface {} via source IP {} (single address family)",
            iface, ip
        )),
        (None, Some(ip)) => Some(format!("Binding sockets to source IP {}", ip)),
        (None, None) => None,
    }
}

/// Run the test engine and emit JSON (or stay silent).
/// `silent` controls whether JSON is printed and whether save errors propagate.
async fn run_test_engine(args: Cli, silent: bool) -> Result<()> {
    let cfg = build_config(&args)?;
    if !silent {
        if let Some(msg) = bind_notice(&cfg) {
            eprintln!("{}", msg);
        }
    }
    let network_info = crate::network::gather_network_info(&args);

    let (evt_tx, mut evt_rx) = mpsc::channel::<TestEvent>(2048);
    let (_, ctrl_rx) = mpsc::channel::<EngineControl>(16);

    let engine = TestEngine::new(cfg);
    let handle = tokio::spawn(async move { engine.run(evt_tx, ctrl_rx).await });

    // Collect throughput samples for connection-quality computation.
    let run_start = std::time::Instant::now();
    let mut dl_points: Vec<(f64, f64)> = Vec::new();
    let mut ul_points: Vec<(f64, f64)> = Vec::new();

    while let Some(ev) = evt_rx.recv().await {
        if let TestEvent::ThroughputTick {
            phase, bps_instant, ..
        } = ev
        {
            if matches!(
                phase,
                crate::model::Phase::Download | crate::model::Phase::Upload
            ) {
                let elapsed = run_start.elapsed().as_secs_f64();
                let mbps = (bps_instant * 8.0) / 1_000_000.0;
                match phase {
                    crate::model::Phase::Download => dl_points.push((elapsed, mbps)),
                    crate::model::Phase::Upload => ul_points.push((elapsed, mbps)),
                    _ => {}
                }
            }
        }
    }

    let mut result = handle
        .await
        .context("test engine task failed")?
        .context("speed test failed")?;

    result.connection_quality = crate::quality::compute(&result, &dl_points, &ul_points);

    let enriched = crate::network::enrich_result(&result, &network_info);

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
            match crate::storage::save_run(&enriched) {
                Ok(p) => eprintln!("{}", crate::event_format::format_saved_line(&p)),
                Err(e) => eprintln!("Save failed: {e:#}"),
            }
        }
    }

    Ok(())
}

async fn run_text(args: Cli) -> Result<()> {
    let cfg = build_config(&args)?;
    if let Some(msg) = bind_notice(&cfg) {
        eprintln!("{}", msg);
    }
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
        // Single source of truth for the per-event line(s). The same
        // formatter feeds the TUI dashboard's Test Activity panel so the two
        // modes can't drift apart.
        for line in crate::event_format::format_event_lines(&ev) {
            eprintln!("{}", line);
        }

        // After printing, capture the data text mode needs locally for the
        // end-of-run metric computation.
        match ev {
            TestEvent::ThroughputTick {
                phase, bps_instant, ..
            } if matches!(
                phase,
                crate::model::Phase::Download | crate::model::Phase::Upload
            ) =>
            {
                let elapsed = run_start.elapsed().as_secs_f64();
                let mbps = (bps_instant * 8.0) / 1_000_000.0;
                match phase {
                    crate::model::Phase::Download => dl_points.push((elapsed, mbps)),
                    crate::model::Phase::Upload => ul_points.push((elapsed, mbps)),
                    _ => {}
                }
            }
            TestEvent::LatencySample {
                phase,
                ok: true,
                rtt_ms: Some(ms),
                during,
            } => match (phase, during) {
                (crate::model::Phase::IdleLatency, None) => {
                    idle_latency_samples.push(ms);
                }
                (crate::model::Phase::Download, Some(crate::model::Phase::Download)) => {
                    loaded_dl_latency_samples.push(ms);
                }
                (crate::model::Phase::Upload, Some(crate::model::Phase::Upload)) => {
                    loaded_ul_latency_samples.push(ms);
                }
                _ => {}
            },
            _ => {}
        }
    }

    let mut result = match handle.await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            eprintln!("Speed test failed: {e:#}");
            return Err(e);
        }
        Err(e) => return Err(e.into()),
    };

    result.connection_quality = crate::quality::compute(&result, &dl_points, &ul_points);

    // Gather network information and enrich result
    let network_info = crate::network::gather_network_info(&args);
    let enriched = crate::network::enrich_result(&result, &network_info);

    handle_exports(&args, &enriched)?;

    // Both text mode and the TUI dashboard print the same summary, from the
    // same function. No per-mode customization.
    for line in crate::event_format::format_result_summary(
        &enriched,
        &dl_points,
        &ul_points,
        &idle_latency_samples,
        &loaded_dl_latency_samples,
        &loaded_ul_latency_samples,
    ) {
        println!("{}", line);
    }
    if args.auto_save {
        match crate::storage::save_run(&enriched) {
            Ok(p) => eprintln!("{}", crate::event_format::format_saved_line(&p)),
            Err(e) => eprintln!("Save failed: {e:#}"),
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
