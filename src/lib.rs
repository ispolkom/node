// src/lib.rs
//! YANDI Node — peer-to-peer network node
//! ======================================
//!
//! Encrypted chat, files and calls, proxying and exit through other jurisdictions, layered-encryption circuits.
//! Module map: `docs/ARCHITECTURE.md`.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Application Layer                         │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Network Layer                             │
//! │        (netlayer - peers, packets, discovery)              │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Core Layer                               │
//! │  (core - identity, cryptography)                            │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Utilities                                │
//! │  (util - types, helpers)                                   │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Modules
//!
//! - **netlayer** - Network transport layer
//! - **dht** - Kademlia distributed hash table
//! - **bootstrap** - Initial peer discovery
//! - **core** - Cryptographic identity and configuration
//! - **util** - Common types and utilities

pub mod bootstrap;
pub mod communication;
pub mod core;
pub mod crypto;
pub mod dataplane;
pub mod dht;
pub mod exit_policy;
pub mod exit_select;
pub mod hops;
pub mod hops_net;
pub mod ip_history;
pub mod mdns;
pub mod mobile_api;
pub mod mobile_files;
pub mod mobile_groups;
pub mod mobile_self;
pub mod mobile_tls;
pub mod netlayer;
pub mod network_offers;
pub mod observability;
pub mod p2p;
pub mod p2p_tunnel;
pub mod protocol;
pub mod proxy;
pub mod reachability;
pub mod reciprocity;
pub mod relay_net;
pub mod route_rules;
pub mod socks5;
pub mod supervisor;
pub mod testnet;
pub mod upstream_proxy;
pub mod util;
pub mod web;

// Re-exports for convenience
pub use core::{
    effective_ws_bind, get_config, init_config, set_ws_bind_override, update_config, ClientConfig,
    NetConfig, NodeIdentity, PortsConfig, WsConfig, YandiConfig,
};
pub use netlayer::adaptive::{AdaptiveController, AdaptiveMetrics, TransportMode};
pub use netlayer::transport::{StreamStats, TransportState};
pub use netlayer::{
    BootstrapConfig, BootstrapManager, EncryptionManager, ExitHandlerRequest, ExternalIpService,
    HelloEvent, HelloPacket, HelloType, IPv6PacketInfo, NetPacket, NetworkTopology,
    NodeCapabilities, NodeIntrospection, NodeProfile, NodeRole, P2PCli, P2PTransport, PacketType,
    PeerInfo, YandiTunManager,
};
pub use util::{
    format_bytes, mask_hash_id, mask_ipv4, mask_ipv6, mask_public_key, HashId, NodePower,
    OSDetector, OperatingSystem, SystemInfo,
};

pub use dht::{DhtQuery, DhtQueryType, DhtResponse, DhtStorage, KBucket, KTable, Kademlia};
pub use netlayer::port_manager::{PortManager, PortState};
// pub use bootstrap::{BootstrapManager, BootstrapNode, BootstrapConfig, BootstrapSource, NodeType};  // TODO: конфликтует с netlayer::bootstrap
pub use dataplane::{
    DataTransport, DataplaneMetrics, MultipathManager, PacketPriority, PathSelector, QoSManager,
    TransportConfig, TransportStats, TransportType as DataTransportType,
};
pub use mdns::{
    DiscoveredNode, MdnsAnnouncer, MdnsBrowser, MdnsService, YANDI_ADMIN_TYPE, YANDI_SERVICE_TYPE,
};
pub use observability::{init_logging, LogLevel, NetworkMetrics};
pub use proxy::{HttpProxyClient, HttpProxyGateway, ProxyConfig};
pub use socks5::{
    ExitNodeHandler, Socks5Address, Socks5AuthMethod, Socks5Client, Socks5Command, Socks5Config,
    Socks5ProxyServer, Socks5Server,
};
pub use web::auth::{load_auth_state, AuthState};
pub use web::{NodeInfo, WebServer};
// P2P Transport for communications (port 9999) - без алиаса, используем полный путь

/// YANDI version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// YANDI name
pub const NAME: &str = "YANDI";

pub mod media;

// Initialize media system
pub fn init_media() -> Result<(), String> {
    media::init()
}

// State Manager - adaptive transport control plane
pub mod state_manager;
