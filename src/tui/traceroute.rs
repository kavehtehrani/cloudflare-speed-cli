//! Traceroute tab rendering.

use crate::tui::state::UiState;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

const BAR_WIDTH: usize = 12;
const HOST_WIDTH: usize = 38;

/// Render the Traceroute tab content.
pub fn draw_traceroute(area: Rect, f: &mut Frame, state: &UiState) {
    let destination = state
        .diagnostics
        .traceroute_summary
        .as_ref()
        .map(|s| s.destination.clone())
        .unwrap_or_else(|| "Cloudflare edge".to_string());

    let completed = state
        .diagnostics
        .traceroute_summary
        .as_ref()
        .map(|s| s.completed)
        .unwrap_or(false);
    let status = if completed { "complete" } else { "partial" };

    let received = state.diagnostics.traceroute_hops.len();
    let max = state.diagnostics.traceroute_max_hops as usize;

    let mut lines: Vec<Line> = Vec::with_capacity(received + 3);

    lines.push(Line::from(vec![
        Span::styled("Traceroute to ", Style::default().fg(Color::Gray)),
        Span::raw(destination),
        Span::raw("    "),
        Span::styled(status, Style::default().fg(Color::Gray)),
        Span::raw(format!(" · {}/{}", received, max)),
    ]));
    lines.push(Line::from(""));

    let max_min_rtt = state
        .diagnostics
        .traceroute_hops
        .iter()
        .filter_map(min_rtt)
        .fold(0.0_f64, f64::max);

    for hop in &state.diagnostics.traceroute_hops {
        let idx = format!("{:>2}", hop.hop_number);
        let host_or_ip = format_host_or_ip(hop);
        let host_field = pad_or_truncate(&host_or_ip, HOST_WIDTH);
        let rtt_field = format_rtts(hop);
        let bar = match min_rtt(hop) {
            Some(v) => bar_for_rtt(v, max_min_rtt, BAR_WIDTH),
            None => String::new(),
        };
        lines.push(Line::from(vec![
            Span::raw(format!(" {}  ", idx)),
            Span::raw(host_field),
            Span::raw("  "),
            Span::raw(rtt_field),
            Span::raw("  "),
            Span::styled(bar, Style::default().fg(Color::Cyan)),
        ]));
    }

    let widget =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Traceroute"));
    f.render_widget(widget, area);
}

fn min_rtt(hop: &crate::model::TracerouteHop) -> Option<f64> {
    hop.rtt_ms
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .fold(None, |acc, v| match acc {
            None => Some(v),
            Some(prev) => Some(prev.min(v)),
        })
}

fn format_host_or_ip(hop: &crate::model::TracerouteHop) -> String {
    match (&hop.hostname, &hop.ip_address) {
        (Some(h), Some(i)) if h != i => format!("{} ({})", h, i),
        (_, Some(i)) => i.clone(),
        (Some(h), None) => h.clone(),
        (None, None) => "*".to_string(),
    }
}

fn format_rtts(hop: &crate::model::TracerouteHop) -> String {
    if hop.rtt_ms.is_empty() {
        return format!("{:^23}", "*");
    }
    let mut parts: Vec<String> = hop
        .rtt_ms
        .iter()
        .take(3)
        .map(|v| format!("{:>6.1}ms", v))
        .collect();
    while parts.len() < 3 {
        parts.push(format!("{:>8}", "--"));
    }
    parts.join(" ")
}

fn pad_or_truncate(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count <= width {
        format!("{:<w$}", s, w = width)
    } else if width == 0 {
        String::new()
    } else {
        let truncated: String = s.chars().take(width - 1).collect();
        format!("{}\u{2026}", truncated)
    }
}

/// Render a horizontal bar for an RTT value.
///
/// `value` is this hop's min RTT in ms, `max` is the largest min RTT across
/// all rendered hops, and `width` is the cell budget. Uses Unicode 1/8 block
/// characters for sub-cell granularity. Returns an empty string when `value`
/// is non-positive or `max` is non-positive.
pub(super) fn bar_for_rtt(value: f64, max: f64, width: usize) -> String {
    if value <= 0.0 || max <= 0.0 || width == 0 {
        return String::new();
    }
    let total_steps = (width as f64) * 8.0;
    let filled_steps = ((value / max) * total_steps).round() as usize;
    let filled_steps = filled_steps.min(width * 8);
    let full_cells = filled_steps / 8;
    let remainder = filled_steps % 8;
    let partials = [
        '\u{258F}', '\u{258E}', '\u{258D}', '\u{258C}', '\u{258B}', '\u{258A}', '\u{2589}',
        '\u{2588}',
    ];
    let mut out = String::with_capacity(width * 4);
    for _ in 0..full_cells {
        out.push('\u{2588}');
    }
    if remainder > 0 && full_cells < width {
        out.push(partials[remainder - 1]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_zero_value_is_empty() {
        assert_eq!(bar_for_rtt(0.0, 100.0, 20), "");
    }

    #[test]
    fn bar_zero_max_is_empty() {
        assert_eq!(bar_for_rtt(50.0, 0.0, 20), "");
    }

    #[test]
    fn bar_full_value_fills_width() {
        let bar = bar_for_rtt(100.0, 100.0, 20);
        assert_eq!(bar.chars().count(), 20);
        assert!(bar.chars().all(|c| c == '\u{2588}'));
    }

    #[test]
    fn bar_half_value_is_half_width() {
        let bar = bar_for_rtt(50.0, 100.0, 20);
        assert_eq!(bar.chars().count(), 10);
    }

    #[test]
    fn bar_clamps_overlarge_value_to_width() {
        let bar = bar_for_rtt(500.0, 100.0, 20);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn bar_subcell_partial() {
        let bar = bar_for_rtt(1.0, 16.0, 2);
        assert_eq!(bar.chars().count(), 1);
        assert_ne!(bar.chars().next(), Some('\u{2588}'));
    }
}
