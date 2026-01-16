mod charts;

use crate::cli::Cli;
use crate::model::{Phase, RunResult, TestEvent};
use crate::network::NetworkInfo;
use crate::orchestrator::{self, UiCommand};
use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
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
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

struct UiState {
    tab: usize,
    paused: bool,
    phase: Phase,
    info: String,
    comments: Option<String>,

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
    network_info: NetworkInfo,
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
            comments: None,
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
            network_info: NetworkInfo::default(),
            interface_name: None,
            network_name: None,
            is_wireless: None,
            interface_mac: None,
            link_speed_mbps: None,
            certificate_filename: None,
        }
    }
}

fn push_wrapped_status_kv(
    out: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    status_area_width: u16,
) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }

    // Account for borders (2 chars on each side)
    let usable_width = status_area_width.saturating_sub(4).max(1);
    let label_text = format!("{label}:");
    let label_width = label_text.chars().count() as u16;

    let value_chars: Vec<char> = value.chars().collect();
    let mut remaining = value_chars.as_slice();
    let mut first = true;

    while !remaining.is_empty() {
        let line_width = if first {
            usable_width.saturating_sub(label_width + 1).max(1)
        } else {
            usable_width.saturating_sub(2).max(1)
        };

        let chars_to_take = (remaining.len() as u16).min(line_width) as usize;
        let (line_chars, rest) = remaining.split_at(chars_to_take);
        let line_text: String = line_chars.iter().collect();

        if first {
            out.push(Line::from(vec![
                Span::styled(label_text.clone(), Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::raw(line_text),
            ]));
            first = false;
        } else {
            out.push(Line::from(vec![Span::raw("  "), Span::raw(line_text)]));
        }

        remaining = rest;
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
                ..Default::default()
            };
        }

        // Use the same calculation method as metrics.rs for consistency
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = sorted.len();

        let min_ms = Some(sorted[0]);
        let max_ms = Some(sorted[n - 1]);

        // Compute metrics using the same method as metrics.rs
        if let Some((mean, median, p25, p75)) = crate::metrics::compute_metrics(samples) {
            // Use the shared jitter computation from metrics.rs
            let jitter_ms = crate::metrics::compute_jitter(samples);

            crate::model::LatencySummary {
                sent,
                received,
                loss,
                min_ms,
                mean_ms: Some(mean),
                median_ms: Some(median),
                p25_ms: Some(p25),
                p75_ms: Some(p75),
                max_ms,
                jitter_ms,
            }
        } else {
            crate::model::LatencySummary {
                sent,
                received,
                loss,
                ..Default::default()
            }
        }
    }
}

pub async fn run(args: Cli) -> Result<()> {
    // Unbounded channels avoid backpressure and task switching in the hot path.
    let (event_tx, event_rx) = mpsc::unbounded_channel::<TestEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCommand>();

    // Gather network info once outside the UI thread; UI only consumes the results.
    let network_info = crate::network::gather_network_info(&args);

    // TUI runs in a dedicated thread to keep all blocking I/O out of the Tokio runtime.
    let ui_args = args.clone();
    let ui_handle =
        std::thread::spawn(move || run_threaded(ui_args, network_info, event_rx, cmd_tx));

    let res = orchestrator::run_controller(&args, event_tx, cmd_rx).await;

    let join_res = tokio::task::spawn_blocking(move || ui_handle.join()).await;
    if let Ok(joined) = join_res {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(anyhow::anyhow!("TUI thread panicked")),
        }
    }

    res
}

/// Run the TUI loop on a dedicated thread.
pub fn run_threaded(
    args: Cli,
    network_info: NetworkInfo,
    mut event_rx: UnboundedReceiver<TestEvent>,
    cmd_tx: UnboundedSender<UiCommand>,
) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).ok();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear().ok();

    let initial_load = terminal
        .size()
        .map(|size| ((size.height as usize).saturating_sub(2) * 3).max(20))
        .unwrap_or(66);

    let mut state = UiState {
        phase: Phase::IdleLatency,
        auto_save: args.auto_save,
        comments: args.comments.clone(),
        ..Default::default()
    };
    // UiState is owned by the UI thread only; no cross-thread mutation.
    state.initial_history_load_size = initial_load;
    state.history = crate::storage::load_recent(initial_load).unwrap_or_default();
    state.history_loaded_count = state.history.len();

    state.network_info = network_info.clone();
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

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    let res = loop {
        // Drain events without blocking to keep UI responsive; unbounded channel avoids backpressure.
        while let Ok(ev) = event_rx.try_recv() {
            match ev {
                TestEvent::RunCompleted { result } => {
                    handle_run_completed(&args, &mut state, *result);
                }
                other => apply_event(&mut state, other),
            }
        }

        if last_tick.elapsed() >= tick_rate {
            terminal.draw(|f| draw(f.area(), f, &state)).ok();
            last_tick = Instant::now();
        }

        // Poll input with a short timeout to avoid blocking the render loop.
        if event::poll(Duration::from_millis(10)).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match (k.modifiers, k.code) {
                    (_, KeyCode::Char('q')) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                        let _ = cmd_tx.send(UiCommand::Quit);
                        break Ok(());
                    }
                    (_, KeyCode::Char('p')) => {
                        state.paused = !state.paused;
                        let _ = cmd_tx.send(UiCommand::Pause(state.paused));
                    }
                    (_, KeyCode::Char('r')) => {
                        if state.tab == 1 {
                            let reload_size = state
                                .initial_history_load_size
                                .max(state.history_loaded_count);
                            match crate::storage::load_recent(reload_size) {
                                Ok(new_history) => {
                                    let old_count = state.history.len();
                                    state.history = new_history;
                                    state.history_loaded_count = state.history.len();

                                    if state.history_selected >= state.history.len()
                                        && !state.history.is_empty()
                                    {
                                        state.history_selected = state.history.len() - 1;
                                    } else if state.history.is_empty() {
                                        state.history_selected = 0;
                                        state.history_scroll_offset = 0;
                                    }

                                    if state.history_scroll_offset >= state.history.len()
                                        && !state.history.is_empty()
                                    {
                                        state.history_scroll_offset =
                                            state.history.len().saturating_sub(20).max(0);
                                    }

                                    let new_count = state.history.len();
                                    if new_count > old_count {
                                        state.info = format!(
                                            "Refreshed: {} new run(s)",
                                            new_count - old_count
                                        );
                                    } else if new_count < old_count {
                                        state.info = format!(
                                            "Refreshed: {} run(s) removed",
                                            old_count - new_count
                                        );
                                    } else {
                                        state.info = "Refreshed".into();
                                    }
                                }
                                Err(e) => {
                                    state.info = format!("Refresh failed: {e:#}");
                                }
                            }
                        } else {
                            state.info = "Restart requested…".into();
                            let _ = cmd_tx.send(UiCommand::Restart);
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
                        }
                    }
                    (_, KeyCode::Char('s')) => {
                        if state.tab == 0 {
                            if let Some(r) = state.last_result.clone() {
                                save_and_show_path(&r, &mut state);
                            } else {
                                state.info = "No completed run to save yet.".into();
                            }
                        }
                    }
                    (_, KeyCode::Char('e')) => {
                        if state.tab == 1
                            && !state.history.is_empty()
                            && state.history_selected < state.history.len()
                        {
                            let r = &state.history[state.history_selected];
                            match export_result_json(r, &state) {
                                Ok(p) => {
                                    let path_str = p.to_string_lossy().to_string();
                                    state.last_exported_path = Some(path_str.clone());
                                    state.info = format!(
                                        "Exported JSON: {} (press 'y' to copy path)",
                                        p.display()
                                    );
                                }
                                Err(e) => {
                                    state.info = format!("JSON export failed: {e:#}");
                                }
                            }
                        }
                    }
                    (_, KeyCode::Char('c')) => {
                        if state.tab == 1
                            && !state.history.is_empty()
                            && state.history_selected < state.history.len()
                        {
                            let r = &state.history[state.history_selected];
                            match export_result_csv(r, &state) {
                                Ok(p) => {
                                    let path_str = p.to_string_lossy().to_string();
                                    state.last_exported_path = Some(path_str.clone());
                                    state.info = format!(
                                        "Exported CSV: {} (press 'y' to copy path)",
                                        p.display()
                                    );
                                }
                                Err(e) => {
                                    state.info = format!("CSV export failed: {e:#}");
                                }
                            }
                        }
                    }
                    (_, KeyCode::Char('y')) => {
                        if state.tab == 1 {
                            if let Some(ref path) = state.last_exported_path {
                                match copy_to_clipboard(path) {
                                    Ok(_) => {
                                        let display_path = if path.len() > 60 {
                                            format!("{}...", &path[..57])
                                        } else {
                                            path.clone()
                                        };
                                        state.info =
                                            format!("✓ Copied to clipboard: {}", display_path);
                                    }
                                    Err(e) => {
                                        state.info = format!("Clipboard copy failed: {e:#}");
                                    }
                                }
                            } else {
                                state.info =
                                    "No exported file path to copy. Export a file first (e/c)"
                                        .into();
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
                        if new_tab == 1 {
                            state.history_selected = 0;
                            state.history_scroll_offset = 0;
                        }
                    }
                    (_, KeyCode::Char('?')) => {
                        state.tab = 2;
                    }
                    (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                        if state.tab == 1 && !state.history.is_empty() && state.history_selected > 0
                        {
                            state.history_selected -= 1;
                            if state.history_selected < state.history_scroll_offset {
                                state.history_scroll_offset = state.history_selected;
                            }
                        }
                    }
                    (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                        if state.tab == 1
                            && !state.history.is_empty()
                            && state.history_selected < state.history.len().saturating_sub(1)
                        {
                            state.history_selected += 1;
                            let estimated_max_items = 30;
                            if state.history_selected
                                >= state.history_scroll_offset + estimated_max_items
                            {
                                state.history_scroll_offset = state
                                    .history_selected
                                    .saturating_sub(estimated_max_items - 1);
                            }

                            let load_threshold = state.history_loaded_count.saturating_sub(10);
                            if state.history_selected >= load_threshold
                                && state.history_loaded_count == state.history.len()
                            {
                                let current_count = state.history.len();
                                let load_more = current_count.max(20);
                                if let Ok(more_history) = crate::storage::load_recent(load_more) {
                                    let existing_ids: std::collections::HashSet<_> =
                                        state.history.iter().map(|r| &r.meas_id).collect();
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
                    (_, KeyCode::Char('d')) => {
                        if state.tab == 1
                            && !state.history.is_empty()
                            && state.history_selected < state.history.len()
                        {
                            let to_delete = state.history[state.history_selected].clone();
                            if let Err(e) = crate::storage::delete_run(&to_delete) {
                                state.info = format!("Delete failed: {e:#}");
                            } else {
                                state.history.remove(state.history_selected);
                                if state.history_scroll_offset >= state.history.len()
                                    && !state.history.is_empty()
                                {
                                    state.history_scroll_offset =
                                        state.history.len().saturating_sub(20).max(0);
                                }
                                if state.history_selected >= state.history.len()
                                    && !state.history.is_empty()
                                {
                                    state.history_selected = state.history.len() - 1;
                                } else if state.history.is_empty() {
                                    state.history_selected = 0;
                                    state.history_scroll_offset = 0;
                                }
                                state.info = "Deleted".into();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode().ok();
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen).ok();
    res
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
        TestEvent::Info(info) => state.info = info.to_message(),
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
        TestEvent::RunCompleted { .. } => {}
    }
}

fn handle_run_completed(args: &Cli, state: &mut UiState, r: RunResult) {
    let reload_size = (state.history_loaded_count + 1).max(state.initial_history_load_size);
    let processed = orchestrator::process_run_completion(
        args,
        &state.network_info,
        reload_size,
        state.auto_save,
        &r,
    );

    state.last_result = Some(processed.enriched.clone());
    state.ip = processed.enriched.ip.clone();
    state.colo = processed.enriched.colo.clone();
    state.asn = processed.enriched.asn.clone();
    state.as_org = processed.enriched.as_org.clone();
    state.server = processed.enriched.server.clone();

    if let Some(path) = processed.auto_saved_path.as_ref() {
        if path.exists() {
            state.info = format!("Saved: {}", path.display());
        } else {
            state.info = format!("Saved (verifying): {}", path.display());
        }
    }
    if !processed.export_messages.is_empty() {
        state.info = processed.export_messages.join("; ");
    }

    state.history = processed.history;
    state.history_loaded_count = state.history.len();
    if state.tab == 1 {
        state.history_selected = 0;
        state.history_scroll_offset = 0;
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

        let dl_values: Vec<f64> = state.dl_points.iter().map(|(_, y)| *y).collect();
        let dl_metrics = crate::metrics::compute_metrics(&dl_values);
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
            charts::ChartRenderParams {
                area: thr_row[0],
                datasets: vec![dl_ds],
                x_axis: Axis::default().bounds([dl_x_min, dl_x_max.max(1.0)]),
                y_axis: Axis::default().title("Mbps").bounds([0.0, y_dl_max]),
                title: dl_title,
                metrics: dl_metrics,
                color: Color::Green,
            },
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

        let ul_values: Vec<f64> = state.ul_points.iter().map(|(_, y)| *y).collect();
        let ul_metrics = crate::metrics::compute_metrics(&ul_values);
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
            charts::ChartRenderParams {
                area: thr_row[1],
                datasets: vec![ul_ds],
                x_axis: Axis::default().bounds([ul_x_min, ul_x_max.max(1.0)]),
                y_axis: Axis::default().title("Mbps").bounds([0.0, y_ul_max]),
                title: ul_title,
                metrics: ul_metrics,
                color: Color::Cyan,
            },
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
        let median = crate::metrics::compute_metrics(&state.idle_latency_samples)
            .map(|(_, med, _, _)| med)
            .unwrap_or(f64::NAN);
        let jitter = crate::metrics::compute_jitter(&state.idle_latency_samples);
        let title = Line::from(format!("Idle Latency ({:.0}ms)", median));
        charts::render_box_plot_with_metrics_inside(
            f,
            lat_row[0],
            &state.idle_latency_samples,
            title,
            None,
            jitter,
        );
    } else {
        let empty = Paragraph::new("Waiting for data...")
            .block(Block::default().borders(Borders::ALL).title("Idle Latency"));
        f.render_widget(empty, lat_row[0]);
    }

    // Download latency
    if state.loaded_dl_latency_samples.len() >= 2 {
        // Use the same median calculation as the metrics below
        let median = crate::metrics::compute_metrics(&state.loaded_dl_latency_samples)
            .map(|(_, med, _, _)| med)
            .unwrap_or(f64::NAN);
        let jitter = crate::metrics::compute_jitter(&state.loaded_dl_latency_samples);
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
            jitter,
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
        let median = crate::metrics::compute_metrics(&state.loaded_ul_latency_samples)
            .map(|(_, med, _, _)| med)
            .unwrap_or(f64::NAN);
        let jitter = crate::metrics::compute_jitter(&state.loaded_ul_latency_samples);
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
            jitter,
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
                    .or(state.interface_name.as_deref())
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
    let mut status_lines = vec![Line::from(vec![
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
    ])];

    // Custom comments (wrapping to fit status area)
    if let Some(comments) = state.comments.as_deref() {
        push_wrapped_status_kv(&mut status_lines, "Comments", comments, main[3].width);
    }

    // Info line - split into two lines if it contains a saved path, with wrapping
    if state.info.starts_with("Saved:") || state.info.starts_with("Saved (verifying):") {
        // Split into label and path
        if let Some(colon_pos) = state.info.find(':') {
            let (label, path) = state.info.split_at(colon_pos + 1);
            let label_text = label.trim().to_string();
            let path_str = path.trim();

            // Wrap the path to fit within available width
            // Account for borders (2 chars on each side)
            let status_area_width = main[3].width.saturating_sub(4);
            let label_width = label_text.chars().count() as u16;
            let path_chars: Vec<char> = path_str.chars().collect();
            let mut remaining = path_chars.as_slice();
            let mut is_first_path_line = true;

            while !remaining.is_empty() {
                // Calculate how many chars fit on this line
                let line_width = if is_first_path_line {
                    // First path line - account for label width
                    status_area_width.saturating_sub(label_width).max(1)
                } else {
                    // Subsequent lines - indent by 2 spaces
                    status_area_width.saturating_sub(2).max(1)
                };

                let chars_to_take = (remaining.len() as u16).min(line_width) as usize;
                let (line_chars, rest) = remaining.split_at(chars_to_take);
                let line_text: String = line_chars.iter().collect();

                if is_first_path_line {
                    // First line - include label and first part of path
                    status_lines.push(Line::from(vec![
                        Span::styled(label_text.clone(), Style::default().fg(Color::Gray)),
                        Span::raw(" "),
                        Span::raw(line_text),
                    ]));
                    is_first_path_line = false;
                } else {
                    // Subsequent lines - indent
                    status_lines.push(Line::from(vec![Span::raw("  "), Span::raw(line_text)]));
                }

                remaining = rest;
            }
        } else {
            status_lines.push(Line::from(vec![
                Span::styled("Info: ", Style::default().fg(Color::Gray)),
                Span::raw(state.info.clone()),
            ]));
        }
    } else {
        status_lines.push(Line::from(vec![
            Span::styled("Info: ", Style::default().fg(Color::Gray)),
            Span::raw(state.info.clone()),
        ]));
    }

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
                Span::styled("avg: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.mean_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("med: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.median_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("p25: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.p25_ms.unwrap_or(f64::NAN))),
            ]),
            Line::from(vec![
                Span::styled("p75: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms", lat.p75_ms.unwrap_or(f64::NAN))),
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
                    .or(state.interface_name.as_deref())
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

/// Save result and update state.info with the saved path message.
fn save_and_show_path(r: &RunResult, state: &mut UiState) {
    match save_result_json(r, state) {
        Ok(path) => {
            // Update last_result to the enriched version that was saved
            // This ensures the path computation matches
            let enriched = enrich_result_with_network_info(r, state);
            state.last_result = Some(enriched);
            // Verify file exists before showing path
            if path.exists() {
                state.info = format!("Saved: {}", path.display());
            } else {
                state.info = format!("Saved (verifying): {}", path.display());
            }
        }
        Err(e) => {
            state.info = format!("Save failed: {e:#}");
        }
    }
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

    // Show info message if it's an export message (when on history tab)
    if state.tab == 1
        && (state.info.starts_with("Exported")
            || state.info.starts_with("JSON export")
            || state.info.starts_with("CSV export")
            || state.info.starts_with("Refreshed")
            || state.info == "Deleted")
    {
        // Wrap long export messages similar to dashboard
        if state.info.starts_with("Exported JSON:") || state.info.starts_with("Exported CSV:") {
            // Split into label and path
            if let Some(colon_pos) = state.info.find(':') {
                let (label, path_part) = state.info.split_at(colon_pos + 1);
                let label_trimmed = label.trim();
                let path_str = path_part.trim();

                // Wrap the path to fit within available width
                // Account for borders (2 chars on each side)
                let history_area_width = area.width.saturating_sub(4);
                let label_with_prefix = format!("Info: {}", label_trimmed);
                let label_width = label_with_prefix.chars().count() as u16;
                let path_chars: Vec<char> = path_str.chars().collect();
                let mut remaining = path_chars.as_slice();
                let mut is_first_path_line = true;

                while !remaining.is_empty() {
                    // Calculate how many chars fit on this line
                    let line_width = if is_first_path_line {
                        // First path line - account for label width
                        history_area_width.saturating_sub(label_width).max(1)
                    } else {
                        // Subsequent lines - indent by 2 spaces
                        history_area_width.saturating_sub(2).max(1)
                    };

                    let chars_to_take = (remaining.len() as u16).min(line_width) as usize;
                    let (line_chars, rest) = remaining.split_at(chars_to_take);
                    let line_text: String = line_chars.iter().collect();

                    if is_first_path_line {
                        // First line - include label and first part of path
                        lines.push(Line::from(vec![
                            Span::styled("Info: ", Style::default().fg(Color::Gray)),
                            Span::styled(label_trimmed, Style::default().fg(Color::Gray)),
                            Span::raw(" "),
                            Span::raw(line_text),
                        ]));
                        is_first_path_line = false;
                    } else {
                        // Subsequent lines - indent
                        lines.push(Line::from(vec![Span::raw("  "), Span::raw(line_text)]));
                    }

                    remaining = rest;
                }
            } else {
                // Fallback if no colon found
                lines.push(Line::from(vec![
                    Span::styled("Info: ", Style::default().fg(Color::Gray)),
                    Span::raw(&state.info),
                ]));
            }
        } else {
            // For other messages (errors, refresh, delete), just show normally
            lines.push(Line::from(vec![
                Span::styled("Info: ", Style::default().fg(Color::Gray)),
                Span::raw(&state.info),
            ]));
        }
    }

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
                    "Idle med {:>6.0} ms",
                    r.idle_latency.median_ms.unwrap_or(f64::NAN)
                ),
                if is_selected { style } else { Style::default() },
            ),
            Span::raw("  "),
            Span::styled(
                r.interface_name.as_deref().unwrap_or("-").to_string(),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Blue)
                },
            ),
            Span::raw("  "),
            Span::styled(
                r.network_name
                    .as_deref()
                    .or(r.interface_name.as_deref())
                    .unwrap_or("-")
                    .to_string(),
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
                let line_width_usize = line_width as usize;
                let mut last_sep_pos = None;
                let mut break_pos = 0;

                for (char_count, (idx, ch)) in remaining.char_indices().enumerate() {
                    if char_count >= line_width_usize {
                        break;
                    }
                    if ch == '/' || ch == '\\' {
                        last_sep_pos = Some(idx);
                    }
                    break_pos = idx + ch.len_utf8();
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
