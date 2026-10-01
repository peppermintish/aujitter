//! One preset catalogue shared by CLI, HTTP and the native desktop.
use crate::config::Settings;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    #[default]
    Everyday,
    Gaming,
    Calls,
    Streaming,
    WifiCheck,
    DropoutHunt,
    Mobile,
    Satellite,
    Quiet,
    Vpn,
    Server,
    Custom,
}

#[derive(Clone, Serialize)]
#[non_exhaustive]
pub struct PresetInfo {
    pub id: Preset,
    pub name: &'static str,
    pub description: &'static str,
    pub interval_ms: u64,
    pub gaming_interval_ms: u64,
    pub timeout_ms: u64,
    pub latency_warning_ms: f64,
    pub jitter_warning_ms: f64,
    pub gaming: bool,
}

pub fn catalogue() -> Vec<PresetInfo> {
    use Preset::*;
    [
        (Everyday, "Everyday", "Balanced monitoring for most home connections. One cycle every 5 seconds.", 5000, 15000, 1000, 150.0, 30.0, false),
        (Gaming, "Gaming", "Low traffic during a match. One cycle every 15 seconds; router reads are skipped. These are test-target timings, not game-server timings.", 5000, 15000, 1000, 150.0, 30.0, true),
        (Calls, "Video calls & work", "Notice delay and variation that can disturb calls. Checks every 5 seconds; does not test audio or video throughput.", 5000, 15000, 1000, 100.0, 20.0, false),
        (Streaming, "Streaming", "Reachability checks every 10 seconds with more tolerant delay thresholds. Does not measure video bitrate or buffering.", 10000, 30000, 1000, 250.0, 60.0, false),
        (WifiCheck, "Wi-Fi investigation", "Compare local gateway and internet timings every 3 seconds. Run outside a match and compare Ethernet or another device.", 3000, 15000, 1000, 100.0, 20.0, false),
        (DropoutHunt, "Recurring dropouts", "Closer observation every 3 seconds with a 2-second timeout. Can still miss shorter events; use outside gaming.", 3000, 15000, 2000, 150.0, 30.0, false),
        (Mobile, "5G / 4G internet", "Checks every 10 seconds with mobile-friendly delay thresholds. Does not assert that the router's active connection is cellular.", 10000, 30000, 1500, 200.0, 60.0, false),
        (Satellite, "Satellite", "Allows higher expected satellite delay. Checks every 10 seconds with a 2-second timeout; tune for your service.", 10000, 30000, 2000, 900.0, 120.0, false),
        (Quiet, "Quiet / limited data", "Minimal checks every 60 seconds. Short interruptions may be missed. Gaming switch extends this to 120 seconds.", 60000, 120000, 1000, 200.0, 60.0, false),
        (Vpn, "VPN / work network", "Checks every 10 seconds with room for tunnel overhead. Does not bypass VPN policy or replace DNS; ICMP may be filtered.", 10000, 30000, 1500, 250.0, 60.0, false),
        (Server, "Unattended server", "Continuous reachability checks every 10 seconds. Keep the foreground service supervised by your host or Docker.", 10000, 30000, 1000, 150.0, 30.0, false),
    ].into_iter().map(|(id, name, description, interval_ms, gaming_interval_ms, timeout_ms, latency_warning_ms, jitter_warning_ms, gaming)| PresetInfo { id, name, description, interval_ms, gaming_interval_ms, timeout_ms, latency_warning_ms, jitter_warning_ms, gaming }).collect()
}

impl Preset {
    pub fn info(self) -> Option<PresetInfo> {
        catalogue().into_iter().find(|item| item.id == self)
    }
    pub fn name(self) -> &'static str {
        self.info().map(|item| item.name).unwrap_or("Custom")
    }
    pub fn apply(self, settings: &mut Settings) -> anyhow::Result<()> {
        let info = self.info().ok_or_else(|| {
            anyhow::anyhow!(
                "Custom is a label for edited preferences; choose a built-in preset to apply"
            )
        })?;
        settings.interval_ms = info.interval_ms;
        settings.gaming_interval_ms = info.gaming_interval_ms;
        settings.timeout_ms = info.timeout_ms;
        settings.latency_warning_ms = info.latency_warning_ms;
        settings.jitter_warning_ms = info.jitter_warning_ms;
        settings.gaming = info.gaming;
        settings.preset = self;
        // Preserve pause, network identity, overrides, targets, retention and router opt-in.
        settings.validate()
    }
    pub fn matches(self, settings: &Settings) -> bool {
        self.info().is_some_and(|info| {
            settings.interval_ms == info.interval_ms
                && settings.gaming_interval_ms == info.gaming_interval_ms
                && settings.timeout_ms == info.timeout_ms
                && settings.latency_warning_ms == info.latency_warning_ms
                && settings.jitter_warning_ms == info.jitter_warning_ms
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_preset_is_valid_and_preserves_user_network_privacy_and_pause() {
        for preset in catalogue() {
            let mut settings = Settings {
                paused: true,
                isp: Some("TPG".into()),
                discover_router: true,
                gateway: Some("192.168.1.1".parse().unwrap()),
                sample_retention_days: 7,
                ..Default::default()
            };
            preset.id.apply(&mut settings).unwrap();
            assert!(settings.paused && settings.discover_router);
            assert_eq!(settings.isp.as_deref(), Some("TPG"));
            assert_eq!(settings.gateway, Some("192.168.1.1".parse().unwrap()));
            assert_eq!(settings.sample_retention_days, 7);
            assert!(settings.gaming_interval_ms >= settings.interval_ms);
            assert!(preset.id.matches(&settings));
        }
        let mut settings = Settings::default();
        Preset::Gaming.apply(&mut settings).unwrap();
        assert!(settings.gaming);
        assert_eq!(settings.effective_interval_ms(), 15000);
        assert_eq!(settings.access_type, crate::model::AccessType::Unknown);
    }
}
