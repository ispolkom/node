//! DPI (Deep Packet Inspection) suspicion heuristics.
//!
//! Simple rule-based detection for potential DPI/blocking.

use crate::state_manager::config::StateManagerConfig;
use crate::state_manager::telemetry::Telemetry;

/// Check if DPI is suspected based on telemetry
pub fn is_dpi_suspected(t: &Telemetry, config: &StateManagerConfig) -> bool {
    // Too many RST packets (connection resets)
    if t.rst_events > config.rst_threshold {
        tracing::debug!("DPI suspicion: RST events {}", t.rst_events);
        return true;
    }

    // ICMP unreachable (target blocked)
    if t.icmp_unreachable > config.icmp_threshold {
        tracing::debug!("DPI suspicion: ICMP unreachable");
        return true;
    }

    // Sudden silence when traffic was expected
    if t.silence_detected {
        tracing::debug!("DPI suspicion: unexpected silence");
        return true;
    }

    // Unusually high entropy (might indicate random DPI injection)
    if t.traffic_entropy > config.entropy_threshold {
        tracing::debug!("DPI suspicion: high entropy {}", t.traffic_entropy);
        return true;
    }

    false
}

/// Advanced DPI detection with persistence
pub struct DpiDetector {
    suspicion_count: u32,
    last_reset: std::time::Instant,
}

impl DpiDetector {
    pub fn new() -> Self {
        Self {
            suspicion_count: 0,
            last_reset: std::time::Instant::now(),
        }
    }

    pub fn update(&mut self, t: &Telemetry, config: &StateManagerConfig) -> bool {
        if is_dpi_suspected(t, config) {
            self.suspicion_count += 1;
        } else {
            // Decay: reset after 60 seconds of no suspicion
            if self.last_reset.elapsed() > std::time::Duration::from_secs(60) {
                self.suspicion_count = self.suspicion_count.saturating_sub(1);
                self.last_reset = std::time::Instant::now();
            }
        }

        // Confirm suspicion after 3 detections
        self.suspicion_count >= 3
    }
}