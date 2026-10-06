// src/dataplane/transport.rs
//! Data Transport
//! ===============
//!
//! Adaptive data transport with multiple strategies

use std::time::{Duration, Instant};
use std::net::SocketAddr;

/// Transport type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    Udp,
    Tcp,
    Quic,
    Tunnel,
    Obfuscated,
}

/// Transport configuration
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Transport type
    pub transport_type: TransportType,

    /// Connection timeout
    pub timeout: Duration,

    /// Max packet size
    pub max_packet_size: usize,

    /// Enable reliability
    pub reliable: bool,

    /// Enable ordering
    pub ordered: bool,

    /// Enable encryption
    pub encrypted: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            transport_type: TransportType::Udp,
            timeout: Duration::from_secs(5),
            max_packet_size: 65536,
            reliable: false,
            ordered: false,
            encrypted: true,
        }
    }
}

/// Data transport statistics
#[derive(Debug, Clone)]
pub struct TransportStats {
    pub transport_type: TransportType,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub latency_ms: u64,
    pub established_at: Instant,
}

/// Data transport with adaptive features
pub struct DataTransport {
    config: TransportConfig,
    stats: TransportStats,
    remote_addr: SocketAddr,
}

impl DataTransport {
    /// Create new data transport
    pub fn new(remote_addr: SocketAddr, config: TransportConfig) -> Self {
        let stats = TransportStats {
            transport_type: config.transport_type,
            bytes_sent: 0,
            bytes_received: 0,
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            latency_ms: 0,
            established_at: Instant::now(),
        };

        Self {
            config,
            stats,
            remote_addr,
        }
    }

    /// Get remote address
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Get configuration
    pub fn config(&self) -> &TransportConfig {
        &self.config
    }

    /// Get statistics
    pub fn stats(&self) -> &TransportStats {
        &self.stats
    }

    /// Update latency
    pub fn update_latency(&mut self, latency_ms: u64) {
        // Exponential moving average
        let alpha = 0.1;
        self.stats.latency_ms = ((alpha * latency_ms as f64) +
            ((1.0 - alpha) * self.stats.latency_ms as f64)) as u64;
    }

    /// Record sent data
    pub fn record_sent(&mut self, bytes: u64, packets: u64) {
        self.stats.bytes_sent += bytes;
        self.stats.packets_sent += packets;
    }

    /// Record received data
    pub fn record_received(&mut self, bytes: u64, packets: u64) {
        self.stats.bytes_received += bytes;
        self.stats.packets_received += packets;
    }

    /// Record lost packet
    pub fn record_loss(&mut self, count: u64) {
        self.stats.packets_lost += count;
    }

    /// Get loss rate
    pub fn loss_rate(&self) -> f64 {
        let total = self.stats.packets_sent + self.stats.packets_lost;
        if total == 0 {
            return 0.0;
        }
        self.stats.packets_lost as f64 / total as f64
    }

    /// Check if transport is healthy
    pub fn is_healthy(&self) -> bool {
        self.loss_rate() < 0.05 && // Less than 5% loss
        self.stats.latency_ms < 1000 && // Less than 1 second latency
        self.stats.bytes_received > 0 // Receiving data
    }
}
