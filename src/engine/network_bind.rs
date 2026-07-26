use anyhow::{anyhow, Context, Result};
use reqwest::ClientBuilder;
use std::io;
use std::net::{IpAddr, SocketAddr};

/// Outbound IP protocol-version restriction for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpFamily {
    V4,
    V6,
}

impl IpFamily {
    pub fn matches(self, ip: IpAddr) -> bool {
        match self {
            IpFamily::V4 => ip.is_ipv4(),
            IpFamily::V6 => ip.is_ipv6(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IpFamily::V4 => "IPv4",
            IpFamily::V6 => "IPv6",
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            IpFamily::V4 => "--ipv4-only",
            IpFamily::V6 => "--ipv6-only",
        }
    }

    fn of(ip: IpAddr) -> IpFamily {
        if ip.is_ipv4() {
            IpFamily::V4
        } else {
            IpFamily::V6
        }
    }
}

pub fn resolve_ip_family(
    ipv4_only: bool,
    ipv6_only: bool,
    bind_ip: Option<IpAddr>,
) -> Result<Option<IpFamily>> {
    let explicit = match (ipv4_only, ipv6_only) {
        (true, true) => {
            return Err(anyhow!(
                "--ipv4-only and --ipv6-only cannot be used together"
            ))
        }
        (true, false) => Some(IpFamily::V4),
        (false, true) => Some(IpFamily::V6),
        (false, false) => None,
    };

    let implied = bind_ip.map(IpFamily::of);

    match (explicit, implied) {
        (Some(e), Some(i)) if e != i => Err(anyhow!(
            "{} was requested but the bound source address {} is {}",
            e.flag(),
            bind_ip.expect("implied family requires a bind IP"),
            i.label(),
        )),
        (Some(e), _) => Ok(Some(e)),
        (None, i) => Ok(i),
    }
}

pub async fn resolve_addrs_for_family(
    host: &str,
    port: u16,
    family: IpFamily,
) -> Result<Vec<SocketAddr>> {
    let target = format!("{}:{}", host, port);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&target)
        .await
        .with_context(|| format!("DNS lookup failed for {}", host))?
        .filter(|a| family.matches(a.ip()))
        .collect();

    if addrs.is_empty() {
        return Err(anyhow!(
            "no {} address resolved for {} ({} in effect)",
            family.label(),
            host,
            family.flag(),
        ));
    }

    Ok(addrs)
}

/// Apply local address binding to a reqwest client builder.
/// If `bind_ip` is Some, binds the client to that local address.
pub fn apply_local_address(builder: ClientBuilder, bind_ip: Option<IpAddr>) -> ClientBuilder {
    match bind_ip {
        Some(ip) => builder.local_address(ip),
        None => builder,
    }
}

/// Whether this platform can bind a socket to an interface *by name* — Linux via
/// `SO_BINDTODEVICE`, macOS via `IP_BOUND_IF`/`IPV6_BOUND_IF`. On these the OS
/// selects a source address per family, so `--interface` stays dual-stack.
/// Everywhere else (Windows, the BSDs) we fall back to binding the interface's
/// own IP (a single address family).
pub const fn device_binding_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}

/// The source address to bind for `--interface` on platforms without device
/// binding, where pinning traffic to an interface means binding one of its
/// addresses. `family` honors `--ipv4-only`/`--ipv6-only`; with no restriction
/// it prefers a routable IPv6 (the system's usual preference and faster path),
/// then IPv4. A link-local IPv6 is only ever a last resort. `None` if the
/// interface has no address of the required family.
pub fn interface_source_ip(interface: &str, family: Option<IpFamily>) -> Option<IpAddr> {
    let addrs = if_addrs::get_if_addrs().ok()?;
    let mut v4: Option<IpAddr> = None;
    let mut global_v6: Option<IpAddr> = None;
    let mut link_local_v6: Option<IpAddr> = None;
    for a in &addrs {
        if a.name != interface {
            continue;
        }
        match &a.addr {
            if_addrs::IfAddr::V4(v4a) => {
                v4.get_or_insert(IpAddr::V4(v4a.ip));
            }
            if_addrs::IfAddr::V6(v6) if crate::network::is_link_local_v6(&v6.ip) => {
                link_local_v6.get_or_insert(IpAddr::V6(v6.ip));
            }
            if_addrs::IfAddr::V6(v6) => {
                global_v6.get_or_insert(IpAddr::V6(v6.ip));
            }
        }
    }
    select_source_ip(family, v4, global_v6, link_local_v6)
}

/// Preference order for `interface_source_ip`, split out so it can be unit-tested
/// without a real interface. With no family restriction we prefer a routable
/// global IPv6, then IPv4; a link-local IPv6 is only ever a last resort, since a
/// bare `fe80::` source (no scope id) can't reach a global destination.
fn select_source_ip(
    family: Option<IpFamily>,
    v4: Option<IpAddr>,
    global_v6: Option<IpAddr>,
    link_local_v6: Option<IpAddr>,
) -> Option<IpAddr> {
    match family {
        Some(IpFamily::V4) => v4,
        Some(IpFamily::V6) => global_v6.or(link_local_v6),
        None => global_v6.or(v4).or(link_local_v6),
    }
}

/// Whether a network interface with the given name currently exists.
pub fn interface_exists(name: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new(&format!("/sys/class/net/{}", name)).exists() {
            return true;
        }
    }
    if_addrs::get_if_addrs()
        .map(|addrs| addrs.iter().any(|a| a.name == name))
        .unwrap_or(false)
}

/// Apply `--interface` / `--source` binding to a reqwest client builder.
///
/// On device-binding platforms `interface` uses `.interface()`, so the OS picks
/// a source address per family and the run stays dual-stack. `bind_ip` (from
/// `--source`, or the interface's resolved IP on platforms without device
/// binding) pins the local address to a single family. The two are mutually
/// exclusive at the CLI, so at most one applies.
pub fn apply_bind(
    builder: ClientBuilder,
    interface: Option<&str>,
    bind_ip: Option<IpAddr>,
) -> ClientBuilder {
    let builder = apply_local_address(builder, bind_ip);

    // `.interface()` exists only on the device-binding platforms; elsewhere the
    // interface was already resolved to `bind_ip` above.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(iface) = interface {
        return builder.interface(iface);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = interface;

    builder
}

/// Bind an already-created socket to a network interface by name: Linux uses
/// `SO_BINDTODEVICE`, macOS uses `IP_BOUND_IF`/`IPV6_BOUND_IF` (chosen by
/// `is_ipv6`). A no-op on every other platform (where the caller instead binds
/// the interface's source IP), so it can be called unconditionally.
#[cfg(target_os = "linux")]
pub fn bind_socket_to_device<S: std::os::unix::io::AsRawFd>(
    sock: &S,
    interface: &str,
    _is_ipv6: bool,
) -> io::Result<()> {
    let cname = std::ffi::CString::new(interface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid interface name"))?;
    let ret = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            cname.as_ptr() as *const libc::c_void,
            cname.as_bytes().len() as libc::socklen_t,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn bind_socket_to_device<S: std::os::unix::io::AsRawFd>(
    sock: &S,
    interface: &str,
    is_ipv6: bool,
) -> io::Result<()> {
    let cname = std::ffi::CString::new(interface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid interface name"))?;
    let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if idx == 0 {
        return Err(io::Error::last_os_error());
    }
    let (level, optname) = if is_ipv6 {
        (libc::IPPROTO_IPV6, libc::IPV6_BOUND_IF)
    } else {
        (libc::IPPROTO_IP, libc::IP_BOUND_IF)
    };
    let idx = idx as libc::c_int;
    let ret = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            level,
            optname,
            &idx as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn bind_socket_to_device<S>(_sock: &S, _interface: &str, _is_ipv6: bool) -> io::Result<()> {
    Ok(())
}

/// Reverse-lookup: find the interface name that owns a given IP address.
pub fn get_interface_for_ip(ip_str: &str) -> Option<String> {
    let target_ip: IpAddr = ip_str.parse().ok()?;
    let addrs = if_addrs::get_if_addrs().ok()?;

    for addr in &addrs {
        let iface_ip = match &addr.addr {
            if_addrs::IfAddr::V4(v4) => IpAddr::V4(v4.ip),
            if_addrs::IfAddr::V6(v6) => IpAddr::V6(v6.ip),
        };
        if iface_ip == target_ip {
            return Some(addr.name.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Name of the loopback interface on the current platform.
    /// Linux/Android call it "lo"; macOS and the BSDs call it "lo0".
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const LOOPBACK_IFACE: &str = "lo";
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const LOOPBACK_IFACE: &str = "lo0";

    #[test]
    fn test_interface_exists_loopback() {
        assert!(interface_exists(LOOPBACK_IFACE));
    }

    #[test]
    fn test_interface_exists_nonexistent() {
        assert!(!interface_exists("nonexistent_iface_xyz"));
    }

    #[test]
    fn test_get_interface_for_ip_loopback() {
        // 127.0.0.1 is bound to the loopback interface ("lo" on Linux, "lo0" on macOS/BSD)
        let iface = get_interface_for_ip("127.0.0.1");
        assert_eq!(iface, Some(LOOPBACK_IFACE.to_string()));
    }

    #[test]
    fn test_get_interface_for_ip_not_found() {
        // No interface should own this arbitrary IP
        let iface = get_interface_for_ip("198.51.100.99");
        assert_eq!(iface, None);
    }

    #[test]
    fn test_get_interface_for_ip_invalid() {
        let iface = get_interface_for_ip("not-an-ip");
        assert_eq!(iface, None);
    }

    #[test]
    fn test_interface_source_ip_loopback() {
        // Loopback has both 127.0.0.1 and ::1; ::1 is routable (not link-local).
        // Unrestricted prefers IPv6; --ipv4-only/--ipv6-only honor the family.
        let v4: IpAddr = "127.0.0.1".parse().unwrap();
        let v6: IpAddr = "::1".parse().unwrap();
        assert_eq!(interface_source_ip(LOOPBACK_IFACE, None), Some(v6));
        assert_eq!(
            interface_source_ip(LOOPBACK_IFACE, Some(IpFamily::V6)),
            Some(v6)
        );
        assert_eq!(
            interface_source_ip(LOOPBACK_IFACE, Some(IpFamily::V4)),
            Some(v4)
        );
    }

    #[test]
    fn test_interface_source_ip_nonexistent() {
        assert!(interface_source_ip("nonexistent_iface_xyz", None).is_none());
    }

    #[test]
    fn test_select_source_ip_prefers_routable_over_link_local() {
        let v4: IpAddr = "192.168.1.50".parse().unwrap();
        let global_v6: IpAddr = "2001:db8::1".parse().unwrap();
        let link_local: IpAddr = "fe80::1".parse().unwrap();

        // Unrestricted: a routable global IPv6 wins when present.
        assert_eq!(
            select_source_ip(None, Some(v4), Some(global_v6), Some(link_local)),
            Some(global_v6)
        );
        // Unrestricted with only a link-local IPv6: a usable IPv4 must win over
        // the unroutable fe80 source (the regression this guards against).
        assert_eq!(
            select_source_ip(None, Some(v4), None, Some(link_local)),
            Some(v4)
        );
        // Link-local is chosen only when nothing routable exists.
        assert_eq!(
            select_source_ip(None, None, None, Some(link_local)),
            Some(link_local)
        );
        // Explicit families honor the request.
        assert_eq!(
            select_source_ip(Some(IpFamily::V4), Some(v4), Some(global_v6), None),
            Some(v4)
        );
        assert_eq!(
            select_source_ip(
                Some(IpFamily::V6),
                Some(v4),
                Some(global_v6),
                Some(link_local)
            ),
            Some(global_v6)
        );
    }

    #[test]
    fn test_resolve_ip_family_no_restriction() {
        assert_eq!(resolve_ip_family(false, false, None).unwrap(), None);
    }

    #[test]
    fn test_resolve_ip_family_explicit_flags() {
        assert_eq!(
            resolve_ip_family(true, false, None).unwrap(),
            Some(IpFamily::V4)
        );
        assert_eq!(
            resolve_ip_family(false, true, None).unwrap(),
            Some(IpFamily::V6)
        );
    }

    #[test]
    fn test_resolve_ip_family_both_flags_conflict() {
        assert!(resolve_ip_family(true, true, None).is_err());
    }

    #[test]
    fn test_resolve_ip_family_implied_by_bind_ip() {
        let v4: IpAddr = "192.168.1.1".parse().unwrap();
        let v6: IpAddr = "::1".parse().unwrap();
        assert_eq!(
            resolve_ip_family(false, false, Some(v4)).unwrap(),
            Some(IpFamily::V4)
        );
        assert_eq!(
            resolve_ip_family(false, false, Some(v6)).unwrap(),
            Some(IpFamily::V6)
        );
    }

    #[test]
    fn test_resolve_ip_family_flag_agrees_with_bind_ip() {
        let v4: IpAddr = "192.168.1.1".parse().unwrap();
        assert_eq!(
            resolve_ip_family(true, false, Some(v4)).unwrap(),
            Some(IpFamily::V4)
        );
    }

    #[test]
    fn test_resolve_ip_family_flag_conflicts_with_bind_ip() {
        // --ipv6-only with a v4 source IP is contradictory.
        let v4: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(resolve_ip_family(false, true, Some(v4)).is_err());
        // --ipv4-only with a v6 source IP is contradictory.
        let v6: IpAddr = "::1".parse().unwrap();
        assert!(resolve_ip_family(true, false, Some(v6)).is_err());
    }

    #[test]
    fn test_ip_family_matches() {
        let v4: IpAddr = "8.8.8.8".parse().unwrap();
        let v6: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(IpFamily::V4.matches(v4));
        assert!(!IpFamily::V4.matches(v6));
        assert!(IpFamily::V6.matches(v6));
        assert!(!IpFamily::V6.matches(v4));
    }

    #[test]
    fn test_apply_local_address_none() {
        // Should build successfully without binding
        let builder = reqwest::Client::builder();
        let client = apply_local_address(builder, None).build();
        assert!(client.is_ok());
    }

    #[test]
    fn test_apply_local_address_some() {
        // Should build successfully with binding
        let builder = reqwest::Client::builder();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let client = apply_local_address(builder, Some(ip)).build();
        assert!(client.is_ok());
    }
}
