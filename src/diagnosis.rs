use crate::{
    config::Settings,
    model::{Diagnosis, Metrics, Probe, ProbeKind, ProbeState, Severity, Topology},
};
use chrono::{DateTime, Utc};
use std::collections::VecDeque;

#[derive(Default)]
pub struct Analyzer {
    history: VecDeque<(DateTime<Utc>, Vec<Probe>)>,
    route: String,
    gateway_replied: bool,
}

impl Analyzer {
    pub fn reset(&mut self) {
        self.history.clear();
        self.gateway_replied = false;
    }
    pub fn observe(
        &mut self,
        at: DateTime<Utc>,
        topology: &Topology,
        probes: &[Probe],
        settings: &Settings,
    ) -> (Metrics, Diagnosis) {
        let route = format!(
            "{:?}/{:?}/{:?}/{:?}",
            topology.interface, topology.gateway, topology.dns_server, settings.internet_targets
        );
        if self.route != route {
            self.reset();
            self.route = route;
        }
        self.gateway_replied |= probes
            .iter()
            .any(|p| p.kind == ProbeKind::GatewayIcmp && p.is_success());
        self.history.push_back((at, probes.to_vec()));
        while self.history.len() > 120
            || self
                .history
                .front()
                .is_some_and(|(time, _)| at.signed_duration_since(*time).num_seconds() > 300)
        {
            self.history.pop_front();
        }
        let mut latencies = Vec::new();
        let mut differences = Vec::new();
        let mut previous: Option<f64> = None;
        let mut echo_attempts = 0;
        let mut echo_failures = 0;
        let mut tcp_attempts = 0;
        let mut tcp_failures = 0;
        for (_, row) in &self.history {
            let echo = row.iter().find(|p| p.kind == ProbeKind::InternetIcmp);
            if let Some(p) = echo {
                if matches!(p.state, ProbeState::Success | ProbeState::Failure) {
                    echo_attempts += 1;
                    echo_failures += usize::from(!p.is_success());
                }
                if let Some(ms) = p.latency_ms {
                    latencies.push(ms);
                    if let Some(last) = previous {
                        differences.push((ms - last).abs());
                    }
                    previous = Some(ms);
                } else {
                    previous = None;
                }
            }
            for p in row.iter().filter(|p| {
                p.kind == ProbeKind::InternetTcp
                    && matches!(p.state, ProbeState::Success | ProbeState::Failure)
            }) {
                tcp_attempts += 1;
                tcp_failures += usize::from(!p.is_success());
            }
        }
        latencies.sort_by(f64::total_cmp);
        let metrics = Metrics {
            latency_ms: probes
                .iter()
                .find(|p| p.kind == ProbeKind::InternetIcmp)
                .and_then(|p| p.latency_ms),
            p95_latency_ms: if latencies.is_empty() {
                None
            } else {
                Some(latencies[((latencies.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)])
            },
            jitter_ms: (!differences.is_empty())
                .then(|| differences.iter().sum::<f64>() / differences.len() as f64),
            // No echo replies means we cannot distinguish filtering from loss.
            icmp_loss_percent: (!latencies.is_empty() && echo_attempts >= 3)
                .then(|| 100.0 * echo_failures as f64 / echo_attempts as f64),
            tcp_failure_percent: (tcp_attempts > 0)
                .then(|| 100.0 * tcp_failures as f64 / tcp_attempts as f64),
            window_samples: self.history.len(),
            icmp_attempts: echo_attempts,
        };
        let mut diagnosis = classify(topology, probes, &metrics, self.gateway_replied, settings);
        if topology.container {
            diagnosis
                .evidence
                .push("Measurements reflect this container's network viewpoint.".into());
        }
        (metrics, diagnosis)
    }
}

fn finding(
    severity: Severity,
    area: &str,
    confidence: &str,
    explanation: &str,
    evidence: Vec<String>,
    steps: &[&str],
) -> Diagnosis {
    Diagnosis {
        severity,
        area: area.into(),
        confidence: confidence.into(),
        explanation: explanation.into(),
        evidence,
        next_steps: steps.iter().map(|s| (*s).into()).collect(),
    }
}

fn classify(
    topology: &Topology,
    probes: &[Probe],
    metrics: &Metrics,
    gateway_replied: bool,
    settings: &Settings,
) -> Diagnosis {
    let tcp: Vec<_> = probes
        .iter()
        .filter(|p| p.kind == ProbeKind::InternetTcp)
        .collect();
    let tcp_ok = tcp.iter().filter(|p| p.is_success()).count();
    let gateway = probes.iter().find(|p| p.kind == ProbeKind::GatewayIcmp);
    let dns = probes.iter().find(|p| p.kind == ProbeKind::Dns);
    let control = probes.iter().find(|p| p.kind == ProbeKind::ControlDns);
    let echo = probes.iter().find(|p| p.kind == ProbeKind::InternetIcmp);
    let mut evidence = vec![format!(
        "{tcp_ok}/{} independent TCP targets replied.",
        tcp.len()
    )];
    if let Some(p) = gateway {
        evidence.push(format!(
            "Gateway echo: {:?}, latency {}.",
            p.state,
            p.latency_ms
                .map(|ms| format!("{ms:.1} ms"))
                .unwrap_or_else(|| "unavailable".into())
        ));
    }
    if let Some(p) = dns {
        evidence.push(format!("Configured DNS resolver: {:?}.", p.state));
    }
    if tcp.is_empty() {
        return Diagnosis::default();
    }
    if tcp_ok == 0 {
        if tcp.iter().any(|p| p.state == ProbeState::Unsupported) {
            return finding(
                Severity::Unknown,
                "probe_unavailable",
                "low",
                "Internet probes are unavailable. No outage can be established from this sample.",
                evidence,
                &["Check probe permissions and retry."],
            );
        }
        if topology.interface.is_none() && topology.gateway.is_none() {
            return finding(
                Severity::Offline,
                "local_link",
                "medium",
                "No default network interface was found, and internet probes failed.",
                evidence,
                &["Check Wi-Fi or the Ethernet cable and the OS network settings."],
            );
        }
        if gateway.is_some_and(Probe::is_success) {
            if topology.router_wan_connected == Some(false) {
                return finding(
                    Severity::Offline,
                    "router_wan",
                    "high",
                    "The router is reachable and reports its WAN connection is down. The cause may still be the access network or ISP.",
                    evidence,
                    &[
                        "Inspect the router's WAN or mobile signal page.",
                        "Compare the incident times with your ISP's outage information.",
                    ],
                );
            }
            return finding(
                Severity::Offline,
                "upstream",
                "medium",
                "The local gateway replied, but every internet TCP target failed. The interruption is beyond the local gateway; router WAN, access network, and ISP remain possible.",
                evidence,
                &[
                    "Check the router's WAN status and, for 5G, signal readings.",
                    "Compare with another device on the same router before contacting your ISP.",
                ],
            );
        }
        if gateway_replied && gateway.is_some_and(|p| p.state == ProbeState::Failure) {
            return finding(
                Severity::Offline,
                "local_network",
                "medium",
                "The previously reachable gateway and internet targets stopped replying. Wi-Fi, Ethernet, the router, or this device may be responsible.",
                evidence,
                &[
                    "Try Ethernet or another device to separate Wi-Fi/device issues from a router failure.",
                    "Check whether the router restarted.",
                ],
            );
        }
        return finding(
            Severity::Offline,
            "unknown",
            "low",
            "Internet TCP probes failed. Gateway echo is unavailable, so there is not enough evidence to locate the interruption.",
            evidence,
            &[
                "Check another device and the router's WAN status.",
                "Configure a known gateway if automatic route detection is unavailable.",
            ],
        );
    }
    if dns.is_some_and(|p| p.state == ProbeState::Failure) {
        let comparative = control.is_some_and(Probe::is_success);
        return finding(
            Severity::Degraded,
            "dns",
            if comparative { "high" } else { "low" },
            if comparative {
                "Internet TCP works, but the configured DNS resolver failed while an independent resolver answered."
            } else {
                "Internet TCP works, but DNS probes failed. DNS filtering, VPN policy, and resolver failure are possible."
            },
            evidence,
            &["Inspect DNS and VPN settings; compare another device's DNS results."],
        );
    }
    if gateway
        .and_then(|p| p.latency_ms)
        .is_some_and(|ms| ms > 50.0)
    {
        return finding(
            Severity::Degraded,
            "local_latency",
            "medium",
            "The local gateway reply took more than 50 ms. Wi-Fi contention, this device, and router response delays are possible; a slow echo reply alone cannot prove router failure.",
            evidence,
            &[
                "Compare Ethernet and another device at the same time.",
                "Check Wi-Fi signal, channel congestion, and competing uploads.",
            ],
        );
    }
    if tcp_ok != tcp.len() {
        return finding(
            Severity::Watch,
            "target_or_route",
            "low",
            "Some internet targets replied and others failed. This may be a target-specific or routing issue; it does not prove an ISP outage.",
            evidence,
            &["Look for repeated failures across independent targets and devices."],
        );
    }
    if echo.is_some_and(|p| p.state == ProbeState::Failure) && metrics.icmp_loss_percent.is_some() {
        return finding(
            Severity::Watch,
            "echo_loss_or_filtering",
            "low",
            "Internet TCP works, but a previously responsive internet echo target did not reply. Packet loss and ICMP filtering remain possible.",
            evidence,
            &[
                "Check whether latency spikes correlate with game lag; ICMP failures alone do not prove an outage.",
            ],
        );
    }
    if metrics
        .latency_ms
        .is_some_and(|ms| ms > settings.latency_warning_ms)
        || metrics
            .jitter_ms
            .is_some_and(|ms| ms > settings.jitter_warning_ms)
    {
        evidence.push(format!(
            "Thresholds: latency {:.0} ms; rolling jitter {:.0} ms.",
            settings.latency_warning_ms, settings.jitter_warning_ms
        ));
        return finding(
            Severity::Degraded,
            "latency",
            "medium",
            "Internet is reachable, but measured echo latency or variation exceeds the configured threshold. Congestion, Wi-Fi, mobile radio conditions, and upstream routing are possible.",
            evidence,
            &[
                "Compare Ethernet with Wi-Fi.",
                "Check other uploads/downloads and mobile signal quality.",
            ],
        );
    }
    finding(
        if metrics.window_samples < 3 {
            Severity::Unknown
        } else {
            Severity::Stable
        },
        "none_detected",
        "observed",
        if metrics.window_samples < 3 {
            "Internet targets are reachable. Collecting a short baseline."
        } else {
            "The monitored targets are reachable and no configured instability threshold is exceeded."
        },
        evidence,
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn probes(gateway_ok: bool, tcp_ok: usize) -> Vec<Probe> {
        let mut rows = vec![if gateway_ok {
            Probe::success(ProbeKind::GatewayIcmp, "192.168.1.1".into(), 1.0)
        } else {
            Probe::failed(
                ProbeKind::GatewayIcmp,
                "192.168.1.1".into(),
                ProbeState::Failure,
                "timeout",
            )
        }];
        for ix in 0..2 {
            rows.push(if ix < tcp_ok {
                Probe::success(ProbeKind::InternetTcp, format!("target{ix}"), 20.0)
            } else {
                Probe::failed(
                    ProbeKind::InternetTcp,
                    format!("target{ix}"),
                    ProbeState::Failure,
                    "timeout",
                )
            });
        }
        rows
    }
    fn topology() -> Topology {
        Topology {
            interface: Some("Ethernet".into()),
            gateway: Some("192.168.1.1".into()),
            ..Default::default()
        }
    }
    #[test]
    fn blocked_router_ping_does_not_imply_router_failure() {
        let d = classify(
            &topology(),
            &probes(false, 2),
            &Metrics::default(),
            false,
            &Settings::default(),
        );
        assert!(!d.severity.is_unstable());
    }
    #[test]
    fn distinguishes_local_and_upstream_but_never_proves_isp_blame() {
        let d = classify(
            &topology(),
            &probes(true, 0),
            &Metrics::default(),
            true,
            &Settings::default(),
        );
        assert_eq!(d.area, "upstream");
        assert_eq!(d.confidence, "medium");
        let d = classify(
            &topology(),
            &probes(false, 0),
            &Metrics::default(),
            true,
            &Settings::default(),
        );
        assert_eq!(d.area, "local_network");
        let d = classify(
            &topology(),
            &probes(false, 0),
            &Metrics::default(),
            false,
            &Settings::default(),
        );
        assert_eq!(d.area, "unknown");
    }
    #[test]
    fn a_single_target_failure_is_not_an_isp_outage() {
        assert_eq!(
            classify(
                &topology(),
                &probes(true, 1),
                &Metrics::default(),
                true,
                &Settings::default()
            )
            .area,
            "target_or_route"
        );
    }
    #[test]
    fn route_change_clears_router_baseline() {
        let mut analyzer = Analyzer::default();
        analyzer.observe(
            Utc::now(),
            &topology(),
            &probes(true, 2),
            &Settings::default(),
        );
        let mut t = topology();
        t.gateway = Some("10.0.0.1".into());
        let (_, d) = analyzer.observe(Utc::now(), &t, &probes(false, 0), &Settings::default());
        assert_eq!(d.area, "unknown");
    }
    #[test]
    fn slow_local_gateway_is_evidence_for_local_latency() {
        let mut row = probes(true, 2);
        row[0].latency_ms = Some(500.0);
        let d = classify(
            &topology(),
            &row,
            &Metrics::default(),
            true,
            &Settings::default(),
        );
        assert_eq!(d.area, "local_latency");
        assert_eq!(d.severity, Severity::Degraded);
        assert!(d.evidence.iter().any(|e| e.contains("500.0 ms")));
    }
}
