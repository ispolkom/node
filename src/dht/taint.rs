// src/dht/taint.rs
//! Taint Tracking System (Stage 3.2)
//! =================================
//!
//! Tracks "tainted" (untrusted) records from suspicious sources

use std::collections::HashSet;
use serde::{Serialize, Deserialize};

/// Taint flags for DHT records
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaintFlag {
    /// Record from unverified source (no signature)
    UnverifiedSource,

    /// Record from blocked origin
    BlockedOrigin,

    /// Record from SUS peer
    SusPeer,

    /// Record with suspicious pattern (e.g., rapidly changing)
    SuspiciousPattern,

    /// Record from unknown/new peer
    NewPeer,

    /// Record failed validation
    ValidationFailed,
}

impl TaintFlag {
    /// Get taint severity (0-100)
    pub fn severity(&self) -> u8 {
        match self {
            TaintFlag::NewPeer => 20,
            TaintFlag::UnverifiedSource => 40,
            TaintFlag::SuspiciousPattern => 60,
            TaintFlag::ValidationFailed => 80,
            TaintFlag::SusPeer => 90,
            TaintFlag::BlockedOrigin => 100,
        }
    }
}

/// Taint tracker for DHT records
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaintTracker {
    /// Set of active taint flags
    flags: HashSet<TaintFlag>,
    /// Timestamp when taint was applied
    tainted_at: u64,
    /// Origin of taint (for debugging)
    origin: String,
}

impl TaintTracker {
    /// Create new taint tracker
    pub fn new(origin: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            flags: HashSet::new(),
            tainted_at: now,
            origin,
        }
    }

    /// Add taint flag
    pub fn taint(&mut self, flag: TaintFlag) {
        self.flags.insert(flag);
    }

    /// Check if has specific taint flag
    pub fn has_taint(&self, flag: TaintFlag) -> bool {
        self.flags.contains(&flag)
    }

    /// Check if is tainted (has any flags)
    pub fn is_tainted(&self) -> bool {
        !self.flags.is_empty()
    }

    /// Get overall taint severity (0-100)
    pub fn severity(&self) -> u8 {
        self.flags.iter()
            .map(|f| f.severity())
            .max()
            .unwrap_or(0)
    }

    /// Check if record is trusted (severity < threshold)
    pub fn is_trusted(&self, threshold: u8) -> bool {
        self.severity() < threshold
    }

    /// Get age of taint (in seconds)
    pub fn age(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.tainted_at)
    }

    /// Clear all taint flags
    pub fn clear(&mut self) {
        self.flags.clear();
    }

    /// Get all taint flags
    pub fn flags(&self) -> &HashSet<TaintFlag> {
        &self.flags
    }
}

/// Taint tracking configuration
#[derive(Debug, Clone)]
pub struct TaintConfig {
    /// Maximum age for taint (seconds)
    pub max_taint_age: u64,
    /// Severity threshold for blocking
    pub block_threshold: u8,
    /// Severity threshold for warnings
    pub warn_threshold: u8,
}

impl Default for TaintConfig {
    fn default() -> Self {
        Self {
            max_taint_age: 24 * 60 * 60,  // 24 hours
            block_threshold: 80,           // Block at 80+ severity
            warn_threshold: 50,            // Warn at 50+ severity
        }
    }
}

/// Taint tracking manager for DHT
#[derive(Debug)]
pub struct TaintManager {
    /// Configuration
    config: TaintConfig,
    /// Statistics
    stats: TaintStats,
}

#[derive(Debug, Clone, Default)]
pub struct TaintStats {
    pub total_tainted: usize,
    pub total_blocked: usize,
    pub total_warnings: usize,
    pub by_flag: std::collections::HashMap<TaintFlag, usize>,
}

impl Default for TaintManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaintManager {
    pub fn new() -> Self {
        Self {
            config: TaintConfig::default(),
            stats: TaintStats::default(),
        }
    }

    /// Evaluate taint tracker and decide action
    pub fn evaluate(&mut self, tracker: &TaintTracker) -> TaintAction {
        let severity = tracker.severity();

        if severity >= self.config.block_threshold {
            self.stats.total_blocked += 1;
            return TaintAction::Block(format!(
                "Record blocked: taint severity {} (threshold: {})",
                severity, self.config.block_threshold
            ));
        }

        if severity >= self.config.warn_threshold {
            self.stats.total_warnings += 1;
            return TaintAction::Warn(format!(
                "Record warning: taint severity {} (threshold: {})",
                severity, self.config.warn_threshold
            ));
        }

        if tracker.is_tainted() {
            return TaintAction::AcceptWithTaint;
        }

        TaintAction::Accept
    }

    /// Get statistics
    pub fn stats(&self) -> &TaintStats {
        &self.stats
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = TaintStats::default();
    }
}

/// Action to take for tainted record
#[derive(Debug, Clone, PartialEq)]
pub enum TaintAction {
    /// Accept record (clean)
    Accept,
    /// Accept but mark as tainted
    AcceptWithTaint,
    /// Warn about record
    Warn(String),
    /// Block record
    Block(String),
}

/// Helper to create taint tracker for unverified sources
pub fn taint_unverified(origin: String) -> TaintTracker {
    let mut tracker = TaintTracker::new(origin);
    tracker.taint(TaintFlag::UnverifiedSource);
    tracker
}

/// Helper to create taint tracker for SUS peers
pub fn taint_sus_peer(origin: String) -> TaintTracker {
    let mut tracker = TaintTracker::new(origin);
    tracker.taint(TaintFlag::SusPeer);
    tracker
}

/// Helper to create taint tracker for blocked origins
pub fn taint_blocked(origin: String) -> TaintTracker {
    let mut tracker = TaintTracker::new(origin);
    tracker.taint(TaintFlag::BlockedOrigin);
    tracker
}

/// Helper to create taint tracker for new peers
pub fn taint_new_peer(origin: String) -> TaintTracker {
    let mut tracker = TaintTracker::new(origin);
    tracker.taint(TaintFlag::NewPeer);
    tracker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_taint_severity() {
        let mut tracker = TaintTracker::new("test".to_string());
        assert_eq!(tracker.severity(), 0);

        tracker.taint(TaintFlag::NewPeer);
        assert_eq!(tracker.severity(), 20);

        tracker.taint(TaintFlag::SusPeer);
        assert_eq!(tracker.severity(), 90);  // Max of flags
    }

    #[test]
    fn test_taint_evaluation() {
        let mut manager = TaintManager::new();

        let mut tracker = TaintTracker::new("test".to_string());
        assert!(matches!(manager.evaluate(&tracker), TaintAction::Accept));

        tracker.taint(TaintFlag::NewPeer);
        assert!(matches!(manager.evaluate(&tracker), TaintAction::AcceptWithTaint));

        tracker.taint(TaintFlag::SusPeer);
        assert!(matches!(manager.evaluate(&tracker), TaintAction::Block(_)));
    }
}
