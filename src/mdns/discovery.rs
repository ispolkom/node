// src/mdns/discovery.rs
//!
//! # mDNS Service Discovery
//!
//! Announces and discovers YANDI nodes on the local network using mDNS/Bonjour.
//! Nodes are announced as `<short-id>.local` for zero-configuration access.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, error, debug};

/// mDNS service type for YANDI nodes
pub const YANDI_SERVICE_TYPE: &str = "_yandi._tcp.local.";

/// mDNS service type for HTTP admin interface
pub const YANDI_ADMIN_TYPE: &str = "_yandi-admin._tcp.local.";

/// Information about a discovered YANDI node
#[derive(Debug, Clone)]
pub struct DiscoveredNode {
    /// Node short ID (e.g., "8283219e")
    pub short_id: String,
    /// Full mDNS hostname (e.g., "8283219e.local")
    pub hostname: String,
    /// Admin interface port (usually 8080)
    pub admin_port: u16,
    /// P2P discovery port (usually 9000)
    pub discovery_port: u16,
    /// P2P data port (usually 10000)
    pub data_port: u16,
    /// Node role (Border, Gateway, Relay, etc.)
    pub role: String,
    /// Node capabilities
    pub capabilities: String,
}

/// mDNS Service Announcer
///
/// Announces this node on the local network via mDNS.
pub struct MdnsAnnouncer {
    daemon: Arc<ServiceDaemon>,
    short_id: String,
    admin_port: u16,
    discovery_port: u16,
    data_port: u16,
    role: String,
}

impl MdnsAnnouncer {
    /// Create a new mDNS announcer
    pub fn new(
        short_id: String,
        admin_port: u16,
        discovery_port: u16,
        data_port: u16,
        role: String,
    ) -> Result<Self, String> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| format!("Failed to create mDNS daemon: {}", e))?;

        Ok(Self {
            daemon: Arc::new(daemon),
            short_id,
            admin_port,
            discovery_port,
            data_port,
            role,
        })
    }

    /// Start announcing this node on mDNS
    pub fn start(&self) -> Result<(), String> {
        // 1. Announce admin interface as <short-id>.local.
        let admin_hostname = format!("{}.local.", self.short_id);
        let admin_service = self.create_admin_service(&admin_hostname)?;

        self.daemon.register(admin_service)
            .map_err(|e| format!("Failed to register admin service: {}", e))?;

        info!("📡 mDNS: Announcing {} → port {} (admin)", admin_hostname, self.admin_port);

        // 2. Announce fixed yandi.local domain for local web UI
        let yandi_local_service = self.create_yandi_local_service()?;
        self.daemon.register(yandi_local_service)
            .map_err(|e| format!("Failed to register yandi.local service: {}", e))?;

        info!("📡 mDNS: Announcing yandi.local → port {} (local web UI)", self.admin_port);

        // 3. Announce P2P service for node discovery
        let p2p_service = self.create_p2p_service(&admin_hostname)?;

        self.daemon.register(p2p_service)
            .map_err(|e| format!("Failed to register P2P service: {}", e))?;

        info!("📡 mDNS: Announcing P2P service (discovery:{}, data:{})",
              self.discovery_port, self.data_port);

        Ok(())
    }

    /// Create admin interface service info
    fn create_admin_service(&self, hostname: &str) -> Result<ServiceInfo, String> {
        use std::collections::HashMap;

        let mut properties = HashMap::new();
        properties.insert("role".to_string(), self.role.clone());
        properties.insert("admin_port".to_string(), self.admin_port.to_string());
        properties.insert("discovery_port".to_string(), self.discovery_port.to_string());
        properties.insert("data_port".to_string(), self.data_port.to_string());

        ServiceInfo::new(
            YANDI_ADMIN_TYPE,
            &self.short_id,
            hostname,
            "", // addresses (empty for mDNS)
            self.admin_port,
            properties,
        ).map_err(|e| format!("Failed to create admin service info: {}", e))
    }

    /// Create P2P service info
    fn create_p2p_service(&self, hostname: &str) -> Result<ServiceInfo, String> {
        use std::collections::HashMap;

        let mut properties = HashMap::new();
        properties.insert("role".to_string(), self.role.clone());
        properties.insert("discovery_port".to_string(), self.discovery_port.to_string());
        properties.insert("data_port".to_string(), self.data_port.to_string());

        ServiceInfo::new(
            YANDI_SERVICE_TYPE,
            &self.short_id,
            hostname,
            "", // addresses (empty for mDNS)
            self.discovery_port, // P2P service announces discovery port
            properties,
        ).map_err(|e| format!("Failed to create P2P service info: {}", e))
    }

    /// Create fixed yandi.local service info
    fn create_yandi_local_service(&self) -> Result<ServiceInfo, String> {
        use std::collections::HashMap;

        let mut properties = HashMap::new();
        properties.insert("role".to_string(), self.role.clone());
        properties.insert("short_id".to_string(), self.short_id.clone());
        properties.insert("admin_port".to_string(), self.admin_port.to_string());
        properties.insert("discovery_port".to_string(), self.discovery_port.to_string());
        properties.insert("data_port".to_string(), self.data_port.to_string());

        ServiceInfo::new(
            YANDI_ADMIN_TYPE,
            "yandi",  // instance name
            "yandi.local.",  // fixed hostname
            "",  // addresses (empty for mDNS)
            self.admin_port,  // web UI port
            properties,
        ).map_err(|e| format!("Failed to create yandi.local service info: {}", e))
    }

    /// Get the daemon handle for later shutdown
    pub fn daemon(&self) -> Arc<ServiceDaemon> {
        Arc::clone(&self.daemon)
    }
}

/// mDNS Service Browser
///
/// Discovers other YANDI nodes on the local network.
pub struct MdnsBrowser {
    daemon: Arc<ServiceDaemon>,
    discovered: Arc<Mutex<HashMap<String, DiscoveredNode>>>,
}

impl MdnsBrowser {
    /// Create a new mDNS browser
    pub fn new() -> Result<Self, String> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| format!("Failed to create mDNS daemon: {}", e))?;

        Ok(Self {
            daemon: Arc::new(daemon),
            discovered: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Start browsing for YANDI nodes
    pub async fn start(&self) -> Result<(), String> {
        let discovered = Arc::clone(&self.discovered);

        // Browse for admin services
        let admin_receiver = self.daemon.browse(YANDI_ADMIN_TYPE)
            .map_err(|e| format!("Failed to browse admin services: {}", e))?;

        let discovered_admin = Arc::clone(&discovered);
        tokio::spawn(async move {
            Self::handle_events(admin_receiver, discovered_admin, "admin").await;
        });

        // Browse for P2P services
        let p2p_receiver = self.daemon.browse(YANDI_SERVICE_TYPE)
            .map_err(|e| format!("Failed to browse P2P services: {}", e))?;

        let discovered_p2p = Arc::clone(&discovered);
        tokio::spawn(async move {
            Self::handle_events(p2p_receiver, discovered_p2p, "p2p").await;
        });

        info!("🔍 mDNS: Browsing for YANDI nodes...");

        Ok(())
    }

    /// Handle mDNS events
    async fn handle_events(
        mut receiver: mdns_sd::Receiver<ServiceEvent>,
        discovered: Arc<Mutex<HashMap<String, DiscoveredNode>>>,
        service_type: &'static str,
    ) {
        while let Ok(event) = receiver.recv_async().await {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    debug!("🔍 mDNS: Resolved {} service: {:?}", service_type, info.get_hostname());

                    let node_info = Self::parse_service_info(&info);
                    if let Some(si) = node_info {
                        info!("✅ mDNS: Found node {} ({})", si.short_id, si.role);
                        let mut map = discovered.lock().await;
                        map.insert(si.short_id.clone(), si);
                    }
                }
                ServiceEvent::ServiceRemoved(_type, full_name) => {
                    debug!("🔍 mDNS: Service removed: {}", full_name);
                    // Extract short ID from full name
                    let short_id = full_name.split('.').next().unwrap_or("");
                    let mut map = discovered.lock().await;
                    map.remove(short_id);
                }
                ServiceEvent::ServiceFound(_type, full_name) => {
                    debug!("🔍 mDNS: Service found: {}", full_name);
                }
                _ => {}
            }
        }
    }

    /// Parse service info from mDNS response
    fn parse_service_info(info: &ServiceInfo) -> Option<DiscoveredNode> {
        let hostname = info.get_hostname();
        let short_id = hostname.trim_end_matches(".local");

        let admin_port = info.get_port();

        // Parse discovery_port from property (returns Option<Option<&[u8]>>)
        let discovery_port = info.get_property_val("discovery_port")
            .and_then(|p| p) // flatten Option<Option<&[u8]>>
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(9000);

        // Parse data_port from property
        let data_port = info.get_property_val("data_port")
            .and_then(|p| p)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(10000);

        // Parse role from property
        let role = info.get_property_val("role")
            .and_then(|p| p)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        Some(DiscoveredNode {
            short_id: short_id.to_string(),
            hostname: hostname.to_string(),
            admin_port,
            discovery_port,
            data_port,
            role,
            capabilities: String::new(), // TODO: parse from properties
        })
    }

    /// Get all discovered nodes
    pub async fn get_discovered(&self) -> Vec<DiscoveredNode> {
        let map = self.discovered.lock().await;
        map.values().cloned().collect()
    }

    /// Get specific node by short ID
    pub async fn get_node(&self, short_id: &str) -> Option<DiscoveredNode> {
        let map = self.discovered.lock().await;
        map.get(short_id).cloned()
    }

    /// Get the daemon handle for later shutdown
    pub fn daemon(&self) -> Arc<ServiceDaemon> {
        Arc::clone(&self.daemon)
    }
}

/// Unified mDNS service (announcer + browser)
///
/// Combines announcing and browsing in one convenient struct.
pub struct MdnsService {
    announcer: Option<MdnsAnnouncer>,
    browser: Option<MdnsBrowser>,
}

impl MdnsService {
    /// Create a new mDNS service
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            announcer: None,
            browser: None,
        })
    }

    /// Enable announcing this node
    pub fn with_announcer(
        mut self,
        short_id: String,
        admin_port: u16,
        discovery_port: u16,
        data_port: u16,
        role: String,
    ) -> Result<Self, String> {
        self.announcer = Some(MdnsAnnouncer::new(
            short_id,
            admin_port,
            discovery_port,
            data_port,
            role,
        )?);
        Ok(self)
    }

    /// Enable browsing for other nodes
    pub fn with_browser(mut self) -> Result<Self, String> {
        self.browser = Some(MdnsBrowser::new()?);
        Ok(self)
    }

    /// Start the mDNS service
    pub async fn start(&mut self) -> Result<(), String> {
        if let Some(announcer) = &self.announcer {
            announcer.start()?;
        }

        if let Some(browser) = &self.browser {
            browser.start().await?;
        }

        Ok(())
    }

    /// Get discovered nodes (if browser enabled)
    pub async fn get_discovered(&self) -> Vec<DiscoveredNode> {
        if let Some(browser) = &self.browser {
            browser.get_discovered().await
        } else {
            Vec::new()
        }
    }

    /// Get specific node (if browser enabled)
    pub async fn get_node(&self, short_id: &str) -> Option<DiscoveredNode> {
        if let Some(browser) = &self.browser {
            browser.get_node(short_id).await
        } else {
            None
        }
    }

    /// Shutdown the mDNS service
    pub fn shutdown(&self) -> Result<(), String> {
        if let Some(announcer) = &self.announcer {
            announcer.daemon().shutdown()
                .map_err(|e| format!("Failed to shutdown announcer: {}", e))?;
        }

        if let Some(browser) = &self.browser {
            browser.daemon().shutdown()
                .map_err(|e| format!("Failed to shutdown browser: {}", e))?;
        }

        Ok(())
    }
}
