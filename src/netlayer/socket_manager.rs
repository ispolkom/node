// src/netlayer/socket_manager.rs
//! Socket Manager
//! ==============
//!
//! Manages UDP sockets with graceful rotation support.

use std::sync::Arc;
use crate::netlayer::tcp_carrier::CSocket;
use tokio::sync::RwLock;

/// Socket pair for discovery and data
#[derive(Clone)]
pub struct SocketPair {
    /// Discovery socket (Hello packets)
    pub discovery: Arc<CSocket>,
    /// Data socket (encrypted session)
    pub data: Arc<CSocket>,
    /// P2P socket (chat, files, calls)
    pub p2p: Arc<CSocket>,
}

impl SocketPair {
    pub fn new(discovery: Arc<CSocket>, data: Arc<CSocket>, p2p: Arc<CSocket>) -> Self {
        Self { discovery, data, p2p }
    }
}

/// Socket manager with rotation support
pub struct SocketManager {
    current: Arc<RwLock<SocketPair>>,
}

impl SocketManager {
    pub fn new(discovery: Arc<CSocket>, data: Arc<CSocket>, p2p: Arc<CSocket>) -> Self {
        Self {
            current: Arc::new(RwLock::new(SocketPair::new(discovery, data, p2p))),
        }
    }

    /// Get current socket pair
    pub async fn get(&self) -> SocketPair {
        self.current.read().await.clone()
    }

    /// Get discovery socket
    pub async fn discovery(&self) -> Arc<CSocket> {
        self.current.read().await.discovery.clone()
    }

    /// Get data socket
    pub async fn data(&self) -> Arc<CSocket> {
        self.current.read().await.data.clone()
    }

    /// Rotate to new sockets
    pub async fn rotate(&self, new_discovery: Arc<CSocket>, new_data: Arc<CSocket>, new_p2p: Arc<CSocket>) {
        println!("[socket_manager] 🔄 Rotating sockets to new endpoints");
        let mut guard = self.current.write().await;
        guard.discovery = new_discovery;
        guard.data = new_data;
        guard.p2p = new_p2p;
        println!("[socket_manager] ✅ Socket rotation complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    async fn wrapped() -> Arc<CSocket> {
        let carrier = crate::netlayer::tcp_carrier::install("00".repeat(32));
        CSocket::new(Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap()), carrier).unwrap()
    }

    #[tokio::test]
    async fn test_socket_manager() {
        let sock1 = wrapped().await;
        let sock2 = wrapped().await;

        let manager = SocketManager::new(sock1.clone(), sock2.clone(), sock1.clone());

        let pair = manager.get().await;
        assert_eq!(pair.discovery.local_addr().unwrap().port(), sock1.local_addr().unwrap().port());

        let new_sock1 = wrapped().await;
        let new_sock2 = wrapped().await;
        manager.rotate(new_sock1.clone(), new_sock2.clone(), new_sock1.clone()).await;

        let pair = manager.get().await;
        assert_eq!(pair.discovery.local_addr().unwrap().port(), new_sock1.local_addr().unwrap().port());
    }
}
