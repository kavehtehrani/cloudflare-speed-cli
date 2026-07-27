//! Traceroute functionality module
//!
//! Provides traceroute functionality to measure network path to Cloudflare edge.
//! Uses raw ICMP sockets when available (requires CAP_NET_RAW or root),
//! with fallback to system traceroute command.

use super::network_bind::IpFamily;
use crate::model::{TestEvent, TracerouteHop, TracerouteSummary};
use anyhow::{Context, Result};
use pnet_packet::icmp::IcmpTypes;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::ErrorKind;
use std::mem::MaybeUninit;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Number of probes per hop
const PROBES_PER_HOP: usize = 3;

/// Timeout for each probe
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Run traceroute to the destination.
///
/// Tries raw ICMP first, falls back to system traceroute if that fails.
/// `family` (from `--ipv4-only` / `--ipv6-only` or the bound source IP's
/// family) restricts which resolved address is probed. When `interface` is set
/// on Linux, `SO_BINDTODEVICE` keeps probes on that NIC.
pub async fn run_traceroute(
    destination: &str,
    max_hops: u8,
    event_tx: &mpsc::Sender<TestEvent>,
    bind_ip: Option<IpAddr>,
    interface: Option<&str>,
    family: Option<IpFamily>,
    cancel: Arc<AtomicBool>,
) -> Result<TracerouteSummary> {
    // Resolve destination to IP, honoring the requested family when set.
    // `to_socket_addrs` blocks, so keep it off the async runtime.
    let dest = destination.to_string();
    let ip = tokio::task::spawn_blocking(move || resolve_destination(&dest, family))
        .await
        .context("resolve task failed")??;

    // Try raw ICMP first
    match run_icmp_traceroute(&ip, max_hops, event_tx, bind_ip, interface, cancel.clone()).await {
        Ok(summary) => return Ok(summary),
        Err(e) => {
            if cancel.load(Ordering::Relaxed) {
                // Cancelled: don't launch the system tool as a "fallback".
                return Err(e);
            }
            // Send info about fallback
            let _ = event_tx
                .send(TestEvent::Info {
                    message: format!("ICMP traceroute unavailable ({}), using system command", e),
                })
                .await;
        }
    }

    // Fall back to system traceroute. `family` is forwarded so a --ipv4-only /
    // --ipv6-only restriction forces the matching family on the system tool.
    run_system_traceroute(
        destination,
        &ip,
        max_hops,
        event_tx,
        bind_ip,
        interface,
        family,
    )
    .await
}

/// Resolve destination hostname to IP address. When `family` is set, return an
/// address of that family; if none exists, error rather than returning an
/// address the run isn't allowed to reach.
fn resolve_destination(destination: &str, family: Option<IpFamily>) -> Result<IpAddr> {
    // Try to parse as IP first
    if let Ok(ip) = destination.parse::<IpAddr>() {
        return Ok(ip);
    }

    // Try DNS resolution
    let addrs: Vec<IpAddr> = format!("{}:0", destination)
        .to_socket_addrs()
        .with_context(|| format!("Failed to resolve {}", destination))?
        .map(|a| a.ip())
        .collect();

    if addrs.is_empty() {
        return Err(anyhow::anyhow!("No addresses found for {}", destination));
    }

    match family {
        Some(f) => addrs.into_iter().find(|a| f.matches(*a)).ok_or_else(|| {
            anyhow::anyhow!("No {} address resolved for {}", f.label(), destination)
        }),
        None => Ok(addrs.into_iter().next().unwrap()),
    }
}

/// Run traceroute using raw ICMP sockets (requires elevated privileges).
async fn run_icmp_traceroute(
    destination: &IpAddr,
    max_hops: u8,
    event_tx: &mpsc::Sender<TestEvent>,
    bind_ip: Option<IpAddr>,
    interface: Option<&str>,
    cancel: Arc<AtomicBool>,
) -> Result<TracerouteSummary> {
    // Check if we're dealing with IPv4 - IPv6 traceroute is more complex
    let dest_v4 = match destination {
        IpAddr::V4(v4) => *v4,
        IpAddr::V6(_) => {
            return Err(anyhow::anyhow!(
                "IPv6 traceroute not yet supported via raw sockets"
            ));
        }
    };

    // Refuse a v6 bind against a v4 destination - the socket would error on
    // bind() anyway, so fail fast with a clearer message.
    if let Some(IpAddr::V6(_)) = bind_ip {
        return Err(anyhow::anyhow!(
            "IPv6 source IP is incompatible with IPv4 destination"
        ));
    }

    // Try to create raw ICMP socket
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4))
        .context("Failed to create raw ICMP socket (need CAP_NET_RAW or root)")?;

    // Bind to source IP if requested so probes leave from --interface / --source.
    if let Some(IpAddr::V4(v4)) = bind_ip {
        socket
            .bind(&SocketAddr::new(IpAddr::V4(v4), 0).into())
            .with_context(|| format!("Failed to bind raw ICMP socket to {}", v4))?;
    }

    // Device-bind to the named interface where supported so the kernel can't
    // reroute the probes via another NIC. The raw socket is IPv4-only here, hence
    // is_ipv6 = false. On platforms without device binding this is a no-op; the
    // interface's IPv4 source was already bound via `bind_ip` above.
    if let Some(iface) = interface {
        super::network_bind::bind_socket_to_device(&socket, iface, false).map_err(|e| {
            anyhow::anyhow!(
                "Failed to bind raw ICMP socket to interface {}: {}",
                iface,
                e
            )
        })?;
    }

    socket.set_read_timeout(Some(PROBE_TIMEOUT))?;
    socket.set_nonblocking(false)?;

    let mut hops = Vec::new();
    let mut completed = false;
    let icmp_id = std::process::id() as u16;

    // Each hop's probe round does blocking sends/recvs (up to PROBES_PER_HOP x
    // PROBE_TIMEOUT), so it runs on the blocking pool to keep the async
    // runtime (and the TUI) responsive; the socket travels in and out.
    let mut sock = socket;
    for ttl in 1..=max_hops {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        sock.set_ttl(ttl as u32)?;

        let cancel_hop = cancel.clone();
        let (sock_back, round) = tokio::task::spawn_blocking(move || {
            let round = probe_hop(&sock, dest_v4, ttl, icmp_id, &cancel_hop);
            (sock, round)
        })
        .await
        .context("traceroute probe task failed")?;
        sock = sock_back;

        let hop = TracerouteHop {
            hop_number: ttl,
            ip_address: round.hop_ip.map(|ip| ip.to_string()),
            hostname: round.hop_ip.and_then(|ip| resolve_hostname(&ip)),
            rtt_ms: round.rtts,
            timeout: round.timeout && round.hop_ip.is_none(),
        };

        // Send hop event
        let _ = event_tx
            .send(TestEvent::TracerouteHop {
                hop_number: ttl,
                hop: hop.clone(),
            })
            .await;

        hops.push(hop);

        if round.reached_destination {
            completed = true;
            break;
        }
    }

    Ok(TracerouteSummary {
        destination: destination.to_string(),
        hops,
        completed,
    })
}

/// One hop's worth of probes, on a blocking socket.
struct HopRound {
    rtts: Vec<f64>,
    hop_ip: Option<IpAddr>,
    reached_destination: bool,
    timeout: bool,
}

fn probe_hop(
    socket: &Socket,
    dest_v4: std::net::Ipv4Addr,
    ttl: u8,
    icmp_id: u16,
    cancel: &AtomicBool,
) -> HopRound {
    let dest_addr = SocketAddr::new(IpAddr::V4(dest_v4), 0);
    let mut rtts = Vec::new();
    let mut hop_ip: Option<IpAddr> = None;
    let mut reached_destination = false;
    let mut timeout = false;

    for probe_num in 0..PROBES_PER_HOP {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let icmp_seq = ((ttl as u16) << 8) | (probe_num as u16);
        let packet = build_icmp_packet(icmp_id, icmp_seq);

        let start = Instant::now();
        if socket.send_to(&packet, &dest_addr.into()).is_err() {
            continue;
        }

        // Read until this probe's deadline, discarding ICMP traffic that is
        // not a reply to this exact probe (a raw socket sees everything
        // ICMP-shaped that reaches the host).
        let deadline = start + PROBE_TIMEOUT;
        loop {
            let now = Instant::now();
            let remaining = match deadline.checked_duration_since(now) {
                Some(r) if !r.is_zero() => r,
                _ => {
                    timeout = true;
                    break;
                }
            };
            if socket.set_read_timeout(Some(remaining)).is_err() {
                timeout = true;
                break;
            }

            let mut recv_buf: [MaybeUninit<u8>; 512] =
                unsafe { MaybeUninit::uninit().assume_init() };
            match socket.recv_from(&mut recv_buf) {
                Ok((len, from)) => {
                    // Safety: recv_from initialized the first `len` bytes.
                    let data: &[u8] =
                        unsafe { std::slice::from_raw_parts(recv_buf.as_ptr() as *const u8, len) };
                    let Some(is_echo_reply) = icmp_reply_matches(data, icmp_id, icmp_seq) else {
                        continue; // unrelated ICMP packet
                    };

                    rtts.push(start.elapsed().as_secs_f64() * 1000.0);

                    let from_addr: SocketAddr = from.as_socket().unwrap_or(dest_addr);
                    if hop_ip.is_none() {
                        hop_ip = Some(from_addr.ip());
                    }
                    if is_echo_reply || from_addr.ip() == IpAddr::V4(dest_v4) {
                        reached_destination = true;
                    }
                    break;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    timeout = true;
                    break;
                }
                Err(_) => {
                    timeout = true;
                    break;
                }
            }
        }
    }

    HopRound {
        rtts,
        hop_ip,
        reached_destination,
        timeout,
    }
}

/// Classify a raw IPv4 datagram as a reply to OUR probe (`id`, `seq`).
/// A raw ICMP socket receives every ICMP packet addressed to the host, so
/// anything that does not carry our id/seq (directly in an EchoReply, or
/// embedded in a TimeExceeded / DestinationUnreachable) must be ignored.
/// Returns `None` for unrelated packets, `Some(true)` for an EchoReply from
/// the destination (path complete), `Some(false)` for a hop reply.
fn icmp_reply_matches(packet: &[u8], id: u16, seq: u16) -> Option<bool> {
    fn be16(b: &[u8], i: usize) -> Option<u16> {
        Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
    }

    let ihl = ((*packet.first()? & 0x0f) as usize) * 4;
    if ihl < 20 {
        return None;
    }
    let icmp = packet.get(ihl..)?;
    let ty = *icmp.first()?;

    if ty == IcmpTypes::EchoReply.0 {
        return (be16(icmp, 4)? == id && be16(icmp, 6)? == seq).then_some(true);
    }
    if ty == IcmpTypes::TimeExceeded.0 || ty == IcmpTypes::DestinationUnreachable.0 {
        // The embedded original datagram starts after the 8-byte ICMP header:
        // its own IP header, then the first 8 bytes of our echo request.
        let inner_ip = icmp.get(8..)?;
        let inner_ihl = ((*inner_ip.first()? & 0x0f) as usize) * 4;
        if inner_ihl < 20 {
            return None;
        }
        let inner_icmp = inner_ip.get(inner_ihl..)?;
        if *inner_icmp.first()? != IcmpTypes::EchoRequest.0 {
            return None;
        }
        return (be16(inner_icmp, 4)? == id && be16(inner_icmp, 6)? == seq).then_some(false);
    }
    None
}

/// Build an ICMP echo request packet.
fn build_icmp_packet(id: u16, seq: u16) -> Vec<u8> {
    let mut packet = vec![0u8; 64];

    // ICMP header
    packet[0] = IcmpTypes::EchoRequest.0; // Type
    packet[1] = 0; // Code
    packet[2] = 0; // Checksum (will be calculated)
    packet[3] = 0;
    packet[4] = (id >> 8) as u8; // Identifier
    packet[5] = (id & 0xff) as u8;
    packet[6] = (seq >> 8) as u8; // Sequence number
    packet[7] = (seq & 0xff) as u8;

    // Payload (timestamp and padding)
    for (i, byte) in packet[8..64].iter_mut().enumerate() {
        *byte = i as u8;
    }

    // Calculate checksum
    let checksum = calculate_icmp_checksum(&packet);
    packet[2] = (checksum >> 8) as u8;
    packet[3] = (checksum & 0xff) as u8;

    packet
}

/// Calculate ICMP checksum.
fn calculate_icmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    while i < data.len() - 1 {
        sum += ((data[i] as u32) << 8) | (data[i + 1] as u32);
        i += 2;
    }

    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    !sum as u16
}

/// Try to resolve an IP address to a hostname.
fn resolve_hostname(_ip: &IpAddr) -> Option<String> {
    // Skip hostname resolution for now to keep it simple
    // In production, we'd want async reverse DNS resolution
    None
}

/// Fall back to system traceroute command.
async fn run_system_traceroute(
    destination: &str,
    destination_ip: &IpAddr,
    max_hops: u8,
    event_tx: &mpsc::Sender<TestEvent>,
    bind_ip: Option<IpAddr>,
    interface: Option<&str>,
    family: Option<IpFamily>,
) -> Result<TracerouteSummary> {
    // Clone strings to avoid lifetime issues with spawn_blocking
    let dest = destination.to_string();
    let dest_ip_str = destination_ip.to_string();

    // Only force an address family when a restriction is in effect
    // (--ipv4-only / --ipv6-only or a bound source IP). Without one we pass no
    // family flag and let the system tool choose, preserving prior behavior on
    // minimal traceroute builds that lack -4/-6.
    let force_v4 = family == Some(IpFamily::V4);
    let force_v6 = family == Some(IpFamily::V6);

    // Determine which command to use based on OS.
    // Note: -n / -d intentionally NOT passed so the OS resolves hostnames.
    // Source IP and interface flags differ per platform:
    //   - tracert (Windows):       -4/-6, -S <srcaddr>, no interface flag.
    //   - traceroute (Linux):      -4/-6, -i <interface>, -s <source>.
    //   - macOS/BSD:               separate traceroute6 binary for IPv6
    //                              (no -6 flag), -i and -s otherwise.
    let (cmd, args): (&'static str, Vec<String>) = if cfg!(target_os = "windows") {
        let mut args = vec!["-h".to_string(), max_hops.to_string()];
        if force_v6 {
            args.push("-6".to_string());
        } else if force_v4 {
            args.push("-4".to_string());
        }
        if let Some(ip) = bind_ip {
            args.push("-S".to_string());
            args.push(ip.to_string());
        }
        args.push(dest.clone());
        ("tracert", args)
    } else {
        // macOS and the BSDs ship a dedicated `traceroute6` rather than a
        // `-6` flag on `traceroute`; Linux's traceroute takes -4/-6 directly.
        let uses_separate_v6_binary = cfg!(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        ));

        let cmd = if force_v6 && uses_separate_v6_binary {
            "traceroute6"
        } else {
            "traceroute"
        };

        let mut args = vec![
            "-m".to_string(),
            max_hops.to_string(),
            "-q".to_string(),
            "3".to_string(),
        ];
        if !uses_separate_v6_binary {
            if force_v6 {
                args.push("-6".to_string());
            } else if force_v4 {
                args.push("-4".to_string());
            }
        }
        if let Some(iface) = interface {
            args.push("-i".to_string());
            args.push(iface.to_string());
        }
        if let Some(ip) = bind_ip {
            args.push("-s".to_string());
            args.push(ip.to_string());
        }
        args.push(dest.clone());
        (cmd, args)
    };

    let output = tokio::task::spawn_blocking(move || Command::new(cmd).args(&args).output())
        .await
        .context("Traceroute task failed")?
        .context("Failed to execute traceroute command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "traceroute exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let hops = parse_traceroute_output(&stdout, event_tx).await;

    let completed = hops
        .last()
        .map(|h| h.ip_address.as_deref() == Some(&dest_ip_str))
        .unwrap_or(false);

    Ok(TracerouteSummary {
        destination: destination.to_string(),
        hops,
        completed,
    })
}

/// Parse traceroute command output into hop structures.
async fn parse_traceroute_output(
    output: &str,
    event_tx: &mpsc::Sender<TestEvent>,
) -> Vec<TracerouteHop> {
    let mut hops = Vec::new();

    for line in output.lines() {
        let line = line.trim();

        // Skip header lines
        if line.is_empty()
            || line.starts_with("traceroute")
            || line.starts_with("Tracing")
            || line.contains("hops max")
        {
            continue;
        }

        // Parse hop line (format varies by OS)
        // Linux: " 1  192.168.1.1  0.123 ms  0.456 ms  0.789 ms"
        // macOS: " 1  192.168.1.1  0.123 ms  0.456 ms  0.789 ms"
        // Windows: "  1    <1 ms    <1 ms    <1 ms  192.168.1.1"

        if let Some(hop) = parse_hop_line(line) {
            let _ = event_tx
                .send(TestEvent::TracerouteHop {
                    hop_number: hop.hop_number,
                    hop: hop.clone(),
                })
                .await;
            hops.push(hop);
        }
    }

    hops
}

/// Parse a single hop line from traceroute output.
///
/// Handles three formats:
/// - Linux/macOS with DNS:    `1  host.name (1.2.3.4)  0.5 ms 0.4 ms 0.6 ms`
/// - Linux/macOS without DNS: `1  1.2.3.4  0.5 ms 0.4 ms 0.6 ms`
/// - Windows with DNS:        `1  <1 ms <1 ms <1 ms  host.name [1.2.3.4]`
fn parse_hop_line(line: &str) -> Option<TracerouteHop> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let hop_number: u8 = parts.first()?.parse().ok()?;

    if parts.iter().skip(1).all(|p| *p == "*") {
        return Some(TracerouteHop {
            hop_number,
            ip_address: None,
            hostname: None,
            rtt_ms: Vec::new(),
            timeout: true,
        });
    }

    let mut ip_address: Option<String> = None;
    let mut hostname: Option<String> = None;
    let mut rtts: Vec<f64> = Vec::new();
    let mut prev_candidate: Option<String> = None;

    for part in parts.iter().skip(1) {
        if *part == "ms" {
            continue;
        }

        // Numeric RTT (handles plain `0.5`, `0.5ms`, and Windows `<1`).
        let cleaned = part.trim_start_matches('<').trim_end_matches("ms");
        if let Ok(rtt) = cleaned.parse::<f64>() {
            rtts.push(rtt);
            prev_candidate = None;
            continue;
        }

        let was_wrapped = part.starts_with('(') || part.starts_with('[');
        let stripped = part
            .trim_start_matches(['(', '['])
            .trim_end_matches([')', ']']);

        if stripped.parse::<IpAddr>().is_ok() {
            if ip_address.is_none() {
                ip_address = Some(stripped.to_string());
                if was_wrapped {
                    if let Some(prev) = prev_candidate.take() {
                        if prev != stripped {
                            hostname = Some(prev);
                        }
                    }
                }
            }
            prev_candidate = None;
        } else {
            // Not an IP, not a number: candidate hostname for the next wrapped IP.
            prev_candidate = Some(part.to_string());
        }
    }

    if ip_address.is_none() && rtts.is_empty() {
        return None;
    }

    Some(TracerouteHop {
        hop_number,
        ip_address,
        hostname,
        rtt_ms: rtts,
        timeout: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Raw IPv4 datagram: header of `ihl_words` 32-bit words, then `payload`.
    fn ipv4_packet(ihl_words: u8, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; ihl_words as usize * 4];
        p[0] = 0x40 | ihl_words;
        p.extend_from_slice(payload);
        p
    }

    fn echo_reply(id: u16, seq: u16) -> Vec<u8> {
        let mut icmp = vec![0u8; 8];
        icmp[0] = IcmpTypes::EchoReply.0;
        icmp[4..6].copy_from_slice(&id.to_be_bytes());
        icmp[6..8].copy_from_slice(&seq.to_be_bytes());
        icmp
    }

    /// TimeExceeded embedding the original echo request with `id`/`seq`.
    fn time_exceeded(id: u16, seq: u16) -> Vec<u8> {
        let mut icmp = vec![0u8; 8];
        icmp[0] = IcmpTypes::TimeExceeded.0;
        let mut inner_ip = vec![0u8; 20];
        inner_ip[0] = 0x45;
        icmp.extend_from_slice(&inner_ip);
        let mut inner_icmp = vec![0u8; 8];
        inner_icmp[0] = IcmpTypes::EchoRequest.0;
        inner_icmp[4..6].copy_from_slice(&id.to_be_bytes());
        inner_icmp[6..8].copy_from_slice(&seq.to_be_bytes());
        icmp.extend_from_slice(&inner_icmp);
        icmp
    }

    #[test]
    fn accepts_matching_echo_reply() {
        let pkt = ipv4_packet(5, &echo_reply(42, 7));
        assert_eq!(icmp_reply_matches(&pkt, 42, 7), Some(true));
    }

    #[test]
    fn accepts_matching_time_exceeded_hop_reply() {
        let pkt = ipv4_packet(5, &time_exceeded(42, 7));
        assert_eq!(icmp_reply_matches(&pkt, 42, 7), Some(false));
    }

    #[test]
    fn rejects_echo_reply_for_another_process() {
        let pkt = ipv4_packet(5, &echo_reply(9999, 7));
        assert_eq!(icmp_reply_matches(&pkt, 42, 7), None);
    }

    #[test]
    fn rejects_time_exceeded_for_another_probe() {
        // Same id (our process) but a different probe's seq: this run's own
        // late reply from a previous TTL must not be credited to this hop.
        let pkt = ipv4_packet(5, &time_exceeded(42, 3));
        assert_eq!(icmp_reply_matches(&pkt, 42, 7), None);
    }

    #[test]
    fn handles_ip_header_with_options() {
        // IHL = 6 words (one option word): the ICMP header is NOT at byte 20.
        let pkt = ipv4_packet(6, &echo_reply(42, 7));
        assert_eq!(icmp_reply_matches(&pkt, 42, 7), Some(true));
    }

    #[test]
    fn rejects_truncated_packet() {
        assert_eq!(icmp_reply_matches(&[0x45, 0, 0], 42, 7), None);
        assert_eq!(icmp_reply_matches(&[], 42, 7), None);
    }

    #[test]
    fn parses_linux_with_hostname() {
        let line = " 1  host.example.com (1.2.3.4)  0.5 ms  0.4 ms  0.6 ms";
        let hop = parse_hop_line(line).unwrap();
        assert_eq!(hop.hop_number, 1);
        assert_eq!(hop.ip_address.as_deref(), Some("1.2.3.4"));
        assert_eq!(hop.hostname.as_deref(), Some("host.example.com"));
        assert_eq!(hop.rtt_ms, vec![0.5, 0.4, 0.6]);
        assert!(!hop.timeout);
    }

    #[test]
    fn parses_linux_without_dns() {
        let line = " 2  1.2.3.4  0.5 ms 0.4 ms 0.6 ms";
        let hop = parse_hop_line(line).unwrap();
        assert_eq!(hop.ip_address.as_deref(), Some("1.2.3.4"));
        assert_eq!(hop.hostname, None);
        assert_eq!(hop.rtt_ms, vec![0.5, 0.4, 0.6]);
    }

    #[test]
    fn parses_linux_when_hostname_equals_ip() {
        // When DNS fails, traceroute often shows `ip (ip)` with both being identical.
        let line = " 3  10.0.0.1 (10.0.0.1)  5.2 ms 4.8 ms 5.1 ms";
        let hop = parse_hop_line(line).unwrap();
        assert_eq!(hop.ip_address.as_deref(), Some("10.0.0.1"));
        assert_eq!(
            hop.hostname, None,
            "hostname should be elided when same as ip"
        );
    }

    #[test]
    fn parses_timeout_line() {
        let line = " 5  * * *";
        let hop = parse_hop_line(line).unwrap();
        assert_eq!(hop.ip_address, None);
        assert_eq!(hop.hostname, None);
        assert!(hop.timeout);
        assert!(hop.rtt_ms.is_empty());
    }

    #[test]
    fn parses_windows_with_hostname() {
        let line = "  1    <1 ms    <1 ms    <1 ms  router.local [192.168.1.1]";
        let hop = parse_hop_line(line).unwrap();
        assert_eq!(hop.ip_address.as_deref(), Some("192.168.1.1"));
        assert_eq!(hop.hostname.as_deref(), Some("router.local"));
        assert_eq!(hop.rtt_ms, vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn first_ip_wins_on_multi_router_hop() {
        // Some hops have two routers responding; we keep the first IP/hostname pair.
        let line =
            " 5  a.example.com (1.1.1.1)  260.2 ms b.example.com (2.2.2.2)  260.1 ms 260.0 ms";
        let hop = parse_hop_line(line).unwrap();
        assert_eq!(hop.ip_address.as_deref(), Some("1.1.1.1"));
        assert_eq!(hop.hostname.as_deref(), Some("a.example.com"));
        assert_eq!(hop.rtt_ms.len(), 3);
    }
}
