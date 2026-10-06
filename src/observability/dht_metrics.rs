// observability/dht_metrics.rs
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct DhtMetrics {
    // Lookup metrics
    pub lookups_initiated: AtomicU64,
    pub lookups_succeeded: AtomicU64,
    pub lookups_failed: AtomicU64,
    pub total_lookup_hops: AtomicU64,
    pub total_lookup_latency_ms: AtomicU64,
    
    // RPC metrics
    pub ping_sent: AtomicU64,
    pub ping_success: AtomicU64,
    pub find_node_sent: AtomicU64,
    pub find_node_success: AtomicU64,
    pub find_value_sent: AtomicU64,
    pub find_value_success: AtomicU64,
    pub store_sent: AtomicU64,
    pub store_success: AtomicU64,
    
    // Storage metrics
    pub total_keys: AtomicU64,
    pub total_values_bytes: AtomicU64,
    
    // Network metrics
    pub active_peers: AtomicU64,
    pub bucket_fill_ratio: AtomicU64, // * 1000 для хранения как u64
}

impl DhtMetrics {
    pub fn new() -> Self {
        Self {
            lookups_initiated: AtomicU64::new(0),
            lookups_succeeded: AtomicU64::new(0),
            lookups_failed: AtomicU64::new(0),
            total_lookup_hops: AtomicU64::new(0),
            total_lookup_latency_ms: AtomicU64::new(0),
            ping_sent: AtomicU64::new(0),
            ping_success: AtomicU64::new(0),
            find_node_sent: AtomicU64::new(0),
            find_node_success: AtomicU64::new(0),
            find_value_sent: AtomicU64::new(0),
            find_value_success: AtomicU64::new(0),
            store_sent: AtomicU64::new(0),
            store_success: AtomicU64::new(0),
            total_keys: AtomicU64::new(0),
            total_values_bytes: AtomicU64::new(0),
            active_peers: AtomicU64::new(0),
            bucket_fill_ratio: AtomicU64::new(0),
        }
    }
    
    pub fn record_lookup(&self, success: bool, hops: u64, latency_ms: u64) {
        self.lookups_initiated.fetch_add(1, Ordering::Relaxed);
        if success {
            self.lookups_succeeded.fetch_add(1, Ordering::Relaxed);
            self.total_lookup_hops.fetch_add(hops, Ordering::Relaxed);
            self.total_lookup_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
        } else {
            self.lookups_failed.fetch_add(1, Ordering::Relaxed);
        }
    }
    
    pub fn get_avg_lookup_hops(&self) -> f64 {
        let total = self.lookups_succeeded.load(Ordering::Relaxed);
        if total == 0 { return 0.0; }
        self.total_lookup_hops.load(Ordering::Relaxed) as f64 / total as f64
    }
    
    pub fn get_avg_lookup_latency_ms(&self) -> f64 {
        let total = self.lookups_succeeded.load(Ordering::Relaxed);
        if total == 0 { return 0.0; }
        self.total_lookup_latency_ms.load(Ordering::Relaxed) as f64 / total as f64
    }
    
    pub fn get_success_rate(&self) -> f64 {
        let total = self.lookups_initiated.load(Ordering::Relaxed);
        if total == 0 { return 1.0; }
        self.lookups_succeeded.load(Ordering::Relaxed) as f64 / total as f64
    }
}

// Добавим структуру для логирования операций
pub struct LookupSpan {
    pub target: Vec<u8>,
    pub start_time: Instant,
    pub hops: u64,
}

impl LookupSpan {
    pub fn new(target: Vec<u8>) -> Self {
        Self {
            target,
            start_time: Instant::now(),
            hops: 0,
        }
    }
    
    pub fn finish(self, success: bool, metrics: &DhtMetrics) {
        let latency = self.start_time.elapsed().as_millis() as u64;
        metrics.record_lookup(success, self.hops, latency);
        
        log::info!(
            "Lookup {}: target={:?}, hops={}, latency={}ms, success={}",
            if success { "succeeded" } else { "failed" },
            hex::encode(&self.target),
            self.hops,
            latency,
            success
        );
    }
}