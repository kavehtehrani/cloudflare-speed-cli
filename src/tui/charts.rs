use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Color,
    style::Style,
    text::{Line, Span},
    widgets::canvas::Line as CanvasLine,
    widgets::{canvas::Canvas, Bar, BarChart, BarGroup, Block, Borders, Chart, Dataset, Paragraph},
    Frame,
};
use std::collections::HashMap;

use super::state::UiState;
use crate::model::RunResult;

/// Helper function to draw a line on a canvas
pub fn draw_line(
    ctx: &mut ratatui::widgets::canvas::Context,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: Color,
) {
    ctx.draw(&CanvasLine {
        x1,
        y1,
        x2,
        y2,
        color,
    });
}

/// Helper function to render a box plot with metrics inside the same bordered box
pub fn render_box_plot_with_metrics_inside(
    f: &mut Frame,
    area: Rect,
    samples: &[f64],
    title: Line,
    color: Option<Color>,
    jitter: Option<f64>,
    loss: Option<f64>,
) {
    // Get inner area (accounting for borders)
    let inner = if area.width > 2 && area.height > 2 {
        Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        }
    } else {
        area
    };

    // Split inner area into chart (top) and metrics (bottom)
    let chart_metrics = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)].as_ref())
        .split(inner);

    // Render box plot in top area (without its own borders, we'll add them to the whole area)
    if samples.len() >= 2 {
        // Create box plot without borders (we'll add borders to the whole widget)
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = sorted.len();
        let (min_val, max_val, q1, med, q3, mean) = if n >= 2 {
            let min = sorted[0];
            let q1_val = sorted[n / 4];
            let med_val = sorted[n / 2];
            let q3_val = sorted[3 * n / 4];
            let max = sorted[n - 1];
            let mean_val = samples.iter().sum::<f64>() / samples.len() as f64;
            (min, max, q1_val, med_val, q3_val, mean_val)
        } else {
            (0.0, 1.0, 0.0, 0.0, 0.0, 0.0)
        };

        let canvas = Canvas::default()
            .x_bounds([min_val - 0.5, max_val + 0.5])
            .y_bounds([-1.0, 1.0])
            .paint(move |ctx| {
                if n >= 2 {
                    // Box (Q1 to Q3)
                    draw_line(ctx, q1, -0.4, q3, -0.4, Color::White);
                    draw_line(ctx, q1, 0.4, q3, 0.4, Color::White);
                    draw_line(ctx, q1, -0.4, q1, 0.4, Color::White);
                    draw_line(ctx, q3, -0.4, q3, 0.4, Color::White);

                    // Median
                    draw_line(ctx, med, -0.4, med, 0.4, Color::Yellow);

                    // Mean
                    draw_line(ctx, mean, -0.4, mean, 0.4, Color::Cyan);

                    // Whiskers
                    draw_line(ctx, min_val, 0.0, q1, 0.0, Color::White);
                    draw_line(ctx, q3, 0.0, max_val, 0.0, Color::White);

                    // Whisker caps
                    draw_line(ctx, min_val, -0.2, min_val, 0.2, Color::White);
                    draw_line(ctx, max_val, -0.2, max_val, 0.2, Color::White);
                }
            });
        f.render_widget(canvas, chart_metrics[0]);

        // Render metrics in bottom area
        if let Some(metrics) = crate::metrics::compute_metrics(samples) {
            let metrics_text = render_metrics_text(metrics, jitter, loss, color);
            f.render_widget(
                Paragraph::new(metrics_text).alignment(Alignment::Center),
                chart_metrics[1],
            );
        }
    } else {
        let empty = Paragraph::new("Waiting for data...");
        f.render_widget(empty, inner);
    }

    // Render the border with title around the whole area
    let block = Block::default().borders(Borders::ALL).title(title);
    f.render_widget(block, area);
}

/// Helper function to render metrics text (avg, med, p25, p75, and optionally jitter, loss)
fn render_metrics_text<'a>(
    metrics: (f64, f64, f64, f64),
    jitter: Option<f64>,
    loss: Option<f64>,
    color: Option<Color>,
) -> Line<'a> {
    let (mean_val, median_val, p25_val, p75_val) = metrics;
    if let Some(c) = color {
        let mut spans = vec![
            Span::styled("avg", Style::default().fg(Color::Gray)),
            Span::styled(format!(" {:.0}", mean_val), Style::default().fg(c)),
            Span::raw(" "),
            Span::styled("med", Style::default().fg(Color::Gray)),
            Span::styled(format!(" {:.0}", median_val), Style::default().fg(c)),
            Span::raw(" "),
            Span::styled("p25", Style::default().fg(Color::Gray)),
            Span::styled(format!(" {:.0}", p25_val), Style::default().fg(c)),
            Span::raw(" "),
            Span::styled("p75", Style::default().fg(Color::Gray)),
            Span::styled(format!(" {:.0}", p75_val), Style::default().fg(c)),
        ];
        if let Some(j) = jitter {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("jit", Style::default().fg(Color::Gray)));
            spans.push(Span::styled(format!(" {:.1}", j), Style::default().fg(c)));
        }
        if let Some(l) = loss {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("loss", Style::default().fg(Color::Gray)));
            spans.push(Span::styled(format!(" {:.1}%", l * 100.0), Style::default().fg(c)));
        }
        Line::from(spans)
    } else {
        let mut parts = format!(
            "avg {:.0} med {:.0} p25 {:.0} p75 {:.0}",
            mean_val, median_val, p25_val, p75_val
        );
        if let Some(j) = jitter {
            parts.push_str(&format!(" jit {:.1}", j));
        }
        if let Some(l) = loss {
            parts.push_str(&format!(" loss {:.1}%", l * 100.0));
        }
        Line::from(parts)
    }
}

/// Helper function to render a throughput chart with metrics inside the same bordered box
pub fn render_chart_with_metrics_inside(
    f: &mut Frame,
    area: Rect,
    datasets: Vec<Dataset>,
    x_axis: ratatui::widgets::Axis,
    y_axis: ratatui::widgets::Axis,
    title: Line,
    metrics: Option<(f64, f64, f64, f64)>,
    color: Color,
) {
    // Get inner area (accounting for borders)
    let inner = if area.width > 2 && area.height > 2 {
        Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        }
    } else {
        area
    };

    // Split inner area into chart (top) and metrics (bottom)
    let chart_metrics = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1)].as_ref())
        .split(inner);

    // Render chart in top area (without its own borders, we'll add them to the whole area)
    let chart_without_borders = Chart::new(datasets).x_axis(x_axis).y_axis(y_axis);
    f.render_widget(chart_without_borders, chart_metrics[0]);

    // Render metrics in bottom area (no jitter or loss for throughput charts)
    if let Some(metrics) = metrics {
        let metrics_text = render_metrics_text(metrics, None, None, Some(color));
        f.render_widget(
            Paragraph::new(metrics_text).alignment(Alignment::Center),
            chart_metrics[1],
        );
    }

    // Render the border with title around the whole area
    let block = Block::default().borders(Borders::ALL).title(title);
    f.render_widget(block, area);
}

pub fn draw_charts(area: Rect, f: &mut Frame, state: &UiState) {
    // Assign consistent colors to networks using a HashMap for reliable lookup
    let network_colors = [
        Color::Green,
        Color::Cyan,
        Color::Magenta,
        Color::Yellow,
        Color::Blue,
        Color::LightRed,
        Color::LightGreen,
        Color::LightCyan,
        Color::LightMagenta,
        Color::LightYellow,
    ];

    // Build color map from available networks
    let network_color_map: HashMap<&str, Color> = state
        .charts_available_networks
        .iter()
        .enumerate()
        .map(|(idx, name)| (name.as_str(), network_colors[idx % network_colors.len()]))
        .collect();

    // Filter history by selected network
    let filtered_data: Vec<&RunResult> = state
        .history
        .iter()
        .filter(|r| {
            if let Some(ref filter_network) = state.charts_network_filter {
                r.network_name.as_ref() == Some(filter_network)
            } else {
                true // Show all
            }
        })
        .collect();

    // Layout: header (2 lines + border) + two charts
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
        .split(area);

    // Header with network filter info
    let filter_display = match &state.charts_network_filter {
        None => "All Networks".to_string(),
        Some(n) => n.clone(),
    };
    let network_count = state.charts_available_networks.len();

    // Build a colored legend showing network -> color mapping
    let mut legend_spans: Vec<Span> = vec![Span::raw("Networks: ")];
    for (idx, network) in state.charts_available_networks.iter().enumerate() {
        if idx > 0 {
            legend_spans.push(Span::raw(", "));
        }
        let color = network_colors[idx % network_colors.len()];
        legend_spans.push(Span::styled(network.as_str(), Style::default().fg(color)));
    }

    let header_text = vec![
        Line::from(vec![
            Span::raw("Filter: "),
            Span::styled(&filter_display, Style::default().fg(Color::Yellow)),
            Span::raw(format!(
                " ({} of {}) - ",
                if state.charts_network_filter.is_none() {
                    0
                } else {
                    state
                        .charts_available_networks
                        .iter()
                        .position(|n| Some(n) == state.charts_network_filter.as_ref())
                        .map(|i| i + 1)
                        .unwrap_or(0)
                },
                network_count
            )),
            Span::styled("←/→", Style::default().fg(Color::Magenta)),
            Span::raw(" or "),
            Span::styled("h/l", Style::default().fg(Color::Magenta)),
            Span::raw(": cycle"),
        ]),
        Line::from(legend_spans),
    ];
    let header = Paragraph::new(header_text).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, chunks[0]);

    // Charts area split vertically (DL on top, UL on bottom)
    let chart_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(chunks[1]);

    // Calculate how many bars can fit based on available width
    // Chart width = chunks[1].width - Y-axis label width (6) - borders (2)
    let available_chart_width = chunks[1].width.saturating_sub(8) as usize;
    // Cap at 200 bars max for performance, but allow wider bars on ultra-wide screens
    let max_bars = available_chart_width.max(1).min(200);

    // Prepare data for charts: take only as many as can fit, then reverse so oldest is on left, newest on right
    let data_points: Vec<_> = filtered_data
        .iter()
        .take(max_bars) // Take only as many as can fit (history is newest-first)
        .collect::<Vec<_>>()
        .into_iter()
        .rev() // Reverse so oldest is on left, newest on right
        .collect();

    if data_points.is_empty() {
        let empty = Paragraph::new("No data available for selected network.")
            .block(Block::default().borders(Borders::ALL).title("Charts"));
        f.render_widget(empty, chunks[1]);
        return;
    }

    let num_bars = data_points.len();

    // Calculate max values for scaling
    let max_dl = data_points
        .iter()
        .map(|r| r.download.mbps)
        .fold(0.0_f64, |a, b| a.max(b))
        .max(10.0);
    let max_ul = data_points
        .iter()
        .map(|r| r.upload.mbps)
        .fold(0.0_f64, |a, b| a.max(b))
        .max(10.0);

    // Compute colors ONCE for all data points (same color for DL and UL of same test)
    let bar_colors: Vec<Color> = data_points
        .iter()
        .map(|r| {
            if state.charts_network_filter.is_none() {
                // "All Networks" view - color by network
                r.network_name
                    .as_ref()
                    .and_then(|n| network_color_map.get(n.as_str()).copied())
                    .unwrap_or(Color::Gray) // Fallback for entries with no network name
            } else {
                // Single network view - use consistent green
                Color::Green
            }
        })
        .collect();

    // Create download bars with per-bar colors
    let dl_bars: Vec<Bar> = data_points
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Bar::default()
                .value(r.download.mbps as u64)
                .style(Style::default().fg(bar_colors[i]))
        })
        .collect();

    // Split download chart area into Y-axis labels and chart
    let dl_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(6), Constraint::Min(0)].as_ref())
        .split(chart_chunks[0]);

    // Recalculate bar width for the actual chart area
    let dl_chart_width = dl_layout[1].width.saturating_sub(2) as usize;
    let dl_bar_width = if num_bars > 0 {
        (dl_chart_width / num_bars).max(1) as u16
    } else {
        1
    };

    // Y-axis labels for download - offset by 1 at top/bottom to align with chart's inner area
    let dl_label_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // border offset (aligns with chart top border)
            Constraint::Length(1), // max
            Constraint::Min(0),    // spacer (fills middle)
            Constraint::Length(1), // 0
            Constraint::Length(1), // border offset (aligns with chart bottom border)
        ])
        .split(dl_layout[0]);

    f.render_widget(
        Paragraph::new(format!("{:>5.0}", max_dl)).style(Style::default().fg(Color::Gray)),
        dl_label_layout[1],
    );
    f.render_widget(
        Paragraph::new(format!("{:>5}", "0")).style(Style::default().fg(Color::Gray)),
        dl_label_layout[3],
    );

    let dl_chart = BarChart::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Download (max {:.0} Mbps)", max_dl)),
        )
        .data(BarGroup::default().bars(&dl_bars))
        .bar_width(dl_bar_width)
        .bar_gap(0)
        .max(max_dl as u64);

    f.render_widget(dl_chart, dl_layout[1]);

    // Create upload bars with same colors as download
    let ul_bars: Vec<Bar> = data_points
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Bar::default()
                .value(r.upload.mbps as u64)
                .style(Style::default().fg(bar_colors[i]))
        })
        .collect();

    // Split upload chart area into Y-axis labels and chart
    let ul_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(6), Constraint::Min(0)].as_ref())
        .split(chart_chunks[1]);

    // Recalculate bar width for upload chart area
    let ul_chart_width = ul_layout[1].width.saturating_sub(2) as usize;
    let ul_bar_width = if num_bars > 0 {
        (ul_chart_width / num_bars).max(1) as u16
    } else {
        1
    };

    // Y-axis labels for upload - offset by 1 at top/bottom to align with chart's inner area
    let ul_label_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // border offset (aligns with chart top border)
            Constraint::Length(1), // max
            Constraint::Min(0),    // spacer (fills middle)
            Constraint::Length(1), // 0
            Constraint::Length(1), // border offset (aligns with chart bottom border)
        ])
        .split(ul_layout[0]);

    f.render_widget(
        Paragraph::new(format!("{:>5.0}", max_ul)).style(Style::default().fg(Color::Gray)),
        ul_label_layout[1],
    );
    f.render_widget(
        Paragraph::new(format!("{:>5}", "0")).style(Style::default().fg(Color::Gray)),
        ul_label_layout[3],
    );

    let ul_chart = BarChart::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Upload (max {:.0} Mbps)", max_ul)),
        )
        .data(BarGroup::default().bars(&ul_bars))
        .bar_width(ul_bar_width)
        .bar_gap(0)
        .max(max_ul as u64);

    f.render_widget(ul_chart, ul_layout[1]);
}
