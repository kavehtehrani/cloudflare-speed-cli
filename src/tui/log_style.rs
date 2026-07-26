//! Colorizes plain-text log lines from `format_event_lines` for the dashboard's
//! Test Activity panel. Pure formatting layer: input strings stay byte-identical
//! to what text mode prints, only the rendering picks up colour.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

const LABEL: Color = Color::Gray;
const NUMBER: Color = Color::Cyan;
const SPEED_DOWN: Color = Color::Green;
const SPEED_UP: Color = Color::Cyan;
const LATENCY: Color = Color::Yellow;
const PERCENT: Color = Color::Magenta;
const HEADER: Color = Color::Magenta;
const PATH: Color = Color::DarkGray;

/// Style a single log line for the Test Activity panel.
pub fn style_log_line(line: &str) -> Line<'static> {
    // Phase header: == Download ==, == Summary ==, etc.
    if line.starts_with("== ") && line.ends_with(" ==") && line.len() >= 6 {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ));
    }

    // Saved: <path> — emphasise the label, dim the path so the eye latches onto
    // the prefix instead of a long filesystem string.
    if let Some(rest) = line.strip_prefix("Saved: ") {
        return Line::from(vec![
            Span::styled(
                "Saved: ",
                Style::default().fg(SPEED_DOWN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(rest.to_string(), Style::default().fg(PATH)),
        ]);
    }
    if let Some(rest) = line.strip_prefix("Saved (verifying): ") {
        return Line::from(vec![
            Span::styled("Saved (verifying): ", Style::default().fg(LATENCY)),
            Span::styled(rest.to_string(), Style::default().fg(PATH)),
        ]);
    }

    // Pick Mbps colour by line context so Download/Upload stay visually distinct.
    let mbps_color = if line.starts_with("Upload") || line.starts_with("UL ") {
        SPEED_UP
    } else {
        SPEED_DOWN
    };

    Line::from(highlight_numbers(line, mbps_color))
}

/// Walk the string, emitting Spans for runs of text and numbers (with optional
/// units). Units recognised: `Mbps`, `ms`, `%` (either attached or after one
/// space).
///
/// A digit run is only treated as a standalone number when it isn't part of an
/// identifier — i.e. the preceding character isn't a letter or `_`, and the
/// trailing character isn't a letter (other than a recognised unit) or `_`.
/// This keeps tokens like `TLSv1_3`, `wlp4s0`, or `TELUS-HSIA-NVCRBC01` out of
/// the highlighter.
fn highlight_numbers(s: &str, mbps_color: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = s.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            // Identifier continuation: previous char in `buf` is a letter or
            // underscore. Absorb the digit run (incl. dots/underscores so we
            // keep "TLSv1_3" together) into the text buffer.
            if buf.chars().last().map(is_ident_char).unwrap_or(false) {
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    buf.push(chars[i]);
                    i += 1;
                }
                continue;
            }

            // Greedily read a number (digits and dots). IP addresses end up
            // captured as one "number" too; they get the plain-number colour.
            let mut num = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                num.push(chars[i]);
                i += 1;
            }
            // Trim a trailing dot ("p25 12." would be wrong) — push it back as text.
            let trailing_dot = num.ends_with('.');
            if trailing_dot {
                num.pop();
            }

            let rest: String = chars[i..].iter().collect();
            let unit = recognised_unit(&rest, mbps_color);

            // If the number is followed by an identifier char (letter or `_`)
            // and we didn't match a recognised unit, treat the whole thing as
            // part of an identifier — flush buf+num as plain text.
            let trailing_is_ident = chars.get(i).copied().map(is_ident_char).unwrap_or(false);
            if unit.is_none() && trailing_is_ident {
                buf.push_str(&num);
                if trailing_dot {
                    buf.push('.');
                }
                continue;
            }

            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    Style::default().fg(LABEL),
                ));
            }

            if let Some((unit_str, unit_len, color, with_space)) = unit {
                let combined = if with_space {
                    format!("{} {}", num, unit_str)
                } else {
                    format!("{}{}", num, unit_str)
                };
                spans.push(Span::styled(combined, Style::default().fg(color)));
                i += unit_len;
            } else {
                spans.push(Span::styled(num, Style::default().fg(NUMBER)));
            }

            if trailing_dot {
                buf.push('.');
            }
        } else {
            buf.push(chars[i]);
            i += 1;
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, Style::default().fg(LABEL)));
    }
    spans
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Returns `(unit_text, consumed_len, color, with_leading_space)` if `rest`
/// begins with a recognised unit. The unit must NOT be followed by another
/// identifier char, so things like `msX` or `Mbpsblah` don't match.
fn recognised_unit(rest: &str, mbps_color: Color) -> Option<(&'static str, usize, Color, bool)> {
    let cases: &[(&str, &str, usize, Color, bool)] = &[
        (" Mbps", "Mbps", 5, mbps_color, true),
        (" ms", "ms", 3, LATENCY, true),
        ("Mbps", "Mbps", 4, mbps_color, false),
        ("ms", "ms", 2, LATENCY, false),
        ("%", "%", 1, PERCENT, false),
    ];
    for (prefix, unit, len, color, with_space) in cases.iter() {
        if rest.starts_with(prefix)
            && !rest[prefix.len()..]
                .chars()
                .next()
                .map(is_ident_char)
                .unwrap_or(false)
        {
            return Some((*unit, *len, *color, *with_space));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(line: &str) -> String {
        // Concatenate the span contents to verify the colorizer preserves text.
        style_log_line(line)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn preserves_input_text() {
        for line in [
            "== Download ==",
            "Download: 282.34 Mbps",
            "Upload: 332.50 Mbps",
            "Idle latency: 83.1 ms",
            "DNS: 231.72ms",
            "TLS: handshake 16.60ms, TLSv1_3 TLS_AES_256_GCM_SHA384",
            "Packet loss probe: 50/100 recv 48 loss 4.0% (12.1ms)",
            "Saved: /home/u/run.json",
            " 1  192.168.1.1 1.2ms 1.3ms 1.4ms",
            "Traceroute to 1.1.1.1 completed (5 hops)",
            "External IPs: v4=1.2.3.4 v6=2001:db8::1",
            "IPv4: 1.2.3.4 - DL 282.34 Mbps, UL 332.50 Mbps, latency 12.3ms",
        ] {
            assert_eq!(rendered(line), line, "round-trip failed for: {line}");
        }
    }

    #[test]
    fn header_is_styled_as_single_span() {
        let line = style_log_line("== Summary ==");
        assert_eq!(line.spans.len(), 1);
    }

    /// Returns the styled portions (spans whose fg is NOT the plain text colour).
    /// Used to assert which substrings the highlighter actually picks out.
    fn highlighted_substrings(line: &str) -> Vec<String> {
        style_log_line(line)
            .spans
            .iter()
            .filter(|s| s.style.fg.is_some() && s.style.fg != Some(LABEL))
            .map(|s| s.content.to_string())
            .collect()
    }

    #[test]
    fn does_not_highlight_digits_inside_identifiers() {
        // Numbers embedded in TLS protocol/cipher names must stay un-coloured.
        let line = "TLS: handshake 16.60ms, TLSv1_3 TLS_AES_256_GCM_SHA384";
        let highlighted = highlighted_substrings(line);
        assert_eq!(highlighted, vec!["16.60ms".to_string()]);
    }

    #[test]
    fn does_not_highlight_digits_in_interface_or_ssid() {
        // Interface names (wlp4s0) and SSID-style strings (TELUS-HSIA-NVCRBC01)
        // contain digits that are part of identifiers — leave them alone.
        let line = "Interface wlp4s0 on TELUS-HSIA-NVCRBC01";
        assert_eq!(highlighted_substrings(line), Vec::<String>::new());
    }

    #[test]
    fn still_highlights_standalone_metrics() {
        let line = "Download: 282.34 Mbps";
        assert_eq!(
            highlighted_substrings(line),
            vec!["282.34 Mbps".to_string()]
        );

        let line = "Packet loss probe: 50/100 recv 48 loss 4.0% (12.1ms)";
        let hl = highlighted_substrings(line);
        assert!(hl.contains(&"4.0%".to_string()), "got {hl:?}");
        assert!(hl.contains(&"12.1ms".to_string()), "got {hl:?}");
    }
}
