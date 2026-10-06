//! State Manager Module
//!
//! Centralized control plane for YANDI transport layer.
//! Implements deterministic FSM for adaptive behavior:
//! - Normal, DualEnabled, Stealth, Turbo states
//! - Hysteresis and cooldown for anti-flapping
//! - Policy engine for action dispatch
//!
//! This module does NOT handle cryptography or payload processing.

pub mod config;
pub mod dpi;
pub mod fsm;
pub mod policy;
pub mod telemetry;

pub use config::StateManagerConfig;
pub use fsm::{ConnState, StateManager};
pub use policy::{Action, PolicyEngine};
pub use telemetry::{Telemetry, TelemetryCollector};
