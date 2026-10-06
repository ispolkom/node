// src/observability/mod.rs
//! Observability Module - Monitoring & Logging
//! ===============================================
//!
//! Simplified metrics and logging for P2P network monitoring

pub mod metrics;
pub mod logging;

pub use metrics::NetworkMetrics;
pub use logging::{init_logging, LogLevel};
