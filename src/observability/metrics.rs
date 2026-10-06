// src/observability/metrics.rs
//! Network Metrics
//! ================
//!
//! Simple metrics collection without external dependencies

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Network performance and health metrics
#[derive(Clone, Debug)]
pub struct NetworkMetrics {
    inner: Arc<MetricsInner>,
}

#[derive(Debug)]
struct MetricsInner {
    // Connection metrics
    connections_total: AtomicU64,
    active_connections: AtomicU64,
    connection_errors: AtomicU64,

    // DHT metrics
    dht_nodes_total: AtomicU64,
    dht_lookups_initiated: AtomicU64,
    dht_lookups_succeeded: AtomicU64,
    dht_lookups_failed: AtomicU64,
    dht_total_lookup_hops: AtomicU64,
    dht_total_lookup_latency_ms: AtomicU64,
    dht_stores: AtomicU64,

    // Transport metrics
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    packets_sent: AtomicU64,
    packets_received: AtomicU64,

    // Error tracking
    errors: AtomicU64,

    // Start time
    started_at: Instant,
}

impl NetworkMetrics {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MetricsInner {
                connections_total: AtomicU64::new(0),
                active_connections: AtomicU64::new(0),
                connection_errors: AtomicU64::new(0),
                dht_nodes_total: AtomicU64::new(0),
                dht_lookups_initiated: AtomicU64::new(0),
                dht_lookups_succeeded: AtomicU64::new(0),
                dht_lookups_failed: AtomicU64::new(0),
                dht_total_lookup_hops: AtomicU64::new(0),
                dht_total_lookup_latency_ms: AtomicU64::new(0),
                dht_stores: AtomicU64::new(0),
                bytes_sent: AtomicU64::new(0),
                bytes_received: AtomicU64::new(0),
                packets_sent: AtomicU64::new(0),
                packets_received: AtomicU64::new(0),
                errors: AtomicU64::new(0),
                started_at: Instant::now(),
            }),
        }
    }

    // Connection metrics
    pub fn inc_connections(&self) {
        self.inner.connections_total.fetch_add(1, Ordering::Relaxed);
        self.inner.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_active_connections(&self) {
        self.inner.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn inc_connection_errors(&self) {
        self.inner.connection_errors.fetch_add(1, Ordering::Relaxed);
    }

    // DHT metrics
    pub fn set_dht_nodes(&self, count: u64) {
        self.inner.dht_nodes_total.store(count, Ordering::Relaxed);
    }

    pub fn inc_dht_lookups(&self) {
        self.inner.dht_lookups_initiated.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_dht_lookup(&self, success: bool, hops: u64, latency_ms: u64) {
        if success {
            self.inner.dht_lookups_succeeded.fetch_add(1, Ordering::Relaxed);
            self.inner.dht_total_lookup_hops.fetch_add(hops, Ordering::Relaxed);
            self.inner.dht_total_lookup_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
        } else {
            self.inner.dht_lookups_failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn inc_dht_stores(&self) {
        self.inner.dht_stores.fetch_add(1, Ordering::Relaxed);
    }

    // Transport metrics
    pub fn add_bytes_sent(&self, bytes: u64) {
        self.inner.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_bytes_received(&self, bytes: u64) {
        self.inner.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn inc_packets_sent(&self) {
        self.inner.packets_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_packets_received(&self) {
        self.inner.packets_received.fetch_add(1, Ordering::Relaxed);
    }

    // Error tracking
    pub fn inc_errors(&self) {
        self.inner.errors.fetch_add(1, Ordering::Relaxed);
    }

    // Get snapshot
    pub fn snapshot(&self) -> MetricsSnapshot {
        let succeeded = self.inner.dht_lookups_succeeded.load(Ordering::Relaxed);
        let failed = self.inner.dht_lookups_failed.load(Ordering::Relaxed);
        let total_lookups = succeeded + failed;
        
        MetricsSnapshot {
            connections_total: self.inner.connections_total.load(Ordering::Relaxed),
            active_connections: self.inner.active_connections.load(Ordering::Relaxed),
            connection_errors: self.inner.connection_errors.load(Ordering::Relaxed),
            dht_nodes_total: self.inner.dht_nodes_total.load(Ordering::Relaxed),
            dht_lookups_total: total_lookups,
            dht_lookups_succeeded: succeeded,
            dht_lookups_failed: failed,
            dht_avg_lookup_hops: if succeeded > 0 {
                self.inner.dht_total_lookup_hops.load(Ordering::Relaxed) as f64 / succeeded as f64
            } else { 0.0 },
            dht_avg_lookup_latency_ms: if succeeded > 0 {
                self.inner.dht_total_lookup_latency_ms.load(Ordering::Relaxed) as f64 / succeeded as f64
            } else { 0.0 },
            dht_stores: self.inner.dht_stores.load(Ordering::Relaxed),
            bytes_sent: self.inner.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.inner.bytes_received.load(Ordering::Relaxed),
            packets_sent: self.inner.packets_sent.load(Ordering::Relaxed),
            packets_received: self.inner.packets_received.load(Ordering::Relaxed),
            errors: self.inner.errors.load(Ordering::Relaxed),
            uptime_secs: self.inner.started_at.elapsed().as_secs(),
        }
    }
}

impl Default for NetworkMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics snapshot
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub connections_total: u64,
    pub active_connections: u64,
    pub connection_errors: u64,
    pub dht_nodes_total: u64,
    pub dht_lookups_total: u64,
    pub dht_lookups_succeeded: u64,
    pub dht_lookups_failed: u64,
    pub dht_avg_lookup_hops: f64,
    pub dht_avg_lookup_latency_ms: f64,
    pub dht_stores: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub errors: u64,
    pub uptime_secs: u64,
}

impl std::fmt::Display for MetricsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let success_rate = if self.dht_lookups_total > 0 {
            (self.dht_lookups_succeeded as f64 / self.dht_lookups_total as f64) * 100.0
        } else { 100.0 };
        
        writeln!(f, "📊 YANDI Network Metrics")?;
        writeln!(f, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        writeln!(f, "Connections:")?;
        writeln!(f, "  Total: {}", self.connections_total)?;
        writeln!(f, "  Active: {}", self.active_connections)?;
        writeln!(f, "  Errors: {}", self.connection_errors)?;
        writeln!(f, "")?;
        writeln!(f, "DHT:")?;
        writeln!(f, "  Nodes: {}", self.dht_nodes_total)?;
        writeln!(f, "  Lookups: {} total ({} success, {} failed)", 
            self.dht_lookups_total, self.dht_lookups_succeeded, self.dht_lookups_failed)?;
        writeln!(f, "  Success Rate: {:.1}%", success_rate)?;
        writeln!(f, "  Avg Hops: {:.2}", self.dht_avg_lookup_hops)?;
        writeln!(f, "  Avg Latency: {:.0}ms", self.dht_avg_lookup_latency_ms)?;
        writeln!(f, "  Stores: {}", self.dht_stores)?;
        writeln!(f, "")?;
        writeln!(f, "Transport:")?;
        writeln!(f, "  Sent: {} bytes ({} packets)", self.bytes_sent, self.packets_sent)?;
        writeln!(f, "  Received: {} bytes ({} packets)", self.bytes_received, self.packets_received)?;
        writeln!(f, "")?;
        writeln!(f, "Errors: {}", self.errors)?;
        writeln!(f, "Uptime: {}s", self.uptime_secs)?;
        Ok(())
    }
}
