//! DNS resolution time measurement module

use crate::model::DnsSummary;
use anyhow::{Context, Result};
use std::net::IpAddr;
use std::time::Instant;
use tokio::net::lookup_host;

/// Measure DNS resolution time for a given hostname.
///
/// Returns a `DnsSummary` containing the resolution time and resolved IP
/// addresses. When `family` restricts the run to one version (`--ipv4-only` /
/// `--ipv6-only`), only addresses of that family are reported, keeping the DNS
/// summary consistent with the rest of the run. The measured time still
/// reflects the single system resolver call (which returns all families).
pub async fn measure_dns_resolution(
    hostname: &str,
    family: Option<super::network_bind::IpFamily>,
) -> Result<DnsSummary> {
    // Get system DNS servers
    let dns_servers = get_system_dns_servers();

    // Ensure we have a port for lookup_host (required by the API)
    let lookup_target = if hostname.contains(':') {
        hostname.to_string()
    } else {
        format!("{}:443", hostname)
    };

    let start = Instant::now();
    let addrs: Vec<std::net::SocketAddr> = lookup_host(&lookup_target)
        .await
        .with_context(|| format!("DNS lookup failed for {}", hostname))?
        .collect();
    let elapsed = start.elapsed();

    let mut ipv4_count = 0;
    let mut ipv6_count = 0;
    let mut resolved_ips = Vec::new();

    for addr in &addrs {
        let ip = addr.ip();
        // Skip the disabled family under --ipv4-only / --ipv6-only.
        if let Some(f) = family {
            if !f.matches(ip) {
                continue;
            }
        }
        resolved_ips.push(ip.to_string());
        match ip {
            IpAddr::V4(_) => ipv4_count += 1,
            IpAddr::V6(_) => ipv6_count += 1,
        }
    }

    // Remove duplicates (same IP may appear with different ports)
    resolved_ips.sort();
    resolved_ips.dedup();

    Ok(DnsSummary {
        hostname: hostname.to_string(),
        resolution_time_ms: elapsed.as_secs_f64() * 1000.0,
        resolved_ips,
        ipv4_count,
        ipv6_count,
        dns_servers,
    })
}

/// Get the system's configured DNS servers.
///
/// On Linux/macOS: Parses /etc/resolv.conf
/// On Windows: Uses ipconfig command
fn get_system_dns_servers() -> Vec<String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        get_dns_from_resolv_conf()
    }

    #[cfg(target_os = "windows")]
    {
        get_dns_from_windows()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Parse /etc/resolv.conf for nameserver entries (Linux/macOS)
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn get_dns_from_resolv_conf() -> Vec<String> {
    use std::fs;

    let content = match fs::read_to_string("/etc/resolv.conf") {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let servers: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            // Skip comments and empty lines
            if line.starts_with('#') || line.is_empty() {
                return None;
            }
            // Look for "nameserver <ip>" lines
            if line.starts_with("nameserver") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    // Validate it's a valid IP address
                    if parts[1].parse::<IpAddr>().is_ok() {
                        return Some(parts[1].to_string());
                    }
                }
            }
            None
        })
        .collect();

    // If we only got 127.0.0.53 (systemd-resolved stub), try to get actual upstream servers
    if servers.len() == 1 && servers[0] == "127.0.0.53" {
        if let Some(upstream) = get_systemd_resolved_servers() {
            if !upstream.is_empty() {
                return upstream;
            }
        }
    }

    servers
}

/// Get actual DNS servers from systemd-resolved via resolvectl
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn get_systemd_resolved_servers() -> Option<Vec<String>> {
    use std::process::Command;

    let output = Command::new("resolvectl").arg("status").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut servers = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        // Look for "DNS Servers:" or "Current DNS Server:" lines
        if line.starts_with("DNS Servers:") || line.starts_with("Current DNS Server:") {
            if let Some(pos) = line.find(':') {
                let server_part = line[pos + 1..].trim();
                // May contain multiple servers separated by spaces
                for server in server_part.split_whitespace() {
                    if server.parse::<IpAddr>().is_ok() && !servers.contains(&server.to_string()) {
                        servers.push(server.to_string());
                    }
                }
            }
        }
    }

    if servers.is_empty() {
        None
    } else {
        Some(servers)
    }
}

/// Get DNS servers on Windows using ipconfig
#[cfg(target_os = "windows")]
fn get_dns_from_windows() -> Vec<String> {
    use std::process::Command;

    let output = match Command::new("ipconfig").arg("/all").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut dns_servers = Vec::new();
    let mut in_dns_section = false;

    for line in stdout.lines() {
        let line = line.trim();

        // Look for DNS Servers section
        if line.contains("DNS Servers") {
            in_dns_section = true;
            // The IP might be on the same line after the colon
            if let Some(pos) = line.find(':') {
                let ip_part = line[pos + 1..].trim();
                if !ip_part.is_empty() && ip_part.parse::<IpAddr>().is_ok() {
                    dns_servers.push(ip_part.to_string());
                }
            }
        } else if in_dns_section {
            // Additional DNS servers are indented on following lines
            if line.is_empty() || line.contains(':') {
                in_dns_section = false;
            } else if line.parse::<IpAddr>().is_ok() {
                dns_servers.push(line.to_string());
            }
        }
    }

    // Remove duplicates
    dns_servers.sort();
    dns_servers.dedup();
    dns_servers
}

/// Extract hostname from a URL string.
pub fn extract_hostname(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
}

/// Fetch external IPv4 and IPv6 addresses by making requests to Cloudflare.
/// Returns (ipv4, ipv6) - either may be None if not available.
///
/// When `family` restricts the run to one version (`--ipv4-only` /
/// `--ipv6-only`), the other family's probe is skipped entirely rather than
/// issued and left to time out.
pub async fn fetch_external_ips(
    base_url: &str,
    interface: Option<&str>,
    bind_ip: Option<std::net::IpAddr>,
    cert_path: Option<&std::path::Path>,
    family: Option<super::network_bind::IpFamily>,
) -> (Option<String>, Option<String>) {
    use super::network_bind::IpFamily;

    let hostname = match extract_hostname(base_url) {
        Some(h) => h,
        None => return (None, None),
    };

    // Resolve to get IPv4 and IPv6 addresses
    let url = format!("{}/__down?bytes=0", base_url);

    let want_v4 = family != Some(IpFamily::V6);
    let want_v6 = family != Some(IpFamily::V4);

    let (ipv4, ipv6) = tokio::join!(
        async {
            if want_v4 {
                fetch_external_ip_version(
                    &url,
                    &hostname,
                    IpVersion::V4,
                    interface,
                    bind_ip,
                    cert_path,
                )
                .await
            } else {
                None
            }
        },
        async {
            if want_v6 {
                fetch_external_ip_version(
                    &url,
                    &hostname,
                    IpVersion::V6,
                    interface,
                    bind_ip,
                    cert_path,
                )
                .await
            } else {
                None
            }
        }
    );

    (ipv4, ipv6)
}

#[derive(Clone, Copy)]
enum IpVersion {
    V4,
    V6,
}

async fn fetch_external_ip_version(
    url: &str,
    hostname: &str,
    version: IpVersion,
    interface: Option<&str>,
    bind_ip: Option<std::net::IpAddr>,
    cert_path: Option<&std::path::Path>,
) -> Option<String> {
    use super::network_bind;
    use std::net::SocketAddr;
    use std::time::Duration;

    // Resolve hostname to get IP addresses
    let lookup_target = format!("{}:443", hostname);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_target)
        .await
        .ok()?
        .collect();

    // Find an address of the requested version
    let target_addr = addrs.into_iter().find(|addr| match version {
        IpVersion::V4 => addr.is_ipv4(),
        IpVersion::V6 => addr.is_ipv6(),
    })?;

    // Build client that resolves to the specific IP
    let mut builder = reqwest::Client::builder()
        .resolve(hostname, target_addr)
        .timeout(Duration::from_secs(5));
    if let Some(path) = cert_path {
        builder = builder.add_root_certificate(super::cert::load_reqwest_certificate(path).ok()?);
    }
    let client = network_bind::apply_bind(builder, interface, bind_ip)
        .build()
        .ok()?;

    // Make request and extract IP from response headers
    let resp = client.get(url).send().await.ok()?;

    // Extract IP from cf-meta-ip header
    resp.headers()
        .get("cf-meta-ip")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_hostname() {
        assert_eq!(
            extract_hostname("https://speed.cloudflare.com"),
            Some("speed.cloudflare.com".to_string())
        );
        assert_eq!(
            extract_hostname("https://example.com:8080/path"),
            Some("example.com".to_string())
        );
        assert_eq!(extract_hostname("not a url"), None);
    }
}
