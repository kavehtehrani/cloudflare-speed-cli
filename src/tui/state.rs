use crate::model::{DnsSummary, IpVersionComparison, Phase, RunResult, TlsSummary, TracerouteHop, TracerouteSummary};
use ratatui::{
    style::Color,
    style::Style,
    text::{Line, Span},
};
use std::time::Instant;
use ratatui_textarea::TextArea;

#[derive(Default)]
pub struct ThroughputUi {
    pub dl_series: Vec<u64>,
    pub ul_series: Vec<u64>,
    pub dl_points: Vec<(f64, f64)>,
    pub ul_points: Vec<(f64, f64)>,
    pub dl_mbps: f64,
    pub ul_mbps: f64,
    pub dl_avg_mbps: f64,
    pub ul_avg_mbps: f64,
    pub dl_bytes_total: u64,
    pub ul_bytes_total: u64,
    pub dl_phase_start: Option<Instant>,
    pub ul_phase_start: Option<Instant>,
}

#[derive(Default)]
pub struct LatencyUi {
    pub idle_lat_series: Vec<u64>,
    pub loaded_dl_lat_series: Vec<u64>,
    pub loaded_ul_lat_series: Vec<u64>,
    pub idle_lat_points: Vec<(f64, f64)>,
    pub loaded_dl_lat_points: Vec<(f64, f64)>,
    pub loaded_ul_lat_points: Vec<(f64, f64)>,
    pub idle_latency_samples: Vec<f64>,
    pub loaded_dl_latency_samples: Vec<f64>,
    pub loaded_ul_latency_samples: Vec<f64>,
    pub idle_latency_sent: u64,
    pub idle_latency_received: u64,
    pub loaded_dl_latency_sent: u64,
    pub loaded_dl_latency_received: u64,
    pub loaded_ul_latency_sent: u64,
    pub loaded_ul_latency_received: u64,
    pub udp_loss_sent: u64,
    pub udp_loss_received: u64,
    pub udp_loss_total: u64,
    pub udp_loss_latest_rtt_ms: Option<f64>,
}

pub struct HistoryUi {
    pub runs: Vec<RunResult>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub loaded_count: usize,
    pub initial_load_size: usize,
    pub filter: String,
    pub filter_editing: bool,
    pub detail_view: bool,
    pub detail_textarea: TextArea<'static>,
    pub detail_search: String,
    pub detail_search_editing: bool,
    pub detail_search_error: Option<String>,
    pub menu_open: bool,
    pub menu_selected: usize,
    pub export_modal_open: bool,
    pub export_modal_path: Option<String>,
    pub export_modal_copied: bool,
    pub comment_modal_open: bool,
    pub comment_modal_textarea: TextArea<'static>,
}

impl Default for HistoryUi {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            loaded_count: 0,
            initial_load_size: 66,
            filter: String::new(),
            filter_editing: false,
            detail_view: false,
            detail_textarea: TextArea::default(),
            detail_search: String::new(),
            detail_search_editing: false,
            detail_search_error: None,
            menu_open: false,
            menu_selected: 0,
            export_modal_open: false,
            export_modal_path: None,
            export_modal_copied: false,
            comment_modal_open: false,
            comment_modal_textarea: TextArea::default(),
        }
    }
}

#[derive(Default)]
pub struct NetworkUi {
    pub ip: Option<String>,
    pub colo: Option<String>,
    pub server: Option<String>,
    pub asn: Option<String>,
    pub as_org: Option<String>,
    pub interface_name: Option<String>,
    pub network_name: Option<String>,
    pub is_wireless: Option<bool>,
    pub interface_mac: Option<String>,
    pub local_ipv4: Option<String>,
    pub local_ipv6: Option<String>,
    pub external_ipv4: Option<String>,
    pub external_ipv6: Option<String>,
    pub certificate_filename: Option<String>,
    pub proxy_url: Option<String>,
}

#[derive(Default)]
pub struct DiagnosticsUi {
    pub dns_summary: Option<DnsSummary>,
    pub tls_summary: Option<TlsSummary>,
    pub ip_comparison: Option<IpVersionComparison>,
    pub traceroute_summary: Option<TracerouteSummary>,
    pub traceroute_enabled: bool,
    pub traceroute_max_hops: u8,
    pub traceroute_hops: Vec<TracerouteHop>,
}

pub struct UiState {
    pub tab: usize,
    pub paused: bool,
    pub phase: Phase,
    pub info: String,
    pub comments: Option<String>,
    pub auto_save: bool,
    pub hide_network_info: bool,
    pub run_start: Instant,
    pub last_result: Option<RunResult>,
    pub update_status: Option<Option<String>>,
    pub text_log: Vec<String>,
    pub dashboard_log_scroll: usize,
    pub charts_network_filter: Option<String>,
    pub charts_available_networks: Vec<String>,
    pub throughput: ThroughputUi,
    pub latency: LatencyUi,
    pub history: HistoryUi,
    pub network: NetworkUi,
    pub diagnostics: DiagnosticsUi,
}

/// Display string used in place of identifying network info when redaction is on.
pub const REDACTED_PLACEHOLDER: &str = "[redacted]";

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: 0,
            paused: false,
            phase: Phase::IdleLatency,
            info: String::new(),
            comments: None,
            auto_save: true,
            hide_network_info: false,
            run_start: Instant::now(),
            last_result: None,
            update_status: None,
            text_log: Vec::new(),
            dashboard_log_scroll: 0,
            charts_network_filter: None,
            charts_available_networks: Vec::new(),
            throughput: ThroughputUi::default(),
            latency: LatencyUi::default(),
            history: HistoryUi::default(),
            network: NetworkUi::default(),
            diagnostics: DiagnosticsUi {
                traceroute_max_hops: 30,
                ..Default::default()
            },
        }
    }
}

/// Update the list of available networks from history for the Charts tab
pub fn update_available_networks(state: &mut UiState) {
    let mut networks: Vec<String> = state
        .history
        .runs
        .iter()
        .filter_map(|r| r.network_name.clone())
        .collect();
    networks.sort();
    networks.dedup();
    state.charts_available_networks = networks;

    // Reset filter if current selection is no longer valid
    if let Some(ref current) = state.charts_network_filter {
        if !state.charts_available_networks.contains(current) {
            state.charts_network_filter = None;
        }
    }
}

pub fn push_wrapped_status_kv(
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
    pub fn push_series(series: &mut Vec<u64>, v: u64) {
        const MAX: usize = 120;
        series.push(v);
        if series.len() > MAX {
            let _ = series.drain(0..(series.len() - MAX));
        }
    }

    pub fn push_point(points: &mut Vec<(f64, f64)>, x: f64, y: f64) {
        const MAX: usize = 1200; // ~2 min at 10Hz
        points.push((x, y));
        if points.len() > MAX {
            let _ = points.drain(0..(points.len() - MAX));
        }
    }

    pub fn push_log_line(log: &mut Vec<String>, line: String) {
        const MAX: usize = 500;
        log.push(line);
        if log.len() > MAX {
            let _ = log.drain(0..(log.len() - MAX));
        }
    }

    /// Clear all per-run live state when restarting a test from the TUI.
    pub fn reset_for_new_run(&mut self) {
        self.phase = Phase::IdleLatency;
        self.paused = false;
        self.info = "Restarting…".into();
        self.last_result = None;
        self.run_start = std::time::Instant::now();

        self.throughput = ThroughputUi::default();
        self.latency = LatencyUi::default();

        let traceroute_enabled = self.diagnostics.traceroute_enabled;
        let traceroute_max_hops = self.diagnostics.traceroute_max_hops;
        self.diagnostics = DiagnosticsUi {
            traceroute_enabled,
            traceroute_max_hops,
            ..Default::default()
        };

        self.text_log.clear();
        self.dashboard_log_scroll = 0;
    }

    /// Prepend a completed run to in-memory history (newest first).
    pub fn prepend_history_run(&mut self, run: RunResult) {
        if self
            .history
            .runs
            .first()
            .map(|r| r.meas_id == run.meas_id)
            .unwrap_or(false)
        {
            return;
        }
        self.history.runs.insert(0, run);
        self.history.loaded_count = self.history.runs.len();
        update_available_networks(self);
    }

    pub fn compute_live_latency_stats(
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
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = sorted.len();

        let min_ms = Some(sorted[0]);
        let max_ms = Some(sorted[n - 1]);

        // Compute metrics using the same method as metrics.rs
        if let Some(m) = crate::metrics::compute_sample_metrics(samples) {
            let jitter_ms = crate::metrics::compute_jitter(samples);

            crate::model::LatencySummary {
                sent,
                received,
                loss,
                min_ms,
                mean_ms: Some(m.mean),
                median_ms: Some(m.median),
                p25_ms: Some(m.p25),
                p75_ms: Some(m.p75),
                p95_ms: Some(m.p95),
                p99_ms: Some(m.p99),
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
