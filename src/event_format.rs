use crate::model::{Phase, RunResult, TestEvent};
use std::path::Path;

/// Line emitted after a run result is auto-saved to disk. Used by both text
/// mode and the TUI so the two surfaces show identical text.
pub fn format_saved_line(path: &Path) -> String {
    format!("Saved: {}", path.display())
}

/// Returns the text-mode log lines for a given `TestEvent`.
///
/// Both the CLI text mode (`run_text`) and the TUI dashboard's Test Activity
/// panel render whatever this function returns, so a future event addition
/// only needs to be added here.
pub fn format_event_lines(ev: &TestEvent) -> Vec<String> {
    match ev {
        TestEvent::PhaseStarted { phase } => vec![format!("== {phase:?} ==")],

        TestEvent::ThroughputTick {
            phase, bps_instant, ..
        } => {
            if matches!(phase, Phase::Download | Phase::Upload) {
                let mbps = (bps_instant * 8.0) / 1_000_000.0;
                vec![format!("{phase:?}: {:.2} Mbps", mbps)]
            } else {
                Vec::new()
            }
        }

        TestEvent::LatencySample {
            phase,
            ok,
            rtt_ms,
            during,
        } => {
            if !ok {
                return Vec::new();
            }
            let Some(ms) = rtt_ms else { return Vec::new() };
            match (phase, during) {
                (Phase::IdleLatency, None) => vec![format!("Idle latency: {:.1} ms", ms)],
                _ => Vec::new(),
            }
        }

        TestEvent::Info { message } => vec![message.clone()],

        TestEvent::UdpLossProgress {
            sent,
            received,
            total,
            rtt_ms,
        } => {
            let loss_pct = if *sent == 0 {
                0.0
            } else {
                ((sent.saturating_sub(*received)) as f64) * 100.0 / *sent as f64
            };
            let rtt_display = rtt_ms
                .map(|v| format!("{:.1}ms", v))
                .unwrap_or_else(|| "timeout".to_string());
            vec![format!(
                "Packet loss probe: {}/{} recv {} loss {:.1}% ({})",
                sent, total, received, loss_pct, rtt_display
            )]
        }

        // MetaInfo is purely structured data consumed by the TUI; text mode
        // does not print it.
        TestEvent::MetaInfo { .. } => Vec::new(),

        TestEvent::DiagnosticDns { summary } => {
            vec![format!("DNS: {:.2}ms", summary.resolution_time_ms)]
        }

        TestEvent::DiagnosticTls { summary } => vec![format!(
            "TLS: handshake {:.2}ms, {} {}",
            summary.handshake_time_ms,
            summary.protocol_version.as_deref().unwrap_or("-"),
            summary.cipher_suite.as_deref().unwrap_or("-")
        )],

        TestEvent::DiagnosticIpComparison { comparison } => {
            let mut lines = Vec::new();
            if let Some(ref v4) = comparison.ipv4_result {
                if v4.available {
                    lines.push(format!(
                        "IPv4: {} - DL {:.2} Mbps, UL {:.2} Mbps, latency {:.1}ms",
                        v4.ip_address, v4.download_mbps, v4.upload_mbps, v4.latency_ms
                    ));
                } else {
                    lines.push(format!("IPv4: unavailable - {:?}", v4.error));
                }
            }
            if let Some(ref v6) = comparison.ipv6_result {
                if v6.available {
                    lines.push(format!(
                        "IPv6: {} - DL {:.2} Mbps, UL {:.2} Mbps, latency {:.1}ms",
                        v6.ip_address, v6.download_mbps, v6.upload_mbps, v6.latency_ms
                    ));
                } else {
                    lines.push(format!("IPv6: unavailable - {:?}", v6.error));
                }
            }
            lines
        }

        TestEvent::TracerouteHop { hop_number, hop } => {
            let addr = hop.ip_address.as_deref().unwrap_or("*");
            let rtts: Vec<String> = hop.rtt_ms.iter().map(|r| format!("{:.1}ms", r)).collect();
            let rtt_str = if rtts.is_empty() {
                "*".to_string()
            } else {
                rtts.join(" ")
            };
            vec![format!("{:>2}  {} {}", hop_number, addr, rtt_str)]
        }

        TestEvent::TracerouteComplete { summary } => vec![format!(
            "Traceroute to {} {} ({} hops)",
            summary.destination,
            if summary.completed {
                "completed"
            } else {
                "incomplete"
            },
            summary.hops.len()
        )],

        TestEvent::ExternalIps { ipv4, ipv6 } => {
            let v4 = ipv4.as_deref().unwrap_or("-");
            let v6 = ipv6.as_deref().unwrap_or("-");
            vec![format!("External IPs: v4={} v6={}", v4, v6)]
        }
    }
}

/// Pad the label with a trailing colon to 10 characters so "Download:"
/// and "Upload:" align in the output column.
const THROUGHPUT_LABEL_WIDTH: usize = 10;

fn fmt_throughput(label: &str, values: &[f64]) -> Option<String> {
    let m = crate::metrics::compute_sample_metrics(values)?;
    Some(format!(
        "{:<width$}avg {:.2} med {:.2} p25 {:.2} p75 {:.2} p95 {:.2} p99 {:.2}",
        format!("{}:", label),
        m.mean,
        m.median,
        m.p25,
        m.p75,
        m.p95,
        m.p99,
        width = THROUGHPUT_LABEL_WIDTH,
    ))
}

fn fmt_latency(
    label: &str,
    samples: &[f64],
    summary: &crate::model::LatencySummary,
) -> Option<String> {
    let m = crate::metrics::compute_sample_metrics(samples)?;
    let p95 = summary.p95_ms.unwrap_or(m.p95);
    let p99 = summary.p99_ms.unwrap_or(m.p99);
    Some(format!(
        "{}: avg {:.1} med {:.1} p25 {:.1} p75 {:.1} p95 {:.1} p99 {:.1} ms (loss {:.1}%, jitter {:.1} ms)",
        label,
        m.mean,
        m.median,
        m.p25,
        m.p75,
        p95,
        p99,
        summary.loss * 100.0,
        summary.jitter_ms.unwrap_or(f64::NAN)
    ))
}

fn y_values(points: &[(f64, f64)]) -> Vec<f64> {
    points.iter().map(|(_, y)| *y).collect()
}

/// Returns the end-of-run summary lines that text mode prints to stdout after
/// the engine completes. The TUI dashboard appends the same lines to its
/// activity panel so users see the final numbers in place of the throughput
/// tick spam.
///
/// The `== Summary ==` header is intentionally NOT emitted here — the engine
/// already fires `PhaseStarted { Phase::Summary }` which `format_event_lines`
/// renders as that header. Duplicating it here would print it twice.
pub fn format_result_summary(
    result: &RunResult,
    dl_points: &[(f64, f64)],
    ul_points: &[(f64, f64)],
    idle_lat_samples: &[f64],
    dl_lat_samples: &[f64],
    ul_lat_samples: &[f64],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    if let Some(meta) = result.meta.as_ref() {
        let extracted = crate::network::extract_metadata(meta);
        let ip = extracted.ip.as_deref().unwrap_or("-");
        let colo = extracted.colo.as_deref().unwrap_or("-");
        let asn = extracted.asn.as_deref().unwrap_or("-");
        let org = extracted.as_org.as_deref().unwrap_or("-");
        out.push(format!("IP/Colo/ASN: {ip} / {colo} / {asn} ({org})"));
    }
    if let Some(server) = result.server.as_deref() {
        out.push(format!("Server: {server}"));
    }
    if let Some(comments) = result.comments.as_deref() {
        if !comments.trim().is_empty() {
            out.push(format!("Comments: {}", comments));
        }
    }

    out.extend(fmt_throughput("Download", &y_values(dl_points)));
    out.extend(fmt_throughput("Upload", &y_values(ul_points)));
    out.extend(fmt_latency(
        "Idle latency",
        idle_lat_samples,
        &result.idle_latency,
    ));
    out.extend(fmt_latency(
        "Loaded latency (download)",
        dl_lat_samples,
        &result.loaded_latency_download,
    ));
    out.extend(fmt_latency(
        "Loaded latency (upload)",
        ul_lat_samples,
        &result.loaded_latency_upload,
    ));

    if let Some(ref exp) = result.experimental_udp {
        let mos_str = exp
            .mos
            .map(|m| format!("MOS {:.1}", m))
            .unwrap_or_else(|| "N/A".to_string());
        let jitter_str = exp
            .latency
            .jitter_ms
            .map(|j| format!("{:.1}ms", j))
            .unwrap_or_else(|| "-".to_string());
        out.push(format!(
            "UDP quality: {} ({}) | loss {:.1}% jitter {} reorder {:.1}% rtt {}ms",
            exp.quality_label,
            mos_str,
            exp.latency.loss * 100.0,
            jitter_str,
            exp.out_of_order_pct,
            exp.latency.median_ms.unwrap_or(f64::NAN)
        ));
    }

    if let Some(ref cq) = result.connection_quality {
        if let Some(ms) = cq.bufferbloat_ms {
            out.push(format!(
                "Bufferbloat: {} ({:.0}ms)",
                cq.bufferbloat_grade, ms
            ));
        }
        if let Some(cv) = cq.stability_cv_pct {
            let mut detail = format!("CV {:.1}%", cv);
            if let Some(dl) = cq.stability_cv_download_pct {
                detail.push_str(&format!(", DL {:.1}%", dl));
            }
            if let Some(ul) = cq.stability_cv_upload_pct {
                detail.push_str(&format!(", UL {:.1}%", ul));
            }
            out.push(format!("Stability: {} ({})", cq.stability_grade, detail));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{empty_run_result, ConnectionQuality};

    #[test]
    fn summary_omits_connection_quality_when_absent() {
        let r = empty_run_result();
        let out = format_result_summary(&r, &[], &[], &[], &[], &[]);
        assert!(!out.iter().any(|l| l.starts_with("Bufferbloat:")));
        assert!(!out.iter().any(|l| l.starts_with("Stability:")));
    }

    #[test]
    fn summary_includes_connection_quality_when_present() {
        let mut r = empty_run_result();
        r.connection_quality = Some(ConnectionQuality {
            bufferbloat_grade: "B".into(),
            bufferbloat_ms: Some(47.3),
            stability_grade: "A".into(),
            stability_cv_pct: Some(3.8),
            stability_cv_download_pct: Some(3.1),
            stability_cv_upload_pct: Some(3.8),
        });
        let out = format_result_summary(&r, &[], &[], &[], &[], &[]);
        assert!(out.iter().any(|l| l == "Bufferbloat: B (47ms)"));
        assert!(out
            .iter()
            .any(|l| l == "Stability: A (CV 3.8%, DL 3.1%, UL 3.8%)"));
    }

    #[test]
    fn summary_skips_per_direction_when_unavailable() {
        let mut r = empty_run_result();
        r.connection_quality = Some(ConnectionQuality {
            bufferbloat_grade: "-".into(),
            bufferbloat_ms: None,
            stability_grade: "A".into(),
            stability_cv_pct: Some(5.0),
            stability_cv_download_pct: None,
            stability_cv_upload_pct: Some(5.0),
        });
        let out = format_result_summary(&r, &[], &[], &[], &[], &[]);
        assert!(!out.iter().any(|l| l.starts_with("Bufferbloat:")));
        assert!(out.iter().any(|l| l == "Stability: A (CV 5.0%, UL 5.0%)"));
    }
}
