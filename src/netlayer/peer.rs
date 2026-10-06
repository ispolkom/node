// src/netlayer/peer.rs
//! Peer Information
//! ================

use crate::util::HashId;
use crate::core::NodeIdentity;
use crate::netlayer::nat::NatStatus;
use crate::netlayer::port_manager::{DEFAULT_DATA_PORT, DEFAULT_DISCOVERY_PORT};
use std::time::{SystemTime, UNIX_EPOCH, Instant};

/// Peer information
pub const PEER_ONLINE_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerInfo {
    pub id: HashId,
    pub addr: String,                    // Discovery endpoint (usually 9000)
    pub data_addr: Option<String>,       // Legacy data endpoint
    pub p2p_data_addr: Option<String>,   // P2P Communication endpoint
    pub p2p_x25519_public: Option<[u8; 32]>,   // P2P X25519 public key from peer
    pub local_addr: Option<String>,
    pub public_addr: Option<String>,
    pub ipv6_virtual: Option<[u8; 16]>,
    pub last_seen: u128,

    // === Peer-specific ports for rotation (new) ===
    pub remote_discovery_port: u16,
    pub remote_data_port: u16,
    pub last_port_update: Instant,
    /// Peer's loss rate (from their perspective) * 1000
    pub peer_loss_rate: u16,
    /// Peer's TX speed (bytes/sec) - what they send to us
    pub peer_tx_speed: u32,
    pub port_update_seq: u64,

    /// NAT status of the peer
    pub nat_status: NatStatus,

    /// 🌐 Маршрутизация: если true — отправка идёт через relay (не direct).
    /// Выставляется heartbeat-логикой при пропущенных подряд direct heartbeat'ах,
    /// сбрасывается при успешном direct probe.
    pub use_relay: bool,

    /// 🌐 Подряд пропущенных direct heartbeat'ов (для перехода на relay).
    pub direct_miss_streak: u8,

    /// 🛰  Capabilities bits peer'а из последнего Hello (см. hello_caps).
    /// Используется для решений: подходит ли peer как relay, introducer, и т.п.
    pub caps_bits: u16,

    /// 🌍 Iter 3: ISO-3166 alpha-2 self-claim peer'а из последнего Hello (опц.).
    /// Используется для выбора foreign-exit hop'а в circuit'е.
    pub jurisdiction: Option<String>,

    /// Node Identity Binding Fix: Ed25519 signing pubkey this peer proved
    /// ownership of during its last Ed25519-verified Hello handshake
    /// (`verify_peer_handshake`/`_static`) — NOT set for peers only known
    /// via unauthenticated peer-exchange gossip. Used to detect a later
    /// Hello claiming the SAME node_id with a DIFFERENT key — see
    /// `P2PTransport::identity_conflict` in transport.rs.
    pub verified_signing_pubkey: Option<[u8; 32]>,
}

impl PeerInfo {
    pub fn new(id: HashId, addr: &str) -> Self {
        Self {
            id,
            addr: addr.to_string(),
            data_addr: None,
            p2p_x25519_public: None,
            p2p_data_addr: None,
            local_addr: None,
            public_addr: None,
            ipv6_virtual: None,
            last_seen: now_millis(),
            remote_discovery_port: DEFAULT_DISCOVERY_PORT,
            remote_data_port: DEFAULT_DATA_PORT,
            last_port_update: Instant::now(),
            port_update_seq: 0,
            peer_loss_rate: 0,
            peer_tx_speed: 0,
            nat_status: NatStatus::Unknown,
            use_relay: false,
            direct_miss_streak: 0,
            caps_bits: 0,
            jurisdiction: None,
            verified_signing_pubkey: None,
        }
    }

    pub fn with_data_addr(id: HashId, addr: &str, data_addr: &str) -> Self {
        Self {
            id,
            addr: addr.to_string(),
            data_addr: Some(data_addr.to_string()),
            p2p_data_addr: None,
            p2p_x25519_public: None,
            local_addr: None,
            public_addr: None,
            ipv6_virtual: None,
            last_seen: now_millis(),
            remote_discovery_port: DEFAULT_DISCOVERY_PORT,
            remote_data_port: DEFAULT_DATA_PORT,
            last_port_update: Instant::now(),
            port_update_seq: 0,
            peer_loss_rate: 0,
            peer_tx_speed: 0,
            nat_status: NatStatus::Unknown,
            use_relay: false,
            direct_miss_streak: 0,
            caps_bits: 0,
            jurisdiction: None,
            verified_signing_pubkey: None,
        }
    }

    pub fn with_addrs(
        id: HashId,
        addr: &str,
        local_addr: Option<String>,
        public_addr: Option<String>,
    ) -> Self {
        Self {
            id,
            addr: addr.to_string(),
            data_addr: None,
            p2p_data_addr: None,
            p2p_x25519_public: None,
            local_addr,
            public_addr,
            ipv6_virtual: None,
            last_seen: now_millis(),
            remote_discovery_port: DEFAULT_DISCOVERY_PORT,
            remote_data_port: DEFAULT_DATA_PORT,
            last_port_update: Instant::now(),
            port_update_seq: 0,
            peer_loss_rate: 0,
            peer_tx_speed: 0,
            nat_status: NatStatus::Unknown,
            use_relay: false,
            direct_miss_streak: 0,
            caps_bits: 0,
            jurisdiction: None,
            verified_signing_pubkey: None,
        }
    }

    pub fn with_ipv6(
        id: HashId,
        addr: &str,
        local_addr: Option<String>,
        public_addr: Option<String>,
        ipv6_virtual: Option<[u8; 16]>,
    ) -> Self {
        Self {
            id,
            addr: addr.to_string(),
            data_addr: None,
            p2p_x25519_public: None,
            p2p_data_addr: None,
            local_addr,
            public_addr,
            ipv6_virtual,
            last_seen: now_millis(),
            remote_discovery_port: DEFAULT_DISCOVERY_PORT,
            remote_data_port: DEFAULT_DATA_PORT,
            last_port_update: Instant::now(),
            port_update_seq: 0,
            peer_loss_rate: 0,
            peer_tx_speed: 0,
            nat_status: NatStatus::Unknown,
            use_relay: false,
            direct_miss_streak: 0,
            caps_bits: 0,
            jurisdiction: None,
            verified_signing_pubkey: None,
        }
    }

    pub fn touch(&mut self) {
        self.last_seen = now_millis();
    }

    /// Update remote ports with sequence protection
    pub fn update_remote_ports(&mut self, discovery_port: u16, data_port: u16, seq: u64, loss_rate: u16, tx_speed: u32) {
        if seq > self.port_update_seq {
            self.remote_discovery_port = discovery_port;
            self.remote_data_port = data_port;
            if let Some((host, _)) = self.addr.rsplit_once(':') {
                self.addr = format!("{}:{}", host, discovery_port);
            }
            if let Some(current_data_addr) = self.data_addr.clone() {
                if let Some((host, _)) = current_data_addr.rsplit_once(':') {
                    self.data_addr = Some(format!("{}:{}", host, data_port));
                }
            }
            self.port_update_seq = seq;
            self.last_port_update = Instant::now();
            self.peer_loss_rate = loss_rate;
            self.peer_tx_speed = tx_speed;
            self.touch();
        }
    }

    pub fn set_nat_status(&mut self, status: NatStatus) {
        self.nat_status = status;
    }

    pub fn has_cap(&self, bit: u16) -> bool {
        (self.caps_bits & bit) != 0
    }

    pub fn is_mobile(&self) -> bool {
        use crate::netlayer::packet::hello_caps::MOBILE;
        self.has_cap(MOBILE)
    }

    pub fn is_anchor(&self) -> bool {
        use crate::netlayer::packet::hello_caps::ANCHOR;
        self.has_cap(ANCHOR)
    }

    pub fn can_serve_relay(&self) -> bool {
        use crate::netlayer::packet::hello_caps::{RELAY, MOBILE};
        self.has_cap(RELAY) && !self.has_cap(MOBILE)
    }

    pub fn can_introduce(&self) -> bool {
        use crate::netlayer::packet::hello_caps::{INTRODUCER, MOBILE};
        self.has_cap(INTRODUCER) && !self.has_cap(MOBILE)
    }

    pub fn get_nat_status(&self) -> NatStatus {
        self.nat_status
    }

    pub fn can_accept_direct_connection(&self) -> bool {
        self.nat_status.can_accept_direct_connection()
    }

    pub fn needs_relay(&self) -> bool {
        self.nat_status.needs_relay()
    }

    pub fn best_reachable_address(&self) -> Option<&str> {
        self.public_addr
            .as_ref()
            .map(|s| s.as_str())
            .or_else(|| Some(self.addr.as_str()))
            .or_else(|| self.local_addr.as_deref())
    }

    pub fn from_identity_with_ipv6(
        identity: &NodeIdentity,
        addr: &str,
        local_addr: Option<String>,
        public_addr: Option<String>,
    ) -> Self {
        let ipv6_string = identity.generate_ipv6_virtual();
        let ipv6_bytes: Option<[u8; 16]> = ipv6_string
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|addr| addr.octets());
        Self::with_ipv6(
            identity.node_id(),
            addr,
            local_addr,
            public_addr,
            ipv6_bytes,
        )
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
