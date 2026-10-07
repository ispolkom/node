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

/// Marker at the END of every bound node id (the start stays random, so the short ids shown and searched by prefix keep telling nodes apart). An id carrying it MUST be derived from the signing key,
/// so nobody can pass a victim's bound id off as an "old random one" (the chance that an old random id starts with it is 2^-32).
pub const BOUND_ID_TAG: [u8; 4] = *b"YB1\0";

/// The node id derived from a signing key: the first 28 bytes of SHA-256(key) + tag.
pub fn derive_node_id(signing_key: &[u8; 32]) -> [u8; 32] {
    let h = NodeName::from_public_key(signing_key).0;
    let mut id = [0u8; 32];
    id[..28].copy_from_slice(&h[..28]);
    id[28..].copy_from_slice(&BOUND_ID_TAG);
    id
}

/// A node id is "bound" when it is exactly the id derived from this signing key: nobody else can claim it.
pub fn id_bound_to_key(node_id: &[u8; 32], signing_key: &[u8; 32]) -> bool {
    &derive_node_id(signing_key) == node_id
}

/// Unix time after which node ids without the tag (made before binding existed) are no longer accepted (2027-04-01 UTC).
/// Until then they are accepted on a first-claim basis (a key is pinned to the id at first sight), so existing nodes keep working.
pub const LEGACY_ID_SUNSET: u64 = 1_806_537_600;

/// Is this (node id, signing key) pair acceptable at time `now`?
/// A tagged id must be bound to the key, always. An untagged (legacy) id is accepted only before the sunset
/// and when `YANDI_REQUIRE_BOUND_IDS` is not set.
pub fn id_acceptable(node_id: &[u8; 32], signing_key: &[u8; 32], now: u64) -> bool {
    if node_id[28..] == BOUND_ID_TAG {
        return id_bound_to_key(node_id, signing_key);
    }
    now < LEGACY_ID_SUNSET && std::env::var_os("YANDI_REQUIRE_BOUND_IDS").is_none()
}

#[cfg(test)]
mod bound_id_tests {
    use super::*;

    #[test]
    fn a_bound_id_belongs_only_to_its_key_and_a_legacy_one_is_accepted_only_before_the_sunset() {
        let key = [7u8; 32];
        let bound = derive_node_id(&key);
        let legacy = [9u8; 32];
        assert_eq!(&bound[28..], &BOUND_ID_TAG);
        assert!(id_bound_to_key(&bound, &key));
        assert!(!id_bound_to_key(&legacy, &key));
        // an attacker with another key cannot claim the victim's bound id, even before the sunset
        assert!(!id_acceptable(&bound, &[8u8; 32], LEGACY_ID_SUNSET - 1));
        assert!(id_acceptable(&bound, &key, LEGACY_ID_SUNSET + 1));
        assert!(id_acceptable(&legacy, &key, LEGACY_ID_SUNSET - 1));
        assert!(!id_acceptable(&legacy, &key, LEGACY_ID_SUNSET));
    }
}

/// serde helper for `HashMap<HashId, T>`: JSON objects need string keys, so the ids are written as hex strings.
/// Use as `#[serde(with = "crate::util::types::hashid_map")]`.
pub mod hashid_map {
    use super::HashId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;

    pub fn serialize<S: Serializer, T: Serialize>(map: &HashMap<HashId, T>, ser: S) -> Result<S::Ok, S::Error> {
        let as_text: std::collections::BTreeMap<String, &T> = map.iter().map(|(k, v)| (hex::encode(k.0), v)).collect();
        as_text.serialize(ser)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(de: D) -> Result<HashMap<HashId, T>, D::Error> {
        let as_text: HashMap<String, T> = HashMap::deserialize(de)?;
        as_text
            .into_iter()
            .map(|(k, v)| HashId::from_hex(&k).map(|id| (id, v)).map_err(serde::de::Error::custom))
            .collect()
    }
}
