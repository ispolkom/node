//! Finite State Machine for transport behavior.
//!
//! States: Normal, DualEnabled, Stealth, Turbo
//! Transitions with hysteresis and cooldown.

use std::time::{Duration, Instant};
use crate::state_manager::config::StateManagerConfig;
use crate::state_manager::telemetry::Telemetry;
use crate::state_manager::dpi::is_dpi_suspected;

/// Connection states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Normal,
    DualEnabled,
    Stealth,
}

impl std::fmt::Display for ConnState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnState::Normal => write!(f, "Normal"),
            ConnState::DualEnabled => write!(f, "DualEnabled"),
            ConnState::Stealth => write!(f, "Stealth"),
        }
    }
}

/// Main State Manager
pub struct StateManager {
    pub state: ConnState,
    pub last_transition: Instant,
    pub last_telemetry: Option<Telemetry>,
    config: StateManagerConfig,
}

impl StateManager {
    pub fn new(config: StateManagerConfig) -> Self {
        Self {
            state: ConnState::Normal,
            last_transition: Instant::now(),
            last_telemetry: None,
            config,
        }
    }

    /// Update state based on new telemetry
    pub fn update(&mut self, telemetry: Telemetry) -> Option<ConnState> {
        let now = Instant::now();
        self.last_telemetry = Some(telemetry);

        // Cooldown check
        if now.duration_since(self.last_transition) < Duration::from_millis(self.config.cooldown_ms) {
            return None;
        }

        let old_state = self.state;
        let new_state = self.compute_next_state(&telemetry);

        if new_state != old_state {
            self.transition(new_state);
            Some(new_state)
        } else {
            None
        }
    }

    /// Compute next state based on telemetry (deterministic)
    fn compute_next_state(&self, t: &Telemetry) -> ConnState {
        match self.state {
            ConnState::Normal => {
                // Normal -> DualEnabled (loss too high)
                if t.loss_rate.max(t.peer_loss_rate) > self.config.loss_threshold_dual_enable {
                    return ConnState::DualEnabled;
                }
                // Normal -> Stealth (DPI suspected)
                if is_dpi_suspected(t, &self.config) {
                    return ConnState::Stealth;
                }
                ConnState::Normal
            }

            ConnState::DualEnabled => {
                // DualEnabled -> Normal (loss recovered)
                if t.loss_rate.max(t.peer_loss_rate) < self.config.loss_threshold_dual_disable {
                    return ConnState::Normal;
                }
                // DualEnabled -> Stealth (DPI suspected)
                if is_dpi_suspected(t, &self.config) {
                    return ConnState::Stealth;
                }
                ConnState::DualEnabled
            }

            ConnState::Stealth => {
                // Stealth -> Normal after timeout
                let in_stealth = Instant::now().duration_since(self.last_transition);
                if in_stealth > Duration::from_secs(self.config.stealth_timeout_secs) {
                    return ConnState::Normal;
                }
                ConnState::Stealth
            }

        }
    }

    fn transition(&mut self, new_state: ConnState) {
        tracing::info!(
            "State transition: {} -> {}",
            self.state,
            new_state
        );
        self.state = new_state;
        self.last_transition = Instant::now();
    }

    pub fn current_state(&self) -> ConnState {
        self.state
    }

    /// Get current loss rate from last telemetry
    pub fn get_current_loss_rate(&self) -> f64 {
        self.last_telemetry.map(|t| t.loss_rate).unwrap_or(0.0)
    }
}