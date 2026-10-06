// src/communication/profile_manager.rs
//! Profile Manager - P2P exchange of profiles and avatars
//! =========================================================
//!
//! Manages peer profiles with avatars, requests profiles from peers,
//! caches avatars locally with CID binding.

use crate::util::HashId;
use crate::communication::types::{ProfileRequest, ProfileData};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Avatar storage directory
const AVATARS_DIR: &str = ".data/avatars";

/// Profile cache entry
#[derive(Debug, Clone)]
pub struct CachedProfile {
    pub short_id: String,
    pub display_name: String,
    pub avatar_hash: String,
    pub avatar_path: Option<PathBuf>,  // Local path to cached avatar
    pub updated_at: u64,
}

/// Profile Manager - handles P2P profile exchange
pub struct ProfileManager {
    /// Cache of peer profiles (short_id -> profile)
    profiles: Arc<RwLock<HashMap<String, CachedProfile>>>,
    /// Our node's short_id
    my_short_id: String,
    /// Avatar storage directory
    avatars_dir: PathBuf,
}

impl ProfileManager {
    /// Create new profile manager
    pub fn new(my_short_id: String) -> Result<Self, String> {
        let home = dirs::home_dir()
            .ok_or("No home directory")?;
        let avatars_dir = home.join(".yandi").join(AVATARS_DIR);

        // Create avatars directory
        std::fs::create_dir_all(&avatars_dir)
            .map_err(|e| format!("Failed to create avatars dir: {}", e))?;

        Ok(Self {
            profiles: Arc::new(RwLock::new(HashMap::new())),
            my_short_id,
            avatars_dir,
        })
    }

    /// Get our profile data for sending to peers
    pub fn get_my_profile(&self) -> Result<ProfileData, String> {
        use crate::core::profile::UserProfile;

        let profile = UserProfile::load()
            .map_err(|e| format!("Failed to load profile: {}", e))?
            .ok_or("No profile found")?;

        // Load avatar data if exists
        let avatar_path = self.avatars_dir.join(format!("{}.jpg", self.my_short_id));
        let avatar_data = if avatar_path.exists() {
            std::fs::read(&avatar_path)
                .map_err(|e| format!("Failed to read avatar: {}", e))?
        } else {
            Vec::new()
        };

        let avatar_hash = profile.avatar_hash
            .unwrap_or_else(|| {
                crate::core::image_processor::get_avatar_hash(&avatar_data)
            });

        Ok(ProfileData::from_bytes(
            self.my_short_id.clone(),
            profile.display_name,
            avatar_hash,
            avatar_data,
        ))
    }

    /// Handle incoming profile request from peer
    pub async fn handle_profile_request(&self, _from: &HashId, _request: ProfileRequest) -> Result<ProfileData, String> {
        self.get_my_profile()
    }

    /// Handle incoming profile data from peer
    pub async fn handle_profile_data(&self, from_short_id: String, data: ProfileData) -> Result<(), String> {
        // Validate avatar data
        if !data.is_avatar_valid() {
            return Err("Invalid avatar data".to_string());
        }

        // Save avatar locally
        let avatar_path = self.avatars_dir.join(format!("{}.jpg", from_short_id));

        // Only update if hash is different (newer avatar)
        let should_update = {
            let cache = self.profiles.read().await;
            match cache.get(&from_short_id) {
                None => true,
                Some(cached) => cached.avatar_hash != data.avatar_hash,
            }
        };

        if should_update {
            // Save avatar file (decode from base64)
            let avatar_bytes = data.avatar_bytes()
                .map_err(|e| format!("Failed to decode avatar: {}", e))?;
            std::fs::write(&avatar_path, &avatar_bytes)
                .map_err(|e| format!("Failed to save avatar: {}", e))?;

            // Update cache
            let mut cache = self.profiles.write().await;
            cache.insert(from_short_id.clone(), CachedProfile {
                short_id: from_short_id.clone(),
                display_name: data.display_name.clone(),
                avatar_hash: data.avatar_hash.clone(),
                avatar_path: Some(avatar_path),
                updated_at: data.timestamp,
            });

            tracing::info!("✅ Profile cached for {} (name: {}, avatar: {}KB)",
                from_short_id,
                data.display_name,
                data.avatar_size() / 1024
            );
        }

        Ok(())
    }

    /// Get cached profile for peer
    pub async fn get_profile(&self, short_id: &str) -> Option<CachedProfile> {
        self.profiles.read().await.get(short_id).cloned()
    }

    /// Check if avatar is cached for peer
    pub async fn has_avatar(&self, short_id: &str) -> bool {
        if let Some(profile) = self.get_profile(short_id).await {
            profile.avatar_path.is_some()
        } else {
            false
        }
    }

    /// Get avatar URL for peer (for frontend)
    pub async fn get_avatar_url(&self, short_id: &str) -> Option<String> {
        if self.has_avatar(short_id).await {
            Some(format!("/api/profile/avatar/peer/{}", short_id))
        } else {
            None
        }
    }

    /// Delete cached profile for peer
    pub async fn delete_profile(&self, short_id: &str) -> Result<(), String> {
        let mut cache = self.profiles.write().await;
        if let Some(profile) = cache.remove(short_id) {
            if let Some(path) = profile.avatar_path {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_data_validation() {
        let data = ProfileData {
            short_id: "abcd1234".to_string(),
            display_name: "Test".to_string(),
            avatar_hash: "hash".to_string(),
            avatar_data: vec![0u8; 50_000], // 50KB
            timestamp: 0,
        };

        assert!(data.is_avatar_valid());
        assert_eq!(data.avatar_size(), 50_000);
    }
}
