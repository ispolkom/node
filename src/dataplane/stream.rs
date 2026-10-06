// src/dataplane/stream.rs
//! Stream Layer over UDP+AES-GCM
//! ================================
//!
//! Reliable ordered streams over encrypted UDP transport
//! KCP-style protocol with SEQ/ACK/retransmission

use crate::util::HashId;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Stream message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamMsgType {
    Data = 0x10,       // Regular data packet
    Ack = 0x11,         // Acknowledgment
    Syn = 0x12,         // Open stream request
    SynAck = 0x13,      // Stream open confirmation
    Fin = 0x14,         // Graceful close
    Reset = 0x15,       // Error/close immediately
    Keepalive = 0x16,   // Keepalive
}

impl StreamMsgType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x10 => Some(StreamMsgType::Data),
            0x11 => Some(StreamMsgType::Ack),
            0x12 => Some(StreamMsgType::Syn),
            0x13 => Some(StreamMsgType::SynAck),
            0x14 => Some(StreamMsgType::Fin),
            0x15 => Some(StreamMsgType::Reset),
            0x16 => Some(StreamMsgType::Keepalive),
            _ => None,
        }
    }

    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Stream header (fixed 20 bytes)
///
/// [MSG_TYPE:1][STREAM_ID:4][SEQ:4][ACK:4][WIN:2][LEN:2][FLAGS:2]
#[derive(Debug, Clone)]
pub struct StreamHeader {
    pub msg_type: StreamMsgType,
    pub stream_id: u32,
    pub seq: u32,
    pub ack: u32,
    pub win: u16,      // Receive window size
    pub len: u16,      // Data length
    pub flags: u16,    // Flags (for future use)
}

impl StreamHeader {
    pub const SIZE: usize = 20;

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }

        let msg_type = StreamMsgType::from_byte(data[0])?;
        let stream_id = u32::from_be_bytes(data[1..5].try_into().ok()?);
        let seq = u32::from_be_bytes(data[5..9].try_into().ok()?);
        let ack = u32::from_be_bytes(data[9..13].try_into().ok()?);
        let win = u16::from_be_bytes(data[13..15].try_into().ok()?);
        let len = u16::from_be_bytes(data[15..17].try_into().ok()?);
        let flags = u16::from_be_bytes(data[17..19].try_into().ok()?);

        Some(Self {
            msg_type,
            stream_id,
            seq,
            ack,
            win,
            len,
            flags,
        })
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        out[0] = self.msg_type.to_byte();
        out[1..5].copy_from_slice(&self.stream_id.to_be_bytes());
        out[5..9].copy_from_slice(&self.seq.to_be_bytes());
        out[9..13].copy_from_slice(&self.ack.to_be_bytes());
        out[13..15].copy_from_slice(&self.win.to_be_bytes());
        out[15..17].copy_from_slice(&self.len.to_be_bytes());
        out[17..19].copy_from_slice(&self.flags.to_be_bytes());
        out
    }
}

/// Stream frame (header + data)
#[derive(Debug, Clone)]
pub struct StreamFrame {
    pub header: StreamHeader,
    pub data: Vec<u8>,
}

impl StreamFrame {
    /// Maximum data payload per packet
    pub const MAX_DATA_SIZE: usize = 1400; // Safe UDP size

    /// Create new frame
    pub fn new(msg_type: StreamMsgType, stream_id: u32, seq: u32, ack: u32, data: Vec<u8>) -> Self {
        let len = data.len() as u16;
        let header = StreamHeader {
            msg_type,
            stream_id,
            seq,
            ack,
            win: 65535, // Default max window
            len,
            flags: 0,
        };
        Self { header, data }
    }

    /// Create ACK frame
    pub fn ack(stream_id: u32, ack: u32, win: u16) -> Self {
        Self {
            header: StreamHeader {
                msg_type: StreamMsgType::Ack,
                stream_id,
                seq: 0,
                ack,
                win,
                len: 0,
                flags: 0,
            },
            data: Vec::new(),
        }
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let header = StreamHeader::from_bytes(data)?;
        let data_start = StreamHeader::SIZE;
        let data_end = data_start + header.len as usize;

        if data.len() < data_end {
            return None;
        }

        Some(Self {
            header,
            data: data[data_start..data_end].to_vec(),
        })
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.header.to_bytes().to_vec();
        out.extend_from_slice(&self.data);
        out
    }

    /// Total size (header + data)
    pub fn total_size(&self) -> usize {
        StreamHeader::SIZE + self.data.len()
    }
}

/// Send packet with retransmission info
#[derive(Debug, Clone)]
pub struct SendPacket {
    pub frame: StreamFrame,
    pub sent_at: Instant,
    pub resent_count: u32,
}

/// Reliable stream state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamState {
    Closed,
    SynSent,     // SYN sent, waiting SYN-ACK
    Established,
    FinSent,     // FIN sent, waiting
    Closing,
    Reset,
}

/// Reliable ordered stream
pub struct ReliableStream {
    pub stream_id: u32,
    pub peer_id: HashId,
    pub state: StreamState,

    // Send side
    pub send_seq: u32,
    pub send_window: u16,
    pub unacked: VecDeque<SendPacket>,  // Packets waiting for ACK
    pub send_buffer: VecDeque<u8>,       // Data waiting to send

    // Receive side
    pub recv_seq: u32,
    pub recv_window: u16,
    pub recv_buffer: VecDeque<u8>,       // Ordered received data

    // Timing
    pub last_activity: Instant,
    pub rtt_ms: u32,                     // Round-trip time
    pub rto_ms: u32,                     // Retransmission timeout

    // Config
    pub max_retransmit: u32,
    pub keepalive_interval: Duration,
}

impl ReliableStream {
    /// Create new stream
    pub fn new(stream_id: u32, peer_id: HashId) -> Self {
        Self {
            stream_id,
            peer_id,
            state: StreamState::Closed,

            send_seq: 0,
            send_window: 65535,
            unacked: VecDeque::with_capacity(256),
            send_buffer: VecDeque::new(),

            recv_seq: 0,
            recv_window: 65535,
            recv_buffer: VecDeque::new(),

            last_activity: Instant::now(),
            rtt_ms: 200,      // Initial RTT estimate
            rto_ms: 1000,     // Initial RTO

            max_retransmit: 10,  // Increased from 5 - more retries for unreliable UDP
            keepalive_interval: Duration::from_secs(10),
        }
    }

    /// Check if stream is writable
    pub fn can_write(&self) -> bool {
        self.state == StreamState::Established &&
        self.unacked.len() < self.send_window as usize &&
        self.send_buffer.len() < 65536 // Max buffer
    }

    /// Write data to stream
    pub fn write(&mut self, data: &[u8]) -> Result<usize, String> {
        if !self.can_write() {
            return Err("Stream not writable".to_string());
        }

        let to_write = data.len().min(65536 - self.send_buffer.len());
        self.send_buffer.extend(&data[..to_write]);
        self.last_activity = Instant::now();

        Ok(to_write)
    }

    /// Read data from stream
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.recv_buffer.len());
        for (i, byte) in self.recv_buffer.drain(..to_read).enumerate() {
            buf[i] = byte;
        }
        self.last_activity = Instant::now();
        to_read
    }

    /// Get available data to read
    pub fn available(&self) -> usize {
        self.recv_buffer.len()
    }

    /// Check if stream is closed
    pub fn is_closed(&self) -> bool {
        matches!(self.state, StreamState::Closed | StreamState::Reset)
    }

    /// Initiate connection (send SYN)
    pub fn connect(&mut self) -> StreamFrame {
        eprintln!("[stream-{}] 🔵 connect() called, changing state from {:?} to SynSent",
                 self.stream_id, self.state);
        self.state = StreamState::SynSent;
        self.last_activity = Instant::now();

        let frame = StreamFrame::new(
            StreamMsgType::Syn,
            self.stream_id,
            self.send_seq,
            0,
            vec![]
        );

        self.send_seq = self.send_seq.wrapping_add(1);
        self.unacked.push_back(SendPacket {
            frame: frame.clone(),
            sent_at: Instant::now(),
            resent_count: 0,
        });

        frame
    }

    /// Accept connection (send SYN-ACK)
    pub fn accept(&mut self) -> StreamFrame {
        eprintln!("[stream-{}] 🟢 accept() called, changing state from {:?} to Established",
                 self.stream_id, self.state);
        self.state = StreamState::Established;
        self.last_activity = Instant::now();

        StreamFrame::new(
            StreamMsgType::SynAck,
            self.stream_id,
            0,
            self.recv_seq,
            vec![]
        )
    }

    /// Handle incoming frame
    pub fn handle_frame(&mut self, frame: &StreamFrame) -> Option<StreamFrame> {
        self.last_activity = Instant::now();

        match frame.header.msg_type {
            StreamMsgType::Syn => {
                // Incoming connection request
                if self.state == StreamState::Closed {
                    Some(self.accept())
                } else {
                    None
                }
            }
            StreamMsgType::SynAck => {
                // Connection confirmed
                if self.state == StreamState::SynSent {
                    eprintln!("[stream-{}] 🟢 Received SYN-ACK, changing state from SynSent to Established",
                             self.stream_id);
                    self.state = StreamState::Established;

                    // Remove SYN from unacked - it's been acknowledged!
                    self.unacked.clear();
                    eprintln!("[stream-{}] ✅ Cleared unacked packets after SYN-ACK", self.stream_id);
                }
                None
            }
            StreamMsgType::Data => {
                // Data packet
                if frame.header.seq == self.recv_seq {
                    // In-order packet
                    eprintln!("[stream-{}] ✅ Received IN-ORDER Data: seq={}, len={}",
                             self.stream_id, frame.header.seq, frame.data.len());
                    self.recv_seq = self.recv_seq.wrapping_add(1);
                    self.recv_buffer.extend(&frame.data);
                    eprintln!("[stream-{}] 📥 recv_buffer now has {} bytes",
                             self.stream_id, self.recv_buffer.len());

                    // Update RTT
                    let now = Instant::now();
                    if let Some(sent) = self.unacked.front() {
                        let rtt = now.duration_since(sent.sent_at).as_millis() as u32;
                        self.rtt_ms = (self.rtt_ms * 3 + rtt) / 4; // EMA
                        self.rto_ms = (self.rtt_ms * 2).max(200);
                    }

                    // Send ACK
                    Some(StreamFrame::ack(self.stream_id, self.recv_seq, self.recv_window))
                } else if frame.header.seq == self.recv_seq.wrapping_add(1) {
                    // Next expected sequence number (for incoming streams)
                    self.recv_seq = self.recv_seq.wrapping_add(1);
                    self.recv_buffer.extend(&frame.data);

                    // Update RTT
                    let now = Instant::now();
                    if let Some(sent) = self.unacked.front() {
                        let rtt = now.duration_since(sent.sent_at).as_millis() as u32;
                        self.rtt_ms = (self.rtt_ms * 3 + rtt) / 4; // EMA
                        self.rto_ms = (self.rtt_ms * 2).max(200);
                    }

                    // Send ACK
                    Some(StreamFrame::ack(self.stream_id, self.recv_seq, self.recv_window))
                } else {
                    // Out of order - just ACK what we expect
                    Some(StreamFrame::ack(self.stream_id, self.recv_seq, self.recv_window))
                }
            }
            StreamMsgType::Ack => {
                // Remove acked packets
                let ack = frame.header.ack;
                while let Some(pkt) = self.unacked.front() {
                    if pkt.frame.header.seq < ack {
                        self.unacked.pop_front();
                    } else {
                        break;
                    }
                }
                None
            }
            StreamMsgType::Fin => {
                // Graceful close
                self.state = StreamState::Closing;
                Some(StreamFrame::new(
                    StreamMsgType::Fin,
                    self.stream_id,
                    0,
                    0,
                    vec![]
                ))
            }
            StreamMsgType::Reset => {
                // Immediate close
                self.state = StreamState::Reset;
                None
            }
            StreamMsgType::Keepalive => {
                None
            }
        }
    }

    /// Get packets to send (with retransmission)
    pub fn get_packets_to_send(&mut self, now: Instant, max_inflight: usize) -> Vec<StreamFrame> {
        let mut to_send = Vec::new();

        // DEBUG: Log state
        if !self.send_buffer.is_empty() {
            eprintln!("[stream-{}] get_packets_to_send: state={:?}, send_buffer={}, unacked={}, max_inflight={}",
                     self.stream_id, self.state, self.send_buffer.len(), self.unacked.len(), max_inflight);
        }

        // STRIKT SEQUENCING: Send only ONE packet at a time, wait for ACK before next
        if self.unacked.len() < max_inflight && self.unacked.is_empty() {
            // Only send if NO unacked packets - strict synchronization!
            if self.state == StreamState::Established && !self.send_buffer.is_empty() {
                let chunk_size = StreamFrame::MAX_DATA_SIZE;

                // Send ONLY one chunk
                let take = self.send_buffer.len().min(chunk_size);
                let data: Vec<_> = self.send_buffer.drain(..take).collect();

                let frame = StreamFrame::new(
                    StreamMsgType::Data,
                    self.stream_id,
                    self.send_seq,
                    self.recv_seq,
                    data,
                );

                self.send_seq = self.send_seq.wrapping_add(1);
                self.unacked.push_back(SendPacket {
                    frame: frame.clone(),
                    sent_at: now,
                    resent_count: 0,
                });

                to_send.push(frame);
                eprintln!("[stream-{}] 📤 Sent ONE packet, waiting for ACK...", self.stream_id);
            }
        }

        // Retransmit timed-out packets (always allow)
        for pkt in &mut self.unacked {
            if now.duration_since(pkt.sent_at) > Duration::from_millis(self.rto_ms as u64) {
                pkt.sent_at = now;
                pkt.resent_count += 1;
                to_send.push(pkt.frame.clone());

                if pkt.resent_count > self.max_retransmit {
                    self.state = StreamState::Reset;
                    break;
                }
            }
        }

        to_send
    }

    /// Close stream gracefully
    pub fn close(&mut self) -> Option<StreamFrame> {
        eprintln!("[stream-{}] ⚠️ close() called, current state={:?}, send_buffer={}",
                 self.stream_id, self.state, self.send_buffer.len());

        if self.state == StreamState::Established {
            // Don't close if there's data in send_buffer that hasn't been sent yet!
            if !self.send_buffer.is_empty() {
                eprintln!("[stream-{}] ⚠️ BLOCKING close(): send_buffer has {} bytes, must send first!",
                         self.stream_id, self.send_buffer.len());
                return None;  // Don't close yet, let data send first
            }

            self.state = StreamState::FinSent;
            eprintln!("[stream-{}] ⚠️ State changed to FinSent", self.stream_id);
            Some(StreamFrame::new(
                StreamMsgType::Fin,
                self.stream_id,
                0,
                0,
                vec![]
            ))
        } else {
            eprintln!("[stream-{}] ⚠️ close() called but state is not Established, returning None", self.stream_id);
            None
        }
    }

    /// Reset stream immediately
    pub fn reset(&mut self) -> StreamFrame {
        self.state = StreamState::Reset;
        StreamFrame::new(
            StreamMsgType::Reset,
            self.stream_id,
            0,
            0,
            vec![]
        )
    }

    /// Check for timeout
    pub fn is_expired(&self, timeout: Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }
}
