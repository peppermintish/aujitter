use crate::model::AccessType;
use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Settings {
    pub interval_ms: u64,
    pub gaming_interval_ms: u64,
    pub timeout_ms: u64,
    pub gaming: bool,
    pub paused: bool,
    pub sample_retention_days: u32,
    pub incident_retention_days: u32,
    pub gateway: Option<IpAddr>,
    pub dns_server: Option<IpAddr>,
    pub internet_targets: Vec<IpAddr>,
    pub isp: Option<String>,
    pub access_type: AccessType,
    pub discover_router: bool,
    pub latency_warning_ms: f64,
    pub jitter_warning_ms: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_ms: 5000,
            gaming_interval_ms: 15000,
            timeout_ms: 1000,
            gaming: false,
            paused: false,
            sample_retention_days: 30,
            incident_retention_days: 365,
            gateway: None,
            dns_server: None,
            internet_targets: vec!["1.1.1.1".parse().unwrap(), "8.8.8.8".parse().unwrap()],
            isp: None,
            access_type: AccessType::Unknown,
            discover_router: false,
            latency_warning_ms: 150.0,
            jitter_warning_ms: 30.0,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (3000..=300000).contains(&self.interval_ms),
            "Normal interval must be between 3 seconds and 5 minutes"
        );
        ensure!(
            (self.interval_ms..=300000).contains(&self.gaming_interval_ms),
            "Gaming interval must be at least the normal interval and at most 5 minutes"
        );
        ensure!(
            (200..=2000).contains(&self.timeout_ms),
            "Probe timeout must be between 200 and 2000 ms"
        );
        ensure!(
            self.internet_targets.len() >= 2 && self.internet_targets.len() <= 4,
            "Configure 2–4 independent internet targets"
        );
        let mut targets = self.internet_targets.clone();
        targets.sort();
        targets.dedup();
        ensure!(
            targets.len() == self.internet_targets.len(),
            "Internet targets must be distinct"
        );
        ensure!(
            self.internet_targets
                .iter()
                .all(|ip| !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast()),
            "Internet targets must be unicast addresses"
        );
        ensure!(
            self.sample_retention_days <= 3650 && self.incident_retention_days <= 3650,
            "Retention must be 0 (unlimited) or at most 3650 days"
        );
        ensure!(
            self.latency_warning_ms.is_finite()
                && self.latency_warning_ms >= 10.0
                && self.latency_warning_ms <= 10000.0,
            "Latency threshold must be 10–10000 ms"
        );
        ensure!(
            self.jitter_warning_ms.is_finite()
                && self.jitter_warning_ms >= 1.0
                && self.jitter_warning_ms <= 1000.0,
            "Jitter threshold must be 1–1000 ms"
        );
        ensure!(
            self.isp.as_ref().is_none_or(|s| s.len() <= 100),
            "ISP label must be at most 100 characters"
        );
        Ok(())
    }
    pub fn effective_interval_ms(&self) -> u64 {
        if self.gaming {
            self.gaming_interval_ms
        } else {
            self.interval_ms
        }
    }
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let settings: Self = if path.exists() {
            serde_json::from_slice(&std::fs::read(path)?).context("Invalid settings file")?
        } else {
            Self::default()
        };
        settings.validate()?;
        Ok(settings)
    }
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        // The lock held by the monitor serializes writers. Keep a recovery copy during replacement.
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temporary, path).context("Could not replace settings file")?;
        Ok(())
    }
}

pub fn data_dir() -> PathBuf {
    ProjectDirs::from("au", "AuJitter", "AuJitter")
        .map(|p| p.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".aujitter"))
}
pub fn default_config() -> PathBuf {
    data_dir().join("settings.json")
}
pub fn default_database() -> PathBuf {
    data_dir().join("history.sqlite3")
}
pub fn validate_listener(address: SocketAddr, token: Option<&str>) -> Result<()> {
    ensure!(
        address.ip().is_loopback() || token.is_some_and(|v| v.len() >= 24),
        "A non-loopback listener requires AUJITTER_TOKEN with at least 24 characters"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enforces_probe_budget() {
        let mut s = Settings {
            interval_ms: 100,
            ..Default::default()
        };
        assert!(s.validate().is_err());
        s.interval_ms = 5000;
        s.internet_targets[1] = s.internet_targets[0];
        assert!(s.validate().is_err());
    }
    #[test]
    fn remote_access_needs_token() {
        assert!(validate_listener("0.0.0.0:9876".parse().unwrap(), None).is_err());
        assert!(validate_listener("127.0.0.1:9876".parse().unwrap(), None).is_ok());
    }
}
