// src/dht/bucket.rs
//! Kademlia K-Buckets Implementation
//! ===================================
//!
//! K-bucket management for peer routing with XOR distance metric

use crate::util::HashId;
use std::time::{SystemTime, Duration};

/// Maximum bucket size (Kademlia recommends K=20)
pub const K_BUCKET_SIZE: usize = 20;

/// Peer in k-bucket with LRU metadata
#[derive(Debug, Clone)]
pub struct BucketPeer {
    pub id: HashId,
    pub addr: String,
    pub last_seen: u64,
    pub access_count: u64,
}

/// One k-bucket - group of peers at "similar" distance with LRU eviction
#[derive(Debug, Clone)]
pub struct KBucket {
    pub peers: Vec<BucketPeer>,
}

impl Default for KBucket {
    fn default() -> Self {
        Self::new()
    }
}

impl KBucket {
    pub fn new() -> Self {
        Self {
            peers: Vec::with_capacity(K_BUCKET_SIZE)
        }
    }

    /// Current Unix timestamp (seconds)
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }

    /// Add or update peer with LRU eviction
    pub fn add_peer(&mut self, mut peer: BucketPeer) {
        if peer.last_seen == 0 {
            peer.last_seen = Self::now();
        }

        // Check if peer already exists
        if let Some(existing_peer) = self.peers.iter_mut().find(|p| p.id == peer.id) {
            existing_peer.last_seen = Self::now();
            existing_peer.access_count += 1;
            existing_peer.addr = peer.addr;
            return;
        }

        // If bucket not full, just add
        if self.peers.len() < K_BUCKET_SIZE {
            self.peers.push(peer);
            return;
        }

        // Bucket full - apply LRU eviction
        self.evict_lru_peer();
        self.peers.push(peer);
    }

    /// LRU eviction: remove oldest peer
    pub fn evict_lru_peer(&mut self) {
        if self.peers.is_empty() {
            return;
        }

        let oldest_index = self.peers
            .iter()
            .enumerate()
            .min_by_key(|(_, peer)| (peer.last_seen, peer.access_count))
            .map(|(idx, _)| idx)
            .unwrap_or(0);

        self.peers.remove(oldest_index);
    }

    /// Remove inactive peers (older than specified time)
    pub fn remove_inactive_peers(&mut self, max_age_seconds: u64) {
        let now = Self::now();
        self.peers.retain(|peer| {
            now.saturating_sub(peer.last_seen) <= max_age_seconds
        });
    }

    /// Get all peers from this bucket
    pub fn all(&self) -> Vec<BucketPeer> {
        self.peers.clone()
    }

    /// Get bucket statistics
    pub fn get_stats(&self) -> BucketStats {
        BucketStats {
            peer_count: self.peers.len(),
            capacity: K_BUCKET_SIZE,
            utilization_percent: (self.peers.len() as f64 / K_BUCKET_SIZE as f64) * 100.0,
        }
    }
}

/// Bucket statistics
#[derive(Debug, Clone)]
pub struct BucketStats {
    pub peer_count: usize,
    pub capacity: usize,
    pub utilization_percent: f64,
}

/// Kademlia table: set of k-buckets with memory management
#[derive(Debug, Clone)]
pub struct KTable {
    pub buckets: Vec<KBucket>,
    pub max_inactive_time: u64,
}

impl Default for KTable {
    fn default() -> Self {
        Self::new()
    }
}

impl KTable {
    /// Create table with 256 buckets
    pub fn new() -> Self {
        Self::with_inactive_timeout(3600) // 1 hour default
    }

    /// Create table with specified inactive timeout
    pub fn with_inactive_timeout(max_inactive_time: u64) -> Self {
        let mut buckets = Vec::with_capacity(256);
        for _ in 0..256 {
            buckets.push(KBucket::new());
        }
        Self {
            buckets,
            max_inactive_time,
        }
    }

    /// Get bucket index by XOR distance
    fn bucket_index(local: &HashId, target: &HashId) -> usize {
        let dist = xor_distance(local, target);
        // Index = by highest non-zero bit
        for (byte_idx, b) in dist.iter().enumerate() {
            if *b != 0 {
                let leading = b.leading_zeros() as usize;
                let bit_index = byte_idx * 8 + (7 - leading);
                return bit_index.min(255);
            }
        }
        // Exact match - put in bucket 0
        0
    }

    /// Add peer to table relative to local_id
    pub fn add_peer(&mut self, local_id: &HashId, peer: BucketPeer) {
        let idx = Self::bucket_index(local_id, &peer.id);
        if let Some(bucket) = self.buckets.get_mut(idx) {
            bucket.add_peer(peer);
        }
    }

    /// Find closest peers to target with result limit
    pub fn closest(&self, target: &HashId, _local: &HashId) -> Vec<BucketPeer> {
        let mut all: Vec<BucketPeer> = self
            .buckets
            .iter()
            .flat_map(|b| b.all().into_iter())
            .collect();

        all.sort_by(|a, b| {
            let da = xor_distance(&a.id, target);
            let db = xor_distance(&b.id, target);
            da.cmp(&db)
        });

        all.into_iter().take(K_BUCKET_SIZE).collect()
    }

    /// Periodic cleanup of inactive peers
    pub fn cleanup_inactive_peers(&mut self) {
        for bucket in &mut self.buckets {
            bucket.remove_inactive_peers(self.max_inactive_time);
        }
    }

    /// Get table statistics
    pub fn get_stats(&self) -> TableStats {
        let mut total_peers = 0;
        let mut total_utilization = 0.0;
        let mut active_buckets = 0;

        for bucket in &self.buckets {
            let stats = bucket.get_stats();
            total_peers += stats.peer_count;
            total_utilization += stats.utilization_percent;
            if stats.peer_count > 0 {
                active_buckets += 1;
            }
        }

        TableStats {
            total_peers,
            total_buckets: self.buckets.len(),
            active_buckets,
            average_utilization: total_utilization / self.buckets.len() as f64,
            max_bucket_size: K_BUCKET_SIZE,
            max_inactive_time: self.max_inactive_time,
        }
    }

    /// Get peer count
    pub fn peer_count(&self) -> usize {
        self.buckets.iter().map(|b| b.peers.len()).sum()
    }
}

/// Table statistics
#[derive(Debug, Clone)]
pub struct TableStats {
    pub total_peers: usize,
    pub total_buckets: usize,
    pub active_buckets: usize,
    pub average_utilization: f64,
    pub max_bucket_size: usize,
    pub max_inactive_time: u64,
}

/// XOR distance between two 256-bit hashes
pub fn xor_distance(a: &HashId, b: &HashId) -> [u8; 32] {
    let mut out = [0u8; 32];
    let a_bytes = a.as_ref();
    let b_bytes = b.as_ref();
    for i in 0..32 {
        out[i] = a_bytes[i] ^ b_bytes[i];
    }
    out
}
