// src/dataplane/mod.rs
//! Dataplane Module - Adaptive Transport Layer
//! =============================================
//!
//! Intelligent transport layer for P2P network adaptation

pub mod stream;
pub mod registry;
pub mod transport;
pub mod qos;
pub mod metrics;
pub mod multipath;

pub use stream::{ReliableStream, StreamFrame, StreamHeader, StreamMsgType, StreamState};
pub use registry::{StreamRegistry, StreamRegistryStats, SharedStreamRegistry};
pub use transport::{DataTransport, TransportConfig, TransportType};
pub use qos::{QoSManager, PacketPriority};
pub use metrics::{DataplaneMetrics, TransportStats};
pub use multipath::{MultipathManager, PathSelector};
