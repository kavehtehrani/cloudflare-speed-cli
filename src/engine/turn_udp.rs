use crate::engine::network_bind;
use crate::model::{ExperimentalUdpSummary, RunConfig, TestEvent, TurnInfo};
use crate::stats::{latency_summary_from_samples, OnlineStats};
use anyhow::{Context, Result};
use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Calculate Mean Opinion Score (MOS) using simplified ITU-T G.107 E-model.
/// (this is lifted from Claude I haven't verified it yet)
/// Returns a score from 1.0 (bad) to 4.5 (excellent).
fn calculate_mos(rtt_ms: f64, jitter_ms: f64, loss_pct: f64) -> Option<f64> {
    if rtt_ms.is_nan() || jitter_ms.is_nan() || loss_pct.is_nan() {
        return None;
    }
    if rtt_ms < 0.0 || jitter_ms < 0.0 || loss_pct < 0.0 {
        return None;
    }

    // One-way delay estimate (RTT/2 + jitter buffer approximation)
    let d = rtt_ms / 2.0 + 2.0 * jitter_ms;

    // Effective latency (capped at 177.3ms per E-model)
    let ld = d.min(177.3);

    // R-factor base calculation
    let mut r = 93.2 - (ld / 40.0);

    // Equipment impairment factor (Ie-eff) based on packet loss
    // Simplified model: loss impact increases with loss percentage
    let ie_eff = 30.0 * (loss_pct / 100.0).min(1.0);
    r -= ie_eff;

    // Clamp R to valid range [0, 100]
    r = r.clamp(0.0, 100.0);

    // Convert R-factor to MOS using standard formula
    let mos = if r < 0.0 {
        1.0
    } else if r > 100.0 {
        4.5
    } else {
        1.0 + 0.035 * r + 7.0e-6 * r * (r - 60.0) * (100.0 - r)
    };

    Some(mos.clamp(1.0, 5.0))
}

/// Determine quality label based on packet loss percentage.
fn quality_label(loss_pct: f64) -> &'static str {
    if loss_pct.is_nan() {
        return "Unknown";
    }
    match loss_pct {
        0.0 => "Excellent",
        x if x < 1.0 => "Good",
        x if x < 2.5 => "Acceptable",
        x if x < 5.0 => "Poor",
        _ => "Bad",
    }
}

// Minimal STUN binding request (RFC5389):
// - type: 0x0001
// - length: 0
// - magic cookie: 0x2112A442
// - transaction id: 12 bytes random
fn build_stun_binding_request(txid: [u8; 12]) -> [u8; 20] {
    let mut b = [0u8; 20];
    b[0] = 0x00;
    b[1] = 0x01;
    b[2] = 0x00;
    b[3] = 0x00;
    b[4] = 0x21;
    b[5] = 0x12;
    b[6] = 0xA4;
    b[7] = 0x42;
    b[8..20].copy_from_slice(&txid);
    b
}

fn is_stun_binding_response(buf: &[u8], txid: [u8; 12]) -> bool {
    if buf.len() < 20 {
        return false;
    }
    // binding success response
    if buf[0] != 0x01 || buf[1] != 0x01 {
        return false;
    }
    // magic cookie
    if buf[4] != 0x21 || buf[5] != 0x12 || buf[6] != 0xA4 || buf[7] != 0x42 {
        return false;
    }
    buf[8..20] == txid
}

fn pick_stun_target(turn: &TurnInfo) -> Option<String> {
    // Prefer stun: URLs. If none, try turn: with udp transport (might still answer binding).
    for u in &turn.urls {
        if u.starts_with("stun:") {
            return Some(u.clone());
        }
    }
    for u in &turn.urls {
        if u.starts_with("turn:") {
            return Some(u.clone());
        }
    }
    None
}

fn parse_host_port(url: &str) -> Result<(String, u16)> {
    // Accept forms:
    // - stun:host:port
    // - stun:host
    // - turn:host:port?transport=udp
    const DEFAULT_STUN_PORT: u16 = 3478;

    let (_, rest) = url.split_once(':').context("bad stun/turn url")?;
    let (hostport, _) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, port_str) = hostport.split_once(':').unwrap_or((hostport, ""));

    anyhow::ensure!(!host.is_empty(), "empty host in stun/turn url");

    let port = if port_str.is_empty() {
        DEFAULT_STUN_PORT
    } else {
        port_str
            .parse::<u16>()
            .context("invalid port in stun/turn url")?
    };

    Ok((host.to_string(), port))
}

pub async fn run_udp_like_loss_probe(
    turn: &TurnInfo,
    cfg: &RunConfig,
    event_tx: &mpsc::Sender<TestEvent>,
    pre_resolved: Option<SocketAddr>,
) -> Result<ExperimentalUdpSummary> {
    let target_url = pick_stun_target(turn).context("no stun/turn url in /__turn")?;
    let (host, port) = parse_host_port(&target_url)?;

    let addr: SocketAddr = if let Some(a) = pre_resolved {
        a
    } else {
        let mut addrs = tokio::net::lookup_host((host.as_str(), port)).await?;
        addrs.next().context("dns returned no addresses")?
    };

    // Bind UDP socket to interface or source IP if specified
    let sock = if cfg.interface.is_some() || cfg.source_ip.is_some() {
        let bind_addr =
            network_bind::resolve_bind_address(cfg.interface.as_ref(), cfg.source_ip.as_ref())?;

        if let Some(addr) = bind_addr {
            // Create socket using socket2 for binding
            let domain = socket2::Domain::for_address(addr);
            let socket =
                socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;

            // Bind to the specified address
            socket.bind(&socket2::SockAddr::from(addr))?;

            // Bind to interface if specified (Linux only)
            #[cfg(target_os = "linux")]
            if let Some(ref iface) = cfg.interface {
                use std::ffi::CString;
                use std::os::unix::io::AsRawFd;

                let ifname = CString::new(iface.as_str()).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid interface name")
                })?;

                unsafe {
                    if libc::setsockopt(
                        socket.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_BINDTODEVICE,
                        ifname.as_ptr() as *const libc::c_void,
                        ifname.as_bytes().len() as libc::socklen_t,
                    ) != 0
                    {
                        return Err(anyhow::anyhow!(
                            "Failed to bind to interface {}: {}",
                            iface,
                            std::io::Error::last_os_error()
                        ));
                    }
                }
            }

            // Convert to tokio UdpSocket
            let std_socket: std::net::UdpSocket = socket.into();
            std_socket.set_nonblocking(true)?;
            UdpSocket::from_std(std_socket)?
        } else {
            // Bind to appropriate address family based on target
            let bind_addr = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
            UdpSocket::bind(bind_addr).await?
        }
    } else {
        // Bind ephemeral UDP - match target address family
        let bind_addr = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        UdpSocket::bind(bind_addr).await?
    };

    sock.connect(addr).await?;

    let timeout = Duration::from_millis(600);
    let interval = Duration::from_millis(80);
    let attempts = 50u64;

    let mut sent = 0u64;
    let mut received = 0u64;
    let mut samples = Vec::<f64>::new();
    let mut online = OnlineStats::default();

    // Out-of-order tracking: map transaction ID to sequence number
    let mut txid_to_seq: HashMap<[u8; 12], u64> = HashMap::new();
    let mut next_expected_seq: u64 = 1;
    let mut out_of_order: u64 = 0;

    for seq in 1..=attempts {
        sent += 1;

        let mut txid = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut txid);
        txid_to_seq.insert(txid, seq);
        let pkt = build_stun_binding_request(txid);

        let start = std::time::Instant::now();
        let _ = sock.send(&pkt).await;

        let mut buf = [0u8; 1500];
        let recv = tokio::time::timeout(timeout, sock.recv(&mut buf)).await;
        match recv {
            Ok(Ok(n)) if is_stun_binding_response(&buf[..n], txid) => {
                received += 1;
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                samples.push(ms);
                online.push(ms);

                // Check for out-of-order: if this packet's seq < expected, it's reordered
                if let Some(&pkt_seq) = txid_to_seq.get(&txid) {
                    if pkt_seq < next_expected_seq {
                        out_of_order += 1;
                    } else {
                        // Update expected to next after this one
                        next_expected_seq = pkt_seq + 1;
                    }
                }

                event_tx
                    .send(TestEvent::UdpLossProgress {
                        sent,
                        received,
                        total: attempts,
                        rtt_ms: Some(ms),
                    })
                    .await
                    .ok();
            }
            _ => {
                // loss/timeout
                event_tx
                    .send(TestEvent::UdpLossProgress {
                        sent,
                        received,
                        total: attempts,
                        rtt_ms: None,
                    })
                    .await
                    .ok();
            }
        }

        tokio::time::sleep(interval).await;
    }

    let latency = latency_summary_from_samples(sent, received, &samples, online.stddev());

    // Calculate loss percentage
    let loss_pct = if sent == 0 {
        0.0
    } else {
        ((sent.saturating_sub(received)) as f64) * 100.0 / sent as f64
    };

    // Calculate out-of-order percentage (relative to received packets)
    let out_of_order_pct = if received == 0 {
        0.0
    } else {
        (out_of_order as f64) * 100.0 / received as f64
    };

    // Calculate MOS using median RTT, jitter, and loss
    let mos = latency.median_ms.and_then(|rtt| {
        latency
            .jitter_ms
            .and_then(|jitter| calculate_mos(rtt, jitter, loss_pct))
    });

    let label = quality_label(loss_pct);

    Ok(ExperimentalUdpSummary {
        target: Some(target_url),
        latency,
        out_of_order,
        out_of_order_pct,
        mos,
        quality_label: label.to_string(),
    })
}
