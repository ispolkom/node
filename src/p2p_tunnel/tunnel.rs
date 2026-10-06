// src/p2p_tunnel/tunnel.rs
//! Pure P2P Tunnel (Peer-to-Peer)

use crate::p2p_tunnel::{TunnelType, TunnelStatus, TunnelInfo};
use crate::util::HashId;
use crate::p2p::P2PTransport;  // Используем P2P transport (порт 9998)
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use anyhow::Result;
use tracing::{info, error, debug};

/// P2P тоннель точка-точка (чистый P2P, БЕЗ выхода в интернет)
#[derive(Clone)]
pub struct P2PTunnel {
    tunnel_id: HashId,
    my_node_id: HashId,
    peer_id: HashId,
    tunnel_type: TunnelType,
    status: TunnelStatus,
    transport: Arc<P2PTransport>,
    /// Канал для входящих данных (voice/video/file)
    data_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Канал для исходящих данных
    data_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>>,
    created_at: u64,
    bytes_sent: Arc<Mutex<u64>>,
    bytes_received: Arc<Mutex<u64>>,
}

impl P2PTunnel {
    /// Создать новый P2P тоннель
    pub fn new(
        my_node_id: HashId,
        peer_id: HashId,
        tunnel_type: TunnelType,
        transport: Arc<P2PTransport>,
    ) -> Self {
        let tunnel_id = HashId::new_random();
        let (data_tx, data_rx) = mpsc::unbounded_channel();

        Self {
            tunnel_id,
            my_node_id,
            peer_id,
            tunnel_type,
            status: TunnelStatus::Requested,
            transport,
            data_tx,
            data_rx: Arc::new(Mutex::new(Some(data_rx))),
            created_at: now_ms(),
            bytes_sent: Arc::new(Mutex::new(0)),
            bytes_received: Arc::new(Mutex::new(0)),
        }
    }

    /// Получить ID тоннеля
    pub fn id(&self) -> HashId {
        self.tunnel_id
    }

    /// Получить peer ID
    pub fn peer_id(&self) -> HashId {
        self.peer_id
    }

    /// Получить тип тоннеля
    pub fn tunnel_type(&self) -> TunnelType {
        self.tunnel_type
    }

    /// Получить статус
    pub fn status(&self) -> TunnelStatus {
        self.status
    }

    /// Установить статус
    pub fn set_status(&mut self, status: TunnelStatus) {
        self.status = status;
    }

    /// Отправить данные через P2P тоннель
    pub async fn send_data(&self, data: Vec<u8>) -> Result<()> {
        use crate::p2p_tunnel::TunnelPacket;
        use crate::p2p_tunnel::P2PTunnelPacket;

        debug!("📤 Sending {} bytes through P2P tunnel to {}",
            data.len(),
            hex::encode(&self.peer_id.0[..8])
        );

        // Упаковать в TunnelData пакет
        let packet = TunnelPacket::new(P2PTunnelPacket::TunnelData, data);
        let packet_bytes = packet.to_bytes();

        // Отправить через P2P transport (сквозное шифрование!)
        self.transport.send_encrypted(self.peer_id, &packet_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to send tunnel data: {}", e))?;

        // Обновить статистику
        let mut sent = self.bytes_sent.lock().await;
        *sent += packet_bytes.len() as u64;

        Ok(())
    }

    /// Получить канал для отправки данных
    pub fn data_tx(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.data_tx.clone()
    }

    /// Обработать входящие данные из тоннеля
    pub async fn handle_tunnel_data(&self, data: Vec<u8>) -> Result<()> {
        let data_len = data.len();
        debug!("📨 Received {} bytes from P2P tunnel", data_len);

        // Отправить в канал для обработки (VoIP/Video/File)
        let _ = self.data_tx.send(data);

        // Обновить статистику
        let mut received = self.bytes_received.lock().await;
        *received += data_len as u64;

        Ok(())
    }

    /// Получить информацию о тоннеле
    pub async fn info(&self) -> TunnelInfo {
        let sent = *self.bytes_sent.lock().await;
        let received = *self.bytes_received.lock().await;

        TunnelInfo {
            tunnel_id: self.tunnel_id,
            peer: self.peer_id,
            tunnel_type: self.tunnel_type,
            status: self.status,
            created_at: self.created_at,
            bytes_sent: sent,
            bytes_received: received,
        }
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
