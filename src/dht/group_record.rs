//! Signed group records for DHT

use serde::{Serialize, Deserialize};
use crate::util::HashId;
use crate::core::NodeIdentity;
use crate::communication::groups::group::{Group, GroupId};

/// Signed group record stored in DHT
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedGroupRecord {
    pub group_id: GroupId,
    pub group_data: Vec<u8>,
    pub public_key: [u8; 32],
    pub timestamp: u64,
    pub sequence: u64,
    pub signature: Vec<u8>,
}

impl SignedGroupRecord {
    pub fn new(group: &Group, identity: &NodeIdentity, sequence: u64) -> Result<Self, String> {
        let group_data = serde_json::to_vec(group)
            .map_err(|e| format!("Serialize error: {}", e))?;
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let mut payload = Vec::new();
        payload.extend_from_slice(&group.id.0);
        payload.extend_from_slice(&group_data);
        payload.extend_from_slice(&identity.signing_public_key);
        payload.extend_from_slice(&timestamp.to_be_bytes());
        payload.extend_from_slice(&sequence.to_be_bytes());
        
        let signature = identity.sign(&payload)?;
        
        Ok(Self {
            group_id: group.id,
            group_data,
            public_key: identity.signing_public_key,
            timestamp,
            sequence,
            signature: signature.to_vec(),
        })
    }
    
    pub fn verify(&self) -> bool {
        if self.signature.len() != 64 {
            return false;
        }
        
        let mut payload = Vec::new();
        payload.extend_from_slice(&self.group_id.0);
        payload.extend_from_slice(&self.group_data);
        payload.extend_from_slice(&self.public_key);
        payload.extend_from_slice(&self.timestamp.to_be_bytes());
        payload.extend_from_slice(&self.sequence.to_be_bytes());
        
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&self.signature[..64]);
        
        NodeIdentity::verify_raw(&self.public_key, &sig_bytes, &payload)
    }
    
    pub fn get_group(&self) -> Result<Group, String> {
        serde_json::from_slice(&self.group_data)
            .map_err(|e| format!("Deserialize error: {}", e))
    }
}