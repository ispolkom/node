// src/socks5/mod.rs
//! SOCKS5 Proxy Protocol
//! =====================
//!
//! Full SOCKS5 proxy implementation for bypass and integration

pub mod protocol;
pub mod server;
pub mod client;
pub mod proxy_protocol;
pub mod exit_node;
pub mod mod_data;

pub use protocol::{Socks5Version, Socks5Command, Socks5Address, Socks5AddressType, Socks5AuthMethod};
pub use server::{Socks5Server, Socks5ProxyServer};
pub use client::Socks5Client;
pub use exit_node::ExitNodeHandler;

// New structures - analogous to HTTP Proxy
pub use mod_data::{Socks5ProxyRequest, Socks5ProxyResponse, Socks5TunnelData};

use std::net::SocketAddr;
use anyhow::anyhow;

/// SOCKS5 configuration
#[derive(Debug, Clone)]
pub struct Socks5Config {
    /// Listen address for server
    pub listen_addr: SocketAddr,
    /// Enable authentication
    pub auth_required: bool,
    /// Username (if auth required)
    pub username: Option<String>,
    /// Password (if auth required)
    pub password: Option<String>,
    /// Enable UDP associate
    pub enable_udp: bool,
}

impl Default for Socks5Config {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:9111".parse().unwrap(),  // ✅ Внешний доступ (нестандартный порт)
            auth_required: false,
            username: None,
            password: None,
            enable_udp: true,
        }
    }
}

/// SOCKS5 error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Socks5Error {
    GeneralFailure,
    ConnectionNotAllowed,
    NetworkUnreachable,
    HostUnreachable,
    ConnectionRefused,
    TtlExpired,
    CommandNotSupported,
    AddressTypeNotSupported,
}

impl Socks5Error {
    /// Convert to SOCKS5 reply byte
    pub fn to_reply_byte(&self) -> u8 {
        match self {
            Socks5Error::GeneralFailure => 0x01,
            Socks5Error::ConnectionNotAllowed => 0x02,
            Socks5Error::NetworkUnreachable => 0x03,
            Socks5Error::HostUnreachable => 0x04,
            Socks5Error::ConnectionRefused => 0x05,
            Socks5Error::TtlExpired => 0x06,
            Socks5Error::CommandNotSupported => 0x07,
            Socks5Error::AddressTypeNotSupported => 0x08,
        }
    }

    /// Create from SOCKS5 reply byte
    pub fn from_reply_byte(byte: u8) -> Self {
        match byte {
            0x01 => Socks5Error::GeneralFailure,
            0x02 => Socks5Error::ConnectionNotAllowed,
            0x03 => Socks5Error::NetworkUnreachable,
            0x04 => Socks5Error::HostUnreachable,
            0x05 => Socks5Error::ConnectionRefused,
            0x06 => Socks5Error::TtlExpired,
            0x07 => Socks5Error::CommandNotSupported,
            0x08 => Socks5Error::AddressTypeNotSupported,
            _ => Socks5Error::GeneralFailure,
        }
    }
}

/// Convert SOCKS5 error to anyhow error
impl From<Socks5Error> for anyhow::Error {
    fn from(err: Socks5Error) -> Self {
        anyhow!("SOCKS5 error: {:?}", err)
    }
}
