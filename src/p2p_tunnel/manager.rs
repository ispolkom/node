// src/p2p_tunnel/manager.rs
//! P2P Tunnel Manager

use crate::p2p_tunnel::{P2PTunnel, TunnelType, TunnelStatus, TunnelRequest, TunnelResponse, TunnelInfo, TunnelPacket, P2PTunnelPacket};
use crate::util::HashId;
use crate::p2p::P2PTransport;  // Используем P2P transport (порт 9998)
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use anyhow::Result;
use tracing::{info, error, debug, warn};

/// Менеджер P2P тоннелей
#[derive(Clone)]
pub struct P2PTunnelManager {
    my_node_id: HashId,
    transport: Arc<P2PTransport>,
    /// Активные тоннели (tunnel_id -> tunnel)
    tunnels: Arc<RwLock<HashMap<HashId, P2PTunnel>>>,
    /// Тоннели по peer_id (peer_id -> tunnel_id)
    peer_tunnels: Arc<Mutex<HashMap<HashId, HashId>>>,
}

impl P2PTunnelManager {
    /// Создать новый менеджер
    pub fn new(my_node_id: HashId, transport: Arc<P2PTransport>) -> Self {
        Self {
            my_node_id,
            transport,
            tunnels: Arc::new(RwLock::new(HashMap::new())),
            peer_tunnels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Запросить создание P2P тоннеля
    pub async fn request_tunnel(&self, peer_id: HashId, tunnel_type: TunnelType) -> Result<P2PTunnel> {
        info!("🔗 Requesting P2P tunnel with {} (type: {:?})",
            hex::encode(&peer_id.0[..8]),
            tunnel_type
        );

        // Проверить: нет ли уже тоннеля с этим peer?
        {
            let peer_tunnels = self.peer_tunnels.lock().await;
            if let Some(existing_tunnel_id) = peer_tunnels.get(&peer_id) {
                let tunnels = self.tunnels.read().await;
                if let Some(tunnel) = tunnels.get(existing_tunnel_id) {
                    if tunnel.status() == TunnelStatus::Established {
                        return Err(anyhow::anyhow!("Tunnel already established with peer {}", hex::encode(&peer_id.0[..8])));
                    }
                }
            }
        }

        // Создать тоннель
        let tunnel = P2PTunnel::new(
            self.my_node_id,
            peer_id,
            tunnel_type,
            self.transport.clone(),
        );

        let tunnel_id = tunnel.id();

        // Сохранить
        {
            let mut tunnels = self.tunnels.write().await;
            tunnels.insert(tunnel_id, tunnel.clone());
        }

        {
            let mut peer_tunnels = self.peer_tunnels.lock().await;
            peer_tunnels.insert(peer_id, tunnel_id);
        }

        // Отправить TunnelRequest
        let request = TunnelRequest {
            tunnel_type,
            codec: None, // TODO: добавить выбор кодека
            initiator: self.my_node_id,
            timestamp: now_ms(),
        };

        let packet = TunnelPacket::tunnel_request(&request)?;
        let packet_bytes = packet.to_bytes();

        self.transport.send_encrypted(peer_id, &packet_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to send tunnel request: {}", e))?;

        info!("✅ Tunnel request sent to {}", hex::encode(&peer_id.0[..8]));

        Ok(tunnel)
    }

    /// Принять запрос на тоннель (автоматический accept)
    pub async fn accept_tunnel(&self, from: HashId, request: TunnelRequest) -> Result<P2PTunnel> {
        info!("✅ Accepting P2P tunnel from {} (type: {:?})",
            hex::encode(&from.0[..8]),
            request.tunnel_type
        );

        // Создать тоннель
        let mut tunnel = P2PTunnel::new(
            self.my_node_id,
            from,
            request.tunnel_type,
            self.transport.clone(),
        );

        tunnel.set_status(TunnelStatus::Established);

        let tunnel_id = tunnel.id();

        // Сохранить
        {
            let mut tunnels = self.tunnels.write().await;
            tunnels.insert(tunnel_id, tunnel.clone());
        }

        {
            let mut peer_tunnels = self.peer_tunnels.lock().await;
            peer_tunnels.insert(from, tunnel_id);
        }

        // Отправить TunnelAccept
        let response = TunnelResponse {
            accepted: true,
            reason: None,
            responder: self.my_node_id,
            timestamp: now_ms(),
        };

        let packet = TunnelPacket::tunnel_response(&response)?;
        let packet_bytes = packet.to_bytes();

        self.transport.send_encrypted(from, &packet_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to send tunnel accept: {}", e))?;

        info!("✅ Tunnel ACCEPTED and established with {}", hex::encode(&from.0[..8]));

        Ok(tunnel)
    }

    /// Отклонить запрос на тоннель
    pub async fn reject_tunnel(&self, from: HashId, reason: &str) -> Result<()> {
        warn!("❌ Rejecting P2P tunnel from {}: {}",
            hex::encode(&from.0[..8]),
            reason
        );

        let response = TunnelResponse {
            accepted: false,
            reason: Some(reason.to_string()),
            responder: self.my_node_id,
            timestamp: now_ms(),
        };

        // Создать пакет с reject
        let packet_bytes = {
            let data = serde_json::to_vec(&response)?;
            let mut bytes = vec![P2PTunnelPacket::TunnelReject.as_byte()];
            bytes.extend_from_slice(&data);
            bytes
        };

        self.transport.send_encrypted(from, &packet_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to send tunnel reject: {}", e))?;

        Ok(())
    }

    /// Обработать входящий пакет тоннеля
    pub async fn handle_packet(&self, from: HashId, packet_bytes: Vec<u8>) -> Result<()> {
        let packet = TunnelPacket::from_bytes(&packet_bytes)
            .ok_or_else(|| anyhow::anyhow!("Invalid tunnel packet"))?;

        debug!("📨 Received tunnel packet: {:?}", packet.packet_type);

        match packet.packet_type {
            P2PTunnelPacket::TunnelRequest => {
                // Распаковать запрос
                let request: TunnelRequest = serde_json::from_slice(&packet.data)?;

                // Автоматически принимаем (TODO: показать UI для подтверждения)
                let _tunnel = self.accept_tunnel(from, request).await?;
            }

            P2PTunnelPacket::TunnelAccept => {
                // Распаковать ответ
                let response: TunnelResponse = serde_json::from_slice(&packet.data)?;

                if response.accepted {
                    // Найти тоннель и обновить статус
                    let mut peer_tunnels = self.peer_tunnels.lock().await;
                    if let Some(tunnel_id) = peer_tunnels.get(&from) {
                        let mut tunnels = self.tunnels.write().await;
                        if let Some(tunnel) = tunnels.get_mut(tunnel_id) {
                            tunnel.set_status(TunnelStatus::Established);
                            info!("✅ Tunnel ESTABLISHED with {}", hex::encode(&from.0[..8]));
                        }
                    }
                } else {
                    // Тоннель отклонён
                    warn!("❌ Tunnel REJECTED by {}: {:?}",
                        hex::encode(&from.0[..8]),
                        response.reason
                    );

                    // Удалить из активных
                    let mut peer_tunnels = self.peer_tunnels.lock().await;
                    let mut tunnels = self.tunnels.write().await;

                    if let Some(tunnel_id) = peer_tunnels.remove(&from) {
                        tunnels.remove(&tunnel_id);
                    }
                }
            }

            P2PTunnelPacket::TunnelReject => {
                // Тоннель отклонён
                warn!("❌ Tunnel REJECTED by {}", hex::encode(&from.0[..8]));

                let mut peer_tunnels = self.peer_tunnels.lock().await;
                let mut tunnels = self.tunnels.write().await;

                if let Some(tunnel_id) = peer_tunnels.remove(&from) {
                    tunnels.remove(&tunnel_id);
                }
            }

            P2PTunnelPacket::TunnelData => {
                // Данные тоннеля - найти нужный тоннель и передать данные
                let peer_tunnels = self.peer_tunnels.lock().await;
                if let Some(tunnel_id) = peer_tunnels.get(&from) {
                    let tunnels = self.tunnels.read().await;
                    if let Some(tunnel) = tunnels.get(tunnel_id) {
                        tunnel.handle_tunnel_data(packet.data).await?;
                    }
                }
            }

            P2PTunnelPacket::TunnelClose => {
                // Закрытие тоннеля
                info!("🔒 Tunnel CLOSE received from {}", hex::encode(&from.0[..8]));

                let mut peer_tunnels = self.peer_tunnels.lock().await;
                let mut tunnels = self.tunnels.write().await;

                if let Some(tunnel_id) = peer_tunnels.remove(&from) {
                    if let Some(mut tunnel) = tunnels.remove(&tunnel_id) {
                        tunnel.set_status(TunnelStatus::Closed);
                    }
                }
            }

            P2PTunnelPacket::TunnelPing => {
                // TODO: ответить pong
            }

            P2PTunnelPacket::TunnelPong => {
                // TODO: обработать pong
            }
        }

        Ok(())
    }

    /// Закрыть тоннель
    pub async fn close_tunnel(&self, peer_id: HashId) -> Result<()> {
        info!("🔒 Closing tunnel with {}", hex::encode(&peer_id.0[..8]));

        let packet = TunnelPacket::tunnel_close(Some("User closed"));

        // Отправить close
        let mut peer_tunnels = self.peer_tunnels.lock().await;
        if let Some(tunnel_id) = peer_tunnels.remove(&peer_id) {
            let mut tunnels = self.tunnels.write().await;
            if let Some(mut tunnel) = tunnels.remove(&tunnel_id) {
                tunnel.set_status(TunnelStatus::Closed);

                let packet_bytes = packet.to_bytes();
                let _ = self.transport.send_encrypted(peer_id, &packet_bytes).await;
            }
        }

        Ok(())
    }

    /// Получить список активных тоннелей
    pub async fn list_tunnels(&self) -> Vec<TunnelInfo> {
        let tunnels = self.tunnels.read().await;
        let mut result = Vec::new();
        for tunnel in tunnels.values() {
            result.push(tunnel.info().await);
        }
        result
    }

    /// Получить тоннель по peer_id
    pub async fn get_tunnel(&self, peer_id: &HashId) -> Option<P2PTunnel> {
        let peer_tunnels = self.peer_tunnels.lock().await;
        let tunnel_id = peer_tunnels.get(peer_id)?;

        let tunnels = self.tunnels.read().await;
        tunnels.get(tunnel_id).cloned()
    }
}

/// Получить текущий timestamp в ms
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
