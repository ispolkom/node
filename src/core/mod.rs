// src/core/mod.rs
//! Core Module
//! ===========
//!
//! Cryptographic identity and configuration

pub mod identity;
pub mod profile;
pub mod crypto;
pub mod config;

pub use identity::NodeIdentity;
pub use config::{NetConfig, YandiConfig, PortsConfig, ClientConfig, WsConfig, init_config, get_config, update_config, set_ws_bind_override, effective_ws_bind};
