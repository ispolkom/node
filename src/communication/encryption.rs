// src/communication/encryption.rs
//! End-to-End encryption for P2P communications
//!
//! TODO: Для прототипа используем встроенное шифрование транспорта
//! В будущем будет отдельное E2E шифрование поверх transport

use crate::util::HashId;
use anyhow::Result;

/// E2E шифрование для коммуникаций (заглушка для прототипа)
pub struct E2EEncryption {
    _placeholder: (),
}

impl E2EEncryption {
    /// Создать новый E2E encryption
    pub fn new() -> Self {
        Self { _placeholder: () }
    }

    /// Зашифровать сообщение (для прототипа - без шифрования)
    pub async fn encrypt_for_peer(&self, _peer_id: HashId, plaintext: &[u8]) -> Result<Vec<u8>> {
        // TODO: Временно без шифрования
        // Transport сам шифрует через send_encrypted
        Ok(plaintext.to_vec())
    }

    /// Расшифровать сообщение (для прототипа - без шифрования)
    pub async fn decrypt_from_peer(&self, _peer_id: HashId, encrypted: &[u8]) -> Result<Vec<u8>> {
        // TODO: Временно без шифрования
        // Transport сам расшифровывает
        Ok(encrypted.to_vec())
    }
}

/// Session key для E2E общения с конкретным peer (для будущего использования)
#[derive(Debug, Clone)]
pub struct SessionKey {
    pub peer_id: HashId,
    pub key: Vec<u8>,        // 256-bit session key
    pub created_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_e2e_noop() {
        // TODO: добавить тесты когда будет реальное шифрование
    }
}
