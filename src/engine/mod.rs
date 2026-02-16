mod cloudflare;
pub mod dns;
pub mod ip_comparison;
mod latency;
mod network_bind;
mod throughput;
pub mod tls;
pub mod traceroute;
mod turn_udp;

use crate::model::{
    DnsSummary, IpVersionComparison, Phase, RunConfig, RunResult, TestEvent, TlsSummary,
    TracerouteSummary,
};
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::mpsc;

/// Check if paused, wait while paused, and return true if cancelled.
/// Returns true if the caller should break out of its loop.
pub(crate) async fn wait_if_paused_or_cancelled(paused: &AtomicBool, cancel: &AtomicBool) -> bool {
    while paused.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cancel.load(Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub enum EngineControl {
    /// Pause (true) or resume (false) the running test
    Pause(bool),
    /// Cancel the test entirely
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

        // Try to get meta from multiple sources in order of preference:
        // 1. /meta endpoint (may have full details)
        // 2. /cdn-cgi/trace endpoint (reliable source for colo, ip, country)
        // 3. Response headers (fallback)
        let mut meta: Option<serde_json::Value> = match cloudflare::fetch_meta(&client).await {
            Ok(v) if !v.as_object().map(|m| m.is_empty()).unwrap_or(true) => Some(v),
            _ => None,
        };

        // If meta is empty or missing colo, try /cdn-cgi/trace
        let has_colo = meta
            .as_ref()
            .and_then(|m| m.get("colo"))
            .and_then(|v| v.as_str())
            .is_some();

        if !has_colo {
            if let Ok(trace_meta) = cloudflare::fetch_trace(&client).await {
                if !trace_meta.as_object().map(|m| m.is_empty()).unwrap_or(true) {
                    // Merge trace_meta into meta
                    if let Some(ref mut existing) = meta {
                        if let (Some(existing_map), Some(trace_map)) =
                            (existing.as_object_mut(), trace_meta.as_object())
                        {
                            for (k, v) in trace_map {
                                if !existing_map.contains_key(k) {
                                    existing_map.insert(k.clone(), v.clone());
                                }
                            }
                        }
                    } else {
                        meta = Some(trace_meta);
                    }
                }
            }
        }

        // Final fallback to response headers
        if meta.is_none() {
            meta = cloudflare::fetch_meta_from_response(&client).await.ok();
        }

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

        // Send meta info early so TUI can display server/colo/ip immediately
        if let Some(ref m) = meta {
            event_tx
                .send(TestEvent::MetaInfo { meta: m.clone() })
                .await
                .ok();
        }

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

        // Run diagnostic tests before the main speed test
        let mut dns_summary: Option<DnsSummary> = None;
        let mut tls_summary: Option<TlsSummary> = None;
        let mut ip_comparison_result: Option<IpVersionComparison> = None;
        let mut traceroute_summary: Option<TracerouteSummary> = None;
        let mut external_ipv4: Option<String> = None;
        let mut external_ipv6: Option<String> = None;

        // DNS Resolution measurement
        if self.cfg.measure_dns {
            if let Some(hostname) = dns::extract_hostname(&self.cfg.base_url) {
                event_tx
                    .send(TestEvent::Info {
                        message: format!("Measuring DNS resolution for {}...", hostname),
                    })
                    .await
                    .ok();

                match dns::measure_dns_resolution(&hostname).await {
                    Ok(summary) => {
                        event_tx
                            .send(TestEvent::DiagnosticDns {
                                summary: summary.clone(),
                            })
                            .await
                            .ok();
                        dns_summary = Some(summary);
                    }
                    Err(e) => {
                        event_tx
                            .send(TestEvent::Info {
                                message: format!("DNS measurement failed: {}", e),
                            })
                            .await
                            .ok();
                    }
                }
            }
        }

        // TLS Handshake measurement
        if self.cfg.measure_tls {
            if let Some((hostname, port)) = tls::extract_host_port(&self.cfg.base_url) {
                event_tx
                    .send(TestEvent::Info {
                        message: format!("Measuring TLS handshake with {}:{}...", hostname, port),
                    })
                    .await
                    .ok();

                match tls::measure_tls_handshake(&hostname, port).await {
                    Ok(summary) => {
                        event_tx
                            .send(TestEvent::DiagnosticTls {
                                summary: summary.clone(),
                            })
                            .await
                            .ok();
                        tls_summary = Some(summary);
                    }
                    Err(e) => {
                        event_tx
                            .send(TestEvent::Info {
                                message: format!("TLS measurement failed: {}", e),
                            })
                            .await
                            .ok();
                    }
                }
            }
        }

        // Fetch external IPs (runs in parallel, part of default diagnostics)
        if self.cfg.measure_dns {
            let (v4, v6) = dns::fetch_external_ips(&self.cfg.base_url).await;
            external_ipv4 = v4.clone();
            external_ipv6 = v6.clone();
            event_tx
                .send(TestEvent::ExternalIps { ipv4: v4, ipv6: v6 })
                .await
                .ok();
        }

        // IPv4 vs IPv6 comparison
        if self.cfg.compare_ip_versions {
            event_tx
                .send(TestEvent::Info {
                    message: "Comparing IPv4 vs IPv6 performance...".to_string(),
                })
                .await
                .ok();

            match ip_comparison::compare_ip_versions(&self.cfg.base_url, &self.cfg.user_agent).await
            {
                Ok(comparison) => {
                    event_tx
                        .send(TestEvent::DiagnosticIpComparison {
                            comparison: comparison.clone(),
                        })
                        .await
                        .ok();
                    ip_comparison_result = Some(comparison);
                }
                Err(e) => {
                    event_tx
                        .send(TestEvent::Info {
                            message: format!("IP comparison failed: {}", e),
                        })
                        .await
                        .ok();
                }
            }
        }

        // Traceroute
        if self.cfg.traceroute {
            if let Some(hostname) = dns::extract_hostname(&self.cfg.base_url) {
                event_tx
                    .send(TestEvent::Info {
                        message: format!(
                            "Running traceroute to {} (max {} hops)...",
                            hostname, self.cfg.traceroute_max_hops
                        ),
                    })
                    .await
                    .ok();

                match traceroute::run_traceroute(&hostname, self.cfg.traceroute_max_hops, &event_tx)
                    .await
                {
                    Ok(summary) => {
                        event_tx
                            .send(TestEvent::TracerouteComplete {
                                summary: summary.clone(),
                            })
                            .await
                            .ok();
                        traceroute_summary = Some(summary);
                    }
                    Err(e) => {
                        event_tx
                            .send(TestEvent::Info {
                                message: format!("Traceroute failed: {}", e),
                            })
                            .await
                            .ok();
                    }
                }
            }
        }

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

        // Prefetch DNS for STUN server during upload to eliminate delay before packet loss phase
        let stun_dns_handle = tokio::spawn(async move {
            tokio::net::lookup_host(("turn.cloudflare.com", 3478_u16))
                .await
                .ok()
                .and_then(|mut addrs| addrs.next())
        });

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
                phase: Phase::PacketLoss,
            })
            .await
            .ok();

        let mut experimental_udp = None;
        let mut udp_error = None;

        let info = crate::model::TurnInfo {
            urls: vec!["stun:turn.cloudflare.com:3478".to_string()],
            username: None,
            credential: None,
        };

        // Use prefetched DNS if available
        let pre_resolved = stun_dns_handle.await.ok().flatten();

        match turn_udp::run_udp_like_loss_probe(&info, &self.cfg, &event_tx, pre_resolved).await {
            Ok(udp) => {
                experimental_udp = Some(udp);
            }
            Err(e) => {
                let msg = format!("UDP probe failed: {e:#}");
                udp_error = Some(msg.clone());
                event_tx
                    .send(TestEvent::Info { message: msg })
                    .await
                    .ok();
            }
        }

        event_tx
            .send(TestEvent::PhaseStarted {
                phase: Phase::Summary,
            })
            .await
            .ok();

        // Abort the control listener task before returning.
        // In Tokio, dropping a JoinHandle does NOT cancel the task - it continues running!
        // This was causing high CPU usage when idle because the task was still waiting
        // on control_rx.recv().await even after the test completed.
        control_handle.abort();
        // Don't await the aborted task - just let it be cleaned up

        Ok(RunResult {
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            timestamp_utc: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "now".into()),
            base_url: self.cfg.base_url.clone(),
            meas_id: self.cfg.meas_id.clone(),
            comments: self.cfg.comments.clone(),
            meta,
            server,
            idle_latency,
            download,
            upload,
            loaded_latency_download,
            loaded_latency_upload,
            turn: None,
            experimental_udp,
            udp_error,
            // Network information - will be populated by TUI when available
            ip: None,
            colo: None,
            asn: None,
            as_org: None,
            interface_name: None,
            network_name: None,
            is_wireless: None,
            interface_mac: None,
            local_ipv4: None,
            local_ipv6: None,
            external_ipv4,
            external_ipv6,
            // Diagnostic results
            dns: dns_summary,
            tls: tls_summary,
            ip_comparison: ip_comparison_result,
            traceroute: traceroute_summary,
        })
    }
}
