use crate::model::{
    DnsSummary, IpVersionComparison, Phase, RunResult, TlsSummary, TracerouteHop, TracerouteSummary,
};
use ratatui::{
    style::Color,
    style::Style,
    text::{Line, Span},
};
use ratatui_textarea::TextArea;
use std::time::Instant;

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
    /// Set once a lazy load finds nothing new on disk, so scrolling at the
    /// end of the list stops re-scanning the runs directory on every keypress.
    pub all_loaded: bool,
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
            all_loaded: false,
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

impl HistoryUi {
    /// True when `r` matches the active filter text (already lowercased).
    /// Searched fields: network name, interface, AS org, colo, comments.
    pub fn run_matches_filter(r: &RunResult, filter_lower: &str) -> bool {
        let matches_field = |opt: &Option<String>| {
            opt.as_ref()
                .map(|s| s.to_lowercase().contains(filter_lower))
                .unwrap_or(false)
        };
        matches_field(&r.network_name)
            || matches_field(&r.interface_name)
            || matches_field(&r.as_org)
            || matches_field(&r.colo)
            || matches_field(&r.comments)
    }

    /// Indices into `runs` of the rows the History tab currently shows,
    /// in display order. Identity when no filter is active.
    pub fn filtered_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.runs.len()).collect();
        }
        let filter_lower = self.filter.to_lowercase();
        self.runs
            .iter()
            .enumerate()
            .filter(|(_, r)| Self::run_matches_filter(r, &filter_lower))
            .map(|(i, _)| i)
            .collect()
    }

    /// Number of rows the History tab currently shows.
    pub fn visible_len(&self) -> usize {
        self.filtered_indices().len()
    }

    /// Index into `runs` of the highlighted row, honoring the active filter.
    /// `None` when nothing is visible. This is the ONLY correct way for an
    /// action (delete/export/comment/detail) to resolve the selected run.
    pub fn selected_run_index(&self) -> Option<usize> {
        let visible = self.filtered_indices();
        if visible.is_empty() {
            None
        } else {
            // Clamp like the renderer does, so the highlighted row and the
            // acted-on row can never diverge.
            Some(visible[self.selected.min(visible.len() - 1)])
        }
    }

    /// The (offset, chunk) to request from storage on the next lazy load:
    /// only entries beyond those already in memory, one chunk at a time, so
    /// scrolling never re-parses the files already loaded.
    pub fn next_load_range(&self) -> (usize, usize) {
        (
            self.runs.len(),
            self.initial_load_size
                .max(crate::constants::HISTORY_LOAD_CHUNK_MIN),
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::empty_run_result;

    fn run_with_network(name: &str) -> RunResult {
        let mut r = empty_run_result();
        r.network_name = Some(name.into());
        r
    }

    fn history_with_networks(names: &[&str]) -> HistoryUi {
        HistoryUi {
            runs: names.iter().map(|n| run_with_network(n)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn filtered_indices_identity_without_filter() {
        let h = history_with_networks(&["HomeWifi", "Office", "home-5g"]);
        assert_eq!(h.filtered_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn filtered_indices_respect_active_filter() {
        let mut h = history_with_networks(&["HomeWifi", "Office", "home-5g"]);
        h.filter = "Home".into();
        assert_eq!(h.filtered_indices(), vec![0, 2]);
    }

    #[test]
    fn selected_run_index_resolves_through_filter() {
        let mut h = history_with_networks(&["HomeWifi", "Office", "home-5g"]);
        h.filter = "home".into();
        // Highlighting the second visible row must resolve to runs[2],
        // not runs[1] (which the filter hides).
        h.selected = 1;
        assert_eq!(h.selected_run_index(), Some(2));
    }

    #[test]
    fn selected_run_index_clamps_to_last_visible_row() {
        let mut h = history_with_networks(&["HomeWifi", "Office", "home-5g"]);
        h.filter = "home".into();
        h.selected = 99;
        assert_eq!(h.selected_run_index(), Some(2));
    }

    #[test]
    fn selected_run_index_none_when_filter_matches_nothing() {
        let mut h = history_with_networks(&["HomeWifi", "Office"]);
        h.filter = "zzz".into();
        h.selected = 0;
        assert_eq!(h.selected_run_index(), None);
    }

    #[test]
    fn selected_run_index_none_when_empty() {
        let h = HistoryUi::default();
        assert_eq!(h.selected_run_index(), None);
    }

    #[test]
    fn next_load_range_starts_beyond_loaded_entries() {
        let h = HistoryUi {
            runs: (0..66).map(|_| empty_run_result()).collect(),
            ..Default::default()
        };
        let (offset, chunk) = h.next_load_range();
        assert_eq!(
            offset,
            h.runs.len(),
            "lazy load must skip what is already in memory"
        );
        assert!(chunk > 0, "lazy load must actually request new entries");
    }
}
