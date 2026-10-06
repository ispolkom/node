//! Bridge between media streams and P2P transport

use std::sync::Arc;
use tokio::sync::mpsc;
use crate::media::session::MediaStream;
use crate::media::transport::{MediaPacket, MediaPacketType};

/// Bridge for sending media over P2P
pub struct MediaP2PBridge {
    stream_id: u64,
    peer_id: crate::util::HashId,
    tx: mpsc::UnboundedSender<MediaPacket>,
}

impl MediaP2PBridge {
    pub fn new(stream_id: u64, peer_id: crate::util::HashId) -> (Self, mpsc::UnboundedReceiver<MediaPacket>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { stream_id, peer_id, tx }, rx)
    }
    
    pub async fn send_audio(&self, encoded: Vec<u8>) -> Result<(), String> {
        let packet = MediaPacket {
            stream_id: self.stream_id,
            sequence: 0, // TODO: add sequence counter
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            payload: encoded,
            media_type: MediaPacketType::Audio,
        };
        
        self.tx.send(packet).map_err(|e| format!("Failed to queue packet: {}", e))
    }
}

/// Handle incoming media packets from P2P
pub async fn handle_media_packet(
    packet: MediaPacket,
    stream: Arc<MediaStream>,
) -> Result<Vec<i16>, String> {
    match packet.media_type {
        MediaPacketType::Audio => {
            stream.receive_audio(&packet.payload)
        }
        MediaPacketType::Video => {
            // TODO: video decoding
            Err("Video not implemented".to_string())
        }
        MediaPacketType::Control => {
            // Handle control messages
            Ok(vec![])
        }
    }
}
