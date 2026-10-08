// src/p2p/transport.rs
//! P2P Transport Layer for Communication
//! =====================================
//!
//! Выделенный транспорт для P2P коммуникаций с большими пакетами:
//! - Port 9000: Hello/Discovery (ОБЩИЙ с netlayer transport)
//! - Port 9998: P2P Data session (MTU 65536)
//!
//! ## Отличия от netlayer/transport.rs:
//! - **MTU: 65536** вместо 1200
//! - **Data port: 9998** вместо 10000
//! - **Пакеты: 0xA0-0xDF** (Communication) вместо 0x30-0x5F (Proxy)
//! - **Без прокси** - только Chat, Files, Voice, Video

use crate::util::HashId;
use crate::core::NodeIdentity;
use crate::p2p::{P2PNatStatus, P2PPacket, P2PPacketType, P2PPeer, P2P_PACKET_HEADER_LEN};
use crate::communication::{CommPacket, CommControlPacket};
use crate::p2p::hello::{P2PHelloPacket, P2PHelloType};
use crate::p2p::encryption_manager::EncryptionManager as P2PEncryptionManager;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc, broadcast};
use tracing::debug;

#[path = "punch.rs"]
mod punch;
#[path = "relay.rs"]
mod relay;

/// 📦 Кэш пакетов для сборки DUAL-PATH (аналог Depot из netlayer)
struct PacketCache {
    /// Пакеты в процессе сборки: packet_id -> PendingPacket
    packets: std::collections::HashMap<u64, PendingPacket>,
    /// Максимальный размер кэша в байтах (16 MB)
    max_bytes: usize,
    /// Текущий размер в байтах
    current_bytes: usize,
    /// Last time expired entries were swept
    last_prune: std::time::Instant,
}

/// Limits for packets that arrive BEFORE they are authenticated: how long and how many entries are kept, how many fragments one
/// packet may have.
const PENDING_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const MAX_PENDING_PACKETS: usize = 20_000;
const MAX_FRAGMENTS: u32 = 64;
/// Largest reassembled packet (the wire format carries the payload length in 16 bits).
const MAX_ASSEMBLED: usize = u16::MAX as usize;

/// Пакет в процессе сборки
struct PendingPacket {
    sender: HashId,
    total_parts: u32,
    parts: std::collections::HashMap<u32, Vec<u8>>,
    last_update: std::time::Instant,
    /// Оригиналы (is_clone=false)
    originals: std::collections::HashMap<u32, Vec<u8>>,
    /// Клоны (is_clone=true)
    clones: std::collections::HashMap<u32, Vec<u8>>,
    /// Какие номера уже получены (не важно оригинал или клон)
    received: std::collections::HashSet<u32>,
}

impl PacketCache {
    /// Создать новый кэш с лимитом max_bytes
    fn new(max_bytes: usize) -> Self {
        Self {
            packets: std::collections::HashMap::new(),
            max_bytes,
            current_bytes: 0,
            last_prune: std::time::Instant::now(),
        }
    }

    /// Bytes of payload held by one pending packet.
    fn stored(p: &PendingPacket) -> usize {
        p.parts.values().chain(p.originals.values()).chain(p.clones.values()).map(|v| v.len()).sum()
    }

    fn drop_entry(&mut self, packet_id: u64) {
        if let Some(p) = self.packets.remove(&packet_id) {
            self.current_bytes = self.current_bytes.saturating_sub(Self::stored(&p));
        }
    }

    /// Make room for `extra` more payload bytes (and one more entry): expired entries are swept once a second, and when
    /// the cache is still full the oldest entries go. Returns false only if the cache cannot take the packet at all.
    fn admit(&mut self, extra: usize) -> bool {
        if self.last_prune.elapsed() >= std::time::Duration::from_secs(1) {
            self.last_prune = std::time::Instant::now();
            let expired: Vec<u64> = self.packets.iter().filter(|(_, p)| p.last_update.elapsed() > PENDING_TTL).map(|(id, _)| *id).collect();
            for id in expired {
                self.drop_entry(id);
            }
        }
        let mut guard = 0;
        while (self.packets.len() >= MAX_PENDING_PACKETS || self.current_bytes + extra > self.max_bytes) && !self.packets.is_empty() && guard < 64 {
            let oldest = self.packets.iter().min_by_key(|(_, p)| p.last_update).map(|(id, _)| *id);
            match oldest {
                Some(id) => self.drop_entry(id),
                None => break,
            }
            guard += 1;
        }
        self.packets.len() < MAX_PENDING_PACKETS && self.current_bytes + extra <= self.max_bytes
    }

    fn add_packet(&mut self, packet_id: u64, sender: HashId, seq_num: u32, total_parts: u32, data: Vec<u8>) -> Option<Vec<u8>> {
        if total_parts == 0 {
            return Some(data);
        }

        use std::collections::hash_map::Entry;

        // Проверяем лимит ДО вставки
        let size_estimate = std::mem::size_of::<u64>() + std::mem::size_of::<HashId>() + (total_parts as usize * 128);
        if self.current_bytes + size_estimate > self.max_bytes {
            self.evict_oldest();
        }

        let pending = match self.packets.entry(packet_id) {
            Entry::Occupied(occ) => occ.into_mut(),
            Entry::Vacant(vac) => {
                vac.insert(PendingPacket {
                    sender,
                    total_parts,
                    parts: std::collections::HashMap::new(),
                    last_update: std::time::Instant::now(),
                    originals: std::collections::HashMap::new(),
                    clones: std::collections::HashMap::new(),
                    received: std::collections::HashSet::new(),
                })
            }
        };

        pending.last_update = std::time::Instant::now();
        pending.parts.insert(seq_num, data);

        if pending.parts.len() == pending.total_parts as usize {
            let mut full_data = Vec::new();
            for i in 0..pending.total_parts {
                if let Some(part) = pending.parts.remove(&i) {
                    full_data.extend(part);
                }
            }
            self.packets.remove(&packet_id);
            return Some(full_data);
        }

        None
    }


    /// Очистить старые пакеты при переполнении
    fn evict_oldest(&mut self) {
        let oldest = self.packets.iter()
            .min_by_key(|(_, p)| p.last_update)
            .map(|(id, _)| *id);
        if let Some(id) = oldest {
            if let Some(p) = self.packets.remove(&id) {
                let size_estimate = std::mem::size_of::<u64>() + (p.total_parts as usize * 128);
                self.current_bytes = self.current_bytes.saturating_sub(size_estimate);
            }
        }
    }
}



/// P2P Transport Manager
///
/// Управляет P2P коммуникациями:
/// - Port 9998: P2P Data (MTU 65536) - для файлов, чата, голоса, видео
/// - Port 9000: Общий discovery (с netlayer transport)
/// Порты канала связи (данные, знакомство): по умолчанию 9998 и 9001, для нескольких узлов на одном компьютере —
/// `YANDI_P2P_DATA_PORT` / `YANDI_P2P_DISCOVERY_PORT`.
pub fn configured_ports() -> (u16, u16) {
    let get = |name: &str, default: u16| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    (get("YANDI_P2P_DATA_PORT", 9998), get("YANDI_P2P_DISCOVERY_PORT", 9001))
}

#[derive(Clone)]
pub struct P2PTransport {
    /// Node identity
    identity: Arc<NodeIdentity>,

    /// P2P Data socket (port 9998) - receive
    data_recv_socket: Arc<UdpSocket>,

    /// P2P Data socket (port 9998) - send
    data_send_socket: Arc<UdpSocket>,
    /// P2P Discovery socket (port 9001) - independent from netlayer
    /// External IP address for P2P data
    external_ip: String,
    discovery_socket: Arc<UdpSocket>,

    /// Known peers
    peers: Arc<Mutex<HashMap<HashId, P2PPeer>>>,

    /// Node capabilities
    capabilities: u16,

    /// Chat packet sender (0xA0-0xAF)
    chat_packet_tx: Option<mpsc::Sender<(HashId, CommPacket)>>,

    /// P2P tunnel packet sender
    p2p_tunnel_tx: Option<mpsc::Sender<(HashId, Vec<u8>)>>,

    /// Media signaling/data sender for WebRTC signaling over dedicated P2P transport
    media_signal_tx: Option<mpsc::Sender<(HashId, P2PPacketType, Vec<u8>)>>,

    /// 🔄 Dual-path: полученные packet_id (для дедупликации)
    packet_cache: Arc<Mutex<PacketCache>>,
    /// P2P encryption manager
    p2p_encryption: Arc<Mutex<P2PEncryptionManager>>,

    /// SEC-10: IP → expected Ed25519 public key for bootstrap nodes with pinned fingerprints
    bootstrap_fingerprints: Arc<std::sync::RwLock<HashMap<String, [u8; 32]>>>,

    /// SEC-11: (node_id, nonce) pairs already accepted within the freshness
    /// window — this Hello layer (separate from netlayer::transport's own
    /// Hello, which had and had fixed the identical gap) had NO timestamp
    /// or nonce check at all: verify_signature() only proves internal
    /// self-consistency, not freshness. A captured, validly-signed Hello
    /// could be replayed later from a different address to hijack a
    /// peer's registered addr/data_addr (every accepted Hello below
    /// unconditionally overwrites the peer table entry). See
    /// `identity_conflict`/`check_replay` in netlayer::transport for the
    /// original writeup of this exact bug class.
    seen_hello_nonces: Arc<Mutex<HashMap<(HashId, u64), std::time::Instant>>>,
    /// Допуск новых узлов (ограничения на наплыв поддельных личностей, см. netlayer::admission)
    admission: Arc<Mutex<crate::netlayer::admission::Admission>>,
    /// Когда в последний раз просили новый ключ у узла (не чаще раза в минуту)
    rekey_requested: Arc<Mutex<HashMap<HashId, std::time::Instant>>>,
    /// when a broken session with a peer was last repaired by a new handshake (see `request_resync`)
    resync_requested: Arc<Mutex<HashMap<HashId, std::time::Instant>>>,
    /// hole punching through introducers (src/p2p/punch.rs)
    punch: Arc<punch::PunchState>,
    /// a UDP relay for the pairs that cannot be connected directly (src/p2p/relay.rs)
    relay: Arc<relay::RelayState>,
    /// this transport itself, for background tasks that need to call back into it
    self_ref: std::sync::OnceLock<std::sync::Weak<P2PTransport>>,

    /// Statistics
    stats_sent_packets: Arc<AtomicU64>,
    stats_recv_packets: Arc<AtomicU64>,
    stats_sent_bytes: Arc<AtomicU64>,
    stats_recv_bytes: Arc<AtomicU64>,

    /// 🚂 Path0 statistics
    stats_sent_path0: Arc<AtomicU64>,
    stats_recv_path0: Arc<AtomicU64>,

    /// 🚂 Path1 statistics
    stats_sent_path1: Arc<AtomicU64>,
    stats_recv_path1: Arc<AtomicU64>,
}

impl P2PTransport {
    /// Create new P2P transport (port 9999, MTU 65536)
    pub async fn new(
        identity: NodeIdentity,
        capabilities: u16,
    ) -> Result<Arc<Self>, String> {
        Self::with_handlers(identity, capabilities, None, None, None, "0.0.0.0".to_string()).await
    }

    /// Create P2P transport with channel handlers
    pub async fn with_handlers(
        identity: NodeIdentity,
        capabilities: u16,
        chat_packet_tx: Option<mpsc::Sender<(HashId, CommPacket)>>,
        p2p_tunnel_tx: Option<mpsc::Sender<(HashId, Vec<u8>)>>,
        media_signal_tx: Option<mpsc::Sender<(HashId, P2PPacketType, Vec<u8>)>>,
        external_ip: String,
    ) -> Result<Arc<Self>, String> {
        let node_id = identity.node_id();
        // Real Two-Node P2P E2E test mandate: these two ports were
        // hardcoded, making it impossible to run two YANDI nodes on one
        // machine. No config field exists for this transport (it's
        // separate from netlayer::transport's discovery/data ports), so
        // — minimal, no schema change — an env var override, defaulting
        // to the original hardcoded values for every existing deployment.
        let (data_port, discovery_port) = configured_ports();

        // Bind P2P data socket
        let data_socket = UdpSocket::bind(format!("0.0.0.0:{data_port}"))
            .await
            .map_err(|e| format!("Failed to bind P2P data socket on port {data_port}: {}", e))?;

        let data_socket = Arc::new(data_socket);
        // Bind P2P discovery socket
        let discovery_socket = UdpSocket::bind(format!("0.0.0.0:{discovery_port}"))
            .await
            .map_err(|e| format!("Failed to bind P2P discovery socket on port {discovery_port}: {}", e))?;
        let discovery_socket = Arc::new(discovery_socket);

        println!("   Discovery: {}", discovery_socket.local_addr().unwrap());

        println!("📡 P2P Transport:");
        println!("   Data: {}", data_socket.local_addr().unwrap());
        println!("   MTU: 65536 bytes (64 KB) - Chat, Files, Voice, Video");
        println!("   🔄 Dual-Path: Path0 + Path1 redundant transmission");
        println!();

        let transport = Arc::new(Self {
            identity: Arc::new(identity),
            data_recv_socket: data_socket.clone(),
            data_send_socket: data_socket,
            discovery_socket: discovery_socket.clone(),
            external_ip: external_ip,
            peers: Arc::new(Mutex::new(HashMap::new())),
            capabilities,
            chat_packet_tx,
            p2p_tunnel_tx,
            media_signal_tx,
            packet_cache: Arc::new(Mutex::new(PacketCache::new(16 * 1024 * 1024))),
            p2p_encryption: Arc::new(Mutex::new(P2PEncryptionManager::new(node_id))),
            bootstrap_fingerprints: Arc::new(std::sync::RwLock::new(HashMap::new())),
            seen_hello_nonces: Arc::new(Mutex::new(HashMap::new())),
            rekey_requested: Arc::new(Mutex::new(HashMap::new())),
            resync_requested: Arc::new(Mutex::new(HashMap::new())),
            punch: Arc::new(punch::PunchState::default()),
            relay: Arc::new(relay::RelayState::default()),
            self_ref: std::sync::OnceLock::new(),
            admission: Arc::new(Mutex::new(crate::netlayer::admission::Admission::new(crate::netlayer::admission::AdmissionConfig::default()))),
            stats_sent_packets: Arc::new(AtomicU64::new(0)),
            stats_recv_packets: Arc::new(AtomicU64::new(0)),
            stats_sent_bytes: Arc::new(AtomicU64::new(0)),
            stats_recv_bytes: Arc::new(AtomicU64::new(0)),
            stats_sent_path0: Arc::new(AtomicU64::new(0)),
            stats_recv_path0: Arc::new(AtomicU64::new(0)),
            stats_sent_path1: Arc::new(AtomicU64::new(0)),
            stats_recv_path1: Arc::new(AtomicU64::new(0)),
        });

        let _ = transport.self_ref.set(Arc::downgrade(&transport));

        // Spawn receive loop
        let transport_clone = transport.clone();
        tokio::spawn(async move {
            transport_clone.receive_loop().await;
        });
        // Spawn discovery listener for P2P handshake
        let transport_clone2 = transport.clone();
        tokio::spawn(async move {
            transport_clone2.discovery_listener().await;
        });

        // Keep the doors in the NAT open: a node behind NAT hears from the outside only while its NAT remembers a conversation, and an idle
        // UDP flow is forgotten within half a minute. A tiny authenticated packet to every known peer keeps the memory alive, and each one
        // also tells the peer our current outside address.
        let keep = Arc::downgrade(&transport);
        crate::supervisor::supervise("p2p_keepalive", crate::supervisor::Policy::Log, move || {
            let keep = keep.clone();
            async move {
                loop {
                    tokio::time::sleep(KEEPALIVE_EVERY).await;
                    let Some(t) = keep.upgrade() else { return };
                    t.relay_housekeeping().await;
                let ids: Vec<HashId> = t.peers.lock().await.keys().copied().collect();
                    for id in ids {
                        let pkt = P2PPacket::new(P2PPacketType::ChatTyping, t.identity.node_id(), false, Vec::new());
                        let _ = t.send_packet_dual_path(id, pkt).await;
                    }
                }
            }
        });

        Ok(transport)
    }

    /// Get discovery address — was hardcoded to the wrong, unrelated
    /// "0.0.0.0:9000" (not even this module's own 9001 default) instead of
    /// reflecting the actual bound socket; harmless as long as nothing
    /// called it, but a real bug for any caller (found while adding a live
    /// test for this module — see p2p_hello_replay_test.rs).
    pub fn discovery_addr(&self) -> String {
        self.discovery_socket.local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "0.0.0.0:9001".to_string())
    }
    /// Куда слать данные этому узлу: внешний адрес и порт, на котором на самом деле открыт приём (был жёстко 9998 — копия узла с
    /// другим портом называла соседям чужой порт).
    pub fn data_addr(&self) -> String {
        let port = self.data_recv_socket.local_addr().map(|a| a.port()).unwrap_or(configured_ports().0);
        format!("{}:{}", self.external_ip, port)
    }

    /// Get node ID
    pub fn node_id(&self) -> HashId {
        self.identity.node_id()
    }

    /// Get short ID (first 8 bytes)
    pub fn short_id(&self) -> String {
        hex::encode(&self.identity.node_id().0[..8])
    }

    /// Add or update peer
    /// Сколько приветствий отклонил допуск / пропустил с запуска.
    pub async fn hellos_refused(&self) -> u64 {
        self.admission.lock().await.rejected_total()
    }

    pub async fn hellos_admitted(&self) -> u64 {
        self.admission.lock().await.allowed_total()
    }

    pub async fn add_peer(&self, peer: P2PPeer) {
        let mut peers = self.peers.lock().await;
        peers.insert(peer.id, peer);
    }

    /// Get peer by ID
    pub async fn get_peer(&self, peer_id: &HashId) -> Option<P2PPeer> {
        let peers = self.peers.lock().await;
        peers.get(peer_id).cloned()
    }

    /// Snapshot of known P2P peers.
    pub async fn list_peers(&self) -> Vec<P2PPeer> {
        let peers = self.peers.lock().await;
        peers.values().cloned().collect()
    }

    /// Find peer by short ID (async version)
    pub async fn find_peer_by_short_id(&self, short_id: &str) -> Option<HashId> {
        let peers = self.peers.lock().await;

        // Linear search through peers
        for (peer_id, _peer) in peers.iter() {
            let peer_short_id = hex::encode(&peer_id.0[..8]);
            if peer_short_id == short_id {
                return Some(*peer_id);
            }
        }

        None
    }


    /// Прежний «зашифрованный» путь туннелей этого канала (p2p_tunnel): на деле слал данные ОТКРЫТЫМ текстом, завёрнутыми как
    /// сообщение чата, и у получателя они попадали в чат, а не в туннель — между узлами туннель не работал ни разу. Отправка открытым
    /// текстом отключена; туннели этого канала ждут решения владельца (починить поверх шифрования или убрать).
    pub async fn send_encrypted(&self, peer_id: HashId, _data: &[u8]) -> Result<(), String> {
        Err(format!("туннель канала переписки отключён: он слал данные незашифрованными (узел {})", hex::encode(&peer_id.0[..8])))
    }

    /// 🔄 Отправить пакет по DUAL-PATH (Path0 + Path1)
    /// Создаёт 2 копии пакета с разными line_id
    pub async fn send_packet_dual_path(&self, peer_id: HashId, mut packet: P2PPacket) -> Result<(), String> {
        // Найти peer address
        let peer_addr = {
            let peers = self.peers.lock().await;
            let peer = peers.get(&peer_id)
                .ok_or_else(|| format!("Peer not found: {}", hex::encode(&peer_id.0[..8])))?;

            peer.p2p_data_addr.clone()
                .ok_or_else(|| format!("Peer {} has no P2P data address", hex::encode(&peer_id.0[..8])))?
        };

        // SEC-01: только зашифрованным. Раньше без общего ключа (или при сбое шифрования) пакет уходил ОТКРЫТЫМ текстом — «лучше
        // отправить, чем потерять»; теперь — отказ, а к собеседнику узел заново стучится, чтобы выработать ключ.
        let rekey_due = { self.p2p_encryption.lock().await.needs_rekey(&peer_id) };
        let sealed = {
            let enc = self.p2p_encryption.lock().await;
            if enc.has_session(&peer_id) {
                // Inside the encrypted part: marker, the packet type (so it cannot be changed on the way) and the original length.
                let prefixed = seal_prefix(packet.packet_type.to_byte(), &packet.payload);
                Some(enc.encrypt(&peer_id, &prefixed))
            } else {
                None
            }
        };
        match sealed {
            Some(Ok(ciphertext)) => {
                packet.payload = ciphertext;
                packet.encrypted = true;
                // Ключ состарился (час или очень много пакетов): договариваемся о новом заранее, старый ещё принимает пакеты в пути
                if rekey_due {
                    let ask = {
                        let mut asked = self.rekey_requested.lock().await;
                        let recently = asked.get(&peer_id).map(|t| t.elapsed() < std::time::Duration::from_secs(60)).unwrap_or(false);
                        if !recently {
                            asked.insert(peer_id, std::time::Instant::now());
                            if asked.len() > 4096 {
                                asked.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(120));
                            }
                        }
                        !recently
                    };
                    if ask {
                        if let Some(addr) = self.peers.lock().await.get(&peer_id).map(|p| p.addr.clone()) {
                            println!("[P2P] 🔑 session key with {} is old — asking for a new one", hex::encode(&peer_id.0[..8]));
                            let _ = self.send_hello_request(&addr).await;
                        }
                    }
                }
            }
            Some(Err(e)) => {
                return Err(format!("Encryption failed for {}: {} — not sent", hex::encode(&peer_id.0[..8]), e));
            }
            None => {
                let knock = self.peers.lock().await.get(&peer_id).map(|p| p.addr.clone());
                if let Some(addr) = knock {
                    let _ = self.send_hello_request(&addr).await;
                }
                return Err(format!("No secure session with {} yet — not sent (handshake requested)", hex::encode(&peer_id.0[..8])));
            }
        }

        // 🔄 Path0: оригинал (line_id=0, is_clone=false)
        packet.line_id = 0;
        packet.is_clone = false;
        let bytes0 = packet.to_bytes();

        self.data_send_socket.send_to(&bytes0, &peer_addr)
            .await
            .map_err(|e| format!("Failed to send Path0 packet to {}: {}", peer_addr, e))?;
        println!("[P2P] 📤 Path0 → {} ({}B, ts: {})", peer_addr, bytes0.len(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());

        self.stats_sent_packets.fetch_add(1, Ordering::Relaxed);
        self.stats_sent_bytes.fetch_add(bytes0.len() as u64, Ordering::Relaxed);
        self.stats_sent_path0.fetch_add(1, Ordering::Relaxed);

        // 🔄 Path1: клон (line_id=1, is_clone=true)
        packet.line_id = 1;
        packet.is_clone = true;
        let bytes1 = packet.to_bytes();

        self.data_send_socket.send_to(&bytes1, &peer_addr)
            .await
            .map_err(|e| format!("Failed to send Path1 packet to {}: {}", peer_addr, e))?;

        self.stats_sent_packets.fetch_add(1, Ordering::Relaxed);
        self.stats_sent_bytes.fetch_add(bytes1.len() as u64, Ordering::Relaxed);
        println!("[P2P] 📤 Path1 → {} ({}B, ts: {})", peer_addr, bytes1.len(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
        self.stats_sent_path1.fetch_add(1, Ordering::Relaxed);

        println!("[P2P] 🔄 DUAL-PATH: Path0 + Path1 → {} ({} bytes)",
                 hex::encode(&peer_id.0[..8]), bytes0.len());

        Ok(())
    }

    /// Receive loop - обрабатывает входящие пакеты с DUAL-PATH сборкой
    async fn receive_loop(&self) {
        println!("[P2P] 🟢 RECEIVE_LOOP STARTED on port 9998");
        let mut buf = vec![0u8; 65536]; // MTU 65536 matches documented MTU

        loop {
            match self.data_recv_socket.recv_from(&mut buf).await {
                Ok((len, from_addr)) => {
                        static LAST_RECV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                        let prev = LAST_RECV.swap(now, std::sync::atomic::Ordering::Relaxed);
                        if prev != 0 && now.saturating_sub(prev) > 1000 {
                            println!("[P2P] ⚠️ Gap in receive: {} ms", now - prev);
                        }
                    // Парсим P2PPacket из сырых байт
                    if let Some(p2p_packet) = P2PPacket::from_bytes(&buf[..len]) {
                        let packet_id = p2p_packet.packet_id;
                        let seq_num = p2p_packet.seq_num;
                        let total_parts = p2p_packet.total_parts;
                        let sender = p2p_packet.sender;
                        let payload = p2p_packet.payload.clone();

                        // These numbers come from an unauthenticated sender: a packet is at most MAX_FRAGMENTS parts and its part
                        // number must be inside it (otherwise the completeness count and the assembly loop can be steered by the sender).
                        if total_parts > MAX_FRAGMENTS || (total_parts > 0 && seq_num >= total_parts) {
                            continue;
                        }

                        // Обычные одиночные пакеты не требуют сборки по частям.
                        // Здесь нужна только дедупликация dual-path, чтобы Path1 не вызывал
                        // повторную доставку того же сообщения.
                        if total_parts == 0 {
                            let is_new = {
                                let mut cache = self.packet_cache.lock().await;
                                if !cache.packets.contains_key(&packet_id) {
                                    let _ = cache.admit(0); // makes room (expired / oldest entries go); single packets store no payload
                                }
                                use std::collections::hash_map::Entry;

                                match cache.packets.entry(packet_id) {
                                    Entry::Occupied(mut entry) => {
                                        let pending = entry.get_mut();
                                        let already_received = pending.received.contains(&0);

                                        if p2p_packet.is_clone {
                                            pending.clones.insert(0, Vec::new());
                                        } else {
                                            pending.originals.insert(0, Vec::new());
                                        }

                                        pending.received.insert(0);
                                        pending.last_update = std::time::Instant::now();
                                        !already_received
                                    }
                                    Entry::Vacant(entry) => {
                                        let mut pending = PendingPacket {
                                            sender,
                                            total_parts: 0,
                                            parts: std::collections::HashMap::new(),
                                            last_update: std::time::Instant::now(),
                                            originals: std::collections::HashMap::new(),
                                            clones: std::collections::HashMap::new(),
                                            received: std::collections::HashSet::new(),
                                        };

                                        if p2p_packet.is_clone {
                                            pending.clones.insert(0, Vec::new());
                                        } else {
                                            pending.originals.insert(0, Vec::new());
                                        }
                                        pending.received.insert(0);
                                        entry.insert(pending);
                                        true
                                    }
                                }
                            };

                            if is_new {
                                self.stats_recv_packets.fetch_add(1, Ordering::Relaxed);
                                self.stats_recv_bytes.fetch_add(len as u64, Ordering::Relaxed);

                                if p2p_packet.line_id == 0 {
                                    self.stats_recv_path0.fetch_add(1, Ordering::Relaxed);
                                } else if p2p_packet.line_id == 1 {
                                    self.stats_recv_path1.fetch_add(1, Ordering::Relaxed);
                                }

                                if let Err(e) = self.handle_packet(&buf[..len], from_addr).await {
                                    eprintln!("❌ [P2P] Error handling single packet: {}", e);
                                }
                            } else if p2p_packet.line_id == 1 {
                                self.stats_recv_path1.fetch_add(1, Ordering::Relaxed);
                            }

                            continue;
                        }

                        // 🔄 Dual-path: сохраняем оригиналы и клоны как в Train

                        let is_new = {

                            let mut cache = self.packet_cache.lock().await;

                            // an unauthenticated fragment is kept only while the cache has room for it
                            if !cache.admit(payload.len()) {
                                continue;
                            }
                            let bytes_before = cache.packets.get(&packet_id).map(PacketCache::stored).unwrap_or(0);

                            use std::collections::hash_map::Entry;

                            let outcome = match cache.packets.entry(packet_id) {

                                Entry::Occupied(mut entry) => {

                                    let pending = entry.get_mut();

                                    let seq = seq_num;
                                    let already_received = pending.received.contains(&seq);

                                    let is_clone = p2p_packet.is_clone;

                                    if is_clone {

                                        pending.clones.insert(seq, payload.clone());

                                    } else {

                                        pending.originals.insert(seq, payload.clone());

                                    }

                                    pending.received.insert(seq);

                                    pending.last_update = std::time::Instant::now();

                                    // Новый если этот seq_num еще не был получен
                                    !already_received

                                }

                                Entry::Vacant(entry) => {

                                    let mut pending = PendingPacket {

                                        sender,

                                        total_parts,

                                        parts: std::collections::HashMap::new(),

                                        last_update: std::time::Instant::now(),

                                        originals: std::collections::HashMap::new(),

                                        clones: std::collections::HashMap::new(),

                                        received: std::collections::HashSet::new(),

                                    };

                                    let seq = seq_num;

                                    let is_clone = p2p_packet.is_clone;

                                    if is_clone {

                                        pending.clones.insert(seq, payload.clone());

                                    } else {

                                        pending.originals.insert(seq, payload.clone());

                                    }

                                    pending.received.insert(seq);

                                    entry.insert(pending);

                                    true

                                }

                            };
                            // account for what this fragment added (a repeated part replaces the old bytes)
                            let bytes_after = cache.packets.get(&packet_id).map(PacketCache::stored).unwrap_or(0);
                            cache.current_bytes = (cache.current_bytes + bytes_after).saturating_sub(bytes_before);
                            outcome
                        };



                        if !is_new {

                            // Уже получали этот seq_num - дубль

                            if len >= 38 {

                                let line_id = buf[36];

                                if line_id == 1 {

                                    self.stats_recv_path1.fetch_add(1, Ordering::Relaxed);

                                }

                            }
                        }


                        // Проверяем собраны ли все части (оригиналы + клоны)

                        let total_received = {

                            let cache = self.packet_cache.lock().await;

                            if let Some(pending) = cache.packets.get(&packet_id) {

                                pending.received.len()

                            } else {

                                0

                            }

                        };



                        if total_received == total_parts as usize {

                            // Все части получены - собираем данные

                            let mut complete_data = Vec::new();

                            for i in 0..total_parts {

                                let data = {

                                    let cache = self.packet_cache.lock().await;

                                    if let Some(pending) = cache.packets.get(&packet_id) {

                                        if let Some(data) = pending.originals.get(&i) {

                                            data.clone()

                                        } else if let Some(data) = pending.clones.get(&i) {

                                            data.clone()

                                        } else {

                                            continue;

                                        }

                                    } else {

                                        continue;

                                    }

                                };

                                complete_data.extend_from_slice(&data);

                            }



                            if complete_data.len() > MAX_ASSEMBLED {
                                // cannot be a real packet (the wire format holds 16-bit payload lengths): drop it instead of panicking later
                                self.packet_cache.lock().await.drop_entry(packet_id);
                                continue;
                            }
                            if !complete_data.is_empty() {

                                self.stats_recv_packets.fetch_add(1, Ordering::Relaxed);

                                self.stats_recv_bytes.fetch_add(len as u64, Ordering::Relaxed);



                                let assembled_packet = P2PPacket {

                                    packet_type: p2p_packet.packet_type,

                                    sender,

                                    encrypted: p2p_packet.encrypted,

                                    is_clone: false,

                                    line_id: p2p_packet.line_id,

                                    packet_id,

                                    seq_num: 0,

                                    total_parts: 0,

                                    payload: complete_data,

                                };

                                let assembled_bytes = assembled_packet.to_bytes();

                                if let Err(e) = self.handle_packet(&assembled_bytes, from_addr).await {

                                    eprintln!("❌ [P2P] Error handling packet: {}", e);

                                }



                                // Очищаем кэш

                                let mut cache = self.packet_cache.lock().await;

                                cache.drop_entry(packet_id);

                            }

                        }

                    } else {
                        eprintln!("[P2P] ⚠️  Failed to parse packet from {}", from_addr);
                    }
                }
                Err(e) => {
                    eprintln!("❌ [P2P] Receive error: {}", e);
                }
            }
        }
    }
    /// Discovery listener for P2P handshake on port 9001
    /// SEC-11: true (fresh, accept) the first time a (node_id, nonce) pair
    /// is seen; false (replay, reject) on any repeat. Evicts entries older
    /// than 5 minutes opportunistically — matching netlayer::transport's
    /// own freshness window — so the map never grows unbounded.
    fn check_replay(
        seen: &mut HashMap<(HashId, u64), std::time::Instant>,
        node_id: HashId,
        nonce: u64,
    ) -> bool {
        let now = std::time::Instant::now();
        seen.retain(|_, seen_at| now.duration_since(*seen_at) < std::time::Duration::from_secs(5 * 60));
        if seen.contains_key(&(node_id, nonce)) {
            return false;
        }
        seen.insert((node_id, nonce), now);
        true
    }

    async fn discovery_listener(self: Arc<Self>) {
        let socket = self.discovery_socket.clone();
        let mut buf = vec![0u8; 4096];
        println!("[P2P] 📡 Starting discovery listener on port 9001");
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, from)) => {
                    let data = &buf[..len];
                    if len == punch::PROBE_LEN && data[0] == punch::PROBE_MAGIC {
                        self.on_punch_probe(data, from, true).await;
                        continue;
                    }
                    match P2PHelloPacket::from_bytes(data) {
                        Ok(hello) => {
                            println!("[P2P] 📨 Received P2P Hello from {}: {:?}", from, hello.hello_type);
                            // SEC-03: reject Hello packets with invalid signatures
                            if let Err(e) = hello.verify_signature() {
                                eprintln!("[P2P] ⚠️ Hello from {} rejected — bad signature: {}", from, e);
                                continue;
                            }

                            // Допуск: подписанное приветствие от неизвестной личности ничего не доказывает (личность создаётся бесплатно).
                            {
                                let (known, table_len) = {
                                    let p = self.peers.lock().await;
                                    (p.contains_key(&hello.node_id), p.len())
                                };
                                let verdict = self.admission.lock().await.check(from.ip(), hello.node_id, known, table_len, std::time::Instant::now());
                                let verdict = match verdict {
                                    Err(crate::netlayer::admission::Reject::TableFull) => {
                                        let mut p = self.peers.lock().await;
                                        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                                        let victim = crate::netlayer::admission::pick_victim(p.values().map(|x| (x.id, x.last_seen, false, false)), now_ms, 120_000);
                                        match victim {
                                            Some(v) => {
                                                p.remove(&v);
                                                Ok(())
                                            }
                                            None => Err(crate::netlayer::admission::Reject::TableFull),
                                        }
                                    }
                                    other => other,
                                };
                                if let Err(reason) = verdict {
                                    let total = self.admission.lock().await.rejected_total();
                                    if total.is_power_of_two() || total % 1000 == 0 {
                                        eprintln!("[P2P] 🚧 Hello refused ({}): {} refused in total", reason.name(), total);
                                    }
                                    continue;
                                }
                            }

                            // SEC-11: a valid signature only proves this Hello is
                            // internally self-consistent — not that it was signed
                            // just now rather than captured once and replayed
                            // later. Reject any (node_id, nonce) already accepted.
                            {
                                let mut seen = self.seen_hello_nonces.lock().await;
                                if !Self::check_replay(&mut seen, hello.node_id, hello.nonce) {
                                    eprintln!("[P2P] ⛔ REJECTED: REPLAY — Hello for {} reuses a nonce already accepted within the freshness window (from {})", hex::encode(&hello.node_id.0[..8]), from);
                                    continue;
                                }
                            }

                            // SEC-12: a signature only proves THIS Hello's own
                            // embedded key signed THIS Hello — never checked
                            // against what key this node_id previously proved
                            // ownership with. Without this, any new signing key
                            // can silently steal an existing peer's node_id (the
                            // general case; SEC-10 above only covers the narrow
                            // case of a pinned bootstrap IP).
                            {
                                let peers = self.peers.lock().await;
                                if let Some(existing) = peers.get(&hello.node_id) {
                                    if let Some(known_key) = existing.ed25519_public {
                                        if known_key != hello.ed25519_public {
                                            eprintln!("[P2P] ⛔ REJECTED: IDENTITY_CONFLICT — {} previously verified with a different signing key (from {})", hex::encode(&hello.node_id.0[..8]), from);
                                            continue;
                                        }
                                    }
                                }
                            }

                            // SEC-10: if this source IP has a pinned bootstrap fingerprint,
                            // verify the Ed25519 key matches — prevents bootstrap impersonation
                            {
                                let from_ip = from.ip().to_string();
                                if let Ok(fps) = self.bootstrap_fingerprints.read() {
                                    if let Some(expected) = fps.get(&from_ip) {
                                        if &hello.ed25519_public != expected {
                                            eprintln!(
                                                "[P2P] ❌ BOOTSTRAP FINGERPRINT MISMATCH from {} — \
                                                 expected {}, got {}. Rejecting.",
                                                from,
                                                hex::encode(&expected[..8]),
                                                hex::encode(&hello.ed25519_public[..8])
                                            );
                                            continue;
                                        }
                                        println!("[P2P] 🔒 Bootstrap fingerprint verified for {}", from_ip);
                                    }
                                }
                            }
                            match hello.hello_type {
                                P2PHelloType::Request => {
                                    let peer_id = hello.node_id;
                                    // PFS: generate fresh ephemeral key, compute session key, return our pub
                                    let our_pub = {
                                        let mut enc = self.p2p_encryption.lock().await;
                                        match enc.complete_hello_responder(peer_id, &hello.x25519_public) {
                                            Ok(pub_bytes) => pub_bytes,
                                            Err(e) => {
                                                eprintln!("[P2P] ❌ PFS responder failed: {}", e);
                                                continue;
                                            }
                                        }
                                    };
                                    let mut ack = P2PHelloPacket::new_ack(
                                        self.identity.node_id(),
                                        our_pub,
                                        self.data_addr(),
                                        hello.nonce,
                                        self.identity.signing_public_key,
                                    );
                                    if let Err(e) = ack.sign(&self.identity) {
                                        eprintln!("[P2P] ❌ Failed to sign ack: {}", e);
                                        continue;
                                    }
                                    let bytes = ack.to_bytes().unwrap();
                                    let _ = socket.send_to(&bytes, from).await;
                                    let p2p_peer = P2PPeer {
                                        id: peer_id,
                                        addr: from.to_string(),
                                        data_addr: None,
                                        p2p_data_addr: Some(self.punch.data_addr_for(&peer_id, &hello.p2p_data_addr, from.ip())),
                                        local_addr: None,
                                        public_addr: None,
                                        ipv6_virtual: None,
                                        last_seen: 0,
                                        nat_status: P2PNatStatus::Unknown,
                                        ed25519_public: Some(hello.ed25519_public),
                                    };
                                    self.peers.lock().await.insert(peer_id, p2p_peer);
                                    println!("[P2P] ✅ Added peer {} via handshake [PFS]", hex::encode(&peer_id.0[..8]));
                                }
                                P2PHelloType::Ack => {
                                    let peer_id = hello.node_id;
                                    let mut key_confirmed = true;
                                    // PFS: use stored ephemeral secret (keyed by nonce) to complete ECDH
                                    {
                                        let mut enc = self.p2p_encryption.lock().await;
                                        // «Запасной» обмен долговременным ключом с одноразовым ключом собеседника давал ключ, которого у
                                        // собеседника нет, — убран: неудача здесь значит «оставлен другой ключ» (встречное знакомство) или
                                        // неизвестное приглашение.
                                        if let Err(e) = enc.complete_hello_initiator(hello.nonce, peer_id, &hello.x25519_public) {
                                            eprintln!("[P2P] PFS initiator: {}", e);
                                            key_confirmed = false;
                                        }
                                    }
                                    let p2p_peer = P2PPeer {
                                        id: peer_id,
                                        addr: from.to_string(),
                                        data_addr: None,
                                        p2p_data_addr: Some(self.punch.data_addr_for(&peer_id, &hello.p2p_data_addr, from.ip())),
                                        local_addr: None,
                                        public_addr: None,
                                        ipv6_virtual: None,
                                        last_seen: 0,
                                        nat_status: P2PNatStatus::Unknown,
                                        ed25519_public: Some(hello.ed25519_public),
                                    };
                                    self.peers.lock().await.insert(peer_id, p2p_peer);
                                    println!("[P2P] ✅ Added peer {} via ACK [PFS]", hex::encode(&peer_id.0[..8]));
                                    // Ответчик переходит на новый ключ только получив первый пакет, зашифрованный им. Если слать нечего, обе стороны
                                    // остаются в разных ключах и не слышат друг друга — поэтому сразу отправляем пустую «служебную» весточку.
                                    if key_confirmed {
                                        let this = self.clone();
                                        tokio::task::spawn(async move {
                                            let pkt = P2PPacket::new(P2PPacketType::ChatTyping, this.identity.node_id(), false, Vec::new());
                                            let _ = this.send_packet_dual_path(peer_id, pkt).await;
                                        });
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[P2P] ⚠️ Invalid P2P Hello packet from {}: {}", from, e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[P2P] ❌ Discovery socket error: {}", e);
                }
            }
        }
    }

    /// Handle incoming packet
    async fn handle_packet(&self, data: &[u8], from: SocketAddr) -> Result<(), String> {
        // 🔄 ПРАВИЛЬНАЯ логика разделения по packet_type

        if data.is_empty() {
            return Err("Empty packet".to_string());
        }

        let packet_type_byte = data[0];

        // a hole punching probe (49 bytes, never parsed as anything else)
        if packet_type_byte == punch::PROBE_MAGIC && data.len() == punch::PROBE_LEN {
            self.on_punch_probe(data, from, false).await;
            return Ok(());
        }

        // Старые открытые форматы (CommPacket 0x50-0x6F, пакеты туннеля 0x80-0x8F) не принимаются: они не зашифрованы, а отправитель
        // в них определялся только по адресу — подделать «сообщение от друга» мог любой, кто знает адрес узла. Никто их больше не шлёт.
        if (0x50..=0x6F).contains(&packet_type_byte) || (0x80..=0x8F).contains(&packet_type_byte) {
            return Err(format!("[P2P] ⛔ plaintext legacy packet 0x{:02X} from {} refused", packet_type_byte, from));
        }

        // 📦 CommPacket (старый формат): 0x50-0x6F
        if packet_type_byte >= 0x50 && packet_type_byte <= 0x6F {
            // Это CommPacket (control 0x50-0x5F, data 0x60-0x6F)
            if let Some(comm_packet) = crate::communication::CommPacket::from_bytes(data) {
                let packet_type_name = format!("{:?}", comm_packet.packet_type);
                println!("[P2P] 📦 Received CommPacket ({}) from {}", packet_type_name, from);

                // Отправляем в chat handler
                if let Some(ref tx) = self.chat_packet_tx {
                    // Ищем peer по from addr
                    let peer_id = {
                        println!("[P2P] 🔓 before peers lock");
                        println!("[P2P] 🔒 after peers lock");
                        let peers = self.peers.lock().await;
                        // Ищем peer по p2p_data_addr или addr
                        peers.iter().find(|(_, p)| {
                            p.p2p_data_addr.as_ref().map(|a| a == &from.to_string()).unwrap_or(false)
                                || p.addr == from.to_string()
                        }).map(|(id, _)| *id)
                    };

                    if let Some(id) = peer_id {
                        let _ = tx.try_send((id, comm_packet));
                    } else {
                        eprintln!("[P2P] ⚠️  Received CommPacket from unknown peer: {}", from);
                    }
                }

                return Ok(());
            }
        }

        // 🔗 P2P Tunnel packets (0x80-0x8F) - для p2p_tunnel manager
        if packet_type_byte >= 0x80 && packet_type_byte <= 0x8F {
            println!("[P2P] 🔗 Received Tunnel packet (0x{:02X}) from {}", packet_type_byte, from);

            // Ищем peer по from addr
            let peer_id = {
                let peers = self.peers.lock().await;
                peers.iter().find(|(_, p)| {
                    p.p2p_data_addr.as_ref().map(|a| a == &from.to_string()).unwrap_or(false)
                        || p.addr == from.to_string()
                }).map(|(id, _)| *id)
            };

            if let Some(id) = peer_id {
                if let Some(ref tx) = self.p2p_tunnel_tx {
                    let _ = tx.send((id, data.to_vec())).await;
                    return Ok(());
                }
            } else {
                eprintln!("[P2P] ⚠️  Received Tunnel packet from unknown peer: {}", from);
            }

            return Ok(());
        }

        // 🚀 P2PPacket (новый формат): 0xA0-0xDF (communication layer)
        // Это может быть либо P2PPacket, либо пакет с tunnel/voip/video
        let mut p2p_packet = P2PPacket::from_bytes(data)
            .ok_or_else(|| "Failed to parse P2P packet".to_string())?;

        // SEC-01: принимаются только зашифрованные пакеты — у открытого отправитель взят из заголовка, и его может подделать кто угодно
        if !p2p_packet.encrypted {
            return Err(format!("[P2P] ⛔ unencrypted packet from {} refused", from));
        }
        if p2p_packet.encrypted {
            let mut enc = self.p2p_encryption.lock().await;
            match enc.decrypt_by_peer_id(&p2p_packet.payload) {
                Ok((sender_id, decrypted_padded)) => {
                    // Verify the claimed sender matches the cryptographic sender
                    if sender_id != p2p_packet.sender {
                        return Err(format!(
                            "[P2P] ❌ Sender mismatch: header={} crypto={}",
                            hex::encode(&p2p_packet.sender.0[..8]),
                            hex::encode(&sender_id.0[..8])
                        ));
                    }
                    // Check the type carried inside the encryption, strip the prefix and the trailing padding
                    match open_prefix(p2p_packet.packet_type.to_byte(), &decrypted_padded) {
                        Ok(payload) => p2p_packet.payload = payload,
                        Err(why) => return Err(format!("[P2P] ❌ {} (from {})", why, from)),
                    }
                }
                Err(e) => {
                    // keys that do not match (not a harmless duplicate of a packet already seen) are repaired by a new handshake
                    if e.contains("authentication failed") || e.contains("No session") {
                        let sender = p2p_packet.sender;
                        drop(enc);
                        self.request_resync(sender).await;
                    }
                    return Err(format!("[P2P] ❌ Decryption failed from {}: {}", from, e));
                }
            }
        }

        // The packet is genuine (it opened under the key of exactly this peer, and a replay would have been refused above): the address it
        // came from is a candidate for where the peer now is. A node behind NAT is reachable only at the address its NAT shows to the
        // outside, and that can change; but the known address is replaced only after the new one has answered a challenge (path validation).
        self.learn_address(p2p_packet.sender, from).await;

        // Логируем
        let packet_type_name = match p2p_packet.packet_type {
            P2PPacketType::ChatMessage => "ChatMessage",
            P2PPacketType::FileTransferStart => "FileTransferStart",
            P2PPacketType::FileChunk => "FileChunk",
            P2PPacketType::FileMissing => "FileMissing",
            P2PPacketType::FileComplete => "FileComplete",
            P2PPacketType::FileTransferEnd => "FileTransferEnd",
            _ => "Unknown",
        };

        println!("[P2P] 📦 Received P2PPacket {} from {}", packet_type_name, from);

        // Сохраняем sender до move
        let sender = p2p_packet.sender;

        // Отправить в соответствующий handler
        match p2p_packet.packet_type {
            // Chat (0xA0-0xAF)
            P2PPacketType::ChatMessage | P2PPacketType::ChatAck |
            P2PPacketType::ChatRead | P2PPacketType::ChatTyping |
            P2PPacketType::ChatDeleteMessage => {
                if let Some(ref tx) = self.chat_packet_tx {
                    // Конвертировать P2PPacket в CommPacket
                    if let Some(comm_packet) = Self::p2p_to_comm_packet(p2p_packet) {
                        let _ = tx.send((sender, comm_packet)).await;
                    }
                }
            }

            // Files (0xD0-0xDF)
            P2PPacketType::FileTransferStart | P2PPacketType::FileChunk |
            P2PPacketType::FileTransferEnd | P2PPacketType::FileTransferCancel |
            P2PPacketType::FileMissing | P2PPacketType::FileComplete => {
                if let Some(ref tx) = self.chat_packet_tx {
                    // Файлы тоже идут через ChatManager
                    if let Some(comm_packet) = Self::p2p_to_comm_packet(p2p_packet) {
                        let _ = tx.send((sender, comm_packet)).await;
                    }
                }
            }

            // Hole punching between the peers of an introducer
            P2PPacketType::PunchReq => {
                self.handle_punch_req(sender, &p2p_packet.payload).await;
            }
            P2PPacketType::PunchIntro => {
                self.handle_punch_intro(sender, &p2p_packet.payload).await;
            }
            P2PPacketType::RelayReq => {
                self.handle_relay_req(sender, &p2p_packet.payload).await;
            }
            P2PPacketType::RelayGrant => {
                self.handle_relay_grant(sender, &p2p_packet.payload).await;
            }
            P2PPacketType::PathChallenge => {
                self.on_path_challenge(sender, from, &p2p_packet.payload).await;
            }
            P2PPacketType::PathResponse => {
                self.on_path_response(sender, from, &p2p_packet.payload).await;
            }

            // Voice (0xB0-0xBF) - пока не реализовано, логируем
            P2PPacketType::VoiceCallRequest | P2PPacketType::VoiceCallAccept |
            P2PPacketType::VoiceCallEnd | P2PPacketType::VoiceCallReject |
            P2PPacketType::VoiceData => {
                if let Some(ref tx) = self.media_signal_tx {
                    let _ = tx.send((sender, p2p_packet.packet_type, p2p_packet.payload)).await;
                } else {
                    debug!("📞 Received Voice packet from {} (no media handler)",
                        hex::encode(&sender.0[..8]));
                }
            }

            // Video (0xC0-0xCF) - пока не реализовано, логируем
            P2PPacketType::VideoCallRequest | P2PPacketType::VideoCallAccept |
            P2PPacketType::VideoCallEnd | P2PPacketType::VideoCallReject |
            P2PPacketType::VideoData => {
                if let Some(ref tx) = self.media_signal_tx {
                    let _ = tx.send((sender, p2p_packet.packet_type, p2p_packet.payload)).await;
                } else {
                    debug!("📹 Received Video packet from {} (no media handler)",
                        hex::encode(&sender.0[..8]));
                }
            }
        }

        Ok(())
    }

    /// Конвертировать P2PPacket в CommPacket
    fn p2p_to_comm_packet(p2p_packet: P2PPacket) -> Option<CommPacket> {
        let comm_type = CommControlPacket::from_byte(p2p_packet.packet_type.to_byte())?;
        Some(CommPacket {
            packet_type: comm_type,
            data: p2p_packet.payload,
        })
    }

    /// Print statistics
    pub fn print_stats(&self) {
        let sent_packets = self.stats_sent_packets.load(Ordering::Relaxed);
        let recv_packets = self.stats_recv_packets.load(Ordering::Relaxed);
        let sent_bytes = self.stats_sent_bytes.load(Ordering::Relaxed);
        let recv_bytes = self.stats_recv_bytes.load(Ordering::Relaxed);

        let sent_path0 = self.stats_sent_path0.load(Ordering::Relaxed);
        let sent_path1 = self.stats_sent_path1.load(Ordering::Relaxed);
        let recv_path0 = self.stats_recv_path0.load(Ordering::Relaxed);
        let recv_path1 = self.stats_recv_path1.load(Ordering::Relaxed);

        println!("📊 [P2P] Statistics:");
        println!("   Sent: {} packets ({} bytes)", sent_packets, sent_bytes);
        println!("   Recv: {} packets ({} bytes)", recv_packets, recv_bytes);
        println!("   🚂 Path0: sent={}, recv={}", sent_path0, recv_path0);
        println!("   🚂 Path1: sent={}, recv={}", sent_path1, recv_path1);

        // Вычисляем loss (примерно)
        let sent_total = sent_path0 + sent_path1;
        let recv_total = recv_path0 + recv_path1;
        if sent_total > 0 {
            let loss = ((sent_total - recv_total) * 100) / sent_total.max(1);
            println!("   🔄 Loss: {}% (deduplicated)", loss);
        }
    }

    /// SEC-10: Register expected Ed25519 fingerprints for bootstrap nodes.
    /// Call this before any connections are made (at startup).
    pub fn set_bootstrap_fingerprints(&self, map: HashMap<String, [u8; 32]>) {
        if let Ok(mut guard) = self.bootstrap_fingerprints.write() {
            *guard = map;
            println!("[P2P] 🔒 Registered {} bootstrap fingerprints", guard.len());
        }
    }

    pub async fn derive_file_key(&self, peer_id: &HashId, file_id: &str) -> Option<[u8; 32]> {
        let enc = self.p2p_encryption.lock().await;
        enc.derive_file_key(peer_id, file_id)
    }

    /// A session that has stopped working (packets from a known peer do not decrypt, messages are not acknowledged) is renegotiated:
    /// ask the peer for a fresh handshake — at most once every 5 seconds per peer. Without this, two nodes whose keys ended up different
    /// (handshakes crossing, answers lost or reordered) stay unable to talk until something else happens to renew the key.
    pub async fn request_resync(&self, peer_id: HashId) {
        {
            let mut asked = self.resync_requested.lock().await;
            if asked.get(&peer_id).map_or(false, |t| t.elapsed() < std::time::Duration::from_secs(5)) {
                return;
            }
            asked.insert(peer_id, std::time::Instant::now());
            if asked.len() > 4096 {
                asked.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(60));
            }
        }
        let addr = self.peers.lock().await.get(&peer_id).map(|p| p.addr.clone());
        if let Some(addr) = addr {
            println!("[P2P] 🔁 session with {} is out of step — asking for a new handshake", hex::encode(&peer_id.0[..8]));
            let _ = self.send_hello_request(&addr).await;
        }
    }

    pub async fn send_hello_request(&self, addr: &str) -> Result<(), String> {
        use crate::p2p::hello::P2PHelloPacket;

        // Create packet with placeholder x25519 — nonce is assigned inside new_request
        let mut hello = P2PHelloPacket::new_request(
            self.identity.node_id(),
            [0u8; 32],
            self.data_addr(),
            self.identity.signing_public_key,
        );

        // PFS: generate a fresh ephemeral X25519 keypair keyed by this hello's nonce
        let ephemeral_pub = {
            let mut enc = self.p2p_encryption.lock().await;
            enc.generate_hello_ephemeral(hello.nonce)
        };
        hello.x25519_public = ephemeral_pub;

        // Sign AFTER setting ephemeral pub (canonical_bytes includes x25519_public)
        hello.sign(&self.identity).map_err(|e| format!("Failed to sign hello: {}", e))?;
        let bytes = hello.to_bytes().map_err(|e| e.to_string())?;
        self.discovery_socket.send_to(&bytes, addr).await
            .map_err(|e| format!("Failed to send: {}", e))?;
        println!("[P2P] ✅ HELLO_REQ sent to {} (PFS ephemeral key)", addr);
        Ok(())
    }
}

/// First byte of the plaintext of every packet: marks the format in which the packet type travels inside the encryption.
const TYPED_MARKER: u8 = 0xA7;

/// `[marker][type][len:4][payload]` — the type travels INSIDE the encryption, so a header byte changed on the way is noticed.
fn seal_prefix(packet_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + payload.len());
    out.push(TYPED_MARKER);
    out.push(packet_type);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reverse of `seal_prefix` for the decrypted (padded) bytes. `header_type` is the type written in the unprotected header: it
/// must equal the one inside. A packet without the marker is refused.
fn open_prefix(header_type: u8, decrypted_padded: &[u8]) -> Result<Vec<u8>, &'static str> {
    if decrypted_padded.first() != Some(&TYPED_MARKER) {
        return Err("packet without an authenticated type refused");
    }
    if decrypted_padded.len() < 6 {
        return Err("Decrypted payload too short");
    }
    if decrypted_padded[1] != header_type {
        return Err("packet type does not match the authenticated type");
    }
    let start = 6usize;
    let original_len = u32::from_be_bytes(decrypted_padded[2..6].try_into().map_err(|_| "bad length")?) as usize;
    if start.checked_add(original_len).map_or(true, |end| end > decrypted_padded.len()) {
        return Err("Length prefix exceeds decrypted data");
    }
    Ok(decrypted_padded[start..start + original_len].to_vec())
}

#[cfg(test)]
mod pending_packet_tests {
    use super::*;

    fn pending(bytes: usize, age: std::time::Duration) -> PendingPacket {
        let mut originals = std::collections::HashMap::new();
        originals.insert(0u32, vec![0u8; bytes]);
        PendingPacket {
            sender: HashId([1; 32]),
            total_parts: 2,
            parts: std::collections::HashMap::new(),
            last_update: std::time::Instant::now() - age,
            originals,
            clones: std::collections::HashMap::new(),
            received: std::collections::HashSet::new(),
        }
    }

    fn put(c: &mut PacketCache, id: u64, bytes: usize, age: std::time::Duration) {
        c.current_bytes += bytes;
        c.packets.insert(id, pending(bytes, age));
    }

    #[test]
    fn the_cache_of_unauthenticated_packets_stays_within_its_byte_and_entry_limits() {
        let mut c = PacketCache::new(10_000);
        for id in 0..50u64 {
            // every new packet first asks for room; oldest entries make way when the byte limit is reached
            assert!(c.admit(1_000));
            put(&mut c, id, 1_000, std::time::Duration::from_secs(id));
        }
        assert!(c.current_bytes <= 10_000, "{}", c.current_bytes);
        assert!(c.packets.len() <= 10);
        // a packet bigger than the whole cache is never admitted
        assert!(!c.admit(20_000));
    }

    #[test]
    fn old_entries_expire_and_their_bytes_are_returned() {
        let mut c = PacketCache::new(1_000_000);
        put(&mut c, 1, 500, PENDING_TTL + std::time::Duration::from_secs(5));
        put(&mut c, 2, 700, std::time::Duration::from_secs(1));
        c.last_prune = std::time::Instant::now() - std::time::Duration::from_secs(5);
        assert!(c.admit(0));
        assert!(!c.packets.contains_key(&1));
        assert!(c.packets.contains_key(&2));
        assert_eq!(c.current_bytes, 700);
    }

    #[test]
    fn the_packet_type_is_carried_inside_the_encryption_and_a_changed_header_type_is_refused() {
        let sealed = seal_prefix(0xB0, b"call me");
        assert_eq!(open_prefix(0xB0, &sealed).unwrap(), b"call me".to_vec());
        // an on-path attacker flips the unprotected type byte of a valid packet: the inner type no longer matches
        assert!(open_prefix(0xB1, &sealed).is_err());
        // padding after the payload is ignored
        let mut padded = sealed.clone();
        padded.extend_from_slice(&[0u8; 40]);
        assert_eq!(open_prefix(0xB0, &padded).unwrap(), b"call me".to_vec());
        // a truncated or lying length is refused, not panicked on
        assert!(open_prefix(0xB0, &[TYPED_MARKER, 0xB0, 0, 0, 0xFF, 0xFF, 1]).is_err());
        assert!(open_prefix(0xB0, &[TYPED_MARKER]).is_err());
    }

    #[test]
    fn a_packet_without_the_authenticated_type_is_refused() {
        let mut untyped = (5u32).to_be_bytes().to_vec();
        untyped.extend_from_slice(b"hello");
        assert!(open_prefix(0xA0, &untyped).is_err());
    }
}

/// how often a packet is sent to every known peer so that NATs keep the conversation in memory.
/// MEASURED (chaos/punchtest2.py, 2026-10-08): a punched UDP path stays alive through 20 s of silence and is dead after 30 s; with a
/// packet every 15 s it stayed alive through 90 s and longer. Do not raise this without repeating that measurement.
pub const KEEPALIVE_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// Адрес для данных собеседника: порт — тот, что он назвал, а IP — тот, с которого пришло его приветствие. Узел за NAT называет свой
/// внутренний адрес (192.168.x.x), и слать данные по нему публичному собеседнику бесполезно.
fn observed_data_addr(declared: &str, seen: std::net::IpAddr) -> String {
    match declared.parse::<std::net::SocketAddr>() {
        Ok(a) if a.ip() == seen => declared.to_string(),
        Ok(a) => std::net::SocketAddr::new(seen, a.port()).to_string(),
        Err(_) => declared.to_string(),
    }
}

#[cfg(test)]
mod observed_addr_tests {
    use super::observed_data_addr;

    #[test]
    fn a_node_behind_nat_is_reached_at_the_address_it_was_seen_from() {
        let seen: std::net::IpAddr = "11.77.254.1".parse().unwrap();
        assert_eq!(observed_data_addr("192.168.1.11:26104", seen), "11.77.254.1:26104");
        assert_eq!(observed_data_addr("0.0.0.0:26104", seen), "11.77.254.1:26104");
        assert_eq!(observed_data_addr("11.77.254.1:26104", seen), "11.77.254.1:26104");
        assert_eq!(observed_data_addr("garbage", seen), "garbage");
    }
}
