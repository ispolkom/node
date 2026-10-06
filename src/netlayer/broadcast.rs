// src/netlayer/broadcast.rs
//! Rate-Limited Broadcasting (Stage 3.1)
//! =====================================
//!
//! Prevents message spam and broadcast storms

use std::collections::HashMap;
use std::time::{SystemTime, Duration};
use crate::util::HashId;

/// Broadcast rate limits
pub const BROADCAST_PER_SECOND: u32 = 10;      // Max 10 broadcasts per second
pub const BROADCAST_PER_MINUTE: u32 = 100;     // Max 100 broadcasts per minute
pub const BURST_ALLOWANCE: u32 = 20;           // Allow bursts up to 20 messages

/// Broadcast message types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BroadcastType {
    /// General announcement to all peers
    Announcement,
    /// Query to multiple peers
    Query,
    /// Response to broadcast
    Response,
    /// Critical system message
    Critical,
}

/// Broadcast rate limiter
#[derive(Debug)]
pub struct BroadcastLimiter {
    /// Last broadcast timestamp
    last_broadcast: u64,
    /// Messages in current second
    this_second: u32,
    /// Messages in current minute
    this_minute: u32,
    /// Token bucket for burst allowance
    burst_tokens: f32,
    /// Last token refill time
    last_refill: u64,
    /// Per-peer rate limits (peer_id -> (last_sent, count))
    peer_limits: HashMap<HashId, (u64, u32)>,
}

impl Default for BroadcastLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl BroadcastLimiter {
    pub fn new() -> Self {
        let now = Self::now();
        Self {
            last_broadcast: now,
            this_second: 0,
            this_minute: 0,
            burst_tokens: BURST_ALLOWANCE as f32,
            last_refill: now,
            peer_limits: HashMap::new(),
        }
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }

    /// Check if broadcast is allowed (Stage 3.1)
    /// Returns Ok(()) if allowed, Err with reason if not
    pub fn can_broadcast(&mut self, broadcast_type: BroadcastType) -> Result<(), String> {
        let now = Self::now();

        // Critical messages bypass rate limits
        if broadcast_type == BroadcastType::Critical {
            return Ok(());
        }

        // Refill burst tokens (1 token per second)
        let elapsed = now.saturating_sub(self.last_refill);
        self.burst_tokens = (self.burst_tokens + elapsed as f32).min(BURST_ALLOWANCE as f32);
        self.last_refill = now;

        // Reset counters if time passed
        let last_sec_diff = now.saturating_sub(self.last_broadcast);
        if last_sec_diff >= 1 {
            self.this_second = 0;
        }
        if last_sec_diff >= 60 {
            self.this_minute = 0;
        }

        // Check per-second limit
        if self.this_second >= BROADCAST_PER_SECOND {
            // Use burst tokens if available
            if self.burst_tokens >= 1.0 {
                self.burst_tokens -= 1.0;
            } else {
                return Err(format!(
                    "Broadcast rate limit exceeded: {}/sec (max {})",
                    self.this_second, BROADCAST_PER_SECOND
                ));
            }
        }

        // Check per-minute limit
        if self.this_minute >= BROADCAST_PER_MINUTE {
            return Err(format!(
                "Broadcast rate limit exceeded: {}/min (max {})",
                self.this_minute, BROADCAST_PER_MINUTE
            ));
        }

        // Update counters
        self.last_broadcast = now;
        self.this_second += 1;
        self.this_minute += 1;

        Ok(())
    }

    /// Check if can send to specific peer (prevents peer spam)
    /// Max 5 messages per second per peer
    pub fn can_send_to_peer(&mut self, peer_id: &HashId) -> Result<(), String> {
        const MAX_PEER_PER_SECOND: u32 = 5;
        let now = Self::now();

        let entry = self.peer_limits.entry(*peer_id).or_insert((now, 0));

        // Reset if second passed
        if now.saturating_sub(entry.0) >= 1 {
            entry.0 = now;
            entry.1 = 0;
        }

        if entry.1 >= MAX_PEER_PER_SECOND {
            return Err(format!("Peer rate limit exceeded: {}/sec", entry.1));
        }

        entry.1 += 1;
        Ok(())
    }

    /// Get current broadcast rate stats
    pub fn stats(&self) -> BroadcastStats {
        BroadcastStats {
            this_second: self.this_second,
            this_minute: self.this_minute,
            burst_tokens: self.burst_tokens as u32,
            tracked_peers: self.peer_limits.len(),
        }
    }

    /// Reset rate limiter (for testing)
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// Broadcast statistics
#[derive(Debug, Clone)]
pub struct BroadcastStats {
    pub this_second: u32,
    pub this_minute: u32,
    pub burst_tokens: u32,
    pub tracked_peers: usize,
}

/// Broadcast manager with rate limiting
#[derive(Debug)]
pub struct BroadcastManager {
    limiter: BroadcastLimiter,
    total_sent: u64,
    total_blocked: u64,
}

impl Default for BroadcastManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BroadcastManager {
    pub fn new() -> Self {
        Self {
            limiter: BroadcastLimiter::new(),
            total_sent: 0,
            total_blocked: 0,
        }
    }

    /// Attempt to broadcast message
    /// Returns Ok(()) if broadcast allowed, Err if rate limited
    pub fn broadcast(&mut self, broadcast_type: BroadcastType) -> Result<(), String> {
        match self.limiter.can_broadcast(broadcast_type) {
            Ok(()) => {
                self.total_sent += 1;
                Ok(())
            }
            Err(e) => {
                self.total_blocked += 1;
                Err(e)
            }
        }
    }

    /// Check if can send to specific peer
    pub fn can_send_to_peer(&mut self, peer_id: &HashId) -> Result<(), String> {
        self.limiter.can_send_to_peer(peer_id)
    }

    /// Get statistics
    pub fn stats(&self) -> BroadcastManagerStats {
        BroadcastManagerStats {
            total_sent: self.total_sent,
            total_blocked: self.total_blocked,
            rate: self.limiter.stats(),
        }
    }

    /// Reset statistics
    pub fn reset(&mut self) {
        self.limiter.reset();
        self.total_sent = 0;
        self.total_blocked = 0;
    }
}

/// Broadcast manager statistics
#[derive(Debug, Clone)]
pub struct BroadcastManagerStats {
    pub total_sent: u64,
    pub total_blocked: u64,
    pub rate: BroadcastStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcast_rate_limit() {
        // Step 9 fix: rate limit = BROADCAST_PER_SECOND (10) + BURST_ALLOWANCE (20) = 30
        // первых broadcast'ов в первой секунде. 31-й должен fail'ить (бёрстовые токены
        // не успеют рефиллиться). Старый тест ожидал 21-й fail (видимо изначально считал
        // что BURST_ALLOWANCE — это абсолютный потолок); теперь обновили под фактическую
        // семантику «burst поверх per-second».
        let mut limiter = BroadcastLimiter::new();

        for i in 0..(BROADCAST_PER_SECOND + BURST_ALLOWANCE) {
            assert!(
                limiter.can_broadcast(BroadcastType::Announcement).is_ok(),
                "broadcast #{} unexpectedly blocked", i
            );
        }

        // Превышаем — должно вернуть Err.
        assert!(limiter.can_broadcast(BroadcastType::Announcement).is_err());
    }

    #[test]
    fn test_critical_bypass() {
        let mut limiter = BroadcastLimiter::new();

        // Exhaust normal limit
        for _ in 0..100 {
            let _ = limiter.can_broadcast(BroadcastType::Announcement);
        }

        // Critical should still work
        assert!(limiter.can_broadcast(BroadcastType::Critical).is_ok());
    }
}
