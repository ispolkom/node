// src/core/crypto_storage.rs
//! Encrypted Storage for Contacts, Gateways, Settings
//! ==================================================
//!
//! AES-256-GCM encryption with key derived from NodeIdentity

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use anyhow::{Result, Context};

/// Directory for encrypted data
const DATA_DIR: &str = ".data";

/// Encrypted file format: [nonce:12][encrypted_data][tag:16]
const NONCE_SIZE: usize = 12;
const TAG_SIZE: usize = 16;

/// Derive encryption key from node_id (CID)
fn derive_key(node_id: &[u8; 32]) -> [u8; 32] {
    // Use SHA-256 to derive key from node_id
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(node_id);
    hasher.update(b"yandi-storage-key"); // salt
    let result = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

/// Get data directory path
pub fn get_data_dir() -> Result<PathBuf> {
    let mut path = std::env::current_dir()
        .context("Failed to get current directory")?;
    path.push(DATA_DIR);
    Ok(path)
}

/// Ensure data directory exists
pub fn ensure_data_dir() -> Result<PathBuf> {
    let dir = get_data_dir()?;
    fs::create_dir_all(&dir)
        .context("Failed to create data directory")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dir)
            .map_err(|e| anyhow::anyhow!("Failed to get metadata: {}", e))?
            .permissions();
        perms.set_mode(0o700); // owner only
        fs::set_permissions(&dir, perms)
            .map_err(|e| anyhow::anyhow!("Failed to set permissions: {}", e))?;
    }

    Ok(dir)
}

/// Encrypt data using AES-256-GCM
pub fn encrypt_data(plaintext: &[u8], node_id: &[u8; 32]) -> Result<Vec<u8>> {
    let key = derive_key(node_id);
    let cipher = Aes256Gcm::new(&key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher.encrypt(&nonce, plaintext)
        .context("Failed to encrypt data")?;

    // Format: [nonce:12][encrypted_data_with_tag]
    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypt data using AES-256-GCM
pub fn decrypt_data(data: &[u8], node_id: &[u8; 32]) -> Result<Vec<u8>> {
    if data.len() < NONCE_SIZE + TAG_SIZE {
        return Err(anyhow::anyhow!("Encrypted data too short"));
    }

    let key = derive_key(node_id);
    let cipher = Aes256Gcm::new(&key.into());

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    nonce_bytes.copy_from_slice(&data[..NONCE_SIZE]);
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = &data[NONCE_SIZE..];

    let plaintext = cipher.decrypt(&nonce, ciphertext)
        .context("Failed to decrypt data - possibly corrupted or wrong key")?;

    Ok(plaintext)
}

/// Load and decrypt JSON file
pub fn load_encrypted_json<T>(filename: &str, node_id: &[u8; 32]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let dir = ensure_data_dir()?;
    let file_path = dir.join(filename);

    if !file_path.exists() {
        return Err(anyhow::anyhow!("File not found: {:?}", file_path));
    }

    let encrypted = fs::read(&file_path)
        .context("Failed to read encrypted file")?;

    let decrypted = decrypt_data(&encrypted, node_id)
        .context("Failed to decrypt file")?;

    let json_str = String::from_utf8(decrypted)
        .context("Decrypted data is not valid UTF-8")?;

    serde_json::from_str(&json_str)
        .context("Failed to parse JSON")
}

/// Encrypt and save JSON file
pub fn save_encrypted_json<T>(filename: &str, data: &T, node_id: &[u8; 32]) -> Result<()>
where
    T: Serialize,
{
    let dir = ensure_data_dir()?;
    let file_path = dir.join(filename);

    let json_str = serde_json::to_string_pretty(data)
        .context("Failed to serialize to JSON")?;

    let encrypted = encrypt_data(json_str.as_bytes(), node_id)
        .context("Failed to encrypt data")?;

    crate::util::private_file::write_private(&file_path, &encrypted)
        .context("Failed to write encrypted file")?;

    Ok(())
}

/// Check if encrypted file exists
pub fn file_exists(filename: &str) -> bool {
    let dir = match get_data_dir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    let file_path = dir.join(filename);
    file_path.exists()
}

/// Delete encrypted file
pub fn delete_file(filename: &str) -> Result<()> {
    let dir = ensure_data_dir()?;
    let file_path = dir.join(filename);

    if file_path.exists() {
        fs::remove_file(&file_path)
            .context("Failed to delete file")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let node_id = [0u8; 32];
        let plaintext = b"Hello, World!";

        let encrypted = encrypt_data(plaintext, &node_id).unwrap();
        let decrypted = decrypt_data(&encrypted, &node_id).unwrap();

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn test_wrong_key_fails() {
        let node_id1 = [0u8; 32];
        let node_id2 = [1u8; 32];
        let plaintext = b"Secret data";

        let encrypted = encrypt_data(plaintext, &node_id1).unwrap();
        let result = decrypt_data(&encrypted, &node_id2);

        assert!(result.is_err());
    }
}
