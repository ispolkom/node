// src/netlayer/bootstrap.rs
//! Bootstrap Configuration
//! =======================
//!
//! Manages bootstrap node list and auto-connection on startup

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashMap;

/// Bootstrap node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapNode {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub jurisdiction: Option<String>,
    #[serde(default)]
    pub cluster: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub role: Option<String>,  // "entry" or "exit"
    /// SEC-10: Ed25519 public key fingerprint (64 hex chars = 32 bytes).
    /// When set, YANDI will refuse Hello packets from this address that
    /// present a different Ed25519 key, preventing bootstrap node impersonation.
    #[serde(default)]
    pub ed25519_fingerprint: Option<String>,
}

fn default_enabled() -> bool {
    true
}

/// Bootstrap configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    #[serde(default)]
    pub comment: String,

    #[serde(default)]
    pub version: String,

    #[serde(default = "default_auto_connect")]
    pub auto_connect: bool,

    #[serde(default = "default_connect_on_startup")]
    pub connect_on_startup: bool,

    #[serde(default = "default_retry_interval")]
    pub retry_interval_seconds: u64,

    #[serde(default)]
    pub nodes: Vec<BootstrapNode>,
}

fn default_auto_connect() -> bool {
    true
}

fn default_connect_on_startup() -> bool {
    true
}

fn default_retry_interval() -> u64 {
    30
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            comment: String::from("YANDI Bootstrap Configuration"),
            version: String::from("1.0"),
            auto_connect: true,
            connect_on_startup: true,
            retry_interval_seconds: 30,
            nodes: Vec::new(),
        }
    }
}

impl BootstrapConfig {
    /// Load bootstrap configuration from file
    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        println!("[bootstrap] 📂 Loading bootstrap config from: {:?}", path);

        if !path.exists() {
            println!("[bootstrap] ⚠️  Bootstrap config not found, using defaults");
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read bootstrap config: {}", e))?;

        let config: BootstrapConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse bootstrap config: {}", e))?;

        println!("[bootstrap] ✅ Loaded {} bootstrap nodes", config.nodes.len());

        Ok(config)
    }

    /// Get enabled bootstrap nodes
    pub fn get_enabled_nodes(&self) -> Vec<&BootstrapNode> {
        self.nodes.iter()
            .filter(|n| n.enabled)
            .collect()
    }

    /// Build a map of IP address → expected Ed25519 public key for nodes that
    /// have a fingerprint configured. Used by SEC-10 verification.
    pub fn fingerprint_map(&self) -> std::collections::HashMap<String, [u8; 32]> {
        let mut map = std::collections::HashMap::new();
        for node in &self.nodes {
            if !node.enabled { continue; }
            let Some(fp) = &node.ed25519_fingerprint else { continue };
            let fp_clean = fp.trim();
            if fp_clean.len() != 64 { continue; }
            let Ok(bytes) = hex::decode(fp_clean) else { continue };
            if bytes.len() != 32 { continue; }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            // Key is the IP part only (strip port so port variants match)
            let ip = node.address.split(':').next().unwrap_or(&node.address).to_string();
            map.insert(ip, key);
        }
        map
    }

    /// Get nodes by jurisdiction
    pub fn get_nodes_by_jurisdiction(&self, jurisdiction: &str) -> Vec<&BootstrapNode> {
        self.nodes.iter()
            .filter(|n| n.enabled && n.jurisdiction.as_ref().map(|j| j == jurisdiction).unwrap_or(false))
            .collect()
    }

    /// Get nodes by cluster
    pub fn get_nodes_by_cluster(&self, cluster: &str) -> Vec<&BootstrapNode> {
        self.nodes.iter()
            .filter(|n| n.enabled && n.cluster.as_ref().map(|c| c == cluster).unwrap_or(false))
            .collect()
    }

    /// Add node to configuration
    pub fn add_node(&mut self, node: BootstrapNode) {
        self.nodes.push(node);
    }

    /// Save configuration to file
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }

        fs::write(path, content)
            .map_err(|e| format!("Failed to write config: {}", e))?;

        println!("[bootstrap] 💾 Saved bootstrap config to: {:?}", path);
        Ok(())
    }
}

/// Jurisdiction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JurisdictionConfig {
    #[serde(default)]
    pub comment: String,

    pub jurisdiction: String,
    #[serde(default)]
    pub country_name: Option<String>,

    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default)]
    pub nodes: Vec<BootstrapNode>,
}

/// Bootstrap manager
pub struct BootstrapManager {
    config: BootstrapConfig,
    config_path: PathBuf,
}

impl BootstrapManager {
    /// Create new bootstrap manager
    pub fn new(config_path: PathBuf) -> Result<Self, String> {
        let config = BootstrapConfig::load_from_file(&config_path)?;

        Ok(Self {
            config,
            config_path,
        })
    }

    /// Get bootstrap configuration
    pub fn config(&self) -> &BootstrapConfig {
        &self.config
    }

    /// Get enabled bootstrap nodes
    pub fn get_bootstrap_nodes(&self) -> Vec<String> {
        self.config.get_enabled_nodes()
            .iter()
            .map(|n| n.address.clone())
            .collect()
    }

    /// Add discovered peer to bootstrap list
    pub fn add_discovered_peer(&mut self, addr: String, name: Option<String>) -> Result<(), String> {
        let node = BootstrapNode {
            name: name.unwrap_or_else(|| format!("discovered-{}", addr.replace(":", "-"))),
            address: addr,
            jurisdiction: None,
            cluster: None,
            enabled: true,
            role: None,
            ed25519_fingerprint: None,
        };

        self.config.add_node(node);
        self.config.save_to_file(&self.config_path)?;

        Ok(())
    }

    /// Reload configuration from file
    pub fn reload(&mut self) -> Result<(), String> {
        self.config = BootstrapConfig::load_from_file(&self.config_path)?;
        println!("[bootstrap] 🔄 Configuration reloaded");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bootstrap_config_default() {
        let config = BootstrapConfig::default();
        assert!(config.auto_connect);
        assert!(config.connect_on_startup);
        assert_eq!(config.nodes.len(), 0);
    }

    #[test]
    fn test_get_enabled_nodes() {
        let mut config = BootstrapConfig::default();
        config.nodes.push(BootstrapNode {
            name: "node1".to_string(),
            address: "127.0.0.1:9000".to_string(),
            jurisdiction: None,
            cluster: None,
            enabled: true,
            role: None,
            ed25519_fingerprint: None,
        });
        config.nodes.push(BootstrapNode {
            name: "node2".to_string(),
            address: "127.0.0.1:9001".to_string(),
            jurisdiction: None,
            cluster: None,
            enabled: false,
            role: None,
            ed25519_fingerprint: None,
        });

        let enabled = config.get_enabled_nodes();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "node1");
    }
}
