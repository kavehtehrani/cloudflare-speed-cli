use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Color,
    style::Style,
    text::{Line, Span},
    widgets::canvas::Line as CanvasLine,
    widgets::{canvas::Canvas, Block, Borders, Chart, Dataset, Paragraph},
    Frame,
};

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
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
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
        if let Some(metrics) = crate::metrics::compute_latency_metrics(samples) {
            let metrics_text = render_metrics_text(metrics, color);
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

/// Helper function to render metrics text (avg, med, p25, p75)
fn render_metrics_text<'a>(metrics: (f64, f64, f64, f64), color: Option<Color>) -> Line<'a> {
    let (mean_val, median_val, p25_val, p75_val) = metrics;
    if let Some(c) = color {
        Line::from(vec![
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
        ])
    } else {
        Line::from(format!(
            "avg {:.0} med {:.0} p25 {:.0} p75 {:.0}",
            mean_val, median_val, p25_val, p75_val
        ))
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

    // Render metrics in bottom area
    if let Some(metrics) = metrics {
        let metrics_text = render_metrics_text(metrics, Some(color));
        f.render_widget(
            Paragraph::new(metrics_text).alignment(Alignment::Center),
            chart_metrics[1],
        );
    }

    // Render the border with title around the whole area
    let block = Block::default().borders(Borders::ALL).title(title);
    f.render_widget(block, area);
}
