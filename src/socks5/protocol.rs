// src/socks5/protocol.rs
//! SOCKS5 Protocol Implementation
//! ===============================
//!
//! RFC 1928 SOCKS5 protocol

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use anyhow::{Result, anyhow};

/// SOCKS5 version (always 0x05)
pub const SOCKS5_VERSION: u8 = 0x05;

/// SOCKS5 version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Socks5Version;

impl Socks5Version {
    pub const fn byte() -> u8 {
        SOCKS5_VERSION
    }
}

/// SOCKS5 command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Socks5Command {
    Connect = 0x01,
    Bind = 0x02,
    UdpAssociate = 0x03,
}

impl Socks5Command {
    /// Parse from byte
    pub fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0x01 => Ok(Socks5Command::Connect),
            0x02 => Ok(Socks5Command::Bind),
            0x03 => Ok(Socks5Command::UdpAssociate),
            _ => Err(anyhow!("Invalid SOCKS5 command: {}", byte)),
        }
    }

    /// Convert to byte
    pub fn to_byte(&self) -> u8 {
        *self as u8
    }
}

/// Address type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Socks5AddressType {
    Ipv4 = 0x01,
    DomainName = 0x03,
    Ipv6 = 0x04,
}

impl Socks5AddressType {
    /// Parse from byte
    pub fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0x01 => Ok(Socks5AddressType::Ipv4),
            0x03 => Ok(Socks5AddressType::DomainName),
            0x04 => Ok(Socks5AddressType::Ipv6),
            _ => Err(anyhow!("Invalid address type: {}", byte)),
        }
    }

    /// Convert to byte
    pub fn to_byte(&self) -> u8 {
        *self as u8
    }
}

/// Authentication method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Socks5AuthMethod {
    NoAuth = 0x00,
    GssApi = 0x01,
    UserPass = 0x02,
    NoAcceptable = 0xFF,
}

impl Socks5AuthMethod {
    /// Parse from byte
    pub fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0x00 => Ok(Socks5AuthMethod::NoAuth),
            0x01 => Ok(Socks5AuthMethod::GssApi),
            0x02 => Ok(Socks5AuthMethod::UserPass),
            0xFF => Ok(Socks5AuthMethod::NoAcceptable),
            _ => Err(anyhow!("Invalid auth method: {}", byte)),
        }
    }

    /// Convert to byte
    pub fn to_byte(&self) -> u8 {
        *self as u8
    }
}

/// SOCKS5 address (IPv4, IPv6, or domain)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Socks5Address {
    Ipv4(Ipv4Addr, u16),           // IP + port
    Ipv6(Ipv6Addr, u16),           // IP + port
    Domain(String, u16),           // Domain + port
}

impl Socks5Address {
    /// Parse from bytes (after address type byte)
    pub fn from_bytes(addr_type: Socks5AddressType, data: &[u8]) -> Result<(Self, usize)> {
        match addr_type {
            Socks5AddressType::Ipv4 => {
                if data.len() < 6 {
                    return Err(anyhow!("Not enough bytes for IPv4 address"));
                }
                let ip = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
                let port = u16::from_be_bytes([data[4], data[5]]);
                Ok((Socks5Address::Ipv4(ip, port), 6))
            }
            Socks5AddressType::Ipv6 => {
                if data.len() < 18 {
                    return Err(anyhow!("Not enough bytes for IPv6 address"));
                }
                let ip_bytes: [u8; 16] = data[0..16].try_into().unwrap();
                let ip = Ipv6Addr::from(ip_bytes);
                let port = u16::from_be_bytes([data[16], data[17]]);
                Ok((Socks5Address::Ipv6(ip, port), 18))
            }
            Socks5AddressType::DomainName => {
                if data.is_empty() {
                    return Err(anyhow!("No domain length byte"));
                }
                let len = data[0] as usize;
                if data.len() < 1 + len + 2 {
                    return Err(anyhow!("Not enough bytes for domain address"));
                }
                let domain = String::from_utf8_lossy(&data[1..1+len]).to_string();
                let port = u16::from_be_bytes([data[1+len], data[1+len+1]]);
                Ok((Socks5Address::Domain(domain, port), 1 + len + 2))
            }
        }
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();

        match self {
            Socks5Address::Ipv4(ip, port) => {
                out.push(Socks5AddressType::Ipv4.to_byte());
                out.extend_from_slice(&ip.octets());
                out.extend_from_slice(&port.to_be_bytes());
            }
            Socks5Address::Ipv6(ip, port) => {
                out.push(Socks5AddressType::Ipv6.to_byte());
                out.extend_from_slice(&ip.octets());
                out.extend_from_slice(&port.to_be_bytes());
            }
            Socks5Address::Domain(domain, port) => {
                out.push(Socks5AddressType::DomainName.to_byte());
                let domain_bytes = domain.as_bytes();
                out.push(domain_bytes.len() as u8);
                out.extend_from_slice(domain_bytes);
                out.extend_from_slice(&port.to_be_bytes());
            }
        }

        out
    }

    /// Get port
    pub fn port(&self) -> u16 {
        match self {
            Socks5Address::Ipv4(_, port) => *port,
            Socks5Address::Ipv6(_, port) => *port,
            Socks5Address::Domain(_, port) => *port,
        }
    }

    /// Try to convert to SocketAddr (fails for domain names)
    pub fn to_socket_addr(&self) -> Option<SocketAddr> {
        match self {
            Socks5Address::Ipv4(ip, port) => {
                Some(SocketAddr::V4(SocketAddrV4::new(*ip, *port)))
            }
            Socks5Address::Ipv6(ip, port) => {
                Some(SocketAddr::V6(SocketAddrV6::new(*ip, *port, 0, 0)))
            }
            Socks5Address::Domain(_, _) => None,
        }
    }

    /// Create from SocketAddr
    pub fn from_socket_addr(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(v4) => {
                Socks5Address::Ipv4(*v4.ip(), v4.port())
            }
            SocketAddr::V6(v6) => {
                Socks5Address::Ipv6(*v6.ip(), v6.port())
            }
        }
    }
}

/// SOCKS5 request header
#[derive(Debug, Clone)]
pub struct Socks5Request {
    pub version: u8,
    pub command: Socks5Command,
    pub reserved: u8,
    pub address: Socks5Address,
}

impl Socks5Request {
    /// Minimum size of SOCKS5 request
    pub const MIN_SIZE: usize = 4; // VER + CMD + RSV + ATYP

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < Self::MIN_SIZE {
            return Err(anyhow!("SOCKS5 request too short"));
        }

        let version = data[0];
        if version != SOCKS5_VERSION {
            return Err(anyhow!("Invalid SOCKS5 version: {}", version));
        }

        let command = Socks5Command::from_byte(data[1])?;
        let addr_type = Socks5AddressType::from_byte(data[3])?;
        let (address, _) = Socks5Address::from_bytes(addr_type, &data[4..])?;

        Ok(Self {
            version,
            command,
            reserved: data[2],
            address,
        })
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![self.version, self.command.to_byte(), self.reserved];
        out.extend_from_slice(&self.address.to_bytes());
        out
    }
}

/// SOCKS5 response
#[derive(Debug, Clone)]
pub struct Socks5Response {
    pub version: u8,
    pub reply: u8,  // 0x00 = success
    pub reserved: u8,
    pub address: Socks5Address,
}

impl Socks5Response {
    /// Create success response
    pub fn success(address: Socks5Address) -> Self {
        Self {
            version: SOCKS5_VERSION,
            reply: 0x00,
            reserved: 0x00,
            address,
        }
    }

    /// Create error response
    pub fn error(error: crate::socks5::Socks5Error, bind_addr: Option<Socks5Address>) -> Self {
        Self {
            version: SOCKS5_VERSION,
            reply: error.to_reply_byte(),
            reserved: 0x00,
            address: bind_addr.unwrap_or(Socks5Address::Ipv4(Ipv4Addr::new(0, 0, 0, 0), 0)),
        }
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![self.version, self.reply, self.reserved];
        out.extend_from_slice(&self.address.to_bytes());
        out
    }
}

/// SOCKS5 authentication selection (client hello)
pub struct Socks5AuthSelect {
    pub version: u8,
    pub methods: Vec<Socks5AuthMethod>,
}

impl Socks5AuthSelect {
    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            return Err(anyhow!("Empty auth select"));
        }

        let version = data[0];
        if version != SOCKS5_VERSION {
            return Err(anyhow!("Invalid SOCKS5 version: {}", version));
        }

        if data.len() < 2 {
            return Err(anyhow!("Auth select too short"));
        }

        let num_methods = data[1] as usize;
        if data.len() < 2 + num_methods {
            return Err(anyhow!("Not enough method bytes"));
        }

        let methods = (0..num_methods)
            .map(|i| Socks5AuthMethod::from_byte(data[2 + i]))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self { version, methods })
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![self.version, self.methods.len() as u8];
        for method in &self.methods {
            out.push(method.to_byte());
        }
        out
    }
}

/// SOCKS5 auth selection response
pub struct Socks5AuthResponse {
    pub version: u8,
    pub method: Socks5AuthMethod,
}

impl Socks5AuthResponse {
    /// Create from selected method
    pub fn new(method: Socks5AuthMethod) -> Self {
        Self {
            version: SOCKS5_VERSION,
            method,
        }
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        vec![self.version, self.method.to_byte()]
    }
}

#[cfg(test)]
mod hostile_input_tests {
    use super::*;

    /// Любые байты в разборщиках: ошибка допустима, паника — нет.
    #[test]
    fn parsers_never_panic_on_garbage() {
        let mut x: u64 = 0x9E3779B97F4A7C15;
        let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        for round in 0..20000 {
            let len = (next() % 300) as usize;
            let mut b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if round % 2 == 0 && !b.is_empty() { b[0] = SOCKS5_VERSION; } // дойти глубже версии
            if round % 4 == 0 && b.len() > 3 { b[3] = [1u8, 3, 4][(next() % 3) as usize]; }
            let _ = Socks5Request::from_bytes(&b);
            let _ = Socks5AuthSelect::from_bytes(&b);
        }
        // край: длина домена больше остатка
        let _ = Socks5Request::from_bytes(&[5, 1, 0, 3, 255, 1, 2]);
        let _ = Socks5Address::from_bytes(Socks5AddressType::DomainName, &[255]);
    }

    /// Недоверенный JSON от узла (запрос/данные туннеля) разбирается без паники.
    #[test]
    fn json_messages_never_panic() {
        use crate::socks5::{Socks5ProxyRequest, Socks5TunnelData};
        for s in [&b""[..], b"{}", b"[", b"{\"request_id\":-1}", b"{\"request_id\":1,\"target_host\":\"x\",\"target_port\":70000,\"command\":1}", &[0xff; 64][..]] {
            let _ = serde_json::from_slice::<Socks5ProxyRequest>(s);
            let _ = serde_json::from_slice::<Socks5TunnelData>(s);
        }
    }
}
