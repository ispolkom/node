// src/protocol/tcp_transport.rs
//!
//! # TCP Transport (Транспорт TCP)
//!
//! Полный аналог UDP транспорта, но поверх TCP.
//!
//! Особенности:
//! - TCP соединения вместо UDP
//! - Dual-Path с клонами (path 0, path 1)
//! - Полное шифрование ECDH + AES-256-GCM
//! - Дедупликация вагонов
//! - Автоматическое переподключение
//! - Поддержка множественных peer

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, anyhow};
use serde::{Serialize, Deserialize};

use crate::util::HashId;
use crate::protocol::tcp_station::{TcpStation, TcpStationConfig, TcpWagon, TcpConnection};
use crate::protocol::{TrainId, Wagon};
use crate::netlayer::{
    encryption::EncryptionManager,
    peer::PeerInfo,
};
use tracing::{info, error, debug, warn};

/// TCP Packet types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TcpPacketType {
    /// Wagon (данные)
    Wagon = 0x60,

    /// Keepalive
    Keepalive = 0x01,

    /// Handshake
    Handshake = 0x10,
}

impl TcpPacketType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x60 => Some(Self::Wagon),
            0x01 => Some(Self::Keepalive),
            0x10 => Some(Self::Handshake),
            _ => None,
        }
    }
}

/// TCP Packet header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpPacketHeader {
    /// Тип пакета
    pub packet_type: u8,

    /// Длина payload
    pub payload_len: u32,

    /// Sequence number
    pub seq_num: u64,

    /// Timestamp
    pub timestamp_ms: u64,
}

impl TcpPacketHeader {
    pub fn new(packet_type: TcpPacketType, payload_len: u32) -> Self {
        Self {
            packet_type: packet_type as u8,
            payload_len,
            seq_num: 0,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    /// Размер заголовка в байтах
    pub const fn size() -> usize {
        1 + 4 + 8 + 8  // packet_type + payload_len + seq_num + timestamp_ms
    }

    /// Упаковать в байты
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::size());
        bytes.push(self.packet_type);
        bytes.extend_from_slice(&self.payload_len.to_be_bytes());
        bytes.extend_from_slice(&self.seq_num.to_be_bytes());
        bytes.extend_from_slice(&self.timestamp_ms.to_be_bytes());
        bytes
    }

    /// Распаковать из байтов
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < Self::size() {
            return Err(anyhow!("Header too short"));
        }

        let packet_type = bytes[0];
        let payload_len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let seq_num = u64::from_be_bytes([
            bytes[5], bytes[6], bytes[7], bytes[8],
            bytes[9], bytes[10], bytes[11], bytes[12],
        ]);
        let timestamp_ms = u64::from_be_bytes([
            bytes[13], bytes[14], bytes[15], bytes[16],
            bytes[17], bytes[18], bytes[19], bytes[20],
        ]);

        Ok(Self {
            packet_type,
            payload_len,
            seq_num,
            timestamp_ms,
        })
    }
}

/// TCP Peer session
pub struct TcpPeerSession {
    /// Peer ID
    pub peer_id: HashId,

    /// TCP соединение
    pub conn: Option<TcpConnection>,

    /// TCP Station для этого peer
    pub station: Arc<TcpStation>,

    /// Время создания
    pub created_at: Instant,

    /// Последняя активность
    pub last_activity: Arc<Mutex<Instant>>,

    /// Sequence counter для отправки
    pub seq_counter: Arc<Mutex<u64>>,
}

impl TcpPeerSession {
    pub fn new(peer_id: HashId, station: Arc<TcpStation>) -> Self {
        let now = Instant::now();
        Self {
            peer_id,
            conn: None,
            station,
            created_at: now,
            last_activity: Arc::new(Mutex::new(now)),
            seq_counter: Arc::new(Mutex::new(0)),
        }
    }

    /// Отправить данные через train
    pub async fn send_data(&self, data: Vec<u8>) -> Result<TrainId> {
        let mut conn_guard = self.conn.as_ref()
            .ok_or_else(|| anyhow!("No connection"))?;

        let mut stream = conn_guard.stream.lock().await;
        let train_id = self.station.send_train(self.peer_id, &mut stream, data).await?;

        // Обновляем активность
        *self.last_activity.lock().await = Instant::now();

        Ok(train_id)
    }
}

/// TCP Transport - менеджер TCP соединений
pub struct TcpTransport {
    /// Локальный node ID
    pub local_id: HashId,

    /// Менеджер шифрования
    pub encryption: Arc<Mutex<EncryptionManager>>,

    /// Активные peer sessions
    pub peers: Arc<Mutex<HashMap<HashId, TcpPeerSession>>>,

    /// TCP listener (для входящих соединений)
    listener: Arc<Mutex<Option<TcpListener>>>,

    /// Порт для прослушивания
    pub bind_port: u16,

    /// Конфигурация станции
    station_config: TcpStationConfig,

    /// Следующий ID соединения
    next_conn_id: Arc<Mutex<u64>>,

    /// Incoming callback
    pub incoming_callback: Arc<Mutex<Option<Box<dyn Fn(HashId, Vec<u8>) + Send + Sync>>>>,
}

impl TcpTransport {
    /// Создать новый TCP транспорт
    pub fn new(
        local_id: HashId,
        encryption: Arc<Mutex<EncryptionManager>>,
        bind_port: u16,
    ) -> Self {
        Self {
            local_id,
            encryption,
            peers: Arc::new(Mutex::new(HashMap::new())),
            listener: Arc::new(Mutex::new(None)),
            bind_port,
            station_config: TcpStationConfig::default(),
            next_conn_id: Arc::new(Mutex::new(1)),
            incoming_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// Запустить TCP транспорт
    pub async fn start(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.bind_port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| anyhow!("Failed to bind TCP listener on {}: {}", addr, e))?;

        {
            let mut lis = self.listener.lock().await;
            *lis = Some(listener);
        }

        info!("🚇 TCP Transport listening on {}", addr);

        // Запускаем accept loop
        let peers = self.peers.clone();
        let encryption = self.encryption.clone();
        let listener = self.listener.clone();
        let next_conn_id = self.next_conn_id.clone();
        let local_id = self.local_id;
        let station_config = self.station_config.clone();
        let incoming_callback = self.incoming_callback.clone();

        tokio::spawn(async move {
            Self::accept_loop(peers, encryption, listener, next_conn_id, local_id, station_config, incoming_callback).await;
        });

        // Запускаем keepalive task
        let peers_clone = self.peers.clone();
        tokio::spawn(async move {
            Self::keepalive_loop(peers_clone).await;
        });

        Ok(())
    }

    /// Accept loop для входящих соединений
    async fn accept_loop(
        peers: Arc<Mutex<HashMap<HashId, TcpPeerSession>>>,
        encryption: Arc<Mutex<EncryptionManager>>,
        listener: Arc<Mutex<Option<TcpListener>>>,
        next_conn_id: Arc<Mutex<u64>>,
        local_id: HashId,
        station_config: TcpStationConfig,
        incoming_callback: Arc<Mutex<Option<Box<dyn Fn(HashId, Vec<u8>) + Send + Sync>>>>,
    ) {
        loop {
            let (mut stream, addr) = {
                let lis = listener.lock().await;
                let (mut stream, addr) = match lis.as_ref() {
                    Some(l) => match l.accept().await {
                        Ok(s) => s,
                        Err(e) => {
                            error!("❌ Accept error: {}", e);
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    },
                    None => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                (stream, addr)
            };

            info!("📥 Incoming TCP connection from {}", addr);

            // Читаем handshake
            match Self::read_handshake(&mut stream).await {
                Ok(peer_id) => {
                    info!("✅ Handshake from {}", hex::encode(&peer_id.0[..8]));

                    // Создаём соединение
                    let conn_id = {
                        let mut id = next_conn_id.lock().await;
                        let cid = *id;
                        *id += 1;
                        cid
                    };

                    let tcp_conn = TcpConnection::new(conn_id, peer_id, stream);

                    // Создаём station
                    let station = Arc::new(TcpStation::new(
                        local_id,
                        encryption.clone(),
                        station_config.clone(),
                    ));

                    // Создаём session
                    let session = TcpPeerSession::new(peer_id, station.clone());
                    let mut peers_guard = peers.lock().await;
                    peers_guard.insert(peer_id, session);

                    // Устанавливаем callback
                    let incoming_cb = incoming_callback.clone();
                    station.set_data_callback(move |source: HashId, data: Vec<u8>| {
                        debug!("📨 Data callback from {}: {} bytes", hex::encode(&source.0[..8]), data.len());

                        // Вызываем incoming callback
                        if let Ok(guard) = incoming_cb.try_lock() {
                            if let Some(cb) = guard.as_ref() {
                                cb(source, data);
                            }
                        }
                    }).await;

                    info!("✅ Peer session created for {}", hex::encode(&peer_id.0[..8]));
                }
                Err(e) => {
                    error!("❌ Handshake error: {}", e);
                }
            }
        }
    }

    /// Прочитать handshake из потока
    async fn read_handshake(stream: &mut TcpStream) -> Result<HashId> {
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await?;
        let len = u32::from_be_bytes(len_bytes) as usize;

        let mut handshake_data = vec![0u8; len];
        stream.read_exact(&mut handshake_data).await?;

        // Десериализуем handshake
        let hs: HandshakePacket = bincode::deserialize(&handshake_data)
            .map_err(|e| anyhow!("Failed to deserialize handshake: {}", e))?;

        Ok(hs.peer_id)
    }

    /// Keepalive loop
    async fn keepalive_loop(peers: Arc<Mutex<HashMap<HashId, TcpPeerSession>>>) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));

        loop {
            interval.tick().await;

            let mut peers_guard = peers.lock().await;

            // Отправляем keepalive всем активным peer
            for (peer_id, session) in peers_guard.iter_mut() {
                if let Some(conn) = &session.conn {
                    // TODO: отправить keepalive
                    debug!("💓 Keepalive for {}", hex::encode(&peer_id.0[..8]));
                }
            }

            // Удаляем неактивные сессии
            peers_guard.retain(|peer_id, session| {
                let last = session.last_activity.try_lock();
                let is_stale = last.map(|l| l.elapsed() > Duration::from_secs(300)).unwrap_or(false);

                if is_stale {
                    info!("🗑️ Removing stale peer {}", hex::encode(&peer_id.0[..8]));
                }

                !is_stale
            });
        }
    }

    /// Подключиться к peer
    pub async fn connect(&self, peer: &PeerInfo) -> Result<()> {
        info!("🔌 Connecting to {} ({})", hex::encode(&peer.id.0[..8]), peer.addr);

        // Создаём TCP соединение
        let stream = TcpStream::connect(&peer.addr).await
            .map_err(|e| anyhow!("Failed to connect to {}: {}", peer.addr, e))?;

        // Создаём station
        let station = Arc::new(TcpStation::new(
            self.local_id,
            self.encryption.clone(),
            self.station_config.clone(),
        ));

        // Отправляем handshake
        let handshake = HandshakePacket {
            peer_id: self.local_id,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };

        let handshake_bytes = bincode::serialize(&handshake)
            .map_err(|e| anyhow!("Failed to serialize handshake: {}", e))?;

        let mut stream = stream;
        stream.write_all(&(handshake_bytes.len() as u32).to_be_bytes()).await?;
        stream.write_all(&handshake_bytes).await?;

        info!("✅ Handshake sent to {}", hex::encode(&peer.id.0[..8]));

        // Создаём соединение
        let conn_id = {
            let mut id = self.next_conn_id.lock().await;
            let cid = *id;
            *id += 1;
            cid
        };

        let tcp_conn = TcpConnection::new(conn_id, peer.id, stream);

        // Создаём session
        let session = TcpPeerSession::new(peer.id, station.clone());
        let mut peers_guard = self.peers.lock().await;
        peers_guard.insert(peer.id, session);

        // Устанавливаем callback
        let incoming_callback = self.incoming_callback.clone();
        station.set_data_callback(move |source: HashId, data: Vec<u8>| {
            debug!("📨 Data callback from {}: {} bytes", hex::encode(&source.0[..8]), data.len());

            if let Ok(guard) = incoming_callback.try_lock() {
                if let Some(cb) = guard.as_ref() {
                    cb(source, data);
                }
            }
        }).await;

        info!("✅ Connected to {}", hex::encode(&peer.id.0[..8]));

        Ok(())
    }

    /// Отправить данные peer'у
    pub async fn send_to(&self, peer_id: HashId, data: Vec<u8>) -> Result<()> {
        let peers_guard = self.peers.lock().await;

        if let Some(session) = peers_guard.get(&peer_id) {
            session.send_data(data).await?;
            Ok(())
        } else {
            Err(anyhow!("No session for peer {}", hex::encode(&peer_id.0[..8])))
        }
    }

    /// Установить callback для входящих данных
    pub async fn set_incoming_callback<F>(&self, callback: F)
    where
        F: Fn(HashId, Vec<u8>) + Send + Sync + 'static
    {
        let mut cb = self.incoming_callback.lock().await;
        *cb = Some(Box::new(callback));
    }

    /// Получить статистику
    pub async fn get_stats(&self) -> TcpTransportStats {
        let peers_guard = self.peers.lock().await;

        TcpTransportStats {
            active_peers: peers_guard.len(),
            bind_port: self.bind_port,
        }
    }
}

/// Handshake packet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakePacket {
    /// Peer ID
    pub peer_id: HashId,

    /// Timestamp
    pub timestamp_ms: u64,
}

/// Статистика TCP транспорта
#[derive(Debug, Clone)]
pub struct TcpTransportStats {
    pub active_peers: usize,
    pub bind_port: u16,
}
