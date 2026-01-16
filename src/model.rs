use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;

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
    pub certificate_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    IdleLatency,
    Download,
    Upload,
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
    Info(InfoEvent),
    MetaInfo {
        meta: serde_json::Value,
    },
    RunCompleted {
        // Box to keep TestEvent size small; RunResult is large and would bloat the enum.
        result: Box<RunResult>,
    },
}

/// Structured info events emitted by the engine and consumed by UI/CLI layers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InfoEvent {
    // UI/CLI messages generated outside the engine.
    Message(String),
    BindInterface { iface: String, ip: IpAddr },
    BindSourceIp { ip: IpAddr },
    FetchingTurn,
}

impl InfoEvent {
    /// Render a human-readable message for UI/CLI layers.
    pub fn to_message(&self) -> String {
        match self {
            InfoEvent::Message(msg) => msg.clone(),
            InfoEvent::BindInterface { iface, ip } => {
                format!(
                    "Binding HTTP connections to interface {} (IP: {})",
                    iface, ip
                )
            }
            InfoEvent::BindSourceIp { ip } => {
                format!("Binding HTTP connections to source IP: {}", ip)
            }
            InfoEvent::FetchingTurn => "Fetching TURN info (experimental)".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencySummary {
    pub sent: u64,
    pub received: u64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
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
    pub link_speed_mbps: Option<u64>,
}
