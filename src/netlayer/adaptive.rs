// src/netlayer/adaptive.rs
//! Adaptive Transport Controller
//! =============================
//!
//! Analyses network metrics and switches between transport modes:
//! - Performance: minimal overhead
//! - Balanced: moderate protection  
//! - Stealth: maximum DPI resistance

use crate::util::HashId;
use std::time::{Duration, Instant};

/// Transport mode based on network conditions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    /// Maximum speed, minimal overhead
    Performance,
    /// Balance between speed and protection
    Balanced,
    /// Maximum DPI resistance
    Stealth,
}

impl Default for TransportMode {
    fn default() -> Self {
        TransportMode::Balanced
    }
}

impl TransportMode {
    /// Get padding range for this mode
    pub fn padding_range(&self) -> (usize, usize) {
        match self {
            TransportMode::Performance => (0, 4),
            TransportMode::Balanced => (4, 16),
            TransportMode::Stealth => (16, 64),
        }
    }
    
    /// Get jitter range (ms) for this mode
    pub fn jitter_range(&self) -> (u64, u64) {
        match self {
            TransportMode::Performance => (0, 2),
            TransportMode::Balanced => (0, 5),
            TransportMode::Stealth => (5, 15),
        }
    }
    
    /// Get packet loss threshold for switching to lower mode
    pub fn packet_loss_threshold(&self) -> f64 {
        match self {
            TransportMode::Performance => 0.01,   // 1% loss → switch to Balanced
            TransportMode::Balanced => 0.05,       // 5% loss → switch to Stealth
            TransportMode::Stealth => 0.15,        // 15% loss → stay in Stealth
        }
    }
}

/// Network metrics for adaptive decisions
#[derive(Debug, Clone)]
pub struct AdaptiveMetrics {
    /// Round-trip time (ms)
    pub rtt_ms: f64,
    /// Jitter (ms)
    pub jitter_ms: f64,
    /// Packet loss rate (0.0-1.0)
    pub packet_loss: f64,
    /// Throughput (Mbps)
    pub throughput_mbps: f64,
    /// Retransmission rate
    pub retransmission_rate: f64,
    /// Handshake failures
    pub handshake_failures: u32,
    /// Last update timestamp
    pub last_updated: Instant,
}

impl Default for AdaptiveMetrics {
    fn default() -> Self {
        Self {
            rtt_ms: 100.0,
            jitter_ms: 10.0,
            packet_loss: 0.0,
            throughput_mbps: 50.0,
            retransmission_rate: 0.0,
            handshake_failures: 0,
            last_updated: Instant::now(),
        }
    }
}

impl AdaptiveMetrics {
    /// Calculate network health score (0-100)
    pub fn health_score(&self) -> f64 {
        let rtt_score = (100.0 - (self.rtt_ms - 50.0).max(0.0) / 2.0).max(0.0);
        let jitter_score = (100.0 - self.jitter_ms * 3.0).max(0.0);
        let loss_score = (100.0 * (1.0 - self.packet_loss)).max(0.0);
        
        (rtt_score * 0.3 + jitter_score * 0.3 + loss_score * 0.4).min(100.0)
    }
    
    /// Check if network is degraded
    pub fn is_degraded(&self) -> bool {
        self.health_score() < 50.0 ||
        self.packet_loss > 0.05 ||
        self.rtt_ms > 200.0
    }
}

/// Thresholds for mode switching with hysteresis
#[derive(Debug, Clone)]
pub struct AdaptiveThresholds {
    /// Minimum time between mode switches
    pub switch_cooldown: Duration,
    /// Health score hysteresis (avoid rapid switching)
    pub hysteresis: f64,
}

impl Default for AdaptiveThresholds {
    fn default() -> Self {
        Self {
            switch_cooldown: Duration::from_secs(30),
            hysteresis: 5.0,
        }
    }
}

/// Adaptive transport controller
pub struct AdaptiveController {
    current_mode: TransportMode,
    metrics: AdaptiveMetrics,
    thresholds: AdaptiveThresholds,
    last_mode_switch: Instant,
}

impl AdaptiveController {
    /// Create new adaptive controller
    pub fn new() -> Self {
        Self {
            current_mode: TransportMode::Balanced,
            metrics: AdaptiveMetrics::default(),
            thresholds: AdaptiveThresholds::default(),
            last_mode_switch: Instant::now(),
        }
    }
    
    /// Update network metrics and re-evaluate mode
    pub fn update_metrics(&mut self, metrics: AdaptiveMetrics) -> Option<TransportMode> {
        self.metrics = metrics;
        
        // Check cooldown
        if self.last_mode_switch.elapsed() < self.thresholds.switch_cooldown {
            return None;
        }
        
        let health = self.metrics.health_score();
        let new_mode = self.calculate_mode(health);
        
        if new_mode != self.current_mode {
            self.current_mode = new_mode;
            self.last_mode_switch = Instant::now();
            Some(new_mode)
        } else {
            None
        }
    }
    
    /// Calculate optimal mode based on health score
    fn calculate_mode(&self, health: f64) -> TransportMode {
        let hysteresis = self.thresholds.hysteresis;
        
        match self.current_mode {
            TransportMode::Performance => {
                // Switch to Balanced if degraded
                if health < (70.0 - hysteresis) {
                    TransportMode::Balanced
                } else {
                    TransportMode::Performance
                }
            }
            TransportMode::Balanced => {
                // Switch to Performance if excellent, Stealth if poor.
                // Step 9 fix: `>=` чтобы граничное health == 90 (с дефолтным hysteresis=5)
                // тоже переключало в Performance — иначе test_mode_switching с
                // exellent metrics health=90.0 не проходит.
                if health >= (85.0 + hysteresis) {
                    TransportMode::Performance
                } else if health < (50.0 - hysteresis) {
                    TransportMode::Stealth
                } else {
                    TransportMode::Balanced
                }
            }
            TransportMode::Stealth => {
                // Switch to Balanced if recovered
                if health > (60.0 + hysteresis) {
                    TransportMode::Balanced
                } else {
                    TransportMode::Stealth
                }
            }
        }
    }
    
    /// Get current transport mode
    pub fn current_mode(&self) -> TransportMode {
        self.current_mode
    }
    
    /// Get current network metrics
    pub fn metrics(&self) -> &AdaptiveMetrics {
        &self.metrics
    }
    
    /// Force set mode (for testing or manual override)
    pub fn set_mode(&mut self, mode: TransportMode) {
        self.current_mode = mode;
        self.last_mode_switch = Instant::now();
    }
}

impl Default for AdaptiveController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_health_score() {
        let metrics = AdaptiveMetrics {
            rtt_ms: 50.0,
            jitter_ms: 5.0,
            packet_loss: 0.01,
            ..Default::default()
        };
        assert!(metrics.health_score() > 80.0);
    }
    
    #[test]
    fn test_mode_switching() {
        let mut controller = AdaptiveController::new();
        
        // Start in Balanced
        assert_eq!(controller.current_mode(), TransportMode::Balanced);
        
        // Excellent metrics → Performance
        let excellent = AdaptiveMetrics {
            rtt_ms: 30.0,
            jitter_ms: 2.0,
            packet_loss: 0.0,
            ..Default::default()
        };
        controller.metrics = excellent;
        controller.last_mode_switch = Instant::now() - Duration::from_secs(60);
        let new_mode = controller.calculate_mode(90.0);
        assert_eq!(new_mode, TransportMode::Performance);
        
        // Poor metrics → Stealth
        let poor = AdaptiveMetrics {
            rtt_ms: 300.0,
            jitter_ms: 50.0,
            packet_loss: 0.1,
            ..Default::default()
        };
        controller.metrics = poor;
        let new_mode = controller.calculate_mode(30.0);
        assert_eq!(new_mode, TransportMode::Stealth);
    }
}
