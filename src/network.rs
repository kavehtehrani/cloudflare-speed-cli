use crate::cli::Cli;
use crate::model::RunResult;
use serde_json::Value;
use std::process::Command;

/// Extracted metadata fields from Cloudflare response
#[derive(Debug, Clone, Default)]
pub struct ExtractedMetadata {
    pub ip: Option<String>,
    pub colo: Option<String>,
    pub asn: Option<String>,
    pub as_org: Option<String>,
}

/// Extract metadata fields (IP, colo, ASN, org) from Cloudflare JSON response.
/// Handles multiple possible field names for compatibility.
pub fn extract_metadata(meta: &Value) -> ExtractedMetadata {
    let ip = ["clientIp", "ip", "clientIP"]
        .iter()
        .find_map(|key| meta.get(*key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let colo = meta
        .get("colo")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let asn = meta.get("asn").and_then(|v| {
        v.as_i64()
            .map(|n| n.to_string())
            .or_else(|| v.as_str().map(|s| s.to_string()))
    });

    let as_org = ["asOrganization", "asnOrg"]
        .iter()
        .find_map(|key| meta.get(*key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    ExtractedMetadata {
        ip,
        colo,
        asn,
        as_org,
    }
}

/// Network information gathered from the system
pub struct NetworkInfo {
    pub interface_name: Option<String>,
    pub network_name: Option<String>,
    pub is_wireless: Option<bool>,
    pub interface_mac: Option<String>,
    pub link_speed_mbps: Option<u64>,
}

/// Gather network interface information based on CLI arguments
pub fn gather_network_info(args: &Cli) -> NetworkInfo {
    let (interface_name, network_name, is_wireless, interface_mac, link_speed_mbps) =
        if let Some(ref iface) = args.interface {
            // Use the specified interface
            let is_wireless = check_if_wireless(iface);
            let network_name = if is_wireless.unwrap_or(false) {
                get_wireless_ssid(iface)
            } else {
                None
            };
            let mac = get_interface_mac(iface);
            let speed = get_interface_speed(iface);
            (Some(iface.clone()), network_name, is_wireless, mac, speed)
        } else {
            // Auto-detect default interface
            gather_default_network_info()
        };

    NetworkInfo {
        interface_name,
        network_name,
        is_wireless,
        interface_mac,
        link_speed_mbps,
    }
}

/// Gather network interface information for the default interface
fn gather_default_network_info() -> (
    Option<String>,
    Option<String>,
    Option<bool>,
    Option<String>,
    Option<u64>,
) {
    // Get default interface by trying to connect to a remote address
    let interface_name = get_default_interface();

    if let Some(ref iface) = interface_name {
        let is_wireless = check_if_wireless(iface);
        let network_name = if is_wireless.unwrap_or(false) {
            get_wireless_ssid(iface)
        } else {
            None
        };
        let mac = get_interface_mac(iface);
        let speed = get_interface_speed(iface);
        (Some(iface.clone()), network_name, is_wireless, mac, speed)
    } else {
        (None, None, None, None, None)
    }
}

/// Get the default network interface name
fn get_default_interface() -> Option<String> {
    // Try to get interface from default route
    if let Ok(output) = Command::new("ip")
        .args(&["route", "show", "default"])
        .output()
    {
        if let Ok(output_str) = String::from_utf8(output.stdout) {
            // Look for "dev <interface>" in the output
            for line in output_str.lines() {
                if let Some(dev_pos) = line.find("dev ") {
                    let rest = &line[dev_pos + 4..];
                    return if let Some(space_pos) = rest.find(' ') {
                        Some(rest[..space_pos].to_string())
                    } else {
                        Some(rest.to_string())
                    };
                }
            }
        }
    }

    // Fallback: try to find first non-loopback interface
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str != "lo" && !name_str.starts_with("docker") && !name_str.starts_with("br-") {
                return Some(name_str.to_string());
            }
        }
    }

    None
}

/// Check if interface is wireless
fn check_if_wireless(iface: &str) -> Option<bool> {
    // Check if /sys/class/net/<iface>/wireless exists
    let wireless_path = format!("/sys/class/net/{}/wireless", iface);
    Some(std::path::Path::new(&wireless_path).exists())
}

/// Get wireless SSID for an interface
fn get_wireless_ssid(iface: &str) -> Option<String> {
    // Try iwgetid first (most reliable)
    if let Ok(output) = Command::new("iwgetid").arg("-r").arg(iface).output() {
        if let Ok(ssid) = String::from_utf8(output.stdout) {
            let ssid = ssid.trim().to_string();
            if !ssid.is_empty() {
                return Some(ssid);
            }
        }
    }

    // Fallback: try iw command
    if let Ok(output) = Command::new("iw").args(&["dev", iface, "info"]).output() {
        if let Ok(output_str) = String::from_utf8(output.stdout) {
            for line in output_str.lines() {
                if line.trim().starts_with("ssid ") {
                    let ssid = line.trim().strip_prefix("ssid ").unwrap_or("").trim();
                    if !ssid.is_empty() {
                        return Some(ssid.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Get MAC address of interface
fn get_interface_mac(iface: &str) -> Option<String> {
    let mac_path = format!("/sys/class/net/{}/address", iface);
    std::fs::read_to_string(mac_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Get link speed in Mbps
fn get_interface_speed(iface: &str) -> Option<u64> {
    let speed_path = format!("/sys/class/net/{}/speed", iface);
    std::fs::read_to_string(speed_path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&speed| speed > 0 && speed < 1_000_000) // Sanity check
}

/// Enrich RunResult with network information and metadata
pub fn enrich_result(result: &RunResult, network_info: &NetworkInfo) -> RunResult {
    let mut enriched = result.clone();

    // Add network interface information
    enriched.interface_name = network_info.interface_name.clone();
    enriched.network_name = network_info.network_name.clone();
    enriched.is_wireless = network_info.is_wireless;
    enriched.interface_mac = network_info.interface_mac.clone();
    enriched.link_speed_mbps = network_info.link_speed_mbps;

    // Extract metadata from result.meta if available
    if let Some(meta) = result.meta.as_ref() {
        let extracted = extract_metadata(meta);
        enriched.ip = extracted.ip;
        enriched.colo = extracted.colo;
        enriched.asn = extracted.asn;
        enriched.as_org = extracted.as_org;
    }

    // Server should already be set from RunResult.server, but preserve it
    // (no need to override)

    enriched
}
