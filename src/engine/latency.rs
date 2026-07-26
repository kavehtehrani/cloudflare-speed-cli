use crate::engine::cloudflare::CloudflareClient;
use crate::engine::wait_if_paused_or_cancelled;
use crate::model::{LatencySummary, Phase, TestEvent};
use crate::stats::{latency_summary_from_samples, OnlineStats};
use anyhow::Result;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[allow(clippy::too_many_arguments)]
pub async fn run_latency_probes(
    client: &CloudflareClient,
    phase: Phase,
    during: Option<Phase>,
    total_duration: Duration,
    interval_ms: u64,
    timeout_ms: u64,
    event_tx: &mpsc::Sender<TestEvent>,
    paused: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> Result<LatencySummary> {
    let start = Instant::now();
    let mut sent = 0u64;
    let mut received = 0u64;
    let mut samples = Vec::<f64>::new();
    let mut online = OnlineStats::default();
    let mut meta_sent = false;

    while start.elapsed() < total_duration {
        if wait_if_paused_or_cancelled(&paused, &cancel).await {
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
                        event_tx.send(TestEvent::MetaInfo { meta }).await.ok();
                        meta_sent = true;
                    }
                }

                event_tx
                    .send(TestEvent::LatencySample {
                        phase,
                        during,
                        rtt_ms: Some(ms),
                        ok: true,
                    })
                    .await
                    .ok();
            }
            Err(_) => {
                event_tx
                    .send(TestEvent::LatencySample {
                        phase,
                        during,
                        rtt_ms: None,
                        ok: false,
                    })
                    .await
                    .ok();
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
