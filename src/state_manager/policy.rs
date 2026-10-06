//! Policy Engine: maps state to actions.
//!
//! Pure deterministic mapping, no side effects.

use crate::state_manager::fsm::ConnState;

/// Actions that can be executed by transport layer
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// No action needed
    None,
    /// Enable DUAL-PATH redundancy
    EnableDualPath,
    /// Disable DUAL-PATH (single path)
    DisableDualPath,
    /// Increase packet padding to specified bytes
    IncreasePadding(u16),
    /// Reset padding to baseline
    ResetPadding,
    /// Enable timing jitter for obfuscation
    EnableJitter,
    /// Disable timing jitter
    DisableJitter,
    /// Rotate discovery and data ports
    RotatePorts,
    /// Optimize for low latency (minimal padding, single path)
    OptimizeLatency,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::None => write!(f, "None"),
            Action::EnableDualPath => write!(f, "EnableDualPath"),
            Action::DisableDualPath => write!(f, "DisableDualPath"),
            Action::IncreasePadding(sz) => write!(f, "IncreasePadding({})", sz),
            Action::ResetPadding => write!(f, "ResetPadding"),
            Action::EnableJitter => write!(f, "EnableJitter"),
            Action::DisableJitter => write!(f, "DisableJitter"),
            Action::RotatePorts => write!(f, "RotatePorts"),
            Action::OptimizeLatency => write!(f, "OptimizeLatency"),
        }
    }
}

/// Policy Engine: pure mapping from state to actions
pub struct PolicyEngine;

impl PolicyEngine {
    pub fn get_actions(state: ConnState) -> Vec<Action> {
        match state {
            ConnState::Normal => vec![
                Action::DisableDualPath,
                Action::ResetPadding,
                Action::DisableJitter,
            ],
            ConnState::DualEnabled => vec![
                Action::EnableDualPath,
                Action::ResetPadding,
                Action::DisableJitter,
            ],
            ConnState::Stealth => vec![
                Action::EnableDualPath,
                Action::IncreasePadding(1024),
                Action::EnableJitter,
                Action::RotatePorts,
            ],
        }
    }

    /// Get primary action (for simple integration)
    pub fn primary_action(state: ConnState) -> Action {
        let actions = Self::get_actions(state);
        actions.first().cloned().unwrap_or(Action::None)
    }
}