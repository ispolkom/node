// src/core/crypto.rs
//! Cryptographic Utilities
//! ========================
//!
//! SHA-256 hashing

use sha2::{Sha256, Digest};

pub fn hash(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    let result = h.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&result[..32]);
    out
}
