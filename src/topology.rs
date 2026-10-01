use crate::{
    config::Settings,
    model::{AccessType, ConnectionHint, Topology},
};
use netdev::interface::types::InterfaceType;

pub fn detect(settings: &Settings) -> Topology {
    let mut result = Topology {
        local_transport: "Unknown".into(),
        isp: settings.isp.clone(),
        access_type: settings.access_type,
        access_evidence: if settings.access_type == AccessType::Unknown {
            "WAN technology is not exposed by the PC network adapter.".into()
        } else {
            "User configured connection type.".into()
        },
        container: std::env::var_os("AUJITTER_CONTAINER").is_some()
            || std::path::Path::new("/.dockerenv").exists(),
        ..Default::default()
    };
    match netdev::get_default_interface() {
        Ok(interface) => {
            result.interface = Some(
                interface
                    .friendly_name
                    .clone()
                    .unwrap_or_else(|| interface.name.clone()),
            );
            result.connection_hint = connection_hint(
                interface.if_type,
                &interface.name,
                interface.friendly_name.as_deref(),
            );
            result.local_transport = match interface.if_type {
                netdev::interface::types::InterfaceType::Wireless80211 => "Wi-Fi".into(),
                netdev::interface::types::InterfaceType::Ethernet
                | netdev::interface::types::InterfaceType::Ethernet3Megabit
                | netdev::interface::types::InterfaceType::FastEthernetT
                | netdev::interface::types::InterfaceType::FastEthernetFx
                | netdev::interface::types::InterfaceType::GigabitEthernet => "Ethernet".into(),
                other => format!("{other:?}"),
            };
            result.local_addresses = interface
                .ipv4
                .iter()
                .map(|ip| ip.addr().to_string())
                .chain(interface.ipv6.iter().map(|ip| ip.addr().to_string()))
                .collect();
            if let Some(gateway) = interface.gateway {
                result.gateway = gateway
                    .ipv4
                    .first()
                    .map(ToString::to_string)
                    .or_else(|| gateway.ipv6.first().map(ToString::to_string));
            }
            result.dns_server = interface.dns_servers.first().map(ToString::to_string);
        }
        Err(e) => result
            .notes
            .push(format!("Default interface could not be detected: {e}")),
    }
    if let Some(gateway) = settings.gateway {
        result.gateway = Some(gateway.to_string());
    }
    if let Some(dns) = settings.dns_server {
        result.dns_server = Some(dns.to_string());
    }
    if result.container {
        result.notes.push("Container viewpoint: its default route may be Docker's bridge or a virtual machine. Configure the physical router IP, or use host networking on Linux.".into());
    }
    result
}

fn connection_hint(kind: InterfaceType, name: &str, friendly: Option<&str>) -> ConnectionHint {
    if matches!(
        kind,
        InterfaceType::Wwan | InterfaceType::Wwanpp | InterfaceType::Wwanpp2 | InterfaceType::Wman
    ) {
        return ConnectionHint::MobileBroadband;
    }
    let name = name.to_ascii_lowercase();
    let named_tunnel = ["utun", "tun", "tap", "wg"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
    });
    let known_adapter = friendly.unwrap_or_default().to_ascii_lowercase();
    let vpn_label = ["wireguard", "wintun", "openvpn", "tailscale", "tap-windows"]
        .iter()
        .any(|label| known_adapter.contains(label) || name.contains(label));
    if kind == InterfaceType::Tunnel || named_tunnel || vpn_label {
        ConnectionHint::Tunnel
    } else {
        ConnectionHint::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognises_adapter_evidence_without_guessing_the_router_uplink() {
        assert_eq!(
            connection_hint(InterfaceType::Wireless80211, "Wi-Fi 5GHz", None),
            ConnectionHint::Unknown
        );
        assert_eq!(
            connection_hint(InterfaceType::Wwanpp, "Cellular", None),
            ConnectionHint::MobileBroadband
        );
        assert_eq!(
            connection_hint(InterfaceType::Tunnel, "hidden", None),
            ConnectionHint::Tunnel
        );
        assert_eq!(
            connection_hint(InterfaceType::Unknown, "utun3", None),
            ConnectionHint::Tunnel
        );
        assert_eq!(
            connection_hint(InterfaceType::Ethernet, "id", Some("WireGuard Tunnel")),
            ConnectionHint::Tunnel
        );
        assert_eq!(
            connection_hint(InterfaceType::Wireless80211, "tunnel home", None),
            ConnectionHint::Unknown
        );
        assert_eq!(
            connection_hint(InterfaceType::Ppp, "ppp0", None),
            ConnectionHint::Unknown
        );
    }
}
