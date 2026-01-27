use crate::model::{DnsSummary, IpVersionComparison, Phase, RunResult, TlsSummary, TracerouteSummary};
use ratatui::{
    style::Color,
    style::Style,
    text::{Line, Span},
};
use std::time::Instant;

pub struct UiState {
    pub tab: usize,
    pub paused: bool,
    pub phase: Phase,
    pub info: String,
    pub comments: Option<String>,

    pub dl_series: Vec<u64>,
    pub ul_series: Vec<u64>,
    pub idle_lat_series: Vec<u64>,
    pub loaded_dl_lat_series: Vec<u64>,
    pub loaded_ul_lat_series: Vec<u64>,

    // Time-series for charts (seconds since run start, value)
    pub run_start: Instant,
    pub dl_points: Vec<(f64, f64)>,
    pub ul_points: Vec<(f64, f64)>,
    pub idle_lat_points: Vec<(f64, f64)>,
    pub loaded_dl_lat_points: Vec<(f64, f64)>,
    pub loaded_ul_lat_points: Vec<(f64, f64)>,

    pub dl_mbps: f64,
    pub ul_mbps: f64,
    pub dl_avg_mbps: f64,
    pub ul_avg_mbps: f64,
    pub dl_bytes_total: u64,
    pub ul_bytes_total: u64,
    pub dl_phase_start: Option<Instant>,
    pub ul_phase_start: Option<Instant>,

    // Live latency samples for real-time stats
    pub idle_latency_samples: Vec<f64>,
    pub loaded_dl_latency_samples: Vec<f64>,
    pub loaded_ul_latency_samples: Vec<f64>,
    pub idle_latency_sent: u64,
    pub idle_latency_received: u64,
    pub loaded_dl_latency_sent: u64,
    pub loaded_dl_latency_received: u64,
    pub loaded_ul_latency_sent: u64,
    pub loaded_ul_latency_received: u64,

    pub last_result: Option<RunResult>,
    pub history: Vec<RunResult>,
    pub history_selected: usize, // Index of selected history item (0 = most recent)
    pub history_scroll_offset: usize,
    pub history_loaded_count: usize,
    pub initial_history_load_size: usize, // Initial load size based on terminal height
    // History filtering
    pub history_filter: String,       // Current filter text
    pub history_filter_editing: bool, // Whether user is typing in filter input
    // Charts tab state
    pub charts_network_filter: Option<String>, // None = all networks, Some(name) = specific network
    pub charts_available_networks: Vec<String>, // List of unique network names from history
    // History detail view state
    pub history_detail_view: bool,    // Whether showing JSON detail view
    pub history_detail_scroll: usize, // Scroll position in detail view
    pub ip: Option<String>,
    pub colo: Option<String>,
    pub server: Option<String>,
    pub asn: Option<String>,
    pub as_org: Option<String>,
    pub auto_save: bool,
    pub last_exported_path: Option<String>,
    // Network interface information
    pub interface_name: Option<String>,
    pub network_name: Option<String>,
    pub is_wireless: Option<bool>,
    pub interface_mac: Option<String>,
    pub local_ipv4: Option<String>,
    pub local_ipv6: Option<String>,
    pub external_ipv4: Option<String>,
    pub external_ipv6: Option<String>,
    pub certificate_filename: Option<String>,
    // Diagnostic results
    pub dns_summary: Option<DnsSummary>,
    pub tls_summary: Option<TlsSummary>,
    pub ip_comparison: Option<IpVersionComparison>,
    pub traceroute_summary: Option<TracerouteSummary>,
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
            history_filter: String::new(),
            history_filter_editing: false,
            charts_network_filter: None,
            charts_available_networks: Vec::new(),
            history_detail_view: false,
            history_detail_scroll: 0,
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
            local_ipv4: None,
            local_ipv6: None,
            external_ipv4: None,
            external_ipv6: None,
            certificate_filename: None,
            // Diagnostic results
            dns_summary: None,
            tls_summary: None,
            ip_comparison: None,
            traceroute_summary: None,
        }
    }
}

/// Update the list of available networks from history for the Charts tab
pub fn update_available_networks(state: &mut UiState) {
    let mut networks: Vec<String> = state
        .history
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
