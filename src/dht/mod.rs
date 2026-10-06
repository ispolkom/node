// src/dht/mod.rs
//! DHT Module - Kademlia Distributed Hash Table
//! ===============================================
//!
//! Implementation of Kademlia DHT for P2P key-value storage and node discovery.

pub mod bucket;
pub mod storage;
pub mod messages;
pub mod kademlia;
pub mod record;
pub mod taint;
pub mod jurisdiction_index;

// Re-exports
pub use bucket::{KBucket, KTable, BucketPeer, BucketStats, TableStats, xor_distance, K_BUCKET_SIZE};
pub use storage::{DhtStorage, DhtRecord, StorageStats, RequestType, PeerEndpoint};
pub use messages::{DhtQuery, DhtResponse, DhtQueryType};
pub use kademlia::Kademlia;
pub use record::{NodeRecord, NODE_RECORD_TTL_SOFT, NODE_RECORD_TTL_HARD};
pub mod hot_cache;
pub mod group_record;

pub use jurisdiction_index::{JurisdictionIndex, JurisdictionEntry};
