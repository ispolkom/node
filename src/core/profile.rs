// src/core/profile.rs
//! User profile management (avatar, display name)

use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use anyhow::Result;

/// User profile information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// Display name (user chosen name, not short_id)
    pub display_name: String,
    /// Avatar as base64 string (optional)
    pub avatar: Option<String>,
    /// Last update timestamp
    pub updated_at: u64,
    /// Short ID of the node (read-only)
    pub short_id: String,
}

impl UserProfile {
    /// Create new profile with default values
    pub fn new(short_id: String) -> Self {
        Self {
            display_name: short_id.clone(),
            avatar: None,
            updated_at: current_timestamp(),
            short_id,
        }
    }

    /// Get profile file path
    fn profile_path() -> PathBuf {
        dirs::home_dir()
            .expect("No home directory")
            .join(".yandi/profile.json")
    }

    /// Load profile from file
    pub fn load() -> Result<Option<Self>> {
        let path = Self::profile_path();
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Save profile to file
    pub fn save(&self) -> Result<()> {
        let path = Self::profile_path();
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Update profile with new values
    pub fn update(&mut self, display_name: Option<String>, avatar: Option<String>) {
        if let Some(name) = display_name {
            if !name.is_empty() {
                self.display_name = name;
            }
        }
        if let Some(avatar_data) = avatar {
            self.avatar = Some(avatar_data);
        }
        self.updated_at = current_timestamp();
    }
}

/// Get current timestamp in seconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
