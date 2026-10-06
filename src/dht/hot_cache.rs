//! Hot Key Cache
//! =============

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use crate::util::HashId;

/// Cache entry with metadata
#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub value: Vec<u8>,
    pub access_count: u64,
    pub last_access: Instant,
    pub created_at: Instant,
    pub last_decay: Instant,
}

impl CacheEntry {
    pub fn new(value: Vec<u8>) -> Self {
        let now = Instant::now();
        Self {
            value,
            access_count: 1,
            last_access: now,
            created_at: now,
            last_decay: now,
        }
    }
    
    pub fn record_access(&mut self) {
        self.access_count += 1;
        self.last_access = Instant::now();
        self.apply_decay();
    }
    
    fn apply_decay(&mut self) {
        let elapsed = self.last_decay.elapsed().as_secs();
        if elapsed > 60 {
            let decay_factor = 0.9f64.powi(elapsed as i32 / 60);
            self.access_count = (self.access_count as f64 * decay_factor) as u64;
            self.last_decay = Instant::now();
        }
    }
    
    pub fn age_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }
    
    pub fn is_hot(&self, threshold: u64) -> bool {
        self.access_count >= threshold
    }
}

/// Hot key cache configuration
#[derive(Debug, Clone)]
pub struct HotCacheConfig {
    pub max_entries: usize,
    pub min_access_threshold: u64,
    pub ttl_seconds: u64,
    pub negative_ttl_seconds: u64,
    pub cleanup_interval_seconds: u64,
}

impl Default for HotCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 10000,
            min_access_threshold: 5,
            ttl_seconds: 300,
            negative_ttl_seconds: 30,
            cleanup_interval_seconds: 60,
        }
    }
}

/// Hot key cache with LRU eviction
#[derive(Debug)]
pub struct HotKeyCache {
    config: HotCacheConfig,
    cache: Arc<Mutex<HashMap<HashId, CacheEntry>>>,
    negative_cache: Arc<Mutex<HashMap<HashId, NegativeEntry>>>,
    lru_list: Arc<Mutex<VecDeque<HashId>>>,
    hits: AtomicU64,
    misses: AtomicU64,
    total_accesses: AtomicU64,
    stores: AtomicU64,
}

impl HotKeyCache {
    pub fn new(config: HotCacheConfig) -> Self {
        Self {
            config,
            cache: Arc::new(Mutex::new(HashMap::new())),
            negative_cache: Arc::new(Mutex::new(HashMap::new())),
            lru_list: Arc::new(Mutex::new(VecDeque::new())),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            total_accesses: AtomicU64::new(0),
            stores: AtomicU64::new(0),
        }
    }
    
    /// Get value from cache
    pub async fn get(&self, key: &HashId) -> Option<Vec<u8>> {
        self.total_accesses.fetch_add(1, Ordering::Relaxed);
        
        let mut cache = self.cache.lock().await;
        
        if let Some(entry) = cache.get_mut(key) {
            entry.record_access();
            self.hits.fetch_add(1, Ordering::Relaxed);
            
            let mut lru = self.lru_list.lock().await;
            if let Some(pos) = lru.iter().position(|k| k == key) {
                lru.remove(pos);
            }
            lru.push_back(key.clone());
            
            Some(entry.value.clone())
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
    
    /// Store value in cache
    pub async fn store(&self, key: HashId, value: Vec<u8>, access_count: u64) {
        if value.is_empty() {
            return;
        }
        
        // Remove from negative cache
        let mut negative = self.negative_cache.lock().await;
        negative.remove(&key);
        drop(negative);
        
        self.stores.fetch_add(1, Ordering::Relaxed);
        
        let mut cache = self.cache.lock().await;
        let mut lru = self.lru_list.lock().await;
        
        if let Some(entry) = cache.get_mut(&key) {
            entry.value = value;
            entry.access_count = access_count;
            entry.last_access = Instant::now();
            if let Some(pos) = lru.iter().position(|k| k == &key) {
                lru.remove(pos);
            }
            lru.push_back(key);
            return;
        }
        
        if access_count >= self.config.min_access_threshold {
            while cache.len() >= self.config.max_entries {
                if let Some(oldest) = lru.pop_front() {
                    cache.remove(&oldest);
                } else {
                    break;
                }
            }
            cache.insert(key.clone(), CacheEntry::new(value));
            lru.push_back(key);
        }
    }
    
    /// Update cache with result from successful lookup
    pub async fn cache_value(&self, key: HashId, value: Vec<u8>, current_hits: u64) {
        self.store(key, value, current_hits + 1).await;
    }
    
    /// Record a miss in negative cache
    pub async fn record_miss(&self, key: HashId) {
        let mut negative = self.negative_cache.lock().await;
        if let Some(entry) = negative.get_mut(&key) {
            entry.record_miss();
        } else {
            negative.insert(key, NegativeEntry::new());
        }
    }
    
    /// Check if key is in negative cache
    pub async fn is_negative(&self, key: &HashId) -> bool {
        let negative = self.negative_cache.lock().await;
        if let Some(entry) = negative.get(key) {
            !entry.is_expired(self.config.negative_ttl_seconds)
        } else {
            false
        }
    }
    
    /// Cleanup expired entries
    pub async fn cleanup(&self) -> usize {
        let mut cache = self.cache.lock().await;
        let mut lru = self.lru_list.lock().await;
        let before = cache.len();
        
        let to_remove: Vec<HashId> = cache
            .iter()
            .filter(|(_, entry)| entry.age_secs() >= self.config.ttl_seconds)
            .map(|(k, _)| k.clone())
            .collect();
        
        for key in to_remove {
            cache.remove(&key);
            if let Some(pos) = lru.iter().position(|k| k == &key) {
                lru.remove(pos);
            }
        }
        
        before - cache.len()
    }
    
    /// Cleanup expired negative cache entries
    pub async fn cleanup_negative(&self) -> usize {
        let mut negative = self.negative_cache.lock().await;
        let before = negative.len();
        negative.retain(|_, entry| !entry.is_expired(self.config.negative_ttl_seconds));
        before - negative.len()
    }
    
    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.lock().await;
        let total = self.total_accesses.load(Ordering::Relaxed);
        let hits = self.hits.load(Ordering::Relaxed);
        
        CacheStats {
            entries: cache.len(),
            hits,
            misses: self.misses.load(Ordering::Relaxed),
            total_accesses: total,
            stores: self.stores.load(Ordering::Relaxed),
            hit_rate: if total > 0 { hits as f64 / total as f64 * 100.0 } else { 0.0 },
        }
    }
}

/// Cache statistics
#[derive(Debug)]
pub struct CacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub total_accesses: u64,
    pub stores: u64,
    pub hit_rate: f64,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "📊 Hot Key Cache Stats")?;
        writeln!(f, "  Entries: {}", self.entries)?;
        writeln!(f, "  Hits: {}", self.hits)?;
        writeln!(f, "  Misses: {}", self.misses)?;
        writeln!(f, "  Stores: {}", self.stores)?;
        writeln!(f, "  Total accesses: {}", self.total_accesses)?;
        writeln!(f, "  Hit rate: {:.1}%", self.hit_rate)?;
        Ok(())
    }
}

/// Negative cache entry (for misses)
#[derive(Debug, Clone)]
pub struct NegativeEntry {
    pub miss_count: u64,
    pub last_miss: std::time::Instant,
}

impl NegativeEntry {
    pub fn new() -> Self {
        Self {
            miss_count: 1,
            last_miss: std::time::Instant::now(),
        }
    }
    
    pub fn record_miss(&mut self) {
        self.miss_count += 1;
        self.last_miss = std::time::Instant::now();
    }
    
    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        self.last_miss.elapsed().as_secs() >= ttl_secs
    }
}
