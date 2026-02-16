use crate::engine::network_bind;
use crate::model::{ExperimentalUdpSummary, RunConfig, TestEvent, TurnInfo};
use crate::stats::{latency_summary_from_samples, OnlineStats};
use anyhow::{Context, Result};
use rand::RngCore;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

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
            UdpSocket::bind("0.0.0.0:0").await?
        }
    } else {
        // Bind ephemeral UDP (default behavior)
        UdpSocket::bind("0.0.0.0:0").await?
    };

    sock.connect(addr).await?;

    let timeout = Duration::from_millis(600);
    let interval = Duration::from_millis(80);
    let attempts = 50u64;

    let mut sent = 0u64;
    let mut received = 0u64;
    let mut samples = Vec::<f64>::new();
    let mut online = OnlineStats::default();

    for _ in 0..attempts {
        sent += 1;

        let mut txid = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut txid);
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
    Ok(ExperimentalUdpSummary {
        target: Some(target_url),
        latency,
    })
}
