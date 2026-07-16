use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::Color,
    style::Style,
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Dataset, GraphType, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
    Frame,
};

use super::super::charts;
use super::super::log_style;
use super::super::state::{push_wrapped_status_kv, UiState, REDACTED_PLACEHOLDER};
use super::{
    external_ip_for_family, max_y, quality_label_color, redact_log_line, show_or_redact,
};

pub fn draw_dashboard_full(area: Rect, f: &mut Frame, state: &UiState) {
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
    if state.throughput.dl_phase_start.is_some() && !state.throughput.dl_points.is_empty() {
        // Calculate x bounds only for download points
        let dl_x_max = state.throughput.dl_points.last().map(|(x, _)| *x).unwrap_or(0.0);
        let dl_x_min = state.throughput.dl_points.first().map(|(x, _)| *x).unwrap_or(0.0);

        let y_dl_max = max_y(&state.throughput.dl_points).max(10.0);
        let y_dl_max = (y_dl_max * 1.10).min(10_000.0);

        // Use all download points (they're already filtered to download phase)
        let dl_ds = Dataset::default()
            .graph_type(GraphType::Line)
            .marker(symbols::Marker::Braille)
            .style(Style::default().fg(Color::Green))
            .data(&state.throughput.dl_points);

        let dl_values: Vec<f64> = state.throughput.dl_points.iter().map(|(_, y)| *y).collect();
        let dl_metrics = crate::metrics::compute_sample_metrics(&dl_values);
        // Use the computed mean from metrics for the title to match what's shown below
        let dl_avg = dl_metrics.map(|m| m.mean).unwrap_or(state.throughput.dl_avg_mbps);
        let dl_title = Line::from(vec![
            Span::raw("Download (inst "),
            Span::styled(
                format!("{:.0}", state.throughput.dl_mbps),
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
        );
        f.render_widget(empty_chart, thr_row[0]);
    }

    // Upload throughput chart (right) - only show when upload phase has data
    if state.throughput.ul_phase_start.is_some() && !state.throughput.ul_points.is_empty() {
        // Calculate x bounds only for upload points
        let ul_x_max = state.throughput.ul_points.last().map(|(x, _)| *x).unwrap_or(0.0);
        let ul_x_min = state.throughput.ul_points.first().map(|(x, _)| *x).unwrap_or(0.0);

        let y_ul_max = max_y(&state.throughput.ul_points).max(10.0);
        let y_ul_max = (y_ul_max * 1.10).min(10_000.0);

        // Use all upload points (they're already filtered to upload phase)
        let ul_ds = Dataset::default()
            .graph_type(GraphType::Line)
            .marker(symbols::Marker::Braille)
            .style(Style::default().fg(Color::Cyan))
            .data(&state.throughput.ul_points);

        let ul_values: Vec<f64> = state.throughput.ul_points.iter().map(|(_, y)| *y).collect();
        let ul_metrics = crate::metrics::compute_sample_metrics(&ul_values);
        // Use the computed mean from metrics for the title to match what's shown below
        let ul_avg = ul_metrics.map(|m| m.mean).unwrap_or(state.throughput.ul_avg_mbps);
        let ul_title = Line::from(vec![
            Span::raw("Upload (inst "),
            Span::styled(
                format!("{:.0}", state.throughput.ul_mbps),
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
    if !state.latency.idle_latency_samples.is_empty() {
        // Use the same median calculation as the metrics below
        let median = crate::metrics::compute_sample_metrics(&state.latency.idle_latency_samples)
            .map(|m| m.median)
            .unwrap_or(f64::NAN);
        let jitter = crate::metrics::compute_jitter(&state.latency.idle_latency_samples);
        let title = Line::from(format!("Idle Latency ({:.0}ms)", median));
        charts::render_box_plot_with_metrics_inside(
            f,
            lat_row[0],
            &state.latency.idle_latency_samples,
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
    if !state.latency.loaded_dl_latency_samples.is_empty() {
        // Use the same median calculation as the metrics below
        let median = crate::metrics::compute_sample_metrics(&state.latency.loaded_dl_latency_samples)
            .map(|m| m.median)
            .unwrap_or(f64::NAN);
        let jitter = crate::metrics::compute_jitter(&state.latency.loaded_dl_latency_samples);
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
            &state.latency.loaded_dl_latency_samples,
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
    if !state.latency.loaded_ul_latency_samples.is_empty() {
        // Use the same median calculation as the metrics below
        let median = crate::metrics::compute_sample_metrics(&state.latency.loaded_ul_latency_samples)
            .map(|m| m.median)
            .unwrap_or(f64::NAN);
        let jitter = crate::metrics::compute_jitter(&state.latency.loaded_ul_latency_samples);
        let title = Line::from(vec![
            Span::raw("Latency Upload ("),
            Span::styled(format!("{:.0}ms", median), Style::default().fg(Color::Cyan)),
            Span::raw(")"),
        ]);
        charts::render_box_plot_with_metrics_inside(
            f,
            lat_row[2],
            &state.latency.loaded_ul_latency_samples,
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
    let (udp_sent, udp_received, udp_total, udp_latest_rtt) = if state.latency.udp_loss_total > 0 {
        (
            state.latency.udp_loss_sent,
            state.latency.udp_loss_received,
            state.latency.udp_loss_total,
            state.latency.udp_loss_latest_rtt_ms,
        )
    } else if let Some(exp) = state
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

    if let Some(err) = state
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

        let rtt_str = udp_latest_rtt
            .map(|v| format!("{:.0}ms", v))
            .unwrap_or_else(|| "-".to_string());

        // Get quality label, MOS, jitter, and reorder info from completed result
        let (quality_label, mos_str, jitter_str, reorder_str) = state
            .last_result
            .as_ref()
            .and_then(|r| r.experimental_udp.as_ref())
            .map(|exp| {
                let label = exp.quality_label.as_str();
                let mos = exp.mos.map(|m| format!("MOS {:.1}", m)).unwrap_or_default();
                let jitter = exp.latency.jitter_ms.map(|j| format!("jitter {:.1}ms", j)).unwrap_or_default();
                let reorder = format!("reorder {:.1}%", exp.out_of_order_pct);
                (label, mos, jitter, reorder)
            })
            .unwrap_or(("", String::new(), String::new(), String::new()));

        // Calculate text width before the bar
        let mut pre_bar_width: usize = 0;
        pre_bar_width += udp_status.len() + 1; // status + space
        if !quality_label.is_empty() {
            pre_bar_width += quality_label.len();
            if !mos_str.is_empty() {
                pre_bar_width += 2 + mos_str.len() + 2; // " (" + mos + ") "
            } else {
                pre_bar_width += 1; // space
            }
        }
        let loss_str = format!("loss {:.1}%", udp_loss_pct);
        let rtt_display = format!("rtt {}", rtt_str);
        pre_bar_width += loss_str.len() + 1 + rtt_display.len(); // loss + space + rtt
        if !jitter_str.is_empty() {
            pre_bar_width += 1 + jitter_str.len();
        }
        if !reorder_str.is_empty() && state.phase != crate::model::Phase::PacketLoss {
            pre_bar_width += 1 + reorder_str.len();
        }
        pre_bar_width += 2; // "  " before bar

        // Calculate text width after the bar
        let ok_str = format!("ok {}", safe_received);
        let lost_str = format!("lost {}", lost);
        let mut post_bar_width: usize = 2 + ok_str.len() + 1 + lost_str.len(); // "  " + ok + " " + lost
        if pending > 0 {
            post_bar_width += format!(" pending {}", pending).len();
        }

        // Calculate bar width from remaining space
        let total_text_width = pre_bar_width + post_bar_width;
        let available_width = udp_inner.width as usize;
        let bar_width = if available_width > total_text_width + 5 {
            available_width - total_text_width
        } else {
            10 // minimum bar width
        };

        // Ensure any loss shows at least one red segment
        let lost_units = if lost > 0 {
            ((lost as f64 / safe_total as f64) * bar_width as f64).ceil().max(1.0) as usize
        } else {
            0
        };
        let recv_units = ((safe_received as f64 / safe_total as f64) * bar_width as f64).floor() as usize;
        let pending_units = bar_width.saturating_sub(recv_units + lost_units);

        let bar_recv = "█".repeat(recv_units);
        let bar_lost = "█".repeat(lost_units);
        let bar_pending = "░".repeat(pending_units);

        let mut spans = vec![
            Span::styled(udp_status, Style::default().fg(Color::Yellow)),
            Span::raw(" "),
        ];

        // Show quality label and MOS when test is complete
        if !quality_label.is_empty() {
            let label_color = quality_label_color(quality_label);
            spans.push(Span::styled(quality_label, Style::default().fg(label_color)));
            if !mos_str.is_empty() {
                spans.push(Span::raw(" ("));
                spans.push(Span::styled(&mos_str, Style::default().fg(label_color)));
                spans.push(Span::raw(") "));
            } else {
                spans.push(Span::raw(" "));
            }
        }

        spans.extend(vec![
            Span::styled(
                loss_str,
                Style::default().fg(if udp_loss_pct == 0.0 { Color::Green } else if udp_loss_pct < 2.5 { Color::Yellow } else { Color::Red }),
            ),
            Span::raw(" "),
            Span::styled(rtt_display, Style::default().fg(Color::Gray)),
        ]);

        // Add jitter and reorder when available
        if !jitter_str.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(&jitter_str, Style::default().fg(Color::Gray)));
        }
        if !reorder_str.is_empty() && state.phase != crate::model::Phase::PacketLoss {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(&reorder_str, Style::default().fg(Color::Gray)));
        }

        spans.extend(vec![
            Span::raw("  "),
            Span::styled(bar_recv, Style::default().fg(Color::Green)),
            Span::styled(bar_lost, Style::default().fg(Color::Red)),
            Span::styled(bar_pending, Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(ok_str, Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(lost_str, Style::default().fg(Color::Red)),
        ]);

        if pending > 0 {
            spans.push(Span::styled(format!(" pending {}", pending), Style::default().fg(Color::DarkGray)));
        }

        f.render_widget(
            Paragraph::new(Line::from(spans)),
            udp_inner,
        );
    } else {
        let msg = if state.phase == crate::model::Phase::PacketLoss {
            "Packet loss probe starting..."
        } else {
            "Packet loss probe starts after upload phase..."
        };
        f.render_widget(Paragraph::new(msg), udp_inner);
    }

    // Network Information and Keyboard Shortcuts side-by-side
    let info_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(main[3]);

    // Network Information panel (left)

    // Determine IP version
    let ip_version = state
        .network
        .ip
        .as_deref()
        .map(|ip| if ip.contains(':') { "IPv6" } else { "IPv4" })
        .unwrap_or("-");

    let ip_version_color = match ip_version {
        "IPv4" => Color::Green,
        "IPv6" => Color::Cyan,
        _ => Color::Gray,
    };

    let is_wireless = state.network.is_wireless.unwrap_or(false);
    let (link_label, link_color) = if is_wireless {
        ("Wireless", Color::Yellow)
    } else {
        ("Wired", Color::Green)
    };

    let hide = state.hide_network_info;
    let network_display = if hide {
        REDACTED_PLACEHOLDER.to_string()
    } else {
        state
            .network
            .network_name
            .as_deref()
            .or_else(|| state.network.interface_name.as_deref())
            .unwrap_or("-")
            .to_string()
    };

    let mut network_lines = vec![
        Line::from(vec![
            Span::styled("Connected via: ", Style::default().fg(Color::Gray)),
            Span::styled(ip_version, Style::default().fg(ip_version_color)),
        ]),
        Line::from(vec![
            Span::styled("Interface: ", Style::default().fg(Color::Gray)),
            Span::raw(show_or_redact(state.network.interface_name.as_deref(), hide).to_string()),
            Span::raw(" ("),
            Span::styled(link_label, Style::default().fg(link_color)),
            Span::raw(")"),
        ]),
        Line::from(vec![
            Span::styled("Network: ", Style::default().fg(Color::Gray)),
            Span::raw(network_display),
        ]),
        Line::from(vec![
            Span::styled("MAC address: ", Style::default().fg(Color::Gray)),
            Span::styled(
                show_or_redact(state.network.interface_mac.as_deref(), hide).to_string(),
                Style::default().fg(Color::Magenta),
            ),
        ]),
    ];

    // Only show Certificate line if a certificate is set
    if let Some(ref cert_filename) = state.network.certificate_filename {
        network_lines.push(Line::from(vec![
            Span::styled("Certificate: ", Style::default().fg(Color::Gray)),
            Span::styled(cert_filename.clone(), Style::default().fg(Color::Cyan)),
        ]));
    }

    // Only show Proxy line if a proxy is set
    if let Some(ref proxy_url) = state.network.proxy_url {
        network_lines.push(Line::from(vec![
            Span::styled("Proxy: ", Style::default().fg(Color::Gray)),
            Span::styled(proxy_url.clone(), Style::default().fg(Color::Yellow)),
        ]));
    }

    network_lines.push(Line::from(vec![
        Span::styled("Server location: ", Style::default().fg(Color::Gray)),
        Span::styled(
            show_or_redact(state.network.server.as_deref(), hide).to_string(),
            Style::default().fg(Color::Cyan),
        ),
    ]));

    // "Your network: ORG (ASNXXXX)" — split so we can dim the AS number.
    let mut your_network: Vec<Span<'static>> = vec![Span::styled(
        "Your network: ",
        Style::default().fg(Color::Gray),
    )];
    if hide {
        your_network.push(Span::styled(
            REDACTED_PLACEHOLDER.to_string(),
            Style::default().fg(Color::Cyan),
        ));
    } else {
        match (state.network.as_org.as_deref(), state.network.asn.as_deref()) {
            (Some(org), Some(asn)) => {
                your_network.push(Span::styled(org.to_string(), Style::default().fg(Color::Cyan)));
                your_network.push(Span::raw(" ("));
                your_network.push(Span::styled(
                    format!("AS{}", asn),
                    Style::default().fg(Color::Magenta),
                ));
                your_network.push(Span::raw(")"));
            }
            (Some(org), None) => {
                your_network.push(Span::styled(org.to_string(), Style::default().fg(Color::Cyan)));
            }
            (None, Some(asn)) => {
                your_network.push(Span::styled(
                    format!("AS{}", asn),
                    Style::default().fg(Color::Magenta),
                ));
            }
            (None, None) => your_network.push(Span::raw("-")),
        }
    }
    network_lines.push(Line::from(your_network));

    network_lines.extend(vec![
        Line::from(vec![
            Span::styled("External IPv4: ", Style::default().fg(Color::Gray)),
            Span::styled(
                show_or_redact(
                    external_ip_for_family(
                        state.network.external_ipv4.as_deref(),
                        state.network.ip.as_deref(),
                        true,
                    ),
                    hide,
                )
                .to_string(),
                Style::default().fg(Color::Green),
            ),
        ]),
        Line::from(vec![
            Span::styled("External IPv6: ", Style::default().fg(Color::Gray)),
            Span::styled(
                show_or_redact(
                    external_ip_for_family(
                        state.network.external_ipv6.as_deref(),
                        state.network.ip.as_deref(),
                        false,
                    ),
                    hide,
                )
                .to_string(),
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ]);

    // Diagnostic results at the end, before the source link
    let has_diagnostics = state.diagnostics.dns_summary.is_some()
        || state.diagnostics.tls_summary.is_some()
        || state.diagnostics.ip_comparison.is_some()
        || state.diagnostics.traceroute_summary.is_some();

    if has_diagnostics {
        network_lines.push(Line::from("")); // Separator

        if let Some(ref dns) = state.diagnostics.dns_summary {
            network_lines.push(Line::from(vec![
                Span::styled("DNS resolution: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{:.2}ms", dns.resolution_time_ms),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }

        if let Some(ref tls) = state.diagnostics.tls_summary {
            network_lines.push(Line::from(vec![
                Span::styled("TLS handshake: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{:.2}ms", tls.handshake_time_ms),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(
                    tls.protocol_version.as_deref().unwrap_or("-").to_string(),
                    Style::default().fg(Color::Green),
                ),
            ]));
        }

        if let Some(ref cmp) = state.diagnostics.ip_comparison {
            let mut cmp_spans: Vec<Span<'static>> = vec![Span::styled(
                "IPv4 vs IPv6: ",
                Style::default().fg(Color::Gray),
            )];
            cmp_spans.push(Span::styled("v4:", Style::default().fg(Color::Gray)));
            match cmp.ipv4_result.as_ref() {
                Some(r) if r.available => cmp_spans.push(Span::styled(
                    format!("{:.1}Mbps", r.download_mbps),
                    Style::default().fg(Color::Green),
                )),
                Some(_) => cmp_spans.push(Span::styled("N/A", Style::default().fg(Color::Red))),
                None => cmp_spans.push(Span::raw("-")),
            }
            cmp_spans.push(Span::raw(" "));
            cmp_spans.push(Span::styled("v6:", Style::default().fg(Color::Gray)));
            match cmp.ipv6_result.as_ref() {
                Some(r) if r.available => cmp_spans.push(Span::styled(
                    format!("{:.1}Mbps", r.download_mbps),
                    Style::default().fg(Color::Cyan),
                )),
                Some(_) => cmp_spans.push(Span::styled("N/A", Style::default().fg(Color::Red))),
                None => cmp_spans.push(Span::raw("-")),
            }
            network_lines.push(Line::from(cmp_spans));
        }

        if let Some(ref tr) = state.diagnostics.traceroute_summary {
            let (status, status_color) = if tr.completed {
                ("complete", Color::Green)
            } else {
                ("partial", Color::Yellow)
            };
            network_lines.push(Line::from(vec![
                Span::styled("Traceroute: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    tr.hops.len().to_string(),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(" hops (", Style::default().fg(Color::Gray)),
                Span::styled(status, Style::default().fg(status_color)),
                Span::styled(")", Style::default().fg(Color::Gray)),
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

    let hide_hint = if state.hide_network_info {
        " Shift+H to reveal "
    } else {
        " Shift+H to hide info "
    };
    let network_info = Paragraph::new(network_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Network Information")
            .title_bottom(
                Line::from(Span::styled(hide_hint, Style::default().fg(Color::DarkGray)))
                    .right_aligned(),
            ),
    );
    f.render_widget(network_info, info_row[0]);

    // Test Activity panel (right): renders the tail of the rolling event log
    // — same lines text mode (`--text`) prints to stderr, single source of
    // truth via `crate::event_format::format_event_lines`. Scroll with
    // ↑/↓/j/k and PgUp/PgDn while on the Dashboard tab.
    let title = if state.dashboard_log_scroll > 0 {
        format!(
            "Test Activity  (scrolled -{}, ↓ to follow)",
            state.dashboard_log_scroll
        )
    } else {
        "Test Activity  (↑↓/PgUp/PgDn to scroll)".to_string()
    };
    let hide_hint = if state.hide_network_info {
        " Shift+H to reveal "
    } else {
        " Shift+H to hide info "
    };
    let panel = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(
            Line::from(Span::styled(hide_hint, Style::default().fg(Color::DarkGray)))
                .right_aligned(),
        );
    let inner = panel.inner(info_row[1]);
    let visible_rows = inner.height as usize;

    let activity_lines: Vec<Line> = if state.text_log.is_empty() {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No run yet — press 'r' to start",
                Style::default().fg(Color::Gray),
            )),
        ]
    } else {
        let total = state.text_log.len();
        // scroll == 0 → show newest visible_rows lines.
        // scroll == N → window ends N lines before the newest.
        let end = total.saturating_sub(state.dashboard_log_scroll);
        let start = end.saturating_sub(visible_rows);
        state.text_log[start..end]
            .iter()
            .map(|s| log_style::style_log_line(&redact_log_line(s, state)))
            .collect()
    };

    let activity = Paragraph::new(activity_lines).block(panel);
    f.render_widget(activity, info_row[1]);

    // Scrollbar on the right edge of the Test Activity panel, only when the
    // log overflows the visible area.
    let total = state.text_log.len();
    if total > visible_rows {
        let max_scroll = total.saturating_sub(visible_rows);
        // Scrollbar position is 0 at the top and `max_scroll` at the bottom.
        // Our `dashboard_log_scroll` is 0 at the bottom (newest), so flip it.
        let position = max_scroll.saturating_sub(state.dashboard_log_scroll);
        let mut scrollbar_state = ScrollbarState::new(max_scroll).position(position);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            info_row[1].inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

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

