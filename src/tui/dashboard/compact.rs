use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Color,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Sparkline},
    Frame,
};

use super::super::state::{UiState, REDACTED_PLACEHOLDER};
use super::{quality_label_color, show_or_redact, udp_split_bar};

pub fn draw_dashboard_compact(area: Rect, f: &mut Frame, state: &UiState) {
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
                            format!("{:.0}", state.throughput.dl_mbps),
                            Style::default().fg(Color::Green),
                        ),
                        Span::raw(" / avg "),
                        Span::styled(
                            format!("{:.0}", state.throughput.dl_avg_mbps),
                            Style::default().fg(Color::Green),
                        ),
                        Span::raw(" Mbps)"),
                    ])),
            )
            .data(&state.throughput.dl_series)
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
                            format!("{:.0}", state.throughput.ul_mbps),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(" / avg "),
                        Span::styled(
                            format!("{:.0}", state.throughput.ul_avg_mbps),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(" Mbps)"),
                    ])),
            )
            .data(&state.throughput.ul_series)
            .style(Style::default().fg(Color::Cyan)),
        top_row[1],
    );

    // Bottom row: Idle latency text box and Status box side by side
    let bottom_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(content[1]);

    // Idle latency stats text box
    let idle_lat = if state.latency.idle_latency_samples.is_empty() && state.latency.idle_latency_sent == 0 {
        None
    } else {
        Some(UiState::compute_live_latency_stats(
            &state.latency.idle_latency_samples,
            state.latency.idle_latency_sent,
            state.latency.idle_latency_received,
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
            Span::raw(show_or_redact(state.network.interface_name.as_deref(), state.hide_network_info).to_string()),
            Span::raw(" ("),
            Span::raw(if state.network.is_wireless.unwrap_or(false) {
                "Wireless"
            } else {
                "Wired"
            }),
            Span::raw(")"),
        ]),
        Line::from(vec![
            Span::styled("Network: ", Style::default().fg(Color::Gray)),
            Span::raw(if state.hide_network_info {
                REDACTED_PLACEHOLDER.to_string()
            } else {
                state
                    .network
                    .network_name
                    .as_deref()
                    .or_else(|| state.network.interface_name.as_deref())
                    .unwrap_or("-")
                    .to_string()
            }),
        ]),
    ];

    // Only show Certificate line if a certificate is set
    if let Some(ref cert_filename) = state.network.certificate_filename {
        meta_lines.push(Line::from(vec![
            Span::styled("Certificate: ", Style::default().fg(Color::Gray)),
            Span::raw(cert_filename),
        ]));
    }

    // Only show Proxy line if a proxy is set
    if let Some(ref proxy_url) = state.network.proxy_url {
        meta_lines.push(Line::from(vec![
            Span::styled("Proxy: ", Style::default().fg(Color::Gray)),
            Span::styled(proxy_url, Style::default().fg(Color::Yellow)),
        ]));
    }

    let hide = state.hide_network_info;
    meta_lines.extend(vec![
        Line::from(vec![
            Span::styled("IP/Colo: ", Style::default().fg(Color::Gray)),
            Span::raw(format!(
                "{} / {}",
                show_or_redact(state.network.ip.as_deref(), hide),
                show_or_redact(state.network.colo.as_deref(), hide),
            )),
        ]),
        Line::from(vec![
            Span::styled("Server: ", Style::default().fg(Color::Gray)),
            Span::raw(show_or_redact(state.network.server.as_deref(), hide).to_string()),
        ]),
    ]);

    // Add condensed diagnostic info if available
    let mut diag_parts: Vec<String> = Vec::new();
    if let Some(ref dns) = state.diagnostics.dns_summary {
        diag_parts.push(format!("DNS:{:.0}ms", dns.resolution_time_ms));
    }
    if let Some(ref tls) = state.diagnostics.tls_summary {
        diag_parts.push(format!("TLS:{:.0}ms", tls.handshake_time_ms));
    }
    if let Some(ref tr) = state.diagnostics.traceroute_summary {
        diag_parts.push(format!("Hops:{}", tr.hops.len()));
    }
    if !diag_parts.is_empty() {
        meta_lines.push(Line::from(vec![
            Span::styled("Diag: ", Style::default().fg(Color::Gray)),
            Span::raw(diag_parts.join(" | ")),
        ]));
    }
    if let Some(exp) = state
        .last_result
        .as_ref()
        .and_then(|r| r.experimental_udp.as_ref())
    {
        let label_color = quality_label_color(&exp.quality_label);
        let mos_str = exp.mos.map(|m| format!(" MOS {:.1}", m)).unwrap_or_default();
        meta_lines.push(Line::from(vec![
            Span::styled("UDP: ", Style::default().fg(Color::Gray)),
            Span::styled(&exp.quality_label, Style::default().fg(label_color)),
            Span::styled(mos_str, Style::default().fg(label_color)),
            Span::styled(format!(" loss {:.1}%", exp.latency.loss * 100.0), Style::default().fg(Color::Yellow)),
            Span::styled(format!(" reorder {:.1}%", exp.out_of_order_pct), Style::default().fg(Color::Gray)),
        ]));
        meta_lines.push(udp_split_bar(exp.latency.sent, exp.latency.received, 12));
    }

    meta_lines.extend(vec![
        Line::from(vec![
            Span::styled("Info: ", Style::default().fg(Color::Gray)),
            Span::raw(&state.info),
        ]),
        Line::from(""),
        Line::from("Keys: q quit | r rerun | p pause | s save json | tab switch | ? help"),
    ]);

    let hide_hint = if state.hide_network_info {
        " Shift+H to reveal "
    } else {
        " Shift+H to hide info "
    };
    let meta = Paragraph::new(meta_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Network Information")
            .title_bottom(
                Line::from(Span::styled(hide_hint, Style::default().fg(Color::DarkGray)))
                    .right_aligned(),
            ),
    );
    f.render_widget(meta, bottom_row[1]);
}

