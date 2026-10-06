//! Media session manager

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};
use crate::media::session::stream::{MediaStream, MediaType, StreamState, AudioConfig};

/// Media session manager
pub struct MediaSessionManager {
    streams: Arc<RwLock<HashMap<u64, Arc<MediaStream>>>>,
    next_id: AtomicU64,
    peer_streams: Arc<RwLock<HashMap<String, Vec<u64>>>>, // peer_id -> stream_ids
}

impl MediaSessionManager {
    pub fn new() -> Self {
        Self {
            streams: Arc::new(RwLock::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            peer_streams: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Create new audio stream
    pub fn create_audio_stream(&self, peer_id: String) -> Result<Arc<MediaStream>, String> {
        self.create_stream(MediaType::Audio, peer_id)
    }
    
    /// Create new video stream
    pub fn create_video_stream(&self, peer_id: String) -> Result<Arc<MediaStream>, String> {
        self.create_stream(MediaType::Video, peer_id)
    }
    
    fn create_stream(&self, stream_type: MediaType, peer_id: String) -> Result<Arc<MediaStream>, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let stream = Arc::new(MediaStream::new(id, stream_type, peer_id.clone()));
        
        // Store stream
        {
            let mut streams = self.streams.write().unwrap();
            streams.insert(id, stream.clone());
        }
        
        // Index by peer
        {
            let mut peer_streams = self.peer_streams.write().unwrap();
            peer_streams.entry(peer_id).or_insert_with(Vec::new).push(id);
        }
        
        Ok(stream)
    }
    
    /// Get stream by ID
    pub fn get_stream(&self, id: u64) -> Option<Arc<MediaStream>> {
        self.streams.read().unwrap().get(&id).cloned()
    }
    
    /// Get all streams for a peer
    pub fn get_peer_streams(&self, peer_id: &str) -> Vec<Arc<MediaStream>> {
        let streams = self.streams.read().unwrap();
        let peer_streams = self.peer_streams.read().unwrap();
        
        peer_streams.get(peer_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| streams.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
    
    /// End stream
    pub fn end_stream(&self, id: u64) -> Result<(), String> {
        let stream = self.get_stream(id).ok_or("Stream not found")?;
        stream.set_state(StreamState::Ended);
        
        // Remove from peer index
        let peer_id = stream.remote_peer().to_string();
        let mut peer_streams = self.peer_streams.write().unwrap();
        if let Some(ids) = peer_streams.get_mut(&peer_id) {
            ids.retain(|&x| x != id);
            if ids.is_empty() {
                peer_streams.remove(&peer_id);
            }
        }
        
        // Remove stream
        self.streams.write().unwrap().remove(&id);
        
        Ok(())
    }
    
    /// End all streams for a peer
    pub fn end_peer_streams(&self, peer_id: &str) {
        let streams = self.get_peer_streams(peer_id);
        for stream in streams {
            let _ = self.end_stream(stream.id());
        }
    }
    
    /// Get active stream count
    pub fn active_stream_count(&self) -> usize {
        self.streams.read().unwrap().len()
    }
    
    /// Configure audio stream with defaults
    pub fn configure_audio_stream(&self, stream_id: u64, config: Option<AudioConfig>) -> Result<(), String> {
        let stream = self.get_stream(stream_id).ok_or("Stream not found")?;
        stream.configure_audio(config.unwrap_or_default())
    }
    
    /// Get stream statistics
    pub fn stream_stats(&self, stream_id: u64) -> Option<super::stream::StreamStats> {
        self.get_stream(stream_id).map(|s| s.stats())
    }
}

impl Default for MediaSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MediaSessionManager {
    fn clone(&self) -> Self {
        Self::new()
    }
}
