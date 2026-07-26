use crate::constants::THROUGHPUT_SAMPLE_INTERVAL;
use crate::engine::cloudflare::CloudflareClient;
use crate::engine::latency::run_latency_probes;
use crate::engine::wait_if_paused_or_cancelled;
use crate::model::{LatencySummary, Phase, RunConfig, TestEvent, ThroughputSummary};
use anyhow::{Context, Result};
use bytes::Bytes;
use futures::{stream, StreamExt};
use reqwest::StatusCode;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Chunk size for upload stream generation (64 KB)
const UPLOAD_CHUNK_SIZE: u64 = 64 * 1024;
const MIN_DOWNLOAD_BYTES_PER_REQ: u64 = 100_000;

fn throughput_summary(bytes: u64, duration: Duration, mbps_samples: &[f64]) -> ThroughputSummary {
    // Compute metrics using the same method as metrics.rs for consistency
    let fallback_mbps = || {
        let secs = duration.as_secs_f64().max(1e-9);
        let bps = (bytes as f64) / secs;
        let mbps = (bps * 8.0) / 1_000_000.0;
        (mbps, mbps, mbps, mbps)
    };

    let (mean_mbps, median_mbps, p25_mbps, p75_mbps, p95_mbps, p99_mbps) =
        crate::metrics::compute_sample_metrics(mbps_samples)
            .map(|m| (m.mean, m.median, m.p25, m.p75, m.p95, m.p99))
            .unwrap_or_else(|| {
                let (mean, med, p25, p75) = fallback_mbps();
                (mean, med, p25, p75, mean, mean)
            });

    let mbps = mean_mbps;

    ThroughputSummary {
        bytes,
        duration_ms: duration.as_millis() as u64,
        mbps,
        mean_mbps: Some(mean_mbps),
        median_mbps: Some(median_mbps),
        p25_mbps: Some(p25_mbps),
        p75_mbps: Some(p75_mbps),
        p95_mbps: Some(p95_mbps),
        p99_mbps: Some(p99_mbps),
    }
}

/// Steady-state view of a phase: excludes the ramp-up (the first
/// `STEADY_STATE_RAMP_FRACTION` of the phase, at least `STEADY_STATE_MIN_RAMP`)
/// from the byte counters AND the per-tick rate samples, so slow start does
/// not drag the reported throughput down. `samples[i]` is (active time at
/// tick i, cumulative bytes at tick i); `mbps_samples[i]` is the rate over
/// the interval ending at tick i. Returns `None` when there is no usable
/// steady window (caller falls back to the whole phase).
fn steady_state_view<'a>(
    samples: &[(Duration, u64)],
    mbps_samples: &'a [f64],
    total_duration: Duration,
) -> Option<(u64, Duration, &'a [f64])> {
    if samples.len() < 2 {
        return None;
    }
    let ramp = total_duration
        .mul_f64(crate::constants::STEADY_STATE_RAMP_FRACTION)
        .max(crate::constants::STEADY_STATE_MIN_RAMP);
    let start_idx = samples.iter().position(|(t, _)| *t >= ramp)?;
    let (t_start, b_start) = samples[start_idx];
    let (t_end, b_end) = *samples.last().unwrap();
    let window = t_end.saturating_sub(t_start);
    if window < THROUGHPUT_SAMPLE_INTERVAL {
        return None;
    }
    // mbps_samples[i] covers the interval ending at samples[i]; the intervals
    // fully inside the window start at start_idx + 1.
    let steady = &mbps_samples[(start_idx + 1).min(mbps_samples.len())..];
    Some((b_end.saturating_sub(b_start), window, steady))
}

pub async fn run_download_with_loaded_latency(
    client: &CloudflareClient,
    cfg: &RunConfig,
    event_tx: &mpsc::Sender<TestEvent>,
    paused: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> Result<(ThroughputSummary, LatencySummary)> {
    // A cancel that arrived before this phase: don't spawn workers at all.
    if cancel.load(Ordering::Relaxed) {
        return Ok((
            throughput_summary(0, Duration::ZERO, &[]),
            LatencySummary::failed(),
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let total = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..cfg.concurrency {
        let http = client.http.clone();
        let base_url = client.down_url();
        let meas_id = client.meas_id.clone();
        let mut bytes_per_req = cfg.download_bytes_per_req;
        let stop2 = stop.clone();
        let total2 = total.clone();
        let errors2 = errors.clone();
        let paused_w = paused.clone();
        let ev_dl = event_tx.clone();

        handles.push(tokio::spawn(async move {
            while !stop2.load(Ordering::Relaxed) {
                // Paused means paused: stop pulling bytes so the link is
                // actually idle, not just unsampled.
                if paused_w.load(Ordering::Relaxed) {
                    tokio::time::sleep(crate::constants::PAUSE_POLL_INTERVAL).await;
                    continue;
                }
                let mut url = base_url.clone();
                url.query_pairs_mut()
                    .append_pair("measId", &meas_id)
                    .append_pair("bytes", &bytes_per_req.to_string());

                let resp = match http.get(url).send().await {
                    Ok(r) => r,
                    Err(_) => {
                        errors2.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(crate::constants::WORKER_ERROR_BACKOFF).await;
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    errors2.fetch_add(1, Ordering::Relaxed);
                    if resp.status() == StatusCode::TOO_MANY_REQUESTS {
                        let next = (bytes_per_req / 2).max(MIN_DOWNLOAD_BYTES_PER_REQ);
                        if next < bytes_per_req {
                            bytes_per_req = next;
                            let _ = ev_dl
                                .send(TestEvent::Info {
                                    message: format!(
                                        "Download: 429 from server, reducing bytes per request to {}",
                                        bytes_per_req
                                    ),
                                })
                                .await;
                        }
                    }
                    tokio::time::sleep(crate::constants::WORKER_ERROR_BACKOFF).await;
                    continue;
                }

                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let Ok(b) = chunk else { break };
                    total2.fetch_add(b.len() as u64, Ordering::Relaxed);
                    if stop2.load(Ordering::Relaxed) || paused_w.load(Ordering::Relaxed) {
                        break;
                    }
                }
            }
        }));
    }

    // Loaded latency task (during download).
    let (lat_tx, mut lat_rx) = mpsc::channel::<LatencySummary>(1);
    let client2 = client.clone();
    let ev2 = event_tx.clone();
    let paused2 = paused.clone();
    let cancel2 = cancel.clone();
    let cfg2 = cfg.clone();
    let lat_handle = tokio::spawn(async move {
        let res = run_latency_probes(
            &client2,
            Phase::Download,
            Some(Phase::Download),
            cfg2.download_duration,
            cfg2.probe_interval_ms,
            cfg2.probe_timeout_ms,
            &ev2,
            paused2,
            cancel2,
        )
        .await
        .unwrap_or_else(|_| LatencySummary::failed());
        let _ = lat_tx.send(res).await;
    });

    // The phase clock counts ACTIVE time only, so a pause neither eats the
    // remaining duration nor leaks into any rate sample.
    let mut active = Duration::ZERO;
    let mut last_bytes = 0u64;
    let mut samples: Vec<(Duration, u64)> = Vec::with_capacity(256);
    let mut mbps_samples: Vec<f64> = Vec::with_capacity(256);

    while active < cfg.download_duration {
        let was_paused = paused.load(Ordering::Relaxed);
        if wait_if_paused_or_cancelled(&paused, &cancel).await {
            break;
        }
        if was_paused {
            // Just resumed: restart the rate baseline so no sample spans the pause.
            last_bytes = total.load(Ordering::Relaxed);
        }

        let tick_start = Instant::now();
        tokio::time::sleep(THROUGHPUT_SAMPLE_INTERVAL).await;
        let tick_len = tick_start.elapsed();
        active += tick_len;

        let now_total = total.load(Ordering::Relaxed);
        let dt = tick_len.as_secs_f64().max(1e-9);
        let dbytes = now_total.saturating_sub(last_bytes);
        let bps_instant = (dbytes as f64) / dt;
        let mbps_instant = (bps_instant * 8.0) / 1_000_000.0;
        last_bytes = now_total;
        samples.push((active, now_total));
        mbps_samples.push(mbps_instant);

        event_tx
            .send(TestEvent::ThroughputTick {
                phase: Phase::Download,
                bytes_total: now_total,
                bps_instant,
            })
            .await
            .ok();
    }

    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.await;
    }

    let duration = active;
    let bytes_total = total.load(Ordering::Relaxed);
    let error_count = errors.load(Ordering::Relaxed);
    if error_count > 0 {
        event_tx
            .send(TestEvent::Info {
                message: format!("Download: {} request(s) failed", error_count),
            })
            .await
            .ok();
    }
    let (bytes, window, steady_mbps) = steady_state_view(&samples, &mbps_samples, duration)
        .unwrap_or((bytes_total, duration, &mbps_samples[..]));
    let dl = throughput_summary(bytes, window, steady_mbps);

    // Wait for latency results with a timeout to prevent indefinite hangs
    let loaded_latency = tokio::time::timeout(Duration::from_secs(30), lat_rx.recv())
        .await
        .context("timed out waiting for loaded latency results")?
        .context("loaded latency task ended unexpectedly")?;

    // Ensure the latency probe task has completed
    let _ = lat_handle.await;

    Ok((dl, loaded_latency))
}

pub async fn run_upload_with_loaded_latency(
    client: &CloudflareClient,
    cfg: &RunConfig,
    event_tx: &mpsc::Sender<TestEvent>,
    paused: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> Result<(ThroughputSummary, LatencySummary)> {
    // A cancel that arrived before this phase: don't spawn workers at all.
    if cancel.load(Ordering::Relaxed) {
        return Ok((
            throughput_summary(0, Duration::ZERO, &[]),
            LatencySummary::failed(),
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let total = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..cfg.concurrency {
        let http = client.http.clone();
        let mut url = client.up_url();
        url.query_pairs_mut().append_pair("measId", &client.meas_id);
        let stop2 = stop.clone();
        let total2 = total.clone();
        let errors2 = errors.clone();
        let paused_w = paused.clone();
        let bytes_per_req = cfg.upload_bytes_per_req;

        handles.push(tokio::spawn(async move {
            while !stop2.load(Ordering::Relaxed) {
                // Paused means paused: stop pushing bytes so the link is
                // actually idle, not just unsampled.
                if paused_w.load(Ordering::Relaxed) {
                    tokio::time::sleep(crate::constants::PAUSE_POLL_INTERVAL).await;
                    continue;
                }
                // Count bytes as the HTTP client pulls chunks from the stream
                // (backpressure-aware). On failure, subtract what we counted so
                // the live chart stays smooth without permanently over-reporting.
                let chunk = Bytes::from(vec![0u8; UPLOAD_CHUNK_SIZE as usize]);

                let full = bytes_per_req / UPLOAD_CHUNK_SIZE;
                let tail = bytes_per_req % UPLOAD_CHUNK_SIZE;
                let counted = Arc::new(AtomicU64::new(0));

                let total2a = total2.clone();
                let counted_a = counted.clone();
                let chunk_full = chunk.clone();
                let s_full = stream::iter(0..full).map(move |_| {
                    total2a.fetch_add(UPLOAD_CHUNK_SIZE, Ordering::Relaxed);
                    counted_a.fetch_add(UPLOAD_CHUNK_SIZE, Ordering::Relaxed);
                    Ok::<Bytes, std::io::Error>(chunk_full.clone())
                });

                let body_stream = if tail == 0 {
                    s_full.boxed()
                } else {
                    let total2b = total2.clone();
                    let counted_b = counted.clone();
                    let chunk_tail = chunk.slice(..tail as usize);
                    let s_tail = stream::once(async move {
                        total2b.fetch_add(tail, Ordering::Relaxed);
                        counted_b.fetch_add(tail, Ordering::Relaxed);
                        Ok::<Bytes, std::io::Error>(chunk_tail)
                    });
                    s_full.chain(s_tail).boxed()
                };

                let body = reqwest::Body::wrap_stream(body_stream);
                // Abort the in-flight request the moment a pause lands, so
                // "paused" doesn't keep saturating the uplink for a whole body.
                let pause_hit = async {
                    while !paused_w.load(Ordering::Relaxed) {
                        tokio::time::sleep(crate::constants::PAUSE_POLL_INTERVAL).await;
                    }
                };
                tokio::select! {
                    res = http.post(url.clone()).body(body).send() => {
                        match res {
                            Ok(resp) if resp.status().is_success() => {}
                            _ => {
                                let rolled_back = counted.load(Ordering::Relaxed);
                                total2.fetch_sub(rolled_back, Ordering::Relaxed);
                                errors2.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    _ = pause_hit => {
                        // Dropped mid-request: those bytes never fully arrived.
                        let rolled_back = counted.load(Ordering::Relaxed);
                        total2.fetch_sub(rolled_back, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // Loaded latency task (during upload).
    let (lat_tx, mut lat_rx) = mpsc::channel::<LatencySummary>(1);
    let client2 = client.clone();
    let ev2 = event_tx.clone();
    let paused2 = paused.clone();
    let cancel2 = cancel.clone();
    let cfg2 = cfg.clone();
    let lat_handle = tokio::spawn(async move {
        let res = run_latency_probes(
            &client2,
            Phase::Upload,
            Some(Phase::Upload),
            cfg2.upload_duration,
            cfg2.probe_interval_ms,
            cfg2.probe_timeout_ms,
            &ev2,
            paused2,
            cancel2,
        )
        .await
        .unwrap_or_else(|_| LatencySummary::failed());
        let _ = lat_tx.send(res).await;
    });

    // Active-time phase clock; see the download loop for the pause semantics.
    let mut active = Duration::ZERO;
    let mut last_bytes = 0u64;
    let mut samples: Vec<(Duration, u64)> = Vec::with_capacity(256);
    let mut mbps_samples: Vec<f64> = Vec::with_capacity(256);

    while active < cfg.upload_duration {
        let was_paused = paused.load(Ordering::Relaxed);
        if wait_if_paused_or_cancelled(&paused, &cancel).await {
            break;
        }
        if was_paused {
            last_bytes = total.load(Ordering::Relaxed);
        }

        let tick_start = Instant::now();
        tokio::time::sleep(THROUGHPUT_SAMPLE_INTERVAL).await;
        let tick_len = tick_start.elapsed();
        active += tick_len;

        let now_total = total.load(Ordering::Relaxed);
        let dt = tick_len.as_secs_f64().max(1e-9);
        let dbytes = now_total.saturating_sub(last_bytes);
        let bps_instant = (dbytes as f64) / dt;
        let mbps_instant = (bps_instant * 8.0) / 1_000_000.0;
        last_bytes = now_total;
        samples.push((active, now_total));
        mbps_samples.push(mbps_instant);

        event_tx
            .send(TestEvent::ThroughputTick {
                phase: Phase::Upload,
                bytes_total: now_total,
                bps_instant,
            })
            .await
            .ok();
    }

    // Tick loop has ended — from the user's perspective the upload phase is over.
    // Announce the next phase now so the dashboard updates immediately, even though
    // the worker drain and latency probe await below may take a moment.
    event_tx
        .send(TestEvent::PhaseStarted {
            phase: Phase::PacketLoss,
        })
        .await
        .ok();

    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.await;
    }

    let duration = active;
    let bytes_total = total.load(Ordering::Relaxed);
    let error_count = errors.load(Ordering::Relaxed);
    if error_count > 0 {
        event_tx
            .send(TestEvent::Info {
                message: format!("Upload: {} request(s) failed", error_count),
            })
            .await
            .ok();
    }
    let (bytes, window, steady_mbps) = steady_state_view(&samples, &mbps_samples, duration)
        .unwrap_or((bytes_total, duration, &mbps_samples[..]));
    let up = throughput_summary(bytes, window, steady_mbps);

    // Wait for latency results with a timeout to prevent indefinite hangs
    let loaded_latency = tokio::time::timeout(Duration::from_secs(30), lat_rx.recv())
        .await
        .context("timed out waiting for loaded latency results")?
        .context("loaded latency task ended unexpectedly")?;

    // Ensure the latency probe task has completed
    let _ = lat_handle.await;

    Ok((up, loaded_latency))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// Ten 1s ticks: bytes crawl for the first two ticks (ramp), then advance
    /// a steady 100 bytes/tick. `mbps[i]` is the rate of the interval ending
    /// at tick i.
    fn ramp_fixture() -> (Vec<(Duration, u64)>, Vec<f64>) {
        let bytes = [5u64, 30, 130, 230, 330, 430, 530, 630, 730, 830];
        let samples: Vec<(Duration, u64)> = bytes
            .iter()
            .enumerate()
            .map(|(i, b)| (secs(i as u64 + 1), *b))
            .collect();
        let mut mbps = vec![0.04, 0.2];
        mbps.extend(std::iter::repeat_n(0.8, 8));
        (samples, mbps)
    }

    #[test]
    fn steady_state_view_excludes_ramp_up() {
        let (samples, mbps) = ramp_fixture();
        // 10s phase: ramp = max(20% x 10s, 1s) = 2s, so steady starts at the
        // first tick at/after t=2s (index 1).
        let (bytes, window, steady) = steady_state_view(&samples, &mbps, secs(10)).unwrap();
        assert_eq!(bytes, 830 - 30);
        assert_eq!(window, secs(8));
        assert_eq!(steady, &mbps[2..]);
    }

    #[test]
    fn steady_state_view_none_when_too_short() {
        assert!(steady_state_view(&[], &[], secs(10)).is_none());
        assert!(steady_state_view(&[(secs(1), 100)], &[0.8], secs(1)).is_none());
        // Every tick still inside the ramp: nothing steady to report.
        let samples = vec![
            (Duration::from_millis(200), 10),
            (Duration::from_millis(400), 20),
        ];
        assert!(steady_state_view(&samples, &[0.4, 0.4], secs(10)).is_none());
    }

    #[test]
    fn throughput_summary_uses_sample_percentiles() {
        let s = throughput_summary(1000, secs(1), &[8.0, 8.0, 8.0, 8.0, 8.0]);
        assert!((s.mbps - 8.0).abs() < 1e-9);
        assert_eq!(s.bytes, 1000);
    }

    #[test]
    fn throughput_summary_falls_back_to_bytes_over_duration() {
        // 1_000_000 bytes in 1s = 8 Mbps
        let s = throughput_summary(1_000_000, secs(1), &[]);
        assert!((s.mbps - 8.0).abs() < 1e-6);
    }
}
