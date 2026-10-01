use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccessType {
    #[default]
    Unknown,
    FiveG,
    FourG,
    Fibre,
    Cable,
    Dsl,
    NbnFttp,
    NbnFttn,
    NbnFttc,
    NbnHfc,
    FixedWireless,
    Satellite,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct Topology {
    pub interface: Option<String>,
    pub local_transport: String,
    pub local_addresses: Vec<String>,
    pub gateway: Option<String>,
    pub dns_server: Option<String>,
    pub access_type: AccessType,
    pub access_evidence: String,
    pub router_model: Option<String>,
    pub router_wan_connected: Option<bool>,
    pub isp: Option<String>,
    pub container: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    GatewayIcmp,
    InternetIcmp,
    InternetTcp,
    Dns,
    ControlDns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Success,
    Failure,
    Unsupported,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Probe {
    pub kind: ProbeKind,
    pub target: String,
    pub state: ProbeState,
    pub latency_ms: Option<f64>,
    pub detail: Option<String>,
}

impl Probe {
    pub fn success(kind: ProbeKind, target: String, latency_ms: f64) -> Self {
        Self {
            kind,
            target,
            state: ProbeState::Success,
            latency_ms: Some(latency_ms),
            detail: None,
        }
    }
    pub fn failed(
        kind: ProbeKind,
        target: String,
        state: ProbeState,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            target,
            state,
            latency_ms: None,
            detail: Some(detail.into()),
        }
    }
    pub fn is_success(&self) -> bool {
        self.state == ProbeState::Success
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Unknown,
    Stable,
    Watch,
    Degraded,
    Offline,
}

impl Severity {
    pub fn is_unstable(self) -> bool {
        matches!(self, Self::Watch | Self::Degraded | Self::Offline)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Collecting evidence",
            Self::Stable => "Stable",
            Self::Watch => "Watch",
            Self::Degraded => "Degraded",
            Self::Offline => "Offline",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Diagnosis {
    pub severity: Severity,
    pub area: String,
    pub confidence: String,
    pub explanation: String,
    pub evidence: Vec<String>,
    pub next_steps: Vec<String>,
}

impl Default for Diagnosis {
    fn default() -> Self {
        Self {
            severity: Severity::Unknown,
            area: "unknown".into(),
            confidence: "insufficient".into(),
            explanation: "Waiting for the first network sample.".into(),
            evidence: vec![],
            next_steps: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct Metrics {
    pub latency_ms: Option<f64>,
    pub p95_latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub icmp_loss_percent: Option<f64>,
    pub tcp_failure_percent: Option<f64>,
    pub window_samples: usize,
    pub icmp_attempts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Sample {
    pub at: DateTime<Utc>,
    pub interval_ms: u64,
    pub topology: Topology,
    pub probes: Vec<Probe>,
    pub metrics: Metrics,
    pub diagnosis: Diagnosis,
    pub observation_gap_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Incident {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub peak_severity: Severity,
    pub unstable_samples: u64,
    pub diagnosis: Diagnosis,
    pub end_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HourlySummary {
    pub hour: String,
    pub samples: u64,
    pub unstable_samples: u64,
    pub offline_samples: u64,
    pub observed_seconds: f64,
    pub unstable_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Dashboard {
    pub app: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub paused: bool,
    pub gaming: bool,
    pub interval_ms: u64,
    pub latest: Option<Sample>,
    pub recent: Vec<Sample>,
    pub incidents: Vec<Incident>,
    pub hourly: Vec<HourlySummary>,
    pub error: Option<String>,
}
