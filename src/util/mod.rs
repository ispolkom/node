// src/util/mod.rs
//! Utility Module
//! ===============
//!
//! Common types and utilities

pub mod data_dir;
pub mod private_file;
pub mod types;
pub mod os_detector;
pub mod mask;
pub mod sysmon;
pub mod external_ipv6;

// Re-exports
pub use types::{HashId, NodeName};
pub use os_detector::{OSDetector, SystemInfo as OsSystemInfo, OperatingSystem, OSFamily};
pub use mask::{mask_hash_id, mask_hex, mask_ipv6, mask_ipv4, mask_public_key, mask_bytes, mask_string, format_bytes};
pub use sysmon::{SystemInfo, NodePower, CpuInfo, MemoryInfo};
pub use external_ipv6::{discover_external_ipv6, stun_query_peer};
