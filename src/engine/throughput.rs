use crate::engine::cloudflare::CloudflareClient;
use crate::engine::latency::{run_latency_probes, LatencyProbeParams};
use crate::model::{LatencySummary, Phase, RunConfig, TestEvent, ThroughputSummary};
use anyhow::{Context, Result};
use bytes::Bytes;
use futures::{stream, StreamExt};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Chunk size for upload stream generation (64 KB)
const UPLOAD_CHUNK_SIZE: u64 = 64 * 1024;

fn throughput_summary(bytes: u64, duration: Duration, mbps_samples: &[f64]) -> ThroughputSummary {
    // Compute metrics using the same method as metrics.rs for consistency
    let fallback_mbps = || {
        let secs = duration.as_secs_f64().max(1e-9);
        let bps = (bytes as f64) / secs;
        let mbps = (bps * 8.0) / 1_000_000.0;
        (mbps, mbps, mbps, mbps)
    };

    let (mean_mbps, median_mbps, p25_mbps, p75_mbps) =
        crate::metrics::compute_metrics(mbps_samples).unwrap_or_else(fallback_mbps);

    let mbps = mean_mbps;

    ThroughputSummary {
        bytes,
        duration_ms: duration.as_millis() as u64,
        mbps,
        mean_mbps: Some(mean_mbps),
        median_mbps: Some(median_mbps),
        p25_mbps: Some(p25_mbps),
        p75_mbps: Some(p75_mbps),
    }
}

fn estimate_steady_window(
    samples: &[(Instant, u64)],
    total_duration: Duration,
) -> Option<(u64, Duration)> {
    if samples.len() < 2 {
        return None;
    }
    let ignore = total_duration.mul_f64(0.20).max(Duration::from_secs(1));
    let t0 = samples[0].0 + ignore;
    let start_idx = samples.iter().position(|(t, _)| *t >= t0).unwrap_or(0);
    let (t_start, b_start) = samples[start_idx];
    let (t_end, b_end) = *samples.last().unwrap();
    let dt = t_end.saturating_duration_since(t_start);
    if dt.as_millis() < 200 {
        return None;
    }
    Some((b_end.saturating_sub(b_start), dt))
}

pub async fn run_download_with_loaded_latency(
    client: &CloudflareClient,
    cfg: &RunConfig,
    event_tx: &mpsc::UnboundedSender<TestEvent>,
    paused: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> Result<(ThroughputSummary, LatencySummary)> {
    let stop = Arc::new(AtomicBool::new(false));
    let total = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..cfg.concurrency {
        let http = client.http.clone();
        let mut url = client.down_url();
        let meas_id = client.meas_id.clone();
        url.query_pairs_mut()
            .append_pair("measId", &meas_id)
            .append_pair("bytes", &cfg.download_bytes_per_req.to_string());
        let stop2 = stop.clone();
        let total2 = total.clone();

        handles.push(tokio::spawn(async move {
            while !stop2.load(Ordering::Relaxed) {
                let resp = match http.get(url.clone()).send().await {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let Ok(b) = chunk else { break };
                    total2.fetch_add(b.len() as u64, Ordering::Relaxed);
                    if stop2.load(Ordering::Relaxed) {
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
        let res = run_latency_probes(LatencyProbeParams {
            client: &client2,
            phase: Phase::Download,
            during: Some(Phase::Download),
            total_duration: cfg2.download_duration,
            interval_ms: cfg2.probe_interval_ms,
            timeout_ms: cfg2.probe_timeout_ms,
            event_tx: &ev2,
            paused: paused2,
            cancel: cancel2,
        })
        .await
        .unwrap_or_else(|_| LatencySummary::failed());
        let _ = lat_tx.send(res).await;
    });

    let start = Instant::now();
    let mut last_bytes = 0u64;
    let mut last_t = Instant::now();
    let mut samples: Vec<(Instant, u64)> = Vec::with_capacity(256);
    let mut mbps_samples: Vec<f64> = Vec::with_capacity(256);

    while start.elapsed() < cfg.download_duration {
        while paused.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let now_total = total.load(Ordering::Relaxed);
        let dt = last_t.elapsed().as_secs_f64().max(1e-9);
        let dbytes = now_total.saturating_sub(last_bytes);
        let bps_instant = (dbytes as f64) / dt;
        let mbps_instant = (bps_instant * 8.0) / 1_000_000.0;
        last_t = Instant::now();
        last_bytes = now_total;
        samples.push((Instant::now(), now_total));
        mbps_samples.push(mbps_instant);

        let _ = event_tx.send(TestEvent::ThroughputTick {
            phase: Phase::Download,
            bytes_total: now_total,
            bps_instant,
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.await;
    }

    let duration = start.elapsed();
    let bytes_total = total.load(Ordering::Relaxed);
    let (bytes, window) =
        estimate_steady_window(&samples, duration).unwrap_or((bytes_total, duration));
    let dl = throughput_summary(bytes, window, &mbps_samples);

    let loaded_latency = lat_rx
        .recv()
        .await
        .context("loaded latency task ended unexpectedly")?;

    // Ensure the latency probe task has completed
    let _ = lat_handle.await;

    Ok((dl, loaded_latency))
}

pub async fn run_upload_with_loaded_latency(
    client: &CloudflareClient,
    cfg: &RunConfig,
    event_tx: &mpsc::UnboundedSender<TestEvent>,
    paused: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> Result<(ThroughputSummary, LatencySummary)> {
    let stop = Arc::new(AtomicBool::new(false));
    let total = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..cfg.concurrency {
        let http = client.http.clone();
        let mut url = client.up_url();
        url.query_pairs_mut().append_pair("measId", &client.meas_id);
        let stop2 = stop.clone();
        let total2 = total.clone();
        let bytes_per_req = cfg.upload_bytes_per_req;

        handles.push(tokio::spawn(async move {
            while !stop2.load(Ordering::Relaxed) {
                // Generate upload body as a bounded stream of bytes.
                // We count bytes as we *produce* chunks for reqwest. This is a close approximation
                // of bytes put on the wire and produces stable realtime Mbps for the UI.
                let chunk = Bytes::from(vec![0u8; UPLOAD_CHUNK_SIZE as usize]);

                let full = bytes_per_req / UPLOAD_CHUNK_SIZE;
                let tail = bytes_per_req % UPLOAD_CHUNK_SIZE;

                let total2a = total2.clone();
                let chunk_full = chunk.clone();
                let s_full = stream::iter(0..full).map(move |_| {
                    total2a.fetch_add(UPLOAD_CHUNK_SIZE, Ordering::Relaxed);
                    Ok::<Bytes, std::io::Error>(chunk_full.clone())
                });

                let body_stream = if tail == 0 {
                    s_full.boxed()
                } else {
                    let total2b = total2.clone();
                    let chunk_tail = chunk.slice(..tail as usize);
                    let s_tail = stream::once(async move {
                        total2b.fetch_add(tail, Ordering::Relaxed);
                        Ok::<Bytes, std::io::Error>(chunk_tail)
                    });
                    s_full.chain(s_tail).boxed()
                };

                let body = reqwest::Body::wrap_stream(body_stream);
                let _ = http.post(url.clone()).body(body).send().await;
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
        let res = run_latency_probes(LatencyProbeParams {
            client: &client2,
            phase: Phase::Upload,
            during: Some(Phase::Upload),
            total_duration: cfg2.upload_duration,
            interval_ms: cfg2.probe_interval_ms,
            timeout_ms: cfg2.probe_timeout_ms,
            event_tx: &ev2,
            paused: paused2,
            cancel: cancel2,
        })
        .await
        .unwrap_or_else(|_| LatencySummary::failed());
        let _ = lat_tx.send(res).await;
    });

    let start = Instant::now();
    let mut last_bytes = 0u64;
    let mut last_t = Instant::now();
    let mut samples: Vec<(Instant, u64)> = Vec::with_capacity(256);
    let mut mbps_samples: Vec<f64> = Vec::with_capacity(256);

    while start.elapsed() < cfg.upload_duration {
        while paused.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let now_total = total.load(Ordering::Relaxed);
        let dt = last_t.elapsed().as_secs_f64().max(1e-9);
        let dbytes = now_total.saturating_sub(last_bytes);
        let bps_instant = (dbytes as f64) / dt;
        let mbps_instant = (bps_instant * 8.0) / 1_000_000.0;
        last_t = Instant::now();
        last_bytes = now_total;
        samples.push((Instant::now(), now_total));
        mbps_samples.push(mbps_instant);

        let _ = event_tx.send(TestEvent::ThroughputTick {
            phase: Phase::Upload,
            bytes_total: now_total,
            bps_instant,
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Switch phase as soon as the timed loop is done; finalization continues below.
    let _ = event_tx.send(TestEvent::PhaseStarted {
        phase: Phase::Summary,
    });

    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.await;
    }

    let duration = start.elapsed();
    let bytes_total = total.load(Ordering::Relaxed);
    let (bytes, window) =
        estimate_steady_window(&samples, duration).unwrap_or((bytes_total, duration));
    let up = throughput_summary(bytes, window, &mbps_samples);

    let loaded_latency = lat_rx
        .recv()
        .await
        .context("loaded latency task ended unexpectedly")?;

    // Ensure the latency probe task has completed
    let _ = lat_handle.await;

    Ok((up, loaded_latency))
}
