use crate::engine::cloudflare::CloudflareClient;
use crate::model::{LatencySummary, Phase, TestEvent};
use crate::stats::{latency_summary_from_samples, OnlineStats};
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Parameters for running a latency probe loop.
pub(crate) struct LatencyProbeParams<'a> {
    pub client: &'a CloudflareClient,
    pub phase: Phase,
    pub during: Option<Phase>,
    pub total_duration: Duration,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub event_tx: &'a mpsc::UnboundedSender<TestEvent>,
    pub paused: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
}

/// Run latency probes based on the provided parameters.
pub(crate) async fn run_latency_probes(params: LatencyProbeParams<'_>) -> Result<LatencySummary> {
    let LatencyProbeParams {
        client,
        phase,
        during,
        total_duration,
        interval_ms,
        timeout_ms,
        event_tx,
        paused,
        cancel,
    } = params;
    let start = Instant::now();
    let mut sent = 0u64;
    let mut received = 0u64;
    let mut samples = Vec::<f64>::new();
    let mut online = OnlineStats::default();
    let mut meta_sent = false;

    while start.elapsed() < total_duration {
        while paused.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        sent += 1;
        let during_str = during.and_then(|p| p.as_query_str());

        let r = client.probe_latency_ms(during_str, timeout_ms).await;
        match r {
            Ok((ms, meta_opt)) => {
                received += 1;
                samples.push(ms);
                online.push(ms);

                // Extract meta from first successful response
                if !meta_sent && phase == Phase::IdleLatency {
                    if let Some(meta) = meta_opt {
                        let _ = event_tx.send(TestEvent::MetaInfo { meta });
                        meta_sent = true;
                    }
                }

                let _ = event_tx.send(TestEvent::LatencySample {
                    phase,
                    during,
                    rtt_ms: Some(ms),
                    ok: true,
                });
            }
            Err(_) => {
                let _ = event_tx.send(TestEvent::LatencySample {
                    phase,
                    during,
                    rtt_ms: None,
                    ok: false,
                });
            }
        }

        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }

    Ok(latency_summary_from_samples(
        sent,
        received,
        &samples,
        online.stddev(),
    ))
}
