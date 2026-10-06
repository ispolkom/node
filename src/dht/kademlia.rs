// src/dht/kademlia.rs
//! Kademlia DHT Core
//! ==================
//!
//! Main Kademlia orchestration for P2P node discovery and storage
use crate::observability::NetworkMetrics;

use crate::dht::bucket::{KTable, BucketPeer};
use crate::dht::hot_cache::{HotKeyCache, HotCacheConfig};
use crate::dht::storage::DhtStorage;
use crate::dht::record::NodeRecord;
use crate::util::HashId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::future::Future;
use std::pin::Pin;
use tokio::sync::oneshot;

/// Type alias for DHT RPC sender function
pub type DhtRpcSender = Box<dyn Fn(HashId, HashId, Vec<u8>) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send>> + Send + Sync>;

/// Kademlia constants
pub const ALPHA: usize = 3;  // Parallelism factor
pub const K: usize = 20;      // Bucket size (k-bucket capacity)

/// Main Kademlia structure for our node
pub struct Kademlia {
    pub node_id: HashId,
    pub ktable: KTable,
    pub storage: DhtStorage,
    pub pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<Vec<u8>>>>>,
    pub next_request_id: Arc<Mutex<u64>>,
    pub metrics: Option<NetworkMetrics>,
    pub adaptive_alpha: AdaptiveAlpha,
    pub hot_cache: HotKeyCache,
    pub rpc_sender: Option<DhtRpcSender>,
}

impl Kademlia {
    /// Create Kademlia node with empty table and storage
    pub fn new(node_id: HashId) -> Self {
        Self {
            node_id,
            ktable: KTable::new(),
            storage: DhtStorage::new(),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            next_request_id: Arc::new(Mutex::new(0)),
            metrics: None,
            adaptive_alpha: AdaptiveAlpha::new(),
            hot_cache: HotKeyCache::new(HotCacheConfig::default()),
            rpc_sender: None,
        }
    }

    /// Set RPC sender for distributed queries
    pub fn set_rpc_sender(&mut self, sender: DhtRpcSender) {
        self.rpc_sender = Some(sender);
    }

    /// Add peer to k-buckets
    pub fn add_peer(&mut self, peer_id: HashId, addr: String) {
        let entry = BucketPeer {
            id: peer_id,
            addr,
            last_seen: std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::ZERO)
                .as_secs(),
            access_count: 1,
        };
        self.ktable.add_peer(&self.node_id, entry);
    }

    /// Find N closest nodes to target
    pub fn closest_n(&self, target: &HashId, n: usize) -> Vec<(HashId, String)> {
        self.ktable
            .closest(target, &self.node_id)
            .into_iter()
            .take(n)
            .map(|bp| (bp.id, bp.addr))
            .collect()
    }

    /// Compatible method for NetLayer: returns "several closest nodes" (default K=8)
    pub fn closest_nodes(&self, target: &HashId) -> Vec<(HashId, String)> {
        self.closest_n(target, 8)
    }

    /// Store value locally under key (legacy compatibility)
    pub fn store_value(&mut self, key: HashId, value: Vec<u8>) {
        self.storage.store(key, value);
    }

    /// Store value with spam protection
    pub fn store_value_protected(&mut self, key: HashId, value: Vec<u8>, origin: String) -> Result<(), String> {
        self.storage.store_with_quota(key, value, origin)
    }

    /// Get value by key (if exists and not expired)
    pub fn get_value(&mut self, key: &HashId) -> Option<Vec<u8>> {
        self.storage.get(key)
    }

    /// Get value with spam protection
    pub fn get_value_protected(&mut self, key: &HashId, origin: &str) -> Result<Option<Vec<u8>>, String> {
        self.storage.get_with_quota(key, origin)
    }

    /// Cleanup storage from expired records
    pub fn cleanup_storage(&mut self) {
        self.storage.cleanup();
    }

    /// Find specific peer by ID (returns address if found)
    pub fn find_node(&self, peer_id: &HashId) -> Option<String> {
        let closest_peers = self.closest_n(peer_id, 1);
        for (id, addr) in closest_peers {
            if id == *peer_id {
                return Some(addr);
            }
        }
        None
    }

    /// Add node to DHT (alias for add_peer)
    pub fn insert_node(&mut self, node_id: HashId, addr: String) {
        self.add_peer(node_id, addr);
    }

    /// Find closest nodes to target (alias for closest_nodes)
    pub fn find_nearest_nodes(&self, target: &HashId, count: usize) -> Vec<(HashId, String)> {
        self.closest_n(target, count)
    }

    /// Perform storage cleanup
    pub fn cleanup(&mut self) {
        self.storage.cleanup();
    }

    /// Store value in DHT with replication to closest nodes
    pub fn store_to_dht(&mut self, key: HashId, value: Vec<u8>, replication_factor: usize) -> Result<Vec<String>, String> {
        self.store_value(key, value.clone());
        let nearest_nodes = self.closest_n(&key, replication_factor);
        let replication_targets: Vec<String> = nearest_nodes
            .into_iter()
            .map(|(_, addr)| addr)
            .collect();
        Ok(replication_targets)
    }

    /// Find value in DHT (first local, then from closest nodes)
    pub async fn find_value_distributed(&mut self, key: &HashId, search_depth: usize) -> Option<Vec<u8>> {
        if let Some(value) = self.get_value(key) {
            return Some(value);
        }

        let nearest_nodes = self.closest_n(key, search_depth);
        
        let rpc_sender = match &self.rpc_sender {
            Some(sender) => sender,
            None => return None,
        };

        for (node_id, _addr) in nearest_nodes {
            use crate::dht::messages::{DhtQuery, DhtQueryType};
            let query = DhtQuery {
                request_id: 0,
                query_type: DhtQueryType::FindValue,
                key: *key,
                value: None,
                limit: K as u8,
            };
            let query_bytes = query.to_bytes();
            
            match rpc_sender(node_id, *key, query_bytes).await {
                Ok(response_bytes) => {
                    if let Some(response) = crate::dht::messages::DhtResponse::from_bytes(&response_bytes) {
                        if let Some(value) = response.value {
                            return Some(value);
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        None
    }

    /// Get storage statistics
    pub fn get_storage_stats(&self) -> usize {
        self.storage.get_stats().records_count
    }

    /// Check if value is stored locally
    pub fn has_value(&mut self, key: &HashId) -> bool {
        self.get_value(key).is_some()
    }

    /// Get extended storage statistics
    pub fn get_storage_stats_extended(&self) -> crate::dht::storage::StorageStats {
        self.storage.get_stats()
    }

    /// Check rate limit for origin
    pub fn check_rate_limit(&mut self, origin: &str, req_type: crate::dht::storage::RequestType) -> Result<(), String> {
        self.storage.check_rate_limit(origin, req_type)
    }

    /// Store a signed NodeRecord in DHT
    pub fn store_node_record(&mut self, record: &NodeRecord, origin: String) -> Result<(), String> {
        self.storage.store_node_record(record, origin)
    }

    /// Get NodeRecord by node name
    pub fn get_node_record(&mut self, node_name: &HashId) -> Option<NodeRecord> {
        self.storage.get_node_record(node_name)
    }

    /// Publish this node's own record to DHT
    pub fn publish_own_record(&mut self, record: NodeRecord) -> Result<(), String> {
        let origin = format!("local:{}", hex::encode(&record.node_name.0[..8]));
        self.store_node_record(&record, origin)
    }

    /// Find NodeRecords for specific nodes
    pub fn find_node_records(&mut self, node_names: &[HashId]) -> Vec<Option<NodeRecord>> {
        node_names.iter()
            .map(|name| self.get_node_record(name))
            .collect()
    }

    /// Cleanup expired NodeRecords
    pub fn cleanup_node_records(&mut self) {
        self.storage.cleanup();
    }

    /// Iterative FIND_NODE - finds K closest nodes to target
    pub fn iterative_find_node(&self, target: &HashId) -> Vec<BucketPeer> {
        let start_time = std::time::Instant::now();
        let mut hops = 0;
        
        let mut shortlist = self.ktable.closest(target, &self.node_id);
        let mut queried = std::collections::HashSet::new();
        let mut closest = shortlist.clone();
        
        loop {
            let to_query: Vec<BucketPeer> = shortlist
                .iter()
                .filter(|p| !queried.contains(&p.id))
                .take(self.adaptive_alpha.current())
                .cloned()
                .collect();
            
            if to_query.is_empty() {
                break;
            }
            
            for peer in &to_query {
                queried.insert(peer.id.clone());
            }
            
            hops += to_query.len();
            
            for peer in to_query {
                let found = self.ktable.closest(target, &self.node_id);
                for new_peer in found {
                    if !shortlist.iter().any(|p| p.id == new_peer.id) {
                        shortlist.push(new_peer);
                    }
                }
            }
            
            shortlist.sort_by(|a, b| {
                let dist_a = crate::dht::bucket::xor_distance(&a.id, target);
                let dist_b = crate::dht::bucket::xor_distance(&b.id, target);
                dist_a.cmp(&dist_b)
            });
            shortlist.truncate(K);
            
            if shortlist.first().map(|p| &p.id) == closest.first().map(|p| &p.id) {
                break;
            }
            closest = shortlist.clone();
        }
        
        if let Some(ref metrics) = self.metrics {
            let latency_ms = start_time.elapsed().as_millis() as u64;
            metrics.record_dht_lookup(true, hops as u64, latency_ms);
            metrics.inc_dht_lookups();
        }
        
        shortlist
    }

    /// Find value iteratively
    pub fn find_value_iterative(&mut self, key: &HashId) -> (Option<Vec<u8>>, Vec<BucketPeer>) {
        let start_time = std::time::Instant::now();
        let mut hops = 0;
        
        let mut shortlist = self.ktable.closest(key, &self.node_id);
        let mut queried = std::collections::HashSet::new();
        let mut closest = shortlist.clone();
        
        loop {
            let to_query: Vec<BucketPeer> = shortlist
                .iter()
                .filter(|p| !queried.contains(&p.id))
                .take(self.adaptive_alpha.current())
                .cloned()
                .collect();
            
            if to_query.is_empty() {
                break;
            }
            
            for peer in &to_query {
                queried.insert(peer.id.clone());
            }
            
            hops += to_query.len();
            
            for _peer in to_query {
                if let Some(value) = self.storage.get(key) {
                    if let Some(ref metrics) = self.metrics {
                        let latency_ms = start_time.elapsed().as_millis() as u64;
                        metrics.record_dht_lookup(true, hops as u64, latency_ms);
                        metrics.inc_dht_lookups();
                    }
                    return (Some(value), shortlist);
                }
                let found = self.ktable.closest(key, &self.node_id);
                for new_peer in found {
                    if !shortlist.iter().any(|p| p.id == new_peer.id) {
                        shortlist.push(new_peer);
                    }
                }
            }
            
            shortlist.sort_by(|a, b| {
                let dist_a = crate::dht::bucket::xor_distance(&a.id, key);
                let dist_b = crate::dht::bucket::xor_distance(&b.id, key);
                dist_a.cmp(&dist_b)
            });
            shortlist.truncate(K);
            
            if shortlist.first().map(|p| &p.id) == closest.first().map(|p| &p.id) {
                break;
            }
            closest = shortlist.clone();
        }
        
        if let Some(ref metrics) = self.metrics {
            let latency_ms = start_time.elapsed().as_millis() as u64;
            metrics.record_dht_lookup(false, hops as u64, latency_ms);
            metrics.inc_dht_lookups();
        }
        
        (None, shortlist)
    }

    /// Find closest peers
    pub fn find_closest_peers(&self, target: &HashId, count: usize) -> Vec<BucketPeer> {
        self.ktable.closest(target, &self.node_id)
            .into_iter()
            .take(count)
            .collect()
    }

    /// Refresh buckets
    pub fn refresh_buckets(&self) -> Vec<HashId> {
        let mut targets = Vec::new();
        for i in 0..160 {
            let bucket_prefix = i as u8;
            let mut random_id = [0u8; 32];
            if bucket_prefix < 32 {
                let first_byte = rand::random::<u8>();
                let mask = 0xFFu8 << (8 - bucket_prefix);
                random_id[0] = (first_byte & (!mask)) | (self.node_id.0[0] & mask);
            }
            for j in 1..32 {
                random_id[j] = rand::random::<u8>();
            }
            targets.push(HashId(random_id));
        }
        targets
    }

    /// Replicate value
    pub fn replicate_value(&self, key: &HashId, value: &[u8]) -> Vec<String> {
        let k_closest = self.ktable.closest(key, &self.node_id);
        k_closest
            .into_iter()
            .take(K)
            .map(|peer| peer.addr)
            .collect()
    }

    /// Prepare replication
    pub fn prepare_replication(&self, key: &HashId, value: &[u8]) -> Vec<String> {
        let closest_nodes = self.ktable.closest(key, &self.node_id);
        let k_closest: Vec<BucketPeer> = closest_nodes.into_iter().take(K).collect();
        k_closest
            .into_iter()
            .map(|peer| peer.addr)
            .collect()
    }

    /// Get all fresh records
    pub fn get_all_fresh_records(&mut self) -> Vec<NodeRecord> {
        let mut records = Vec::new();
        let keys: Vec<_> = self.storage.records.keys().cloned().collect();
        for key in keys {
            if let Some(record) = self.get_node_record(&key) {
                records.push(record);
            }
        }
        records
    }

    /// Generate next request ID
    pub fn next_request_id(&self) -> u64 {
        let mut id = self.next_request_id.lock().unwrap();
        let current = *id;
        *id = current.wrapping_add(1);
        current
    }
    
    /// Register pending request
    pub fn register_pending(&self, request_id: u64) -> oneshot::Receiver<Vec<u8>> {
        let (tx, rx) = oneshot::channel();
        self.pending_requests.lock().unwrap().insert(request_id, tx);
        rx
    }
    
    /// Handle incoming DHT response
    pub fn handle_response(&self, request_id: u64, data: Vec<u8>) -> bool {
        if let Some(sender) = self.pending_requests.lock().unwrap().remove(&request_id) {
            let _ = sender.send(data);
            true
        } else {
            false
        }
    }

    /// Start background tasks
    pub fn start_background_tasks_mutex(dht: Arc<tokio::sync::Mutex<Self>>) {
        let dht_refresh = dht.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                let dht_guard = dht_refresh.lock().await;
                let targets = dht_guard.refresh_buckets();
                for target in targets {
                    dht_guard.iterative_find_node(&target);
                }
            }
        });
        
        let dht_cleanup = dht.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                let mut dht_guard = dht_cleanup.lock().await;
                dht_guard.storage.cleanup();
            }
        });
    }

    /// Set metrics collector
    pub fn set_metrics(&mut self, metrics: NetworkMetrics) {
        self.metrics = Some(metrics);
    }

    /// Record RPC success
    pub fn record_rpc_success(&mut self) {
        self.adaptive_alpha.record_success();
    }
    
    /// Record RPC timeout
    pub fn record_rpc_timeout(&mut self) {
        self.adaptive_alpha.record_timeout();
    }
}

/// Adaptive parallelism factor
pub struct AdaptiveAlpha {
    base_alpha: usize,
    current_alpha: usize,
    timeout_count: u64,
    success_count: u64,
}

impl AdaptiveAlpha {
    pub fn new() -> Self {
        Self {
            base_alpha: ALPHA,
            current_alpha: ALPHA,
            timeout_count: 0,
            success_count: 0,
        }
    }
    
    pub fn record_timeout(&mut self) {
        self.timeout_count += 1;
        self.adjust();
    }
    
    pub fn record_success(&mut self) {
        self.success_count += 1;
        self.adjust();
    }
    
    fn adjust(&mut self) {
        let total = self.timeout_count + self.success_count;
        if total < 10 {
            return;
        }
        
        let timeout_ratio = self.timeout_count as f64 / total as f64;
        
        if timeout_ratio > 0.3 {
            self.current_alpha = (self.base_alpha * 2).min(10);
        } else if timeout_ratio < 0.1 && self.current_alpha > self.base_alpha {
            self.current_alpha = self.base_alpha;
        }
    }
    
    pub fn current(&self) -> usize {
        self.current_alpha
    }
    
    pub fn reset_counters(&mut self) {
        self.timeout_count = 0;
        self.success_count = 0;
    }
}

impl std::fmt::Debug for Kademlia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kademlia")
            .field("node_id", &self.node_id)
            .field("ktable", &self.ktable)
            .field("storage", &self.storage)
            .field("pending_requests", &self.pending_requests)
            .field("next_request_id", &self.next_request_id)
            .field("metrics", &self.metrics)
            .field("adaptive_alpha", &self.adaptive_alpha)
            .field("hot_cache", &self.hot_cache)
            .field("rpc_sender", &"<function>")
            .finish()
    }
}

impl std::fmt::Debug for AdaptiveAlpha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdaptiveAlpha")
            .field("base_alpha", &self.base_alpha)
            .field("current_alpha", &self.current_alpha)
            .field("timeout_count", &self.timeout_count)
            .field("success_count", &self.success_count)
            .finish()
    }
}
