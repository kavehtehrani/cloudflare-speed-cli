//! IPv4 vs IPv6 comparison module
//!
//! Runs abbreviated speed tests on both IPv4 and IPv6 to compare performance.

use crate::model::{IpVersionComparison, IpVersionResult};
use anyhow::Result;
use reqwest::Url;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::lookup_host;

/// Duration for each abbreviated speed test (download/upload)
const TEST_DURATION: Duration = Duration::from_secs(3);

/// Run IPv4 vs IPv6 comparison tests.
///
/// Resolves the hostname to both IPv4 and IPv6 addresses, then runs
/// abbreviated speed tests on each protocol.
pub async fn compare_ip_versions(base_url: &str, user_agent: &str) -> Result<IpVersionComparison> {
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

    // Test IPv4
    let ipv4_result = if let Some(ip) = ipv4_addr {
        Some(test_ip_version(base_url, hostname, port, ip, user_agent).await)
    } else {
        Some(IpVersionResult {
            ip_address: "N/A".to_string(),
            download_mbps: 0.0,
            upload_mbps: 0.0,
            latency_ms: 0.0,
            available: false,
            error: Some("No IPv4 address resolved".to_string()),
        })
    };

    // Test IPv6
    let ipv6_result = if let Some(ip) = ipv6_addr {
        Some(test_ip_version(base_url, hostname, port, ip, user_agent).await)
    } else {
        Some(IpVersionResult {
            ip_address: "N/A".to_string(),
            download_mbps: 0.0,
            upload_mbps: 0.0,
            latency_ms: 0.0,
            available: false,
            error: Some("No IPv6 address resolved".to_string()),
        })
    };

    Ok(IpVersionComparison {
        ipv4_result,
        ipv6_result,
    })
}

/// Test a specific IP version by forcing requests to that IP.
async fn test_ip_version(
    base_url: &str,
    hostname: &str,
    port: u16,
    ip: IpAddr,
    user_agent: &str,
) -> IpVersionResult {
    let socket_addr = SocketAddr::new(ip, port);

    // Build a client that resolves hostname to specific IP
    let client = match reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(Duration::from_secs(30))
        .resolve(hostname, socket_addr)
        .build()
    {
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
    let latency_ms = match measure_latency(&client, base_url).await {
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
    let download_mbps = match run_download_test(&client, base_url, TEST_DURATION).await {
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
    let upload_mbps = match run_upload_test(&client, base_url, TEST_DURATION).await {
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

/// Measure latency to the server.
async fn measure_latency(client: &reqwest::Client, base_url: &str) -> Result<f64> {
    let url = format!("{}/__down?bytes=0", base_url);
    let start = Instant::now();
    let _resp = client.get(&url).send().await?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

/// Run abbreviated download test.
async fn run_download_test(
    client: &reqwest::Client,
    base_url: &str,
    duration: Duration,
) -> Result<f64> {
    let url = format!("{}/__down?bytes=5000000", base_url); // 5MB chunks
    let start = Instant::now();
    let mut total_bytes: u64 = 0;

    while start.elapsed() < duration {
        let resp = client.get(&url).send().await?;
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
) -> Result<f64> {
    let url = format!("{}/__up", base_url);
    let upload_data = vec![0u8; 5_000_000]; // 5MB chunks
    let start = Instant::now();
    let mut total_bytes: u64 = 0;

    while start.elapsed() < duration {
        let _resp = client.post(&url).body(upload_data.clone()).send().await?;
        total_bytes += upload_data.len() as u64;
    }

    let elapsed_secs = start.elapsed().as_secs_f64();
    let mbps = (total_bytes as f64 * 8.0) / (elapsed_secs * 1_000_000.0);
    Ok(mbps)
}
