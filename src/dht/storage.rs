// src/dht/storage.rs
//! DHT Storage with TTL and Rate Limiting
//! =======================================
//!
//! Key-value storage for DHT with spam protection

use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use serde::{Serialize, Deserialize};
use crate::util::HashId;
use crate::dht::record::NodeRecord;

/// TTL for DHT records (Kademlia recommends ≤ 24 hours)
pub const DHT_TTL: u64 = 24 * 60 * 60;

/// Maximum record size (1MB)
pub const MAX_RECORD_SIZE: usize = 1024 * 1024;

/// Maximum number of records
pub const MAX_RECORDS: usize = 10000;

/// Maximum requests per minute from one source
pub const MAX_REQUESTS_PER_MINUTE: u32 = 100;

/// Maximum storage size (100MB)
pub const MAX_STORAGE_BYTES: usize = 100 * 1024 * 1024;

/// Request type for rate limiting (Stage 2.1: Per-second limits)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestType {
    Store,
    FindValue,
    FindNode,
    Iterate,  // For iteration/scan operations
}
/// Request tracking info (Stage 2.1: Per-second and per-minute tracking)
#[derive(Debug)]
pub struct RequestTracker {
    /// Last second reset time
    pub last_second_reset: u64,
    /// Requests in current second
    pub requests_this_second: u32,
    /// Last minute reset time
    pub last_minute_reset: u64,
    /// Requests in current minute
    pub requests_this_minute: u32,
}

/// Rate limits per request type (Stage 2.1: Granular rate limiting)
#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    /// Maximum requests per second
    pub per_second: u32,
    /// Maximum requests per minute (fallback)
    pub per_minute: u32,
    /// Block duration in seconds when limit exceeded
    pub block_duration: u64,
}

impl RateLimit {
    /// Conservative limits for expensive operations
    pub const fn conservative(per_second: u32, per_minute: u32) -> Self {
        Self { per_second, per_minute, block_duration: 300 }
    }

    /// Standard limits for normal operations
    pub const fn standard(per_second: u32, per_minute: u32) -> Self {
        Self { per_second, per_minute, block_duration: 60 }
    }
}

/// RATE_LIMITS configuration (Stage 2.1)
/// Prevents DHT spam and mass attacks
pub const RATE_LIMITS: &[(RequestType, RateLimit)] = &[
    // STORE: Most expensive operation (5/sec = 18K/hour max)
    (RequestType::Store, RateLimit::conservative(5, 100)),

    // FIND_VALUE: Expensive lookup (10/sec = 36K/hour max)
    (RequestType::FindValue, RateLimit::standard(10, 200)),

    // FIND_NODE: Cheap operation (20/sec = 72K/hour max)
    (RequestType::FindNode, RateLimit::standard(20, 500)),

    // ITERATE: Very expensive, must be limited (3/sec = 10K/hour max)
    (RequestType::Iterate, RateLimit::conservative(3, 50)),
];

/// Get rate limit for request type
pub fn get_rate_limit(req_type: RequestType) -> RateLimit {
    RATE_LIMITS.iter()
        .find(|(rt, _)| *rt == req_type)
        .map(|(_, limit)| *limit)
        .unwrap_or_else(|| RateLimit::standard(10, 100))
}



/// Value stored in DHT with metadata
#[derive(Clone, Debug)]
pub struct DhtRecord {
    pub value: Vec<u8>,
    pub timestamp: u64,
    pub origin: String,
    pub size: usize,
    pub access_count: u64,
}

/// Extended key-value storage for DHT with spam protection
#[derive(Debug)]
pub struct DhtStorage {
    pub records: HashMap<HashId, DhtRecord>,
    pub storage_bytes: usize,
    pub request_counters: HashMap<String, RequestTracker>,
    pub blocked_origins: HashMap<String, u64>,
    pub sus_peers: HashMap<[u8; 8], SusPeerTracker>,
}

impl Default for DhtStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl DhtStorage {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
            storage_bytes: 0,
            request_counters: HashMap::new(),
            blocked_origins: HashMap::new(),
            sus_peers: HashMap::new(),
        }
    }

    /// Current Unix timestamp (seconds)
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }

    /// Check rate limit for origin
    /// Check rate limit for origin (Stage 2.1: Per-second and per-minute limits)
    pub fn check_rate_limit(&mut self, origin: &str, req_type: RequestType) -> Result<(), String> {
        let limit = get_rate_limit(req_type);
        let now = Self::now();

        // Check if origin is blocked
        if let Some(blocked_until) = self.blocked_origins.get(origin) {
            if now < *blocked_until {
                return Err(format!("Origin {} is blocked until {} (rate limit exceeded)", 
                    origin, blocked_until));
            } else {
                // Unblock origin
                self.blocked_origins.remove(origin);
            }
        }

        // Get or create tracker for this origin
        let tracker = self.request_counters.entry(origin.to_string()).or_insert(RequestTracker {
            last_second_reset: now,
            requests_this_second: 0,
            last_minute_reset: now,
            requests_this_minute: 0,
        });

        // Reset per-second counter if second passed
        if now.saturating_sub(tracker.last_second_reset) >= 1 {
            tracker.last_second_reset = now;
            tracker.requests_this_second = 0;
        }

        // Reset per-minute counter if minute passed
        if now.saturating_sub(tracker.last_minute_reset) >= 60 {
            tracker.last_minute_reset = now;
            tracker.requests_this_minute = 0;
        }

        // Check per-second limit (PRIMARY protection)
        if tracker.requests_this_second >= limit.per_second {
            // Block for configured duration
            self.blocked_origins.insert(origin.to_string(), now + limit.block_duration);
            return Err(format!("Rate limit exceeded for {}: {} req/sec (max {})", 
                origin, tracker.requests_this_second, limit.per_second));
        }

        // Check per-minute limit (SECONDARY protection)
        if tracker.requests_this_minute >= limit.per_minute {
            self.blocked_origins.insert(origin.to_string(), now + limit.block_duration);
            return Err(format!("Rate limit exceeded for {}: {} req/min (max {})", 
                origin, tracker.requests_this_minute, limit.per_minute));
        }

        // Increment counters
        tracker.requests_this_second += 1;
        tracker.requests_this_minute += 1;
        
        Ok(())
    }

    /// Check storage quotas
    fn check_storage_quotas(&self, new_value_size: usize) -> Result<(), String> {
        if new_value_size > MAX_RECORD_SIZE {
            return Err(format!("Record size {} exceeds maximum {}", new_value_size, MAX_RECORD_SIZE));
        }

        if self.records.len() >= MAX_RECORDS {
            return Err(format!("Storage has {} records, maximum {}", self.records.len(), MAX_RECORDS));
        }

        if self.storage_bytes.saturating_add(new_value_size) > MAX_STORAGE_BYTES {
            return Err(format!("Storage size {} bytes exceeds maximum {}",
                self.storage_bytes.saturating_add(new_value_size), MAX_STORAGE_BYTES));
        }

        Ok(())
    }

    /// Remove oldest records to free space
    fn evict_oldest_records(&mut self, needed_space: usize) {
        let mut records_by_age: Vec<(HashId, usize)> = self.records
            .iter()
            .map(|(key, rec)| (*key, rec.size))
            .collect();

        records_by_age.sort_by(|&(key1, _), &(key2, _)| {
            let rec1 = self.records.get(&key1).unwrap();
            let rec2 = self.records.get(&key2).unwrap();
            rec1.timestamp.cmp(&rec2.timestamp)
        });

        let mut freed_space = 0;
        for (key, size) in records_by_age {
            if freed_space >= needed_space {
                break;
            }

            freed_space += size;
            self.records.remove(&key);
        }

        self.storage_bytes = self.storage_bytes.saturating_sub(freed_space);
    }

    /// Store value with quota checks and rate limiting
    pub fn store_with_quota(&mut self, key: HashId, value: Vec<u8>, origin: String) -> Result<(), String> {
        self.check_rate_limit(&origin, RequestType::Store)?;

        let value_size = value.len();
        self.check_storage_quotas(value_size)?;

        // Free space if needed
        if self.storage_bytes.saturating_add(value_size) > MAX_STORAGE_BYTES {
            let needed_space = self.storage_bytes.saturating_add(value_size).saturating_sub(MAX_STORAGE_BYTES);
            self.evict_oldest_records(needed_space);
        }

        // Remove old record if exists
        if let Some(old_record) = self.records.get(&key) {
            self.storage_bytes = self.storage_bytes.saturating_sub(old_record.size);
        }

        let rec = DhtRecord {
            value: value.clone(),
            timestamp: Self::now(),
            origin: origin.clone(),
            size: value_size,
            access_count: 0,
        };

        self.records.insert(key, rec);
        self.storage_bytes += value_size;

        Ok(())
    }

    /// Store a signed NodeRecord with signature verification
    /// Rejects records with invalid signatures or expired node names
    pub fn store_node_record(&mut self, record: &NodeRecord, origin: String) -> Result<(), String> {
        // 🔒 Verify signature before storing
        if !record.verify() {
            return Err(format!("Rejected NodeRecord with invalid signature from {}", origin));
        }

        // 🔒 Check if record is expired
        if record.is_expired() {
            return Err(format!("Rejected expired NodeRecord from {}", origin));
        }

        // Serialize record for storage
        let value = record.to_bytes()?;

        // Store under node_name as key
        let key = HashId(record.node_name.0);

        // Check for existing record with higher sequence
        if let Some(existing_value) = self.get(&key) {
            if let Ok(existing_record) = NodeRecord::from_bytes(&existing_value) {
                if existing_record.sequence >= record.sequence {
                    return Err(format!("Rejected stale NodeRecord (seq {} vs {})",
                        record.sequence, existing_record.sequence));
                }
            }
        }

        // Store with quota checks
        self.store_with_quota(key, value, origin)
    }

    /// Get NodeRecord by node name
    pub fn get_node_record(&mut self, node_name: &HashId) -> Option<NodeRecord> {
        if let Some(data) = self.get(node_name) {
            if let Ok(record) = NodeRecord::from_bytes(&data) {
                // Verify on retrieval too
                if record.verify() && !record.is_expired() {
                    return Some(record);
                }
            }
        }
        None
    }

    /// Get value with rate limit check
    pub fn get_with_quota(&mut self, key: &HashId, origin: &str) -> Result<Option<Vec<u8>>, String> {
        self.check_rate_limit(origin, RequestType::FindValue)?;

        if let Some(rec) = self.records.get_mut(key) {
            rec.access_count += 1;

            // Check TTL
            if Self::now().saturating_sub(rec.timestamp) > DHT_TTL {
                self.storage_bytes = self.storage_bytes.saturating_sub(rec.size);
                self.records.remove(key);
                return Ok(None);
            }

            return Ok(Some(rec.value.clone()));
        }

        Ok(None)
    }

    /// Legacy compatibility
    pub fn store(&mut self, key: HashId, value: Vec<u8>) {
        let _ = self.store_with_quota(key, value, "legacy".to_string());
    }

    /// Legacy compatibility
    pub fn get(&mut self, key: &HashId) -> Option<Vec<u8>> {
        self.get_with_quota(key, "legacy").unwrap_or(None)
    }

    /// Cleanup expired records and blocked origins
    pub fn cleanup(&mut self) {
        let now = Self::now();

        // Remove expired records
        let mut removed_size = 0;
        self.records.retain(|_, rec| {
            if now.saturating_sub(rec.timestamp) > DHT_TTL {
                removed_size += rec.size;
                false
            } else {
                true
            }
        });

        self.storage_bytes = self.storage_bytes.saturating_sub(removed_size);

        // Clear expired blocks
        self.blocked_origins.retain(|_, &mut blocked_until| blocked_until > now);
        // Clear old request counters (older than 10 minutes)
        self.request_counters.retain(|_, tracker| {
            now.saturating_sub(tracker.last_minute_reset) <= 600
        });

        // Cleanup old SUS peer entries (Stage 2.3)
        self.cleanup_sus_peers();
    }

    /// Get storage statistics
    pub fn get_stats(&self) -> StorageStats {
        StorageStats {
            records_count: self.records.len(),
            storage_bytes: self.storage_bytes,
            blocked_origins: self.blocked_origins.len(),
            active_requesters: self.request_counters.len(),
        }
    }
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStats {
    pub records_count: usize,
    pub storage_bytes: usize,
    pub blocked_origins: usize,
    pub active_requesters: usize,
}

/// Peer endpoint information for DHT storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEndpoint {
    pub address: String,
    pub last_seen: u64,
    pub connection_type: String,
    pub quality: f32,
    pub success_count: u32,
    pub failure_count: u32,
}

impl PeerEndpoint {
    pub fn new(address: String, connection_type: String) -> Self {
        Self {
            address,
            last_seen: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs(),
            connection_type,
            quality: 0.5,
            success_count: 0,
            failure_count: 0,
        }
    }

    pub fn update_last_seen(&mut self) {
        self.last_seen = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
    }

    pub fn update_quality(&mut self, success: bool) {
        if success {
            self.success_count += 1;
            self.quality = (self.quality * 0.9) + (1.0 * 0.1);
        } else {
            self.failure_count += 1;
            self.quality = (self.quality * 0.9) + (0.0 * 0.1);
        }
        self.quality = self.quality.clamp(0.0, 1.0);
    }

    pub fn is_fresh(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();

        (now - self.last_seen) < DHT_TTL
    }
}

// ========== Stage 2.3: SUS Peer Blocking System ==========
// SUS = Suspicious Unverified Source
// Blocks peers that repeatedly fail verification

/// SUS peer tracking info
#[derive(Debug, Clone)]
pub struct SusPeerTracker {
    /// Number of failed verifications
    pub failed_count: u32,
    /// First failure timestamp
    pub first_failure: u64,
    /// Last failure timestamp
    pub last_failure: u64,
}

/// SUS threshold: block after N failed verifications
pub const SUS_THRESHOLD: u32 = 3;

/// SUS block duration: 1 hour
pub const SUS_BLOCK_DURATION: u64 = 60 * 60;

/// SUS cleanup interval: remove old entries after 24 hours
pub const SUS_CLEANUP_AGE: u64 = 24 * 60 * 60;

impl DhtStorage {
    /// Add SUS peer: record a failed verification (Stage 2.3)
    /// Returns true if peer should be blocked
    pub fn add_sus_peer(&mut self, peer_id: &HashId) -> bool {
        let now = Self::now();
        let entry = self.sus_peers.entry(peer_id.to_fixed()).or_insert(SusPeerTracker {
            failed_count: 0,
            first_failure: now,
            last_failure: now,
        });

        entry.failed_count += 1;
        entry.last_failure = now;

        // Check if threshold exceeded
        if entry.failed_count >= SUS_THRESHOLD {
            println!("[dht] 🚫 SUS peer blocked: {} ({} failures)", 
                hex::encode(&peer_id.0[..8]), entry.failed_count);
            true
        } else {
            println!("[dht] ⚠️  SUS peer warning: {} ({} failures)", 
                hex::encode(&peer_id.0[..8]), entry.failed_count);
            false
        }
    }

    /// Check if peer is SUS blocked (Stage 2.3)
    pub fn is_sus_blocked(&self, peer_id: &HashId) -> bool {
        if let Some(tracker) = self.sus_peers.get(&peer_id.to_fixed()) {
            if tracker.failed_count >= SUS_THRESHOLD {
                // Check if block expired
                let now = Self::now();
                if now.saturating_sub(tracker.last_failure) < SUS_BLOCK_DURATION {
                    return true;
                }
            }
        }
        false
    }

    /// Get SUS failure count for peer
    pub fn get_sus_failures(&self, peer_id: &HashId) -> u32 {
        self.sus_peers.get(&peer_id.to_fixed())
            .map(|t| t.failed_count)
            .unwrap_or(0)
    }

    /// Clear SUS entry for peer (manual unblock)
    pub fn clear_sus_peer(&mut self, peer_id: &HashId) {
        self.sus_peers.remove(&peer_id.to_fixed());
    }

    /// Cleanup old SUS entries (older than SUS_CLEANUP_AGE)
    pub fn cleanup_sus_peers(&mut self) {
        let now = Self::now();
        self.sus_peers.retain(|_, tracker| {
            now.saturating_sub(tracker.first_failure) < SUS_CLEANUP_AGE
        });
    }
}

// Helper trait to convert HashId to fixed-size key for SUS tracking
trait ToFixed {
    fn to_fixed(&self) -> [u8; 8];
}

impl ToFixed for HashId {
    fn to_fixed(&self) -> [u8; 8] {
        // Use first 8 bytes as key (good enough for SUS tracking)
        let mut key = [0u8; 8];
        key.copy_from_slice(&self.0[..8]);
        key
    }
}

#[cfg(test)]
mod hostile_peer_origin_isolation_tests {
    use super::*;

    /// "Valid hostile peer" audit (2026-09-15/16): netlayer::transport's
    /// live STORE/FIND_VALUE handlers used to call the bare
    /// `.store()`/`.get()` methods, which hardcode `origin="legacy"` —
    /// meaning every peer on the network shared ONE rate-limit bucket
    /// here, regardless of who actually sent the request. This proves
    /// the fix (real per-peer origin, e.g. hex-encoded node_id) actually
    /// isolates one abusive peer's rate limit from everyone else's,
    /// using DhtStorage exactly as the live handlers now do.
    #[test]
    fn one_peer_exhausting_their_quota_does_not_block_a_different_peer() {
        let mut storage = DhtStorage::new();
        let attacker_origin = "attacker-node-id-hex".to_string();
        let honest_origin = "honest-node-id-hex".to_string();

        // Exhaust the Store rate limit (5/sec) for the attacker only.
        let store_limit = get_rate_limit(RequestType::Store).per_second;
        for i in 0..store_limit {
            let key = HashId([i as u8; 32]);
            storage.store_with_quota(key, vec![1, 2, 3], attacker_origin.clone())
                .expect("attacker's own requests within their own limit must succeed");
        }
        let attacker_blocked = storage.store_with_quota(
            HashId([200; 32]), vec![9], attacker_origin.clone(),
        );
        assert!(attacker_blocked.is_err(), "attacker must be rate-limited after exceeding their own quota");

        // A genuinely different, honest peer's very next STORE must NOT
        // be caught by the attacker's block — this is exactly the
        // cross-peer DoS the shared "legacy" bucket used to cause.
        let honest_result = storage.store_with_quota(
            HashId([201; 32]), vec![4, 5, 6], honest_origin.clone(),
        );
        assert!(
            honest_result.is_ok(),
            "CROSS-PEER DOS: an honest peer's request was blocked by a DIFFERENT peer's rate limit: {:?}",
            honest_result
        );

        // Same isolation for the read side (FIND_VALUE).
        let key = HashId([201; 32]);
        for _ in 0..get_rate_limit(RequestType::FindValue).per_second {
            let _ = storage.get_with_quota(&key, &attacker_origin);
        }
        assert!(
            storage.get_with_quota(&key, &attacker_origin).is_err(),
            "attacker must be rate-limited on lookups too after exceeding their own quota"
        );
        assert!(
            storage.get_with_quota(&key, &honest_origin).is_ok(),
            "CROSS-PEER DOS: an honest peer's lookup was blocked by a DIFFERENT peer's rate limit"
        );
    }
}
