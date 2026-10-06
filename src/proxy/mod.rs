// src/proxy/mod.rs
//! HTTP Proxy Module for DPI Bypass
//! ==================================
//!
//! This module provides HTTP proxy functionality for bypassing DPI (Deep Packet Inspection).
//!
//! ## Architecture
//!
//! **Client Side (Proxy Mode):**
//! ```text
//! [Browser] → HTTP localhost:8080 → [Local Proxy] → P2P UDP → [Gateway Node]
//! ```
//!
//! **Gateway Side:**
//! ```text
//! [Gateway Node] → P2P UDP → [Reverse Proxy] → HTTPS → [Target Server]
//! ```
//!
//! ## Flow
//!
//! 1. Browser makes HTTP request to localhost:8080
//!    - Example: `http://localhost:8080/youtube.com/watch?v=xxx`
//!
//! 2. Local proxy (client node):
//!    - Parses the URL
//!    - Converts to HTTPS: `https://youtube.com/watch?v=xxx`
//!    - Packs into P2P UDP packet
//!    - Sends to gateway node via YANDI P2P network
//!
//! 3. Gateway node:
//!    - Receives P2P UDP packet
//!    - Unpacks the HTTPS request
//!    - Makes REAL HTTPS request to target (from its own IP)
//!    - Gets response
//!    - Packs response into P2P UDP packet
//!    - Sends back to client
//!
//! 4. Local proxy:
//!    - Receives response from gateway
//!    - Sends to browser
//!
//! ## DPI Bypass
//!
//! - **Provider sees**: Only UDP YANDI packets (encrypted)
//! - **Target sees**: Normal HTTPS request from gateway node
//! - **Browser sees**: Normal HTTP response
//!
//! ## Usage
//!
//! **Client Mode:**
//! ```rust,ignore
//! use yandi::proxy::HttpProxyClient;
//!
//! let proxy = HttpProxyClient::new(
//!     transport.clone(),
//!     gateway_node_id
//! );
//!
//! // Listen on localhost:8080
//! proxy.start(8080).await?;
//! ```
//!
//! **Browser Configuration:**
//! - Set HTTP proxy: `127.0.0.1:8080`
//! - NO authentication required
//! - Supports ALL browsers and apps with HTTP proxy
//!
//! **Gateway Mode:**
//! ```rust,ignore
//! use yandi::proxy::HttpProxyGateway;
//!
//! let gateway = HttpProxyGateway::new(transport.clone());
//!
//! // Handle incoming proxy requests
//! gateway.run().await?;
//! ```

pub mod client;
pub mod gateway;
pub mod url_mapper;

pub use client::HttpProxyClient;
pub use gateway::HttpProxyGateway;
pub use url_mapper::UrlMapper;

use crate::util::HashId;
use serde::{Serialize, Deserialize};

/// HTTP proxy request (sent via P2P)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRequest {
    /// Request ID (for matching responses)
    pub request_id: u64,
    /// Target URL (HTTPS)
    pub url: String,
    /// HTTP method
    pub method: String,
    /// HTTP headers
    pub headers: Vec<(String, String)>,
    /// Request body
    pub body: Vec<u8>,
}

impl ProxyRequest {
    /// ⚡ Сериализовать через bincode (в 3-5 раз быстрее JSON)
    pub fn to_bincode(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// ⚡ Десериализовать из bincode
    pub fn from_bincode(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }
}

/// HTTP proxy response (sent via P2P)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyResponse {
    /// Request ID (must match request)
    pub request_id: u64,
    /// HTTP status code
    pub status: u16,
    /// Response headers
    pub headers: Vec<(String, String)>,
    /// Response body
    pub body: Vec<u8>,
}

impl ProxyResponse {
    /// ⚡ Сериализовать через bincode (в 3-5 раз быстрее JSON)
    pub fn to_bincode(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// ⚡ Десериализовать из bincode
    pub fn from_bincode(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }
}

/// Tunnel data for CONNECT (bi-directional streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyTunnelData {
    /// Tunnel ID (matches CONNECT request_id)
    pub tunnel_id: u64,
    /// Data chunk
    pub data: Vec<u8>,
    /// Close flag (true if this is the last chunk)
    pub close: bool,
}

impl ProxyTunnelData {
    /// ⚡ Сериализовать через bincode (в 3-5 раз быстрее JSON)
    pub fn to_bincode(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// ⚡ Десериализовать из bincode
    pub fn from_bincode(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }
}

/// Proxy configuration
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Client mode: listen address
    pub client_listen_addr: String,
    /// Client mode: gateway node ID
    pub gateway_node: Option<HashId>,
    /// Timeout for requests (seconds)
    pub timeout_secs: u64,
    /// Optional authentication token
    pub auth_token: Option<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            client_listen_addr: "127.0.0.1:8080".to_string(),
            gateway_node: None,
            timeout_secs: 30,
            auth_token: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_config_with_auth() {
        let config = ProxyConfig {
            client_listen_addr: "127.0.0.1:8080".to_string(),
            gateway_node: None,
            timeout_secs: 30,
            auth_token: Some("secret123".to_string()),
        };

        assert_eq!(config.client_listen_addr, "127.0.0.1:8080");
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.auth_token, Some("secret123".to_string()));
    }

    #[test]
    fn test_proxy_config_default_no_auth() {
        let config = ProxyConfig::default();

        assert_eq!(config.auth_token, None);
    }
}
