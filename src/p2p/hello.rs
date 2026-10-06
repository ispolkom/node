// src/p2p/hello.rs
//! P2P Handshake (independent from netlayer)

use crate::util::HashId;
use serde::{Serialize, Deserialize};
use serde_with::serde_as;

pub const P2P_MAGIC: [u8; 4] = *b"P2P1";
pub const P2P_CURRENT_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum P2PHelloType {
    Request = 0,
    Ack = 1,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PHelloPacket {
    pub hello_type: P2PHelloType,
    pub version: u8,
    pub node_id: HashId,
    pub x25519_public: [u8; 32],
    pub timestamp: u64,
    pub nonce: u64,
    pub p2p_data_addr: String,
    // SEC-03: Ed25519 identity key binds node_id to the ECDH key exchange
    pub ed25519_public: [u8; 32],
    #[serde_as(as = "serde_with::Bytes")]
    pub signature: [u8; 64],
}

impl P2PHelloPacket {
    pub fn new_request(
        node_id: HashId,
        x25519_public: [u8; 32],
        p2p_data_addr: String,
        ed25519_public: [u8; 32],
    ) -> Self {
        Self {
            hello_type: P2PHelloType::Request,
            version: P2P_CURRENT_VERSION,
            node_id,
            x25519_public,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            nonce: rand::random(),
            p2p_data_addr,
            ed25519_public,
            signature: [0u8; 64],
        }
    }

    pub fn new_ack(
        node_id: HashId,
        x25519_public: [u8; 32],
        p2p_data_addr: String,
        request_nonce: u64,
        ed25519_public: [u8; 32],
    ) -> Self {
        Self {
            hello_type: P2PHelloType::Ack,
            version: P2P_CURRENT_VERSION,
            node_id,
            x25519_public,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            nonce: request_nonce,
            p2p_data_addr,
            ed25519_public,
            signature: [0u8; 64],
        }
    }

    /// Deterministic byte representation of all fields except signature.
    /// This is what gets signed and verified.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(130);
        buf.push(self.hello_type as u8);
        buf.push(self.version);
        buf.extend_from_slice(&self.node_id.0);
        buf.extend_from_slice(&self.x25519_public);
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        buf.extend_from_slice(&self.nonce.to_be_bytes());
        let addr = self.p2p_data_addr.as_bytes();
        buf.extend_from_slice(&(addr.len() as u32).to_be_bytes());
        buf.extend_from_slice(addr);
        buf.extend_from_slice(&self.ed25519_public);
        buf
    }

    /// Sign this packet in-place with the node's Ed25519 signing key.
    pub fn sign(&mut self, identity: &crate::core::NodeIdentity) -> Result<(), String> {
        let msg = self.canonical_bytes();
        let sig_vec = identity.sign(&msg)?;
        if sig_vec.len() != 64 {
            return Err(format!("Unexpected signature length: {}", sig_vec.len()));
        }
        self.signature.copy_from_slice(&sig_vec);
        Ok(())
    }

    /// Verify the embedded Ed25519 signature against the embedded ed25519_public key.
    pub fn verify_signature(&self) -> Result<(), String> {
        use ed25519_dalek::Verifier;
        let msg = self.canonical_bytes();
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&self.ed25519_public)
            .map_err(|e| format!("Invalid ed25519 key: {}", e))?;
        let sig = ed25519_dalek::Signature::from_bytes(&self.signature);
        verifying_key
            .verify(&msg, &sig)
            .map_err(|_| "Hello signature verification failed".to_string())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&P2P_MAGIC);
        let data = bincode::serialize(self).map_err(|e| e.to_string())?;
        buf.extend_from_slice(&data);
        Ok(buf)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < 4 {
            return Err("Packet too short".to_string());
        }
        if &data[0..4] != P2P_MAGIC.as_ref() {
            return Err("Invalid P2P magic".to_string());
        }
        bincode::deserialize(&data[4..]).map_err(|e| e.to_string())
    }
}
