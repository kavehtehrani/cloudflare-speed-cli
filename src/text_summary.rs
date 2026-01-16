//! Text summary builder for CLI output.
//!
//! This module computes metrics and formats human-readable lines for text mode.

use crate::metrics;
use crate::model::RunResult;
use anyhow::{Context, Result};

/// Pre-formatted lines for text output.
pub(crate) struct TextSummary {
    pub lines: Vec<String>,
}

/// Build a text summary from enriched results and raw samples.
pub(crate) fn build_text_summary(
    enriched: &RunResult,
    dl_points: &[(f64, f64)],
    ul_points: &[(f64, f64)],
    idle_latency_samples: &[f64],
    loaded_dl_latency_samples: &[f64],
    loaded_ul_latency_samples: &[f64],
) -> Result<TextSummary> {
    let mut lines = Vec::new();

    if let Some(meta) = enriched.meta.as_ref() {
        let extracted = crate::network::extract_metadata(meta);
        let ip = extracted.ip.as_deref().unwrap_or("-");
        let colo = extracted.colo.as_deref().unwrap_or("-");
        let asn = extracted.asn.as_deref().unwrap_or("-");
        let org = extracted.as_org.as_deref().unwrap_or("-");
        lines.push(format!("IP/Colo/ASN: {ip} / {colo} / {asn} ({org})"));
    }
    if let Some(server) = enriched.server.as_deref() {
        lines.push(format!("Server: {server}"));
    }
    if let Some(comments) = enriched.comments.as_deref() {
        if !comments.trim().is_empty() {
            lines.push(format!("Comments: {}", comments));
        }
    }

    let dl_values: Vec<f64> = dl_points.iter().map(|(_, y)| *y).collect();
    let (dl_mean, dl_median, dl_p25, dl_p75) = metrics::compute_metrics(&dl_values)
        .context("insufficient download throughput data to compute metrics")?;
    lines.push(format!(
        "Download: avg {:.2} med {:.2} p25 {:.2} p75 {:.2}",
        dl_mean, dl_median, dl_p25, dl_p75
    ));

    let ul_values: Vec<f64> = ul_points.iter().map(|(_, y)| *y).collect();
    let (ul_mean, ul_median, ul_p25, ul_p75) = metrics::compute_metrics(&ul_values)
        .context("insufficient upload throughput data to compute metrics")?;
    lines.push(format!(
        "Upload:   avg {:.2} med {:.2} p25 {:.2} p75 {:.2}",
        ul_mean, ul_median, ul_p25, ul_p75
    ));

    let (idle_mean, idle_median, idle_p25, idle_p75) =
        metrics::compute_metrics(idle_latency_samples)
            .context("insufficient idle latency data to compute metrics")?;
    lines.push(format!(
        "Idle latency: avg {:.1} med {:.1} p25 {:.1} p75 {:.1} ms (loss {:.1}%, jitter {:.1} ms)",
        idle_mean,
        idle_median,
        idle_p25,
        idle_p75,
        enriched.idle_latency.loss * 100.0,
        enriched.idle_latency.jitter_ms.unwrap_or(f64::NAN)
    ));

    let (dl_lat_mean, dl_lat_median, dl_lat_p25, dl_lat_p75) =
        metrics::compute_metrics(loaded_dl_latency_samples)
            .context("insufficient loaded download latency data to compute metrics")?;
    lines.push(format!(
        "Loaded latency (download): avg {:.1} med {:.1} p25 {:.1} p75 {:.1} ms (loss {:.1}%, jitter {:.1} ms)",
        dl_lat_mean,
        dl_lat_median,
        dl_lat_p25,
        dl_lat_p75,
        enriched.loaded_latency_download.loss * 100.0,
        enriched.loaded_latency_download.jitter_ms.unwrap_or(f64::NAN)
    ));

    let (ul_lat_mean, ul_lat_median, ul_lat_p25, ul_lat_p75) =
        metrics::compute_metrics(loaded_ul_latency_samples)
            .context("insufficient loaded upload latency data to compute metrics")?;
    lines.push(format!(
        "Loaded latency (upload): avg {:.1} med {:.1} p25 {:.1} p75 {:.1} ms (loss {:.1}%, jitter {:.1} ms)",
        ul_lat_mean,
        ul_lat_median,
        ul_lat_p25,
        ul_lat_p75,
        enriched.loaded_latency_upload.loss * 100.0,
        enriched.loaded_latency_upload.jitter_ms.unwrap_or(f64::NAN)
    ));

    if let Some(ref exp) = enriched.experimental_udp {
        lines.push(format!(
            "Experimental UDP-like loss probe: loss {:.1}% med {} ms (target {:?})",
            exp.latency.loss * 100.0,
            exp.latency.median_ms.unwrap_or(f64::NAN),
            exp.target
        ));
    }

    Ok(TextSummary { lines })
}
