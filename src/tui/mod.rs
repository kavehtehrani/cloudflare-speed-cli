mod charts;
mod dashboard;
mod export;
mod help;
mod history;
mod state;

pub use state::UiState;

use crate::cli::{build_config, Cli};
use crate::engine::{EngineControl, TestEngine};
use crate::model::{Phase, RunResult, TestEvent};
use anyhow::{Context, Result};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::{future, StreamExt};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Color,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Tabs},
    Terminal,
};
use std::{io, time::Duration, time::Instant};
use tokio::sync::mpsc;

use charts::draw_charts;
use dashboard::draw_dashboard;
use export::{copy_to_clipboard, enrich_result_with_network_info, export_result_csv, export_result_json, save_and_show_path};
use help::draw_help;
use history::{show_history, draw_history_detail};
use state::update_available_networks;

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
        comments: args.comments.clone(),
        ..Default::default()
    };
    state.initial_history_load_size = initial_load;
    state.history = crate::storage::load_recent(initial_load).unwrap_or_default();
    state.history_loaded_count = state.history.len();
    update_available_networks(&mut state);

    // Gather network interface information using shared module
    let network_info = crate::network::gather_network_info(&args);
    state.interface_name = network_info.interface_name.clone();
    state.network_name = network_info.network_name.clone();
    state.is_wireless = network_info.is_wireless;
    state.interface_mac = network_info.interface_mac.clone();
    state.local_ipv4 = network_info.local_ipv4.clone();
    state.local_ipv6 = network_info.local_ipv6.clone();
    state.certificate_filename = args
        .certificate
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());
    state.proxy_url = args.proxy.clone();

    // Spawn background task to check for updates (non-blocking, silent on error)
    let (update_tx, mut update_rx) = tokio::sync::mpsc::channel::<Option<String>>(1);
    tokio::spawn(async move {
        if let Some(status) = crate::update::check_for_update().await {
            let _ = update_tx.send(status).await;
        }
    });

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    // Start first run if test_on_launch is enabled
    let mut run_ctx = if args.test_on_launch {
        Some(start_run(&args).await?)
    } else {
        None
    };

    let res = loop {
        tokio::select! {
            _ = tick.tick() => {
                terminal.draw(|f| draw(f.area(), f, &state)).ok();
            }
            Some(status) = update_rx.recv() => {
                state.update_status = Some(status);
            }
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else { continue };
                if let Event::Key(k) = ev {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }

                    // Handle filter input mode (when on history tab and editing filter)
                    if state.tab == 1 && state.history_filter_editing {
                        match k.code {
                            KeyCode::Esc => {
                                // Cancel editing, clear filter
                                state.history_filter_editing = false;
                                state.history_filter.clear();
                                state.history_selected = 0;
                                state.history_scroll_offset = 0;
                            }
                            KeyCode::Enter => {
                                // Apply filter and exit editing mode
                                state.history_filter_editing = false;
                                state.history_selected = 0;
                                state.history_scroll_offset = 0;
                            }
                            KeyCode::Backspace => {
                                state.history_filter.pop();
                            }
                            KeyCode::Char(c) => {
                                state.history_filter.push(c);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Handle detail view mode (when on history tab and viewing JSON detail)
                    if state.tab == 1 && state.history_detail_view {
                        match k.code {
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                                // Exit detail view
                                state.history_detail_view = false;
                                state.history_detail_scroll = 0;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                // Scroll up
                                state.history_detail_scroll =
                                    state.history_detail_scroll.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                // Scroll down
                                state.history_detail_scroll += 1;
                            }
                            KeyCode::PageUp => {
                                state.history_detail_scroll =
                                    state.history_detail_scroll.saturating_sub(20);
                            }
                            KeyCode::PageDown => {
                                state.history_detail_scroll += 20;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match (k.modifiers, k.code) {
                        (_, KeyCode::Char('q')) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                            if let Some(ref ctx) = run_ctx {
                                ctx.ctrl_tx.send(EngineControl::Cancel).await.ok();
                            }
                            break Ok(());
                        }
                        (_, KeyCode::Char('p')) => {
                            if let Some(ref ctx) = run_ctx {
                                state.paused = !state.paused;
                                ctx.ctrl_tx.send(EngineControl::Pause(state.paused)).await.ok();
                            }
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
                                        update_available_networks(&mut state);

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
                                if let Some(ref mut ctx) = run_ctx {
                                    ctx.ctrl_tx.send(EngineControl::Cancel).await.ok();
                                    if let Some(h) = ctx.handle.take() {
                                        let _ = h.await;
                                    }
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
                                // Clear diagnostic results
                                state.dns_summary = None;
                                state.tls_summary = None;
                                state.ip_comparison = None;
                                state.traceroute_summary = None;
                                run_ctx = Some(start_run(&args).await?);
                            }
                        }
                        (_, KeyCode::Char('s')) => {
                            // Only save on dashboard (auto-save location)
                            if state.tab == 0 {
                                if let Some(r) = state.last_result.clone() {
                                    save_and_show_path(&r, &mut state);
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
                        (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                            // Shift+Tab cycles backwards
                            let new_tab = if state.tab == 0 { 3 } else { state.tab - 1 };
                            state.tab = new_tab;
                            if new_tab == 1 {
                                state.history_selected = 0;
                                state.history_scroll_offset = 0;
                            }
                        }
                        (_, KeyCode::Tab) => {
                            let new_tab = (state.tab + 1) % 4;
                            state.tab = new_tab;
                            // Reset history selection when switching to history tab
                            if new_tab == 1 {
                                state.history_selected = 0;
                                state.history_scroll_offset = 0;
                            }
                        }
                        (_, KeyCode::Char('?')) => {
                            state.tab = 3; // help
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
                                                update_available_networks(&mut state);
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
                        // Enter key to view JSON detail (only on History tab)
                        (_, KeyCode::Enter) => {
                            if state.tab == 1 && !state.history.is_empty() {
                                state.history_detail_view = true;
                                state.history_detail_scroll = 0;
                            }
                        }
                        // Filter controls (only on History tab)
                        (_, KeyCode::Char('/')) => {
                            if state.tab == 1 {
                                state.history_filter_editing = true;
                            }
                        }
                        (_, KeyCode::Esc) => {
                            if state.tab == 1 && !state.history_filter.is_empty() {
                                // Clear filter when Escape pressed and filter is active
                                state.history_filter.clear();
                                state.history_selected = 0;
                                state.history_scroll_offset = 0;
                            }
                        }
                        // Charts tab: cycle through networks with left/right or h/l
                        (_, KeyCode::Left) | (_, KeyCode::Char('h')) => {
                            if state.tab == 2 && !state.charts_available_networks.is_empty() {
                                // Cycle backwards: All -> last network -> ... -> first network -> All
                                match &state.charts_network_filter {
                                    None => {
                                        // Currently "All", go to last network
                                        state.charts_network_filter = Some(
                                            state.charts_available_networks.last().unwrap().clone(),
                                        );
                                    }
                                    Some(current) => {
                                        // Find current index and go to previous
                                        if let Some(idx) = state
                                            .charts_available_networks
                                            .iter()
                                            .position(|n| n == current)
                                        {
                                            if idx == 0 {
                                                state.charts_network_filter = None; // Go to "All"
                                            } else {
                                                state.charts_network_filter = Some(
                                                    state.charts_available_networks[idx - 1].clone(),
                                                );
                                            }
                                        } else {
                                            state.charts_network_filter = None;
                                        }
                                    }
                                }
                            }
                        }
                        (_, KeyCode::Right) | (_, KeyCode::Char('l')) => {
                            if state.tab == 2 && !state.charts_available_networks.is_empty() {
                                // Cycle forwards: All -> first network -> ... -> last network -> All
                                match &state.charts_network_filter {
                                    None => {
                                        // Currently "All", go to first network
                                        state.charts_network_filter = Some(
                                            state.charts_available_networks.first().unwrap().clone(),
                                        );
                                    }
                                    Some(current) => {
                                        // Find current index and go to next
                                        if let Some(idx) = state
                                            .charts_available_networks
                                            .iter()
                                            .position(|n| n == current)
                                        {
                                            if idx >= state.charts_available_networks.len() - 1 {
                                                state.charts_network_filter = None; // Go to "All"
                                            } else {
                                                state.charts_network_filter = Some(
                                                    state.charts_available_networks[idx + 1].clone(),
                                                );
                                            }
                                        } else {
                                            state.charts_network_filter = None;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            // wrapping in conditional async to avoid spiking cpu usage when run_ctx is None
            maybe_engine_ev = async {
                if let Some(ref mut ctx) = run_ctx {
                    ctx.event_rx.recv().await
                } else {
                    future::pending().await
                }
            } => {
                match maybe_engine_ev {
                    None => {
                        // engine finished; wait for result
                        if let Some(ctx) = &mut run_ctx {
                            if let Some(h) = ctx.handle.take() {
                            match h.await {
                                Ok(Ok(r)) => {
                                    if state.auto_save {
                                        save_and_show_path(&r, &mut state);
                                    }
                                    if let Some(meta) = r.meta.as_ref() {
                                        let extracted = crate::network::extract_metadata(meta);
                                        state.ip = extracted.ip;
                                        state.colo = extracted.colo;
                                        state.asn = extracted.asn;
                                        state.as_org = extracted.as_org;
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
                                    update_available_networks(&mut state);
                                    // Reset selection to show the new test (most recent) if on history tab
                                    if state.tab == 1 {
                                        state.history_selected = 0;
                                        state.history_scroll_offset = 0;
                                    }
                                }
                                Ok(Err(e)) => state.info = format!("Run failed: {e:#}"),
                                Err(e) => state.info = format!("Run join failed: {e}"),
                            }
                            }
                            run_ctx = None;
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
                Phase::PacketLoss => {
                    state.udp_loss_sent = 0;
                    state.udp_loss_received = 0;
                    state.udp_loss_total = 0;
                    state.udp_loss_latest_rtt_ms = None;
                }
                _ => {}
            }
        }
        TestEvent::Info { message } => state.info = message,
        TestEvent::MetaInfo { meta } => {
            // Extract IP, colo, ASN, and org from meta
            let extracted = crate::network::extract_metadata(&meta);
            state.ip = extracted.ip;
            state.colo = extracted.colo;
            state.asn = extracted.asn;
            state.as_org = extracted.as_org;

            // Extract city for server location (if available, use it directly)
            if let Some(city) = meta.get("city").and_then(|v| v.as_str()) {
                // If we have city, use it for server location
                if let Some(country) = meta.get("country").and_then(|v| v.as_str()) {
                    state.server = Some(format!("{}, {}", city, country));
                } else {
                    state.server = Some(city.to_string());
                }
            } else if let Some(ref colo) = state.colo {
                // Use colo code as server if no city available
                state.server = Some(colo.clone());
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
        TestEvent::UdpLossProgress {
            sent,
            received,
            total,
            rtt_ms,
        } => {
            state.udp_loss_sent = sent;
            state.udp_loss_received = received;
            state.udp_loss_total = total;
            state.udp_loss_latest_rtt_ms = rtt_ms;
            let loss_pct = if sent == 0 {
                0.0
            } else {
                ((sent.saturating_sub(received)) as f64) * 100.0 / sent as f64
            };
            state.info = format!(
                "Packet loss probe: {}/{} (loss {:.1}%)",
                sent, total, loss_pct
            );
        }
        // Diagnostic events - store results and display summary in info bar
        TestEvent::DiagnosticDns { summary } => {
            state.info = format!(
                "DNS: {} resolved in {:.2}ms ({} IPs)",
                summary.hostname,
                summary.resolution_time_ms,
                summary.resolved_ips.len()
            );
            state.dns_summary = Some(summary);
        }
        TestEvent::DiagnosticTls { summary } => {
            state.info = format!(
                "TLS: {:.2}ms, {}",
                summary.handshake_time_ms,
                summary.protocol_version.as_deref().unwrap_or("-")
            );
            state.tls_summary = Some(summary);
        }
        TestEvent::DiagnosticIpComparison { comparison } => {
            let v4_info = comparison
                .ipv4_result
                .as_ref()
                .map(|r| {
                    if r.available {
                        format!("v4:{:.0}Mbps", r.download_mbps)
                    } else {
                        "v4:N/A".to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string());
            let v6_info = comparison
                .ipv6_result
                .as_ref()
                .map(|r| {
                    if r.available {
                        format!("v6:{:.0}Mbps", r.download_mbps)
                    } else {
                        "v6:N/A".to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string());
            state.info = format!("IP Comparison: {} / {}", v4_info, v6_info);
            state.ip_comparison = Some(comparison);
        }
        TestEvent::TracerouteHop { hop_number, hop } => {
            let addr = hop.ip_address.as_deref().unwrap_or("*");
            let rtt = hop
                .rtt_ms
                .first()
                .map(|r| format!("{:.1}ms", r))
                .unwrap_or_else(|| "*".to_string());
            state.info = format!("Traceroute hop {}: {} {}", hop_number, addr, rtt);
        }
        TestEvent::TracerouteComplete { summary } => {
            state.info = format!(
                "Traceroute: {} hops to {}",
                summary.hops.len(),
                summary.destination
            );
            state.traceroute_summary = Some(summary);
        }
        TestEvent::ExternalIps { ipv4, ipv6 } => {
            state.external_ipv4 = ipv4;
            state.external_ipv6 = ipv6;
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
        Line::from("Charts"),
        Line::from("Help"),
    ])
    .select(state.tab)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(match &state.update_status {
                Some(Some(v)) => Line::from(vec![
                    Span::raw(format!("cloudflare-speed-cli v{} ", env!("CARGO_PKG_VERSION"))),
                    Span::styled(format!("(v{} available)", v), Style::default().fg(Color::Cyan)),
                ]),
                Some(None) => Line::from(format!("cloudflare-speed-cli v{} (latest)", env!("CARGO_PKG_VERSION"))),
                None => Line::from(format!("cloudflare-speed-cli v{}", env!("CARGO_PKG_VERSION"))),
            }),
    )
    .highlight_style(Style::default().fg(Color::Yellow));
    f.render_widget(tabs, chunks[0]);

    match state.tab {
        0 => draw_dashboard(chunks[1], f, state),
        1 => {
            if state.history_detail_view {
                draw_history_detail(chunks[1], f, state)
            } else {
                show_history(chunks[1], f, state)
            }
        }
        2 => draw_charts(chunks[1], f, state),
        _ => draw_help(chunks[1], f),
    }
}
