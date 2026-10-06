// src/p2p/packet.rs
//! P2P Packet Types for Communication Layer

use crate::util::HashId;

pub const P2P_PACKET_HEADER_LEN: usize = 54;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum P2PPacketType {
    ChatMessage = 0xA0,
    ChatAck = 0xA1,
    ChatRead = 0xA2,
    ChatTyping = 0xA3,
    ChatDeleteMessage = 0xA4,
    VoiceCallRequest = 0xB0,
    VoiceCallAccept = 0xB1,
    VoiceCallEnd = 0xB2,
    VoiceCallReject = 0xB3,
    VoiceData = 0xB4,
    VideoCallRequest = 0xC0,
    VideoCallAccept = 0xC1,
    VideoCallEnd = 0xC2,
    VideoCallReject = 0xC3,
    VideoData = 0xC4,
    FileTransferStart = 0xD0,
    FileChunk = 0xD1,
    FileTransferEnd = 0xD2,
    FileTransferCancel = 0xD3,
    FileMissing = 0xD4,
    FileComplete = 0xD5,
}

impl P2PPacketType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0xA0 => Some(P2PPacketType::ChatMessage),
            0xA1 => Some(P2PPacketType::ChatAck),
            0xA2 => Some(P2PPacketType::ChatRead),
            0xA3 => Some(P2PPacketType::ChatTyping),
            0xA4 => Some(P2PPacketType::ChatDeleteMessage),
            0xB0 => Some(P2PPacketType::VoiceCallRequest),
            0xB1 => Some(P2PPacketType::VoiceCallAccept),
            0xB2 => Some(P2PPacketType::VoiceCallEnd),
            0xB3 => Some(P2PPacketType::VoiceCallReject),
            0xB4 => Some(P2PPacketType::VoiceData),
            0xC0 => Some(P2PPacketType::VideoCallRequest),
            0xC1 => Some(P2PPacketType::VideoCallAccept),
            0xC2 => Some(P2PPacketType::VideoCallEnd),
            0xC3 => Some(P2PPacketType::VideoCallReject),
            0xC4 => Some(P2PPacketType::VideoData),
            0xD0 => Some(P2PPacketType::FileTransferStart),
            0xD1 => Some(P2PPacketType::FileChunk),
            0xD2 => Some(P2PPacketType::FileTransferEnd),
            0xD3 => Some(P2PPacketType::FileTransferCancel),
            0xD4 => Some(P2PPacketType::FileMissing),
            0xD5 => Some(P2PPacketType::FileComplete),
            _ => None,
        }
    }

    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone)]
pub struct P2PPacket {
    pub packet_type: P2PPacketType,
    pub sender: HashId,
    pub encrypted: bool,
    pub is_clone: bool,
    pub line_id: u8,
    pub packet_id: u64,
    pub seq_num: u32,
    pub total_parts: u32,
    pub payload: Vec<u8>,
}

impl P2PPacket {
    pub fn new(packet_type: P2PPacketType, sender: HashId, encrypted: bool, payload: Vec<u8>) -> Self {
        Self::with_clone(packet_type, sender, encrypted, false, payload)
    }

    pub fn with_clone(packet_type: P2PPacketType, sender: HashId, encrypted: bool, is_clone: bool, payload: Vec<u8>) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
        let random: u64 = rand::random();
        let packet_id = timestamp ^ random;

        Self {
            packet_type,
            sender,
            encrypted,
            is_clone,
            line_id: 0,
            packet_id,
            seq_num: 0,
            total_parts: 0,
            payload,
        }
    }

    pub fn with_line_id(mut self, line_id: u8) -> Self {
        self.line_id = line_id;
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let payload_len = u16::try_from(self.payload.len()).expect("Payload too large");
        let mut out = Vec::new();
        let mut flags = 0u8;
        if self.encrypted { flags |= 0b0000_0001; }
        if self.is_clone { flags |= 0b0000_0010; }
        out.push(flags);
        out.push(self.packet_type.to_byte());
        out.extend_from_slice(self.sender.as_ref());
        out.extend_from_slice(&payload_len.to_be_bytes());
        out.push(self.line_id);
        out.extend_from_slice(&self.packet_id.to_be_bytes());
        out.extend_from_slice(&self.seq_num.to_be_bytes());
        out.extend_from_slice(&self.total_parts.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 54 { return None; }
        let flags = data[0];
        let encrypted = (flags & 0b0000_0001) != 0;
        let is_clone = (flags & 0b0000_0010) != 0;
        let packet_type = P2PPacketType::from_byte(data[1])?;
        let mut sender_bytes = [0u8; 32];
        sender_bytes.copy_from_slice(&data[2..34]);
        let sender = HashId(sender_bytes);
        let payload_len = u16::from_be_bytes([data[34], data[35]]) as usize;
        let line_id = data[36];
        let packet_id = u64::from_be_bytes(data[37..45].try_into().ok()?);
        let seq_num = u32::from_be_bytes(data[45..49].try_into().ok()?);
        let total_parts = u32::from_be_bytes(data[49..53].try_into().ok()?);
        if data.len() < 53 + payload_len { return None; }
        let payload = data[53..53 + payload_len].to_vec();
        Some(Self {
            packet_type,
            sender,
            encrypted,
            is_clone,
            line_id,
            packet_id,
            seq_num,
            total_parts,
            payload,
        })
    }
}
