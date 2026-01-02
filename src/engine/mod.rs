mod cloudflare;
mod latency;
mod network_bind;
mod throughput;
mod turn_udp;

use crate::model::{Phase, RunConfig, RunResult, TestEvent};
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
#[allow(dead_code)] // used by TUI control loop (M2+)
pub enum EngineControl {
    Pause(bool),
    Cancel,
}

pub struct TestEngine {
    cfg: RunConfig,
}

impl TestEngine {
    pub fn new(cfg: RunConfig) -> Self {
        Self { cfg }
    }

    pub async fn run(
        self,
        event_tx: mpsc::Sender<TestEvent>,
        mut control_rx: mpsc::Receiver<EngineControl>,
    ) -> Result<RunResult> {
        let client = cloudflare::CloudflareClient::new(&self.cfg)?;

        let paused = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));

        // Try to get meta from /meta endpoint first, then fall back to response headers
        let meta: Option<serde_json::Value> = match cloudflare::fetch_meta(&client).await {
            Ok(v) if !v.as_object().map(|m| m.is_empty()).unwrap_or(true) => Some(v),
            _ => {
                // Fall back to extracting from response headers
                cloudflare::fetch_meta_from_response(&client).await.ok()
            }
        };

        let locations = cloudflare::fetch_locations(&client).await.ok();
        let server = meta
            .as_ref()
            .and_then(|m: &serde_json::Value| {
                m.get("colo").and_then(|v: &serde_json::Value| v.as_str())
            })
            .and_then(|colo| {
                locations
                    .as_ref()
                    .and_then(|loc| cloudflare::map_colo_to_server(loc, colo))
            });

        // Control listener.
        let paused2 = paused.clone();
        let cancel2 = cancel.clone();
        let control_handle = tokio::spawn(async move {
            while let Some(msg) = control_rx.recv().await {
                match msg {
                    EngineControl::Pause(p) => paused2.store(p, Ordering::Relaxed),
                    EngineControl::Cancel => {
                        cancel2.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        });

        event_tx
            .send(TestEvent::PhaseStarted {
                phase: Phase::IdleLatency,
            })
            .await
            .ok();

        let idle_latency = latency::run_latency_probes(
            &client,
            Phase::IdleLatency,
            None,
            self.cfg.idle_latency_duration,
            self.cfg.probe_interval_ms,
            self.cfg.probe_timeout_ms,
            &event_tx,
            paused.clone(),
            cancel.clone(),
        )
        .await?;

        if self.cfg.experimental {
            event_tx
                .send(TestEvent::Info {
                    message: "Fetching TURN info (experimental)".into(),
                })
                .await
                .ok();
        }

        event_tx
            .send(TestEvent::PhaseStarted {
                phase: Phase::Download,
            })
            .await
            .ok();

        let (download, loaded_latency_download) = throughput::run_download_with_loaded_latency(
            &client,
            &self.cfg,
            &event_tx,
            paused.clone(),
            cancel.clone(),
        )
        .await?;

        event_tx
            .send(TestEvent::PhaseStarted {
                phase: Phase::Upload,
            })
            .await
            .ok();

        let (upload, loaded_latency_upload) = throughput::run_upload_with_loaded_latency(
            &client,
            &self.cfg,
            &event_tx,
            paused,
            cancel.clone(),
        )
        .await?;

        event_tx
            .send(TestEvent::PhaseStarted {
                phase: Phase::Summary,
            })
            .await
            .ok();

        let mut turn = None;
        let mut experimental_udp = None;
        if self.cfg.experimental {
            if let Ok(info) = cloudflare::fetch_turn(&client).await {
                experimental_udp = turn_udp::run_udp_like_loss_probe(&info, &self.cfg)
                    .await
                    .ok();
                turn = Some(info);
            }
        }

        // Abort the control listener task before returning.
        // In Tokio, dropping a JoinHandle does NOT cancel the task - it continues running!
        // This was causing high CPU usage when idle because the task was still waiting
        // on control_rx.recv().await even after the test completed.
        control_handle.abort();
        // Don't await the aborted task - just let it be cleaned up

        Ok(RunResult {
            timestamp_utc: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "now".into()),
            base_url: self.cfg.base_url.clone(),
            meas_id: self.cfg.meas_id.clone(),
            meta,
            server,
            idle_latency,
            download,
            upload,
            loaded_latency_download,
            loaded_latency_upload,
            turn,
            experimental_udp,
            // Network information - will be populated by TUI when available
            ip: None,
            colo: None,
            asn: None,
            as_org: None,
            interface_name: None,
            network_name: None,
            is_wireless: None,
            interface_mac: None,
            link_speed_mbps: None,
        })
    }
}
