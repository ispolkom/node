//! Media transport over P2P network

use crate::media::session::MediaStream;
use crate::media::codecs::OpusEncoder;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Media packet for transmission
#[derive(Debug, Clone)]
pub struct MediaPacket {
    pub stream_id: u64,
    pub sequence: u32,
    pub timestamp: u64,
    pub payload: Vec<u8>,
    pub media_type: MediaPacketType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaPacketType {
    Audio,
    Video,
    Control,
}

/// Media transport sender
pub struct MediaSender {
    stream_id: u64,
    sequence: u32,
    tx: mpsc::UnboundedSender<MediaPacket>,
}

impl MediaSender {
    pub fn new(stream_id: u64, tx: mpsc::UnboundedSender<MediaPacket>) -> Self {
        Self {
            stream_id,
            sequence: 0,
            tx,
        }
    }
    
    pub async fn send_audio(&mut self, encoded: Vec<u8>) -> Result<(), String> {
        let packet = MediaPacket {
            stream_id: self.stream_id,
            sequence: self.sequence,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            payload: encoded,
            media_type: MediaPacketType::Audio,
        };
        
        self.sequence = self.sequence.wrapping_add(1);
        self.tx.send(packet).map_err(|e| format!("Failed to send: {}", e))?;
        Ok(())
    }
}

/// Media transport receiver
pub struct MediaReceiver {
    stream_id: u64,
    rx: mpsc::UnboundedReceiver<MediaPacket>,
}

impl MediaReceiver {
    pub fn new(stream_id: u64, rx: mpsc::UnboundedReceiver<MediaPacket>) -> Self {
        Self { stream_id, rx }
    }
    
    pub async fn receive(&mut self) -> Option<MediaPacket> {
        self.rx.recv().await
    }
}

/// Create media transport pair
pub fn media_transport_channel() -> (MediaSender, MediaReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    let sender = MediaSender::new(0, tx);
    let receiver = MediaReceiver::new(0, rx);
    (sender, receiver)
}
