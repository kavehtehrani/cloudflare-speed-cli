//! IPv4 vs IPv6 comparison module
//!
//! Runs abbreviated speed tests on both IPv4 and IPv6 to compare performance.

use super::network_bind::IpFamily;
use crate::model::{IpVersionComparison, IpVersionResult};
use anyhow::Result;
use reqwest::Url;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::net::lookup_host;

/// Duration for each abbreviated speed test (download/upload)
const TEST_DURATION: Duration = Duration::from_secs(3);

/// Run IPv4 vs IPv6 comparison tests.
///
/// Resolves the hostname to both IPv4 and IPv6 addresses, then runs
/// abbreviated speed tests on each protocol.
pub async fn compare_ip_versions(
    base_url: &str,
    user_agent: &str,
    interface: Option<&str>,
    bind_ip: Option<IpAddr>,
    cert_path: Option<&std::path::Path>,
    family: Option<IpFamily>,
    cancel: &AtomicBool,
) -> Result<IpVersionComparison> {
    let url = Url::parse(base_url)?;
    let hostname = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("No host in URL"))?;
    let port = url.port_or_known_default().unwrap_or(443);

    // Resolve hostname to get both IPv4 and IPv6 addresses
    let lookup_target = format!("{}:{}", hostname, port);
    let addrs: Vec<SocketAddr> = lookup_host(&lookup_target).await?.collect();

    let mut ipv4_addr: Option<IpAddr> = None;
    let mut ipv6_addr: Option<IpAddr> = None;

    for addr in addrs {
        match addr.ip() {
            ip @ IpAddr::V4(_) if ipv4_addr.is_none() => ipv4_addr = Some(ip),
            ip @ IpAddr::V6(_) if ipv6_addr.is_none() => ipv6_addr = Some(ip),
            _ => {}
        }
        if ipv4_addr.is_some() && ipv6_addr.is_some() {
            break;
        }
    }

    // A run-wide restriction (--ipv4-only / --ipv6-only) wins over the
    // comparison's "test both": skip the disabled family rather than connect
    // on it, and explain why in the result.
    let test_v4 = family != Some(IpFamily::V6);
    let test_v6 = family != Some(IpFamily::V4);

    // Test IPv4
    let ipv4_result = Some(if cancel.load(Ordering::Relaxed) {
        unavailable_result("Cancelled")
    } else if !test_v4 {
        skipped_result(family)
    } else if let Some(ip) = ipv4_addr {
        test_ip_version(
            base_url, hostname, port, ip, user_agent, interface, bind_ip, cert_path, cancel,
        )
        .await
    } else {
        unavailable_result("No IPv4 address resolved")
    });

    // Test IPv6
    let ipv6_result = Some(if cancel.load(Ordering::Relaxed) {
        unavailable_result("Cancelled")
    } else if !test_v6 {
        skipped_result(family)
    } else if let Some(ip) = ipv6_addr {
        test_ip_version(
            base_url, hostname, port, ip, user_agent, interface, bind_ip, cert_path, cancel,
        )
        .await
    } else {
        unavailable_result("No IPv6 address resolved")
    });

    Ok(IpVersionComparison {
        ipv4_result,
        ipv6_result,
    })
}

/// Result placeholder for a family that resolved no address.
fn unavailable_result(error: &str) -> IpVersionResult {
    IpVersionResult {
        ip_address: "N/A".to_string(),
        download_mbps: 0.0,
        upload_mbps: 0.0,
        latency_ms: 0.0,
        available: false,
        error: Some(error.to_string()),
    }
}

/// Result placeholder for a family skipped because of `--ipvX-only`.
fn skipped_result(family: Option<IpFamily>) -> IpVersionResult {
    let reason = family
        .map(|f| format!("Skipped ({} in effect)", f.flag()))
        .unwrap_or_else(|| "Skipped".to_string());
    unavailable_result(&reason)
}

/// Test a specific IP version by forcing requests to that IP.
#[allow(clippy::too_many_arguments)]
async fn test_ip_version(
    base_url: &str,
    hostname: &str,
    port: u16,
    ip: IpAddr,
    user_agent: &str,
    interface: Option<&str>,
    bind_ip: Option<IpAddr>,
    cert_path: Option<&std::path::Path>,
    cancel: &AtomicBool,
) -> IpVersionResult {
    use super::network_bind;

    let socket_addr = SocketAddr::new(ip, port);

    // Build a client that resolves hostname to specific IP
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(Duration::from_secs(30))
        .resolve(hostname, socket_addr);
    if let Some(path) = cert_path {
        match super::cert::load_reqwest_certificate(path) {
            Ok(cert) => builder = builder.add_root_certificate(cert),
            Err(e) => {
                return IpVersionResult {
                    ip_address: ip.to_string(),
                    download_mbps: 0.0,
                    upload_mbps: 0.0,
                    latency_ms: 0.0,
                    available: false,
                    error: Some(format!("Failed to load certificate: {}", e)),
                };
            }
        }
    }
    let client = match network_bind::apply_bind(builder, interface, bind_ip).build() {
        Ok(c) => c,
        Err(e) => {
            return IpVersionResult {
                ip_address: ip.to_string(),
                download_mbps: 0.0,
                upload_mbps: 0.0,
                latency_ms: 0.0,
                available: false,
                error: Some(format!("Failed to build client: {}", e)),
            };
        }
    };

    // Measure latency first
    let latency_ms = match measure_latency(&client, base_url, cancel).await {
        Ok(lat) => lat,
        Err(e) => {
            return IpVersionResult {
                ip_address: ip.to_string(),
                download_mbps: 0.0,
                upload_mbps: 0.0,
                latency_ms: 0.0,
                available: false,
                error: Some(format!("Latency test failed: {}", e)),
            };
        }
    };

    // Run abbreviated download test
    let download_mbps = match run_download_test(&client, base_url, TEST_DURATION, cancel).await {
        Ok(mbps) => mbps,
        Err(e) => {
            return IpVersionResult {
                ip_address: ip.to_string(),
                download_mbps: 0.0,
                upload_mbps: 0.0,
                latency_ms,
                available: false,
                error: Some(format!("Download test failed: {}", e)),
            };
        }
    };

    // Run abbreviated upload test
    let upload_mbps = match run_upload_test(&client, base_url, TEST_DURATION, cancel).await {
        Ok(mbps) => mbps,
        Err(e) => {
            return IpVersionResult {
                ip_address: ip.to_string(),
                download_mbps,
                upload_mbps: 0.0,
                latency_ms,
                available: true, // download worked
                error: Some(format!("Upload test failed: {}", e)),
            };
        }
    };

    IpVersionResult {
        ip_address: ip.to_string(),
        download_mbps,
        upload_mbps,
        latency_ms,
        available: true,
        error: None,
    }
}

/// Measure request latency on a warm connection: one throwaway request
/// establishes TCP+TLS, then the median of several tiny requests is reported.
/// This keeps the number comparable to the main test's probe latency instead
/// of ~3x the path RTT (connect + handshake + TTFB).
async fn measure_latency(
    client: &reqwest::Client,
    base_url: &str,
    cancel: &AtomicBool,
) -> Result<f64> {
    let url = format!("{}/__down?bytes=0", base_url);
    let warmup = client.get(&url).send().await?.error_for_status()?;
    let _ = warmup.bytes().await;

    let mut samples = Vec::with_capacity(crate::constants::IP_COMPARISON_LATENCY_PROBES);
    for _ in 0..crate::constants::IP_COMPARISON_LATENCY_PROBES {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let start = Instant::now();
        let resp = client.get(&url).send().await?.error_for_status()?;
        let _ = resp.bytes().await;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    crate::metrics::compute_sample_metrics(&samples)
        .map(|m| m.median)
        .ok_or_else(|| anyhow::anyhow!("no latency samples collected"))
}

/// Run abbreviated download test.
async fn run_download_test(
    client: &reqwest::Client,
    base_url: &str,
    duration: Duration,
    cancel: &AtomicBool,
) -> Result<f64> {
    let url = format!("{}/__down?bytes=5000000", base_url); // 5MB chunks
    let start = Instant::now();
    let mut total_bytes: u64 = 0;

    while start.elapsed() < duration && !cancel.load(Ordering::Relaxed) {
        let resp = client.get(&url).send().await?.error_for_status()?;
        let bytes = resp.bytes().await?;
        total_bytes += bytes.len() as u64;
    }

    let elapsed_secs = start.elapsed().as_secs_f64();
    let mbps = (total_bytes as f64 * 8.0) / (elapsed_secs * 1_000_000.0);
    Ok(mbps)
}

/// Run abbreviated upload test.
async fn run_upload_test(
    client: &reqwest::Client,
    base_url: &str,
    duration: Duration,
    cancel: &AtomicBool,
) -> Result<f64> {
    let url = format!("{}/__up", base_url);
    let upload_data = vec![0u8; 5_000_000]; // 5MB chunks
    let start = Instant::now();
    let mut total_bytes: u64 = 0;

    while start.elapsed() < duration && !cancel.load(Ordering::Relaxed) {
        client
            .post(&url)
            .body(upload_data.clone())
            .send()
            .await?
            .error_for_status()?;
        total_bytes += upload_data.len() as u64;
    }

    let elapsed_secs = start.elapsed().as_secs_f64();
    let mbps = (total_bytes as f64 * 8.0) / (elapsed_secs * 1_000_000.0);
    Ok(mbps)
}
