// src/p2p/peer.rs
//! P2P peer model

use crate::util::HashId;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2PNatStatus {
    Unknown,
    Public,
    BehindNat,
}

impl P2PNatStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Public => "Public",
            Self::BehindNat => "BehindNAT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct P2PPeer {
    pub id: HashId,
    pub addr: String,
    pub data_addr: Option<String>,
    pub p2p_data_addr: Option<String>,
    pub local_addr: Option<String>,
    pub public_addr: Option<String>,
    pub ipv6_virtual: Option<[u8; 16]>,
    pub last_seen: u128,
    pub nat_status: P2PNatStatus,
    pub ed25519_public: Option<[u8; 32]>,
}
