use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Color,
    style::Style,
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Dataset, GraphType, Paragraph, Sparkline},
    Frame,
};

use super::charts;
use super::state::{push_wrapped_status_kv, UiState};

/// Helper function to get the maximum y value from a series of points
pub fn max_y(points: &[(f64, f64)]) -> f64 {
    points.iter().map(|(_, y)| *y).fold(0.0, |a, b| a.max(b))
}

fn udp_split_bar(sent: u64, received: u64, width: usize) -> Line<'static> {
    let safe_sent = sent.max(1);
    let safe_received = received.min(safe_sent);
    let lost = safe_sent.saturating_sub(safe_received);
    let ok_units = ((safe_received as f64 / safe_sent as f64) * width as f64).round() as usize;
    let lost_units = width.saturating_sub(ok_units);

    let ok_part = "=".repeat(ok_units);
    let lost_part = "x".repeat(lost_units);

    Line::from(vec![
        Span::styled("UDP split: ", Style::default().fg(Color::Gray)),
        Span::raw("["),
        Span::styled(ok_part, Style::default().fg(Color::Green)),
        Span::styled(lost_part, Style::default().fg(Color::Red)),
        Span::raw("] "),
        Span::styled(format!("ok {} lost {}", safe_received, lost), Style::default().fg(Color::Gray)),
    ])
}

pub fn draw_dashboard(area: Rect, f: &mut Frame, state: &UiState) {
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
                Constraint::Length(3),  // Packet loss (UDP) row
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

    // Latency box plots: Idle, Loaded DL, Loaded UL
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
            None,
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
            None,
        );
    } else {
        let empty = Paragraph::new("Waiting for data...").block(
            Block::default()
                .borders(Borders::ALL)
                .title("Latency Upload"),
        );
        f.render_widget(empty, lat_row[2]);
    }

    // Packet loss row (full width) with live progress during measurement
    let (udp_sent, udp_received, udp_total, udp_latest_rtt) = if state.udp_loss_total > 0 {
        (
            state.udp_loss_sent,
            state.udp_loss_received,
            state.udp_loss_total,
            state.udp_loss_latest_rtt_ms,
        )
    } else if let Some(ref exp) = state
        .last_result
        .as_ref()
        .and_then(|r| r.experimental_udp.as_ref())
    {
        (
            exp.latency.sent,
            exp.latency.received,
            exp.latency.sent,
            exp.latency.median_ms,
        )
    } else {
        (0, 0, 0, None)
    };
    let udp_loss_pct = if udp_sent == 0 {
        0.0
    } else {
        ((udp_sent.saturating_sub(udp_received)) as f64) * 100.0 / udp_sent as f64
    };
    let udp_status = if state.phase == crate::model::Phase::PacketLoss {
        "running"
    } else if udp_sent > 0 {
        "complete"
    } else {
        "waiting"
    };
    let udp_block = Block::default()
        .borders(Borders::ALL)
        .title("Packet Loss (UDP/TURN)");
    let udp_inner = udp_block.inner(main[2]);
    f.render_widget(udp_block, main[2]);

    if let Some(ref err) = state
        .last_result
        .as_ref()
        .and_then(|r| r.udp_error.as_ref())
    {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Packet loss probe failed: ", Style::default().fg(Color::Gray)),
                Span::styled(err.as_str(), Style::default().fg(Color::Yellow)),
            ])),
            udp_inner,
        );
    } else if udp_total > 0 || udp_sent > 0 {
        let safe_total = udp_total.max(udp_sent).max(1);
        let safe_received = udp_received.min(udp_sent);
        let lost = udp_sent.saturating_sub(safe_received);
        let pending = safe_total.saturating_sub(udp_sent);

        let bar_width = udp_inner.width.saturating_sub(50) as usize;
        let bar_width = bar_width.max(10);

        let recv_units = ((safe_received as f64 / safe_total as f64) * bar_width as f64).round() as usize;
        let lost_units = ((lost as f64 / safe_total as f64) * bar_width as f64).round() as usize;
        let pending_units = bar_width.saturating_sub(recv_units + lost_units);

        let bar_recv = "█".repeat(recv_units);
        let bar_lost = "█".repeat(lost_units);
        let bar_pending = "░".repeat(pending_units);

        let rtt_str = udp_latest_rtt
            .map(|v| format!("{:.0}ms", v))
            .unwrap_or_else(|| "-".to_string());

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(udp_status, Style::default().fg(Color::Yellow)),
                Span::raw(" "),
                Span::styled(
                    format!("{}/{}", udp_sent, safe_total),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("loss {:.1}%", udp_loss_pct),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(format!("rtt {}", rtt_str), Style::default().fg(Color::Gray)),
                Span::raw("  "),
                Span::styled(bar_recv, Style::default().fg(Color::Green)),
                Span::styled(bar_lost, Style::default().fg(Color::Red)),
                Span::styled(bar_pending, Style::default().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled(format!("ok {}", safe_received), Style::default().fg(Color::Green)),
                Span::raw(" "),
                Span::styled(format!("lost {}", lost), Style::default().fg(Color::Red)),
                if pending > 0 {
                    Span::styled(format!(" pending {}", pending), Style::default().fg(Color::DarkGray))
                } else {
                    Span::raw("")
                },
            ])),
            udp_inner,
        );
    } else {
        f.render_widget(
            Paragraph::new("Packet loss probe starts after upload phase..."),
            udp_inner,
        );
    }

    // Network Information and Keyboard Shortcuts side-by-side
    let info_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)].as_ref())
        .split(main[3]);

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
                    .or_else(|| state.interface_name.as_deref())
                    .unwrap_or("-"),
            ),
        ]),
        Line::from(vec![
            Span::styled("MAC address: ", Style::default().fg(Color::Gray)),
            Span::raw(state.interface_mac.as_deref().unwrap_or("-")),
        ]),
    ];

    // Only show Certificate line if a certificate is set
    if let Some(ref cert_filename) = state.certificate_filename {
        network_lines.push(Line::from(vec![
            Span::styled("Certificate: ", Style::default().fg(Color::Gray)),
            Span::raw(cert_filename),
        ]));
    }

    // Only show Proxy line if a proxy is set
    if let Some(ref proxy_url) = state.proxy_url {
        network_lines.push(Line::from(vec![
            Span::styled("Proxy: ", Style::default().fg(Color::Gray)),
            Span::styled(proxy_url, Style::default().fg(Color::Yellow)),
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
            Span::styled("External IPv4: ", Style::default().fg(Color::Gray)),
            Span::raw(
                state
                    .external_ipv4
                    .as_deref()
                    .unwrap_or(state.ip.as_deref().unwrap_or("-")),
            ),
        ]),
        Line::from(vec![
            Span::styled("External IPv6: ", Style::default().fg(Color::Gray)),
            Span::raw(state.external_ipv6.as_deref().unwrap_or("-")),
        ]),
    ]);

    // Add diagnostic results if available
    let has_diagnostics = state.dns_summary.is_some()
        || state.tls_summary.is_some()
        || state.ip_comparison.is_some()
        || state.traceroute_summary.is_some();

    if has_diagnostics {
        network_lines.push(Line::from("")); // Separator

        if let Some(ref dns) = state.dns_summary {
            network_lines.push(Line::from(vec![
                Span::styled("DNS resolution: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.2}ms", dns.resolution_time_ms)),
            ]));
        }

        if let Some(ref tls) = state.tls_summary {
            network_lines.push(Line::from(vec![
                Span::styled("TLS handshake: ", Style::default().fg(Color::Gray)),
                Span::raw(format!(
                    "{:.2}ms {}",
                    tls.handshake_time_ms,
                    tls.protocol_version.as_deref().unwrap_or("-")
                )),
            ]));
        }

        if let Some(ref cmp) = state.ip_comparison {
            let v4_str = cmp
                .ipv4_result
                .as_ref()
                .map(|r| {
                    if r.available {
                        format!("{:.1}Mbps", r.download_mbps)
                    } else {
                        "N/A".to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string());
            let v6_str = cmp
                .ipv6_result
                .as_ref()
                .map(|r| {
                    if r.available {
                        format!("{:.1}Mbps", r.download_mbps)
                    } else {
                        "N/A".to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string());
            network_lines.push(Line::from(vec![
                Span::styled("IPv4 vs IPv6: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("v4:{} v6:{}", v4_str, v6_str)),
            ]));
        }

        if let Some(ref tr) = state.traceroute_summary {
            let status = if tr.completed { "complete" } else { "partial" };
            network_lines.push(Line::from(vec![
                Span::styled("Traceroute: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{} hops ({})", tr.hops.len(), status)),
            ]));
        }
    }

    network_lines.extend(vec![
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
        push_wrapped_status_kv(&mut status_lines, "Comments", comments, main[4].width);
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
            let status_area_width = main[4].width.saturating_sub(4);
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
    f.render_widget(status, main[4]);
}

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

    // Only show Proxy line if a proxy is set
    if let Some(ref proxy_url) = state.proxy_url {
        meta_lines.push(Line::from(vec![
            Span::styled("Proxy: ", Style::default().fg(Color::Gray)),
            Span::styled(proxy_url, Style::default().fg(Color::Yellow)),
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
            Span::styled("Server: ", Style::default().fg(Color::Gray)),
            Span::raw(state.server.as_deref().unwrap_or("-")),
        ]),
    ]);

    // Add condensed diagnostic info if available
    let mut diag_parts: Vec<String> = Vec::new();
    if let Some(ref dns) = state.dns_summary {
        diag_parts.push(format!("DNS:{:.0}ms", dns.resolution_time_ms));
    }
    if let Some(ref tls) = state.tls_summary {
        diag_parts.push(format!("TLS:{:.0}ms", tls.handshake_time_ms));
    }
    if let Some(ref tr) = state.traceroute_summary {
        diag_parts.push(format!("Hops:{}", tr.hops.len()));
    }
    if !diag_parts.is_empty() {
        meta_lines.push(Line::from(vec![
            Span::styled("Diag: ", Style::default().fg(Color::Gray)),
            Span::raw(diag_parts.join(" | ")),
        ]));
    }
    if let Some(ref exp) = state
        .last_result
        .as_ref()
        .and_then(|r| r.experimental_udp.as_ref())
    {
        meta_lines.push(Line::from(vec![
            Span::styled("UDP loss (TURN): ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:.1}%", exp.latency.loss * 100.0), Style::default().fg(Color::Yellow)),
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

    let meta = Paragraph::new(meta_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Network Information"),
    );
    f.render_widget(meta, bottom_row[1]);
}
