//! WebRTC signaling for SDP and ICE

pub mod sdp;
pub mod ice;

use serde::{Serialize, Deserialize};

/// Signaling message for WebRTC negotiation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingMessage {
    pub from: String,
    pub to: String,
    pub message_type: SignalingType,
    pub sdp: Option<SdpInfo>,
    pub ice: Option<IceCandidate>,
}

/// Type of signaling message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalingType {
    Offer,
    Answer,
    IceCandidate,
    Hangup,
}

/// SDP information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdpInfo {
    pub sdp: String,
    pub stream_id: String,
}

/// ICE candidate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
}
