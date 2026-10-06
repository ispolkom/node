//! Configuration for State Manager
//!
//! All thresholds, hysteresis values, and cooldown timers.

use serde::{Deserialize, Serialize};

/// Main configuration for StateManager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateManagerConfig {
    // Telemetry windows
    pub telemetry_window_secs: u32,
    pub telemetry_sample_interval_ms: u32,

    // Loss thresholds
    pub loss_threshold_dual_enable: f64,      // > this -> DualEnabled
    pub loss_threshold_dual_disable: f64,     // < this -> back to Normal

    // RTT thresholds
    pub rtt_threshold_turbo_ms: f64,          // < this + loss=0 -> Turbo
    pub rtt_threshold_turbo_exit_ms: f64,     // > this -> exit Turbo

    // DPI suspicion thresholds
    pub rst_threshold: u32,                   // > this -> DPI suspected
    pub icmp_threshold: u32,                  // > this -> DPI suspected
    pub entropy_threshold: f64,               // > this -> DPI suspected

    // Timing
    pub cooldown_ms: u64,                     // minimum time between transitions
    pub stealth_timeout_secs: u64,            // time in Stealth before returning to Normal
    pub hysteresis_secs: u64,                 // time to confirm condition

    // Action parameters
    pub dual_path_enabled: bool,
    pub max_padding_bytes: u16,
    pub min_padding_bytes: u16,
    pub jitter_ms: u16,
}

impl Default for StateManagerConfig {
    fn default() -> Self {
        Self {
            telemetry_window_secs: 60,
            telemetry_sample_interval_ms: 1000,

            loss_threshold_dual_enable: 0.01,    // 1%
            loss_threshold_dual_disable: 0.005,  // 0.5%

            rtt_threshold_turbo_ms: 0.0,
            rtt_threshold_turbo_exit_ms: 60.0,

            rst_threshold: 3,
            icmp_threshold: 1,
            entropy_threshold: 0.85,

            cooldown_ms: 5000,                   // 5 seconds
            stealth_timeout_secs: 600,           // 10 minutes
            hysteresis_secs: 5,                  // 5 seconds

            dual_path_enabled: true,
            max_padding_bytes: 1280,
            min_padding_bytes: 512,
            jitter_ms: 50,
        }
    }
}
