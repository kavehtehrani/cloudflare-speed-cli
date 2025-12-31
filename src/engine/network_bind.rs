use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};

/// Get the IP address of a network interface using the `if-addrs` crate
pub fn get_interface_ip(interface: &str) -> Result<IpAddr> {
    use if_addrs::get_if_addrs;

    let addrs = get_if_addrs().context("Failed to enumerate network interfaces")?;

    // Prefer IPv4 addresses
    for addr in &addrs {
        if addr.name == interface {
            if let if_addrs::IfAddr::V4(v4) = &addr.addr {
                return Ok(IpAddr::V4(v4.ip));
            }
        }
    }

    // Fallback to IPv6 if no IPv4 found
    for addr in &addrs {
        if addr.name == interface {
            if let if_addrs::IfAddr::V6(v6) = &addr.addr {
                return Ok(IpAddr::V6(v6.ip));
            }
        }
    }

    Err(anyhow::anyhow!(
        "Interface {} not found or has no IP address assigned",
        interface
    ))
}

/// Resolve binding address from interface name or source IP
pub fn resolve_bind_address(
    interface: Option<&String>,
    source_ip: Option<&String>,
) -> Result<Option<SocketAddr>> {
    if let Some(ip_str) = source_ip {
        let ip: IpAddr = ip_str.parse().context("Invalid source IP address format")?;
        return Ok(Some(SocketAddr::new(ip, 0)));
    }

    if let Some(iface) = interface {
        let ip = get_interface_ip(iface)
            .with_context(|| format!("Failed to get IP for interface {}", iface))?;
        return Ok(Some(SocketAddr::new(ip, 0)));
    }

    Ok(None)
}
