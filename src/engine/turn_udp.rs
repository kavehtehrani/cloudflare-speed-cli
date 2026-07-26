use crate::engine::network_bind::{self, IpFamily};
use crate::model::{ExperimentalUdpSummary, RunConfig, TestEvent, TurnInfo};
use crate::stats::{latency_summary_from_samples, OnlineStats};
use anyhow::{anyhow, Context, Result};
use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Calculate Mean Opinion Score (MOS) using a simplified ITU-T G.107 E-model.
/// Returns a score from 1.0 (bad) to 5.0 (excellent).
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

/// When `buf` is a STUN binding success response whose transaction id belongs
/// to a probe we sent, returns that probe's sequence number.
fn match_binding_response(buf: &[u8], txid_to_seq: &HashMap<[u8; 12], u64>) -> Option<u64> {
    if buf.len() < 20 {
        return None;
    }
    // binding success response
    if buf[0] != 0x01 || buf[1] != 0x01 {
        return None;
    }
    // magic cookie
    if buf[4] != 0x21 || buf[5] != 0x12 || buf[6] != 0xA4 || buf[7] != 0x42 {
        return None;
    }
    let mut txid = [0u8; 12];
    txid.copy_from_slice(&buf[8..20]);
    txid_to_seq.get(&txid).copied()
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

/// Tracks probe-response arrivals for the loss/reorder metrics: dedupes
/// duplicate responses per sequence number and counts an arrival as
/// out-of-order when an earlier-sequenced probe lands after a
/// later-sequenced one already has.
#[derive(Default)]
struct ArrivalTracker {
    arrived: std::collections::HashSet<u64>,
    max_arrived_seq: u64,
    out_of_order: u64,
}

impl ArrivalTracker {
    /// Records an arrival; returns false for duplicates.
    fn record(&mut self, seq: u64) -> bool {
        if !self.arrived.insert(seq) {
            return false;
        }
        if seq < self.max_arrived_seq {
            self.out_of_order += 1;
        } else {
            self.max_arrived_seq = seq;
        }
        true
    }
}

pub async fn run_udp_like_loss_probe(
    turn: &TurnInfo,
    cfg: &RunConfig,
    event_tx: &mpsc::Sender<TestEvent>,
    pre_resolved: Vec<SocketAddr>,
    family: Option<IpFamily>,
    cancel: &AtomicBool,
) -> Result<ExperimentalUdpSummary> {
    let target_url = pick_stun_target(turn).context("no stun/turn url in /__turn")?;
    let (host, port) = parse_host_port(&target_url)?;

    // Use prefetched addresses when available, otherwise resolve now.
    let resolved: Vec<SocketAddr> = if pre_resolved.is_empty() {
        tokio::net::lookup_host((host.as_str(), port))
            .await?
            .collect()
    } else {
        pre_resolved
    };

    if resolved.is_empty() {
        return Err(anyhow!("dns returned no addresses for {}", host));
    }

    // Keep only target addresses of the requested family. `family` already
    // folds in any bound source IP's family, and the match matters either way:
    // a UDP socket bound to a v4 source can't connect() to a v6 peer
    // (EAFNOSUPPORT) and vice versa.
    let candidates: Vec<SocketAddr> = match family {
        Some(f) => resolved
            .iter()
            .copied()
            .filter(|a| f.matches(a.ip()))
            .collect(),
        None => resolved,
    };

    if candidates.is_empty() {
        return Err(anyhow!(
            "no {} address resolved for {}",
            family.map(|f| f.label()).unwrap_or("usable"),
            host
        ));
    }

    let (sock, _addr) = bind_and_connect_udp(&candidates, cfg).await?;

    let timeout = crate::constants::UDP_PROBE_TIMEOUT;
    let interval = crate::constants::UDP_PROBE_INTERVAL;
    let attempts = cfg.udp_packets;

    let mut sent = 0u64;
    let mut received = 0u64;
    let mut samples = Vec::<f64>::new();
    let mut online = OnlineStats::default();

    let mut txid_to_seq: HashMap<[u8; 12], u64> = HashMap::new();
    let mut send_times: HashMap<u64, Instant> = HashMap::new();
    let mut tracker = ArrivalTracker::default();

    for seq in 1..=attempts {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        sent += 1;

        let mut txid = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut txid);
        txid_to_seq.insert(txid, seq);
        let pkt = build_stun_binding_request(txid);

        let start = Instant::now();
        send_times.insert(seq, start);
        let _ = sock.send(&pkt).await;

        // Read until this probe's deadline. A delayed response to an EARLIER
        // probe must not consume this slot (that would record two losses for
        // one late packet): credit it to its own sequence and keep waiting.
        let deadline = start + timeout;
        let mut got_current = false;
        let mut buf = [0u8; 1500];
        loop {
            let now = Instant::now();
            let Some(remaining) = deadline.checked_duration_since(now) else {
                break;
            };
            match tokio::time::timeout(remaining, sock.recv(&mut buf)).await {
                Ok(Ok(n)) => {
                    let Some(rx_seq) = match_binding_response(&buf[..n], &txid_to_seq) else {
                        continue; // unrelated datagram
                    };
                    if !tracker.record(rx_seq) {
                        continue; // duplicate response
                    }
                    received += 1;
                    let rtt_ms = send_times
                        .get(&rx_seq)
                        .map(|t0| t0.elapsed().as_secs_f64() * 1000.0);
                    if let Some(ms) = rtt_ms {
                        samples.push(ms);
                        online.push(ms);
                    }
                    if rx_seq == seq {
                        got_current = true;
                    }
                    event_tx
                        .send(TestEvent::UdpLossProgress {
                            sent,
                            received,
                            total: attempts,
                            rtt_ms,
                        })
                        .await
                        .ok();
                    if got_current {
                        break;
                    }
                }
                // Socket error or deadline elapsed.
                Ok(Err(_)) | Err(_) => break,
            }
        }

        if !got_current {
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

        tokio::time::sleep(interval).await;
    }

    let out_of_order = tracker.out_of_order;

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

/// Create a UDP socket honoring `--interface` / `--source`, then `connect()`
/// to the first candidate the kernel accepts. Returns the connected socket
/// and the address it ended up connected to. Each candidate must match the
/// bind IP family - the caller is expected to have already filtered.
async fn bind_and_connect_udp(
    candidates: &[SocketAddr],
    cfg: &RunConfig,
) -> Result<(UdpSocket, SocketAddr)> {
    // Source IP comes from --source (or the interface-IP fallback on platforms
    // without device binding); --interface itself is applied as a device bind in
    // build_udp_socket so dual-stack candidate selection still works.
    let bind_addr = cfg.resolved_bind_ip.map(|ip| SocketAddr::new(ip, 0));

    let mut last_err: Option<anyhow::Error> = None;
    for &addr in candidates {
        let sock = match build_udp_socket(addr, bind_addr, cfg.interface.as_deref()) {
            Ok(s) => s,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };

        match sock.connect(addr).await {
            Ok(()) => return Ok((sock, addr)),
            Err(e) => last_err = Some(anyhow!(e).context(format!("connect to {} failed", addr))),
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("no UDP candidates to try")))
}

/// Build a single UDP socket: bind to `bind_addr` (the source IP, or the
/// interface's resolved IP on platforms without device binding) when set,
/// otherwise an ephemeral wildcard bind matching the target family. When an
/// interface name is given on a device-binding platform, pin the socket to that
/// device so the kernel can't reroute the packets out a different NIC.
fn build_udp_socket(
    target: SocketAddr,
    bind_addr: Option<SocketAddr>,
    interface: Option<&str>,
) -> Result<UdpSocket> {
    let is_ipv6 = bind_addr.map(|a| a.is_ipv6()).unwrap_or(target.is_ipv6());

    let std_socket: std::net::UdpSocket = if let Some(addr) = bind_addr {
        let domain = socket2::Domain::for_address(addr);
        let socket =
            socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
        socket.bind(&socket2::SockAddr::from(addr))?;
        socket.into()
    } else {
        let any = if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        std::net::UdpSocket::bind(any)?
    };

    // Device binding where supported; a no-op otherwise (the interface was
    // already resolved to `bind_addr`).
    if let Some(iface) = interface {
        network_bind::bind_socket_to_device(&std_socket, iface, is_ipv6)
            .map_err(|e| anyhow!("Failed to bind to interface {}: {}", iface, e))?;
    }

    std_socket.set_nonblocking(true)?;
    Ok(UdpSocket::from_std(std_socket)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_mos_excellent_for_low_latency_no_loss() {
        let mos = calculate_mos(20.0, 2.0, 0.0).unwrap();
        assert!(mos >= 4.0, "expected excellent MOS, got {mos}");
    }

    #[test]
    fn calculate_mos_degrades_with_loss() {
        let good = calculate_mos(30.0, 5.0, 0.0).unwrap();
        let bad = calculate_mos(30.0, 5.0, 10.0).unwrap();
        assert!(bad < good);
    }

    #[test]
    fn calculate_mos_rejects_invalid_inputs() {
        assert!(calculate_mos(f64::NAN, 1.0, 0.0).is_none());
        assert!(calculate_mos(-1.0, 1.0, 0.0).is_none());
    }

    #[test]
    fn arrival_tracker_in_order_arrivals_are_not_reordered() {
        let mut t = ArrivalTracker::default();
        assert!(t.record(1));
        assert!(t.record(2));
        assert!(t.record(3));
        assert_eq!(t.out_of_order, 0);
    }

    #[test]
    fn arrival_tracker_counts_reordered_arrival() {
        let mut t = ArrivalTracker::default();
        t.record(1);
        t.record(3);
        t.record(2); // lands after 3 already arrived: reordered
        assert_eq!(t.out_of_order, 1);
    }

    #[test]
    fn arrival_tracker_ignores_duplicate_responses() {
        let mut t = ArrivalTracker::default();
        assert!(t.record(1));
        assert!(!t.record(1));
        assert_eq!(t.out_of_order, 0);
    }
}
