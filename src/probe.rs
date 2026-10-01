use crate::model::{Probe, ProbeKind, ProbeState};
use regex::Regex;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        LazyLock,
        atomic::{AtomicU16, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpStream, UdpSocket},
    process::Command,
    time::timeout,
};

static RTT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[=<]\s*(\d+(?:[.,]\d+)?)\s*ms").unwrap());
static DNS_ID: AtomicU16 = AtomicU16::new(1);

pub fn parse_ping_latency(output: &str) -> Option<f64> {
    RTT.captures(output)
        .and_then(|c| c[1].replace(',', ".").parse::<f64>().ok())
}

pub async fn ping(kind: ProbeKind, target: IpAddr, timeout_ms: u64) -> Probe {
    let mut command = Command::new("ping");
    #[cfg(target_os = "windows")]
    {
        command.args(["-n", "1", "-w", &timeout_ms.to_string()]);
        command.creation_flags(0x08000000);
    }
    #[cfg(target_os = "linux")]
    {
        command.args([
            "-n",
            "-c",
            "1",
            "-W",
            &format!("{:.3}", timeout_ms as f64 / 1000.0),
        ]);
    }
    #[cfg(target_os = "macos")]
    {
        command.args(["-n", "-c", "1", "-W", &timeout_ms.to_string()]);
    }
    command.arg(target.to_string()).kill_on_drop(true);
    match timeout(Duration::from_millis(timeout_ms + 500), command.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.success()
                && let Some(ms) = parse_ping_latency(&stdout)
            {
                return Probe::success(kind, target.to_string(), ms);
            }
            let lower = format!("{stdout} {stderr}").to_lowercase();
            let unsupported = lower.contains("operation not permitted")
                || lower.contains("permission denied")
                || lower.contains("not recognized");
            Probe::failed(
                kind,
                target.to_string(),
                if unsupported {
                    ProbeState::Unsupported
                } else {
                    ProbeState::Failure
                },
                if unsupported {
                    "ICMP unavailable: permissions or ping utility"
                } else {
                    "No timed ICMP echo reply (could be blocked or lost)"
                },
            )
        }
        Ok(Err(e)) => Probe::failed(
            kind,
            target.to_string(),
            ProbeState::Unsupported,
            format!("Cannot run ping: {e}"),
        ),
        Err(_) => Probe::failed(
            kind,
            target.to_string(),
            ProbeState::Failure,
            "ICMP probe timed out",
        ),
    }
}

pub async fn tcp(target: IpAddr, timeout_ms: u64) -> Probe {
    let address = SocketAddr::new(target, 443);
    let started = Instant::now();
    match timeout(
        Duration::from_millis(timeout_ms),
        TcpStream::connect(address),
    )
    .await
    {
        Ok(Ok(stream)) => {
            drop(stream);
            Probe::success(
                ProbeKind::InternetTcp,
                address.to_string(),
                started.elapsed().as_secs_f64() * 1000.0,
            )
        }
        Ok(Err(e)) => Probe::failed(
            ProbeKind::InternetTcp,
            address.to_string(),
            ProbeState::Failure,
            e.to_string(),
        ),
        Err(_) => Probe::failed(
            ProbeKind::InternetTcp,
            address.to_string(),
            ProbeState::Failure,
            "TCP connect timed out",
        ),
    }
}

fn dns_query(id: u16) -> Vec<u8> {
    let mut bytes = vec![0; 12];
    bytes[0..2].copy_from_slice(&id.to_be_bytes());
    bytes[2] = 1;
    bytes[5] = 1;
    for label in ["example", "com"] {
        bytes.push(label.len() as u8);
        bytes.extend(label.as_bytes());
    }
    bytes.extend([0, 0, 1, 0, 1]);
    bytes
}

fn skip_name(bytes: &[u8], offset: &mut usize) -> bool {
    for _ in 0..128 {
        let Some(&length) = bytes.get(*offset) else {
            return false;
        };
        *offset += 1;
        if length == 0 {
            return true;
        }
        if length & 0xc0 == 0xc0 {
            let Some(&tail) = bytes.get(*offset) else {
                return false;
            };
            let pointer = ((length as usize & 0x3f) << 8) | tail as usize;
            if pointer >= *offset - 1 {
                return false;
            }
            *offset += 1;
            return true;
        }
        if length > 63 || *offset + length as usize > bytes.len() {
            return false;
        }
        *offset += length as usize;
    }
    false
}

fn valid_dns_response(query: &[u8], bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.len() < query.len() || bytes[0..2] != query[0..2] {
        return Err("Invalid DNS transaction");
    }
    if bytes[2] & 0x80 == 0 || bytes[2] & 0x78 != 0 || bytes[2] & 0x02 != 0 {
        return Err("Invalid or truncated DNS response");
    }
    if bytes[3] & 0x0f != 0 {
        return Err("DNS resolver returned an error");
    }
    if bytes[4..6] != [0, 1] || bytes[12..query.len()] != query[12..] {
        return Err("DNS question does not match");
    }
    let answers = u16::from_be_bytes([bytes[6], bytes[7]]) as usize;
    if answers == 0 || answers > 128 {
        return Err("DNS returned no usable answer");
    }
    let mut offset = query.len();
    let mut found = false;
    for _ in 0..answers {
        if !skip_name(bytes, &mut offset) || bytes.len() < offset + 10 {
            return Err("Malformed DNS answer");
        }
        let kind = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        let class = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let length = u16::from_be_bytes([bytes[offset + 8], bytes[offset + 9]]) as usize;
        offset += 10;
        if bytes.len() < offset + length {
            return Err("Truncated DNS answer");
        }
        found |= kind == 1 && class == 1 && length == 4;
        offset += length;
    }
    if found {
        Ok(())
    } else {
        Err("DNS returned no IPv4 address for example.com")
    }
}

pub async fn dns(kind: ProbeKind, target: IpAddr, timeout_ms: u64) -> Probe {
    let started = Instant::now();
    let query = dns_query(DNS_ID.fetch_add(1, Ordering::Relaxed));
    let result = timeout(Duration::from_millis(timeout_ms), async {
        let socket = UdpSocket::bind(if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .await
        .map_err(|e| e.to_string())?;
        socket
            .connect(SocketAddr::new(target, 53))
            .await
            .map_err(|e| e.to_string())?;
        socket.send(&query).await.map_err(|e| e.to_string())?;
        let mut bytes = [0u8; 2048];
        let length = socket.recv(&mut bytes).await.map_err(|e| e.to_string())?;
        valid_dns_response(&query, &bytes[..length]).map_err(str::to_owned)
    })
    .await;
    match result {
        Ok(Ok(())) => Probe::success(
            kind,
            target.to_string(),
            started.elapsed().as_secs_f64() * 1000.0,
        ),
        Ok(Err(e)) => Probe::failed(kind, target.to_string(), ProbeState::Failure, e),
        Err(_) => Probe::failed(
            kind,
            target.to_string(),
            ProbeState::Failure,
            "DNS query timed out",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_localized_echo_replies() {
        for (text, expected) in [
            ("Reply bytes=32 time=21ms TTL=56", 21.0),
            ("来自 1.1.1.1 的回复: 字节=32 时间<1ms TTL=57", 1.0),
            ("64 bytes: time=14.25 ms", 14.25),
            ("temps=12,5 ms", 12.5),
        ] {
            assert_eq!(parse_ping_latency(text), Some(expected));
        }
        assert_eq!(
            parse_ping_latency("Reply from 192.168.1.1: Destination host unreachable."),
            None
        );
    }
    #[test]
    fn validates_dns_transaction_and_answers() {
        let q = dns_query(1234);
        let mut response = q.clone();
        response[2] = 0x81;
        response[3] = 0x80;
        response[7] = 1;
        response.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 93, 184, 215, 14]);
        assert!(valid_dns_response(&q, &response).is_ok());
        response[0] ^= 1;
        assert!(valid_dns_response(&q, &response).is_err());
        response[0] ^= 1;
        response[3] = 0x83;
        assert!(valid_dns_response(&q, &response).is_err());
        assert!(valid_dns_response(&q, &[0; 3]).is_err());
    }
}
