use ratatui::{
    layout::Rect,
    style::Color,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn draw_help(area: Rect, f: &mut Frame) {
    let p = Paragraph::new(vec![
        Line::from("Keybinds:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("q", Style::default().fg(Color::Magenta)),
            Span::raw(" / "),
            Span::styled("Ctrl-C", Style::default().fg(Color::Magenta)),
            Span::raw("  Quit"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("r", Style::default().fg(Color::Magenta)),
            Span::raw("           Rerun"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("p", Style::default().fg(Color::Magenta)),
            Span::raw("           Pause/Resume"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("s", Style::default().fg(Color::Magenta)),
            Span::raw("           Save JSON"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("a", Style::default().fg(Color::Magenta)),
            Span::raw("           Toggle auto-save"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("tab", Style::default().fg(Color::Magenta)),
            Span::raw("         Switch tabs"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("?", Style::default().fg(Color::Magenta)),
            Span::raw("           Show this help"),
        ]),
        Line::from(""),
        Line::from("History tab:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("↑/↓", Style::default().fg(Color::Magenta)),
            Span::raw(" or "),
            Span::styled("j/k", Style::default().fg(Color::Magenta)),
            Span::raw("  Navigate"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("e", Style::default().fg(Color::Magenta)),
            Span::raw("           Export selected as JSON"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("c", Style::default().fg(Color::Magenta)),
            Span::raw("           Export selected as CSV"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("y", Style::default().fg(Color::Magenta)),
            Span::raw("           Copy exported path to clipboard"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("d", Style::default().fg(Color::Magenta)),
            Span::raw("           Delete selected"),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("r", Style::default().fg(Color::Magenta)),
            Span::raw("           Refresh history"),
        ]),
        Line::from(""),
        Line::from("Repository:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "https://github.com/kavehtehrani/cloudflare-speed-cli",
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(p, area);
}
