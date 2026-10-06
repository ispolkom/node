// src/util/types.rs
//! Common Types
//! =============
//!
//! Core type definitions used across the project

use rand::Rng;
use serde::{Deserialize, Serialize};

/// Universal node identifier - 32-byte hash
/// Used as network address instead of IP
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HashId(pub [u8; 32]);

impl HashId {
    /// Generate a random hash ID
    pub fn new_random() -> Self {
        let mut rng = rand::thread_rng();
        let mut id = [0u8; 32];
        rng.fill(&mut id);
        Self(id)
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Convert from hex string
    pub fn from_hex(hex_str: &str) -> Result<Self, String> {
        let bytes = hex::decode(hex_str)
            .map_err(|e| format!("Invalid hex: {}", e))?;

        if bytes.len() != 32 {
            return Err(format!("Invalid length: {} (expected 32)", bytes.len()));
        }

        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        Ok(Self(id))
    }
}

impl Default for HashId {
    fn default() -> Self {
        Self([0u8; 32])
    }
}

impl AsRef<[u8]> for HashId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Display for HashId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Self-certifying node name = hash of public key
/// This identity CANNOT be forged without possessing the private key
/// Used for all peer verification and DHT operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeName(pub [u8; 32]);

impl NodeName {
    /// Derive node name from public key (self-certifying!)
    /// node_name = SHA256(public_key)
    pub fn from_public_key(public_key: &[u8; 32]) -> Self {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(public_key);
        let result = hasher.finalize();

        let mut name = [0u8; 32];
        name.copy_from_slice(&result[..32]);
        Self(name)
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Convert from hex string
    pub fn from_hex(hex_str: &str) -> Result<Self, String> {
        let bytes = hex::decode(hex_str)
            .map_err(|e| format!("Invalid hex: {}", e))?;

        if bytes.len() != 32 {
            return Err(format!("Invalid length: {} (expected 32)", bytes.len()));
        }

        let mut name = [0u8; 32];
        name.copy_from_slice(&bytes);
        Ok(Self(name))
    }

    /// Get first 8 bytes for short display
    pub fn short(&self) -> String {
        hex::encode(&self.0[..8])
    }

    /// Verify that a public key matches this node name
    pub fn verify_public_key(&self, public_key: &[u8; 32]) -> bool {
        &Self::from_public_key(public_key).0 == &self.0
    }
}

impl Default for NodeName {
    fn default() -> Self {
        Self([0u8; 32])
    }
}

impl AsRef<[u8]> for NodeName {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Display for NodeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short())
    }
}

// Legacy compatibility
impl From<NodeName> for HashId {
    fn from(name: NodeName) -> Self {
        Self(name.0)
    }
}

impl From<HashId> for NodeName {
    fn from(id: HashId) -> Self {
        Self(id.0)
    }
}

// Legacy type alias for compatibility
pub type LegacyHashId = [u8; 8];
