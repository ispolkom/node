// src/netlayer/nat_pmp.rs
//! NAT-PMP клиент (RFC 6886).
//!
//! Опрашивает gateway по UDP/5351 и:
//! 1. Узнаёт внешний IP роутера (op=0).
//! 2. Создаёт UDP port mapping (op=1) для нашего data-порта.
//!
//! Возвращает (external_ip, external_port, lifetime_seconds).
//! Если gateway не отвечает или не поддерживает NAT-PMP — Err.
//! Не подходит для CGNAT (там ничего нельзя пробить).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

const NAT_PMP_PORT: u16 = 5351;
const VERSION: u8 = 0;
const OP_EXTERNAL_ADDR: u8 = 0;
const OP_MAP_UDP: u8 = 1;

/// Результат успешного маппинга.
#[derive(Debug, Clone)]
pub struct PmpMapping {
    pub external_ip: Ipv4Addr,
    pub external_port: u16,
    pub internal_port: u16,
    pub lifetime_secs: u32,
}

/// Найти default gateway IPv4 через `ip route` (Linux).
/// Возвращает None, если gateway не определён.
fn detect_default_gateway() -> Option<Ipv4Addr> {
    let out = std::process::Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    // Формат: "default via 192.168.1.1 dev eth0 ..."
    let mut tokens = s.split_whitespace();
    while let Some(t) = tokens.next() {
        if t == "via" {
            if let Some(ip) = tokens.next() {
                return ip.parse().ok();
            }
        }
    }
    None
}

/// Запросить external IP у роутера (op=0).
async fn query_external_ip(sock: &UdpSocket, gateway: Ipv4Addr) -> Result<Ipv4Addr, String> {
    let req = [VERSION, OP_EXTERNAL_ADDR];
    sock.send_to(&req, SocketAddr::new(IpAddr::V4(gateway), NAT_PMP_PORT))
        .await
        .map_err(|e| format!("NAT-PMP send: {}", e))?;
    let mut buf = [0u8; 64];
    let (n, _) = timeout(Duration::from_secs(2), sock.recv_from(&mut buf))
        .await
        .map_err(|_| "NAT-PMP timeout".to_string())?
        .map_err(|e| format!("NAT-PMP recv: {}", e))?;
    if n < 12 {
        return Err(format!("NAT-PMP reply too short: {}", n));
    }
    if buf[0] != VERSION || buf[1] != OP_EXTERNAL_ADDR | 0x80 {
        return Err(format!("NAT-PMP bad header: {:?}", &buf[..2]));
    }
    let result_code = u16::from_be_bytes([buf[2], buf[3]]);
    if result_code != 0 {
        return Err(format!("NAT-PMP error code: {}", result_code));
    }
    let ip = Ipv4Addr::new(buf[8], buf[9], buf[10], buf[11]);
    Ok(ip)
}

/// Создать UDP port mapping (op=1).
async fn map_udp_port(
    sock: &UdpSocket,
    gateway: Ipv4Addr,
    internal_port: u16,
    requested_external: u16,
    lifetime_secs: u32,
) -> Result<(u16, u32), String> {
    let mut req = [0u8; 12];
    req[0] = VERSION;
    req[1] = OP_MAP_UDP;
    req[2] = 0; // reserved
    req[3] = 0;
    req[4..6].copy_from_slice(&internal_port.to_be_bytes());
    req[6..8].copy_from_slice(&requested_external.to_be_bytes());
    req[8..12].copy_from_slice(&lifetime_secs.to_be_bytes());
    sock.send_to(&req, SocketAddr::new(IpAddr::V4(gateway), NAT_PMP_PORT))
        .await
        .map_err(|e| format!("NAT-PMP map send: {}", e))?;
    let mut buf = [0u8; 64];
    let (n, _) = timeout(Duration::from_secs(2), sock.recv_from(&mut buf))
        .await
        .map_err(|_| "NAT-PMP map timeout".to_string())?
        .map_err(|e| format!("NAT-PMP map recv: {}", e))?;
    if n < 16 {
        return Err(format!("NAT-PMP map reply too short: {}", n));
    }
    if buf[0] != VERSION || buf[1] != OP_MAP_UDP | 0x80 {
        return Err(format!("NAT-PMP map bad header: {:?}", &buf[..2]));
    }
    let result_code = u16::from_be_bytes([buf[2], buf[3]]);
    if result_code != 0 {
        return Err(format!("NAT-PMP map error: {}", result_code));
    }
    let mapped_external = u16::from_be_bytes([buf[10], buf[11]]);
    let granted_lifetime = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
    Ok((mapped_external, granted_lifetime))
}

/// Полный цикл: gateway → external IP → mapping. Lifetime — желаемое время жизни (роутер может
/// вернуть меньше). Internal port = наш data-порт. requested_external = тот же port (роутер может дать другой).
pub async fn request_mapping(
    internal_port: u16,
    lifetime_secs: u32,
) -> Result<PmpMapping, String> {
    let gateway = detect_default_gateway()
        .ok_or_else(|| "default gateway not found".to_string())?;
    if !gateway.is_private() {
        // Если gateway уже публичный — мы не за NAT, NAT-PMP не нужен.
        return Err("gateway is public — no NAT".to_string());
    }
    let sock = UdpSocket::bind("0.0.0.0:0").await
        .map_err(|e| format!("bind: {}", e))?;
    let external_ip = query_external_ip(&sock, gateway).await?;
    let (external_port, granted) =
        map_udp_port(&sock, gateway, internal_port, internal_port, lifetime_secs).await?;
    Ok(PmpMapping {
        external_ip,
        external_port,
        internal_port,
        lifetime_secs: granted,
    })
}
