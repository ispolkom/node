// src/p2p/encryption.rs
//! P2P End-to-End encryption
//!
//! P2P E2E encryption boundary.
//!
//! The authenticated E2E protocol is not implemented yet. This boundary
//! therefore fails closed instead of returning plaintext.

use crate::util::HashId;
use anyhow::Result;

/// P2P E2E encryption boundary. Deliberately unavailable until implemented.
pub struct P2PEncryption {}

impl P2PEncryption {
    /// Создать новый P2P E2E encryption
    pub fn new() -> Self {
        Self {}
    }

    /// Refuse to send data until authenticated E2E is available.
    pub async fn encrypt_for_peer(&self, _peer_id: HashId, _plaintext: &[u8]) -> Result<Vec<u8>> {
        Err(anyhow::anyhow!(
            "authenticated P2P E2E encryption is not implemented"
        ))
    }

    /// Refuse to interpret data as E2E until authenticated E2E is available.
    pub async fn decrypt_from_peer(&self, _peer_id: HashId, _encrypted: &[u8]) -> Result<Vec<u8>> {
        Err(anyhow::anyhow!(
            "authenticated P2P E2E decryption is not implemented"
        ))
    }
}

/// Session key для E2E общения с конкретным peer (для будущего использования)
#[derive(Debug, Clone)]
pub struct P2PSessionKey {
    pub peer_id: HashId,
    pub key: Vec<u8>, // 256-bit session key
    pub created_at: u64,
}
