use crate::cli::Cli;
use crate::engine::network_bind::IpFamily;
use crate::model::RunResult;
use serde_json::Value;
use std::process::Command;

/// Path to the legacy macOS airport CLI. Apple removed this binary in macOS 14.4 (Sonoma),
/// so it is only useful as a fallback on macOS 13 and earlier.
#[cfg(target_os = "macos")]
const MACOS_AIRPORT_PATH: &str =
    "/System/Library/PrivateFrameworks/Apple80211.framework/Versions/Current/Resources/airport";

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
    pub local_ipv4: Option<String>,
    pub local_ipv6: Option<String>,
}

/// Gather network interface information based on CLI arguments
pub fn gather_network_info(args: &Cli) -> NetworkInfo {
    // Determine the interface: explicit --interface, reverse-lookup from --source, or auto-detect
    let resolved_iface = args.interface.clone().or_else(|| {
        args.source
            .as_ref()
            .and_then(|ip| crate::engine::network_bind::get_interface_for_ip(ip))
    });

    let (interface_name, network_name, is_wireless, interface_mac) =
        if let Some(ref iface) = resolved_iface {
            let is_wireless = check_if_wireless(iface);
            let network_name = if is_wireless.unwrap_or(false) {
                get_wireless_ssid(iface)
            } else {
                None
            };
            let mac = get_interface_mac(iface);
            (Some(iface.clone()), network_name, is_wireless, mac)
        } else {
            // Auto-detect default interface. Under --ipv4-only / --ipv6-only,
            // look up the default route for that family so the reported
            // interface matches the one the test actually uses (these can be
            // different NICs). No --interface/--source here, so the family is
            // determined solely by the flags.
            let family = match (args.ipv4_only, args.ipv6_only) {
                (true, false) => Some(IpFamily::V4),
                (false, true) => Some(IpFamily::V6),
                _ => None,
            };
            gather_default_network_info(family)
        };

    let (local_ipv4, local_ipv6) = get_interface_ips(interface_name.as_deref());

    NetworkInfo {
        interface_name,
        network_name,
        is_wireless,
        interface_mac,
        local_ipv4,
        local_ipv6,
    }
}

/// Gather network interface information for the default interface.
///
/// `family` restricts the default-route lookup to a single address family
/// (from `--ipv4-only` / `--ipv6-only`); `None` keeps the platform default.
fn gather_default_network_info(
    family: Option<IpFamily>,
) -> (Option<String>, Option<String>, Option<bool>, Option<String>) {
    // Look up the default-route interface for the requested family.
    let interface_name = get_default_interface(family);

    if let Some(ref iface) = interface_name {
        let is_wireless = check_if_wireless(iface);
        let network_name = if is_wireless.unwrap_or(false) {
            get_wireless_ssid(iface)
        } else {
            None
        };
        let mac = get_interface_mac(iface);
        (Some(iface.clone()), network_name, is_wireless, mac)
    } else {
        (None, None, None, None)
    }
}

/// Get the default network interface name for the given address family.
/// `None` queries the IPv4 default route (the platform default).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn get_default_interface(family: Option<IpFamily>) -> Option<String> {
    // `ip -6 route show default` for IPv6; the v4 table otherwise.
    let route_args: &[&str] = if family == Some(IpFamily::V6) {
        &["-6", "route", "show", "default"]
    } else {
        &["route", "show", "default"]
    };

    // Try to get interface from default route
    if let Ok(output) = Command::new("ip").args(route_args).output() {
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

    // Fallback: first non-loopback interface. Only when no specific family was
    // requested, since this guess can't tell IPv4 from IPv6 interfaces and
    // would be misleading under --ipv4-only / --ipv6-only.
    if family.is_none() {
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str != "lo"
                    && !name_str.starts_with("docker")
                    && !name_str.starts_with("br-")
                {
                    return Some(name_str.to_string());
                }
            }
        }
    }

    None
}

#[cfg(target_os = "freebsd")]
fn get_default_interface(family: Option<IpFamily>) -> Option<String> {
    let output = Command::new("netstat")
        .args(&["-rn", "--libxo=json"])
        .output()
        .ok()?;

    let output_str = String::from_utf8(output.stdout).ok()?;
    parse_default_iface_from_netstat_json(&output_str, family)
}

/// Pick the default-route interface for `family` from FreeBSD
/// `netstat -rn --libxo=json` output.
///
/// netstat labels the families "Internet" (IPv4) and "Internet6" (IPv6).
/// Restricting to the requested one means that when the v4 and v6 default
/// routes live on different interfaces we return the one the test actually
/// uses, instead of whichever family netstat happens to list first.
///
/// Kept as a pure function (no command execution) and compiled in test builds
/// on every platform so the parsing/selection can be unit-tested without a
/// FreeBSD host.
#[cfg(any(target_os = "freebsd", test))]
fn parse_default_iface_from_netstat_json(json: &str, family: Option<IpFamily>) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;

    let af_matches = |af: Option<&str>| match family {
        Some(IpFamily::V4) => af == Some("Internet"),
        Some(IpFamily::V6) => af == Some("Internet6"),
        None => matches!(af, Some("Internet" | "Internet6")),
    };

    let families = v["statistics"]["route-information"]["route-table"]["rt-family"].as_array()?;

    for family_entry in families {
        if af_matches(family_entry["address-family"].as_str()) {
            if let Some(entries) = family_entry["rt-entry"].as_array() {
                for entry in entries {
                    if entry["destination"].as_str() == Some("default") {
                        return Some(entry["interface-name"].as_str()?.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "netbsd")]
fn get_default_interface(family: Option<IpFamily>) -> Option<String> {
    let netstat_args: &[&str] = if family == Some(IpFamily::V6) {
        &["-L", "-finet6", "-nr"]
    } else {
        &["-L", "-finet", "-nr"]
    };

    if let Ok(output) = Command::new("netstat").args(netstat_args).output() {
        if output.status.success() {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                for line in output_str.lines() {
                    let line = line.trim();
                    if line.starts_with("default") {
                        if let Some(iface) = line.split_whitespace().last() {
                            let iface = iface.trim().to_string();
                            if !iface.is_empty() {
                                return Some(iface);
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn get_default_interface(family: Option<IpFamily>) -> Option<String> {
    // `route -n get -inet6 default` for IPv6; the v4 default otherwise.
    let route_args: &[&str] = if family == Some(IpFamily::V6) {
        &["-n", "get", "-inet6", "default"]
    } else {
        &["-n", "get", "default"]
    };

    // Use `route -n get [-inet6] default` to find the default interface
    if let Ok(output) = Command::new("route").args(route_args).output() {
        if output.status.success() {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                for line in output_str.lines() {
                    let line = line.trim();
                    if line.starts_with("interface:") {
                        if let Some(iface) = line.splitn(2, ':').nth(1) {
                            let iface = iface.trim().to_string();
                            if !iface.is_empty() {
                                return Some(iface);
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: first non-loopback, non-tunnel interface. Only when no specific
    // family was requested, since this guess can't distinguish IPv4 from IPv6
    // interfaces and would mislead under --ipv4-only / --ipv6-only.
    if family.is_none() {
        if let Ok(interfaces) = if_addrs::get_if_addrs() {
            for iface in interfaces {
                if iface.is_loopback() {
                    continue;
                }
                // Skip common virtual/tunnel interfaces
                if iface.name.starts_with("utun")
                    || iface.name.starts_with("awdl")
                    || iface.name.starts_with("llw")
                    || iface.name.starts_with("bridge")
                {
                    continue;
                }
                return Some(iface.name);
            }
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn get_default_interface(family: Option<IpFamily>) -> Option<String> {
    // The IPv6 default route is ::/0; the IPv4 default route is 0.0.0.0/0.
    let dest_prefix = if family == Some(IpFamily::V6) {
        "::/0"
    } else {
        "0.0.0.0/0"
    };
    let route_cmd = format!(
        "Get-NetRoute -DestinationPrefix {} | Sort-Object RouteMetric | Select-Object -First 1 -ExpandProperty InterfaceAlias",
        dest_prefix
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", route_cmd.as_str()])
        .output()
        .ok()?;

    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Fallback: any active adapter. Only when no specific family was requested,
    // since this guess can't distinguish IPv4 from IPv6 interfaces.
    if family.is_none() {
        let output = Command::new("powershell")
            .args(&[
                "-NoProfile",
                "-Command",
                "Get-NetAdapter | Where-Object Status -eq 'Up' | Select-Object -First 1 -ExpandProperty InterfaceAlias",
            ])
            .output()
            .ok()?;

        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }

    None
}

/// Check if interface is wireless
#[cfg(any(target_os = "linux", target_os = "android"))]
fn check_if_wireless(iface: &str) -> Option<bool> {
    // Check if /sys/class/net/<iface>/wireless exists
    let wireless_path = format!("/sys/class/net/{}/wireless", iface);
    Some(std::path::Path::new(&wireless_path).exists())
}

#[cfg(target_os = "freebsd")]
fn check_if_wireless(iface: &str) -> Option<bool> {
    // An very quick and dirty wireless check for FreeBSD
    // but it works
    let output = Command::new("ifconfig")
        .args(&["-g", "wlan"])
        .output()
        .ok()?;
    let output_str = String::from_utf8(output.stdout).ok()?;

    let is_wireless = output_str.lines().any(|line| line.trim() == iface);

    Some(is_wireless)
}

#[cfg(target_os = "netbsd")]
fn check_if_wireless(iface: &str) -> Option<bool> {
    if let Ok(output) = Command::new("ifconfig").arg(iface).output() {
        if output.status.success() {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                for line in output_str.lines() {
                    let line = line.trim();
                    if line.starts_with("ssid ") {
                        return Some(true);
                    }
                }
            }
        }
    }

    Some(false)
}

#[cfg(target_os = "macos")]
fn check_if_wireless(iface: &str) -> Option<bool> {
    // Parse `networksetup -listallhardwareports` to check if the interface is Wi-Fi
    let output = Command::new("networksetup")
        .arg("-listallhardwareports")
        .output()
        .ok()?;
    let output_str = String::from_utf8(output.stdout).ok()?;

    let mut is_wifi_section = false;
    for line in output_str.lines() {
        let line = line.trim();
        if line.starts_with("Hardware Port:") {
            let port_name = line
                .splitn(2, ':')
                .nth(1)
                .unwrap_or("")
                .trim()
                .to_lowercase();
            is_wifi_section = port_name.contains("wi-fi") || port_name.contains("airport");
        } else if line.starts_with("Device:") {
            if let Some(device) = line.splitn(2, ':').nth(1) {
                if device.trim() == iface {
                    return Some(is_wifi_section);
                }
            }
        }
    }

    // Interface wasn't listed (e.g. utun/VPN); we don't know, so return None
    None
}

#[cfg(target_os = "windows")]
fn check_if_wireless(iface: &str) -> Option<bool> {
    let output = Command::new("netsh")
        .args(&["wlan", "show", "interfaces"])
        .output()
        .ok()?;

    if output.status.success() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        return Some(output_str.contains(iface));
    }
    Some(false)
}

/// Get wireless SSID for an interface
#[cfg(any(target_os = "linux", target_os = "android"))]
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
    if let Ok(output) = Command::new("iw").args(["dev", iface, "info"]).output() {
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

#[cfg(target_os = "freebsd")]
fn get_wireless_ssid(iface: &str) -> Option<String> {
    let output = Command::new("ifconfig").arg(iface).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let output_str = String::from_utf8_lossy(&output.stdout);

    for line in output_str.lines() {
        let mut tokens = line.split_whitespace();

        while let Some(tok) = tokens.next() {
            if tok == "ssid" || tok == "meshid" {
                if let Some(next_tok) = tokens.next() {
                    if next_tok.starts_with('"') {
                        // If the SSID contains whitespace, it's double quoted
                        // ssid "an example ssid" channel 1 ...
                        let mut ssid = next_tok.trim_start_matches('"').to_string();

                        for next_tok in tokens.by_ref() {
                            ssid.push(' ');
                            if next_tok.ends_with('"') {
                                ssid.push_str(next_tok.trim_end_matches('"'));
                                break;
                            } else {
                                ssid.push_str(next_tok);
                            }
                        }

                        return Some(ssid);
                    } else {
                        // No double quotation if no white space
                        // ssid an_example_ssid channel 1 ...
                        return Some(next_tok.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "netbsd")]
fn get_wireless_ssid(iface: &str) -> Option<String> {
    if let Ok(output) = Command::new("ifconfig").arg(iface).output() {
        if output.status.success() {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                for line in output_str.lines() {
                    let line = line.trim();
                    if line.starts_with("ssid ") {
                        let line = match line.find(" nwkey ") {
                            Some(pos) => &line[..pos],
                            None => line,
                        };
                        let line = match line.find("ssid ") {
                            Some(pos) => &line[pos + "ssid ".len()..],
                            None => line,
                        };

                        let ssid = line.trim().to_string();
                        let ssid = ssid.trim_matches('"').to_string();
                        if !ssid.is_empty() {
                            return Some(ssid);
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn get_wireless_ssid(iface: &str) -> Option<String> {
    // Try `networksetup -getairportnetwork <iface>` (public API)
    if let Ok(output) = Command::new("networksetup")
        .args(&["-getairportnetwork", iface])
        .output()
    {
        if let Ok(output_str) = String::from_utf8(output.stdout) {
            let output_str = output_str.trim();
            if let Some(ssid) = output_str.strip_prefix("Current Wi-Fi Network:") {
                let ssid = ssid.trim().to_string();
                if !ssid.is_empty() {
                    return Some(ssid);
                }
            }
        }
    }

    // Fallback: try the legacy airport command (removed in macOS 14.4, but works on older versions)
    if let Ok(output) = Command::new(MACOS_AIRPORT_PATH).arg("-I").output() {
        if let Ok(output_str) = String::from_utf8(output.stdout) {
            for line in output_str.lines() {
                let line = line.trim();
                if line.starts_with("SSID:") {
                    if let Some(ssid) = line.splitn(2, ':').nth(1) {
                        let ssid = ssid.trim().to_string();
                        if !ssid.is_empty() {
                            return Some(ssid);
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn get_wireless_ssid(iface: &str) -> Option<String> {
    let output = Command::new("netsh")
        .args(&["wlan", "show", "interfaces"])
        .output()
        .ok()?;

    if output.status.success() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        let mut current_iface = String::new();
        for line in output_str.lines() {
            let line = line.trim();
            if line.starts_with("Name") {
                if let Some(name) = line.split(':').nth(1) {
                    current_iface = name.trim().to_string();
                }
            }
            if current_iface == iface && line.starts_with("SSID") {
                if let Some(ssid) = line.split(':').nth(1) {
                    let ssid = ssid.trim().to_string();
                    if !ssid.is_empty() {
                        return Some(ssid);
                    }
                }
            }
        }
    }
    None
}

/// Get MAC address of interface
#[cfg(any(target_os = "linux", target_os = "android"))]
fn get_interface_mac(iface: &str) -> Option<String> {
    let mac_path = format!("/sys/class/net/{}/address", iface);
    std::fs::read_to_string(mac_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
// The same code works both for macOS and FreeBSD
fn get_interface_mac(iface: &str) -> Option<String> {
    // Use `ifconfig <iface>` and parse the `ether` line
    if let Ok(output) = Command::new("ifconfig").arg(iface).output() {
        if output.status.success() {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                for line in output_str.lines() {
                    let line = line.trim();
                    if line.starts_with("ether ") {
                        if let Some(mac) = line.split_whitespace().nth(1) {
                            return Some(mac.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(target_os = "netbsd")]
fn get_interface_mac(iface: &str) -> Option<String> {
    // Use `ifconfig <iface>` and parse the `address` line
    if let Ok(output) = Command::new("ifconfig").arg(iface).output() {
        if output.status.success() {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                for line in output_str.lines() {
                    let line = line.trim();
                    if line.starts_with("address: ") {
                        if let Some(mac) = line.split_whitespace().nth(1) {
                            return Some(mac.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn get_interface_mac(iface: &str) -> Option<String> {
    let output = Command::new("powershell")
        .args(&[
            "-NoProfile",
            "-Command",
            &format!("(Get-NetAdapter -Name '{}').LinkLayerAddress", iface),
        ])
        .output()
        .ok()?;

    if output.status.success() {
        let mac = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !mac.is_empty() {
            return Some(mac.replace('-', ":"));
        }
    }
    None
}

/// Get IPv4 and IPv6 addresses for an interface
fn get_interface_ips(interface_name: Option<&str>) -> (Option<String>, Option<String>) {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return (None, None);
    };

    let mut ipv4: Option<String> = None;
    let mut ipv6: Option<String> = None;

    for iface in interfaces {
        // If interface name is specified, only look at that interface
        if let Some(target) = interface_name {
            if iface.name != target {
                continue;
            }
        }

        // Skip loopback
        if iface.is_loopback() {
            continue;
        }

        match iface.addr {
            if_addrs::IfAddr::V4(ref addr) => {
                if ipv4.is_none() {
                    ipv4 = Some(addr.ip.to_string());
                }
            }
            if_addrs::IfAddr::V6(ref addr) => {
                // Skip link-local addresses (fe80::)
                let ip = addr.ip;
                if !ip.is_loopback() && !is_link_local_v6(&ip) && ipv6.is_none() {
                    ipv6 = Some(ip.to_string());
                }
            }
        }
    }

    (ipv4, ipv6)
}

/// Check if an IPv6 address is link-local (fe80::/10)
pub fn is_link_local_v6(ip: &std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    (segments[0] & 0xffc0) == 0xfe80
}

/// Enrich RunResult with network information and metadata
pub fn enrich_result(result: &RunResult, network_info: &NetworkInfo) -> RunResult {
    let mut enriched = result.clone();

    // Add network interface information
    enriched.interface_name = network_info.interface_name.clone();
    enriched.network_name = network_info.network_name.clone();
    enriched.is_wireless = network_info.is_wireless;
    enriched.interface_mac = network_info.interface_mac.clone();
    enriched.local_ipv4 = network_info.local_ipv4.clone();
    enriched.local_ipv6 = network_info.local_ipv6.clone();

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

#[cfg(test)]
mod tests {
    use super::*;

    /// FreeBSD `netstat -rn --libxo=json` with the v4 and v6 default routes on
    /// *separate* interfaces (em0 for IPv4, em1 for IPv6). This is the exact
    /// scenario the bug report describes.
    const NETSTAT_SEPARATE_IFACES: &str = r#"{
      "statistics": {
        "route-information": {
          "route-table": {
            "rt-family": [
              {
                "address-family": "Internet",
                "rt-entry": [
                  { "destination": "default", "gateway": "192.0.2.1", "interface-name": "em0" },
                  { "destination": "192.0.2.0/24", "interface-name": "em0" }
                ]
              },
              {
                "address-family": "Internet6",
                "rt-entry": [
                  { "destination": "default", "gateway": "fe80::1", "interface-name": "em1" },
                  { "destination": "fe80::/10", "interface-name": "em1" }
                ]
              }
            ]
          }
        }
      }
    }"#;

    #[test]
    fn freebsd_picks_ipv4_default_iface() {
        let iface =
            parse_default_iface_from_netstat_json(NETSTAT_SEPARATE_IFACES, Some(IpFamily::V4));
        assert_eq!(iface.as_deref(), Some("em0"));
    }

    #[test]
    fn freebsd_picks_ipv6_default_iface() {
        let iface =
            parse_default_iface_from_netstat_json(NETSTAT_SEPARATE_IFACES, Some(IpFamily::V6));
        assert_eq!(iface.as_deref(), Some("em1"));
    }

    #[test]
    fn freebsd_no_family_returns_first_default() {
        // With no restriction, fall back to the first family netstat lists.
        let iface = parse_default_iface_from_netstat_json(NETSTAT_SEPARATE_IFACES, None);
        assert_eq!(iface.as_deref(), Some("em0"));
    }

    #[test]
    fn freebsd_requested_family_absent_returns_none() {
        // Only an IPv4 default route exists; requesting IPv6 must not fall back
        // to the IPv4 interface.
        let v4_only = r#"{
          "statistics": { "route-information": { "route-table": { "rt-family": [
            { "address-family": "Internet", "rt-entry": [
              { "destination": "default", "interface-name": "em0" }
            ]}
          ]}}}
        }"#;
        assert_eq!(
            parse_default_iface_from_netstat_json(v4_only, Some(IpFamily::V6)),
            None
        );
        assert_eq!(
            parse_default_iface_from_netstat_json(v4_only, Some(IpFamily::V4)).as_deref(),
            Some("em0")
        );
    }

    #[test]
    fn freebsd_no_default_route_returns_none() {
        // A family present but with no "default" destination yields None.
        let no_default = r#"{
          "statistics": { "route-information": { "route-table": { "rt-family": [
            { "address-family": "Internet", "rt-entry": [
              { "destination": "192.0.2.0/24", "interface-name": "em0" }
            ]}
          ]}}}
        }"#;
        assert_eq!(
            parse_default_iface_from_netstat_json(no_default, Some(IpFamily::V4)),
            None
        );
    }

    #[test]
    fn freebsd_malformed_json_returns_none() {
        assert_eq!(
            parse_default_iface_from_netstat_json("not json", Some(IpFamily::V4)),
            None
        );
    }
}
