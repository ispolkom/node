// src/netlayer/mod.rs
//! Netlayer Module - Network Transport Layer
//! ============================================
//!
//! P2P network communication layer

pub mod peer;
pub mod packet;
pub mod external_ip;
pub mod interface_detector;
pub mod node_introspection;
pub mod encryption;
pub mod tunnel;
pub mod bootstrap;
pub mod admission;
pub mod transport;
pub mod port_manager;
pub mod socket_manager;
pub mod adaptive;
pub mod cli;
pub mod tun_device;
pub mod tun_exit;
pub mod rawip_tunnel;
pub mod nat;
pub mod nat_pmp;
pub mod tls_cert;
pub mod ws_transport;
pub mod pairing;
pub mod broadcast;

// Re-exports
pub use peer::PeerInfo;
pub use packet::{NetPacket, PacketType, HelloPacket, HelloType};
pub use external_ip::ExternalIpService;
pub use interface_detector::{NetworkTopology, NetworkInterface};
pub use node_introspection::{NodeIntrospection, NodeCapabilities, NodeRole, NodeProfile};
pub use encryption::{EncryptionManager, encrypt_data, decrypt_data};
pub use tunnel::{TunnelManager, TunnelInfo, TunnelStatus, TunnelTimeout, tunnel_monitor_task};
pub use bootstrap::{BootstrapConfig, BootstrapNode, BootstrapManager};
pub use socket_manager::{SocketManager, SocketPair};
pub use transport::{P2PTransport, HelloEvent, IncomingPacket, ExitHandlerRequest};
pub use cli::P2PCli;
pub use tun_device::{YandiTunDevice, YandiTunManager, IPv6PacketInfo};
pub use rawip_tunnel::RawIpTunnel;
pub mod relay;
