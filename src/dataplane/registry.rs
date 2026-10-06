// src/dataplane/registry.rs
//! Stream Registry
//! ===============
//!
//! Manages multiple reliable streams over P2P transport

use crate::dataplane::stream::ReliableStream;
use crate::util::HashId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Stream limits (DoS protection)
pub struct StreamLimits {
    /// Max streams per peer
    pub max_streams_per_peer: usize,

    /// Max total streams
    pub max_total_streams: usize,

    /// Max in-flight frames per stream
    pub max_inflight_per_stream: usize,

    /// Max buffered bytes per peer
    pub max_buffered_bytes_per_peer: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            max_streams_per_peer: 256,  // Increased - browsers open many connections
            max_total_streams: 1024,
            max_inflight_per_stream: 128,
            max_buffered_bytes_per_peer: 16 * 1024 * 1024, // 16 MB
        }
    }
}

/// Stream registry - manages all active streams
pub struct StreamRegistry {
    /// Next stream ID (local)
    next_stream_id: u32,

    /// Active streams: stream_id -> stream
    streams: HashMap<u32, ReliableStream>,

    /// Peer streams: peer_id -> Vec<stream_id>
    peer_streams: HashMap<HashId, Vec<u32>>,

    /// Per-peer buffered bytes counter
    peer_buffered_bytes: HashMap<HashId, usize>,

    /// Stream limits
    limits: StreamLimits,
}

impl StreamRegistry {
    /// Create new registry
    pub fn new() -> Self {
        Self {
            next_stream_id: 1,
            streams: HashMap::new(),
            peer_streams: HashMap::new(),
            peer_buffered_bytes: HashMap::new(),
            limits: StreamLimits::default(),
        }
    }

    /// Create new registry with custom limits
    pub fn with_limits(limits: StreamLimits) -> Self {
        Self {
            next_stream_id: 1,
            streams: HashMap::new(),
            peer_streams: HashMap::new(),
            peer_buffered_bytes: HashMap::new(),
            limits,
        }
    }

    /// Generate new stream ID
    pub fn next_stream_id(&mut self) -> u32 {
        let id = self.next_stream_id;
        self.next_stream_id = id.wrapping_add(1);
        id
    }

    /// Create new stream
    pub fn create_stream(&mut self, peer_id: HashId) -> Result<u32, String> {
        // Check total stream limit
        if self.streams.len() >= self.limits.max_total_streams {
            return Err("Max total streams reached".to_string());
        }

        // Check per-peer stream limit
        let peer_count = self.peer_streams.entry(peer_id).or_default().len();
        if peer_count >= self.limits.max_streams_per_peer {
            return Err(format!("Max streams per peer reached: {}", peer_count));
        }

        let stream_id = self.next_stream_id();

        let stream = ReliableStream::new(stream_id, peer_id);
        self.streams.insert(stream_id, stream);
        self.peer_streams.entry(peer_id).or_default().push(stream_id);

        println!("[stream-registry] ✅ Created stream {} for peer {}",
                 stream_id, hex::encode(&peer_id.0[..8]));

        Ok(stream_id)
    }

    /// Create new stream with specific ID (for incoming SYN)
    pub fn create_stream_with_id(&mut self, peer_id: HashId, stream_id: u32) -> Result<u32, String> {
        // Check if stream already exists
        if self.streams.contains_key(&stream_id) {
            return Err(format!("Stream {} already exists", stream_id));
        }

        // Check total stream limit
        if self.streams.len() >= self.limits.max_total_streams {
            return Err("Max total streams reached".to_string());
        }

        // Check per-peer stream limit
        let peer_count = self.peer_streams.entry(peer_id).or_default().len();
        if peer_count >= self.limits.max_streams_per_peer {
            return Err(format!("Max streams per peer reached: {}", peer_count));
        }

        let stream = ReliableStream::new(stream_id, peer_id);
        self.streams.insert(stream_id, stream);
        self.peer_streams.entry(peer_id).or_default().push(stream_id);

        println!("[stream-registry] ✅ Created stream {} for peer {} (incoming SYN)",
                 stream_id, hex::encode(&peer_id.0[..8]));

        Ok(stream_id)
    }

    /// Get stream by ID
    pub fn get_stream(&self, stream_id: u32) -> Option<&ReliableStream> {
        self.streams.get(&stream_id)
    }

    /// Get mutable stream by ID
    pub fn get_stream_mut(&mut self, stream_id: u32) -> Option<&mut ReliableStream> {
        self.streams.get_mut(&stream_id)
    }

    /// Remove stream
    pub fn remove_stream(&mut self, stream_id: u32) -> Option<ReliableStream> {
        if let Some(stream) = self.streams.remove(&stream_id) {
            // Remove from peer_streams
            if let Some(streams) = self.peer_streams.get_mut(&stream.peer_id) {
                streams.retain(|id| *id != stream_id);
            }

            println!("[stream-registry] Removed stream {} for peer {}",
                     stream_id, hex::encode(&stream.peer_id.0[..8]));

            Some(stream)
        } else {
            None
        }
    }

    /// Get all streams for peer
    pub fn get_peer_streams(&self, peer_id: &HashId) -> Vec<u32> {
        self.peer_streams.get(peer_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Get all active stream IDs
    pub fn all_stream_ids(&self) -> Vec<u32> {
        self.streams.keys().copied().collect()
    }

    /// Get count of active streams
    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// Get count of streams per peer
    pub fn peer_stream_count(&self, peer_id: &HashId) -> usize {
        self.peer_streams.get(peer_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Cleanup closed/expired streams
    pub fn cleanup(&mut self, timeout_secs: u64) -> usize {
        let mut to_remove = Vec::new();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        for (id, stream) in &self.streams {
            if stream.is_closed() || stream.is_expired(timeout) {
                to_remove.push(*id);
            }
        }

        let count = to_remove.len();
        for id in to_remove {
            self.remove_stream(id);
        }

        count
    }

    /// Get statistics
    pub fn stats(&self) -> StreamRegistryStats {
        let total_streams = self.streams.len();
        let mut establishing = 0;
        let mut established = 0;
        let mut closing = 0;

        for stream in self.streams.values() {
            match stream.state {
                crate::dataplane::stream::StreamState::SynSent => establishing += 1,
                crate::dataplane::stream::StreamState::Established => established += 1,
                crate::dataplane::stream::StreamState::FinSent | crate::dataplane::stream::StreamState::Closing => closing += 1,
                _ => {}
            }
        }

        StreamRegistryStats {
            total_streams,
            establishing,
            established,
            closing,
            peers: self.peer_streams.len() as u32,
        }
    }
}

impl Default for StreamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Stream registry statistics
#[derive(Debug, Clone)]
pub struct StreamRegistryStats {
    pub total_streams: usize,
    pub establishing: usize,
    pub established: usize,
    pub closing: usize,
    pub peers: u32,
}

/// Shared stream registry
pub type SharedStreamRegistry = Arc<Mutex<StreamRegistry>>;
