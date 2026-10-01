//! Optional UPnP IGD reads. The action allowlist contains no router mutation.
use crate::model::AccessType;
use anyhow::{Result, ensure};
use reqwest::{Client, Url};
use roxmltree::Document;
use std::{net::IpAddr, time::Duration};
use tokio::{net::UdpSocket, time::timeout};

#[derive(Clone, Debug)]
pub struct RouterDevice {
    pub model: String,
    pub access_type: AccessType,
    pub evidence: String,
    pub wan_connected: Option<bool>,
    gateway: IpAddr,
    services: Vec<(String, Url)>,
}
impl RouterDevice {
    pub fn gateway(&self) -> IpAddr {
        self.gateway
    }
}

fn allowed_url(url: &Url, gateway: IpAddr) -> bool {
    url.scheme() == "http"
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            == Some(gateway)
}

fn text<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
}

async fn bounded_body(mut response: reqwest::Response) -> Result<String> {
    response.error_for_status_ref()?;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            data.len() + chunk.len() <= 65536,
            "Router response exceeds 64 KiB"
        );
        data.extend_from_slice(&chunk);
    }
    let value = String::from_utf8(data)?;
    ensure!(
        !value.contains("<!DOCTYPE"),
        "Router XML DTDs are not supported"
    );
    Ok(value)
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()?)
}

fn parse_description(xml: &str, location: &Url, gateway: IpAddr) -> Result<RouterDevice> {
    ensure!(
        allowed_url(location, gateway),
        "Discovery URL must use the gateway's numeric IP"
    );
    let doc = Document::parse(xml)?;
    let device = doc
        .descendants()
        .find(|node| {
            node.has_tag_name("device")
                && text(*node, "deviceType").is_some_and(|s| s.contains(":InternetGatewayDevice:"))
        })
        .ok_or_else(|| anyhow::anyhow!("Device is not a UPnP internet gateway"))?;
    let model = text(device, "modelName")
        .unwrap_or("UPnP gateway")
        .to_string();
    let manufacturer = text(device, "manufacturer").unwrap_or("");
    let model_lower = model.to_lowercase();
    let cellular = model_lower.contains("5g gateway")
        || model_lower.contains("5g cpe")
        || model_lower.contains("5g cellular");
    let mut result = RouterDevice {
        model: format!("{manufacturer} {model}").trim().to_string(),
        access_type: AccessType::Unknown,
        evidence: if cellular {
            "Router model advertises 5G capability; its active backhaul is not confirmed.".into()
        } else {
            "Router model discovered through UPnP; active WAN technology is unknown.".into()
        },
        wan_connected: None,
        gateway,
        services: vec![],
    };
    for service in device.descendants().filter(|n| n.has_tag_name("service")) {
        let (Some(kind), Some(control)) =
            (text(service, "serviceType"), text(service, "controlURL"))
        else {
            continue;
        };
        // Restrict both service namespace and action; never trust a discovered action list.
        if ![
            "urn:schemas-upnp-org:service:WANIPConnection:",
            "urn:schemas-upnp-org:service:WANPPPConnection:",
            "urn:schemas-upnp-org:service:WANCommonInterfaceConfig:",
        ]
        .iter()
        .any(|s| kind.starts_with(s))
        {
            continue;
        }
        let url = location.join(control)?;
        if allowed_url(&url, gateway) {
            result.services.push((kind.to_string(), url));
        }
        if result.services.len() >= 4 {
            break;
        }
    }
    Ok(result)
}

pub async fn discover(gateway: IpAddr) -> Result<Option<RouterDevice>> {
    if !gateway.is_ipv4()
        || gateway.is_loopback()
        || gateway.is_unspecified()
        || gateway.is_multicast()
    {
        return Ok(None);
    }
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.set_multicast_ttl_v4(1)?;
    socket.send_to(b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n", "239.255.255.250:1900").await?;
    timeout(Duration::from_millis(1500), async {
        let mut buffer = [0; 4096];
        for _ in 0..16 {
            let (size, source) = socket.recv_from(&mut buffer).await?;
            if source.ip() != gateway {
                continue;
            }
            let reply = String::from_utf8_lossy(&buffer[..size]);
            let Some(location) = reply.lines().find_map(|line| {
                line.split_once(':')
                    .filter(|(key, _)| key.eq_ignore_ascii_case("location"))
                    .map(|(_, value)| value.trim())
            }) else {
                continue;
            };
            let Ok(location) = Url::parse(location) else {
                continue;
            };
            if !allowed_url(&location, gateway) {
                continue;
            }
            let xml = bounded_body(client()?.get(location.clone()).send().await?).await?;
            return Ok(Some(parse_description(&xml, &location, gateway)?));
        }
        Ok(None)
    })
    .await
    .unwrap_or(Ok(None))
}

pub async fn refresh(device: &mut RouterDevice) -> Result<()> {
    device.wan_connected = None; // Do not retain a stale WAN-down report after a failed refresh.
    let client = client()?;
    for (kind, url) in &device.services {
        ensure!(allowed_url(url, device.gateway), "Router endpoint changed");
        let action = if kind.contains(":WANCommonInterfaceConfig:") {
            "GetCommonLinkProperties"
        } else {
            "GetStatusInfo"
        };
        let body = format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{kind}\"/></s:Body></s:Envelope>"
        );
        let response = client
            .post(url.clone())
            .header("Content-Type", "text/xml; charset=utf-8")
            .header("SOAPAction", format!("\"{kind}#{action}\""))
            .body(body)
            .send()
            .await?;
        let xml = bounded_body(response).await?;
        let doc = Document::parse(&xml)?;
        let value = |name| {
            doc.descendants()
                .find(|n| n.has_tag_name(name))
                .and_then(|n| n.text())
        };
        if let Some(status) = value("NewConnectionStatus") {
            device.wan_connected = match status {
                "Connected" => Some(true),
                "Disconnected" | "Connecting" | "Disconnecting" | "PendingDisconnect" => {
                    Some(false)
                }
                _ => None,
            };
        }
        if let Some(access) = value("NewWANAccessType") {
            let access_type = match access {
                "DSL" | "POTS" => AccessType::Dsl,
                "Cable" => AccessType::Cable,
                _ => AccessType::Unknown,
            };
            if access_type != AccessType::Unknown {
                device.access_type = access_type;
                device.evidence =
                    format!("UPnP WAN interface reports {access}; NBN subtype is not exposed.");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_discovery_redirects_and_wifi_band_as_cellular_evidence() {
        let gateway = "192.168.1.1".parse().unwrap();
        assert!(!allowed_url(
            &Url::parse("http://example.com/router.xml").unwrap(),
            gateway
        ));
        assert!(!allowed_url(
            &Url::parse("http://192.168.1.2/router.xml").unwrap(),
            gateway
        ));
        let xml = "<root><device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType><modelName>Dual band 5GHz WiFi router</modelName><serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>http://example.com/control</controlURL></service></serviceList></device></root>";
        let device = parse_description(
            xml,
            &Url::parse("http://192.168.1.1/root.xml").unwrap(),
            gateway,
        )
        .unwrap();
        assert_eq!(device.access_type, AccessType::Unknown);
        assert!(device.services.is_empty());
    }
}
