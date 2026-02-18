use serde::{Deserialize, Serialize};
use std::time::Duration;

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
