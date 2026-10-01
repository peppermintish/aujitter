use crate::{
    config::Settings,
    model::{AccessType, Topology},
};

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
            result.local_transport = format!("{:?}", interface.if_type);
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
