// src/dht/record.rs
//! Signed Node Records for DHT
//! ============================
//!
//! Self-certifying node records with cryptographic signatures

use serde::{Serialize, Deserialize};
use crate::util::NodeName;
use crate::core::NodeIdentity;

/// TTL settings for NodeRecords
pub const NODE_RECORD_TTL_SOFT: u64 = 60 * 60;      // 1 hour - soft refresh
pub const NODE_RECORD_TTL_HARD: u64 = 24 * 60 * 60;  // 24 hours - hard expiration

/// Signed node record stored in DHT
/// This proves that a node owns its identity without centralized authority
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRecord {
    /// Self-certifying node name = SHA256(public_key)
    pub node_name: NodeName,

    /// Ed25519 public key for verification
    pub public_key: [u8; 32],

    /// Record creation timestamp
    pub timestamp: u64,

    /// Sequence number for replay protection
    /// Higher sequences invalidate older ones
    pub sequence: u64,

    /// Signature of (node_name + public_key + timestamp + sequence)
    pub signature: Vec<u8>,

    /// Optional endpoint information
    pub endpoint: Option<String>,

    /// Node capabilities (from Hello packet)
    pub capabilities: u16,
}

impl NodeRecord {
    /// Create a new NodeRecord from identity
    pub fn new(
        identity: &NodeIdentity,
        sequence: u64,
        endpoint: Option<String>,
        capabilities: u16,
    ) -> Result<Self, String> {
        let node_name = identity.node_name();
        let public_key = identity.signing_public_key;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Create signature payload
        let payload = Self::create_signature_payload(&node_name, &public_key, timestamp, sequence);
        let signature = identity.sign(&payload)?;

        let signature_vec = signature.to_vec();

        Ok(Self {
            node_name,
            public_key,
            timestamp,
            sequence,
            signature: signature_vec,
            endpoint,
            capabilities,
        })
    }

    /// Create payload for signature
    pub fn create_signature_payload(node_name: &NodeName, public_key: &[u8; 32], timestamp: u64, sequence: u64) -> Vec<u8> {
        let mut payload = Vec::with_capacity(32 + 32 + 8 + 8);
        payload.extend_from_slice(&node_name.0);
        payload.extend_from_slice(public_key);
        payload.extend_from_slice(&timestamp.to_be_bytes());
        payload.extend_from_slice(&sequence.to_be_bytes());
        payload
    }

    /// Verify the signature on this NodeRecord
    pub fn verify(&self) -> bool {
        // Step 1: Verify self-certifying identity
        if !self.node_name.verify_public_key(&self.public_key) {
            return false;
        }

        // Step 2: Verify signature
        if self.signature.len() != 64 {
            return false;
        }
        let payload = Self::create_signature_payload(&self.node_name, &self.public_key, self.timestamp, self.sequence);
        
        // Convert Vec<u8> to [u8; 64] for verification
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&self.signature[..64]);
        NodeIdentity::verify_node(&self.node_name, &self.public_key, &sig_bytes, &payload)
    }

    /// Check if this record is newer than another (by sequence number)
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.sequence > other.sequence
    }

    /// Check if this record is expired (hard TTL)
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        now.saturating_sub(self.timestamp) > NODE_RECORD_TTL_HARD
    }

    /// Check if this record needs refresh (soft TTL)
    pub fn needs_refresh(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        now.saturating_sub(self.timestamp) > NODE_RECORD_TTL_SOFT
    }

    /// Get remaining time before hard expiration (in seconds)
    pub fn time_until_expiration(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let age = now.saturating_sub(self.timestamp);
        NODE_RECORD_TTL_HARD.saturating_sub(age)
    }

    /// Serialize to bytes for DHT storage
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        bincode::serialize(self)
            .map_err(|e| format!("Failed to serialize NodeRecord: {}", e))
    }

    /// Deserialize from bytes from DHT storage
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        bincode::deserialize(data)
            .map_err(|e| format!("Failed to deserialize NodeRecord: {}", e))
    }

    /// Get node name as hex string (for logging)
    pub fn node_name_hex(&self) -> String {
        self.node_name.to_hex()
    }

    /// Get short node name (first 8 bytes)
    pub fn node_name_short(&self) -> String {
        self.node_name.short()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NodeIdentity;

    #[test]
    fn test_node_record_signature() {
        // Create a test identity
        let identity = NodeIdentity::new();
        
        // Create a NodeRecord
        let sequence = 1;
        let endpoint = Some("192.168.1.1:8080".to_string());
        let capabilities = 0x0001;
        
        let record = NodeRecord::new(&identity, sequence, endpoint.clone(), capabilities)
            .expect("Failed to create NodeRecord");
        
        // Verify the record is valid
        assert!(record.verify(), "NodeRecord signature verification failed");
        
        // Verify node name matches
        let expected_node_name = identity.node_name();
        assert_eq!(record.node_name, expected_node_name, "Node name mismatch");
        
        // Verify public key matches
        assert_eq!(record.public_key, identity.signing_public_key, "Public key mismatch");
        
        // Verify endpoint is stored correctly
        assert_eq!(record.endpoint, endpoint, "Endpoint mismatch");
        
        // Verify capabilities are stored
        assert_eq!(record.capabilities, capabilities, "Capabilities mismatch");
        
        // Verify sequence number
        assert_eq!(record.sequence, sequence, "Sequence mismatch");
        
        // Test timestamp is recent (within last 5 seconds)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(now - record.timestamp <= 5, "Timestamp is not recent");
    }
    
    #[test]
    fn test_node_record_newer_than() {
        let identity = NodeIdentity::new();
        
        let older = NodeRecord::new(&identity, 1, None, 0).unwrap();
        let newer = NodeRecord::new(&identity, 2, None, 0).unwrap();
        
        assert!(newer.is_newer_than(&older), "Newer record should be newer");
        assert!(!older.is_newer_than(&newer), "Older record should not be newer");
    }
    
    #[test]
    fn test_node_record_expiration() {
        let identity = NodeIdentity::new();
        let record = NodeRecord::new(&identity, 1, None, 0).unwrap();
        
        // New record should not be expired
        assert!(!record.is_expired(), "New record should not be expired");
        assert!(!record.needs_refresh(), "New record should not need refresh");
        
        // Time to expiration should be close to HARD TTL
        assert!(record.time_until_expiration() <= NODE_RECORD_TTL_HARD);
        assert!(record.time_until_expiration() > NODE_RECORD_TTL_HARD - 10);
    }
    
    #[test]
    fn test_node_record_serialization() {
        let identity = NodeIdentity::new();
        let original = NodeRecord::new(&identity, 42, Some("10.0.0.1:9000".to_string()), 0xFFFF).unwrap();
        
        // Serialize
        let bytes = original.to_bytes().expect("Serialization failed");
        
        // Deserialize
        let deserialized = NodeRecord::from_bytes(&bytes).expect("Deserialization failed");
        
        // Verify all fields match
        assert_eq!(deserialized.node_name, original.node_name);
        assert_eq!(deserialized.public_key, original.public_key);
        assert_eq!(deserialized.timestamp, original.timestamp);
        assert_eq!(deserialized.sequence, original.sequence);
        assert_eq!(deserialized.signature, original.signature);
        assert_eq!(deserialized.endpoint, original.endpoint);
        assert_eq!(deserialized.capabilities, original.capabilities);
        
        // Verify signature still valid
        assert!(deserialized.verify(), "Deserialized record signature invalid");
    }
    
    #[test]
    fn test_node_record_tampered() {
        let identity = NodeIdentity::new();
        let mut record = NodeRecord::new(&identity, 1, Some("192.168.1.1:8080".to_string()), 0).unwrap();
        
        // Tamper with the sequence number
        record.sequence = 999;
        
        // Verification should fail because signature no longer matches
        assert!(!record.verify(), "Tampered record should fail verification");
        
        // Restore and tamper with timestamp
        let mut record = NodeRecord::new(&identity, 1, Some("192.168.1.1:8080".to_string()), 0).unwrap();
        record.timestamp = 0;
        
        assert!(!record.verify(), "Tampered timestamp should fail verification");
        
        // Tamper with public key
        let mut record = NodeRecord::new(&identity, 1, Some("192.168.1.1:8080".to_string()), 0).unwrap();
        record.public_key = [0u8; 32];
        
        assert!(!record.verify(), "Tampered public key should fail verification");
    }
}
