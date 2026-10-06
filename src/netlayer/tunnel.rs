// src/netlayer/tunnel.rs
//! Tunnel Management
//! ==================
//!
//! Manages encrypted tunnels on port 10000 with:
//! - Heartbeat/keepalive monitoring
//! - Automatic tunnel restoration
//! - Session timeout detection

use crate::util::HashId;
use crate::netlayer::peer::PeerInfo;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Tunnel timeout configuration
#[derive(Clone, Debug)]
pub struct TunnelTimeout {
    /// Heartbeat interval (how often to send ping)
    pub heartbeat_interval: Duration,

    /// Session timeout (close tunnel if no packets for this duration)
    pub session_timeout: Duration,

    /// Reconnect delay (wait before reconnecting)
    pub reconnect_delay: Duration,
}

impl Default for TunnelTimeout {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(5),  // Ping every 5 seconds
            session_timeout: Duration::from_secs(300),   // Close tunnel after 5 minutes silence
            reconnect_delay: Duration::from_millis(500), // Wait 500ms before reconnect
        }
    }
}

/// Tunnel status
#[derive(Debug, Clone, PartialEq)]
pub enum TunnelStatus {
    /// Tunnel is active (receiving packets)
    Active,
    /// Tunnel is stale (no packets recently, but not timed out)
    Stale,
    /// Tunnel is closed (timed out)
    Closed,
}

/// Tunnel information
#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub peer_id: HashId,
    pub status: TunnelStatus,
    pub last_activity: Instant,
    pub packets_received: u64,
    pub packets_sent: u64,
}

impl TunnelInfo {
    pub fn new(peer_id: HashId) -> Self {
        Self {
            peer_id,
            status: TunnelStatus::Active,
            last_activity: Instant::now(),
            packets_received: 0,
            packets_sent: 0,
        }
    }

    pub fn is_expired(&self, timeout: Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    pub fn update_activity(&mut self) {
        self.last_activity = Instant::now();
        self.packets_received += 1;
        self.status = TunnelStatus::Active;
    }

    pub fn record_sent(&mut self) {
        self.packets_sent += 1;
    }
}

/// Tunnel manager
///
/// Manages encrypted tunnels on port 10000:
/// - Tracks last activity per peer
/// - Detects stale/closed tunnels
/// - Triggers reconnection via Hello exchange
pub struct TunnelManager {
    tunnels: HashMap<HashId, TunnelInfo>,
    timeout_config: TunnelTimeout,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            tunnels: HashMap::new(),
            timeout_config: TunnelTimeout::default(),
        }
    }

    pub fn with_timeout(timeout_config: TunnelTimeout) -> Self {
        Self {
            tunnels: HashMap::new(),
            timeout_config,
        }
    }

    /// Register tunnel for peer (after Hello exchange)
    pub fn register_tunnel(&mut self, peer_id: HashId) {
        println!("[tunnel] 🔓 Tunnel registered for peer: {}",
                 hex::encode(&peer_id.0[..8]));
        self.tunnels.insert(peer_id, TunnelInfo::new(peer_id));
    }

    /// Update tunnel activity (called when encrypted packet received)
    pub fn update_activity(&mut self, peer_id: &HashId) {
        if let Some(tunnel) = self.tunnels.get_mut(peer_id) {
            tunnel.update_activity();
        }
    }

    /// Check if tunnel is active for peer
    pub fn is_tunnel_active(&self, peer_id: &HashId) -> bool {
        self.tunnels
            .get(peer_id)
            .map(|t| !t.is_expired(self.timeout_config.session_timeout))
            .unwrap_or(false)
    }

    /// Get tunnel info
    pub fn get_tunnel(&self, peer_id: &HashId) -> Option<&TunnelInfo> {
        self.tunnels.get(peer_id)
    }

    /// Close tunnel for peer
    pub fn close_tunnel(&mut self, peer_id: &HashId) {
        if let Some(mut tunnel) = self.tunnels.remove(peer_id) {
            tunnel.status = TunnelStatus::Closed;
            println!("[tunnel] 🔒 Tunnel closed for peer: {}",
                     hex::encode(&peer_id.0[..8]));
        }
    }

    /// Check for expired tunnels and return list of peer IDs
    pub fn check_expired_tunnels(&mut self) -> Vec<HashId> {
        let mut expired = Vec::new();

        for (peer_id, tunnel) in self.tunnels.iter_mut() {
            if tunnel.is_expired(self.timeout_config.session_timeout) {
                println!("[tunnel] ⚠️  Tunnel expired for peer: {} (idle for {:?})",
                         hex::encode(&peer_id.0[..8]),
                         tunnel.last_activity.elapsed());
                tunnel.status = TunnelStatus::Closed;
                expired.push(*peer_id);
            }
        }

        // Remove expired tunnels
        for peer_id in &expired {
            self.tunnels.remove(peer_id);
        }

        expired
    }

    /// Get all active tunnels
    pub fn get_active_tunnels(&self) -> Vec<&TunnelInfo> {
        self.tunnels
            .values()
            .filter(|t| !t.is_expired(self.timeout_config.session_timeout))
            .collect()
    }

    /// Get tunnel count
    pub fn tunnel_count(&self) -> usize {
        self.tunnels.len()
    }
}

/// Background task to monitor tunnels
pub async fn tunnel_monitor_task(
    manager: Arc<Mutex<TunnelManager>>,
    on_tunnel_expired: tokio::sync::mpsc::Sender<HashId>,
) {
    println!("[tunnel] 🔍 Tunnel monitor started");

    let mut interval = tokio::time::interval(Duration::from_millis(500));

    loop {
        interval.tick().await;

        let expired = {
            let mut manager = manager.lock().await;
            manager.check_expired_tunnels()
        };

        // Notify about expired tunnels
        for peer_id in expired {
            println!("[tunnel] 🔄 Notifying tunnel expired: {}",
                     hex::encode(&peer_id.0[..8]));
            if let Err(e) = on_tunnel_expired.send(peer_id).await {
                println!("[tunnel] ❌ Failed to send tunnel expiration notification: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_tunnel_expiry() {
        let mut manager = TunnelManager::new();

        // Create tunnel with very short timeout
        manager.timeout_config = TunnelTimeout {
            heartbeat_interval: Duration::from_millis(100),
            session_timeout: Duration::from_millis(200),
            reconnect_delay: Duration::from_millis(50),
        };

        let peer_id = HashId([1u8; 32]);
        manager.register_tunnel(peer_id);

        assert!(manager.is_tunnel_active(&peer_id));

        // Wait for expiry
        sleep(Duration::from_millis(250)).await;

        assert!(!manager.is_tunnel_active(&peer_id));

        let expired = manager.check_expired_tunnels();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], peer_id);
    }
}
