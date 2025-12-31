mod charts;

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
    style::Color,
    style::Style,
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Dataset, GraphType, Paragraph, Sparkline, Tabs},
    Terminal,
};
use std::{io, time::Duration, time::Instant};
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
    history_scroll_offset: usize,
    history_loaded_count: usize,
    initial_history_load_size: usize, // Initial load size based on terminal height
    ip: Option<String>,
    colo: Option<String>,
    server: Option<String>,
    asn: Option<String>,
    as_org: Option<String>,
    auto_save: bool,
    last_exported_path: Option<String>,
    // Network interface information
    interface_name: Option<String>,
    network_name: Option<String>,
    is_wireless: Option<bool>,
    interface_mac: Option<String>,
    link_speed_mbps: Option<u64>,
    certificate_filename: Option<String>,
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
            history_scroll_offset: 0,
            history_loaded_count: 0,
            initial_history_load_size: 66, // Default initial load size
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
            certificate_filename: None,
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
        let variance =
            samples.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
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

pub async fn run(args: Cli) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).ok();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear().ok();

    // Get terminal size to determine initial history load
    // Load 3x the visible height initially (for smooth scrolling)
    // Default to 24 rows if we can't get terminal size
    let initial_load = terminal
        .size()
        .map(|size| ((size.height as usize).saturating_sub(2) * 3).max(20))
        .unwrap_or(66); // Default: (24-2)*3 = 66 items

    let mut state = UiState {
        phase: Phase::IdleLatency,
        auto_save: args.auto_save,
        ..Default::default()
    };
    state.initial_history_load_size = initial_load;
    state.history = crate::storage::load_recent(initial_load).unwrap_or_default();
    state.history_loaded_count = state.history.len();

    // Gather network interface information using shared module
    let network_info = crate::network::gather_network_info(&args);
    state.interface_name = network_info.interface_name.clone();
    state.network_name = network_info.network_name.clone();
    state.is_wireless = network_info.is_wireless;
    state.interface_mac = network_info.interface_mac.clone();
    state.link_speed_mbps = network_info.link_speed_mbps;
    state.certificate_filename = args
        .certificate
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

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
                            // Refresh history (only when on history tab)
                            if state.tab == 1 {
                                let reload_size = state.initial_history_load_size.max(state.history_loaded_count);
                                match crate::storage::load_recent(reload_size) {
                                    Ok(new_history) => {
                                        let old_count = state.history.len();
                                        state.history = new_history;
                                        state.history_loaded_count = state.history.len();

                                        // Adjust selection if needed
                                        if state.history_selected >= state.history.len() && !state.history.is_empty() {
                                            state.history_selected = state.history.len() - 1;
                                        } else if state.history.is_empty() {
                                            state.history_selected = 0;
                                            state.history_scroll_offset = 0;
                                        }

                                        // Adjust scroll offset if needed
                                        if state.history_scroll_offset >= state.history.len() && !state.history.is_empty() {
                                            state.history_scroll_offset = state.history.len().saturating_sub(20).max(0);
                                        }

                                        let new_count = state.history.len();
                                        if new_count > old_count {
                                            state.info = format!("Refreshed: {} new run(s)", new_count - old_count);
                                        } else if new_count < old_count {
                                            state.info = format!("Refreshed: {} run(s) removed", old_count - new_count);
                                        } else {
                                            state.info = "Refreshed".into();
                                        }
                                    }
                                    Err(e) => {
                                        state.info = format!("Refresh failed: {e:#}");
                                    }
                                }
                            } else {
                                // Rerun (only when NOT on history tab)
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
                                state.idle_latency_samples.clear();
                                state.loaded_dl_latency_samples.clear();
                                state.loaded_ul_latency_samples.clear();
                                state.idle_latency_sent = 0;
                                state.idle_latency_received = 0;
                                state.loaded_dl_latency_sent = 0;
                                state.loaded_dl_latency_received = 0;
                                state.loaded_ul_latency_sent = 0;
                                state.loaded_ul_latency_received = 0;
                                state.phase = Phase::IdleLatency;
                                state.paused = false;
                                run_ctx = start_run(&args).await?;
                            }
                        }
                        (_, KeyCode::Char('s')) => {
                            // Only save on dashboard (auto-save location)
                            if state.tab == 0 {
                                if let Some(r) = state.last_result.as_ref() {
                                    match save_result_json(r, &state) {
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
                                    match export_result_json(r, &state) {
                                        Ok(p) => {
                                            let path_str = p.to_string_lossy().to_string();
                                            state.last_exported_path = Some(path_str.clone());
                                            state.info = format!("Exported JSON: {} (press 'y' to copy path)", p.display());
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
                                    match export_result_csv(r, &state) {
                                        Ok(p) => {
                                            let path_str = p.to_string_lossy().to_string();
                                            state.last_exported_path = Some(path_str.clone());
                                            state.info = format!("Exported CSV: {} (press 'y' to copy path)", p.display());
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
                                            // Truncate very long paths in the message
                                            let display_path = if path.len() > 60 {
                                                format!("{}...", &path[..57])
                                            } else {
                                                path.clone()
                                            };
                                            state.info = format!("✓ Copied to clipboard: {}", display_path);
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
                                state.history_scroll_offset = 0;
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
                                    // Auto-scroll: if selected item is above visible area, scroll up
                                    if state.history_selected < state.history_scroll_offset {
                                        state.history_scroll_offset = state.history_selected;
                                    }
                                }
                            }
                        }
                        (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                            if state.tab == 1 && !state.history.is_empty() {
                                // Down/j goes to older items (higher index in array)
                                // Allow navigation through all items; display will show what fits
                                if state.history_selected < state.history.len().saturating_sub(1) {
                                    state.history_selected += 1;
                                    // Auto-scroll: keep selected item visible
                                    // Use a reasonable estimate for max_items (will be recalculated in draw)
                                    let estimated_max_items = 30; // reasonable default
                                    if state.history_selected >= state.history_scroll_offset + estimated_max_items {
                                        state.history_scroll_offset = state.history_selected.saturating_sub(estimated_max_items - 1);
                                    }

                                    // Lazy load: if we're near the end of loaded items, load more
                                    let load_threshold = state.history_loaded_count.saturating_sub(10);
                                    if state.history_selected >= load_threshold && state.history_loaded_count == state.history.len() {
                                        // Load more items (another batch of the same size)
                                        let current_count = state.history.len();
                                        let load_more = current_count.max(20); // Load at least as many as we have, or 20
                                        if let Ok(more_history) = crate::storage::load_recent(load_more) {
                                            // Only add items we don't already have
                                            let existing_ids: std::collections::HashSet<_> = state.history
                                                .iter()
                                                .map(|r| &r.meas_id)
                                                .collect();
                                            let new_items: Vec<_> = more_history
                                                .into_iter()
                                                .filter(|r| !existing_ids.contains(&r.meas_id))
                                                .collect();
                                            if !new_items.is_empty() {
                                                state.history.extend(new_items);
                                                state.history_loaded_count = state.history.len();
                                            }
                                        }
                                    }
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
                                        // Adjust scroll offset if needed
                                        if state.history_scroll_offset >= state.history.len() && !state.history.is_empty() {
                                            state.history_scroll_offset = state.history.len().saturating_sub(20).max(0);
                                        }
                                        // Adjust selection if needed
                                        if state.history_selected >= state.history.len() && !state.history.is_empty() {
                                            state.history_selected = state.history.len() - 1;
                                        } else if state.history.is_empty() {
                                            state.history_selected = 0;
                                            state.history_scroll_offset = 0;
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
                                        let enriched = enrich_result_with_network_info(&r, &state);
                                        crate::storage::save_run(&enriched).ok();
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
                                    // Enrich result with network info before storing
                                    let enriched = enrich_result_with_network_info(&r, &state);
                                    state.last_result = Some(enriched.clone());

                                    // Handle command-line export flags
                                    let mut export_messages = Vec::new();
                                    if let Some(export_path) = args.export_json.as_deref() {
                                        match crate::storage::export_json(export_path, &enriched) {
                                            Ok(_) => export_messages.push(format!("Exported JSON: {}", export_path.display())),
                                            Err(e) => export_messages.push(format!("Export JSON failed: {e:#}")),
                                        }
                                    }
                                    if let Some(export_path) = args.export_csv.as_deref() {
                                        match crate::storage::export_csv(export_path, &enriched) {
                                            Ok(_) => export_messages.push(format!("Exported CSV: {}", export_path.display())),
                                            Err(e) => export_messages.push(format!("Export CSV failed: {e:#}")),
                                        }
                                    }
                                    if !export_messages.is_empty() {
                                        state.info = export_messages.join("; ");
                                    }

                                    // Reload history to include the new test
                                    // Load at least one more than we had before to ensure the new test is included
                                    let reload_size = (state.history_loaded_count + 1).max(state.initial_history_load_size);
                                    state.history = crate::storage::load_recent(reload_size).unwrap_or_default();
                                    state.history_loaded_count = state.history.len();
                                    // Reset selection to show the new test (most recent) if on history tab
                                    if state.tab == 1 {
                                        state.history_selected = 0;
                                        state.history_scroll_offset = 0;
                                    }
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
            state.asn = meta.get("asn").and_then(|v| {
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
        TestEvent::LatencySample {
            phase,
            during,
            rtt_ms,
            ok,
        } => {
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
                                state
                                    .idle_latency_samples
                                    .drain(0..(state.idle_latency_samples.len() - 10000));
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
                                state
                                    .loaded_dl_latency_samples
                                    .drain(0..(state.loaded_dl_latency_samples.len() - 10000));
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
                                state
                                    .loaded_ul_latency_samples
                                    .drain(0..(state.loaded_ul_latency_samples.len() - 10000));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        TestEvent::ThroughputTick {
            phase,
            bytes_total,
            bps_instant,
        } => {
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
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("cloudflare-speed-cli"),
    )
    .highlight_style(Style::default().fg(Color::Yellow));
    f.render_widget(tabs, chunks[0]);

    match state.tab {
        0 => draw_dashboard(chunks[1], f, state),
        1 => draw_history(chunks[1], f, state),
        _ => draw_help(chunks[1], f),
    }
}

/// Helper function to render a box plot with metrics inside the same bordered box

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
                Constraint::Length(13), // Throughput charts row with metrics (side-by-side)
                Constraint::Length(10), // Latency box plots with metrics below (idle + loaded DL + loaded UL)
                Constraint::Min(0),     // Network Information + Keyboard Shortcuts (side-by-side)
                Constraint::Length(5),  // Status row (full width at bottom)
            ]
            .as_ref(),
        )
        .split(area);

    // Throughput charts side-by-side: DL left, UL right
    let thr_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(main[0]);

    // Download throughput chart (left) - only show when download phase has data
    if state.dl_phase_start.is_some() && !state.dl_points.is_empty() {
        // Calculate x bounds only for download points
        let dl_x_max = state.dl_points.last().map(|(x, _)| *x).unwrap_or(0.0);
        let dl_x_min = state.dl_points.first().map(|(x, _)| *x).unwrap_or(0.0);

        let y_dl_max = max_y(&state.dl_points).max(10.0);
        let y_dl_max = (y_dl_max * 1.10).min(10_000.0);

        // Use all download points (they're already filtered to download phase)
        let dl_ds = Dataset::default()
            .graph_type(GraphType::Line)
            .marker(symbols::Marker::Braille)
            .style(Style::default().fg(Color::Green))
            .data(&state.dl_points);

        let dl_metrics = crate::metrics::compute_throughput_metrics(&state.dl_points);
        // Use the computed mean from metrics for the title to match what's shown below
        let dl_avg = dl_metrics
            .map(|(mean, _, _, _)| mean)
            .unwrap_or(state.dl_avg_mbps);
        let dl_title = Line::from(vec![
            Span::raw("Download (inst "),
            Span::styled(
                format!("{:.0}", state.dl_mbps),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" / avg "),
            Span::styled(format!("{:.0}", dl_avg), Style::default().fg(Color::Green)),
            Span::raw(" Mbps)"),
        ]);
        charts::render_chart_with_metrics_inside(
            f,
            thr_row[0],
            vec![dl_ds],
            Axis::default().bounds([dl_x_min, dl_x_max.max(1.0)]),
            Axis::default().title("Mbps").bounds([0.0, y_dl_max]),
            dl_title,
            dl_metrics,
            Color::Green,
        );
    } else {
        // Show empty placeholder when download hasn't started
        let empty_chart = Paragraph::new("Waiting for download phase...").block(
            Block::default()
                .borders(Borders::ALL)
                .title(Line::from(vec![
                    Span::raw("Download (inst "),
                    Span::styled(
                        format!("{:.0}", state.dl_mbps),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(" / avg "),
                    Span::styled(
                        format!("{:.0}", state.dl_avg_mbps),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(" Mbps)"),
                ])),
        );
        f.render_widget(empty_chart, thr_row[0]);
    }

    // Upload throughput chart (right) - only show when upload phase has data
    if state.ul_phase_start.is_some() && !state.ul_points.is_empty() {
        // Calculate x bounds only for upload points
        let ul_x_max = state.ul_points.last().map(|(x, _)| *x).unwrap_or(0.0);
        let ul_x_min = state.ul_points.first().map(|(x, _)| *x).unwrap_or(0.0);

        let y_ul_max = max_y(&state.ul_points).max(10.0);
        let y_ul_max = (y_ul_max * 1.10).min(10_000.0);

        // Use all upload points (they're already filtered to upload phase)
        let ul_ds = Dataset::default()
            .graph_type(GraphType::Line)
            .marker(symbols::Marker::Braille)
            .style(Style::default().fg(Color::Cyan))
            .data(&state.ul_points);

        let ul_metrics = crate::metrics::compute_throughput_metrics(&state.ul_points);
        // Use the computed mean from metrics for the title to match what's shown below
        let ul_avg = ul_metrics
            .map(|(mean, _, _, _)| mean)
            .unwrap_or(state.ul_avg_mbps);
        let ul_title = Line::from(vec![
            Span::raw("Upload (inst "),
            Span::styled(
                format!("{:.0}", state.ul_mbps),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(" / avg "),
            Span::styled(format!("{:.0}", ul_avg), Style::default().fg(Color::Cyan)),
            Span::raw(" Mbps)"),
        ]);
        charts::render_chart_with_metrics_inside(
            f,
            thr_row[1],
            vec![ul_ds],
            Axis::default().bounds([ul_x_min, ul_x_max.max(1.0)]),
            Axis::default().title("Mbps").bounds([0.0, y_ul_max]),
            ul_title,
            ul_metrics,
            Color::Cyan,
        );
    } else {
        // Show empty placeholder when upload hasn't started
        let empty_chart = Paragraph::new("Waiting for upload phase...").block(
            Block::default()
                .borders(Borders::ALL)
                .title(Line::from(vec![
                    Span::raw("Upload (inst "),
                    Span::styled(
                        format!("{:.0}", state.ul_mbps),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw(" / avg "),
                    Span::styled(
                        format!("{:.0}", state.ul_avg_mbps),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw(" Mbps)"),
                ])),
        );
        f.render_widget(empty_chart, thr_row[1]);
    }

    // Latency box plots: Idle, Loaded DL, Loaded UL (with metrics inside each box)
    let lat_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage(33),
                Constraint::Percentage(33),
                Constraint::Percentage(34),
            ]
            .as_ref(),
        )
        .split(main[1]);

    // Idle latency
    if state.idle_latency_samples.len() >= 2 {
        // Use the same median calculation as the metrics below
        let median = crate::metrics::compute_latency_metrics(&state.idle_latency_samples)
            .map(|(_, med, _, _)| med)
            .unwrap_or(f64::NAN);
        let title = Line::from(format!("Idle Latency ({:.0}ms)", median));
        charts::render_box_plot_with_metrics_inside(
            f,
            lat_row[0],
            &state.idle_latency_samples,
            title,
            None,
        );
    } else {
        let empty = Paragraph::new("Waiting for data...")
            .block(Block::default().borders(Borders::ALL).title("Idle Latency"));
        f.render_widget(empty, lat_row[0]);
    }

    // Download latency
    if state.loaded_dl_latency_samples.len() >= 2 {
        // Use the same median calculation as the metrics below
        let median = crate::metrics::compute_latency_metrics(&state.loaded_dl_latency_samples)
            .map(|(_, med, _, _)| med)
            .unwrap_or(f64::NAN);
        let title = Line::from(vec![
            Span::raw("Latency Download ("),
            Span::styled(
                format!("{:.0}ms", median),
                Style::default().fg(Color::Green),
            ),
            Span::raw(")"),
        ]);
        charts::render_box_plot_with_metrics_inside(
            f,
            lat_row[1],
            &state.loaded_dl_latency_samples,
            title,
            Some(Color::Green),
        );
    } else {
        let empty = Paragraph::new("Waiting for data...").block(
            Block::default()
                .borders(Borders::ALL)
                .title("Latency Download"),
        );
        f.render_widget(empty, lat_row[1]);
    }

    // Upload latency
    if state.loaded_ul_latency_samples.len() >= 2 {
        // Use the same median calculation as the metrics below
        let median = crate::metrics::compute_latency_metrics(&state.loaded_ul_latency_samples)
            .map(|(_, med, _, _)| med)
            .unwrap_or(f64::NAN);
        let title = Line::from(vec![
            Span::raw("Latency Upload ("),
            Span::styled(format!("{:.0}ms", median), Style::default().fg(Color::Cyan)),
            Span::raw(")"),
        ]);
        charts::render_box_plot_with_metrics_inside(
            f,
            lat_row[2],
            &state.loaded_ul_latency_samples,
            title,
            Some(Color::Cyan),
        );
    } else {
        let empty = Paragraph::new("Waiting for data...").block(
            Block::default()
                .borders(Borders::ALL)
                .title("Latency Upload"),
        );
        f.render_widget(empty, lat_row[2]);
    }

    // Network Information and Keyboard Shortcuts side-by-side
    let info_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)].as_ref())
        .split(main[2]);

    // Network Information panel (left)
    let saved_path = state
        .last_result
        .as_ref()
        .and_then(|r| crate::storage::get_run_path(r).ok());

    // Determine IP version
    let ip_version = state
        .ip
        .as_deref()
        .map(|ip| if ip.contains(':') { "IPv6" } else { "IPv4" })
        .unwrap_or("-");

    let mut network_lines = vec![
        Line::from(vec![
            Span::styled("Connected via: ", Style::default().fg(Color::Gray)),
            Span::raw(ip_version),
        ]),
        Line::from(vec![
            Span::styled("Interface: ", Style::default().fg(Color::Gray)),
            Span::raw(state.interface_name.as_deref().unwrap_or("-")),
            Span::raw(" ("),
            Span::raw(if state.is_wireless.unwrap_or(false) {
                "Wireless"
            } else {
                "Wired"
            }),
            Span::raw(")"),
        ]),
        Line::from(vec![
            Span::styled("Network: ", Style::default().fg(Color::Gray)),
            Span::raw(
                state
                    .network_name
                    .as_deref()
                    .or_else(|| state.interface_name.as_deref())
                    .unwrap_or("-"),
            ),
        ]),
        Line::from(vec![
            Span::styled("MAC address: ", Style::default().fg(Color::Gray)),
            Span::raw(state.interface_mac.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Link speed: ", Style::default().fg(Color::Gray)),
            Span::raw(
                state
                    .link_speed_mbps
                    .map(|s| format!("{} Mbps", s))
                    .unwrap_or_else(|| "-".to_string()),
            ),
        ]),
    ];

    // Only show Certificate line if a certificate is set
    if let Some(ref cert_filename) = state.certificate_filename {
        network_lines.push(Line::from(vec![
            Span::styled("Certificate: ", Style::default().fg(Color::Gray)),
            Span::raw(cert_filename),
        ]));
    }

    network_lines.extend(vec![
        Line::from(vec![
            Span::styled("Server location: ", Style::default().fg(Color::Gray)),
            Span::raw(state.server.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Your network: ", Style::default().fg(Color::Gray)),
            Span::raw(match (state.as_org.as_deref(), state.asn.as_deref()) {
                (Some(org), Some(asn)) => format!("{} (AS{})", org, asn),
                (Some(org), None) => org.to_string(),
                (None, Some(asn)) => format!("AS{}", asn),
                (None, None) => "-".to_string(),
            }),
        ]),
        Line::from(vec![
            Span::styled("Your IP address: ", Style::default().fg(Color::Gray)),
            Span::raw(state.ip.as_deref().unwrap_or("-")),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Source: ", Style::default().fg(Color::Gray)),
            Span::styled(
                "https://speed.cloudflare.com/",
                Style::default().fg(Color::Blue),
            ),
        ]),
    ]);

    let network_info = Paragraph::new(network_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Network Information"),
    );
    f.render_widget(network_info, info_row[0]);

    // Keyboard Shortcuts panel (right)
    let shortcuts_lines = vec![
        Line::from(vec![
            Span::raw("  "),
            Span::styled("q", Style::default().fg(Color::Magenta)),
            Span::raw("     Quit"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("r", Style::default().fg(Color::Magenta)),
            Span::raw("     Rerun test"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("p", Style::default().fg(Color::Magenta)),
            Span::raw("     Pause/Resume"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("s", Style::default().fg(Color::Magenta)),
            Span::raw("     Save JSON"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("a", Style::default().fg(Color::Magenta)),
            Span::raw("     Toggle auto-save"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("tab", Style::default().fg(Color::Magenta)),
            Span::raw("   Switch tabs"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("?", Style::default().fg(Color::Magenta)),
            Span::raw("     Help"),
        ]),
    ];

    let shortcuts = Paragraph::new(shortcuts_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Keyboard Shortcuts"),
    );
    f.render_widget(shortcuts, info_row[1]);

    // Status panel (full width at bottom)
    let status_lines = vec![
        Line::from(vec![
            Span::styled("Phase: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{:?}", state.phase)),
            Span::raw("   "),
            Span::styled("Paused: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}", state.paused)),
            Span::raw("   "),
            Span::styled("Auto-save: ", Style::default().fg(Color::Gray)),
            Span::styled(
                if state.auto_save { "ON" } else { "OFF" },
                if state.auto_save {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("Info: ", Style::default().fg(Color::Gray)),
            Span::raw(&state.info),
        ]),
        Line::from(vec![
            Span::styled("Saved JSON: ", Style::default().fg(Color::Gray)),
            Span::raw(
                saved_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("none"),
            ),
        ]),
    ];

    let status =
        Paragraph::new(status_lines).block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(status, main[3]);
}

fn draw_dashboard_compact(area: Rect, f: &mut ratatui::Frame, state: &UiState) {
    // Split into top (sparklines) and bottom (text boxes)
    let content = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(8)].as_ref())
        .split(area);

    // Top row: Download and Upload sparklines side by side
    let top_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(content[0]);

    // Download sparkline with speed in title (numbers colored green)
    f.render_widget(
        Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Line::from(vec![
                        Span::raw("Download (inst "),
                        Span::styled(
                            format!("{:.0}", state.dl_mbps),
                            Style::default().fg(Color::Green),
                        ),
                        Span::raw(" / avg "),
                        Span::styled(
                            format!("{:.0}", state.dl_avg_mbps),
                            Style::default().fg(Color::Green),
                        ),
                        Span::raw(" Mbps)"),
                    ])),
            )
            .data(&state.dl_series)
            .style(Style::default().fg(Color::Green)),
        top_row[0],
    );

    // Upload sparkline with speed in title (numbers colored cyan)
    f.render_widget(
        Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Line::from(vec![
                        Span::raw("Upload (inst "),
                        Span::styled(
                            format!("{:.0}", state.ul_mbps),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(" / avg "),
                        Span::styled(
                            format!("{:.0}", state.ul_avg_mbps),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(" Mbps)"),
                    ])),
            )
            .data(&state.ul_series)
            .style(Style::default().fg(Color::Cyan)),
        top_row[1],
    );

    // Bottom row: Idle latency text box and Status box side by side
    let bottom_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(content[1]);

    // Idle latency stats text box
    let idle_lat = if state.idle_latency_samples.is_empty() && state.idle_latency_sent == 0 {
        None
    } else {
        Some(UiState::compute_live_latency_stats(
            &state.idle_latency_samples,
            state.idle_latency_sent,
            state.idle_latency_received,
        ))
    };
    let format_latency = |lat: &crate::model::LatencySummary| -> Vec<Line> {
        vec![
            Line::from(vec![
                Span::styled("p50: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.p50_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("p90: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.p90_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("p99: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.p99_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("Jitter: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.jitter_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("Loss: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0}%", lat.loss * 100.0)),
            ]),
        ]
    };
    let idle_stats = Paragraph::new(
        idle_lat
            .as_ref()
            .map(format_latency)
            .unwrap_or_else(|| vec![Line::from("Waiting for data...")]),
    )
    .block(Block::default().borders(Borders::ALL).title("Idle Latency"));
    f.render_widget(idle_stats, bottom_row[0]);

    let mut meta_lines = vec![
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
            Span::raw(if state.is_wireless.unwrap_or(false) {
                "Wireless"
            } else {
                "Wired"
            }),
            Span::raw(")"),
        ]),
        Line::from(vec![
            Span::styled("Network: ", Style::default().fg(Color::Gray)),
            Span::raw(
                state
                    .network_name
                    .as_deref()
                    .or_else(|| state.interface_name.as_deref())
                    .unwrap_or("-"),
            ),
        ]),
    ];

    // Only show Certificate line if a certificate is set
    if let Some(ref cert_filename) = state.certificate_filename {
        meta_lines.push(Line::from(vec![
            Span::styled("Certificate: ", Style::default().fg(Color::Gray)),
            Span::raw(cert_filename),
        ]));
    }

    meta_lines.extend(vec![
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
    ]);

    let meta = Paragraph::new(meta_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Network Information"),
    );
    f.render_widget(meta, bottom_row[1]);
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
            Span::raw("           Save JSON"),
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
        Line::from(vec![
            Span::raw("  "),
            Span::styled("r", Style::default().fg(Color::Magenta)),
            Span::raw("           Refresh history"),
        ]),
        Line::from(""),
        Line::from("Repository:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "https://github.com/kavehtehrani/cloudflare-speed-cli",
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(p, area);
}

/// Enrich RunResult with network information from UiState.
/// This uses the shared enrichment function and then adds TUI-specific state (IP, colo, etc.)
fn enrich_result_with_network_info(r: &RunResult, state: &UiState) -> RunResult {
    // Create NetworkInfo from UiState
    let network_info = crate::network::NetworkInfo {
        interface_name: state.interface_name.clone(),
        network_name: state.network_name.clone(),
        is_wireless: state.is_wireless,
        interface_mac: state.interface_mac.clone(),
        link_speed_mbps: state.link_speed_mbps,
    };

    // Use shared enrichment function
    let mut enriched = crate::network::enrich_result(r, &network_info);

    // Override with TUI state values (which may have been updated from meta)
    enriched.ip = state.ip.clone();
    enriched.colo = state.colo.clone();
    enriched.asn = state.asn.clone();
    enriched.as_org = state.as_org.clone();

    // Server might already be set, but update from state if available
    if enriched.server.is_none() {
        enriched.server = state.server.clone();
    }
    enriched
}

/// Save JSON to the default auto-save location.
fn save_result_json(r: &RunResult, state: &UiState) -> Result<std::path::PathBuf> {
    let enriched = enrich_result_with_network_info(r, state);
    crate::storage::save_run(&enriched)
}

/// Export JSON to a user-specified file location.
/// Returns the absolute path of the exported file.
fn export_result_json(r: &RunResult, state: &UiState) -> Result<std::path::PathBuf> {
    // Generate a default filename based on timestamp
    let default_name = format!(
        "cloudflare-speed-{}-{}.json",
        r.timestamp_utc.replace(':', "-").replace('T', "_"),
        &r.meas_id[..8.min(r.meas_id.len())]
    );

    // Get absolute path from current directory
    let current_dir = std::env::current_dir().context("get current directory")?;
    let path = current_dir.join(default_name);
    let enriched = enrich_result_with_network_info(r, state);
    crate::storage::export_json(&path, &enriched)?;
    Ok(path)
}

/// Export CSV to a user-specified file location.
/// Returns the absolute path of the exported file.
fn export_result_csv(r: &RunResult, state: &UiState) -> Result<std::path::PathBuf> {
    // Generate a default filename based on timestamp
    let default_name = format!(
        "cloudflare-speed-{}-{}.csv",
        r.timestamp_utc.replace(':', "-").replace('T', "_"),
        &r.meas_id[..8.min(r.meas_id.len())]
    );

    // Get absolute path from current directory
    let current_dir = std::env::current_dir().context("get current directory")?;
    let path = current_dir.join(default_name);
    let enriched = enrich_result_with_network_info(r, state);
    crate::storage::export_csv(&path, &enriched)?;
    Ok(path)
}

// Global clipboard manager channel - initialized once on first use
use std::sync::mpsc as std_mpsc;
use std::sync::OnceLock;

static CLIPBOARD_SENDER: OnceLock<std_mpsc::Sender<String>> = OnceLock::new();

/// Initialize the clipboard manager thread if not already initialized.
/// This creates a background thread that processes clipboard operations sequentially,
/// keeping each clipboard instance alive for a sufficient duration.
fn init_clipboard_manager() -> Result<&'static std_mpsc::Sender<String>> {
    CLIPBOARD_SENDER.get_or_init(|| {
        let (tx, rx) = std_mpsc::channel::<String>();

        // Spawn a dedicated thread to manage clipboard operations
        std::thread::spawn(move || {
            use arboard::Clipboard;

            for text in rx {
                // Create a new clipboard instance for each operation
                if let Ok(mut clipboard) = Clipboard::new() {
                    // Set the text
                    if clipboard.set_text(&text).is_ok() {
                        // Keep the clipboard instance alive for 2 seconds
                        // This gives clipboard managers plenty of time to read the contents
                        std::thread::sleep(Duration::from_secs(2));
                    }
                    // Clipboard is dropped here
                }
            }
        });

        tx
    });

    CLIPBOARD_SENDER
        .get()
        .ok_or_else(|| anyhow::anyhow!("Failed to initialize clipboard manager"))
}

/// Copy text to clipboard.
/// Uses a background thread manager to keep clipboard instances alive for a sufficient duration
/// to ensure clipboard managers have time to read the contents on Linux.
/// Returns immediately after queuing the clipboard operation, without blocking the main thread.
fn copy_to_clipboard(text: &str) -> Result<()> {
    let sender = init_clipboard_manager()?;
    sender
        .send(text.to_string())
        .map_err(|_| anyhow::anyhow!("Clipboard manager channel closed"))?;
    Ok(())
}

fn draw_history(area: Rect, f: &mut ratatui::Frame, state: &UiState) {
    let mut lines: Vec<Line> = Vec::new();

    // Calculate how many items can fit in the available area
    // Subtract 2 for header lines
    let max_items = (area.height as usize).saturating_sub(2);

    // Show total count and current position
    let total_count = state.history.len();
    let current_pos = if total_count > 0 {
        state.history_selected + 1
    } else {
        0
    };

    lines.push(Line::from(vec![
        Span::raw(format!("History ({}/{}", current_pos, total_count)),
        if total_count > max_items {
            Span::raw(format!(", showing {} items", max_items))
        } else {
            Span::raw("")
        },
        Span::raw(") - "),
        Span::styled("↑/↓/j/k", Style::default().fg(Color::Magenta)),
        Span::raw(": navigate, "),
        Span::styled("r", Style::default().fg(Color::Magenta)),
        Span::raw(": refresh, "),
        Span::styled("d", Style::default().fg(Color::Magenta)),
        Span::raw(": delete, "),
        Span::styled("e", Style::default().fg(Color::Magenta)),
        Span::raw(": export JSON, "),
        Span::styled("c", Style::default().fg(Color::Magenta)),
        Span::raw(": export CSV"),
    ]));
    lines.push(Line::from(""));

    // Apply scroll offset and take only visible items
    // Auto-adjust scroll to keep selected item visible (this should have been done in event handler, but handle edge cases here)
    let scroll_offset = {
        let mut offset = state
            .history_scroll_offset
            .min(state.history.len().saturating_sub(1));
        // Ensure selected item is visible
        if state.history_selected < offset {
            offset = state.history_selected;
        } else if state.history_selected >= offset + max_items {
            offset = state.history_selected.saturating_sub(max_items - 1);
        }
        offset
    };

    let history_display: Vec<_> = state
        .history
        .iter()
        .skip(scroll_offset)
        .take(max_items)
        .collect();

    for (display_idx, r) in history_display.iter().enumerate() {
        // Calculate actual history index (accounting for scroll offset)
        let history_idx = scroll_offset + display_idx;
        let is_selected = state.tab == 1 && history_idx == state.history_selected;

        // Parse and format timestamp to human-readable format in local timezone
        let timestamp_str: String = {
            let s = &r.timestamp_utc;
            // Parse RFC3339 format manually and convert to local time
            // Format: "2024-01-15T14:30:45Z" or "2024-01-15T14:30:45+00:00"
            if s.len() >= 19 && s.contains('T') {
                let date_time: String = s.chars().take(19).collect();
                if let Some(t_pos) = date_time.find('T') {
                    let date_part = &date_time[..t_pos];
                    let time_part = &date_time[t_pos + 1..];

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
                                    let utc_dt =
                                        time::PrimitiveDateTime::new(date, time).assume_utc();

                                    // Get local offset and convert
                                    match time::UtcOffset::current_local_offset() {
                                        Ok(local_offset) => {
                                            let local_dt = utc_dt.to_offset(local_offset);
                                            let local_date = local_dt.date();
                                            let local_time = local_dt.time();
                                            // Format offset as +HH:MM or -HH:MM
                                            let offset_hours = local_offset.whole_hours();
                                            let offset_minutes = local_offset.whole_minutes() % 60;
                                            let offset_sign =
                                                if offset_hours >= 0 { '+' } else { '-' };
                                            let offset_str = format!(
                                                "{}{:02}:{:02}",
                                                offset_sign,
                                                offset_hours.abs(),
                                                offset_minutes.abs()
                                            );
                                            format!(
                                                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
                                                local_date.year(),
                                                local_date.month() as u8,
                                                local_date.day(),
                                                local_time.hour(),
                                                local_time.minute(),
                                                local_time.second(),
                                                offset_str
                                            )
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
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(ratatui::style::Modifier::REVERSED)
        } else {
            Style::default()
        };

        // Line number (1-indexed, newest = 1)
        let line_num = history_idx + 1;

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>2}. ", line_num),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::styled(if is_selected { "> " } else { "  " }, style),
            Span::styled(
                timestamp_str,
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::raw("  "),
            Span::styled(
                format!("DL {:>7.0} Mbps", r.download.mbps),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
            Span::raw("  "),
            Span::styled(
                format!("UL {:>7.0} Mbps", r.upload.mbps),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Cyan)
                },
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "Idle p50 {:>6.0} ms",
                    r.idle_latency.p50_ms.unwrap_or(f64::NAN)
                ),
                if is_selected { style } else { Style::default() },
            ),
            Span::raw("  "),
            Span::styled(
                format!("{}", r.interface_name.as_deref().unwrap_or("-")),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Blue)
                },
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{}",
                    r.network_name
                        .as_deref()
                        .or_else(|| r.interface_name.as_deref())
                        .unwrap_or("-")
                ),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Magenta)
                },
            ),
        ]));
    }

    if state.history.is_empty() {
        lines.push(Line::from("No history available."));
    }

    // Show exported path if available
    if let Some(ref path) = state.last_exported_path {
        lines.push(Line::from(""));

        // Wrap long paths to fit within the available width
        // Account for borders (2 chars on each side)
        let available_width = area.width.saturating_sub(4); // borders
        let prefix = "Last exported: ";
        let prefix_len = prefix.chars().count() as u16;
        let max_path_width = available_width.saturating_sub(prefix_len);

        // Split path into chunks that fit
        let path_str = path.as_str();
        let mut remaining = path_str;
        let mut is_first_line = true;

        while !remaining.is_empty() {
            let line_width = if is_first_line {
                // First line can use less width since we have the prefix
                max_path_width.max(1)
            } else {
                // Subsequent lines can use full width (with 2 char indent)
                available_width.saturating_sub(2).max(1)
            };

            let remaining_chars = remaining.chars().count() as u16;
            if remaining_chars <= line_width {
                // Entire remaining path fits
                if is_first_line {
                    lines.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Gray)),
                        Span::styled(remaining, Style::default().fg(Color::Cyan)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(remaining, Style::default().fg(Color::Cyan)),
                    ]));
                }
                break;
            } else {
                // Need to split - find a good break point
                let mut char_count = 0;
                let mut last_sep_pos = None;
                let mut break_pos = 0;

                for (idx, ch) in remaining.char_indices() {
                    if char_count >= line_width {
                        break;
                    }
                    if ch == '/' || ch == '\\' {
                        last_sep_pos = Some(idx);
                    }
                    break_pos = idx + ch.len_utf8();
                    char_count += 1;
                }

                // Prefer breaking at path separator, otherwise break at line width
                let split_pos = if let Some(sep_pos) = last_sep_pos {
                    if sep_pos > 0 {
                        sep_pos + 1 // Include the separator
                    } else {
                        break_pos
                    }
                } else {
                    break_pos
                };

                let (chunk, rest) = remaining.split_at(split_pos);
                if is_first_line {
                    lines.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Gray)),
                        Span::styled(chunk, Style::default().fg(Color::Cyan)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(chunk, Style::default().fg(Color::Cyan)),
                    ]));
                }
                remaining = rest;
                is_first_line = false;
            }
        }

        lines.push(Line::from(vec![
            Span::styled("Press ", Style::default().fg(Color::Gray)),
            Span::styled("y", Style::default().fg(Color::Magenta)),
            Span::styled(
                " to copy path to clipboard",
                Style::default().fg(Color::Gray),
            ),
        ]));
    }

    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("History"));
    f.render_widget(p, area);
}

fn max_y(points: &[(f64, f64)]) -> f64 {
    points.iter().map(|(_, y)| *y).fold(0.0, |a, b| a.max(b))
}

// Compute latency metrics (mean, median, 25th percentile, 75th percentile) from samples
