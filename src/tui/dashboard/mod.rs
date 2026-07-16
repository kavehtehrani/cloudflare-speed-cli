use ratatui::{
    style::Color,
    style::Style,
    text::{Line, Span},
    Frame,
};

use super::state::{UiState, REDACTED_PLACEHOLDER};
use std::borrow::Cow;

/// Returns `REDACTED_PLACEHOLDER` when `hide` is true, otherwise `value` or `"-"` for `None`.
/// Used to conceal identifying network info (IP, MAC, SSID, ISP, location) for
/// screenshot/demo sharing without altering stored history.
pub(super) fn show_or_redact<'a>(value: Option<&'a str>, hide: bool) -> &'a str {
    if hide {
        REDACTED_PLACEHOLDER
    } else {
        value.unwrap_or("-")
    }
}

/// Display value for an "External IPv4"/"External IPv6" panel row: the probe
/// result when available, otherwise the meta client IP. The fallback must stay
/// family-checked — a `-4`/`-6` run skips the other family's probe, and the
/// client IP would otherwise show up on the wrong row.
pub(super) fn external_ip_for_family<'a>(
    external: Option<&'a str>,
    meta_ip: Option<&'a str>,
    want_ipv4: bool,
) -> Option<&'a str> {
    external.or_else(|| {
        meta_ip.filter(|ip| {
            ip.parse::<std::net::IpAddr>()
                .map(|addr| addr.is_ipv4() == want_ipv4)
                .unwrap_or(false)
        })
    })
}

/// Redacts identifying info from a single Test Activity log line.
///
/// Strategy: substring-replace values the engine already populated into `state`
/// (covers IP/ASN/ISP/server/colo/network/interface). Then pattern-replace any
/// remaining IPv4 dotted-quads (covers traceroute hops and IP-comparison IPs
/// that aren't in `state`). Cheap no-op when redaction is off — returns the
/// borrowed input without allocating.
pub(super) fn redact_log_line<'a>(line: &'a str, state: &UiState) -> Cow<'a, str> {
    if !state.hide_network_info {
        return Cow::Borrowed(line);
    }

    let mut needles: Vec<&str> = [
        state.network.external_ipv6.as_deref(),
        state.network.external_ipv4.as_deref(),
        state.network.ip.as_deref(),
        state.network.interface_mac.as_deref(),
        state.network.as_org.as_deref(),
        state.network.network_name.as_deref(),
        state.network.interface_name.as_deref(),
        state.network.server.as_deref(),
        state.network.colo.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|v| v.len() >= 2 && *v != "-")
    .collect();
    // Longest-first so e.g. "fe80::1234" isn't half-replaced by a shorter
    // "1234" needle that snuck in.
    needles.sort_by_key(|v| std::cmp::Reverse(v.len()));

    let mut s = line.to_string();
    for needle in needles {
        s = replace_token(&s, needle, REDACTED_PLACEHOLDER);
    }
    s = redact_ipv4_in(&s);
    Cow::Owned(s)
}

/// `haystack.replace(needle, replacement)` but only at alphanumeric word
/// boundaries, so an ASN like `13335` doesn't smear into a throughput value
/// like `13335.7 Mbps`.
pub(super) fn replace_token(haystack: &str, needle: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    for (i, _) in haystack.match_indices(needle) {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + needle.len()..].chars().next();
        let is_boundary = |c: Option<char>| match c {
            None => true,
            Some(c) => !c.is_alphanumeric(),
        };
        if is_boundary(before) && is_boundary(after) {
            out.push_str(&haystack[last..i]);
            out.push_str(replacement);
            last = i + needle.len();
        }
    }
    out.push_str(&haystack[last..]);
    out
}

/// Replaces every IPv4 dotted-quad in `s` with `REDACTED_PLACEHOLDER`.
/// Conservative: requires four 0–255 octets and rejects sequences that are part
/// of a longer dotted/digit run (so `1.23ms` and `1.2.3.4.5` don't match).
pub(super) fn redact_ipv4_in(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            if let Some(end) = ipv4_match_end(bytes, i) {
                out.push_str(REDACTED_PLACEHOLDER);
                i = end;
                continue;
            }
        }
        let cp_end = next_utf8_char_end(bytes, i);
        out.push_str(std::str::from_utf8(&bytes[i..cp_end]).unwrap_or(""));
        i = cp_end;
    }
    out
}

pub(super) fn ipv4_match_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    for octet_idx in 0..4 {
        if octet_idx > 0 {
            if bytes.get(i).copied() != Some(b'.') {
                return None;
            }
            i += 1;
        }
        let octet_start = i;
        while i < bytes.len() && (i - octet_start) < 3 && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == octet_start {
            return None;
        }
        let octet: u32 = std::str::from_utf8(&bytes[octet_start..i])
            .ok()?
            .parse()
            .ok()?;
        if octet > 255 {
            return None;
        }
    }
    // Reject when extending into a longer number / 5th octet so we don't
    // grab the leading 4 octets of `1.2.3.4.5`.
    if let Some(&next) = bytes.get(i) {
        if next.is_ascii_digit() {
            return None;
        }
        if next == b'.' {
            if let Some(&peek) = bytes.get(i + 1) {
                if peek.is_ascii_digit() {
                    return None;
                }
            }
        }
    }
    Some(i)
}

pub(super) fn next_utf8_char_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
        i += 1;
    }
    i
}

/// Helper function to get the maximum y value from a series of points
pub fn max_y(points: &[(f64, f64)]) -> f64 {
    points.iter().map(|(_, y)| *y).fold(0.0, |a, b| a.max(b))
}

pub(super) fn udp_split_bar(sent: u64, received: u64, width: usize) -> Line<'static> {
    let safe_sent = sent.max(1);
    let safe_received = received.min(safe_sent);
    let lost = safe_sent.saturating_sub(safe_received);
    // Ensure any loss shows at least one red segment
    let lost_units = if lost > 0 {
        (width as f64 * lost as f64 / safe_sent as f64).ceil().max(1.0) as usize
    } else {
        0
    };
    let ok_units = width.saturating_sub(lost_units);

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

/// Get color for quality label based on loss severity
pub(super) fn quality_label_color(label: &str) -> Color {
    match label {
        "Excellent" | "Good" => Color::Green,
        "Acceptable" => Color::Yellow,
        "Poor" => Color::Magenta,
        "Bad" => Color::Red,
        _ => Color::Gray,
    }
}


mod compact;
mod full;

use ratatui::layout::Rect;

pub fn draw_dashboard(area: Rect, f: &mut Frame, state: &UiState) {
    // Small terminal: keep the compact dashboard (gauges + sparklines).
    // Large terminal: show full charts (like the website) alongside the live cards.
    // Total fixed-height rows in the full dashboard:
    //   13 (throughput) + 10 (latency) + 3 (UDP) + 5 (status) = 31
    // We need at least ~3 rows for the Network Info / Test Activity row,
    // so fall back to the compact layout below 34 rows. Otherwise the
    // Status panel gets clipped at the bottom.
    if area.height < 34 {
        return compact::draw_dashboard_compact(area, f, state);
    }
    full::draw_dashboard_full(area, f, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_ip_prefers_probe_result() {
        assert_eq!(
            external_ip_for_family(Some("1.2.3.4"), Some("2001:db8::1"), true),
            Some("1.2.3.4")
        );
        assert_eq!(
            external_ip_for_family(Some("2001:db8::1"), Some("1.2.3.4"), false),
            Some("2001:db8::1")
        );
    }

    #[test]
    fn external_ip_falls_back_to_meta_ip_of_same_family() {
        assert_eq!(
            external_ip_for_family(None, Some("1.2.3.4"), true),
            Some("1.2.3.4")
        );
        assert_eq!(
            external_ip_for_family(None, Some("2001:db8::1"), false),
            Some("2001:db8::1")
        );
    }

    #[test]
    fn external_ip_ignores_meta_ip_of_other_family() {
        // --ipv6-only: the v4 probe is skipped and the meta client IP is the
        // IPv6 address; it must not appear on the IPv4 row (issue #49).
        assert_eq!(external_ip_for_family(None, Some("2001:db8::1"), true), None);
        assert_eq!(external_ip_for_family(None, Some("1.2.3.4"), false), None);
    }

    #[test]
    fn external_ip_handles_missing_or_invalid_meta_ip() {
        assert_eq!(external_ip_for_family(None, None, true), None);
        assert_eq!(external_ip_for_family(None, Some("not-an-ip"), true), None);
    }
}
