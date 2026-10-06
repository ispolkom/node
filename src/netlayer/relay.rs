// src/netlayer/relay.rs
//! NOR - Node Over Relay
//! =====================
//!
//! Relay mechanism for nodes behind NAT to communicate via public relay nodes

use crate::util::HashId;
use crate::netlayer::packet::{
    RelayConnectRequest, RelayConnectResponse, RelayDataPacket, RelayClosePacket,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use std::net::SocketAddr;

/// Relay session timeout configuration
#[derive(Clone, Debug)]
pub struct RelayTimeout {
    /// Session timeout (close relay session if no packets for this duration)
    pub session_timeout: Duration,
    /// Connection timeout (wait for relay response)
    pub connection_timeout: Duration,
    /// Heartbeat interval
    pub heartbeat_interval: Duration,
}

impl Default for RelayTimeout {
    fn default() -> Self {
        Self {
            session_timeout: Duration::from_secs(300),  // 5 minutes
            connection_timeout: Duration::from_secs(10), // 10 seconds
            heartbeat_interval: Duration::from_secs(30), // 30 seconds
        }
    }
}

/// Relay session status
#[derive(Debug, Clone, PartialEq)]
pub enum RelaySessionStatus {
    /// Connecting (waiting for relay response)
    Connecting,
    /// Active (data flowing through relay)
    Active,
    /// Stale (no packets recently, but not timed out)
    Stale,
    /// Closed
    Closed,
    /// Failed (connection rejected or timeout)
    Failed,
}

/// Relay session information
#[derive(Debug, Clone)]
pub struct RelaySession {
    /// Unique session ID
    pub session_id: u64,
    /// Source peer (behind NAT or initiator)
    pub source_peer: HashId,
    /// Target peer (behind NAT or destination)
    pub target_peer: HashId,
    /// Source socket address (for relay server)
    pub source_addr: Option<SocketAddr>,
    /// Target socket address (for relay server)
    pub target_addr: Option<SocketAddr>,
    /// Session status
    pub status: RelaySessionStatus,
    /// Last activity timestamp
    pub last_activity: Instant,
    /// Bytes forwarded
    pub bytes_forwarded: u64,
    /// Packets forwarded
    pub packets_forwarded: u64,
    /// Session created at
    pub created_at: Instant,
}

impl RelaySession {
    pub fn new(session_id: u64, source_peer: HashId, target_peer: HashId) -> Self {
        Self {
            session_id,
            source_peer,
            target_peer,
            source_addr: None,
            target_addr: None,
            status: RelaySessionStatus::Connecting,
            last_activity: Instant::now(),
            bytes_forwarded: 0,
            packets_forwarded: 0,
            created_at: Instant::now(),
        }
    }

    pub fn is_expired(&self, timeout: Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    pub fn update_activity(&mut self, bytes: usize) {
        self.last_activity = Instant::now();
        self.packets_forwarded += 1;
        self.bytes_forwarded += bytes as u64;
        self.status = RelaySessionStatus::Active;
    }

    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    pub fn idle_time(&self) -> Duration {
        self.last_activity.elapsed()
    }
}

/// Relay manager - manages relay sessions
///
/// Two modes:
/// 1. Relay Server: Public node that forwards traffic between NAT'd peers
/// 2. Relay Client: Node behind NAT that initiates relay connections
pub struct RelayManager {
    /// Active relay sessions (relay server mode)
    sessions: HashMap<u64, RelaySession>,
    /// Peer to session mapping (for quick lookup)
    peer_to_session: HashMap<HashId, u64>,
    /// Session ID counter
    next_session_id: u64,
    /// Timeout configuration
    timeout_config: RelayTimeout,
    /// Are we acting as relay server?
    is_relay_server: bool,
}

impl RelayManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            peer_to_session: HashMap::new(),
            next_session_id: 1,
            timeout_config: RelayTimeout::default(),
            is_relay_server: false,
        }
    }

    pub fn with_timeout(timeout_config: RelayTimeout) -> Self {
        Self {
            sessions: HashMap::new(),
            peer_to_session: HashMap::new(),
            next_session_id: 1,
            timeout_config,
            is_relay_server: false,
        }
    }

    /// Enable relay server mode (this node will relay traffic for others)
    pub fn set_relay_server_mode(&mut self, enabled: bool) {
        self.is_relay_server = enabled;
        if enabled {
            println!("[relay] 🌐 Relay server mode ENABLED - will forward traffic for NAT'd peers");
        } else {
            println!("[relay] 🌐 Relay server mode DISABLED");
        }
    }

    pub fn is_relay_server(&self) -> bool {
        self.is_relay_server
    }

    /// Generate new session ID
    fn generate_session_id(&mut self) -> u64 {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.wrapping_add(1);
        id
    }

    /// Create relay session (relay server mode)
    pub fn create_session(
        &mut self,
        source_peer: HashId,
        target_peer: HashId,
    ) -> u64 {
        let session_id = self.generate_session_id();
        let mut session = RelaySession::new(session_id, source_peer, target_peer);

        println!("[relay] 🔗 Creating relay session {} for {} -> {}",
                 session_id,
                 hex::encode(&source_peer.0[..8]),
                 hex::encode(&target_peer.0[..8]));

        session.status = RelaySessionStatus::Connecting;
        self.sessions.insert(session_id, session.clone());
        self.peer_to_session.insert(source_peer, session_id);

        session_id
    }

    /// Activate relay session (both peers connected)
    pub fn activate_session(&mut self, session_id: u64) -> Result<(), String> {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.status = RelaySessionStatus::Active;
            session.last_activity = Instant::now();
            println!("[relay] ✅ Relay session {} ACTIVATED", session_id);
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// Update session activity (relay server mode)
    pub fn update_session_activity(&mut self, session_id: u64, bytes: usize) -> Result<(), String> {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.update_activity(bytes);
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// Close relay session
    pub fn close_session(&mut self, session_id: u64, reason: &str) {
        if let Some(mut session) = self.sessions.remove(&session_id) {
            session.status = RelaySessionStatus::Closed;
            self.peer_to_session.remove(&session.source_peer);
            self.peer_to_session.remove(&session.target_peer);

            println!("[relay] 🔒 Relay session {} closed: {} (forwarded {} bytes in {} packets)",
                     session_id, reason,
                     session.bytes_forwarded,
                     session.packets_forwarded);
        }
    }

    /// Get session by ID
    pub fn get_session(&self, session_id: u64) -> Option<&RelaySession> {
        self.sessions.get(&session_id)
    }

    /// Get session by peer ID
    pub fn get_session_by_peer(&self, peer_id: &HashId) -> Option<&RelaySession> {
        if let Some(session_id) = self.peer_to_session.get(peer_id) {
            self.sessions.get(session_id)
        } else {
            None
        }
    }

    /// Get session for mutation
    pub fn get_session_mut(&mut self, session_id: u64) -> Option<&mut RelaySession> {
        self.sessions.get_mut(&session_id)
    }

    /// Set peer addresses in session (relay server mode)
    pub fn set_peer_addresses(
        &mut self,
        session_id: u64,
        source_addr: SocketAddr,
        target_addr: SocketAddr,
    ) -> Result<(), String> {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.source_addr = Some(source_addr);
            session.target_addr = Some(target_addr);
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// Check for expired sessions
    pub fn check_expired_sessions(&mut self) -> Vec<u64> {
        let mut expired = Vec::new();

        for (session_id, session) in self.sessions.iter_mut() {
            if session.is_expired(self.timeout_config.session_timeout) {
                println!("[relay] ⚠️  Relay session {} expired (idle for {:?})",
                         session_id, session.idle_time());
                session.status = RelaySessionStatus::Closed;
                expired.push(*session_id);
            }
        }

        // Remove expired sessions
        for session_id in &expired {
            self.close_session(*session_id, "expired");
        }

        expired
    }

    /// Get all active sessions
    pub fn get_active_sessions(&self) -> Vec<&RelaySession> {
        self.sessions
            .values()
            .filter(|s| !s.is_expired(self.timeout_config.session_timeout))
            .collect()
    }

    /// Get session count
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Get relay statistics
    pub fn get_stats(&self) -> RelayStats {
        let active_sessions = self.get_active_sessions();

        let total_bytes: u64 = active_sessions.iter()
            .map(|s| s.bytes_forwarded)
            .sum();

        let total_packets: u64 = active_sessions.iter()
            .map(|s| s.packets_forwarded)
            .sum();

        RelayStats {
            active_sessions: active_sessions.len(),
            total_bytes_forwarded: total_bytes,
            total_packets_forwarded: total_packets,
            is_relay_server: self.is_relay_server,
        }
    }
}

/// Relay statistics
#[derive(Debug, Clone)]
pub struct RelayStats {
    pub active_sessions: usize,
    pub total_bytes_forwarded: u64,
    pub total_packets_forwarded: u64,
    pub is_relay_server: bool,
}

/// Background task to monitor relay sessions
pub async fn relay_monitor_task(
    manager: Arc<Mutex<RelayManager>>,
    on_session_expired: tokio::sync::mpsc::Sender<u64>,
) {
    println!("[relay] 🔍 Relay monitor started");

    let mut interval = tokio::time::interval(Duration::from_secs(10));

    loop {
        interval.tick().await;

        let expired = {
            let mut manager = manager.lock().await;
            manager.check_expired_sessions()
        };

        // Notify about expired sessions
        for session_id in expired {
            println!("[relay] 🔄 Relay session {} expired", session_id);
            let _ = on_session_expired.send(session_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relay_session_creation() {
        let mut manager = RelayManager::new();

        let peer_a = HashId([1u8; 32]);
        let peer_b = HashId([2u8; 32]);

        let session_id = manager.create_session(peer_a, peer_b);

        assert_eq!(manager.session_count(), 1);

        let session = manager.get_session(session_id).unwrap();
        assert_eq!(session.source_peer, peer_a);
        assert_eq!(session.target_peer, peer_b);
        assert_eq!(session.status, RelaySessionStatus::Connecting);
    }

    #[test]
    fn test_relay_session_expiration() {
        let mut manager = RelayManager::new();

        let peer_a = HashId([1u8; 32]);
        let peer_b = HashId([2u8; 32]);

        manager.timeout_config = RelayTimeout {
            session_timeout: Duration::from_millis(100),
            connection_timeout: Duration::from_millis(50),
            heartbeat_interval: Duration::from_millis(25),
        };

        let session_id = manager.create_session(peer_a, peer_b);
        assert_eq!(manager.session_count(), 1);

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(150));

        let expired = manager.check_expired_sessions();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], session_id);
        assert_eq!(manager.session_count(), 0);
    }

    #[test]
    fn test_relay_server_mode() {
        let mut manager = RelayManager::new();

        assert!(!manager.is_relay_server());

        manager.set_relay_server_mode(true);
        assert!(manager.is_relay_server());

        manager.set_relay_server_mode(false);
        assert!(!manager.is_relay_server());
    }

    #[test]
    fn test_peer_to_session_lookup() {
        let mut manager = RelayManager::new();

        let peer_a = HashId([1u8; 32]);
        let peer_b = HashId([2u8; 32]);

        let session_id = manager.create_session(peer_a, peer_b);

        let session = manager.get_session_by_peer(&peer_a).unwrap();
        assert_eq!(session.session_id, session_id);
        assert_eq!(session.source_peer, peer_a);
    }
}