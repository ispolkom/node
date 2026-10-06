// src/p2p/encryption.rs
//! P2P End-to-End encryption
//!
//! TODO: Для прототипа используем встроенное шифрование P2P transport
//! В будущем будет отдельное E2E шифрование поверх transport

use crate::util::HashId;
use anyhow::Result;

/// E2E шифрование для P2P коммуникаций (заглушка для прототипа)
pub struct P2PEncryption {
    _placeholder: (),
}

impl P2PEncryption {
    /// Создать новый P2P E2E encryption
    pub fn new() -> Self {
        Self { _placeholder: () }
    }

    /// Зашифровать сообщение (для прототипа - без шифрования)
    pub async fn encrypt_for_peer(&self, _peer_id: HashId, plaintext: &[u8]) -> Result<Vec<u8>> {
        // TODO: Временно без шифрования
        // P2P transport сам шифрует через send_encrypted
        Ok(plaintext.to_vec())
    }

    /// Расшифровать сообщение (для прототипа - без шифрования)
    pub async fn decrypt_from_peer(&self, _peer_id: HashId, encrypted: &[u8]) -> Result<Vec<u8>> {
        // TODO: Временно без шифрования
        // P2P transport сам расшифровывает
        Ok(encrypted.to_vec())
    }
}

/// Session key для E2E общения с конкретным peer (для будущего использования)
#[derive(Debug, Clone)]
pub struct P2PSessionKey {
    pub peer_id: HashId,
    pub key: Vec<u8>,        // 256-bit session key
    pub created_at: u64,
}
