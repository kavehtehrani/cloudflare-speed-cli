use crate::cli::{build_config, Cli};
use crate::engine::{EngineControl, TestEngine};
use crate::model::{Phase, RunResult, TestEvent};
use anyhow::{Context, Result};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Sparkline, Tabs},
    Terminal,
};
use std::{io, time::Duration, time::Instant};
use std::process::Command;
use tokio::sync::mpsc;

struct UiState {
    tab: usize,
    paused: bool,
    phase: Phase,
    info: String,

    dl_series: Vec<u64>,
    ul_series: Vec<u64>,
    idle_lat_series: Vec<u64>,
    loaded_dl_lat_series: Vec<u64>,
    loaded_ul_lat_series: Vec<u64>,

    // Time-series for charts (seconds since run start, value)
    run_start: Instant,
    dl_points: Vec<(f64, f64)>,
    ul_points: Vec<(f64, f64)>,
    idle_lat_points: Vec<(f64, f64)>,
    loaded_dl_lat_points: Vec<(f64, f64)>,
    loaded_ul_lat_points: Vec<(f64, f64)>,

    dl_mbps: f64,
    ul_mbps: f64,
    dl_avg_mbps: f64,
    ul_avg_mbps: f64,
    dl_bytes_total: u64,
    ul_bytes_total: u64,
    dl_phase_start: Option<Instant>,
    ul_phase_start: Option<Instant>,

    // Live latency samples for real-time stats
    idle_latency_samples: Vec<f64>,
    loaded_dl_latency_samples: Vec<f64>,
    loaded_ul_latency_samples: Vec<f64>,
    idle_latency_sent: u64,
    idle_latency_received: u64,
    loaded_dl_latency_sent: u64,
    loaded_dl_latency_received: u64,
    loaded_ul_latency_sent: u64,
    loaded_ul_latency_received: u64,

    last_result: Option<RunResult>,
    history: Vec<RunResult>,
    history_selected: usize, // Index of selected history item (0 = most recent)
    ip: Option<String>,
    colo: Option<String>,
    server: Option<String>,
    asn: Option<String>,
    as_org: Option<String>,
    auto_save: bool,
    last_exported_path: Option<String>, // Full path of last exported file (for clipboard)
    // Network interface information
    interface_name: Option<String>,
    network_name: Option<String>, // SSID for wireless, or network name
    is_wireless: Option<bool>,
    interface_mac: Option<String>,
    link_speed_mbps: Option<u64>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: 0,
            paused: false,
            phase: Phase::IdleLatency,
            info: String::new(),
            dl_series: Vec::new(),
            ul_series: Vec::new(),
            idle_lat_series: Vec::new(),
            loaded_dl_lat_series: Vec::new(),
            loaded_ul_lat_series: Vec::new(),
            run_start: Instant::now(),
            dl_points: Vec::new(),
            ul_points: Vec::new(),
            idle_lat_points: Vec::new(),
            loaded_dl_lat_points: Vec::new(),
            loaded_ul_lat_points: Vec::new(),
            dl_mbps: 0.0,
            ul_mbps: 0.0,
            dl_avg_mbps: 0.0,
            ul_avg_mbps: 0.0,
            dl_bytes_total: 0,
            ul_bytes_total: 0,
            dl_phase_start: None,
            ul_phase_start: None,
            idle_latency_samples: Vec::new(),
            loaded_dl_latency_samples: Vec::new(),
            loaded_ul_latency_samples: Vec::new(),
            idle_latency_sent: 0,
            idle_latency_received: 0,
            loaded_dl_latency_sent: 0,
            loaded_dl_latency_received: 0,
            loaded_ul_latency_sent: 0,
            loaded_ul_latency_received: 0,
            last_result: None,
            history: Vec::new(),
            history_selected: 0,
            ip: None,
            colo: None,
            server: None,
            asn: None,
            as_org: None,
            auto_save: true,
            last_exported_path: None,
            interface_name: None,
            network_name: None,
            is_wireless: None,
            interface_mac: None,
            link_speed_mbps: None,
        }
    }
}

impl UiState {
    fn push_series(series: &mut Vec<u64>, v: u64) {
        const MAX: usize = 120;
        series.push(v);
        if series.len() > MAX {
            let _ = series.drain(0..(series.len() - MAX));
        }
    }

    fn push_point(points: &mut Vec<(f64, f64)>, x: f64, y: f64) {
        const MAX: usize = 1200; // ~2 min at 10Hz
        points.push((x, y));
        if points.len() > MAX {
            let _ = points.drain(0..(points.len() - MAX));
        }
    }

    fn compute_live_latency_stats(
        samples: &[f64],
        sent: u64,
        received: u64,
    ) -> crate::model::LatencySummary {
        use hdrhistogram::Histogram;
        let loss = if sent == 0 {
            0.0
        } else {
            ((sent - received) as f64) / (sent as f64)
        };

        if samples.is_empty() {
            return crate::model::LatencySummary {
                sent,
                received,
                loss,
                min_ms: None,
                p50_ms: None,
                p90_ms: None,
                p99_ms: None,
                max_ms: None,
                jitter_ms: None,
            };
        }

        // Compute jitter (stddev) from samples
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let variance = samples.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
        let jitter_ms = Some(variance.sqrt());

        // Use HDRHistogram for percentiles
        let mut h = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
        for &ms in samples {
            let us = (ms * 1000.0).round().clamp(1.0, 60_000_000.0) as u64;
            let _ = h.record(us);
        }

        crate::model::LatencySummary {
            sent,
            received,
            loss,
            min_ms: Some((h.min() as f64) / 1000.0),
            p50_ms: Some((h.value_at_quantile(0.50) as f64) / 1000.0),
            p90_ms: Some((h.value_at_quantile(0.90) as f64) / 1000.0),
            p99_ms: Some((h.value_at_quantile(0.99) as f64) / 1000.0),
            max_ms: Some((h.max() as f64) / 1000.0),
            jitter_ms,
        }
    }
}

/// Gather network interface information
fn gather_network_info() -> (Option<String>, Option<String>, Option<bool>, Option<String>, Option<u64>) {
    // Get default interface by trying to connect to a remote address
    let interface_name = get_default_interface();
    
    if let Some(ref iface) = interface_name {
        let is_wireless = check_if_wireless(iface);
        let network_name = if is_wireless.unwrap_or(false) {
            get_wireless_ssid(iface)
        } else {
            None
        };
        let mac = get_interface_mac(iface);
        let speed = get_interface_speed(iface);
        (Some(iface.clone()), network_name, is_wireless, mac, speed)
    } else {
        (None, None, None, None, None)
    }
}

/// Get the default network interface name
fn get_default_interface() -> Option<String> {
    // Try to get interface from default route
    if let Ok(output) = Command::new("ip")
        .args(&["route", "show", "default"])
        .output()
    {
        if let Ok(output_str) = String::from_utf8(output.stdout) {
            // Look for "dev <interface>" in the output
            for line in output_str.lines() {
                if let Some(dev_pos) = line.find("dev ") {
                    let rest = &line[dev_pos + 4..];
                    if let Some(space_pos) = rest.find(' ') {
                        return Some(rest[..space_pos].to_string());
                    } else {
                        return Some(rest.to_string());
                    }
                }
            }
        }
    }
    
    // Fallback: try to find first non-loopback interface
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str != "lo" && !name_str.starts_with("docker") && !name_str.starts_with("br-") {
                return Some(name_str.to_string());
            }
        }
    }
    
    None
}

/// Check if interface is wireless
fn check_if_wireless(iface: &str) -> Option<bool> {
    // Check if /sys/class/net/<iface>/wireless exists
    let wireless_path = format!("/sys/class/net/{}/wireless", iface);
    Some(std::path::Path::new(&wireless_path).exists())
}

/// Get wireless SSID for an interface
fn get_wireless_ssid(iface: &str) -> Option<String> {
    // Try iwgetid first (most reliable)
    if let Ok(output) = Command::new("iwgetid")
        .arg("-r")
        .arg(iface)
        .output()
    {
        if let Ok(ssid) = String::from_utf8(output.stdout) {
            let ssid = ssid.trim().to_string();
            if !ssid.is_empty() {
                return Some(ssid);
            }
        }
    }
    
    // Fallback: try iw command
    if let Ok(output) = Command::new("iw")
        .args(&["dev", iface, "info"])
        .output()
    {
        if let Ok(output_str) = String::from_utf8(output.stdout) {
            for line in output_str.lines() {
                if line.trim().starts_with("ssid ") {
                    let ssid = line.trim().strip_prefix("ssid ").unwrap_or("").trim();
                    if !ssid.is_empty() {
                        return Some(ssid.to_string());
                    }
                }
            }
        }
    }
    
    None
}

/// Get MAC address of interface
fn get_interface_mac(iface: &str) -> Option<String> {
    let mac_path = format!("/sys/class/net/{}/address", iface);
    std::fs::read_to_string(mac_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Get link speed in Mbps
fn get_interface_speed(iface: &str) -> Option<u64> {
    let speed_path = format!("/sys/class/net/{}/speed", iface);
    std::fs::read_to_string(speed_path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&speed| speed > 0 && speed < 1_000_000) // Sanity check
}

pub async fn run(args: Cli) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).ok();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear().ok();

    let mut state = UiState {
        phase: Phase::IdleLatency,
        auto_save: args.auto_save,
        ..Default::default()
    };
    state.history = crate::storage::load_recent(20).unwrap_or_default();
    
    // Gather network interface information
    let (interface_name, network_name, is_wireless, interface_mac, link_speed_mbps) = gather_network_info();
    state.interface_name = interface_name;
    state.network_name = network_name;
    state.is_wireless = is_wireless;
    state.interface_mac = interface_mac;
    state.link_speed_mbps = link_speed_mbps;

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    // Start first run.
    let mut run_ctx = start_run(&args).await?;

    let res = loop {
        tokio::select! {
            _ = tick.tick() => {
                terminal.draw(|f| draw(f.area(), f, &state)).ok();
            }
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else { continue };
                if let Event::Key(k) = ev {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    match (k.modifiers, k.code) {
                        (_, KeyCode::Char('q')) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                            run_ctx.ctrl_tx.send(EngineControl::Cancel).await.ok();
                            break Ok(());
                        }
                        (_, KeyCode::Char('p')) => {
                            state.paused = !state.paused;
                            run_ctx.ctrl_tx.send(EngineControl::Pause(state.paused)).await.ok();
                        }
                        (_, KeyCode::Char('r')) => {
                            state.info = "Restarting…".into();
                            run_ctx.ctrl_tx.send(EngineControl::Cancel).await.ok();
                            if let Some(h) = run_ctx.handle.take() {
                                let _ = h.await;
                            }
                            state.last_result = None;
                            state.run_start = Instant::now();
                            state.dl_series.clear();
                            state.ul_series.clear();
                            state.idle_lat_series.clear();
                            state.loaded_dl_lat_series.clear();
                            state.loaded_ul_lat_series.clear();
                            state.dl_points.clear();
                            state.ul_points.clear();
                            state.idle_lat_points.clear();
                            state.loaded_dl_lat_points.clear();
                            state.loaded_ul_lat_points.clear();
                            state.dl_mbps = 0.0;
                            state.ul_mbps = 0.0;
                            state.dl_avg_mbps = 0.0;
                            state.ul_avg_mbps = 0.0;
                            state.dl_bytes_total = 0;
                            state.ul_bytes_total = 0;
                            state.dl_phase_start = None;
                            state.ul_phase_start = None;
                            state.paused = false;
                            run_ctx = start_run(&args).await?;
                        }
                        (_, KeyCode::Char('s')) => {
                            // Only save on dashboard (auto-save location)
                            if state.tab == 0 {
                                if let Some(r) = state.last_result.as_ref() {
                                    match save_result_json(r) {
                                        Ok(p) => {
                                            state.info = format!("Saved JSON: {}", p.display());
                                        }
                                        Err(e) => {
                                            state.info = format!("Save failed: {e:#}");
                                        }
                                    }
                                } else {
                                    state.info = "No completed run to save yet.".into();
                                }
                            }
                        }
                        // Export functions only work in history tab
                        (_, KeyCode::Char('e')) => {
                            if state.tab == 1 && !state.history.is_empty() {
                                if state.history_selected < state.history.len() {
                                    let r = &state.history[state.history_selected];
                                    match export_result_json(r) {
                                        Ok(p) => {
                                            let path_str = p.to_string_lossy().to_string();
                                            state.last_exported_path = Some(path_str.clone());
                                            state.info = format!("Exported JSON: {}", p.display());
                                        }
                                        Err(e) => {
                                            state.info = format!("JSON export failed: {e:#}");
                                        }
                                    }
                                }
                            }
                        }
                        (_, KeyCode::Char('c')) => {
                            if state.tab == 1 && !state.history.is_empty() {
                                if state.history_selected < state.history.len() {
                                    let r = &state.history[state.history_selected];
                                    match export_result_csv(r) {
                                        Ok(p) => {
                                            let path_str = p.to_string_lossy().to_string();
                                            state.last_exported_path = Some(path_str.clone());
                                            state.info = format!("Exported CSV: {}", p.display());
                                        }
                                        Err(e) => {
                                            state.info = format!("CSV export failed: {e:#}");
                                        }
                                    }
                                }
                            }
                        }
                        (_, KeyCode::Char('y')) => {
                            // Copy last exported path to clipboard (yank)
                            if state.tab == 1 {
                                if let Some(ref path) = state.last_exported_path {
                                    match copy_to_clipboard(path) {
                                        Ok(_) => {
                                            state.info = format!("Copied to clipboard: {}", path);
                                        }
                                        Err(e) => {
                                            state.info = format!("Clipboard copy failed: {e:#}");
                                        }
                                    }
                                } else {
                                    state.info = "No exported file path to copy. Export a file first (e/c)".into();
                                }
                            }
                        }
                        (_, KeyCode::Char('a')) => {
                            state.auto_save = !state.auto_save;
                            state.info = if state.auto_save {
                                "Auto-save enabled".into()
                            } else {
                                "Auto-save disabled".into()
                            };
                        }
                        (_, KeyCode::Tab) => {
                            let new_tab = (state.tab + 1) % 3;
                            state.tab = new_tab;
                            // Reset history selection when switching to history tab
                            if new_tab == 1 {
                                state.history_selected = 0;
                            }
                        }
                        (_, KeyCode::Char('?')) => {
                            state.tab = 2; // help
                        }
                        // History navigation and deletion (only when on History tab)
                        (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                            if state.tab == 1 && !state.history.is_empty() {
                                // Up/k goes to newer items (lower index, towards 0)
                                if state.history_selected > 0 {
                                    state.history_selected -= 1;
                                }
                            }
                        }
                        (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                            if state.tab == 1 && !state.history.is_empty() {
                                // Down/j goes to older items (higher index in array)
                                // Allow navigation through all items; display will show what fits
                                if state.history_selected < state.history.len().saturating_sub(1) {
                                    state.history_selected += 1;
                                }
                            }
                        }
                        (_, KeyCode::Char('d')) => {
                            if state.tab == 1 && !state.history.is_empty() {
                                // history_selected directly maps to history index (newest first)
                                if state.history_selected < state.history.len() {
                                    let to_delete = state.history[state.history_selected].clone();
                                    if let Err(e) = crate::storage::delete_run(&to_delete) {
                                        state.info = format!("Delete failed: {e:#}");
                                    } else {
                                        state.history.remove(state.history_selected);
                                        // Adjust selection if needed
                                        if state.history_selected >= state.history.len() && !state.history.is_empty() {
                                            state.history_selected = state.history.len() - 1;
                                        } else if state.history.is_empty() {
                                            state.history_selected = 0;
                                        }
                                        state.info = "Deleted".into();
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            maybe_engine_ev = run_ctx.event_rx.recv() => {
                match maybe_engine_ev {
                    None => {
                        // engine finished; wait for result
                        if let Some(h) = run_ctx.handle.take() {
                            match h.await {
                                Ok(Ok(r)) => {
                                    if state.auto_save {
                                        crate::storage::save_run(&r).ok();
                                    }
                                    if let Some(meta) = r.meta.as_ref() {
                                        // Try multiple possible field names for IP
                                        state.ip = meta
                                            .get("clientIp")
                                            .or_else(|| meta.get("ip"))
                                            .or_else(|| meta.get("clientIP"))
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string());
                                        state.colo = meta
                                            .get("colo")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string());
                                        // Extract ASN and organization
                                        state.asn = meta
                                            .get("asn")
                                            .and_then(|v| v.as_i64())
                                            .map(|n| n.to_string())
                                            .or_else(|| meta.get("asn").and_then(|v| v.as_str()).map(|s| s.to_string()));
                                        state.as_org = meta
                                            .get("asOrganization")
                                            .or_else(|| meta.get("asnOrg"))
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string());
                                    }
                                    // Server should be set from RunResult.server
                                    if r.server.is_some() {
                                        state.server = r.server.clone();
                                    }
                                    state.last_result = Some(r);
                                    state.history = crate::storage::load_recent(20).unwrap_or_default();
                                    state.info = "Done. (r rerun, q quit)".into();
                                }
                                Ok(Err(e)) => state.info = format!("Run failed: {e:#}"),
                                Err(e) => state.info = format!("Run join failed: {e}"),
                            }
                        }
                    }
                    Some(ev) => apply_event(&mut state, ev),
                }
            }
        }
    };

    // Restore terminal.
    disable_raw_mode().ok();
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen).ok();
    res
}

struct RunCtx {
    ctrl_tx: mpsc::Sender<EngineControl>,
    event_rx: mpsc::Receiver<TestEvent>,
    handle: Option<tokio::task::JoinHandle<Result<RunResult>>>,
}

async fn start_run(args: &Cli) -> Result<RunCtx> {
    let cfg = build_config(args);
    let (event_tx, event_rx) = mpsc::channel::<TestEvent>(4096);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<EngineControl>(32);
    let engine = TestEngine::new(cfg);
    let handle = tokio::spawn(async move { engine.run(event_tx, ctrl_rx).await });
    Ok(RunCtx {
        ctrl_tx,
        event_rx,
        handle: Some(handle),
    })
}

fn apply_event(state: &mut UiState, ev: TestEvent) {
    match ev {
        TestEvent::PhaseStarted { phase } => {
            state.phase = phase;
            state.info = format!("Phase: {phase:?}");
            match phase {
                Phase::IdleLatency => {
                    // Reset idle latency tracking
                    state.idle_latency_samples.clear();
                    state.idle_latency_sent = 0;
                    state.idle_latency_received = 0;
                }
                Phase::Download => {
                    state.dl_phase_start = Some(Instant::now());
                    state.dl_bytes_total = 0;
                    state.dl_avg_mbps = 0.0;
                    // Reset loaded DL latency tracking
                    state.loaded_dl_latency_samples.clear();
                    state.loaded_dl_latency_sent = 0;
                    state.loaded_dl_latency_received = 0;
                }
                Phase::Upload => {
                    state.ul_phase_start = Some(Instant::now());
                    state.ul_bytes_total = 0;
                    state.ul_avg_mbps = 0.0;
                    // Reset loaded UL latency tracking
                    state.loaded_ul_latency_samples.clear();
                    state.loaded_ul_latency_sent = 0;
                    state.loaded_ul_latency_received = 0;
                }
                _ => {}
            }
        }
        TestEvent::Info { message } => state.info = message,
        TestEvent::MetaInfo { meta } => {
            // Extract IP and colo from meta
            state.ip = meta
                .get("clientIp")
                .or_else(|| meta.get("ip"))
                .or_else(|| meta.get("clientIP"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            state.colo = meta
                .get("colo")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            
            // Extract ASN and organization
            state.asn = meta
                .get("asn")
                .and_then(|v| {
                    if let Some(n) = v.as_i64() {
                        Some(n.to_string())
                    } else {
                        v.as_str().map(|s| s.to_string())
                    }
                });
            state.as_org = meta
                .get("asOrganization")
                .or_else(|| meta.get("asnOrg"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            
            // Extract city for server location (if available, use it directly)
            if let Some(city) = meta.get("city").and_then(|v| v.as_str()) {
                // If we have city, use it for server location
                // Could combine with country if available
                if let Some(country) = meta.get("country").and_then(|v| v.as_str()) {
                    state.server = Some(format!("{}, {}", city, country));
                } else {
                    state.server = Some(city.to_string());
                }
            } else if state.colo.is_some() && state.server.is_none() {
                // If we have colo but no city, we'll wait for RunResult.server
                // which comes from map_colo_to_server
            }
        }
        TestEvent::LatencySample { phase, during, rtt_ms, ok } => {
            let t = state.run_start.elapsed().as_secs_f64();
            match (phase, during) {
                (Phase::IdleLatency, _) => {
                    state.idle_latency_sent += 1;
                    if ok {
                        state.idle_latency_received += 1;
                        if let Some(ms) = rtt_ms {
                            let v = ms.round().clamp(0.0, 5000.0) as u64;
                            UiState::push_series(&mut state.idle_lat_series, v);
                            UiState::push_point(&mut state.idle_lat_points, t, ms);
                            state.idle_latency_samples.push(ms);
                            // Keep reasonable sample size
                            if state.idle_latency_samples.len() > 10000 {
                                state.idle_latency_samples.drain(0..(state.idle_latency_samples.len() - 10000));
                            }
                        }
                    }
                }
                (Phase::Download, Some(Phase::Download)) => {
                    state.loaded_dl_latency_sent += 1;
                    if ok {
                        state.loaded_dl_latency_received += 1;
                        if let Some(ms) = rtt_ms {
                            let v = ms.round().clamp(0.0, 5000.0) as u64;
                            UiState::push_series(&mut state.loaded_dl_lat_series, v);
                            UiState::push_point(&mut state.loaded_dl_lat_points, t, ms);
                            state.loaded_dl_latency_samples.push(ms);
                            if state.loaded_dl_latency_samples.len() > 10000 {
                                state.loaded_dl_latency_samples.drain(0..(state.loaded_dl_latency_samples.len() - 10000));
                            }
                        }
                    }
                }
                (Phase::Upload, Some(Phase::Upload)) => {
                    state.loaded_ul_latency_sent += 1;
                    if ok {
                        state.loaded_ul_latency_received += 1;
                        if let Some(ms) = rtt_ms {
                            let v = ms.round().clamp(0.0, 5000.0) as u64;
                            UiState::push_series(&mut state.loaded_ul_lat_series, v);
                            UiState::push_point(&mut state.loaded_ul_lat_points, t, ms);
                            state.loaded_ul_latency_samples.push(ms);
                            if state.loaded_ul_latency_samples.len() > 10000 {
                                state.loaded_ul_latency_samples.drain(0..(state.loaded_ul_latency_samples.len() - 10000));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        TestEvent::ThroughputTick { phase, bytes_total, bps_instant } => {
            let mbps = (bps_instant * 8.0) / 1_000_000.0;
            let t = state.run_start.elapsed().as_secs_f64();
            match phase {
                Phase::Download => {
                    state.dl_mbps = mbps;
                    state.dl_bytes_total = bytes_total;
                    if let Some(t0) = state.dl_phase_start {
                        let secs = t0.elapsed().as_secs_f64().max(1e-9);
                        state.dl_avg_mbps = ((bytes_total as f64) / secs) * 8.0 / 1_000_000.0;
                    }
                    let v = state.dl_mbps.round().clamp(0.0, 10_000.0) as u64;
                    UiState::push_series(&mut state.dl_series, v);
                    UiState::push_point(&mut state.dl_points, t, state.dl_mbps.max(0.0));
                }
                Phase::Upload => {
                    state.ul_mbps = mbps;
                    state.ul_bytes_total = bytes_total;
                    if let Some(t0) = state.ul_phase_start {
                        let secs = t0.elapsed().as_secs_f64().max(1e-9);
                        state.ul_avg_mbps = ((bytes_total as f64) / secs) * 8.0 / 1_000_000.0;
                    }
                    let v = state.ul_mbps.round().clamp(0.0, 10_000.0) as u64;
                    UiState::push_series(&mut state.ul_series, v);
                    UiState::push_point(&mut state.ul_points, t, state.ul_mbps.max(0.0));
                }
                _ => {}
            }
        }
    }
}

fn draw(area: Rect, f: &mut ratatui::Frame, state: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
        .split(area);

    let tabs = Tabs::new(vec![
        Line::from("Dashboard"),
        Line::from("History"),
        Line::from("Help"),
    ])
        .select(state.tab)
        .block(Block::default().borders(Borders::ALL).title("cloudflare-speed-cli"))
        .highlight_style(Style::default().fg(Color::Yellow));
    f.render_widget(tabs, chunks[0]);

    match state.tab {
        0 => draw_dashboard(chunks[1], f, state),
        1 => draw_history(chunks[1], f, state),
        _ => draw_help(chunks[1], f),
    }
}

fn draw_dashboard(area: Rect, f: &mut ratatui::Frame, state: &UiState) {
    // Small terminal: keep the compact dashboard (gauges + sparklines).
    // Large terminal: show full charts (like the website) alongside the live cards.
    if area.height < 28 {
        return draw_dashboard_compact(area, f, state);
    }

    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(3),  // Speed boxes row (side-by-side)
                Constraint::Length(12), // Throughput charts row (side-by-side)
                Constraint::Length(8),  // Latency stats row (idle + loaded DL + loaded UL)
                Constraint::Min(0),     // Status + shortcuts
            ]
            .as_ref(),
        )
        .split(area);

    // Speed boxes at the top: DL left, UL right
    let speed_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(main[0]);
    
    // Download speed box
    let dl_info = Paragraph::new(Line::from(vec![
        Span::styled(format!("inst {:.1} / avg {:.1}", state.dl_mbps, state.dl_avg_mbps), Style::default().fg(Color::Green)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green))
            .title("Download Speed")
    );
    f.render_widget(dl_info, speed_row[0]);
    
    // Upload speed box
    let ul_info = Paragraph::new(Line::from(vec![
        Span::styled(format!("inst {:.1} / avg {:.1}", state.ul_mbps, state.ul_avg_mbps), Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title("Upload Speed")
    );
    f.render_widget(ul_info, speed_row[1]);

    // Throughput charts side-by-side: DL left, UL right
    let thr_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(main[1]);

    let (_x_min, x_max) = x_bounds(&state.dl_points, &state.ul_points);
    
    // Download throughput chart (left)
    let y_dl_max = max_y(&state.dl_points).max(10.0);
    let y_dl_max = (y_dl_max * 1.10).min(10_000.0);
    let dl_ds = Dataset::default()
        .graph_type(GraphType::Line)
        .marker(symbols::Marker::Braille)
        .style(Style::default().fg(Color::Green))
        .data(window(&state.dl_points, x_max, 60.0));
    let dl_chart = Chart::new(vec![dl_ds])
        .block(Block::default().borders(Borders::ALL).title("Download Throughput (last 60s)"))
        .x_axis(
            Axis::default()
                .bounds([(x_max - 60.0).max(0.0), x_max.max(1.0)]),
        )
        .y_axis(Axis::default().title("Mbps").bounds([0.0, y_dl_max]));
    f.render_widget(dl_chart, thr_row[0]);

    // Upload throughput chart (right)
    let y_ul_max = max_y(&state.ul_points).max(10.0);
    let y_ul_max = (y_ul_max * 1.10).min(10_000.0);
    let ul_ds = Dataset::default()
        .graph_type(GraphType::Line)
        .marker(symbols::Marker::Braille)
        .style(Style::default().fg(Color::Cyan))
        .data(window(&state.ul_points, x_max, 60.0));
    let ul_chart = Chart::new(vec![ul_ds])
        .block(Block::default().borders(Borders::ALL).title("Upload Throughput (last 60s)"))
        .x_axis(
            Axis::default()
                .bounds([(x_max - 60.0).max(0.0), x_max.max(1.0)]),
        )
        .y_axis(Axis::default().title("Mbps").bounds([0.0, y_ul_max]));
    f.render_widget(ul_chart, thr_row[1]);

    // Latency stats (numeric, not charts): Idle, Loaded DL, Loaded UL
    let lat_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)].as_ref())
        .split(main[2]);

    // Helper to format latency stats
    let format_latency = |lat: &crate::model::LatencySummary| -> Vec<Line> {
        vec![
            Line::from(vec![
                Span::styled("p50: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.1} ms", lat.p50_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("p90: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.1} ms", lat.p90_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("p99: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.1} ms", lat.p99_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("Jitter: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.1} ms", lat.jitter_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("Loss: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.2}%", lat.loss * 100.0)),
            ]),
        ]
    };

    // Idle latency stats (live from samples)
    let idle_lat = if state.idle_latency_samples.is_empty() && state.idle_latency_sent == 0 {
        None
    } else {
        Some(UiState::compute_live_latency_stats(
            &state.idle_latency_samples,
            state.idle_latency_sent,
            state.idle_latency_received,
        ))
    };
    let idle_stats = Paragraph::new(
        idle_lat.as_ref().map(format_latency).unwrap_or_else(|| {
            vec![Line::from("Waiting for data...")]
        })
    )
    .block(Block::default().borders(Borders::ALL).title("Idle Latency"));
    f.render_widget(idle_stats, lat_row[0]);

    // Loaded latency during download stats (live from samples)
    let dl_loaded_lat = if state.loaded_dl_latency_samples.is_empty() && state.loaded_dl_latency_sent == 0 {
        None
    } else {
        Some(UiState::compute_live_latency_stats(
            &state.loaded_dl_latency_samples,
            state.loaded_dl_latency_sent,
            state.loaded_dl_latency_received,
        ))
    };
    let dl_loaded_stats = Paragraph::new(
        dl_loaded_lat.as_ref().map(format_latency).unwrap_or_else(|| {
            vec![Line::from("Waiting for data...")]
        })
    )
    .block(Block::default().borders(Borders::ALL).title("Loaded Latency (Download)"));
    f.render_widget(dl_loaded_stats, lat_row[1]);

    // Loaded latency during upload stats (live from samples)
    let ul_loaded_lat = if state.loaded_ul_latency_samples.is_empty() && state.loaded_ul_latency_sent == 0 {
        None
    } else {
        Some(UiState::compute_live_latency_stats(
            &state.loaded_ul_latency_samples,
            state.loaded_ul_latency_sent,
            state.loaded_ul_latency_received,
        ))
    };
    let ul_loaded_stats = Paragraph::new(
        ul_loaded_lat.as_ref().map(format_latency).unwrap_or_else(|| {
            vec![Line::from("Waiting for data...")]
        })
    )
    .block(Block::default().borders(Borders::ALL).title("Loaded Latency (Upload)"));
    f.render_widget(ul_loaded_stats, lat_row[2]);

    // Combined Status and Controls panel
    let saved_path = state.last_result.as_ref()
        .and_then(|r| crate::storage::get_run_path(r).ok());
    
    // Determine IP version
    let ip_version = state.ip.as_deref()
        .map(|ip| if ip.contains(':') { "IPv6" } else { "IPv4" })
        .unwrap_or("-");
    
    let combined = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Phase: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{:?}", state.phase)),
            Span::raw("   "),
            Span::styled("Paused: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}", state.paused)),
        ]),
        Line::from(vec![
            Span::styled("Connected via: ", Style::default().fg(Color::Gray)),
            Span::raw(ip_version),
        ]),
        Line::from(vec![
            Span::styled("Interface: ", Style::default().fg(Color::Gray)),
            Span::raw(state.interface_name.as_deref().unwrap_or("-")),
            Span::raw(" ("),
            Span::raw(if state.is_wireless.unwrap_or(false) { "Wireless" } else { "Wired" }),
            Span::raw(")"),
        ]),
        Line::from(vec![
            Span::styled("Network: ", Style::default().fg(Color::Gray)),
            Span::raw(
                state.network_name.as_deref()
                    .or_else(|| state.interface_name.as_deref())
                    .unwrap_or("-")
            ),
        ]),
        Line::from(vec![
            Span::styled("MAC address: ", Style::default().fg(Color::Gray)),
            Span::raw(state.interface_mac.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Link speed: ", Style::default().fg(Color::Gray)),
            Span::raw(
                state.link_speed_mbps
                    .map(|s| format!("{} Mbps", s))
                    .unwrap_or_else(|| "-".to_string())
            ),
        ]),
        Line::from(vec![
            Span::styled("Server location: ", Style::default().fg(Color::Gray)),
            Span::raw(state.server.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Your network: ", Style::default().fg(Color::Gray)),
            Span::raw(
                match (state.as_org.as_deref(), state.asn.as_deref()) {
                    (Some(org), Some(asn)) => format!("{} (AS{})", org, asn),
                    (Some(org), None) => org.to_string(),
                    (None, Some(asn)) => format!("AS{}", asn),
                    (None, None) => "-".to_string(),
                }
            ),
        ]),
        Line::from(vec![
            Span::styled("Your IP address: ", Style::default().fg(Color::Gray)),
            Span::raw(state.ip.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Info: ", Style::default().fg(Color::Gray)),
            Span::raw(&state.info),
        ]),
        Line::from(""),
        Line::from("Keyboard Shortcuts:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("q", Style::default().fg(Color::Magenta)),
            Span::raw(" / "),
            Span::styled("Ctrl-C", Style::default().fg(Color::Magenta)),
            Span::raw("  Quit"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("r", Style::default().fg(Color::Magenta)),
            Span::raw("           Rerun test"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("p", Style::default().fg(Color::Magenta)),
            Span::raw("           Pause/Resume"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("s", Style::default().fg(Color::Magenta)),
            Span::raw("           Save JSON (auto location)"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("a", Style::default().fg(Color::Magenta)),
            Span::raw("           Toggle auto-save"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("tab", Style::default().fg(Color::Magenta)),
            Span::raw("         Switch tabs"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("?", Style::default().fg(Color::Magenta)),
            Span::raw("           Help"),
        ]),
        Line::from(vec![
            Span::styled("Auto-save: ", Style::default().fg(Color::Gray)),
            Span::styled(
                if state.auto_save { "ON" } else { "OFF" },
                if state.auto_save {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                }
            ),
        ]),
        Line::from(vec![
            Span::styled("Saved JSON: ", Style::default().fg(Color::Gray)),
            Span::raw(
                saved_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("none")
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Source: ", Style::default().fg(Color::Gray)),
            Span::styled("https://speed.cloudflare.com/", Style::default().fg(Color::Blue)),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title("Status & Controls"));
    f.render_widget(combined, main[3]);
}

fn draw_dashboard_compact(area: Rect, f: &mut ratatui::Frame, state: &UiState) {
    let content = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(8)].as_ref())
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(content[0]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3), Constraint::Min(0)].as_ref())
        .split(cols[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3), Constraint::Min(0)].as_ref())
        .split(cols[1]);

    // Download sparkline with inst/avg box below
    f.render_widget(
        Sparkline::default()
            .block(Block::default().borders(Borders::ALL).title("DL sparkline (Mbps)"))
            .data(&state.dl_series)
            .style(Style::default().fg(Color::Green)),
        left[0],
    );
    let dl_info = Paragraph::new(Line::from(vec![
        Span::styled(format!("inst {:.1} / avg {:.1}", state.dl_mbps, state.dl_avg_mbps), Style::default().fg(Color::Green)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green))
            .title("Download Speed")
    );
    f.render_widget(dl_info, left[1]);

    // Upload sparkline with inst/avg box below
    f.render_widget(
        Sparkline::default()
            .block(Block::default().borders(Borders::ALL).title("UL sparkline (Mbps)"))
            .data(&state.ul_series)
            .style(Style::default().fg(Color::Cyan)),
        right[0],
    );
    let ul_info = Paragraph::new(Line::from(vec![
        Span::styled(format!("inst {:.1} / avg {:.1}", state.ul_mbps, state.ul_avg_mbps), Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title("Upload Speed")
    );
    f.render_widget(ul_info, right[1]);

    f.render_widget(
        Sparkline::default()
            .block(Block::default().borders(Borders::ALL).title("Idle latency (ms)"))
            .data(&state.idle_lat_series)
            .style(Style::default().fg(Color::Magenta)),
        right[2],
    );

    let meta = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Phase: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{:?}", state.phase)),
            Span::raw("   "),
            Span::styled("Paused: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}", state.paused)),
        ]),
        Line::from(vec![
            Span::styled("Interface: ", Style::default().fg(Color::Gray)),
            Span::raw(state.interface_name.as_deref().unwrap_or("-")),
            Span::raw(" ("),
            Span::raw(if state.is_wireless.unwrap_or(false) { "Wireless" } else { "Wired" }),
            Span::raw(")"),
        ]),
        Line::from(vec![
            Span::styled("Network: ", Style::default().fg(Color::Gray)),
            Span::raw(
                state.network_name.as_deref()
                    .or_else(|| state.interface_name.as_deref())
                    .unwrap_or("-")
            ),
        ]),
        Line::from(vec![
            Span::styled("IP/Colo: ", Style::default().fg(Color::Gray)),
            Span::raw(format!(
                "{} / {}",
                state.ip.as_deref().unwrap_or("-"),
                state.colo.as_deref().unwrap_or("-")
            )),
        ]),
        Line::from(vec![
            Span::styled("Info: ", Style::default().fg(Color::Gray)),
            Span::raw(&state.info),
        ]),
        Line::from(vec![
            Span::styled("Server: ", Style::default().fg(Color::Gray)),
            Span::raw(state.server.as_deref().unwrap_or("-")),
        ]),
        Line::from(""),
        Line::from("Keys: q quit | r rerun | p pause | s save json | tab switch | ? help"),
    ])
    .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(meta, content[1]);
}

fn draw_help(area: Rect, f: &mut ratatui::Frame) {
    let p = Paragraph::new(vec![
        Line::from("Keybinds:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("q", Style::default().fg(Color::Magenta)),
            Span::raw(" / "),
            Span::styled("Ctrl-C", Style::default().fg(Color::Magenta)),
            Span::raw("  Quit"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("r", Style::default().fg(Color::Magenta)),
            Span::raw("           Rerun"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("p", Style::default().fg(Color::Magenta)),
            Span::raw("           Pause/Resume"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("s", Style::default().fg(Color::Magenta)),
            Span::raw("           Save JSON (auto location)"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("a", Style::default().fg(Color::Magenta)),
            Span::raw("           Toggle auto-save"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("tab", Style::default().fg(Color::Magenta)),
            Span::raw("         Switch tabs"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("?", Style::default().fg(Color::Magenta)),
            Span::raw("           Show this help"),
        ]),
        Line::from(""),
        Line::from("History tab:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("↑/↓", Style::default().fg(Color::Magenta)),
            Span::raw(" or "),
            Span::styled("j/k", Style::default().fg(Color::Magenta)),
            Span::raw("  Navigate"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("e", Style::default().fg(Color::Magenta)),
            Span::raw("           Export selected as JSON"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("c", Style::default().fg(Color::Magenta)),
            Span::raw("           Export selected as CSV"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("y", Style::default().fg(Color::Magenta)),
            Span::raw("           Copy exported path to clipboard"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("d", Style::default().fg(Color::Magenta)),
            Span::raw("           Delete selected"),
        ]),
        Line::from(""),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(p, area);
}

/// Save JSON to the default auto-save location.
fn save_result_json(r: &RunResult) -> Result<std::path::PathBuf> {
    crate::storage::save_run(r)
}

/// Export JSON to a user-specified file location.
/// Returns the absolute path of the exported file.
fn export_result_json(r: &RunResult) -> Result<std::path::PathBuf> {
    // Generate a default filename based on timestamp
    let default_name = format!(
        "cloudflare-speed-{}-{}.json",
        r.timestamp_utc.replace(':', "-").replace('T', "_"),
        &r.meas_id[..8.min(r.meas_id.len())]
    );
    
    // Get absolute path from current directory
    let current_dir = std::env::current_dir().context("get current directory")?;
    let path = current_dir.join(default_name);
    crate::storage::export_json(&path, r)?;
    Ok(path)
}

/// Export CSV to a user-specified file location.
/// Returns the absolute path of the exported file.
fn export_result_csv(r: &RunResult) -> Result<std::path::PathBuf> {
    // Generate a default filename based on timestamp
    let default_name = format!(
        "cloudflare-speed-{}-{}.csv",
        r.timestamp_utc.replace(':', "-").replace('T', "_"),
        &r.meas_id[..8.min(r.meas_id.len())]
    );
    
    // Get absolute path from current directory
    let current_dir = std::env::current_dir().context("get current directory")?;
    let path = current_dir.join(default_name);
    crate::storage::export_csv(&path, r)?;
    Ok(path)
}

/// Copy text to clipboard.
fn copy_to_clipboard(text: &str) -> Result<()> {
    use arboard::Clipboard;
    let mut clipboard = Clipboard::new().context("initialize clipboard")?;
    clipboard.set_text(text).context("copy to clipboard")?;
    Ok(())
}

fn draw_history(area: Rect, f: &mut ratatui::Frame, state: &UiState) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("Most recent runs (↑/↓/j/k: navigate, d: delete):"));
    lines.push(Line::from(""));
    
    // Calculate how many items can fit in the available area
    // Subtract 2 for header lines, and 1 more for the "No history" message if needed
    let max_items = (area.height as usize).saturating_sub(2);
    
    // History is already ordered newest first, so we display it directly
    let history_display: Vec<_> = state.history.iter().take(max_items).collect();
    for (i, r) in history_display.iter().enumerate() {
        // i directly maps to history index since we're not reversing
        let is_selected = state.tab == 1 && i == state.history_selected;
        
        // Parse and format timestamp to human-readable format in local timezone
        let timestamp_str: String = {
            let s = &r.timestamp_utc;
            // Parse RFC3339 format manually and convert to local time
            // Format: "2024-01-15T14:30:45Z" or "2024-01-15T14:30:45+00:00"
            if s.len() >= 19 && s.contains('T') {
                let date_time: String = s.chars().take(19).collect();
                if let Some(t_pos) = date_time.find('T') {
                    let date_part = &date_time[..t_pos];
                    let time_part = &date_time[t_pos+1..];
                    
                    // Parse date components
                    if let (Some(year), Some(month), Some(day)) = (
                        date_part.get(0..4).and_then(|s| s.parse::<i32>().ok()),
                        date_part.get(5..7).and_then(|s| s.parse::<u8>().ok()),
                        date_part.get(8..10).and_then(|s| s.parse::<u8>().ok()),
                    ) {
                        // Parse time components
                        if let (Some(hour), Some(minute), Some(second)) = (
                            time_part.get(0..2).and_then(|s| s.parse::<u8>().ok()),
                            time_part.get(3..5).and_then(|s| s.parse::<u8>().ok()),
                            time_part.get(6..8).and_then(|s| s.parse::<u8>().ok()),
                        ) {
                            // Try to create UTC datetime and convert to local
                            if let Ok(month_enum) = time::Month::try_from(month) {
                                if let (Ok(date), Ok(time)) = (
                                    time::Date::from_calendar_date(year, month_enum, day),
                                    time::Time::from_hms(hour, minute, second),
                                ) {
                                    let utc_dt = time::PrimitiveDateTime::new(date, time).assume_utc();
                                    
                                    // Get local offset and convert
                                    match time::UtcOffset::current_local_offset() {
                                        Ok(local_offset) => {
                                            let local_dt = utc_dt.to_offset(local_offset);
                                            let local_date = local_dt.date();
                                            let local_time = local_dt.time();
                                            // Format offset as +HH:MM or -HH:MM
                                            let offset_hours = local_offset.whole_hours();
                                            let offset_minutes = local_offset.whole_minutes() % 60;
                                            let offset_sign = if offset_hours >= 0 { '+' } else { '-' };
                                            let offset_str = format!("{}{:02}:{:02}", offset_sign, offset_hours.abs(), offset_minutes.abs());
                                            format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}", 
                                                local_date.year(), local_date.month() as u8, local_date.day(),
                                                local_time.hour(), local_time.minute(), local_time.second(), offset_str)
                                        }
                                        Err(_) => {
                                            // Fallback to UTC if local offset can't be determined
                                            format!("{} {} UTC", date_part, time_part)
                                        }
                                    }
                                } else {
                                    format!("{} {} UTC", date_part, time_part)
                                }
                            } else {
                                format!("{} {} UTC", date_part, time_part)
                            }
                        } else {
                            format!("{} {} UTC", date_part, time_part)
                        }
                    } else {
                        format!("{} {} UTC", date_part, time_part)
                    }
                } else {
                    format!("{} UTC", s)
                }
            } else {
                format!("{} UTC", s)
            }
        };
        
        let style = if is_selected {
            Style::default().fg(Color::Yellow).add_modifier(ratatui::style::Modifier::REVERSED)
        } else {
            Style::default()
        };
        
        // Line number (1-indexed, newest = 1)
        let line_num = i + 1;
        
        lines.push(Line::from(vec![
            Span::styled(format!("{:>2}. ", line_num), if is_selected { style } else { Style::default().fg(Color::Gray) }),
            Span::styled(if is_selected { "> " } else { "  " }, style),
            Span::styled(timestamp_str, if is_selected { style } else { Style::default().fg(Color::Gray) }),
            Span::raw("  "),
            Span::styled(format!("DL {:>7.2} Mbps", r.download.mbps), if is_selected { style } else { Style::default().fg(Color::Green) }),
            Span::raw("  "),
            Span::styled(format!("UL {:>7.2} Mbps", r.upload.mbps), if is_selected { style } else { Style::default().fg(Color::Cyan) }),
            Span::raw("  "),
            Span::styled(format!("Idle p50 {:>6.1} ms", r.idle_latency.p50_ms.unwrap_or(f64::NAN)), if is_selected { style } else { Style::default() }),
        ]));
    }
    
    if state.history.is_empty() {
        lines.push(Line::from("No history available."));
    }
    
    // Show exported path if available
    if let Some(ref path) = state.last_exported_path {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Last exported: ", Style::default().fg(Color::Gray)),
            Span::styled(path, Style::default().fg(Color::Cyan)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Press ", Style::default().fg(Color::Gray)),
            Span::styled("y", Style::default().fg(Color::Magenta)),
            Span::styled(" to copy path to clipboard", Style::default().fg(Color::Gray)),
        ]));
    }
    
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("History"));
    f.render_widget(p, area);
}

fn max_y(points: &[(f64, f64)]) -> f64 {
    points
        .iter()
        .map(|(_, y)| *y)
        .fold(0.0, |a, b| a.max(b))
}

fn x_bounds(a: &[(f64, f64)], b: &[(f64, f64)]) -> (f64, f64) {
    let xmax = a
        .last()
        .map(|(x, _)| *x)
        .unwrap_or(0.0)
        .max(b.last().map(|(x, _)| *x).unwrap_or(0.0));
    let xmin = 0.0;
    (xmin, xmax)
}

fn window(points: &[(f64, f64)], x_max: f64, secs: f64) -> &[(f64, f64)] {
    let x0 = (x_max - secs).max(0.0);
    let idx = points
        .iter()
        .position(|(x, _)| *x >= x0)
        .unwrap_or(points.len());
    &points[idx..]
}


