// src/dataplane/metrics.rs
//! Dataplane Metrics
//! ===================
//!
//! Real-time metrics for transport monitoring

use std::time::Instant;

/// Real-time metrics for transport
#[derive(Debug, Clone)]
pub struct DataplaneMetrics {
    pub transport_stats: Vec<TransportStats>,
    pub active_transports: usize,
    pub total_bytes: u64,
    pub total_packets: u64,
    pub uptime: Instant,
}

/// Transport statistics snapshot
#[derive(Debug, Clone)]
pub struct TransportStats {
    pub transport_type: String,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub latency_ms: u64,
    pub loss_rate: f64,
}
