use crate::{
    config::Settings,
    diagnosis::Analyzer,
    model::{
        Dashboard, Incident, MonitoringProfile, Probe, ProbeKind, ProbeState, Sample, Severity,
        Topology,
    },
    presets::{self, Preset},
    probe, router,
    store::Store,
    topology,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::{collections::VecDeque, net::IpAddr, sync::Arc, time::Duration};
use tokio::{
    sync::{RwLock, watch},
    task::JoinSet,
    time::Instant,
};

#[derive(Default)]
pub struct IncidentTracker {
    active: Option<Incident>,
    healthy_streak: u8,
}
impl IncidentTracker {
    pub fn observe(&mut self, sample: &Sample) {
        if sample.diagnosis.severity.is_unstable() {
            self.healthy_streak = 0;
            let active = self.active.get_or_insert_with(|| Incident {
                id: 0,
                started_at: sample.at,
                ended_at: None,
                peak_severity: sample.diagnosis.severity,
                unstable_samples: 0,
                diagnosis: sample.diagnosis.clone(),
                end_reason: None,
            });
            active.unstable_samples += 1;
            if sample.diagnosis.severity >= active.peak_severity {
                active.peak_severity = sample.diagnosis.severity;
                active.diagnosis = sample.diagnosis.clone();
            }
        } else if sample.diagnosis.severity == Severity::Stable {
            self.healthy_streak = self.healthy_streak.saturating_add(1).min(3);
            if self.healthy_streak >= 3
                && let Some(active) = &mut self.active
            {
                active.ended_at = Some(sample.at);
                active.end_reason = Some("three healthy samples".into());
            }
        } else {
            self.healthy_streak = 0;
        }
    }
    fn after_record(&mut self) {
        if self
            .active
            .as_ref()
            .is_some_and(|row| row.ended_at.is_some())
        {
            self.active = None;
        }
    }
}

pub fn initial_dashboard(settings: &Settings) -> Dashboard {
    Dashboard {
        app: "aujitter".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        started_at: Utc::now(),
        paused: settings.paused,
        gaming: settings.gaming,
        interval_ms: settings.effective_interval_ms(),
        preset: settings.preset,
        effective_preset: if settings.preset == Preset::Automatic {
            Preset::Everyday
        } else {
            settings.preset
        },
        preset_reason: (settings.preset == Preset::Automatic)
            .then(|| "Waiting for connection information. Using Everyday timing.".into()),
        latest: None,
        recent: vec![],
        incidents: vec![],
        hourly: vec![],
        error: None,
    }
}

pub fn refresh_profile(view: &mut Dashboard, settings: &Settings) -> Result<()> {
    let topology = view
        .latest
        .as_ref()
        .map(|row| row.topology.clone())
        .unwrap_or_default();
    let resolved = presets::resolve(settings, &topology)?;
    view.paused = settings.paused;
    view.gaming = settings.gaming;
    view.preset = settings.preset;
    view.effective_preset = resolved.settings().preset;
    view.preset_reason = resolved.reason().map(str::to_owned);
    view.interval_ms = resolved.settings().effective_interval_ms();
    Ok(())
}

pub async fn collect(settings: &Settings, topology: Topology, analyzer: &mut Analyzer) -> Sample {
    let at = Utc::now();
    let mut jobs = JoinSet::new();
    let timeout_ms = settings.timeout_ms;
    if let Some(gateway) = topology
        .gateway
        .as_ref()
        .and_then(|s| s.parse::<IpAddr>().ok())
    {
        jobs.spawn(probe::ping(ProbeKind::GatewayIcmp, gateway, timeout_ms));
    }
    jobs.spawn(probe::ping(
        ProbeKind::InternetIcmp,
        settings.internet_targets[0],
        timeout_ms,
    ));
    for target in &settings.internet_targets {
        jobs.spawn(probe::tcp(*target, timeout_ms));
    }
    if let Some(dns) = topology
        .dns_server
        .as_ref()
        .and_then(|s| s.parse::<IpAddr>().ok())
    {
        jobs.spawn(probe::dns(ProbeKind::Dns, dns, timeout_ms));
    }
    let mut probes = Vec::new();
    while let Some(result) = jobs.join_next().await {
        match result {
            Ok(probe) => probes.push(probe),
            Err(e) => probes.push(Probe::failed(
                ProbeKind::InternetTcp,
                "probe task".into(),
                ProbeState::Unsupported,
                e.to_string(),
            )),
        }
    }
    if probes
        .iter()
        .any(|p| p.kind == ProbeKind::Dns && p.state == ProbeState::Failure)
    {
        let primary: IpAddr = "1.1.1.1".parse().unwrap();
        let control = if topology.dns_server.as_deref() == Some("1.1.1.1") {
            "8.8.8.8".parse().unwrap()
        } else {
            primary
        };
        probes.push(probe::dns(ProbeKind::ControlDns, control, timeout_ms).await);
    }
    probes.sort_by_key(|p| format!("{:?}/{}", p.kind, p.target));
    let (metrics, diagnosis) = analyzer.observe(at, &topology, &probes, settings);
    Sample {
        at,
        interval_ms: settings.effective_interval_ms(),
        topology,
        probes,
        metrics,
        diagnosis,
        observation_gap_seconds: None,
        monitoring_profile: Some(MonitoringProfile {
            preset: settings.preset,
            reason: None,
            timeout_ms: settings.timeout_ms,
            latency_warning_ms: settings.latency_warning_ms,
            jitter_warning_ms: settings.jitter_warning_ms,
        }),
    }
}

pub async fn run(
    store: Arc<Store>,
    settings: Arc<RwLock<Settings>>,
    dashboard: Arc<RwLock<Dashboard>>,
    mut stop: watch::Receiver<bool>,
    max_samples: Option<u64>,
) -> Result<()> {
    let previous = store.latest()?;
    store.close_interrupted(
        previous.as_ref().map(|s| s.at).unwrap_or_else(Utc::now),
        "monitor restarted; later duration unobserved",
    )?;
    let mut analyzer = Analyzer::default();
    let mut incidents = IncidentTracker::default();
    let mut recent = VecDeque::new();
    let mut previous_at: Option<DateTime<Utc>> = None;
    let mut previous_interval_ms = 0;
    let mut samples = 0;
    let mut detected = Topology::default();
    let mut last_topology = Instant::now() - Duration::from_secs(60);
    let mut device: Option<router::RouterDevice> = None;
    let mut last_discovery = Instant::now() - Duration::from_secs(1800);
    let mut last_router_read = Instant::now() - Duration::from_secs(60);
    let mut last_prune = Instant::now() - Duration::from_secs(3600);
    let mut last_overrides = ((None, None), false);
    let mut was_paused = false;
    loop {
        if *stop.borrow() {
            break;
        }
        let started = Instant::now();
        let mut current = settings.read().await.clone();
        {
            let mut view = dashboard.write().await;
            refresh_profile(&mut view, &current)?;
        }
        if current.paused {
            if !was_paused {
                store.close_interrupted(previous_at.unwrap_or_else(Utc::now), "monitor paused")?;
                incidents = IncidentTracker::default();
                analyzer.reset();
                previous_at = None;
            }
            was_paused = true;
        } else {
            was_paused = false;
            let overrides = (current_overrides(&current), current.discover_router);
            if last_topology.elapsed() >= Duration::from_secs(30) || overrides != last_overrides {
                let cfg = current.clone();
                detected = tokio::task::spawn_blocking(move || topology::detect(&cfg)).await?;
                last_topology = Instant::now();
                last_overrides = overrides;
            }
            // Overrides apply immediately; OS topology is refreshed every 30 seconds.
            if let Some(gateway) = current.gateway {
                detected.gateway = Some(gateway.to_string());
            }
            if let Some(dns) = current.dns_server {
                detected.dns_server = Some(dns.to_string());
            }
            if device
                .as_ref()
                .is_some_and(|d| detected.gateway.as_deref() != Some(&d.gateway().to_string()))
            {
                device = None;
                last_discovery = Instant::now() - Duration::from_secs(1800);
            }
            detected.isp = current.isp.clone();
            detected.access_type = current.access_type;
            detected.access_evidence = if current.access_type == crate::model::AccessType::Unknown {
                "Active WAN technology is not exposed by the PC adapter.".into()
            } else {
                "User configured connection type.".into()
            };
            if current.discover_router
                && !current.gaming
                && last_discovery.elapsed() >= Duration::from_secs(1800)
            {
                if let Some(gateway) = detected.gateway.as_ref().and_then(|s| s.parse().ok()) {
                    match router::discover(gateway).await {
                        Ok(found) => device = found,
                        Err(error) => {
                            tracing::debug!(%error, "Optional router discovery unavailable")
                        }
                    }
                }
                last_discovery = Instant::now();
            }
            if current.discover_router
                && let Some(router) = &mut device
            {
                if !current.gaming && last_router_read.elapsed() >= Duration::from_secs(60) {
                    if let Err(error) = router::refresh(router).await {
                        tracing::debug!(%error, "Optional router status unavailable");
                    }
                    last_router_read = Instant::now();
                }
                detected.router_model = Some(router.model.clone());
                detected.router_wan_connected =
                    if !current.gaming && last_router_read.elapsed() < Duration::from_secs(120) {
                        router.wan_connected
                    } else {
                        None
                    };
                if current.access_type == crate::model::AccessType::Unknown {
                    detected.access_type = router.access_type;
                    detected.access_evidence = router.evidence.clone();
                }
            } else {
                detected.router_model = None;
                detected.router_wan_connected = None;
            }
            let configured_preset = current.preset;
            let resolved = presets::resolve(&current, &detected)?;
            current = resolved.settings().clone();
            {
                let mut view = dashboard.write().await;
                view.preset = configured_preset;
                view.effective_preset = current.preset;
                view.preset_reason = resolved.reason().map(str::to_owned);
                view.interval_ms = current.effective_interval_ms();
            }
            let gap = observation_gap(
                previous_at,
                Utc::now(),
                previous_interval_ms.max(current.effective_interval_ms()),
            );
            if gap.is_some() {
                store.close_interrupted(
                    previous_at.unwrap_or_else(Utc::now),
                    "observation gap; outage duration unknown",
                )?;
                incidents = IncidentTracker::default();
                analyzer.reset();
            }
            let mut sample = collect(&current, detected.clone(), &mut analyzer).await;
            if let Some(profile) = &mut sample.monitoring_profile {
                profile.reason = resolved.reason().map(str::to_owned);
            }
            let elapsed = previous_at
                .map(|at| sample.at.signed_duration_since(at).num_milliseconds() as f64 / 1000.0)
                .unwrap_or(0.0);
            sample.observation_gap_seconds = gap;
            incidents.observe(&sample);
            store.record(
                &sample,
                &mut incidents.active,
                if sample.observation_gap_seconds.is_some() {
                    0.0
                } else {
                    elapsed.max(0.0).min(previous_interval_ms as f64 / 1000.0)
                },
            )?;
            incidents.after_record();
            previous_at = Some(sample.at);
            previous_interval_ms = current.effective_interval_ms();
            tracing::info!(severity = ?sample.diagnosis.severity, area = %sample.diagnosis.area, latency_ms = ?sample.metrics.latency_ms, "Network sample recorded");
            recent.push_back(sample.clone());
            if recent.len() > 120 {
                recent.pop_front();
            }
            let mut view = dashboard.write().await;
            view.latest = Some(sample);
            view.recent = recent.iter().cloned().collect();
            view.incidents = store.incidents(100)?;
            view.hourly = store.hourly(24)?;
            view.error = None;
            drop(view);
            samples += 1;
            if max_samples.is_some_and(|max| samples >= max) {
                break;
            }
        }
        if last_prune.elapsed() >= Duration::from_secs(3600) {
            store.prune(
                Utc::now(),
                current.sample_retention_days,
                current.incident_retention_days,
            )?;
            last_prune = Instant::now();
        }
        // No backlog or catch-up burst after sleep, slow probes, or an overloaded machine.
        let next = started
            + Duration::from_millis(if current.paused {
                1000
            } else {
                current.effective_interval_ms()
            });
        tokio::select! { _ = tokio::time::sleep_until(next.max(Instant::now() + Duration::from_millis(100))) => {}, _ = stop.changed() => {} }
    }
    store.close_interrupted(previous_at.unwrap_or_else(Utc::now), "monitor stopped")?;
    Ok(())
}
fn current_overrides(settings: &Settings) -> (Option<IpAddr>, Option<IpAddr>) {
    (settings.gateway, settings.dns_server)
}
fn observation_gap(
    previous: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    interval_ms: u64,
) -> Option<f64> {
    let elapsed = now.signed_duration_since(previous?).num_milliseconds() as f64 / 1000.0;
    (elapsed < 0.0 || elapsed > interval_ms as f64 / 1000.0 * 3.0).then_some(elapsed.abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sleep_and_clock_change_are_gaps_not_measured_outages() {
        let at = Utc::now();
        assert!(observation_gap(Some(at), at + chrono::Duration::seconds(5), 5000).is_none());
        assert_eq!(
            observation_gap(Some(at), at + chrono::Duration::minutes(20), 5000),
            Some(1200.0)
        );
        assert_eq!(
            observation_gap(Some(at), at - chrono::Duration::seconds(1), 5000),
            Some(1.0)
        );
    }
    #[test]
    fn records_one_sample_incident_and_requires_stable_recovery() {
        let mut tracker = IncidentTracker::default();
        let mut sample = Sample {
            at: Utc::now(),
            interval_ms: 5000,
            topology: Topology::default(),
            probes: vec![],
            metrics: Default::default(),
            diagnosis: Default::default(),
            observation_gap_seconds: None,
            monitoring_profile: None,
        };
        sample.diagnosis.severity = Severity::Watch;
        tracker.observe(&sample);
        assert_eq!(tracker.active.as_ref().unwrap().unstable_samples, 1);
        sample.diagnosis.severity = Severity::Stable;
        for _ in 0..2 {
            sample.at += chrono::Duration::seconds(5);
            tracker.observe(&sample);
            assert!(tracker.active.as_ref().unwrap().ended_at.is_none());
        }
        sample.at += chrono::Duration::seconds(5);
        tracker.observe(&sample);
        assert!(tracker.active.as_ref().unwrap().ended_at.is_some());
        tracker.after_record();
        for _ in 0..1000 {
            tracker.observe(&sample);
        }
        assert!(tracker.active.is_none());
    }
}
