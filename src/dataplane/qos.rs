// src/dataplane/qos.rs
//! Quality of Service
//! ==================
//!
//! Packet prioritization and traffic management

use std::collections::VecDeque;

/// Packet priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PacketPriority {
    Critical = 0,  // Highest priority (control messages, key exchange)
    High = 1,       // Real-time data (VoIP, video)
    Normal = 2,     // Regular data
    Low = 3,        // Bulk transfer
    Background = 4, // Lowest priority
}

/// QoS managed packet
#[derive(Debug, Clone)]
pub struct QoSPacket {
    pub priority: PacketPriority,
    pub data: Vec<u8>,
    pub timestamp: u64,
}

/// QoS Manager for packet prioritization
pub struct QoSManager {
    queues: [VecDeque<QoSPacket>; 5],
    max_queue_size: usize,
    total_packets: u64,
    dropped_packets: u64,
}

impl QoSManager {
    pub fn new(max_queue_size: usize) -> Self {
        Self {
            queues: [
                VecDeque::with_capacity(max_queue_size),
                VecDeque::with_capacity(max_queue_size),
                VecDeque::with_capacity(max_queue_size),
                VecDeque::with_capacity(max_queue_size),
                VecDeque::with_capacity(max_queue_size),
            ],
            max_queue_size,
            total_packets: 0,
            dropped_packets: 0,
        }
    }

    /// Add packet to queue
    pub fn enqueue(&mut self, packet: QoSPacket) -> Result<(), ()> {
        let priority = packet.priority as usize;

        if self.queues[priority].len() >= self.max_queue_size {
            self.dropped_packets += 1;
            return Err(()); // Queue full
        }

        self.queues[priority].push_back(packet);
        self.total_packets += 1;
        Ok(())
    }

    /// Get next packet (priority-based)
    pub fn dequeue(&mut self) -> Option<QoSPacket> {
        for queue in &mut self.queues {
            if let Some(packet) = queue.pop_front() {
                return Some(packet);
            }
        }
        None
    }

    /// Get queue statistics
    pub fn queue_stats(&self) -> [usize; 5] {
        [
            self.queues[0].len(),
            self.queues[1].len(),
            self.queues[2].len(),
            self.queues[3].len(),
            self.queues[4].len(),
        ]
    }

    /// Get total packets
    pub fn total_packets(&self) -> u64 {
        self.total_packets
    }

    /// Get dropped packets
    pub fn dropped_packets(&self) -> u64 {
        self.dropped_packets
    }

    /// Clear all queues
    pub fn clear(&mut self) {
        for queue in &mut self.queues {
            queue.clear();
        }
        self.total_packets = 0;
    }
}
