mod charts;
mod dashboard;
mod export;
mod help;
mod history;
mod log_style;
mod state;
mod traceroute;

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
use history::{
    draw_history_comment_modal, draw_history_detail, draw_history_export_modal, draw_history_menu,
    show_history,
};
use state::update_available_networks;

/// Puts the user's shell back in order: raw mode off, alternate screen left.
/// Safe to call more than once.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

/// Restores the terminal on drop, so every exit path out of `run` (normal
/// return, `?` early return, or panic unwinding) leaves the shell usable
/// instead of stuck in raw mode inside the alternate screen.
struct TerminalRestoreGuard;

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub async fn run(args: Cli) -> Result<()> {
    // Restore the terminal before the panic message prints; otherwise it
    // lands invisible inside the alternate screen and the shell needs `reset`.
    // Every panic is then fatal: the hook fires for spawned-task panics too,
    // and once the terminal is restored the TUI must not keep drawing into
    // the primary screen, so exit instead of limping on.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original_hook(info);
        std::process::exit(1);
    }));

    enable_raw_mode().context("enable raw mode")?;
    let _restore_guard = TerminalRestoreGuard;
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
        hide_network_info: args.hide_network_info,
        ..Default::default()
    };
    state.history.initial_load_size = initial_load;
    state.history.runs = crate::storage::load_recent(initial_load).unwrap_or_default();
    state.history.loaded_count = state.history.runs.len();
    update_available_networks(&mut state);

    // Gather network interface information using shared module
    let network_info = crate::network::gather_network_info(&args);
    state.network.interface_name = network_info.interface_name.clone();
    state.network.network_name = network_info.network_name.clone();
    state.network.is_wireless = network_info.is_wireless;
    state.network.interface_mac = network_info.interface_mac.clone();
    state.network.local_ipv4 = network_info.local_ipv4.clone();
    state.network.local_ipv6 = network_info.local_ipv6.clone();
    state.network.certificate_filename = args
        .certificate
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());
    state.network.proxy_url = args.proxy.clone();
    state.diagnostics.traceroute_enabled = args.traceroute;
    state.diagnostics.traceroute_max_hops = args.traceroute_max_hops;

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
                terminal.draw(|f| draw(f.area(), f, &mut state)).ok();
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
                    if state.tab == 1 && state.history.filter_editing {
                        match k.code {
                            KeyCode::Esc => {
                                // Cancel editing, clear filter
                                state.history.filter_editing = false;
                                state.history.filter.clear();
                                state.history.selected = 0;
                                state.history.scroll_offset = 0;
                            }
                            KeyCode::Enter => {
                                // Apply filter and exit editing mode
                                state.history.filter_editing = false;
                                state.history.selected = 0;
                                state.history.scroll_offset = 0;
                            }
                            KeyCode::Backspace => {
                                state.history.filter.pop();
                            }
                            KeyCode::Char(c) => {
                                state.history.filter.push(c);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Handle comment editor modal (text input mode)
                    if state.tab == 1 && state.history.comment_modal_open {
                        handle_history_comment_modal_key(&mut state, k);
                        continue;
                    }

                    // Handle export result modal (when on history tab and modal is open)
                    if state.tab == 1 && state.history.export_modal_open {
                        handle_history_export_modal_key(&mut state, k.code);
                        continue;
                    }

                    // Handle context menu mode (when on history tab and menu is open)
                    if state.tab == 1 && state.history.menu_open {
                        handle_history_menu_key(&mut state, k.code);
                        continue;
                    }

                    // Handle detail view mode (when on history tab and viewing JSON detail)
                    if state.tab == 1 && state.history.detail_view {
                        handle_history_detail_key(&mut state, k);
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
                        (_, KeyCode::Char('H')) => {
                            state.hide_network_info = !state.hide_network_info;
                        }
                        (_, KeyCode::Char('r')) => {
                            // Refresh history (only when on history tab)
                            if state.tab == 1 {
                                let reload_size = state.history.initial_load_size.max(state.history.loaded_count);
                                match crate::storage::load_recent(reload_size) {
                                    Ok(new_history) => {
                                        let old_count = state.history.runs.len();
                                        state.history.runs = new_history;
                                        state.history.loaded_count = state.history.runs.len();
                                        update_available_networks(&mut state);

                                        // Adjust selection if needed
                                        if state.history.selected >= state.history.runs.len() && !state.history.runs.is_empty() {
                                            state.history.selected = state.history.runs.len() - 1;
                                        } else if state.history.runs.is_empty() {
                                            state.history.selected = 0;
                                            state.history.scroll_offset = 0;
                                        }

                                        // Adjust scroll offset if needed
                                        if state.history.scroll_offset >= state.history.runs.len() && !state.history.runs.is_empty() {
                                            state.history.scroll_offset = state.history.runs.len().saturating_sub(20).max(0);
                                        }

                                        let new_count = state.history.runs.len();
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
                                state.reset_for_new_run();
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
                            let tab_count = if state.diagnostics.traceroute_enabled { 5 } else { 4 };
                            let new_tab = if state.tab == 0 { tab_count - 1 } else { state.tab - 1 };
                            state.tab = new_tab;
                            if new_tab == 1 {
                                state.history.selected = 0;
                                state.history.scroll_offset = 0;
                            }
                        }
                        (_, KeyCode::Tab) => {
                            let tab_count = if state.diagnostics.traceroute_enabled { 5 } else { 4 };
                            let new_tab = (state.tab + 1) % tab_count;
                            state.tab = new_tab;
                            // Reset history selection when switching to history tab
                            if new_tab == 1 {
                                state.history.selected = 0;
                                state.history.scroll_offset = 0;
                            }
                        }
                        (_, KeyCode::Char('?')) => {
                            state.tab = if state.diagnostics.traceroute_enabled { 4 } else { 3 }; // help
                        }
                        (_, KeyCode::Enter) => {
                            // Quick-open detail view for the selected history row.
                            if state.tab == 1 {
                                open_history_detail(&mut state);
                            }
                        }
                        (_, KeyCode::Char(' ')) => {
                            if state.tab == 1
                                && !state.history.runs.is_empty()
                                && !state.history.filter_editing
                                && !state.history.detail_view
                                && !state.history.export_modal_open
                                && !state.history.comment_modal_open
                            {
                                state.history.menu_open = true;
                                state.history.menu_selected = history::MENU_ITEM_VIEW;
                            }
                        }
                        // History navigation and deletion (only when on History tab)
                        (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                            if state.tab == 1 && !state.history.runs.is_empty() {
                                if state.history.selected > 0 {
                                    state.history.selected -= 1;
                                }
                            } else if state.tab == 0 {
                                // Dashboard: scroll Test Activity log back one line.
                                let max_scroll = state.text_log.len().saturating_sub(1);
                                state.dashboard_log_scroll =
                                    (state.dashboard_log_scroll + 1).min(max_scroll);
                            }
                        }
                        (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                            if state.tab == 0 {
                                // Dashboard: scroll Test Activity log forward one line.
                                state.dashboard_log_scroll =
                                    state.dashboard_log_scroll.saturating_sub(1);
                            } else if state.tab == 1 && !state.history.runs.is_empty() {
                                if state.history.selected < state.history.runs.len().saturating_sub(1) {
                                    state.history.selected += 1;

                                    // Lazy load: if near end of loaded items, load more
                                    lazy_load_more_history(&mut state);
                                }
                            }
                        }
                        (_, KeyCode::PageUp) => {
                            if state.tab == 1 && !state.history.runs.is_empty() {
                                let page_size = 20;
                                state.history.selected = state.history.selected.saturating_sub(page_size);
                            } else if state.tab == 0 {
                                let max_scroll = state.text_log.len().saturating_sub(1);
                                state.dashboard_log_scroll =
                                    (state.dashboard_log_scroll + 10).min(max_scroll);
                            }
                        }
                        (_, KeyCode::PageDown) => {
                            if state.tab == 0 {
                                state.dashboard_log_scroll =
                                    state.dashboard_log_scroll.saturating_sub(10);
                            } else if state.tab == 1 && !state.history.runs.is_empty() {
                                let page_size = 20;
                                let max_idx = state.history.runs.len().saturating_sub(1);
                                state.history.selected = (state.history.selected + page_size).min(max_idx);

                                // Lazy load if near the end
                                lazy_load_more_history(&mut state);
                            }
                        }
                        // Filter controls (only on History tab)
                        (_, KeyCode::Char('/')) => {
                            if state.tab == 1 {
                                state.history.filter_editing = true;
                            }
                        }
                        (_, KeyCode::Esc) => {
                            if state.tab == 1 && !state.history.filter.is_empty() {
                                // Clear filter when Escape pressed and filter is active
                                state.history.filter.clear();
                                state.history.selected = 0;
                                state.history.scroll_offset = 0;
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
                                Ok(Ok(mut r)) => {
                                    r.connection_quality =
                                        crate::quality::compute(&r, &state.throughput.dl_points, &state.throughput.ul_points);
                                    if state.auto_save {
                                        save_and_show_path(&r, &mut state);
                                    }
                                    if let Some(meta) = r.meta.as_ref() {
                                        let extracted = crate::network::extract_metadata(meta);
                                        state.network.ip = extracted.ip;
                                        state.network.colo = extracted.colo;
                                        state.network.asn = extracted.asn;
                                        state.network.as_org = extracted.as_org;
                                    }
                                    // Server should be set from RunResult.server
                                    if r.server.is_some() {
                                        state.network.server = r.server.clone();
                                    }
                                    // Enrich result with network info before storing
                                    let enriched = enrich_result_with_network_info(&r, &state);
                                    state.last_result = Some(enriched.clone());

                                    // Emit the same end-of-run summary lines text mode prints into the
                                    // dashboard's Test Activity panel so users see the final numbers.
                                    for line in crate::event_format::format_result_summary(
                                        &enriched,
                                        &state.throughput.dl_points,
                                        &state.throughput.ul_points,
                                        &state.latency.idle_latency_samples,
                                        &state.latency.loaded_dl_latency_samples,
                                        &state.latency.loaded_ul_latency_samples,
                                    ) {
                                        UiState::push_log_line(&mut state.text_log, line);
                                    }

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
                                    state.prepend_history_run(enriched.clone());
                                    if state.tab == 1 {
                                        state.history.selected = 0;
                                        state.history.scroll_offset = 0;
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

    // _restore_guard's Drop restores the terminal here and on every early
    // return above.
    res
}

fn lazy_load_more_history(state: &mut UiState) {
    let load_threshold = state.history.loaded_count.saturating_sub(10);
    if state.history.selected < load_threshold || state.history.loaded_count != state.history.runs.len() {
        return;
    }
    let current_count = state.history.runs.len();
    let load_more = current_count.max(20);
    if let Ok(more_history) = crate::storage::load_recent(load_more) {
        let existing_ids: std::collections::HashSet<_> =
            state.history.runs.iter().map(|r| &r.meas_id).collect();
        let new_items: Vec<_> = more_history
            .into_iter()
            .filter(|r| !existing_ids.contains(&r.meas_id))
            .collect();
        if !new_items.is_empty() {
            state.history.runs.extend(new_items);
            state.history.loaded_count = state.history.runs.len();
            update_available_networks(state);
        }
    }
}

struct RunCtx {
    ctrl_tx: mpsc::Sender<EngineControl>,
    event_rx: mpsc::Receiver<TestEvent>,
    handle: Option<tokio::task::JoinHandle<Result<RunResult>>>,
}

async fn start_run(args: &Cli) -> Result<RunCtx> {
    let cfg = build_config(args)?;
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
    // Append the same lines text mode would print to the activity log.
    let added = crate::event_format::format_event_lines(&ev);
    if !added.is_empty() {
        let added_count = added.len();
        for line in added {
            UiState::push_log_line(&mut state.text_log, line);
        }
        // If the user has scrolled away from the bottom, keep their view
        // anchored on the same lines by pushing the offset out by the number
        // of new lines. Scroll == 0 stays pinned to the newest (auto-follow).
        if state.dashboard_log_scroll > 0 {
            let max_scroll = state.text_log.len().saturating_sub(1);
            state.dashboard_log_scroll = state
                .dashboard_log_scroll
                .saturating_add(added_count)
                .min(max_scroll);
        }
    }

    match ev {
        TestEvent::PhaseStarted { phase } => {
            state.phase = phase;
            state.info = format!("Phase: {phase:?}");
            match phase {
                Phase::IdleLatency => {
                    // Reset idle latency tracking
                    state.latency.idle_latency_samples.clear();
                    state.latency.idle_latency_sent = 0;
                    state.latency.idle_latency_received = 0;
                }
                Phase::Download => {
                    state.throughput.dl_phase_start = Some(Instant::now());
                    state.throughput.dl_bytes_total = 0;
                    state.throughput.dl_avg_mbps = 0.0;
                    // Reset loaded DL latency tracking
                    state.latency.loaded_dl_latency_samples.clear();
                    state.latency.loaded_dl_latency_sent = 0;
                    state.latency.loaded_dl_latency_received = 0;
                }
                Phase::Upload => {
                    state.throughput.ul_phase_start = Some(Instant::now());
                    state.throughput.ul_bytes_total = 0;
                    state.throughput.ul_avg_mbps = 0.0;
                    // Reset loaded UL latency tracking
                    state.latency.loaded_ul_latency_samples.clear();
                    state.latency.loaded_ul_latency_sent = 0;
                    state.latency.loaded_ul_latency_received = 0;
                }
                Phase::PacketLoss => {
                    state.latency.udp_loss_sent = 0;
                    state.latency.udp_loss_received = 0;
                    state.latency.udp_loss_total = 0;
                    state.latency.udp_loss_latest_rtt_ms = None;
                }
                _ => {}
            }
        }
        TestEvent::Info { message } => state.info = message,
        TestEvent::MetaInfo { meta } => {
            // Extract IP, colo, ASN, and org from meta
            let extracted = crate::network::extract_metadata(&meta);
            state.network.ip = extracted.ip;
            state.network.colo = extracted.colo;
            state.network.asn = extracted.asn;
            state.network.as_org = extracted.as_org;

            // Extract city for server location (if available, use it directly)
            if let Some(city) = meta.get("city").and_then(|v| v.as_str()) {
                // If we have city, use it for server location
                if let Some(country) = meta.get("country").and_then(|v| v.as_str()) {
                    state.network.server = Some(format!("{}, {}", city, country));
                } else {
                    state.network.server = Some(city.to_string());
                }
            } else if let Some(ref colo) = state.network.colo {
                // Use colo code as server if no city available
                state.network.server = Some(colo.clone());
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
                    state.latency.idle_latency_sent += 1;
                    if ok {
                        state.latency.idle_latency_received += 1;
                        if let Some(ms) = rtt_ms {
                            let v = ms.round().clamp(0.0, 5000.0) as u64;
                            UiState::push_series(&mut state.latency.idle_lat_series, v);
                            UiState::push_point(&mut state.latency.idle_lat_points, t, ms);
                            state.latency.idle_latency_samples.push(ms);
                            // Keep reasonable sample size
                            if state.latency.idle_latency_samples.len() > 10000 {
                                state
                                    .latency
                                    .idle_latency_samples
                                    .drain(0..(state.latency.idle_latency_samples.len() - 10000));
                            }
                        }
                    }
                }
                (Phase::Download, Some(Phase::Download)) => {
                    state.latency.loaded_dl_latency_sent += 1;
                    if ok {
                        state.latency.loaded_dl_latency_received += 1;
                        if let Some(ms) = rtt_ms {
                            let v = ms.round().clamp(0.0, 5000.0) as u64;
                            UiState::push_series(&mut state.latency.loaded_dl_lat_series, v);
                            UiState::push_point(&mut state.latency.loaded_dl_lat_points, t, ms);
                            state.latency.loaded_dl_latency_samples.push(ms);
                            if state.latency.loaded_dl_latency_samples.len() > 10000 {
                                state
                                    .latency
                                    .loaded_dl_latency_samples
                                    .drain(0..(state.latency.loaded_dl_latency_samples.len() - 10000));
                            }
                        }
                    }
                }
                (Phase::Upload, Some(Phase::Upload)) => {
                    state.latency.loaded_ul_latency_sent += 1;
                    if ok {
                        state.latency.loaded_ul_latency_received += 1;
                        if let Some(ms) = rtt_ms {
                            let v = ms.round().clamp(0.0, 5000.0) as u64;
                            UiState::push_series(&mut state.latency.loaded_ul_lat_series, v);
                            UiState::push_point(&mut state.latency.loaded_ul_lat_points, t, ms);
                            state.latency.loaded_ul_latency_samples.push(ms);
                            if state.latency.loaded_ul_latency_samples.len() > 10000 {
                                state
                                    .latency
                                    .loaded_ul_latency_samples
                                    .drain(0..(state.latency.loaded_ul_latency_samples.len() - 10000));
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
                    state.throughput.dl_mbps = mbps;
                    state.throughput.dl_bytes_total = bytes_total;
                    if let Some(t0) = state.throughput.dl_phase_start {
                        let secs = t0.elapsed().as_secs_f64().max(1e-9);
                        state.throughput.dl_avg_mbps = ((bytes_total as f64) / secs) * 8.0 / 1_000_000.0;
                    }
                    let v = state.throughput.dl_mbps.round().clamp(0.0, 10_000.0) as u64;
                    UiState::push_series(&mut state.throughput.dl_series, v);
                    UiState::push_point(&mut state.throughput.dl_points, t, state.throughput.dl_mbps.max(0.0));
                }
                Phase::Upload => {
                    state.throughput.ul_mbps = mbps;
                    state.throughput.ul_bytes_total = bytes_total;
                    if let Some(t0) = state.throughput.ul_phase_start {
                        let secs = t0.elapsed().as_secs_f64().max(1e-9);
                        state.throughput.ul_avg_mbps = ((bytes_total as f64) / secs) * 8.0 / 1_000_000.0;
                    }
                    let v = state.throughput.ul_mbps.round().clamp(0.0, 10_000.0) as u64;
                    UiState::push_series(&mut state.throughput.ul_series, v);
                    UiState::push_point(&mut state.throughput.ul_points, t, state.throughput.ul_mbps.max(0.0));
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
            state.latency.udp_loss_sent = sent;
            state.latency.udp_loss_received = received;
            state.latency.udp_loss_total = total;
            state.latency.udp_loss_latest_rtt_ms = rtt_ms;
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
            state.diagnostics.dns_summary = Some(summary);
        }
        TestEvent::DiagnosticTls { summary } => {
            state.info = format!(
                "TLS: {:.2}ms, {}",
                summary.handshake_time_ms,
                summary.protocol_version.as_deref().unwrap_or("-")
            );
            state.diagnostics.tls_summary = Some(summary);
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
            state.diagnostics.ip_comparison = Some(comparison);
        }
        TestEvent::TracerouteHop { hop_number, hop } => {
            let addr = hop.ip_address.as_deref().unwrap_or("*");
            let rtt = hop
                .rtt_ms
                .first()
                .map(|r| format!("{:.1}ms", r))
                .unwrap_or_else(|| "*".to_string());
            state.info = format!("Traceroute hop {}: {} {}", hop_number, addr, rtt);
            state.diagnostics.traceroute_hops.push(hop);
        }
        TestEvent::TracerouteComplete { summary } => {
            state.info = format!(
                "Traceroute: {} hops to {}",
                summary.hops.len(),
                summary.destination
            );
            state.diagnostics.traceroute_summary = Some(summary);
        }
        TestEvent::ExternalIps { ipv4, ipv6 } => {
            state.network.external_ipv4 = ipv4;
            state.network.external_ipv6 = ipv6;
        }
    }
}

fn open_history_detail(state: &mut UiState) {
    if state.history.runs.is_empty() || state.history.selected >= state.history.runs.len() {
        return;
    }
    let r = &state.history.runs[state.history.selected];
    let json = serde_json::to_string_pretty(r)
        .unwrap_or_else(|e| format!("Error serializing JSON: {}", e));
    let lines: Vec<String> = json.lines().map(String::from).collect();
    let mut textarea = ratatui_textarea::TextArea::new(if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    });
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    state.history.detail_textarea = textarea;
    state.history.detail_search.clear();
    state.history.detail_search_editing = false;
    state.history.detail_search_error = None;
    state.history.detail_view = true;
}

fn close_history_detail(state: &mut UiState) {
    state.history.detail_view = false;
    state.history.detail_textarea = ratatui_textarea::TextArea::default();
    state.history.detail_search.clear();
    state.history.detail_search_editing = false;
    state.history.detail_search_error = None;
}

fn apply_detail_search_pattern(state: &mut UiState) {
    let pattern = state.history.detail_search.clone();
    match state.history.detail_textarea.set_search_pattern(&pattern) {
        Ok(_) => {
            state.history.detail_search_error = None;
            if !pattern.is_empty() {
                state.history.detail_textarea.search_forward(false);
            }
        }
        Err(e) => {
            state.history.detail_search_error = Some(e.to_string());
        }
    }
}

fn handle_history_detail_key(state: &mut UiState, k: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;

    if state.history.detail_search_editing {
        match k.code {
            KeyCode::Esc => {
                // Cancel search input: clear pattern and exit editing mode.
                state.history.detail_search.clear();
                state.history.detail_search_editing = false;
                state.history.detail_search_error = None;
                let _ = state.history.detail_textarea.set_search_pattern("");
            }
            KeyCode::Enter => {
                state.history.detail_search_editing = false;
                apply_detail_search_pattern(state);
            }
            KeyCode::Backspace => {
                state.history.detail_search.pop();
                apply_detail_search_pattern(state);
            }
            KeyCode::Char(c) => {
                state.history.detail_search.push(c);
                apply_detail_search_pattern(state);
            }
            _ => {}
        }
        return;
    }

    match k.code {
        KeyCode::Esc => {
            if !state.history.detail_search.is_empty() {
                // Active search: clear it instead of closing the view.
                state.history.detail_search.clear();
                state.history.detail_search_error = None;
                let _ = state.history.detail_textarea.set_search_pattern("");
            } else {
                close_history_detail(state);
            }
        }
        KeyCode::Char('q') => close_history_detail(state),
        KeyCode::Char('/') => {
            state.history.detail_search_editing = true;
            state.history.detail_search_error = None;
        }
        KeyCode::Char('n') => {
            if !state.history.detail_search.is_empty() {
                state.history.detail_textarea.search_forward(false);
            }
        }
        KeyCode::Char('N') => {
            if !state.history.detail_search.is_empty() {
                state.history.detail_textarea.search_back(false);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state
                .history
                .detail_textarea
                .move_cursor(ratatui_textarea::CursorMove::Up);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state
                .history
                .detail_textarea
                .move_cursor(ratatui_textarea::CursorMove::Down);
        }
        KeyCode::PageUp => {
            state
                .history
                .detail_textarea
                .scroll(ratatui_textarea::Scrolling::PageUp);
        }
        KeyCode::PageDown => {
            state
                .history
                .detail_textarea
                .scroll(ratatui_textarea::Scrolling::PageDown);
        }
        KeyCode::Home => {
            state
                .history
                .detail_textarea
                .move_cursor(ratatui_textarea::CursorMove::Top);
        }
        KeyCode::End => {
            state
                .history
                .detail_textarea
                .move_cursor(ratatui_textarea::CursorMove::Bottom);
        }
        _ => {}
    }
}

fn open_export_modal(state: &mut UiState, path: String) {
    state.history.export_modal_open = true;
    state.history.export_modal_path = Some(path);
    state.history.export_modal_copied = false;
}

fn export_history_selected_json(state: &mut UiState) {
    if state.history.runs.is_empty() || state.history.selected >= state.history.runs.len() {
        return;
    }
    let r = &state.history.runs[state.history.selected];
    match export_result_json(r, state) {
        Ok(p) => open_export_modal(state, p.to_string_lossy().to_string()),
        Err(e) => state.info = format!("JSON export failed: {e:#}"),
    }
}

fn export_history_selected_csv(state: &mut UiState) {
    if state.history.runs.is_empty() || state.history.selected >= state.history.runs.len() {
        return;
    }
    let r = &state.history.runs[state.history.selected];
    match export_result_csv(r, state) {
        Ok(p) => open_export_modal(state, p.to_string_lossy().to_string()),
        Err(e) => state.info = format!("CSV export failed: {e:#}"),
    }
}

fn open_comment_modal(state: &mut UiState) {
    if state.history.runs.is_empty() || state.history.selected >= state.history.runs.len() {
        return;
    }
    let existing = state.history.runs[state.history.selected]
        .comments
        .clone()
        .unwrap_or_default();
    let lines = if existing.is_empty() {
        vec![String::new()]
    } else {
        existing.lines().map(String::from).collect()
    };
    let mut textarea = ratatui_textarea::TextArea::new(lines);
    textarea.move_cursor(ratatui_textarea::CursorMove::End);
    // Disable the default underlined "current line" style — the textarea is single-line
    // and the underline looks out of place.
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea.set_placeholder_text("Type a comment\u{2026}");
    state.history.comment_modal_textarea = textarea;
    state.history.comment_modal_open = true;
}

fn handle_history_comment_modal_key(state: &mut UiState, k: crossterm::event::KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            state.history.comment_modal_open = false;
            state.history.comment_modal_textarea = ratatui_textarea::TextArea::default();
        }
        KeyCode::Enter => {
            if state.history.selected < state.history.runs.len() {
                let value = state
                    .history
                    .comment_modal_textarea
                    .lines()
                    .join(" ")
                    .trim()
                    .to_string();
                let new_comment = if value.is_empty() { None } else { Some(value) };
                state.history.runs[state.history.selected].comments = new_comment;
                if let Err(e) = crate::storage::save_run(&state.history.runs[state.history.selected]) {
                    state.info = format!("Save comment failed: {e:#}");
                } else {
                    state.info = "Comment saved".into();
                }
            }
            state.history.comment_modal_open = false;
            state.history.comment_modal_textarea = ratatui_textarea::TextArea::default();
        }
        _ => {
            // Delegate all other keys (arrows, Home/End, Backspace, Delete, characters)
            // to the textarea widget.
            state.history.comment_modal_textarea.input(Event::Key(k));
        }
    }
}

fn handle_history_export_modal_key(state: &mut UiState, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Enter => {
            state.history.export_modal_open = false;
        }
        KeyCode::Char('c') => {
            if let Some(ref path) = state.history.export_modal_path {
                match copy_to_clipboard(path) {
                    Ok(_) => state.history.export_modal_copied = true,
                    Err(e) => state.info = format!("Clipboard copy failed: {e:#}"),
                }
            }
        }
        _ => {}
    }
}

fn delete_history_selected(state: &mut UiState) {
    if state.history.runs.is_empty() || state.history.selected >= state.history.runs.len() {
        return;
    }
    let to_delete = state.history.runs[state.history.selected].clone();
    if let Err(e) = crate::storage::delete_run(&to_delete) {
        state.info = format!("Delete failed: {e:#}");
        return;
    }
    state.history.runs.remove(state.history.selected);
    if state.history.scroll_offset >= state.history.runs.len() && !state.history.runs.is_empty() {
        state.history.scroll_offset = state.history.runs.len().saturating_sub(20).max(0);
    }
    if state.history.selected >= state.history.runs.len() && !state.history.runs.is_empty() {
        state.history.selected = state.history.runs.len() - 1;
    } else if state.history.runs.is_empty() {
        state.history.selected = 0;
        state.history.scroll_offset = 0;
    }
    state.info = "Deleted".into();
}

fn handle_history_menu_key(state: &mut UiState, code: KeyCode) {
    let n = history::MENU_ITEM_COUNT;
    match code {
        KeyCode::Esc | KeyCode::Char(' ') => {
            state.history.menu_open = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.history.menu_selected = (state.history.menu_selected + n - 1) % n;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.history.menu_selected = (state.history.menu_selected + 1) % n;
        }
        KeyCode::Enter => {
            let idx = state.history.menu_selected;
            state.history.menu_open = false;
            match idx {
                history::MENU_ITEM_VIEW => open_history_detail(state),
                history::MENU_ITEM_EDIT_COMMENT => open_comment_modal(state),
                history::MENU_ITEM_EXPORT_JSON => export_history_selected_json(state),
                history::MENU_ITEM_EXPORT_CSV => export_history_selected_csv(state),
                history::MENU_ITEM_DELETE => delete_history_selected(state),
                _ => {}
            }
        }
        _ => {}
    }
}

fn draw(area: Rect, f: &mut ratatui::Frame, state: &mut UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
        .split(area);

    let mut tab_titles: Vec<Line> = vec![
        Line::from("Dashboard"),
        Line::from("History"),
    ];
    if state.diagnostics.traceroute_enabled {
        tab_titles.push(Line::from("Traceroute"));
    }
    tab_titles.push(Line::from("Charts"));
    tab_titles.push(Line::from("Help"));

    let tabs = Tabs::new(tab_titles)
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

    let traceroute_idx: Option<usize> = if state.diagnostics.traceroute_enabled { Some(2) } else { None };
    let charts_idx: usize = if state.diagnostics.traceroute_enabled { 3 } else { 2 };

    match state.tab {
        0 => draw_dashboard(chunks[1], f, state),
        1 => {
            if state.history.detail_view {
                draw_history_detail(chunks[1], f, &mut *state)
            } else {
                show_history(chunks[1], f, &mut *state);
                if state.history.menu_open {
                    draw_history_menu(chunks[1], f, &*state);
                }
                if state.history.export_modal_open {
                    draw_history_export_modal(chunks[1], f, &*state);
                }
                if state.history.comment_modal_open {
                    draw_history_comment_modal(chunks[1], f, &*state);
                }
            }
        }
        i if Some(i) == traceroute_idx => traceroute::draw_traceroute(chunks[1], f, state),
        i if i == charts_idx => draw_charts(chunks[1], f, state),
        _ => draw_help(chunks[1], f),
    }
}
