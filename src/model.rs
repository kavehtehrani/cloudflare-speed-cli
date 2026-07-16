use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;

/// Latency `loss` is stored internally as a fraction (0.0–1.0) but serialized to
/// JSON as a percentage (0–100). Other percentage fields (e.g. `out_of_order_pct`)
/// are stored and serialized as percentages directly.
mod loss_percent_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(value * 100.0)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<f64, D::Error>
    where
        D: Deserializer<'de>,
    {
        let percent = f64::deserialize(deserializer)?;
        Ok(percent / 100.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub base_url: String,
    pub meas_id: String,
    #[serde(default)]
    pub comments: Option<String>,
    pub download_bytes_per_req: u64,
    pub upload_bytes_per_req: u64,
    pub concurrency: usize,
    #[serde(with = "humantime_serde")]
    pub idle_latency_duration: Duration,
    #[serde(with = "humantime_serde")]
    pub download_duration: Duration,
    #[serde(with = "humantime_serde")]
    pub upload_duration: Duration,
    pub probe_interval_ms: u64,
    pub probe_timeout_ms: u64,
    pub user_agent: String,
    pub experimental: bool,
    pub interface: Option<String>,
    pub source_ip: Option<String>,
    #[serde(skip)]
    pub resolved_bind_ip: Option<IpAddr>,
    pub proxy: Option<String>,
    pub certificate_path: Option<std::path::PathBuf>,
    // Diagnostic options
    pub measure_dns: bool,
    pub measure_tls: bool,
    pub compare_ip_versions: bool,
    pub traceroute: bool,
    pub traceroute_max_hops: u8,
    pub ipv4_only: bool,
    pub ipv6_only: bool,
    pub udp_packets: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    IdleLatency,
    Download,
    Upload,
    PacketLoss,
    Summary,
}

impl Phase {
    /// Convert phase to query string value for latency probes during throughput tests
    pub fn as_query_str(self) -> Option<&'static str> {
        match self {
            Phase::Download => Some("download"),
            Phase::Upload => Some("upload"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TestEvent {
    PhaseStarted {
        phase: Phase,
    },
    LatencySample {
        phase: Phase,
        during: Option<Phase>,
        rtt_ms: Option<f64>,
        ok: bool,
    },
    ThroughputTick {
        phase: Phase,
        bytes_total: u64,
        bps_instant: f64,
    },
    UdpLossProgress {
        sent: u64,
        received: u64,
        total: u64,
        rtt_ms: Option<f64>,
    },
    Info {
        message: String,
    },
    MetaInfo {
        meta: serde_json::Value,
    },
    // Diagnostic events
    DiagnosticDns {
        summary: DnsSummary,
    },
    DiagnosticTls {
        summary: TlsSummary,
    },
    DiagnosticIpComparison {
        comparison: IpVersionComparison,
    },
    TracerouteHop {
        hop_number: u8,
        hop: TracerouteHop,
    },
    TracerouteComplete {
        summary: TracerouteSummary,
    },
    ExternalIps {
        ipv4: Option<String>,
        ipv6: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencySummary {
    pub sent: u64,
    pub received: u64,
    #[serde(with = "loss_percent_serde")]
    pub loss: f64,
    pub min_ms: Option<f64>,
    pub mean_ms: Option<f64>,
    pub median_ms: Option<f64>,
    pub p25_ms: Option<f64>,
    pub p75_ms: Option<f64>,
    #[serde(default)]
    pub p95_ms: Option<f64>,
    #[serde(default)]
    pub p99_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
}

impl Default for LatencySummary {
    fn default() -> Self {
        Self {
            sent: 0,
            received: 0,
            loss: 0.0,
            min_ms: None,
            mean_ms: None,
            median_ms: None,
            p25_ms: None,
            p75_ms: None,
            p95_ms: None,
            p99_ms: None,
            max_ms: None,
            jitter_ms: None,
        }
    }
}

impl LatencySummary {
    /// Create a LatencySummary representing a failed/empty measurement
    pub fn failed() -> Self {
        Self {
            loss: 1.0,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputSummary {
    pub bytes: u64,
    pub duration_ms: u64,
    pub mbps: f64,
    pub mean_mbps: Option<f64>,
    pub median_mbps: Option<f64>,
    pub p25_mbps: Option<f64>,
    pub p75_mbps: Option<f64>,
    #[serde(default)]
    pub p95_mbps: Option<f64>,
    #[serde(default)]
    pub p99_mbps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnInfo {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentalUdpSummary {
    pub target: Option<String>,
    pub latency: LatencySummary,
    /// Count of out-of-order packets received
    #[serde(default)]
    pub out_of_order: u64,
    /// Percentage of packets received out of order
    #[serde(default)]
    pub out_of_order_pct: f64,
    /// Mean Opinion Score (1.0-5.0) for voice quality estimate
    #[serde(default)]
    pub mos: Option<f64>,
    /// Quality label based on packet loss: Excellent/Good/Acceptable/Poor/Bad
    #[serde(default)]
    pub quality_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub timestamp_utc: String,
    pub base_url: String,
    pub meas_id: String,
    #[serde(default)]
    pub comments: Option<String>,
    pub meta: Option<serde_json::Value>,
    #[serde(default)]
    pub server: Option<String>,
    pub idle_latency: LatencySummary,
    pub download: ThroughputSummary,
    pub upload: ThroughputSummary,
    pub loaded_latency_download: LatencySummary,
    pub loaded_latency_upload: LatencySummary,
    pub turn: Option<TurnInfo>,
    pub experimental_udp: Option<ExperimentalUdpSummary>,
    /// Error message when TURN fetch or UDP probe failed (for UI display)
    #[serde(skip, default)]
    pub udp_error: Option<String>,
    // Network information
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub colo: Option<String>,
    #[serde(default)]
    pub asn: Option<String>,
    #[serde(default)]
    pub as_org: Option<String>,
    #[serde(default)]
    pub interface_name: Option<String>,
    #[serde(default)]
    pub network_name: Option<String>,
    #[serde(default)]
    pub is_wireless: Option<bool>,
    #[serde(default)]
    pub interface_mac: Option<String>,
    #[serde(default)]
    pub local_ipv4: Option<String>,
    #[serde(default)]
    pub local_ipv6: Option<String>,
    #[serde(default)]
    pub external_ipv4: Option<String>,
    #[serde(default)]
    pub external_ipv6: Option<String>,
    // Diagnostic results
    #[serde(default)]
    pub dns: Option<DnsSummary>,
    #[serde(default)]
    pub tls: Option<TlsSummary>,
    #[serde(default)]
    pub ip_comparison: Option<IpVersionComparison>,
    #[serde(default)]
    pub traceroute: Option<TracerouteSummary>,
    #[serde(default)]
    pub connection_quality: Option<ConnectionQuality>,
}

#[cfg(test)]
pub(crate) fn empty_run_result() -> RunResult {
    RunResult {
        version: None,
        timestamp_utc: String::new(),
        base_url: String::new(),
        meas_id: String::new(),
        comments: None,
        meta: None,
        server: None,
        idle_latency: LatencySummary::default(),
        download: ThroughputSummary {
            bytes: 0,
            duration_ms: 0,
            mbps: 0.0,
            mean_mbps: None,
            median_mbps: None,
            p25_mbps: None,
            p75_mbps: None,
            p95_mbps: None,
            p99_mbps: None,
        },
        upload: ThroughputSummary {
            bytes: 0,
            duration_ms: 0,
            mbps: 0.0,
            mean_mbps: None,
            median_mbps: None,
            p25_mbps: None,
            p75_mbps: None,
            p95_mbps: None,
            p99_mbps: None,
        },
        loaded_latency_download: LatencySummary::default(),
        loaded_latency_upload: LatencySummary::default(),
        turn: None,
        experimental_udp: None,
        udp_error: None,
        ip: None,
        colo: None,
        asn: None,
        as_org: None,
        interface_name: None,
        network_name: None,
        is_wireless: None,
        interface_mac: None,
        local_ipv4: None,
        local_ipv6: None,
        external_ipv4: None,
        external_ipv6: None,
        dns: None,
        tls: None,
        ip_comparison: None,
        traceroute: None,
        connection_quality: None,
    }
}

// ============================================================================
// Diagnostic Structs
// ============================================================================

/// Summary of DNS resolution time measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsSummary {
    pub hostname: String,
    pub resolution_time_ms: f64,
    pub resolved_ips: Vec<String>,
    pub ipv4_count: usize,
    pub ipv6_count: usize,
    /// System DNS servers used for resolution
    #[serde(default)]
    pub dns_servers: Vec<String>,
}

/// Summary of TLS handshake time measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsSummary {
    pub handshake_time_ms: f64,
    pub protocol_version: Option<String>,
    pub cipher_suite: Option<String>,
}

/// Comparison of IPv4 vs IPv6 performance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpVersionComparison {
    pub ipv4_result: Option<IpVersionResult>,
    pub ipv6_result: Option<IpVersionResult>,
}

/// Result for a single IP version test
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpVersionResult {
    pub ip_address: String,
    pub download_mbps: f64,
    pub upload_mbps: f64,
    pub latency_ms: f64,
    pub available: bool,
    pub error: Option<String>,
}

/// Summary of traceroute results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracerouteSummary {
    pub destination: String,
    pub hops: Vec<TracerouteHop>,
    pub completed: bool,
}

/// A single hop in a traceroute
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracerouteHop {
    pub hop_number: u8,
    pub ip_address: Option<String>,
    pub hostname: Option<String>,
    pub rtt_ms: Vec<f64>,
    pub timeout: bool,
}

/// Derived connection-quality grades from a single run.
///
/// When one half is uncomputable but the other isn't:
/// `bufferbloat_grade == "-"` (with `bufferbloat_ms == None`) means no bloat grade;
/// `stability_grade == "-"` (with `stability_cv_pct == None`) means no stability grade.
/// When both halves are uncomputable, `RunResult.connection_quality` is `None` and this
/// struct is never constructed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionQuality {
    pub bufferbloat_grade: String,
    pub bufferbloat_ms: Option<f64>,
    pub stability_grade: String,
    pub stability_cv_pct: Option<f64>,
    pub stability_cv_download_pct: Option<f64>,
    pub stability_cv_upload_pct: Option<f64>,
}
