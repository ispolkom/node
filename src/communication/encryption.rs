// src/communication/encryption.rs
//! End-to-End encryption for P2P communications
//!
//! E2E encryption boundary.
//!
//! The real authenticated E2E protocol is not implemented yet. Until it is,
//! this boundary must fail closed instead of returning plaintext.

use crate::util::HashId;
use anyhow::Result;

/// E2E encryption boundary. Deliberately unavailable until a real protocol is wired.
pub struct E2EEncryption;

impl E2EEncryption {
    /// Создать новый E2E encryption
    pub fn new() -> Self {
        Self
    }

    /// Refuse to send data until authenticated E2E is available.
    pub async fn encrypt_for_peer(&self, _peer_id: HashId, _plaintext: &[u8]) -> Result<Vec<u8>> {
        Err(anyhow::anyhow!(
            "authenticated E2E encryption is not implemented"
        ))
    }

    /// Refuse to interpret data as E2E until authenticated E2E is available.
    pub async fn decrypt_from_peer(&self, _peer_id: HashId, _encrypted: &[u8]) -> Result<Vec<u8>> {
        Err(anyhow::anyhow!(
            "authenticated E2E decryption is not implemented"
        ))
    }
}

/// Session key для E2E общения с конкретным peer (для будущего использования)
#[derive(Debug, Clone)]
pub struct SessionKey {
    pub peer_id: HashId,
    pub key: Vec<u8>, // 256-bit session key
    pub created_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_e2e_is_fail_closed() {
        let e2e = E2EEncryption::new();
        assert!(e2e
            .encrypt_for_peer(HashId::default(), b"secret")
            .await
            .is_err());
        assert!(e2e
            .decrypt_from_peer(HashId::default(), b"plaintext")
            .await
            .is_err());
    }
}
