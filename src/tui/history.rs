use crate::model::RunResult;
use ratatui::{
    layout::Rect,
    style::Color,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::state::UiState;

pub fn show_history(area: Rect, f: &mut Frame, state: &UiState) {
    let mut lines: Vec<Line> = Vec::new();

    // Filter history based on filter text (case-insensitive search in network_name, interface_name, as_org, colo)
    let filter_lower = state.history_filter.to_lowercase();
    let filtered_history: Vec<&RunResult> = if state.history_filter.is_empty() {
        state.history.iter().collect()
    } else {
        state
            .history
            .iter()
            .filter(|r| {
                let matches_field = |opt: &Option<String>| {
                    opt.as_ref()
                        .map(|s| s.to_lowercase().contains(&filter_lower))
                        .unwrap_or(false)
                };
                matches_field(&r.network_name)
                    || matches_field(&r.interface_name)
                    || matches_field(&r.as_org)
                    || matches_field(&r.colo)
                    || matches_field(&r.comments)
            })
            .collect()
    };

    // Calculate how many items can fit in the available area
    // Subtract 4 for: controls line, filter line (optional), column headers, borders
    let max_items = (area.height as usize).saturating_sub(4);

    // Show total count and current position
    let total_count = filtered_history.len();
    let current_pos = if total_count > 0 {
        state.history_selected.min(total_count.saturating_sub(1)) + 1
    } else {
        0
    };

    // Build header line with controls
    let mut header_spans = vec![Span::raw(format!("History ({}/{}", current_pos, total_count))];
    if !state.history_filter.is_empty() {
        header_spans.push(Span::styled(
            format!(" filtered from {}", state.history.len()),
            Style::default().fg(Color::Yellow),
        ));
    }
    if total_count > max_items {
        header_spans.push(Span::raw(format!(", showing {}", max_items)));
    }
    header_spans.extend(vec![
        Span::raw(") - "),
        Span::styled("Enter", Style::default().fg(Color::Magenta)),
        Span::raw(": view, "),
        Span::styled("/", Style::default().fg(Color::Magenta)),
        Span::raw(": filter, "),
        Span::styled("↑↓", Style::default().fg(Color::Magenta)),
        Span::raw(": nav, "),
        Span::styled("r", Style::default().fg(Color::Magenta)),
        Span::raw(": refresh, "),
        Span::styled("d", Style::default().fg(Color::Magenta)),
        Span::raw(": del, "),
        Span::styled("e", Style::default().fg(Color::Magenta)),
        Span::raw("/"),
        Span::styled("c", Style::default().fg(Color::Magenta)),
        Span::raw(": export"),
    ]);
    lines.push(Line::from(header_spans));

    // Show filter input or current filter
    if state.history_filter_editing {
        lines.push(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Cyan)),
            Span::styled(&state.history_filter, Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::White)), // cursor
            Span::styled(
                "  (Enter to apply, Esc to cancel)",
                Style::default().fg(Color::Gray),
            ),
        ]));
    } else if !state.history_filter.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Cyan)),
            Span::styled(&state.history_filter, Style::default().fg(Color::Yellow)),
            Span::styled("  (Esc to clear)", Style::default().fg(Color::Gray)),
        ]));
    }

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

    // Add column headers (left-aligned, matching data column widths exactly)
    lines.push(Line::from(vec![
        Span::styled("#    ", Style::default().fg(Color::Gray)), // 5 chars
        Span::styled(
            "Timestamp                   ",
            Style::default().fg(Color::Gray),
        ), // 28 chars
        Span::styled("DL        ", Style::default().fg(Color::Green)), // 10 chars
        Span::styled("UL        ", Style::default().fg(Color::Cyan)), // 10 chars
        Span::styled("Ping      ", Style::default().fg(Color::Gray)), // 10 chars
        Span::styled("Loss     ", Style::default().fg(Color::Yellow)), // 9 chars
        Span::styled("Interface    ", Style::default().fg(Color::Blue)), // 13 chars
        Span::styled("Network", Style::default().fg(Color::Magenta)),
    ]));

    // Clamp selection to filtered history bounds
    let effective_selected = state
        .history_selected
        .min(filtered_history.len().saturating_sub(1));

    // Apply scroll offset and take only visible items
    // Auto-adjust scroll to keep selected item visible
    let scroll_offset = {
        let mut offset = state
            .history_scroll_offset
            .min(filtered_history.len().saturating_sub(1));
        // Ensure selected item is visible
        if effective_selected < offset {
            offset = effective_selected;
        } else if effective_selected >= offset + max_items {
            offset = effective_selected.saturating_sub(max_items - 1);
        }
        offset
    };

    let history_display: Vec<_> = filtered_history
        .iter()
        .skip(scroll_offset)
        .take(max_items)
        .collect();

    for (display_idx, r) in history_display.iter().enumerate() {
        // Calculate actual index in filtered view (accounting for scroll offset)
        let filtered_idx = scroll_offset + display_idx;
        let is_selected = state.tab == 1 && filtered_idx == effective_selected;

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
        let line_num = filtered_idx + 1;

        // Format interface and network names, truncating if needed
        let interface = r.interface_name.as_deref().unwrap_or("-");
        let network = r
            .network_name
            .as_deref()
            .or_else(|| r.interface_name.as_deref())
            .unwrap_or("-");
        let history_loss_text = r
            .experimental_udp
            .as_ref()
            .map(|u| format!("{:.1}%", u.latency.loss * 100.0))
            .unwrap_or_else(|| "-".to_string());

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<4}{}", line_num, if is_selected { ">" } else { " " }), // 5 chars total
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::styled(
                format!("{:<28}", timestamp_str), // 28 chars
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::styled(
                format!("{:<10.1}", r.download.mbps), // 10 chars
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
            Span::styled(
                format!("{:<10.1}", r.upload.mbps), // 10 chars
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Cyan)
                },
            ),
            Span::styled(
                format!("{:<10.1}", r.idle_latency.median_ms.unwrap_or(f64::NAN)), // 10 chars
                if is_selected { style } else { Style::default() },
            ),
            Span::styled(
                format!("{:<9}", history_loss_text), // 9 chars
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
            Span::styled(
                format!("{:<13}", interface), // 13 chars
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Blue)
                },
            ),
            Span::styled(
                network.to_string(),
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
    } else if filtered_history.is_empty() && !state.history_filter.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "No results match filter: ",
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(&state.history_filter, Style::default().fg(Color::White)),
        ]));
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
                let mut char_count = 0;
                let mut last_sep_pos = None;
                let mut break_pos = 0;

                for (idx, ch) in remaining.char_indices() {
                    if char_count >= line_width {
                        break;
                    }
                    if ch == '/' || ch == '\\' {
                        last_sep_pos = Some(idx);
                    }
                    break_pos = idx + ch.len_utf8();
                    char_count += 1;
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

pub fn draw_history_detail(area: Rect, f: &mut Frame, state: &UiState) {
    let mut lines: Vec<Line> = Vec::new();

    // Get the filtered history to find the correct selected item
    let filter_lower = state.history_filter.to_lowercase();
    let filtered_history: Vec<&RunResult> = if state.history_filter.is_empty() {
        state.history.iter().collect()
    } else {
        state
            .history
            .iter()
            .filter(|r| {
                let matches_field = |opt: &Option<String>| {
                    opt.as_ref()
                        .map(|s| s.to_lowercase().contains(&filter_lower))
                        .unwrap_or(false)
                };
                matches_field(&r.network_name)
                    || matches_field(&r.interface_name)
                    || matches_field(&r.as_org)
                    || matches_field(&r.colo)
                    || matches_field(&r.comments)
            })
            .collect()
    };

    let effective_selected = state
        .history_selected
        .min(filtered_history.len().saturating_sub(1));

    if let Some(result) = filtered_history.get(effective_selected) {
        // Header with navigation help
        lines.push(Line::from(vec![
            Span::styled("JSON Detail View", Style::default().fg(Color::Cyan)),
            Span::raw(" - "),
            Span::styled("Esc/Enter/q", Style::default().fg(Color::Magenta)),
            Span::raw(": back, "),
            Span::styled("↑↓/jk", Style::default().fg(Color::Magenta)),
            Span::raw(": scroll, "),
            Span::styled("PgUp/PgDn", Style::default().fg(Color::Magenta)),
            Span::raw(": fast scroll"),
        ]));
        lines.push(Line::from(""));

        // Serialize the result to pretty JSON
        let json_str = serde_json::to_string_pretty(result).unwrap_or_else(|e| format!("Error serializing JSON: {}", e));

        // Split JSON into lines for display
        let json_lines: Vec<&str> = json_str.lines().collect();
        let total_lines = json_lines.len();

        // Calculate available height for JSON content (subtract header lines and borders)
        let available_height = (area.height as usize).saturating_sub(5);

        // Clamp scroll offset locally (don't mutate state in draw)
        let max_scroll = total_lines.saturating_sub(available_height);
        let scroll_offset = state.history_detail_scroll.min(max_scroll);

        // Show scroll position
        let scroll_info = if total_lines > available_height {
            format!(
                " (lines {}-{} of {})",
                scroll_offset + 1,
                (scroll_offset + available_height).min(total_lines),
                total_lines
            )
        } else {
            String::new()
        };
        lines.push(Line::from(vec![
            Span::styled(
                result.network_name.as_deref().unwrap_or("Unknown Network"),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" - "),
            Span::styled(&result.timestamp_utc, Style::default().fg(Color::Gray)),
            Span::styled(scroll_info, Style::default().fg(Color::Gray)),
        ]));
        lines.push(Line::from(""));

        // Add JSON lines with syntax highlighting
        for line in json_lines.iter().skip(scroll_offset).take(available_height) {
            // Simple syntax highlighting
            let styled_line = if line.trim().starts_with('"') && line.contains(':') {
                // Key-value line
                if let Some(colon_pos) = line.find(':') {
                    let (key_part, value_part) = line.split_at(colon_pos + 1);
                    Line::from(vec![
                        Span::styled(key_part.to_string(), Style::default().fg(Color::Cyan)),
                        Span::styled(value_part.to_string(), Style::default().fg(Color::White)),
                    ])
                } else {
                    Line::from(Span::raw(line.to_string()))
                }
            } else if line.trim().starts_with('}')
                || line.trim().starts_with(']')
                || line.trim().starts_with('{')
                || line.trim().starts_with('[')
            {
                // Brackets
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Gray),
                ))
            } else {
                Line::from(Span::raw(line.to_string()))
            };
            lines.push(styled_line);
        }
    } else {
        lines.push(Line::from("No item selected."));
    }

    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("History - JSON Detail"),
    );
    f.render_widget(p, area);
}
