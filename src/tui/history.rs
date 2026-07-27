use crate::model::RunResult;
use ratatui::{
    layout::{Margin, Rect},
    style::Color,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use super::state::UiState;

const NETWORK_COL_WIDTH: usize = 18;
const MIN_COMMENT_COL_WIDTH: usize = 8;

/// Flattens whitespace (newlines, tabs, multiple spaces) to single spaces and
/// truncates to `max_chars`, appending `…` if truncated. Returns at most `max_chars`
/// chars wide. If `max_chars == 0`, returns empty string.
fn truncate_for_cell(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    // Flatten whitespace
    let flat: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let total = flat.chars().count();
    if total <= max_chars {
        flat
    } else if max_chars == 1 {
        "\u{2026}".to_string()
    } else {
        let take = max_chars.saturating_sub(1);
        let truncated: String = flat.chars().take(take).collect();
        format!("{}\u{2026}", truncated)
    }
}

const MENU_WIDTH: u16 = 32;
const MENU_BORDER_OVERHEAD: u16 = 2;
const MENU_FOOTER_LINES: u16 = 2;

pub const MENU_ITEM_VIEW: usize = 0;
pub const MENU_ITEM_EDIT_COMMENT: usize = 1;
pub const MENU_ITEM_EXPORT_JSON: usize = 2;
pub const MENU_ITEM_EXPORT_CSV: usize = 3;
pub const MENU_ITEM_DELETE: usize = 4;
pub const MENU_ITEM_COUNT: usize = 5;

const MENU_FOOTER_LINE_1: &str = "\u{2191}\u{2193}: nav  Enter: select";
const MENU_FOOTER_LINE_2: &str = "Esc: close";

/// Returns the labels for the actions menu, derived from current state.
/// The label for the comment item is "Add comment" or "Edit comment"
/// depending on whether the selected run already has a comment.
pub fn menu_labels(state: &UiState) -> [&'static str; MENU_ITEM_COUNT] {
    let has_comment = state
        .history
        .selected_run_index()
        .and_then(|i| state.history.runs[i].comments.as_deref())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let comment_label = if has_comment {
        "Edit comment"
    } else {
        "Add comment"
    };
    [
        "View detail",
        comment_label,
        "Export as JSON",
        "Export as CSV",
        "Delete",
    ]
}

pub fn show_history(area: Rect, f: &mut Frame, state: &mut UiState) {
    let mut lines: Vec<Line> = Vec::new();

    // Same filtered view the action handlers resolve through (selected_run_index),
    // so the highlighted row and the acted-on run can never diverge.
    let filtered_history: Vec<&RunResult> = state
        .history
        .filtered_indices()
        .into_iter()
        .map(|i| &state.history.runs[i])
        .collect();

    // Calculate how many items can fit in the available area
    // Subtract 4 for: controls line, filter line (optional), column headers, borders
    let max_items = (area.height as usize).saturating_sub(4);

    // Show total count and current position
    let total_count = filtered_history.len();
    let current_pos = if total_count > 0 {
        state.history.selected.min(total_count.saturating_sub(1)) + 1
    } else {
        0
    };

    // Build header line with controls
    let mut header_spans = vec![Span::raw(format!(
        "History ({}/{}",
        current_pos, total_count
    ))];
    if !state.history.filter.is_empty() {
        header_spans.push(Span::styled(
            format!(" filtered from {}", state.history.runs.len()),
            Style::default().fg(Color::Yellow),
        ));
    }
    if total_count > max_items {
        header_spans.push(Span::raw(format!(", showing {}", max_items)));
    }
    header_spans.extend(vec![
        Span::raw(") - "),
        Span::styled("Enter", Style::default().fg(Color::Magenta)),
        Span::raw(": detail, "),
        Span::styled("Space", Style::default().fg(Color::Magenta)),
        Span::raw(": actions, "),
        Span::styled("/", Style::default().fg(Color::Magenta)),
        Span::raw(": filter, "),
        Span::styled("r", Style::default().fg(Color::Magenta)),
        Span::raw(": refresh, "),
        Span::styled("\u{2191}\u{2193}", Style::default().fg(Color::Magenta)),
        Span::raw("/"),
        Span::styled("PgUp/Dn", Style::default().fg(Color::Magenta)),
        Span::raw(": nav"),
    ]);
    lines.push(Line::from(header_spans));

    // Show filter input or current filter
    if state.history.filter_editing {
        lines.push(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Cyan)),
            Span::styled(&state.history.filter, Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::White)), // cursor
            Span::styled(
                "  (Enter to apply, Esc to cancel)",
                Style::default().fg(Color::Gray),
            ),
        ]));
    } else if !state.history.filter.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Cyan)),
            Span::styled(&state.history.filter, Style::default().fg(Color::Yellow)),
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
    // Column widths: # 5, Timestamp 28, DL 10, UL 10, Ping 10, Loss 9, Bloat 6, Stab 6, Interface 13, Network 18, Comment fills remainder
    let fixed_col_width: u16 = 5 + 28 + 10 + 10 + 10 + 9 + 6 + 6 + 13 + NETWORK_COL_WIDTH as u16;
    let comment_col_width = area
        .width
        .saturating_sub(fixed_col_width + 2 /* borders */)
        .max(MIN_COMMENT_COL_WIDTH as u16) as usize;
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
        Span::styled("Bloat ", Style::default().fg(Color::LightMagenta)), // 6 chars
        Span::styled("Stab  ", Style::default().fg(Color::LightMagenta)), // 6 chars
        Span::styled("Interface    ", Style::default().fg(Color::Blue)), // 13 chars
        Span::styled(
            format!("{:<width$}", "Network", width = NETWORK_COL_WIDTH),
            Style::default().fg(Color::Magenta),
        ),
        Span::styled("Comment", Style::default().fg(Color::Gray)),
    ]));

    // Clamp selection to filtered history bounds
    let effective_selected = state
        .history
        .selected
        .min(filtered_history.len().saturating_sub(1));

    // Auto-adjust scroll to keep selected item visible
    // Only scroll when selection goes off-screen (not before)
    let mut offset = state
        .history
        .scroll_offset
        .min(filtered_history.len().saturating_sub(1));
    if effective_selected < offset {
        offset = effective_selected;
    } else if max_items > 0 && effective_selected >= offset + max_items {
        offset = effective_selected - max_items + 1;
    }
    state.history.scroll_offset = offset;
    let scroll_offset = offset;

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
        let interface = if state.hide_network_info {
            crate::tui::state::REDACTED_PLACEHOLDER
        } else {
            r.interface_name.as_deref().unwrap_or("-")
        };
        let network = if state.hide_network_info {
            crate::tui::state::REDACTED_PLACEHOLDER
        } else {
            r.network_name
                .as_deref()
                .or(r.interface_name.as_deref())
                .unwrap_or("-")
        };
        let history_loss_text = r
            .experimental_udp
            .as_ref()
            .map(|u| format!("{:.1}%", u.latency.loss * 100.0))
            .unwrap_or_else(|| "-".to_string());

        let (bloat_letter, stab_letter) = match &r.connection_quality {
            Some(cq) => (cq.bufferbloat_grade.as_str(), cq.stability_grade.as_str()),
            None => ("-", "-"),
        };

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
            // Width 6 to match the headers exactly.
            Span::styled(
                format!("{:<6}", bloat_letter),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::LightMagenta)
                },
            ),
            Span::styled(
                format!("{:<6}", stab_letter),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::LightMagenta)
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
                format!(
                    "{:<width$}",
                    truncate_for_cell(network, NETWORK_COL_WIDTH),
                    width = NETWORK_COL_WIDTH
                ),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Magenta)
                },
            ),
            Span::styled(
                truncate_for_cell(r.comments.as_deref().unwrap_or(""), comment_col_width),
                if is_selected {
                    style
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ]));
    }

    if state.history.runs.is_empty() {
        lines.push(Line::from("No history available."));
    } else if filtered_history.is_empty() && !state.history.filter.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "No results match filter: ",
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(&state.history.filter, Style::default().fg(Color::White)),
        ]));
    }

    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("History"));
    f.render_widget(p, area);

    // Render scrollbar on the right edge if there are more items than visible
    if total_count > max_items {
        let mut scrollbar_state =
            ScrollbarState::new(total_count.saturating_sub(max_items)).position(scroll_offset);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

pub fn draw_history_detail(area: Rect, f: &mut Frame, state: &mut UiState) {
    // Header with run identity and navigation help
    let mut header_lines: Vec<Line> = Vec::new();

    let selected_run = state.history.selected_run_index();

    header_lines.push(Line::from(vec![
        Span::styled("JSON Detail", Style::default().fg(Color::Cyan)),
        Span::raw(" - "),
        Span::styled("Esc/q", Style::default().fg(Color::Magenta)),
        Span::raw(": back, "),
        Span::styled("/", Style::default().fg(Color::Magenta)),
        Span::raw(": search (regex), "),
        Span::styled("n", Style::default().fg(Color::Magenta)),
        Span::raw("/"),
        Span::styled("N", Style::default().fg(Color::Magenta)),
        Span::raw(": next/prev, "),
        Span::styled(
            "\u{2191}\u{2193}/jk/PgUp/PgDn",
            Style::default().fg(Color::Magenta),
        ),
        Span::raw(": scroll"),
    ]));

    if let Some(result) = selected_run.map(|i| &state.history.runs[i]) {
        let net_label = if state.hide_network_info {
            crate::tui::state::REDACTED_PLACEHOLDER
        } else {
            result.network_name.as_deref().unwrap_or("Unknown Network")
        };
        header_lines.push(Line::from(vec![
            Span::styled(net_label, Style::default().fg(Color::Yellow)),
            Span::raw(" - "),
            Span::styled(&result.timestamp_utc, Style::default().fg(Color::Gray)),
        ]));
    }

    // Search status line: shows current pattern (or input cursor while editing)
    if state.history.detail_search_editing {
        let mut spans = vec![
            Span::styled("Search: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                &state.history.detail_search,
                Style::default().fg(Color::White),
            ),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::styled(
                "  (Enter to confirm, Esc to cancel)",
                Style::default().fg(Color::Gray),
            ),
        ];
        if let Some(ref err) = state.history.detail_search_error {
            spans.push(Span::styled(
                format!("  [regex error: {}]", err),
                Style::default().fg(Color::Red),
            ));
        }
        header_lines.push(Line::from(spans));
    } else if !state.history.detail_search.is_empty() {
        header_lines.push(Line::from(vec![
            Span::styled("Search: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                &state.history.detail_search,
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("  (Esc to clear)", Style::default().fg(Color::Gray)),
        ]));
    }

    let header_height = header_lines.len() as u16;

    // Outer block
    let outer = Block::default()
        .borders(Borders::ALL)
        .title("History - JSON Detail");
    let inner_area = outer.inner(area);
    f.render_widget(outer, area);

    // Render header at the top of the inner area
    let header_area = Rect {
        x: inner_area.x,
        y: inner_area.y,
        width: inner_area.width,
        height: header_height.min(inner_area.height),
    };
    let header_paragraph = Paragraph::new(header_lines);
    f.render_widget(header_paragraph, header_area);

    // Render the textarea below the header
    let body_area = Rect {
        x: inner_area.x,
        y: inner_area.y + header_area.height,
        width: inner_area.width,
        height: inner_area.height.saturating_sub(header_area.height),
    };
    if body_area.height > 0 {
        f.render_widget(&state.history.detail_textarea, body_area);
    }
}

pub fn draw_history_menu(area: Rect, f: &mut Frame, state: &UiState) {
    let labels = menu_labels(state);
    let item_count = labels.len() as u16;
    let inner_height = item_count + 1 /* spacer */ + MENU_FOOTER_LINES;
    let modal_height = inner_height + MENU_BORDER_OVERHEAD;
    let modal_width = MENU_WIDTH;

    // Clamp if the available area is smaller than the modal.
    let modal_width = modal_width.min(area.width);
    let modal_height = modal_height.min(area.height);

    let x = area.x + area.width.saturating_sub(modal_width) / 2;
    let y = area.y + area.height.saturating_sub(modal_height) / 2;
    let modal_area = Rect {
        x,
        y,
        width: modal_width,
        height: modal_height,
    };

    let mut lines: Vec<Line> = Vec::with_capacity(labels.len() + 3);

    for (idx, label) in labels.iter().enumerate() {
        let is_selected = state.history.menu_selected == idx;

        let marker = if is_selected { "> " } else { "  " };
        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(ratatui::style::Modifier::REVERSED)
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![Span::styled(
            format!(" {}{}", marker, label),
            style,
        )]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        format!("  {}", MENU_FOOTER_LINE_1),
        Style::default().fg(Color::Gray),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!("  {}", MENU_FOOTER_LINE_2),
        Style::default().fg(Color::Gray),
    )]));

    f.render_widget(Clear, modal_area);
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Actions"));
    f.render_widget(p, modal_area);
}

const COMMENT_MODAL_MIN_WIDTH: u16 = 40;
const COMMENT_MODAL_MAX_WIDTH: u16 = 78;
const COMMENT_MODAL_BORDER_OVERHEAD: u16 = 2;
const COMMENT_MODAL_HINT: &str = "Enter: save  Esc: cancel";
const COMMENT_MODAL_EDITOR_HEIGHT: u16 = 1;

pub fn draw_history_comment_modal(area: Rect, f: &mut Frame, state: &UiState) {
    let modal_width = COMMENT_MODAL_MAX_WIDTH
        .min(area.width)
        .max(COMMENT_MODAL_MIN_WIDTH.min(area.width));

    let footer_lines: u16 = 1;
    let inner_height = COMMENT_MODAL_EDITOR_HEIGHT + footer_lines;
    let modal_height = (inner_height + COMMENT_MODAL_BORDER_OVERHEAD).min(area.height);

    let x = area.x + area.width.saturating_sub(modal_width) / 2;
    let y = area.y + area.height.saturating_sub(modal_height) / 2;
    let modal_area = Rect {
        x,
        y,
        width: modal_width,
        height: modal_height,
    };

    f.render_widget(Clear, modal_area);
    let outer = Block::default().borders(Borders::ALL).title("Edit Comment");
    let inner_area = outer.inner(modal_area);
    f.render_widget(outer, modal_area);

    // Split inner area: editor on top, footer hint at bottom.
    let editor_height = inner_area
        .height
        .saturating_sub(footer_lines)
        .min(COMMENT_MODAL_EDITOR_HEIGHT);
    let editor_area = Rect {
        x: inner_area.x,
        y: inner_area.y,
        width: inner_area.width,
        height: editor_height,
    };
    let footer_area = Rect {
        x: inner_area.x,
        y: inner_area.y + editor_height,
        width: inner_area.width,
        height: inner_area.height.saturating_sub(editor_height),
    };

    f.render_widget(&state.history.comment_modal_textarea, editor_area);

    let hint = Paragraph::new(Line::from(vec![Span::styled(
        COMMENT_MODAL_HINT,
        Style::default().fg(Color::Gray),
    )]));
    f.render_widget(hint, footer_area);
}

const EXPORT_MODAL_MIN_WIDTH: u16 = 32;
const EXPORT_MODAL_MAX_WIDTH: u16 = 78;
const EXPORT_MODAL_HEADER_LINES: u16 = 2; // title + blank
const EXPORT_MODAL_FOOTER_LINES: u16 = 2; // blank + hint
const EXPORT_MODAL_BORDER_OVERHEAD: u16 = 2;
const EXPORT_MODAL_HINT_COPY: &str = "c: copy path  Esc/Enter: close";
const EXPORT_MODAL_HINT_COPIED: &str = "\u{2713} Copied  Esc/Enter: close";

/// Wraps a filesystem path into lines that each fit within `width` characters,
/// preferring to break at path separators when possible.
fn wrap_path(path: &str, width: u16) -> Vec<String> {
    let mut out = Vec::new();
    let width = width.max(1) as usize;
    let mut remaining = path;

    while !remaining.is_empty() {
        let remaining_chars = remaining.chars().count();
        if remaining_chars <= width {
            out.push(remaining.to_string());
            break;
        }

        let mut last_sep_pos = None;
        let mut break_pos = 0;
        for (char_count, (idx, ch)) in remaining.char_indices().enumerate() {
            if char_count >= width {
                break;
            }
            if ch == '/' || ch == '\\' {
                last_sep_pos = Some(idx);
            }
            break_pos = idx + ch.len_utf8();
        }

        let split_pos = match last_sep_pos {
            Some(sep_pos) if sep_pos > 0 => sep_pos + 1, // include separator
            _ => break_pos,
        };

        let (chunk, rest) = remaining.split_at(split_pos);
        out.push(chunk.to_string());
        remaining = rest;
    }

    if out.is_empty() {
        out.push(String::new());
    }
    out
}

pub fn draw_history_export_modal(area: Rect, f: &mut Frame, state: &UiState) {
    let Some(ref path) = state.history.export_modal_path else {
        return;
    };

    let modal_width = EXPORT_MODAL_MAX_WIDTH
        .min(area.width)
        .max(EXPORT_MODAL_MIN_WIDTH.min(area.width));
    let inner_width = modal_width.saturating_sub(EXPORT_MODAL_BORDER_OVERHEAD + 2); // -2 for left/right padding
    let path_lines = wrap_path(path, inner_width);

    let path_line_count = path_lines.len() as u16;
    let inner_height = EXPORT_MODAL_HEADER_LINES + path_line_count + EXPORT_MODAL_FOOTER_LINES;
    let modal_height = (inner_height + EXPORT_MODAL_BORDER_OVERHEAD).min(area.height);

    let x = area.x + area.width.saturating_sub(modal_width) / 2;
    let y = area.y + area.height.saturating_sub(modal_height) / 2;
    let modal_area = Rect {
        x,
        y,
        width: modal_width,
        height: modal_height,
    };

    let mut lines: Vec<Line> = Vec::with_capacity(path_lines.len() + 4);
    lines.push(Line::from(vec![Span::styled(
        " Exported to:",
        Style::default().fg(Color::Gray),
    )]));
    lines.push(Line::from(""));
    for chunk in path_lines {
        lines.push(Line::from(vec![Span::styled(
            format!(" {}", chunk),
            Style::default().fg(Color::Cyan),
        )]));
    }
    lines.push(Line::from(""));
    let hint = if state.history.export_modal_copied {
        EXPORT_MODAL_HINT_COPIED
    } else {
        EXPORT_MODAL_HINT_COPY
    };
    let hint_color = if state.history.export_modal_copied {
        Color::Green
    } else {
        Color::Gray
    };
    lines.push(Line::from(vec![Span::styled(
        format!(" {}", hint),
        Style::default().fg(hint_color),
    )]));

    f.render_widget(Clear, modal_area);
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Export"));
    f.render_widget(p, modal_area);
}
