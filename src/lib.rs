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

pub mod netlayer;
pub mod dht;
pub mod bootstrap;
pub mod dataplane;
pub mod socks5;
pub mod observability;
pub mod core;
pub mod util;
pub mod crypto;
pub mod proxy;
pub mod protocol;
pub mod mdns;
pub mod web;
pub mod communication;
pub mod p2p_tunnel;
pub mod p2p;
pub mod testnet;
pub mod exit_policy;
pub mod network_offers;
pub mod exit_select;
pub mod reciprocity;
pub mod hops;
pub mod hops_net;
pub mod reachability;
pub mod relay_net;
pub mod mobile_tls;
pub mod route_rules;
pub mod supervisor;

// Re-exports for convenience
pub use core::{NodeIdentity, NetConfig, YandiConfig, PortsConfig, ClientConfig, WsConfig, init_config, get_config, update_config, set_ws_bind_override, effective_ws_bind};
pub use util::{HashId, OSDetector, SystemInfo, NodePower, OperatingSystem, mask_hash_id, mask_ipv6, mask_ipv4, mask_public_key, format_bytes};
pub use netlayer::{PeerInfo, NetPacket, PacketType, HelloPacket, HelloType, ExternalIpService, NetworkTopology, NodeIntrospection, NodeCapabilities, NodeRole, NodeProfile, EncryptionManager, P2PTransport, HelloEvent, P2PCli, BootstrapManager, BootstrapConfig, ExitHandlerRequest, YandiTunManager, IPv6PacketInfo};
pub use netlayer::adaptive::{AdaptiveController, TransportMode, AdaptiveMetrics};
pub use netlayer::transport::{TransportState, StreamStats};

pub use netlayer::port_manager::{PortManager, PortState};
pub use dht::{Kademlia, KTable, KBucket, DhtStorage, DhtQuery, DhtResponse, DhtQueryType};
// pub use bootstrap::{BootstrapManager, BootstrapNode, BootstrapConfig, BootstrapSource, NodeType};  // TODO: конфликтует с netlayer::bootstrap
pub use dataplane::{DataTransport, TransportConfig, TransportType as DataTransportType, QoSManager, PacketPriority, DataplaneMetrics, TransportStats, MultipathManager, PathSelector};
pub use socks5::{Socks5Server, Socks5ProxyServer, Socks5Client, Socks5Config, Socks5Command, Socks5AuthMethod, Socks5Address, ExitNodeHandler};
pub use observability::{NetworkMetrics, init_logging, LogLevel};
pub use proxy::{HttpProxyClient, HttpProxyGateway, ProxyConfig};
pub use mdns::{MdnsService, MdnsAnnouncer, MdnsBrowser, DiscoveredNode, YANDI_SERVICE_TYPE, YANDI_ADMIN_TYPE};
pub use web::{WebServer, NodeInfo};
pub use web::auth::{AuthState, load_auth_state};
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
