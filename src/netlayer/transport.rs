// src/netlayer/transport.rs
//! P2P Transport Layer
//! ===================
//!
//! Two-port UDP transport:
//! - Port 9000: Hello/Discovery (unencrypted metadata exchange)
//! - Port 10000: Encrypted data session (after Hello exchange)

use crate::util::HashId;
use std::time::Instant;
use crate::core::NodeIdentity;
use crate::netlayer::{
    port_manager::{PortManager, DEFAULT_DATA_PORT, DEFAULT_DISCOVERY_PORT},
    adaptive::AdaptiveController,
    peer::PeerInfo,
    packet::{HelloPacket, HelloType, NetPacket, PacketType},
    encryption::EncryptionManager,
    tunnel::TunnelManager,
    nat::{NatStatus, MappingBehavior},
    interface_detector::NetworkTopology,
    relay::RelayManager,
    socket_manager::{SocketManager, SocketPair},
};
use crate::dht::Kademlia;
use crate::dataplane::{StreamRegistry, StreamFrame, SharedStreamRegistry};
use rand::Rng;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::UdpSocket;
use crate::netlayer::tcp_carrier::{self, CSocket};
use tokio::sync::{Mutex, mpsc, oneshot};

// Obfuscation settings
pub const MIN_PADDING: usize = 8;   // Reduced from 16
pub const MAX_PADDING: usize = 32;  // Reduced from 256
pub const MIN_JITTER_MS: u64 = 0;   // No minimum
pub const MAX_JITTER_MS: u64 = 10;  // Reduced from 100ms

/// Port rotation settings for DPI resistance
pub const PORT_ROTATION_INTERVAL_MIN: u64 = 5;   // Rotate every 5-10 minutes
pub const PORT_ROTATION_INTERVAL_MAX: u64 = 10;
pub const EPHEMERAL_PORT_MIN: u16 = 49152;      // Dynamic/private ports
pub const EPHEMERAL_PORT_MAX: u16 = 65535;


/// 🚂 WAGON-Level Statistics (для анализа потерь пакетов)
#[derive(Debug)]
pub struct WagonStats {
    /// Wagons sent (per path)
    pub sent_path0: AtomicU64,
    pub sent_path1: AtomicU64,
    pub sent_total: AtomicU64,

    /// Wagons received (per path)
    pub recv_path0: AtomicU64,
    pub recv_path1: AtomicU64,
    pub recv_total: AtomicU64,

    /// Wagons with bad checksum
    pub checksum_failed: AtomicU64,

    /// Wagons dropped (timeout, buffer full, etc.)
    pub dropped: AtomicU64,

    /// Wagons retransmitted (via NACK)
    pub retransmitted: AtomicU64,

    /// 🔴 Path0 wagons lost (Path1 arrived instead)
    pub path0_lost: AtomicU64,
}

impl WagonStats {
    pub fn new() -> Self {
        Self {
            sent_path0: AtomicU64::new(0),
            sent_path1: AtomicU64::new(0),
            sent_total: AtomicU64::new(0),
            recv_path0: AtomicU64::new(0),
            recv_path1: AtomicU64::new(0),
            recv_total: AtomicU64::new(0),
            checksum_failed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            retransmitted: AtomicU64::new(0),
            path0_lost: AtomicU64::new(0),
        }
    }

    /// Print statistics every N seconds
    pub fn print_stats(&self) {
        let sent0 = self.sent_path0.load(Ordering::Relaxed);
        let sent1 = self.sent_path1.load(Ordering::Relaxed);
        let sent_total = self.sent_total.load(Ordering::Relaxed);

        let recv0 = self.recv_path0.load(Ordering::Relaxed);
        let recv1 = self.recv_path1.load(Ordering::Relaxed);
        let recv_total = self.recv_total.load(Ordering::Relaxed);

        let checksum_failed = self.checksum_failed.load(Ordering::Relaxed);
        let dropped = self.dropped.load(Ordering::Relaxed);
        let retransmitted = self.retransmitted.load(Ordering::Relaxed);
        let path0_lost = self.path0_lost.load(Ordering::Relaxed);

        // Use signed arithmetic to handle cases where recv > sent (e.g., gateway receiving more than sending)
        let loss0 = if sent0 > 0 {
            let sent_i64 = sent0 as i64;
            let recv_i64 = recv0 as i64;
            let loss = ((sent_i64 - recv_i64) * 100 / sent_i64).max(0);
            loss as u64
        } else {
            0
        };

        let loss1 = if sent1 > 0 {
            let sent_i64 = sent1 as i64;
            let recv_i64 = recv1 as i64;
            let loss = ((sent_i64 - recv_i64) * 100 / sent_i64).max(0);
            loss as u64
        } else {
            0
        };

        let loss_total = if sent_total > 0 {
            let sent_i64 = sent_total as i64;
            let recv_i64 = recv_total as i64;
            let loss = ((sent_i64 - recv_i64) * 100 / sent_i64).max(0);
            loss as u64
        } else {
            0
        };

        println!("🚂 [WAGON-STATS] Path0: sent={}, recv={}, loss={}%", sent0, recv0, loss0);
        println!("🚂 [WAGON-STATS] Path1: sent={}, recv={}, loss={}%", sent1, recv1, loss1);
        println!("🚂 [WAGON-STATS] TOTAL: sent={}, recv={}, loss={}%, checksum_fail={}, dropped={}, retrans={}, path0_lost={}",
                 sent_total, recv_total, loss_total, checksum_failed, dropped, retransmitted, path0_lost);
    }
}

/// Global wagon statistics (accessible from everywhere)
pub static WAGON_STATS: std::sync::OnceLock<WagonStats> = std::sync::OnceLock::new();

pub fn get_wagon_stats() -> &'static WagonStats {

    WAGON_STATS.get_or_init(|| WagonStats::new())
}

/// Stream statistics
pub struct StreamStats {
    pub stream_id: u32,
    pub state: String,
    pub send_seq: u32,
    pub recv_seq: u32,
    pub unacked: usize,
    pub rtt_ms: u32,
    pub available: usize,
}

#[derive(Debug, Clone, Default)]
pub struct WebTransportMetrics {
    pub rx_speed: f64,
    pub tx_speed: f64,
    pub peer_rx_estimate: f64,
    pub avg_rtt_ms: u32,
    pub path0_loss_incoming_pct: f64,
    pub peer_path0_loss_pct: f64,
    pub clone_hit_pct: f64,
    pub clone_hit_rate: f64,
    pub wagons_per_sec: f64,
    pub active_trains: usize,
    pub depot_bytes: usize,
    pub delivered_cache_size: usize,
    pub evictions_total: u64,
    pub evictions_delta: u64,
    pub timeout_total: u64,
    pub cleanup_total: u64,
    pub total_wagons: u64,
    pub total_clone_hits: u64,
    pub wagon_sent_total: u64,
    pub wagon_recv_total: u64,
    pub wagon_drop_total: u64,
    pub wagon_checksum_failed_total: u64,
    pub wagon_retrans_total: u64,
    pub wagon_drop_crc_pct: f64,
    pub peer_count: usize,
}

#[derive(Debug, Clone)]
struct WebMetricsCache {
    sampled_at: Instant,
    rx_bytes: u64,
    tx_bytes: u64,
    wagons_received: u64,
    clone_hits: u64,
    path0_loss_events: u64,
    evictions_total: u64,
}

impl Default for WebMetricsCache {
    fn default() -> Self {
        Self {
            sampled_at: Instant::now(),
            rx_bytes: 0,
            tx_bytes: 0,
            wagons_received: 0,
            clone_hits: 0,
            path0_loss_events: 0,
            evictions_total: 0,
        }
    }
}

/// Transport state for adaptive control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    /// Normal operation
    Active,
    /// Port rotation in progress
    Rotating,
    /// Degraded performance (using stealth mode)
    Degraded,
}

impl Default for TransportState {
    fn default() -> Self {
        TransportState::Active
    }
}



/// Incoming packet from network
#[derive(Debug, Clone)]
pub struct IncomingPacket {
    pub from: SocketAddr,
    pub data: Vec<u8>,
}

struct RotatedListenerHandles {
    discovery_port: u16,
    data_port: u16,
    discovery_stop: oneshot::Sender<()>,
    data_stop: oneshot::Sender<()>,
}

/// Hello event for discovery
#[derive(Debug, Clone)]
pub enum HelloEvent {
    Request {
        from: SocketAddr,
        packet: HelloPacket,
    },
    Ack {
        from: SocketAddr,
        packet: HelloPacket,
    },
}

/// Request to start Exit Node Handler
#[derive(Debug, Clone)]
pub struct ExitHandlerRequest {
    pub peer_id: HashId,
    pub peer_addr: String,
}

/// 🚇 Integration Step 1: откуда пришёл декриптнутый wagon — нужно для решений в
/// `dispatch_decrypted_wagon` (например, на WS нет UDP-сокета для ack).
#[derive(Debug, Clone)]
pub enum WagonSource {
    /// Пакет пришёл через UDP. Нужны peer_addr и порт-инфо для ответных send_to.
    Udp { peer_addr: String },
    /// Пакет пришёл через WS-канал (mobile↔anchor). Ответы идут через `ws_outgoing` или send_encrypted.
    Ws,
}

/// Request to start proxy gateway (автозапуск!)
#[derive(Debug, Clone)]
pub struct ProxyGatewayRequest {
    /// ID ноды которая запрашивает gateway
    pub peer_id: HashId,
    /// Адрес ноды
    pub peer_addr: String,
    /// Short ID для логирования
    pub short_id: String,
}

/// P2P Transport Manager
///
/// Manages two UDP sockets:
/// - Discovery socket (port 9000) for Hello packets
/// - Data socket (port 10000) for encrypted traffic
#[derive(Clone)]
pub struct P2PTransport {
    /// Node identity
    identity: Arc<NodeIdentity>,
    local_id: HashId,

    /// Encryption manager
    encryption: Arc<Mutex<EncryptionManager>>,

    /// Tunnel manager
    tunnels: Arc<Mutex<TunnelManager>>,
        dht: Arc<Mutex<Kademlia>>,

    /// Discovery socket (port 9000)
    discovery_socket: Arc<CSocket>,

    /// Data receive socket (port 10000) - for recv_from only
    data_recv_socket: Arc<CSocket>,

    /// Data send socket (port 10000) - for send_to only
    data_send_socket: Arc<CSocket>,
    /// Socket manager for port rotation
    socket_manager: Arc<SocketManager>,
    /// Port rotation notification channel
    port_rotation_tx: tokio::sync::watch::Sender<SocketPair>,
    /// Currently active rotated listeners that should be retired on next rotation
    active_rotated_listeners: Arc<tokio::sync::Mutex<Option<RotatedListenerHandles>>>,

    /// Known peers
    peers: Arc<Mutex<HashMap<HashId, PeerInfo>>>,

    /// Hello event sender
    hello_tx: tokio::sync::broadcast::Sender<HelloEvent>,

    /// Node capabilities (based on system resources)
    capabilities: u16,

    /// DHT for peer discovery

    /// Stream registry for reliable streams
    streams: SharedStreamRegistry,

    /// Exit handler request sender (optional, only for main node)
    exit_handler_tx: Option<mpsc::Sender<ExitHandlerRequest>>,

    /// Proxy gateway request sender (optional, for auto-starting gateway)
    proxy_gateway_tx: Option<mpsc::Sender<ProxyGatewayRequest>>,

    /// Proxy request sender (optional, for proxy gateway)
    proxy_request_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyRequest)>>,

    /// Proxy response sender (optional, for proxy client)
    proxy_response_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyResponse)>>,

    /// Proxy tunnel data sender (optional, for CONNECT tunneling)
    proxy_tunnel_data_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyTunnelData)>>,

    /// SOCKS5 gateway request sender
    socks5_gateway_tx: Option<mpsc::Sender<ProxyGatewayRequest>>,
    /// SOCKS5 request sender
    socks5_request_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5ProxyRequest)>>,
    /// SOCKS5 response sender
    socks5_response_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5ProxyResponse)>>,
    /// SOCKS5 tunnel data sender
    socks5_tunnel_data_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5TunnelData)>>,

    /// 🔄 NACK sender (optional, for wagon retransmission) - отправляет NACK в gateway
    nack_tx: Option<mpsc::Sender<(HashId, crate::protocol::WagonNack)>>,

    /// YTP Station для обработки поездов
    pub station: Arc<Mutex<Option<Arc<crate::protocol::Station>>>>,
    /// RX byte counter for speed monitoring
    pub rx_bytes_counter: Arc<AtomicU64>,
    /// TX byte counter for speed monitoring
    pub tx_bytes_counter: Arc<AtomicU64>,
    /// Last RX bytes for speed calculation
    pub last_rx_bytes: Arc<tokio::sync::Mutex<u64>>,
    /// Last RX time for speed calculation
    pub last_rx_time: Arc<tokio::sync::Mutex<Instant>>,
    /// Independent cache for web/UI telemetry sampling
    web_metrics_cache: Arc<tokio::sync::Mutex<WebMetricsCache>>,

    /// TUN wagon sender (optional, for TUN exit node)
    tun_wagon_tx: Option<mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagon)>>,
    /// TUN wagon response sender (optional, for TUN entry node)
    tun_wagon_resp_tx: Option<mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagonResponse)>>,

    /// P2P tunnel packet sender (optional, for P2P tunnel manager)
    p2p_tunnel_tx: Option<mpsc::Sender<(HashId, Vec<u8>)>>,

    /// Chat packet sender (optional, for chat manager)
    chat_packet_tx: Option<mpsc::Sender<(HashId, crate::communication::CommPacket)>>,
    /// Group packet sender (optional, for group chat manager)
    group_packet_tx: Option<mpsc::Sender<(HashId, crate::communication::GroupPacket)>>,



    /// Node Identity Binding Fix: node_id -> pinned Ed25519 signing pubkey,
    /// sourced from the owner's own `trusted_peers.json` (see
    /// `set_pinned_identities`). A Hello claiming a pinned node_id with a
    /// DIFFERENT signing key is a hard IDENTITY_CONFLICT, rejected before
    /// any PeerInfo/DHT/online-state update — see `identity_conflict()`.
    /// Empty by default (fully permissive, matching prior behavior) until
    /// main.rs loads the peer directory and calls the setter.
    pinned_identities: Arc<tokio::sync::RwLock<HashMap<HashId, [u8; 32]>>>,

    /// Node Identity Binding Fix — Barrier 3 (replay protection).
    /// Two-Phase Verification (`verify_peer_handshake_static`) already checks
    /// the Ed25519 signature and a ±5 minute timestamp window, but neither
    /// proves a given Hello was freshly SIGNED now rather than captured once
    /// and resent later within that same window — a captured Hello from a
    /// pinned peer is still a byte-for-byte valid signature. Every accepted
    /// Hello unconditionally overwrites the sender's PeerInfo.addr and DHT
    /// entry with the packet's source address, so a replay from an attacker
    /// address would silently redirect all future traffic meant for that
    /// peer. Tracks (node_id, nonce) pairs already accepted within the
    /// freshness window; a repeat is rejected as a replay, not processed as
    /// a new handshake. See `check_replay()`.
    seen_hello_nonces: Arc<tokio::sync::Mutex<HashMap<(HashId, u64), Instant>>>,
    /// Допуск новых узлов: ограничения на наплыв поддельных личностей (см. netlayer::admission)
    admission: Arc<tokio::sync::Mutex<crate::netlayer::admission::Admission>>,
    /// Когда в последний раз просили новый ключ у узла (не чаще раза в минуту)
    rekey_requested: Arc<tokio::sync::Mutex<HashMap<HashId, Instant>>>,

    /// New peer notification sender (optional, for P2P transport synchronization)
    new_peer_tx: Option<mpsc::Sender<PeerInfo>>,
    /** External IP address (detected via external service).
     * Обёрнуто в RwLock, чтобы фоновая задача могла обновлять при смене ISP/адреса. */
    external_ip: Arc<tokio::sync::RwLock<Option<String>>>,

    /// 🌐 Поведение NAT-маппинга нашего узла (EIM/EDM/NoNat) — для решения о hole punching.
    pub local_nat_mapping: Arc<tokio::sync::RwLock<MappingBehavior>>,

    /// 🌐 Сырые observed-адреса от разных peer'ов: peer_id → (ip:port мы у них).
    /// Когда здесь ≥2 уникальных entry — считаем mapping behavior.
    pub mapping_probes: Arc<tokio::sync::RwLock<HashMap<HashId, String>>>,

    /// 📡 WS-выход на peer'а: если peer подключён через WS-туннель (Mobile↔Anchor),
    /// его peer_id появляется в этой мапе с каналом для отправки. send_encrypted
    /// проверяет эту мапу первой и при наличии — шлёт wagon через WS, иначе UDP.
    pub ws_outgoing: Arc<tokio::sync::Mutex<HashMap<HashId, mpsc::Sender<Vec<u8>>>>>,


    /// 🔐 Step 4: PairedClientStore — anchor хранит session-токены спаренных клиентов.
    /// Загружается при старте из `~/.yandi/paired_clients.json`. На mobile-стороне пуст.
    /// Используется при обработке 0xC0 RESUME: lookup session_id → verify HMAC → refresh TTL.
    pub paired_clients: Arc<tokio::sync::Mutex<crate::netlayer::pairing::PairedClientStore>>,

    /// 🔁 Step 6: PairedAnchorStore — mobile хранит список своих anchor'ов в порядке
    /// предпочтения. Загружается при старте из `~/.yandi/paired_anchors.json`.
    /// Watchdog task поверх этого store'а делает auto-reconnect.
    pub paired_anchors: Arc<tokio::sync::Mutex<crate::netlayer::pairing::PairedAnchorStore>>,


    /// 🆕 Hardening Step 5: индекс `jurisdiction → Vec<anchor>` (lite-вариант
    /// DHT find_by_jurisdiction). Обновляется при приёме Hello'а с jurisdiction TLV.
    pub jurisdiction_index: Arc<crate::dht::JurisdictionIndex>,

    /** Network topology information */
    topology: Option<NetworkTopology>,
    /// State Manager for adaptive control

    pub state_manager: Arc<tokio::sync::Mutex<crate::state_manager::StateManager>>,

    /// Telemetry collector

    pub telemetry_collector: Arc<tokio::sync::Mutex<crate::state_manager::TelemetryCollector>>,

    /// Last control plane update time

    pub last_control_update: Arc<tokio::sync::Mutex<std::time::Instant>>,

    /// Relay manager for NAT traversal
    pub relay_manager: Arc<Mutex<RelayManager>>,
    pub port_manager: Arc<PortManager>,
    /// Relay request sender
    pub adaptive_controller: Arc<tokio::sync::Mutex<AdaptiveController>>,
    relay_request_tx: Option<mpsc::Sender<(HashId, HashId)>>,
    /// Relay response sender
    relay_response_tx: Option<mpsc::Sender<(HashId, crate::netlayer::packet::RelayConnectResponse)>>,
    /// Relay data sender
    relay_data_tx: Option<mpsc::Sender<(HashId, HashId, Vec<u8>)>>,
}

impl P2PTransport {
    /// Create new P2P transport
    ///
    /// Binds to:
    /// - 0.0.0.0:9000 for discovery (Hello)
    /// - 0.0.0.0:10000 for encrypted data
    pub async fn new(identity: NodeIdentity, capabilities: u16) -> Result<Arc<Self>, String> {
        Self::with_handlers(identity, capabilities, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, DEFAULT_DISCOVERY_PORT, DEFAULT_DATA_PORT).await
    }

    /// Create new P2P transport with exit handler channel
    pub async fn with_exit_handler(
        identity: NodeIdentity,
        capabilities: u16,
        exit_handler_tx: Option<mpsc::Sender<ExitHandlerRequest>>,
    ) -> Result<Arc<Self>, String> {
        Self::with_handlers(identity, capabilities, exit_handler_tx, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, DEFAULT_DISCOVERY_PORT, DEFAULT_DATA_PORT).await
    }

    /// Create new P2P transport with exit handler and proxy request channels
    pub async fn with_handlers(
        identity: NodeIdentity,
        capabilities: u16,
        exit_handler_tx: Option<mpsc::Sender<ExitHandlerRequest>>,
        proxy_gateway_tx: Option<mpsc::Sender<ProxyGatewayRequest>>,
        proxy_request_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyRequest)>>,
        proxy_response_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyResponse)>>,
        proxy_tunnel_data_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyTunnelData)>>,
        nack_tx: Option<mpsc::Sender<(HashId, crate::protocol::WagonNack)>>,
        // SOCKS5 handlers (аналогично HTTP Proxy)
        socks5_gateway_tx: Option<mpsc::Sender<ProxyGatewayRequest>>,  // 🧦 SOCKS5 Gateway auto-start
        socks5_request_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5ProxyRequest)>>,
        socks5_response_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5ProxyResponse)>>,
        socks5_tunnel_data_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5TunnelData)>>,
        // TUN wagon handler
        tun_wagon_tx: Option<mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagon)>>,
        tun_wagon_resp_tx: Option<mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagonResponse)>>,
        // P2P tunnel handler
        p2p_tunnel_tx: Option<mpsc::Sender<(HashId, Vec<u8>)>>,
        // Chat handler
        chat_packet_tx: Option<mpsc::Sender<(HashId, crate::communication::CommPacket)>>,
    group_packet_tx: Option<mpsc::Sender<(HashId, crate::communication::GroupPacket)>>,
        // New peer notification (for P2P transport sync)
        new_peer_tx: Option<mpsc::Sender<PeerInfo>>,
        external_ip: Option<String>,
        topology: Option<NetworkTopology>,
        // Relay handlers
        relay_request_tx: Option<mpsc::Sender<(HashId, HashId)>>,
        relay_response_tx: Option<mpsc::Sender<(HashId, crate::netlayer::packet::RelayConnectResponse)>>,
        relay_data_tx: Option<mpsc::Sender<(HashId, HashId, Vec<u8>)>>,
        // Real Two-Node P2P E2E test mandate: these were hardcoded to
        // DEFAULT_DISCOVERY_PORT/DEFAULT_DATA_PORT, making it impossible
        // to run two YANDI nodes on one machine (confirmed live: second
        // process failed with "Address already in use"). `config.ports.
        // discovery`/`data` already existed in YandiConfig and were
        // already read by main.rs for display/identity-file naming —
        // this just threads those same values into the actual bind.
        discovery_port: u16,
        data_port: u16,
    ) -> Result<Arc<Self>, String> {
        println!("[transport] Initializing P2P transport");

        // ⚠️ Hardening — sysctl preflight: проверяем что kernel позволит наш SO_RCVBUF.
        // Если net.core.rmem_max ниже 4 МБ — ядро silently capped'ит наш RCVBUF (см.
        // setsockopt ниже) и под нагрузкой ловим UDP-drop'ы в ядре, что выглядит как
        // "белый экран при шейпинге провайдера". Тыкаем пользователя инструкцией.
        if let Ok(v) = std::fs::read_to_string("/proc/sys/net/core/rmem_max") {
            if let Ok(rmem_max) = v.trim().parse::<u64>() {
                if rmem_max < 4_000_000 {
                    eprintln!("[transport] ⚠️  net.core.rmem_max = {} (< 4 MB!).", rmem_max);
                    eprintln!("[transport]    Ядро будет silently капать SO_RCVBUF, под нагрузкой увидите UDP-drops.");
                    eprintln!("[transport]    Поправить: sudo sysctl -w net.core.rmem_max=67108864");
                    eprintln!("[transport]              sudo sysctl -w net.core.wmem_max=67108864");
                }
            }
        }
        if let Ok(v) = std::fs::read_to_string("/proc/sys/net/core/wmem_max") {
            if let Ok(wmem_max) = v.trim().parse::<u64>() {
                if wmem_max < 4_000_000 {
                    eprintln!("[transport] ⚠️  net.core.wmem_max = {} (< 4 MB).", wmem_max);
                    eprintln!("[transport]    Send burst'ы тоже могут drop'аться.");
                }
            }
        }

        // Create encryption manager with our node_id
        let encryption = Arc::new(Mutex::new(EncryptionManager::new(identity.node_id())));

        // Bind discovery socket (configurable — see discovery_port param doc above)
        let discovery_socket = UdpSocket::bind(format!("0.0.0.0:{}", discovery_port))
            .await
            .map_err(|e| format!("Failed to bind discovery socket: {}", e))?;
        // носитель: UDP, а при неработающем UDP — запасной путь по TCP/TLS (`tcp_carrier`)
        let carrier = tcp_carrier::install(identity.node_id().to_hex());
        let discovery_socket = CSocket::new(Arc::new(discovery_socket), carrier.clone())
            .map_err(|e| format!("Failed to wrap discovery socket: {}", e))?;

        println!(
            "[transport] 🔓 Fallback discovery socket bound to 0.0.0.0:{}",
            discovery_port
        );

        // Bind ONE data socket with HUGE buffers for 60 Mbps! (port configurable)
        let data_socket = UdpSocket::bind(format!("0.0.0.0:{}", data_port))
            .await
            .map_err(|e| format!("Failed to bind data socket: {}", e))?;

        // 🚂 Increase UDP buffer sizes to handle 60 Mbps streaming
        // 60 Mbps = 7.5 MB/s, need at least 4 MB buffer for 500ms safety
        // Using libc for platform-specific socket option setting
        let std_socket = data_socket.into_std()
            .map_err(|e| format!("Failed to convert socket to std: {}", e))?;

        #[cfg(unix)]
        {
            use libc::{setsockopt, SOL_SOCKET, SO_RCVBUF, SO_SNDBUF};
            use std::os::unix::io::AsRawFd;

            let fd = std_socket.as_raw_fd();
            let buffer_size: i32 = 4 * 1024 * 1024; // 4 MB

            // Set receive buffer size
            if unsafe { setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &buffer_size as *const i32 as *const _, std::mem::size_of::<i32>() as u32) } == 0 {
                println!("[transport] 🚂 Set recv_buffer_size to 4 MB for high-speed streaming");
            } else {
                eprintln!("[transport] ⚠️  Failed to set recv_buffer_size to 4MB");
            }

            // Set send buffer size
            if unsafe { setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &buffer_size as *const i32 as *const _, std::mem::size_of::<i32>() as u32) } == 0 {
                println!("[transport] 🚂 Set send_buffer_size to 4 MB for high-speed streaming");
            } else {
                eprintln!("[transport] ⚠️  Failed to set send_buffer_size to 4MB");
            }
        }

        #[cfg(not(unix))]
        {
            eprintln!("[transport] ⚠️  UDP buffer size setting not supported on this platform");
        }

        let data_socket = tokio::net::UdpSocket::from_std(std_socket)
            .map_err(|e| format!("Failed to convert socket to tokio: {}", e))?;

        let data_recv_socket = CSocket::new(Arc::new(data_socket), carrier.clone())
            .map_err(|e| format!("Failed to wrap data socket: {}", e))?;
        let data_send_socket = Arc::clone(&data_recv_socket);

        println!(
            "[transport] 🔒 Fallback data socket bound to 0.0.0.0:{}",
            data_port
        );

        println!("[transport] 🔗 Data send socket created (shared with recv)");

        // Create socket manager for port rotation
        let socket_manager = Arc::new(SocketManager::new(
            discovery_socket.clone(),
            data_recv_socket.clone(),
            data_recv_socket.clone(), // временно используем data сокет для p2p
        ));

        // Create port rotation notification channel
        let initial_pair = SocketPair::new(
            discovery_socket.clone(),
            data_recv_socket.clone(),
            data_recv_socket.clone(),
        );
        let (port_rotation_tx, _port_rotation_rx) = tokio::sync::watch::channel(initial_pair);

        // Create hello event channel (broadcast - multiple subscribers)
        let (hello_tx, _hello_rx) = tokio::sync::broadcast::channel(100);

        let identity = Arc::new(identity);

        let peers: Arc<Mutex<HashMap<HashId, PeerInfo>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Create tunnel manager
        let tunnels: Arc<Mutex<TunnelManager>> =
            Arc::new(Mutex::new(TunnelManager::new()));

        // Create DHT
        let node_id = identity.node_id();
        let dht: Arc<Mutex<Kademlia>> =
            Arc::new(Mutex::new(Kademlia::new(node_id)));
        Kademlia::start_background_tasks_mutex(dht.clone());

        let streams: SharedStreamRegistry =
            Arc::new(Mutex::new(StreamRegistry::new()));

        // Create placeholder for Station (will be set after transport is created)
        let station: Arc<Mutex<Option<Arc<crate::protocol::Station>>>> =
            Arc::new(Mutex::new(None));

        // Create relay manager
        let relay_manager = Arc::new(Mutex::new(RelayManager::new()));
        // Seed with the ports we actually bound above, not the hardcoded
        // defaults — see discovery_port/data_port param doc.
        let port_manager = Arc::new(PortManager::new(discovery_port, data_port));

        let port_manager_for_rotation = port_manager.clone();
        let adaptive_controller = Arc::new(tokio::sync::Mutex::new(AdaptiveController::new()));

        // Spawn tunnel monitor task
        let tunnels_clone2 = tunnels.clone();
        tokio::spawn(async move {
            Self::tunnel_monitor(tunnels_clone2).await;
        });

        // 🔒 Publish NodeRecord to DHT (after short delay to ensure DHT is ready)
        let dht_clone2 = dht.clone();
        let identity_clone = identity.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            
            let node_record = crate::dht::record::NodeRecord::new(
                &identity_clone,
                0,  // initial sequence number
                None,  // endpoint
                0,  // capabilities (will be updated later)
            );
            
            if let Ok(record) = node_record {
                let mut dht_lock = dht_clone2.lock().await;
                if let Err(e) = dht_lock.publish_own_record(record.clone()) {
                    println!("[transport] ⚠️  Failed to publish NodeRecord: {}", e);
                } else {
                    println!("[transport] ✅ Published NodeRecord to DHT: {}", record.node_name_short());
                }
            }
        });

        // 🔒 Periodic NodeRecord replication (refresh every soft TTL / 2 = 30 minutes)
        let dht_clone3 = dht.clone();
        let identity_clone2 = identity.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30 * 60));
            let mut sequence = 0u64;
            
            loop {
                interval.tick().await;
                sequence += 1;
                
                let node_record = crate::dht::record::NodeRecord::new(
                    &identity_clone2,
                    sequence,
                    None,  // endpoint
                    0,  // capabilities
                );
                
                if let Ok(record) = node_record {
                    let mut dht_lock = dht_clone3.lock().await;
                    match dht_lock.publish_own_record(record.clone()) {
                        Ok(()) => {
                            println!("[transport] 🔄 Republished NodeRecord (seq={}): {}", 
                                     sequence, record.node_name_short());
                        }
                        Err(e) => {
                            println!("[transport] ⚠️  Failed to republish NodeRecord: {}", e);
                        }
                    }
                    
                    // Cleanup expired records
                    dht_lock.cleanup_node_records();
                }
            }
        });


        // Spawn stream manager task
        let streams_clone = streams.clone();
        let socket_manager_for_streams = socket_manager.clone();
        let encryption_clone3 = encryption.clone();
        let peers_clone4 = peers.clone();
        tokio::spawn(async move {
            Self::stream_manager_task(streams_clone, socket_manager_for_streams, encryption_clone3, peers_clone4).await;
        });

        println!("[transport] ✅ P2P transport initialized");
        println!(
            "[transport] Fallback discovery: 0.0.0.0:{} (always open)",
            discovery_port
        );
        println!(
            "[transport] Fallback data:      0.0.0.0:{} (always open)",
            data_port
        );
        println!("[transport] Capabilities: 0b{:016b}", capabilities);

        // Create transport object first
        let transport = Self {
            identity,
            local_id: node_id.clone(),
            encryption: encryption.clone(),
            tunnels: tunnels.clone(),
            discovery_socket: discovery_socket.clone(),
            data_recv_socket: data_recv_socket.clone(),
            data_send_socket: data_send_socket.clone(),
            peers: peers.clone(),
            hello_tx: hello_tx.clone(),
            capabilities,
            dht: dht.clone(),
            streams: streams.clone(),
            exit_handler_tx: exit_handler_tx.clone(),
            proxy_gateway_tx: proxy_gateway_tx.clone(),
            proxy_request_tx: proxy_request_tx.clone(),
            proxy_response_tx: proxy_response_tx.clone(),
            proxy_tunnel_data_tx: proxy_tunnel_data_tx.clone(),
            socks5_gateway_tx: socks5_gateway_tx.clone(),
            socks5_request_tx: socks5_request_tx.clone(),
            socks5_response_tx: socks5_response_tx.clone(),
            socks5_tunnel_data_tx: socks5_tunnel_data_tx.clone(),
            nack_tx: nack_tx.clone(), // 🔄 NACK channel
            station: station.clone(),
            tun_wagon_tx: tun_wagon_tx.clone(),
            tun_wagon_resp_tx: tun_wagon_resp_tx.clone(),
            p2p_tunnel_tx: p2p_tunnel_tx.clone(),
            chat_packet_tx: chat_packet_tx.clone(),
            group_packet_tx: group_packet_tx.clone(),
            new_peer_tx: new_peer_tx.clone(),
            external_ip: Arc::new(tokio::sync::RwLock::new(external_ip.clone())),
            local_nat_mapping: Arc::new(tokio::sync::RwLock::new(MappingBehavior::Unknown)),
            mapping_probes: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            ws_outgoing: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            paired_clients: {
                let path = crate::netlayer::pairing::default_paired_clients_path();
                let store = crate::netlayer::pairing::PairedClientStore::load_or_default(&path);
                Arc::new(tokio::sync::Mutex::new(store))
            },
            paired_anchors: {
                let path = crate::netlayer::pairing::default_paired_anchors_path();
                let store = crate::netlayer::pairing::PairedAnchorStore::load_or_default(&path);
                Arc::new(tokio::sync::Mutex::new(store))
            },
            jurisdiction_index: Arc::new(crate::dht::JurisdictionIndex::new()),
            topology: topology.clone(),
            state_manager: Arc::new(tokio::sync::Mutex::new(crate::state_manager::StateManager::new(crate::state_manager::StateManagerConfig::default()))),

            telemetry_collector: Arc::new(tokio::sync::Mutex::new(crate::state_manager::TelemetryCollector::new(60))),

            last_control_update: Arc::new(tokio::sync::Mutex::new(std::time::Instant::now())),
            relay_manager: relay_manager.clone(),
            relay_request_tx: relay_request_tx.clone(),
            relay_response_tx: relay_response_tx.clone(),
            relay_data_tx: relay_data_tx.clone(),
            port_manager: port_manager.clone(),
            adaptive_controller: adaptive_controller.clone(),
            socket_manager: socket_manager.clone(),
            rx_bytes_counter: Arc::new(AtomicU64::new(0)),
            tx_bytes_counter: Arc::new(AtomicU64::new(0)),
            last_rx_bytes: Arc::new(tokio::sync::Mutex::new(0)),
            last_rx_time: Arc::new(tokio::sync::Mutex::new(std::time::Instant::now())),
            web_metrics_cache: Arc::new(tokio::sync::Mutex::new(WebMetricsCache::default())),
            port_rotation_tx: port_rotation_tx.clone(),
            active_rotated_listeners: Arc::new(tokio::sync::Mutex::new(None)),
            pinned_identities: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            seen_hello_nonces: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            rekey_requested: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            admission: Arc::new(tokio::sync::Mutex::new(crate::netlayer::admission::Admission::new(crate::netlayer::admission::AdmissionConfig::for_power(
                crate::util::NodePower::from_capability_bits(capabilities).unwrap_or(crate::util::NodePower::Medium),
            )))),
        };

        // Create Arc<P2PTransport> for Station
        let transport_arc = Arc::new(transport);
        // Spawn heartbeat task
        let tunnels_clone = tunnels.clone();
        let dht_clone = dht.clone();
        let encryption_clone = encryption.clone();
        let peers_clone = peers.clone();
        let transport_arc_heartbeat = transport_arc.clone();
        tokio::spawn(async move {
            Self::heartbeat_task(transport_arc_heartbeat, tunnels_clone, dht_clone, node_id.clone(), encryption_clone, peers_clone).await;
        });

        // 📡 WS-over-TLS server: anchor принимает входящие соединения от мобилок на 8443.
        // Mobile / non-anchor его не запускают.
        if transport_arc.is_anchor() {
            let transport_arc_ws = transport_arc.clone();
            let tls_node_id_hex = hex::encode(&transport_arc_ws.identity.node_id().0[..8]);
            tokio::spawn(async move {
                use crate::netlayer::tls_cert::TlsIdentity;
                use crate::netlayer::ws_transport::WsServer;

                let tls = match TlsIdentity::load_or_generate_default(
                    &tls_node_id_hex,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("[ws-server] TLS identity failed: {}", e);
                        return;
                    }
                };
                println!("[ws-server] 🔐 TLS fingerprint: {}", tls.fingerprint_hex);

                // Hardening Step 1: bind берётся из CLI override → config → default.
                // Если не парсится или bind зафейлился (например permission-denied на 443
                // без root) — пробуем fallback 0.0.0.0:8443 с warning'ом.
                let configured = crate::core::effective_ws_bind();
                let parsed: Option<std::net::SocketAddr> = configured.parse().ok();
                let fallback: std::net::SocketAddr = "0.0.0.0:8443".parse().unwrap();
                let mut server = match parsed {
                    Some(addr) => match WsServer::bind(addr, &tls).await {
                        Ok(s) => s,
                        Err(e) if addr != fallback => {
                            eprintln!("[ws-server] ⚠️  bind {} failed: {} — fallback на {}",
                                      addr, e, fallback);
                            match WsServer::bind(fallback, &tls).await {
                                Ok(s) => s,
                                Err(e2) => {
                                    eprintln!("[ws-server] fallback bind {} тоже failed: {} — disabled",
                                              fallback, e2);
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[ws-server] bind {} failed: {} — disabled", addr, e);
                            return;
                        }
                    },
                    None => {
                        eprintln!("[ws-server] ⚠️  не парсится ws-bind {:?} — fallback на {}",
                                  configured, fallback);
                        match WsServer::bind(fallback, &tls).await {
                            Ok(s) => s,
                            Err(e) => {
                                eprintln!("[ws-server] fallback bind {} failed: {} — disabled",
                                          fallback, e);
                                return;
                            }
                        }
                    }
                };
                println!("[ws-server] 🌐 listening on {} (wss://...)", server.local_addr);

                while let Some(mut conn) = server.accept_rx.recv().await {
                    let transport_for_pump = transport_arc_ws.clone();
                    tokio::spawn(async move {
                        // Hardening Step 2: первое сообщение — Hello ИЛИ plaintext 0xC0 RESUME.
                        // Hello → классический flow, ECDH → session.
                        // RESUME → fast-path: lookup session_id в paired_clients, restore session-key,
                        //          send 0xC1 ACK encrypted. Пропускаем Hello/ECDH.
                        let first_bytes = match tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            conn.incoming.recv(),
                        ).await {
                            Ok(Some(b)) => b,
                            _ => {
                                eprintln!("[ws-server] {} no first packet in 5s, dropping", conn.peer_addr);
                                return;
                            }
                        };

                        let peer_id: HashId = if first_bytes.first().copied() == Some(crate::netlayer::pairing::PKT_RESUME) {
                            // ----- plaintext-RESUME fast path -----
                            use crate::netlayer::pairing::{
                                decode_resume, encode_resume_ack, verify_resume_mac,
                                ResumeStatus, DEFAULT_SESSION_TTL_SECS,
                            };
                            let (embedded_node_id, session_id, addr, mac) = match decode_resume(&first_bytes) {
                                Ok(t) => t,
                                Err(e) => {
                                    eprintln!("[ws-server] {} bad RESUME: {}", conn.peer_addr, e);
                                    return;
                                }
                            };
                            // Lookup session_id в paired_clients store
                            let mut status = ResumeStatus::Unknown;
                            let mut hit_pk: Option<String> = None;
                            let mut sk: Option<[u8; 32]> = None;
                            {
                                let store = transport_for_pump.paired_clients.lock().await;
                                for (pk, tok) in store.clients.iter() {
                                    if tok.session_id == session_id {
                                        if tok.is_expired() {
                                            status = ResumeStatus::Expired;
                                            hit_pk = Some(pk.clone());
                                            break;
                                        }
                                        let secret = match tok.resume_secret() {
                                            Ok(s) => s,
                                            Err(_) => continue,
                                        };
                                        if verify_resume_mac(&secret, session_id, &addr, &mac)
                                            && Self::resume_node_matches(pk, &embedded_node_id)
                                            && tok.node_id_hex.as_deref().map_or(true, |h| h == hex::encode(embedded_node_id.0))
                                        {
                                            status = ResumeStatus::Ok;
                                            hit_pk = Some(pk.clone());
                                            sk = tok.session_key();
                                        } else {
                                            status = ResumeStatus::BadMac;
                                            hit_pk = Some(pk.clone());
                                        }
                                        break;
                                    }
                                }
                            }
                            if !matches!(status, ResumeStatus::Ok) || sk.is_none() {
                                eprintln!("[ws-server] RESUME from {} ({}) status={:?}, key={}",
                                          conn.peer_addr,
                                          hex::encode(&embedded_node_id.0[..8]),
                                          status, sk.is_some());
                                // Шлём plaintext ACK (mobile может его прочесть без сессии)
                                let ack = encode_resume_ack(status, None);
                                let _ = conn.outgoing.send(ack).await;
                                return;
                            }
                            // Refresh TTL & persist
                            if let Some(pk) = hit_pk {
                                let path = crate::netlayer::pairing::default_paired_clients_path();
                                let mut store = transport_for_pump.paired_clients.lock().await;
                                store.refresh(&pk, DEFAULT_SESSION_TTL_SECS);
                                if let Some(t) = store.clients.get_mut(&pk) {
                                    t.node_id_hex.get_or_insert_with(|| hex::encode(embedded_node_id.0));
                                }
                                if let Err(e) = store.save(&path) {
                                    eprintln!("[ws-server] persist paired_clients: {}", e);
                                }
                            }
                            // Restore session-key и регистрируем peer
                            let v = {
                                let mut enc = transport_for_pump.encryption.lock().await;
                                enc.restore_session(embedded_node_id, sk.unwrap())
                            };
                            println!("[ws-server] 🔁 RESUME {} v{} (session {:#x}, new addr {})",
                                     hex::encode(&embedded_node_id.0[..8]), v, session_id, addr);
                            let mut peer = PeerInfo::new(embedded_node_id, &conn.peer_addr);
                            peer.touch();
                            transport_for_pump.peers.lock().await.insert(embedded_node_id, peer);
                            transport_for_pump.ws_outgoing.lock().await
                                .insert(embedded_node_id, conn.outgoing.clone());
                            // Encrypted 0xC1 ACK
                            let ack_plain = encode_resume_ack(ResumeStatus::Ok, None);
                            match transport_for_pump.send_encrypted(embedded_node_id, &ack_plain).await {
                                Ok(_) => println!("[ws-server] 🔒 0xC1 sent encrypted to {}",
                                                  hex::encode(&embedded_node_id.0[..8])),
                                Err(e) => eprintln!("[ws-server] encrypted ACK send failed: {}", e),
                            }
                            embedded_node_id
                        } else {
                            // ----- classical Hello flow -----
                            let hello = match HelloPacket::from_bytes(&first_bytes) {
                                Ok(h) => h,
                                Err(e) => {
                                    eprintln!("[ws-server] {} bad Hello: {}", conn.peer_addr, e);
                                    return;
                                }
                            };
                            println!("[ws-server] 👋 Hello from {} ({})",
                                     hex::encode(&hello.node_id.0[..8]), conn.peer_addr);

                            // Same barriers as the UDP discovery path: signature + self-certifying
                            // identity + freshness, nonce replay, pinned/verified key conflict.
                            if let Err(e) = P2PTransport::verify_peer_handshake_static(&hello) {
                                eprintln!("[ws-server] {} Hello REJECTED: {}", conn.peer_addr, e);
                                return;
                            }
                            {
                                let mut seen = transport_for_pump.seen_hello_nonces.lock().await;
                                if !Self::check_replay(&mut seen, hello.node_id, hello.nonce) {
                                    eprintln!("[ws-server] {} Hello REJECTED: replayed nonce", conn.peer_addr);
                                    return;
                                }
                            }
                            {
                                let pins = transport_for_pump.pinned_identities.read().await;
                                if Self::identity_conflict(&pins, hello.node_id, &hello.public_key) {
                                    eprintln!("[ws-server] {} Hello REJECTED: pinned identity conflict", conn.peer_addr);
                                    return;
                                }
                            }
                            if let Some(existing) = transport_for_pump.peers.lock().await.get(&hello.node_id) {
                                if matches!(existing.verified_signing_pubkey, Some(k) if k != hello.public_key) {
                                    eprintln!("[ws-server] {} Hello REJECTED: verified key conflict", conn.peer_addr);
                                    return;
                                }
                            }

                            let mut peer = PeerInfo::new(hello.node_id, &conn.peer_addr);
                            peer.caps_bits = hello.capabilities;
                            peer.jurisdiction = hello.jurisdiction.clone();
                            peer.verified_signing_pubkey = Some(hello.public_key);
                            peer.touch();
                            transport_for_pump.peers.lock().await.insert(hello.node_id, peer);
                            transport_for_pump.ws_outgoing.lock().await
                                .insert(hello.node_id, conn.outgoing.clone());

                            let my_node_id = transport_for_pump.identity.node_id();
                            let mut cid8 = [0u8; 8];
                            cid8.copy_from_slice(&my_node_id.0[..8]);
                            let ack = HelloPacket::new_ack(
                                my_node_id,
                                transport_for_pump.identity.signing_public_key,
                                transport_for_pump.identity.public_key,
                                cid8,
                                transport_for_pump.capabilities,
                                hello.nonce,
                            );
                            let ack = match transport_for_pump.sign_hello(ack) {
                                Ok(a) => a,
                                Err(e) => {
                                    eprintln!("[ws-server] Hello-Ack sign: {}", e);
                                    return;
                                }
                            };
                            match ack.to_bytes() {
                                Ok(ack_bytes) => {
                                    if let Err(e) = conn.outgoing.send(ack_bytes).await {
                                        eprintln!("[ws-server] failed to send Hello-Ack: {}", e);
                                        return;
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[ws-server] Hello-Ack to_bytes: {}", e);
                                    return;
                                }
                            }
                            hello.node_id
                        };
                        while let Some(bytes) = conn.incoming.recv().await {
                            let dec = {
                                let mut enc = transport_for_pump.encryption.lock().await;
                                enc.decrypt_by_peer_id(&bytes)
                            };
                            match dec {
                                Ok((sender_id, plain)) => {
                                    if sender_id != peer_id {
                                        eprintln!("[ws-server] dropped: packet from {} on a connection bound to {}",
                                                  hex::encode(&sender_id.0[..8]), hex::encode(&peer_id.0[..8]));
                                        continue;
                                    }
                                    if let Some(p) = transport_for_pump.peers.lock().await.get_mut(&sender_id) {
                                        p.touch();
                                    }
                                    let handled = transport_for_pump
                                        .dispatch_decrypted_wagon(sender_id, &plain, WagonSource::Ws)
                                        .await;
                                    if !handled {
                                        // Не наш integration-пакет — legacy fallthrough на WS.
                                        // Полный legacy-match здесь пока не нужен: чат/прокси
                                        // через WS не ходят (они идут UDP-data_listener'ом).
                                        // Логируем как раньше для диагностики.
                                        println!("[ws-server] 📥 wagon {} B from {} (first byte 0x{:02x}) — not dispatched",
                                                 plain.len(),
                                                 hex::encode(&sender_id.0[..8]),
                                                 plain.first().copied().unwrap_or(0));
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[ws-server] decrypt failed from {}: {}",
                                              hex::encode(&peer_id.0[..8]), e);
                                }
                            }
                        }
                        // Соединение закрылось — убираем peer из ws_outgoing.
                        transport_for_pump.ws_outgoing.lock().await.remove(&peer_id);
                        println!("[ws-server] 🛑 connection {} closed", conn.peer_addr);
                    });
                }
            });
        }

        // 🔁 Step 6: Mobile auto-reconnect watchdog.
        // На Mobile запускаем task который следит за наличием primary anchor в ws_outgoing.
        // Если anchor отсутствует — пробуем connect_to_anchor_ws по списку из
        // PairedAnchorStore (primary, secondary, ...). Exponential backoff 1s..60s.
        if transport_arc.is_mobile() {
            let transport_arc_watchdog = transport_arc.clone();
            tokio::spawn(async move {
                let mut backoff_secs = 1u64;
                let max_backoff = 60u64;
                // Тёплый старт: ждём 5 с чтобы основная инициализация прошла.
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                loop {
                    let store_snapshot = {
                        let s = transport_arc_watchdog.paired_anchors.lock().await;
                        s.anchors.clone()
                    };
                    if store_snapshot.is_empty() {
                        // Нет paired anchor'ов — нечего реконнектить. Спим долго.
                        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                        continue;
                    }
                    // Есть ли уже live-link к какому-нибудь из наших anchor'ов?
                    let live = {
                        let ws = transport_arc_watchdog.ws_outgoing.lock().await;
                        store_snapshot.iter().any(|e| ws.contains_key(&e.payload.anchor_id))
                    };
                    if live {
                        backoff_secs = 1;
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        continue;
                    }
                    // Не подключены ни к одному paired anchor'у — пробуем по списку.
                    let mut connected = false;
                    for entry in store_snapshot.iter() {
                        let url = &entry.payload.anchor_url;
                        let fp = &entry.payload.fingerprint_hex;
                        println!("[watchdog] 🔁 trying {} (fp prefix: {}…)",
                                 url, fp.chars().take(12).collect::<String>());
                        match transport_arc_watchdog.connect_to_anchor_ws(url, fp).await {
                            Ok(anchor_id) => {
                                println!("[watchdog] ✅ reconnected to {} ({})",
                                         url, hex::encode(&anchor_id.0[..8]));
                                connected = true;
                                break;
                            }
                            Err(e) => {
                                eprintln!("[watchdog] ❌ {}: {}", url, e);
                            }
                        }
                    }
                    if connected {
                        backoff_secs = 1;
                    } else {
                        backoff_secs = (backoff_secs.saturating_mul(2)).min(max_backoff);
                        eprintln!("[watchdog] all paired anchors unreachable; backoff {}s", backoff_secs);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                }
            });
        }

        // 🌐 NAT-PMP: пытаемся открыть порт на роутере (RFC 6886).
        // На Mobile / lite-клиенте бессмысленно (нет своего gateway или его трогать нельзя).
        if transport_arc.is_mobile() {
            println!("[transport] 📱 Mobile mode: NAT-PMP disabled");
        } else if crate::testnet::active() {
            // тренировочная копия узла порты на роутере не открывает
            println!("[transport] тренировочная сеть: NAT-PMP выключен");
        } else {
        let transport_arc_pmp = transport_arc.clone();
        let pmp_internal_port = DEFAULT_DATA_PORT;
        tokio::spawn(async move {
            // Первая попытка: тёплый старт через 3 с после стартапа.
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            loop {
                match crate::netlayer::nat_pmp::request_mapping(pmp_internal_port, 3600).await {
                    Ok(mapping) => {
                        let new_ip = mapping.external_ip.to_string();
                        let prev = transport_arc_pmp.external_ip.read().await.clone();
                        if prev.as_deref() != Some(new_ip.as_str()) {
                            *transport_arc_pmp.external_ip.write().await = Some(new_ip.clone());
                            println!("[nat-pmp] ✅ port {} → {}:{} (lifetime {}s) — external_ip обновлён",
                                     mapping.internal_port,
                                     crate::util::mask_ipv4(&new_ip),
                                     mapping.external_port,
                                     mapping.lifetime_secs);
                        } else {
                            println!("[nat-pmp] ✅ port {} mapping refreshed (lifetime {}s)",
                                     mapping.internal_port, mapping.lifetime_secs);
                        }
                        // Рефрешим за половину lifetime'a (clamp 600..1800 с).
                        let refresh = (mapping.lifetime_secs / 2).clamp(600, 1800) as u64;
                        tokio::time::sleep(tokio::time::Duration::from_secs(refresh)).await;
                    }
                    Err(e) => {
                        eprintln!("[nat-pmp] ⚠️  mapping failed: {} — retry in 30 min", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(1800)).await;
                    }
                }
            }
        });
        }

        // 🌐 NAT mapping probe: раз в 60 с спрашиваем у 2+ peer'ов какой у них наш observed addr.
        // Это позволяет различать EIM vs EDM (RFC 4787) — для решения о hole punching.
        // Mobile не нуждается в этом: всегда идёт через своего anchor'а.
        if !transport_arc.is_mobile() {
            let transport_arc_nat_probe = transport_arc.clone();
            tokio::spawn(async move {
                // Тёплый старт: ждём 15 с пока peer'ы выйдут на связь.
                tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
                let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let peer_ids: Vec<HashId> = {
                        let peers_lock = transport_arc_nat_probe.peers.lock().await;
                        peers_lock.keys().copied().take(4).collect()
                    };
                    if peer_ids.len() < 2 {
                        continue;
                    }
                    for pid in peer_ids {
                        let probe = vec![0xA2u8];
                        if let Err(e) = transport_arc_nat_probe.send_encrypted(pid, &probe).await {
                            eprintln!("[nat] mapping probe to {} failed: {}",
                                      hex::encode(&pid.0[..8]), e);
                        }
                    }
                }
            });
        } else {
            println!("[transport] 📱 Mobile mode: NAT mapping probe disabled");
        }

        // 🌐 External IP refresh: каждые 5 минут перепроверяем,
        // при смене — пере-broadcast Hello, чтобы peer'ы знали наш новый адрес.
        let transport_arc_ip_refresh = transport_arc.clone();
        tokio::spawn(async move {
            let svc = crate::netlayer::external_ip::ExternalIpService::new();
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300));
            interval.tick().await; // skip первый tick (только что определили на старте)
            loop {
                interval.tick().await;
                match svc.get_external_ip().await {
                    Ok(new_ip) => {
                        let prev = transport_arc_ip_refresh.external_ip.read().await.clone();
                        let changed = match &prev {
                            Some(p) => p != &new_ip,
                            None => true,
                        };
                        if changed {
                            *transport_arc_ip_refresh.external_ip.write().await = Some(new_ip.clone());
                            println!("[transport] 🌐 External IP changed: {:?} → {} — re-broadcasting Hello",
                                     prev, crate::util::mask_ipv4(&new_ip));
                            // Пере-broadcast Hello всем известным peer'ам.
                            let addrs: Vec<String> = {
                                let peers_lock = transport_arc_ip_refresh.peers.lock().await;
                                peers_lock.values().map(|p| p.addr.clone()).collect()
                            };
                            for addr in addrs {
                                if let Err(e) = transport_arc_ip_refresh.send_hello_request(&addr).await {
                                    eprintln!("[transport] ⚠️  re-Hello to {}: {}",
                                              crate::util::mask_ipv4(&addr), e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[transport] ⚠️  external_ip refresh failed: {}", e);
                    }
                }
            }
        });

        // Create Station with transport reference
        let station_arc = Arc::new(
            crate::protocol::Station::with_defaults(
                node_id.clone(),
                transport_arc.clone(),
            )
        );

        // Store Station in transport
        {
            let mut station_guard = transport_arc.station.lock().await;
            *station_guard = Some(station_arc);
        }

        println!("[transport] 🚂 Station initialized for YTP train handling");
        Self::spawn_discovery_listener_task(transport_arc.clone(), discovery_socket.clone(), None);
        Self::spawn_data_listener_task(transport_arc.clone(), data_recv_socket.clone(), None);

        // Spawn port rotation task
        let port_manager_clone = port_manager_for_rotation;
        let transport_arc_for_rotation = transport_arc.clone();
        tokio::spawn(async move {
            Self::port_rotation_task(port_manager_clone, transport_arc_for_rotation).await;
        });

//        // Spawn peer exchange task for network discovery
//
//        let transport_arc_peer_exchange = transport_arc.clone();
//
//        tokio::spawn(async move {
//
//            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(120)); // каждые 2 минуты
//
//            loop {
//
//                interval.tick().await;
//
//                
//
//                // Получаем список известных пиров
//
//                let peers: Vec<PeerInfo> = transport_arc_peer_exchange.get_peers().await;
//
//                if peers.is_empty() {
//
//                    continue;
//
//                }
//
//                
//
//                // Для каждого пира отправляем список
//
//                for peer in peers {
//
//                    let peer_id = peer.id;
//
//                    if let Err(e) = transport_arc_peer_exchange.send_peer_list(peer_id).await {
//
//                        println!("[transport] ⚠️  Failed to send peer list to {}: {}", 
//
//                                 hex::encode(&peer_id.0[..8]), e);
//
//                    } else {
//
//                        println!("[transport] 🔄 Sent peer list to {}", hex::encode(&peer_id.0[..8]));
//
//                    }
//
//                }
//
//            }
//
//        });



        Ok(transport_arc)
    }

    /// Tunnel monitor task - checks for expired tunnels periodically
    async fn tunnel_monitor(tunnels: Arc<Mutex<TunnelManager>>) {
        println!("[transport] 🔍 Tunnel monitor started");

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));

        loop {
            interval.tick().await;

            let expired = {
                let mut manager = tunnels.lock().await;
                manager.check_expired_tunnels()
            };

            if !expired.is_empty() {
                println!("[transport] ⚠️  {} expired tunnel(s) detected", expired.len());
                for peer_id in &expired {
                    println!("[transport]    Peer: {}", hex::encode(&peer_id.0[..8]));
                }
            }
        }
    }

    /// Heartbeat task - sends keepalive pings to active tunnels
    async fn heartbeat_task(transport: Arc<P2PTransport>, 
        tunnels: Arc<Mutex<TunnelManager>>,
        dht: Arc<Mutex<Kademlia>>,
        local_id: HashId,
        encryption: Arc<Mutex<EncryptionManager>>,
        peers: Arc<Mutex<HashMap<HashId, PeerInfo>>>,
    ) {
        println!("[transport] 💓 Heartbeat task started (interval: 5s)");

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        let mut seq_counter: u64 = 0;

        loop {
            interval.tick().await;
            // Update control plane (adaptive behavior)
            transport.update_control_plane().await;
            // Calculate RX speed
            let now = std::time::Instant::now();
            let current_rx = transport.rx_bytes_counter.load(Ordering::Relaxed);
            let mut last_rx_guard = transport.last_rx_bytes.lock().await;
            let mut last_time_guard = transport.last_rx_time.lock().await;
            let elapsed = now.duration_since(*last_time_guard).as_secs_f64();
            let rx_speed = if elapsed > 0.0 { (current_rx - *last_rx_guard) as f64 / elapsed } else { 0.0 };
            *last_rx_guard = current_rx;
            *last_time_guard = now;
            // tracing::debug!("[speed] RX: {:.2} B/s, {:.2} KB/s", rx_speed, rx_speed / 1024.0);

            // Get active tunnels
            let active_peers: Vec<(HashId, String)> = {
                let tunnels_lock = tunnels.lock().await;
                let active = tunnels_lock.get_active_tunnels();
                let mut peers_lock = peers.lock().await;
                active.iter().filter_map(|t| {
                    if let Some(peer) = peers_lock.get(&t.peer_id) {
                        let addr = peer.data_addr.as_ref().unwrap_or(&peer.addr).clone();
                        Some((t.peer_id, addr))
                    } else {
                        None
                    }
                }).collect()
            };

            // Перед отправкой — оценим, протух ли direct-путь, и переключим на relay при необходимости.
            // Порог: peer.last_seen старше 11 секунд = пропущено ≥ 2 heartbeat ACK подряд.
            // 3 пропуска подряд → use_relay = true (отдаём весь трафик к нему через relay).
            const MISS_THRESHOLD_MS: u128 = 11_000;
            const MISS_STREAK_TO_FAIL_OVER: u8 = 3;
            let mut newly_relayed: Vec<HashId> = Vec::new();
            {
                let mut peers_lock = peers.lock().await;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                for peer in peers_lock.values_mut() {
                    if now_ms.saturating_sub(peer.last_seen) > MISS_THRESHOLD_MS {
                        peer.direct_miss_streak = peer.direct_miss_streak.saturating_add(1);
                        if peer.direct_miss_streak >= MISS_STREAK_TO_FAIL_OVER && !peer.use_relay {
                            peer.use_relay = true;
                            newly_relayed.push(peer.id);
                            println!("[transport] 🛰  peer {} → ROUTE VIA RELAY ({} miss streak)",
                                     hex::encode(&peer.id.0[..8]),
                                     peer.direct_miss_streak);
                        }
                    }
                }
            }
            // Параллельно пытаемся инициировать hole punching — может откроем direct ещё до того,
            // как relay нагрузится трафиком.
            for tid in newly_relayed {
                let t = transport.clone();
                tokio::spawn(async move {
                    if let Err(e) = t.initiate_hole_punch(tid).await {
                        eprintln!("[punch] initiate failed for {}: {}",
                                  hex::encode(&tid.0[..8]), e);
                    }
                });
            }

            // Send heartbeat REQ to each active peer
            for (peer_id, addr) in active_peers {
                // Create HEARTBEAT_REQ message: [MSG_TYPE:1][SEQ:8][TIMESTAMP:8]
                let mut heartbeat_req = vec![0u8; 17];
                heartbeat_req[0] = 0x01; // HEARTBEAT_REQ
                heartbeat_req[1..9].copy_from_slice(&seq_counter.to_be_bytes());
                heartbeat_req[9..17].copy_from_slice(
                    &std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        .to_be_bytes()
                );

                seq_counter = seq_counter.wrapping_add(1);

                // Encrypt and send
                let peer_info = {
                    let peers_lock = peers.lock().await;
                    peers_lock.get(&peer_id).cloned()
                };

                if let Some(peer) = peer_info {
                    if peer.use_relay {
                        // Через relay: используем общий send_encrypted, который
                        // сам обернёт в RelayDataPacket. Параллельно пробуем
                        // direct probe раз в ~30 с — для восстановления direct.
                        if let Err(e) = transport.send_encrypted(peer_id, &heartbeat_req).await {
                            println!("[transport] ⚠️  heartbeat via relay failed for {}: {}",
                                     hex::encode(&peer_id.0[..8]), e);
                        }
                        // Direct probe раз в 30 с (каждые 6 тиков по 5 с).
                        if seq_counter % 6 == 0 {
                            let encrypted = {
                                let enc_lock = encryption.lock().await;
                                enc_lock.encrypt(&peer, &heartbeat_req)
                            };
                            if let Ok(data) = encrypted {
                                let _ = transport.socket_manager.data().await
                                    .send_to(&data, &addr).await;
                            }
                        }
                    } else {
                        let encrypted = {
                            let enc_lock = encryption.lock().await;
                            enc_lock.encrypt(&peer, &heartbeat_req)
                        };

                        if let Ok(data) = encrypted {
                            if let Err(e) = transport.socket_manager.data().await.send_to(&data, &addr).await {
                                println!("[transport] ⚠️  Failed to send heartbeat REQ to {}: {}",
                                         crate::util::mask_ipv4(&addr), e);
                            }
                        }
                    }
                }
            }
        }
    }


    fn spawn_discovery_listener_task(
        transport: Arc<P2PTransport>,
        socket: Arc<CSocket>,
        shutdown: Option<oneshot::Receiver<()>>,
    ) {
        let peers = transport.peers.clone();
        let hello_tx = transport.hello_tx.clone();
        let dht = transport.dht.clone();
        let local_id = transport.local_id.clone();
        let new_peer_tx = transport.new_peer_tx.clone();
        let external_ip = transport.external_ip.clone();
        let jurisdiction_index = transport.jurisdiction_index.clone();
        let pinned_identities = transport.pinned_identities.clone();
        let seen_hello_nonces = transport.seen_hello_nonces.clone();
        let admission = transport.admission.clone();

        tokio::spawn(async move {
            Self::discovery_listener(
                socket,
                peers,
                hello_tx,
                dht,
                local_id,
                new_peer_tx,
                external_ip,
                jurisdiction_index,
                pinned_identities,
                seen_hello_nonces,
                admission,
                shutdown,
            ).await;
        });
    }

    fn spawn_data_listener_task(
        transport: Arc<P2PTransport>,
        socket: Arc<CSocket>,
        shutdown: Option<oneshot::Receiver<()>>,
    ) {
        let peers = transport.peers.clone();
        let encryption = transport.encryption.clone();
        let tunnels = transport.tunnels.clone();
        let dht = transport.dht.clone();
        let local_id = transport.local_id.clone();
        let streams = transport.streams.clone();
        let exit_handler_tx = transport.exit_handler_tx.clone();
        let proxy_gateway_tx = transport.proxy_gateway_tx.clone();
        let proxy_request_tx = transport.proxy_request_tx.clone();
        let proxy_response_tx = transport.proxy_response_tx.clone();
        let proxy_tunnel_data_tx = transport.proxy_tunnel_data_tx.clone();
        let nack_tx = transport.nack_tx.clone();
        let socks5_gateway_tx = transport.socks5_gateway_tx.clone();
        let socks5_request_tx = transport.socks5_request_tx.clone();
        let socks5_response_tx = transport.socks5_response_tx.clone();
        let socks5_tunnel_data_tx = transport.socks5_tunnel_data_tx.clone();
        let tun_wagon_tx = transport.tun_wagon_tx.clone();
        let tun_wagon_resp_tx = transport.tun_wagon_resp_tx.clone();
        let p2p_tunnel_tx = transport.p2p_tunnel_tx.clone();
        let chat_packet_tx = transport.chat_packet_tx.clone();
        let group_packet_tx = transport.group_packet_tx.clone();
        let station = Some(transport.station.clone());
        let relay_request_tx = transport.relay_request_tx.clone();
        let relay_response_tx = transport.relay_response_tx.clone();
        let relay_data_tx = transport.relay_data_tx.clone();
        let transport_for_listener = transport.clone();

        tokio::spawn(async move {
            Self::data_listener(
                transport_for_listener,
                socket,
                peers,
                encryption,
                tunnels,
                dht,
                local_id,
                streams,
                exit_handler_tx,
                proxy_gateway_tx,
                proxy_request_tx,
                proxy_response_tx,
                proxy_tunnel_data_tx,
                nack_tx,
                socks5_gateway_tx,
                socks5_request_tx,
                socks5_response_tx,
                socks5_tunnel_data_tx,
                tun_wagon_tx,
                tun_wagon_resp_tx,
                p2p_tunnel_tx,
                chat_packet_tx,
                group_packet_tx,
                station,
                relay_request_tx,
                relay_response_tx,
                relay_data_tx,
                shutdown,
            ).await;
        });
    }

    fn rewrite_unspecified_endpoint(endpoint: &str, actual_ip: &std::net::IpAddr) -> String {
        if endpoint.starts_with("0.0.0.0:") {
            if let Some((_, port)) = endpoint.rsplit_once(':') {
                return format!("{}:{}", actual_ip, port);
            }
        }
        endpoint.to_string()
    }

    fn active_port_summary(&self) -> String {
        let state = self.port_manager.current_state();
        format!(
            "fallback={}/{}, active={}/{}",
            DEFAULT_DISCOVERY_PORT,
            DEFAULT_DATA_PORT,
            state.discovery_port,
            state.data_port
        )
    }

    /// Port rotation task - periodically rotates UDP ports for DPI resistance
    async fn port_rotation_task(
        port_manager: Arc<PortManager>,
        transport: Arc<P2PTransport>,
    ) {
        println!("[transport] 🔄 Port rotation task started (interval: 5+ min)");
        
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        let mut seq_counter: u64 = 0;
        
        loop {
            interval.tick().await;
            
            if port_manager.should_rotate() {
                println!("[port_rotation_task] should_rotate returned: {}", port_manager.should_rotate());
                println!("[transport] 🎯 Port rotation triggered!");
                seq_counter = seq_counter.wrapping_add(1);
                // Get current loss rate from State Manager
                let loss_rate = {
                    let sm = transport.state_manager.lock().await;
                    (sm.get_current_loss_rate() * 1000.0) as u16
                };
                // Get current RX speed
                let rx_speed = {
                    let current_rx = transport.rx_bytes_counter.load(Ordering::Relaxed);
                    let last_rx = *transport.last_rx_bytes.lock().await;
                    let elapsed = transport.last_rx_time.lock().await.elapsed().as_secs_f64();
                    if elapsed > 0.0 { (current_rx - last_rx) as u32 } else { 0 }
                };

                
                // Get current ports for logging
                let state = port_manager.current_state();
                println!("[transport] Current ports: {} (discovery), {} (data)",
                         state.discovery_port, state.data_port);
                
                // Generate new port pair
                let (new_discovery_port, new_data_port) = PortManager::generate_port_pair();
                
                println!("[transport] New ports: {} (discovery), {} (data)", 
                         new_discovery_port, new_data_port);
                
                // Create new sockets
                match Self::create_new_sockets(new_discovery_port, new_data_port).await {
                    Ok((new_discovery, new_data)) => {
                        println!("[transport] ✅ New sockets bound successfully");
                        let (new_discovery_stop_tx, new_discovery_stop_rx) = oneshot::channel();
                        let (new_data_stop_tx, new_data_stop_rx) = oneshot::channel();

                        Self::spawn_discovery_listener_task(
                            transport.clone(),
                            new_discovery.clone(),
                            Some(new_discovery_stop_rx),
                        );
                        Self::spawn_data_listener_task(
                            transport.clone(),
                            new_data.clone(),
                            Some(new_data_stop_rx),
                        );
                        
                        // === NOTIFY ALL PEERS ===
                        let active_peers: Vec<HashId> = {
                            let peers_lock = transport.peers.lock().await;
                            peers_lock.keys().copied().collect()
                        };
                        
                        let port_update_packet = PortUpdatePacket::new(
                            new_discovery_port,
                            new_data_port,
                            seq_counter,
                            loss_rate,
                            rx_speed
                        );
                        let packet_bytes = port_update_packet.to_bytes();
                        
                        println!("[transport] 📢 Notifying {} peers", active_peers.len());
                        
                        for peer_id in &active_peers {
                            let _ = transport.send_encrypted(*peer_id, &packet_bytes).await;
                        }
                        // === END NOTIFICATION ===
                        
                        // Update socket manager
                        transport.socket_manager.rotate(
                            new_discovery.clone(),
                            new_data.clone(),
                            new_data.clone(), // временно используем data сокет для p2p
                        ).await;
                        transport.port_manager
                            .set_current_ports(new_discovery_port, new_data_port)
                            .await;
                        
                        // Notify listeners about new sockets
                        let new_pair = SocketPair::new(new_discovery, new_data.clone(), new_data);
                        let _ = transport.port_rotation_tx.send(new_pair);

                        let previous_rotated = {
                            let mut listeners = transport.active_rotated_listeners.lock().await;
                            let previous = listeners.take();
                            *listeners = Some(RotatedListenerHandles {
                                discovery_port: new_discovery_port,
                                data_port: new_data_port,
                                discovery_stop: new_discovery_stop_tx,
                                data_stop: new_data_stop_tx,
                            });
                            previous
                        };

                        if let Some(previous) = previous_rotated {
                            let overlap = port_manager.overlap_duration();
                            tokio::spawn(async move {
                                println!(
                                    "[transport] ⏳ grace-old ports alive for {:?}: {}/{}",
                                    overlap, previous.discovery_port, previous.data_port
                                );
                                tokio::time::sleep(overlap).await;
                                let _ = previous.discovery_stop.send(());
                                let _ = previous.data_stop.send(());
                                println!(
                                    "[transport] 🔒 Closed expired grace-old ports: {}/{}",
                                    previous.discovery_port, previous.data_port
                                );
                            });
                        }
                        println!("[transport] 📍 Port state: {}", transport.active_port_summary());
                        println!("[transport] ✅ Socket rotation complete!");
                    }
                    Err(e) => {
                        eprintln!("[transport] ❌ Socket rotation failed: {}", e);
                        eprintln!("[transport] Will retry on next check");
                    }
                }
            }
        }
    }
    async fn create_new_sockets(
        discovery_port: u16,
        data_port: u16,
    ) -> Result<(Arc<CSocket>, Arc<CSocket>), String> {
        // Bind new discovery socket
        let new_discovery = UdpSocket::bind(format!("0.0.0.0:{}", discovery_port))
            .await
            .map_err(|e| format!("Failed to bind discovery socket to port {}: {}", discovery_port, e))?;
        let new_discovery = Arc::new(new_discovery);
        
        // Bind new data socket with large buffers
        let new_data = UdpSocket::bind(format!("0.0.0.0:{}", data_port))
            .await
            .map_err(|e| format!("Failed to bind data socket to port {}: {}", data_port, e))?;
        
        // Set large buffers for data socket (4MB for 60 Mbps streaming)
        #[cfg(unix)]
        {
            use libc::{setsockopt, SOL_SOCKET, SO_RCVBUF, SO_SNDBUF};
            use std::os::unix::io::AsRawFd;
            
            let std_socket = new_data.into_std()
                .map_err(|e| format!("Failed to convert socket: {}", e))?;
            let fd = std_socket.as_raw_fd();
            let buffer_size: i32 = 16 * 1024 * 1024; // 16 MB
            
            unsafe {
                setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &buffer_size as *const i32 as *const _, 
                           std::mem::size_of::<i32>() as u32);
                setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &buffer_size as *const i32 as *const _, 
                           std::mem::size_of::<i32>() as u32);
            }
            
            let new_data = tokio::net::UdpSocket::from_std(std_socket)
                .map_err(|e| format!("Failed to convert back to tokio: {}", e))?;
            let new_data = Arc::new(new_data);
            
            let carrier = tcp_carrier::global().ok_or_else(|| "tcp carrier is not installed".to_string())?;
            let new_discovery = CSocket::new(new_discovery, carrier.clone()).map_err(|e| e.to_string())?;
            let new_data = CSocket::new(new_data, carrier).map_err(|e| e.to_string())?;
            Ok((new_discovery, new_data))
        }
        
        #[cfg(not(unix))]
        {
            let new_data = Arc::new(new_data);
            let carrier = tcp_carrier::global().ok_or_else(|| "tcp carrier is not installed".to_string())?;
            let new_discovery = CSocket::new(new_discovery, carrier.clone()).map_err(|e| e.to_string())?;
            let new_data = CSocket::new(new_data, carrier).map_err(|e| e.to_string())?;
            Ok((new_discovery, new_data))
        }
    }

    /// Stream manager task - handles reliable streams
    async fn stream_manager_task(
        streams: SharedStreamRegistry,
        socket_manager: Arc<SocketManager>,
        encryption: Arc<Mutex<EncryptionManager>>,
        peers: Arc<Mutex<HashMap<HashId, PeerInfo>>>,
    ) {
        println!("[transport] 🌊 Stream manager started");

        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(100));

        loop {
            interval.tick().await;

            let now = std::time::Instant::now();

            // Get all streams that need to send packets
            let mut frames_to_send: Vec<(HashId, StreamFrame)> = Vec::new();

            {
                let mut streams_lock = streams.lock().await;
                let stream_ids = streams_lock.all_stream_ids();

                // Skip if no streams
                if stream_ids.is_empty() {
                    continue;
                }

                for stream_id in stream_ids {
                    if let Some(stream) = streams_lock.get_stream_mut(stream_id) {
                        // Use max_inflight limit (128 default)
                        let packets = stream.get_packets_to_send(now, 128);
                        if !packets.is_empty() {
                            eprintln!("[stream-manager] Stream {} has {} packets to send",
                                     stream_id, packets.len());
                        }
                        for frame in packets {
                            frames_to_send.push((stream.peer_id, frame));
                        }
                    }
                }

                // Cleanup expired streams (30 seconds - more aggressive cleanup)
                let cleaned = streams_lock.cleanup(30);
                if cleaned > 0 {
                    println!("[transport] 🧹 Cleaned up {} expired streams", cleaned);
                }
            }

            // Skip if nothing to send
            if frames_to_send.is_empty() {
                continue;
            }

            eprintln!("[stream-manager] 📤 Total {} frames to send", frames_to_send.len());

            // Send frames
            for (peer_id, frame) in frames_to_send {
                let peer_info = {
                    let peers_lock = peers.lock().await;
                    peers_lock.get(&peer_id).cloned()
                };

                if let Some(peer) = peer_info {
                    let frame_bytes = frame.to_bytes();

                    eprintln!("[stream-manager] 📦 Sending frame: stream_id={}, type={:?}, len={}",
                             frame.header.stream_id, frame.header.msg_type, frame_bytes.len());

                    let encrypted = {
                        let enc_lock = encryption.lock().await;
                        enc_lock.encrypt(&peer, &frame_bytes)
                    };

                    if let Ok(data) = encrypted {
                        let addr = peer.data_addr.as_ref().unwrap_or(&peer.addr);
                        eprintln!("[stream-manager] 🔒 Encrypted to {} bytes, sending to {}",
                                 data.len(), addr);
                        if let Err(e) = socket_manager.data().await.send_to(&data, addr).await {
                            eprintln!("[stream-manager] ❌ Send failed: {}", e);
                        } else {
                            eprintln!("[stream-manager] ✅ Sent successfully");
                        }
                    } else {
                        eprintln!("[stream-manager] ❌ Encryption failed");
                    }
                }
            }
        }
    }

    /// Sign a Hello/Hello-Ack with the node's identity (same scheme as the UDP discovery path).
    fn sign_hello(&self, mut hello: HelloPacket) -> Result<HelloPacket, String> {
        let signature = self.identity.sign(&hello.challenge_data())
            .map_err(|e| format!("Failed to sign Hello: {}", e))?;
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&signature);
        hello.signature = crate::netlayer::packet::Signature(sig_bytes);
        Ok(hello)
    }

    /// RESUME binding: the node id claimed in a plaintext RESUME must be acceptable for the Ed25519 key the token was issued to
    /// (store key = client signing key, hex): a tagged id has to be derived from that key; an old random id is accepted
    /// only before the sunset (and is then remembered in the token, see `SessionToken::node_id_hex`).
    fn resume_node_matches(client_pubkey_hex: &str, claimed: &HashId) -> bool {
        match hex::decode(client_pubkey_hex) {
            Ok(b) if b.len() == 32 => {
                let mut k = [0u8; 32];
                k.copy_from_slice(&b);
                let now_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                crate::util::types::id_acceptable(&claimed.0, &k, now_s)
            }
            _ => false,
        }
    }

    /// Node Identity Binding Fix: true if `node_id` is pinned (from the
    /// owner's own trusted_peers.json) to a signing key OTHER than
    /// `claimed_pubkey` — a genuine, hard identity conflict. False
    /// (permissive) for an unpinned node_id, matching prior behavior for
    /// every peer the owner hasn't explicitly recorded.
    fn identity_conflict(
        pins: &HashMap<HashId, [u8; 32]>,
        node_id: HashId,
        claimed_pubkey: &[u8; 32],
    ) -> bool {
        matches!(pins.get(&node_id), Some(pinned) if pinned != claimed_pubkey)
    }

    /// Node Identity Binding Fix — Barrier 3: true (fresh, accept) the FIRST
    /// time a (node_id, nonce) pair is seen; false (replay, reject) on any
    /// repeat within the freshness window `verify_peer_handshake_static`
    /// already enforces (±5 minutes). Opportunistically evicts entries older
    /// than that window so the map never grows unbounded — anything older
    /// would already fail the timestamp check on its own, so it can never
    /// legitimately need to be remembered longer than that.
    fn check_replay(
        seen: &mut HashMap<(HashId, u64), Instant>,
        node_id: HashId,
        nonce: u64,
    ) -> bool {
        let now = Instant::now();
        seen.retain(|_, seen_at| now.duration_since(*seen_at) < std::time::Duration::from_secs(5 * 60));
        if seen.contains_key(&(node_id, nonce)) {
            return false;
        }
        seen.insert((node_id, nonce), now);
        true
    }

    /// Discovery listener task (port 9000)
    ///
    /// Handles Hello Request/Ack packets
    async fn discovery_listener(
        socket: Arc<CSocket>,
        peers: Arc<Mutex<HashMap<HashId, PeerInfo>>>,
        hello_tx: tokio::sync::broadcast::Sender<HelloEvent>,
        dht: Arc<Mutex<Kademlia>>,
        local_id: HashId,
        new_peer_tx: Option<mpsc::Sender<PeerInfo>>,
        external_ip: Arc<tokio::sync::RwLock<Option<String>>>,
        jurisdiction_index: Arc<crate::dht::JurisdictionIndex>,
        pinned_identities: Arc<tokio::sync::RwLock<HashMap<HashId, [u8; 32]>>>,
        seen_hello_nonces: Arc<tokio::sync::Mutex<HashMap<(HashId, u64), Instant>>>,
        admission: Arc<tokio::sync::Mutex<crate::netlayer::admission::Admission>>,
        mut shutdown: Option<oneshot::Receiver<()>>,
    ) {
        let local_addr = socket.local_addr().ok();
        let listener_role = local_addr
            .map(|addr| {
                if addr.port() == DEFAULT_DISCOVERY_PORT {
                    "fallback"
                } else {
                    "rotated"
                }
            })
            .unwrap_or("unknown");
        println!(
            "[transport] 📡 Starting {} discovery listener on {:?}",
            listener_role, local_addr
        );

        let mut buf = [0u8; 65535];

        loop {
            let recv_result = if let Some(stop_rx) = shutdown.as_mut() {
                tokio::select! {
                    _ = stop_rx => {
                        println!(
                            "[transport] 📴 {} discovery listener stopped on {:?}",
                            listener_role, local_addr
                        );
                        break;
                    }
                    result = socket.recv_from(&mut buf) => result,
                }
            } else {
                socket.recv_from(&mut buf).await
            };

            match recv_result {
                Ok((len, from)) => {
                    let data = buf[..len].to_vec();

                    // Try to parse as Hello packet
                    match HelloPacket::from_bytes(&data) {
                        Ok(hello_packet) => {
                            println!("[transport] 📨 Received Hello packet from {}", from);

                            // 🔒 Two-Phase Peer Verification (Stage 1.3)
                            match P2PTransport::verify_peer_handshake_static(&hello_packet) {
                                Ok(()) => {
                                    println!("[transport] ✅ Handshake successful, processing Hello packet");
                                }
                                Err(e) => {
                                    println!("[transport] ⚠️  REJECTED: {}", e);
                                    continue;
                                }
                            }

                            // 🚧 Допуск: подписанное приветствие от неизвестной личности ничего не доказывает (личность создаётся бесплатно),
                            // поэтому новых узлов принимаем с ограничениями, а таблица пиров не растёт выше потолка.
                            {
                                let (known, table_len) = {
                                    let p = peers.lock().await;
                                    (p.contains_key(&hello_packet.node_id), p.len())
                                };
                                let verdict = admission.lock().await.check(from.ip(), hello_packet.node_id, known, table_len, Instant::now());
                                let verdict = match verdict {
                                    Err(crate::netlayer::admission::Reject::TableFull) => {
                                        // полна: вытесняем самого давнего неприкреплённого, не якоря; если некого — отказ
                                        let pins = pinned_identities.read().await;
                                        let mut p = peers.lock().await;
                                        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                                        let victim = crate::netlayer::admission::pick_victim(
                                            p.values().map(|x| (x.id, x.last_seen, pins.contains_key(&x.id), (x.caps_bits & crate::netlayer::packet::hello_caps::ANCHOR) != 0)),
                                            now_ms,
                                            120_000,
                                        );
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
                                    let total = admission.lock().await.rejected_total();
                                    if total.is_power_of_two() || total % 1000 == 0 {
                                        println!("[transport] 🚧 Hello refused ({}): {} refused in total", reason.name(), total);
                                    }
                                    continue;
                                }
                            }

                            // 🔒 Node Identity Binding Fix — Barrier 3.
                            // Signature + timestamp-window checks above prove
                            // this Hello was validly signed within the last
                            // ±5 minutes — NOT that it was signed just now.
                            // A captured copy of a real, still-fresh Hello is
                            // byte-for-byte re-signable by nobody but replay
                            // needs no key at all: resending the same bytes
                            // from a different source address would still
                            // pass every check above, and every accepted
                            // Hello below unconditionally overwrites the
                            // sender's PeerInfo.addr + DHT entry with
                            // whatever address it arrived from. Reject any
                            // (node_id, nonce) pair already accepted inside
                            // the freshness window.
                            {
                                let mut seen = seen_hello_nonces.lock().await;
                                if !Self::check_replay(&mut seen, hello_packet.node_id, hello_packet.nonce) {
                                    println!(
                                        "[transport] ⛔ REJECTED: REPLAY — Hello for node_id {} reuses a nonce already accepted within the freshness window (from {})",
                                        hex::encode(&hello_packet.node_id.0[..8]), from
                                    );
                                    continue;
                                }
                            }

                            // 🔒 Node Identity Binding Fix — Barrier 1.
                            // A Hello passing Ed25519 verification only proves
                            // internal self-consistency (signer's own claimed
                            // key matches its own claimed node_id in THIS
                            // packet) — it proves nothing about whether this
                            // is the SAME node_id we've trusted/seen before.
                            // Checked BEFORE any PeerInfo/DHT/online-state
                            // update, exactly like the IPv6-spoofing check
                            // right below it.
                            {
                                let pins = pinned_identities.read().await;
                                if Self::identity_conflict(&pins, hello_packet.node_id, &hello_packet.public_key) {
                                    println!(
                                        "[transport] ⛔ REJECTED: IDENTITY_CONFLICT — node_id {} is pinned to a different signing key than this Hello claims (from {})",
                                        hex::encode(&hello_packet.node_id.0[..8]), from
                                    );
                                    continue;
                                }
                            }
                            if let Some(existing) = peers.lock().await.get(&hello_packet.node_id) {
                                if let Some(known_key) = existing.verified_signing_pubkey {
                                    if known_key != hello_packet.public_key {
                                        println!(
                                            "[transport] ⛔ REJECTED: IDENTITY_CONFLICT — node_id {} previously verified with a different signing key (from {})",
                                            hex::encode(&hello_packet.node_id.0[..8]), from
                                        );
                                        continue;
                                    }
                                }
                            }



                            // 🔒 Verify IPv6 virtual address (Stage 2.2: Prevent spoofing)
                            if hello_packet.ipv6_virtual.is_some() && !hello_packet.verify_ipv6_virtual() {
                                println!("[transport] ⚠️  REJECTED: IPv6 virtual address spoofing detected!");
                                println!("[transport]    node_id: {}", hex::encode(&hello_packet.node_id.0[..8]));
                                if let Some(ipv6) = hello_packet.ipv6_virtual {
                                    println!("[transport]    provided ipv6: {}", hex::encode(&ipv6[..]));
                                }
                                let expected = hello_packet.expected_ipv6_virtual();
                                println!("[transport]    expected ipv6: {}", hex::encode(&expected[..]));
                                continue;
                            }

                            // Handle Hello packet
                            let event = match hello_packet.hello_type {
                                HelloType::Request => {
                                    println!("[transport]   Type: HELLO_REQ from {}",
                                             hex::encode(&hello_packet.node_id.0[..8]));

                                    // Add peer to known peers
                                    // Extract discovery_endpoint (port 10000) and replace 0.0.0.0 with real IP
                                    let data_endpoint = hello_packet.discovery_endpoint
                                        .as_ref()
                                        .map(|ep| Self::rewrite_unspecified_endpoint(ep, &from.ip()));

                                    // Extract p2p_data_addr (port 9998) and replace 0.0.0.0 with real IP
                                    let p2p_data_endpoint = hello_packet.p2p_data_addr
                                        .as_ref()
                                        .map(|ep| Self::rewrite_unspecified_endpoint(ep, &from.ip()));

                                    // Extract P2P X25519 public key from peer
                                    let p2p_x25519_key = hello_packet.p2p_x25519_public;
                                    println!("[transport] 🔑 Received P2P public key from peer: {:02x?}", p2p_x25519_key);

                                    let mut peer = if let Some(data_ep) = data_endpoint {
                                        PeerInfo::with_data_addr(hello_packet.node_id, &from.to_string(), &data_ep)
                                    } else {
                                        PeerInfo::new(hello_packet.node_id, &from.to_string())
                                    };

                                    // Store IPv6 virtual address
                                    peer.ipv6_virtual = hello_packet.ipv6_virtual;
                                    // Node Identity Binding Fix: record the key this Hello
                                    // proved ownership of, so a LATER Hello for the same
                                    // node_id with a different key is caught even without
                                    // a static pin (see the general check above).
                                    peer.verified_signing_pubkey = Some(hello_packet.public_key);
                                    // Store P2P X25519 public key
                                    peer.p2p_x25519_public = p2p_x25519_key;

                                    // Store P2P data address
                                    peer.p2p_data_addr = p2p_data_endpoint;

                                    // 🛰  Берём caps_bits из Hello — peer объявляет роль через них.
                                    peer.caps_bits = hello_packet.capabilities;
                                    // 🌍 Iter 3: jurisdiction self-claim peer'а (опц.).
                                    peer.jurisdiction = hello_packet.jurisdiction.clone();
                                    // 🆕 Hardening Step 5: если peer объявил себя anchor'ом
                                    // с jurisdiction TLV — фиксируем в root-level индексе,
                                    // чтобы find_anchors_by_jurisdiction мог его найти даже
                                    // когда peer-table очистилась.
                                    if (peer.caps_bits & crate::netlayer::packet::hello_caps::ANCHOR) != 0 {
                                        if let Some(j) = peer.jurisdiction.as_ref() {
                                            jurisdiction_index.announce(
                                                j,
                                                peer.id,
                                                from.to_string(),
                                            );
                                        }
                                    }

                                    // Detect NAT status from Hello packet
                                    let ext_ip_snapshot = external_ip.read().await.clone();
                                    let nat_status = NatStatus::from_hello_packet(
                                        &hello_packet,
                                        &from,
                                        ext_ip_snapshot.as_deref(),
                                    );
                                    peer.set_nat_status(nat_status);
                                    println!("[transport] 👤 Peer {} NAT status: {}",
                                             hex::encode(&peer.id.0[..8]), nat_status.as_str());

                                    peers.lock().await.insert(peer.id, peer.clone());

                                    // Notify P2P transport about new peer
                                    if let Some(ref tx) = new_peer_tx {
                                        let _ = tx.send(peer.clone()).await;
                                    }

                                    // Add peer to DHT
                                    {
                                        let mut dht_lock = dht.lock().await;
                                        dht_lock.add_peer(peer.id, from.to_string());
                                        println!("[transport] 📊 Added peer to DHT: {} (total peers: {})",
                                                 hex::encode(&peer.id.0[..8]), dht_lock.ktable.peer_count());
                                    }

                                    HelloEvent::Request {
                                        from,
                                        packet: hello_packet,
                                    }
                                }
                                HelloType::Ack => {
                                    println!("[transport]   Type: HELLO_ACK from {}",
                                             hex::encode(&hello_packet.node_id.0[..8]));

                                    // Add peer to known peers
                                    let data_endpoint = hello_packet.discovery_endpoint
                                        .as_ref()
                                        .map(|ep| Self::rewrite_unspecified_endpoint(ep, &from.ip()));

                                    // Extract p2p_data_addr (port 9998) and replace 0.0.0.0 with real IP
                                    let p2p_data_endpoint = hello_packet.p2p_data_addr
                                        .as_ref()
                                        .map(|ep| Self::rewrite_unspecified_endpoint(ep, &from.ip()));

                                    // Extract P2P X25519 public key from peer
                                    let p2p_x25519_key = hello_packet.p2p_x25519_public;

                                    let mut peer = if let Some(data_ep) = data_endpoint {
                                        PeerInfo::with_data_addr(hello_packet.node_id, &from.to_string(), &data_ep)
                                    } else {
                                        PeerInfo::new(hello_packet.node_id, &from.to_string())
                                    };

                                    peer.ipv6_virtual = hello_packet.ipv6_virtual;
                                    // Node Identity Binding Fix: record the key this Hello
                                    // proved ownership of, so a LATER Hello for the same
                                    // node_id with a different key is caught even without
                                    // a static pin (see the general check above).
                                    peer.verified_signing_pubkey = Some(hello_packet.public_key);
                                    // Store P2P X25519 public key
                                    peer.p2p_x25519_public = p2p_x25519_key;

                                    // Store P2P data address
                                    peer.p2p_data_addr = p2p_data_endpoint;

                                    // 🛰  Берём caps_bits из Hello.
                                    peer.caps_bits = hello_packet.capabilities;

                                    // Detect NAT status from Hello packet
                                    let ext_ip_snapshot = external_ip.read().await.clone();
                                    let nat_status = NatStatus::from_hello_packet(
                                        &hello_packet,
                                        &from,
                                        ext_ip_snapshot.as_deref(),
                                    );
                                    peer.set_nat_status(nat_status);
                                    println!("[transport] 👤 Peer {} NAT status: {}, caps=0b{:016b}",
                                             hex::encode(&peer.id.0[..8]), nat_status.as_str(), peer.caps_bits);

                                    peers.lock().await.insert(peer.id, peer.clone());

                                    // Notify P2P transport about new peer
                                    if let Some(ref tx) = new_peer_tx {
                                        let _ = tx.send(peer.clone()).await;
                                    }

                                    // Add peer to DHT
                                    {
                                        let mut dht_lock = dht.lock().await;
                                        dht_lock.add_peer(peer.id, from.to_string());
                                        println!("[transport] 📊 Added peer to DHT: {} (total peers: {})",
                                                 hex::encode(&peer.id.0[..8]), dht_lock.ktable.peer_count());
                                    }

                                    HelloEvent::Ack {
                                        from,
                                        packet: hello_packet,
                                    }
                                }
                            };

                            // Send event
                            let _ = hello_tx.send(event);
                        }
                        Err(e) => {
                            // Проверяем, является ли это "Invalid magic" ошибкой (чужой протокол)
                            if e.contains("Invalid magic") {
                                // Это не наш пакет - возможно сканирование портов или случайный трафик
                                // Silently игнорируем чтобы не засорять логи
                                eprintln!("[transport] 🚫 Discarding non-YANDI packet from {} (invalid magic)", from);
                            } else {
                                // Другие ошибки логируем полностью
                                println!("[transport] ⚠️  Invalid Hello packet from {}: {}", from, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("[transport] ❌ Discovery socket error: {}", e);
                }
            }
        }
    }

    /// Data listener task (port 10000)
    ///
    /// Handles encrypted data packets
    async fn data_listener(transport: Arc<P2PTransport>, 
        socket: Arc<CSocket>,
        peers: Arc<Mutex<HashMap<HashId, PeerInfo>>>,
        encryption: Arc<Mutex<EncryptionManager>>,
        tunnels: Arc<Mutex<TunnelManager>>,
        dht: Arc<Mutex<Kademlia>>,
        local_id: HashId,
        streams: SharedStreamRegistry,
        exit_handler_tx: Option<mpsc::Sender<ExitHandlerRequest>>,
        proxy_gateway_tx: Option<mpsc::Sender<ProxyGatewayRequest>>,
        proxy_request_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyRequest)>>,
        proxy_response_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyResponse)>>,
        proxy_tunnel_data_tx: Option<mpsc::Sender<(HashId, crate::proxy::ProxyTunnelData)>>,
        // 🔄 NACK handler
        nack_tx: Option<mpsc::Sender<(HashId, crate::protocol::WagonNack)>>,
        // SOCKS5 handlers
        socks5_gateway_tx: Option<mpsc::Sender<ProxyGatewayRequest>>,  // 🧦 SOCKS5 Gateway auto-start
        socks5_request_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5ProxyRequest)>>,
        socks5_response_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5ProxyResponse)>>,
        socks5_tunnel_data_tx: Option<mpsc::Sender<(HashId, crate::socks5::Socks5TunnelData)>>,
        // TUN wagon handler
        tun_wagon_tx: Option<mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagon)>>,
        tun_wagon_resp_tx: Option<mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagonResponse)>>,
        // P2P tunnel handler
        p2p_tunnel_tx: Option<mpsc::Sender<(HashId, Vec<u8>)>>,
        // Chat handler
        chat_packet_tx: Option<mpsc::Sender<(HashId, crate::communication::CommPacket)>>,
    group_packet_tx: Option<mpsc::Sender<(HashId, crate::communication::GroupPacket)>>,
        station: Option<Arc<Mutex<Option<Arc<crate::protocol::Station>>>>>,
        // Relay handlers
        relay_request_tx: Option<mpsc::Sender<(HashId, HashId)>>,
        relay_response_tx: Option<mpsc::Sender<(HashId, crate::netlayer::packet::RelayConnectResponse)>>,
        relay_data_tx: Option<mpsc::Sender<(HashId, HashId, Vec<u8>)>>,
        mut shutdown: Option<oneshot::Receiver<()>>,
    ) {
        let local_addr = socket.local_addr().ok();
        let listener_role = local_addr
            .map(|addr| {
                if addr.port() == DEFAULT_DATA_PORT {
                    "fallback"
                } else {
                    "rotated"
                }
            })
            .unwrap_or("unknown");
        println!(
            "[transport] 🔒 Starting {} data listener on {:?}",
            listener_role, local_addr
        );

        let mut buf = [0u8; 65535];

        loop {
            let recv_result = if let Some(stop_rx) = shutdown.as_mut() {
                tokio::select! {
                    _ = stop_rx => {
                        println!(
                            "[transport] 📴 {} data listener stopped on {:?}",
                            listener_role, local_addr
                        );
                        break;
                    }
                    result = socket.recv_from(&mut buf) => result,
                }
            } else {
                socket.recv_from(&mut buf).await
            };

            match recv_result {
                Ok((len, from)) => {
                    let data = buf[..len].to_vec();
                    // Update RX byte counter for speed monitoring
                    transport.rx_bytes_counter.fetch_add(len as u64, Ordering::Relaxed);

                    // НОВЫЙ ФОРМАТ: [peer_id:32][nonce:12][encrypted_data][tag:16]
                    // peer_id PLAINTEXT - позволяет найти peer БЕЗ дешифровки
                    let mut enc = encryption.lock().await;

                    // Извлекаем peer_id и дешифруем в одной операции
                    match enc.decrypt_by_peer_id(&data) {
                        // Сначала проверяем, что пакет предназначен для нас
                        // peer_id из заголовка - это отправитель, не получатель
                        // У нас нет receiver в текущем формате, поэтому пропускаем
                        Ok((peer_id, decrypted)) => {

                            drop(enc); // Release lock before processing

                            // 🚇 Integration Step 1: пробую integration-dispatch (0xB0..0xB3 circuit,
                            // 0xC0 resume и т.п.). Если вернул true — пакет обработан, остальное
                            // не нужно. Если false — идём в legacy match как раньше.
                            let dispatched = transport
                                .dispatch_decrypted_wagon(
                                    peer_id,
                                    &decrypted,
                                    WagonSource::Udp { peer_addr: from.to_string() },
                                )
                                .await;
                            if dispatched {
                                continue;
                            }

                            // Get peer info (for logging, etc.)
                            let peer = {
                                let peers_lock = peers.lock().await;
                                peers_lock.get(&peer_id).cloned()
                            };
                                // Parse message type
                                if decrypted.len() >= 1 {
                                    let msg_type = decrypted[0];

                                    match msg_type {
                                        // HEARTBEAT_REQ (0x01)
                                        0x01 if decrypted.len() >= 17 => {
                                            // Extract SEQ and timestamp
                                            let seq = u64::from_be_bytes(decrypted[1..9].try_into().unwrap_or([0u8; 8]));
                                            let _timestamp = u64::from_be_bytes(decrypted[9..17].try_into().unwrap_or([0u8; 8]));

                                            // Send HEARTBEAT_ACK: [MSG_TYPE:1][SEQ:8]
                                            let mut heartbeat_ack = vec![0u8; 9];
                                            heartbeat_ack[0] = 0x02; // HEARTBEAT_ACK
                                            heartbeat_ack[1..9].copy_from_slice(&seq.to_be_bytes());

                                            let peer_for_send = {
                                                let peers_lock = peers.lock().await;
                                                peers_lock.get(&peer_id).cloned()
                                            };

                                            if let Some(p) = peer_for_send {
                                                let encrypted_ack = {
                                                    let enc_lock = encryption.lock().await;
                                                    enc_lock.encrypt(&p, &heartbeat_ack)
                                                };

                                                if let Ok(ack_data) = encrypted_ack {
                                                    let addr = p.data_addr.as_ref().unwrap_or(&p.addr);
                                                    if let Err(e) = socket.send_to(&ack_data, addr).await {
                                                        // Silent fail
                                                    }
                                                }
                                            }

                                            // Update activity
                                            {
                                                let mut peers_lock = peers.lock().await;
                                                if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                    p.touch();
                                                }
                                            }
                                            {
                                                let mut tunnels_lock = tunnels.lock().await;
                                                tunnels_lock.update_activity(&peer_id);
                                            }
                                        }
                                        // HEARTBEAT_ACK (0x02)
                                        0x02 if decrypted.len() >= 9 => {
                                            // Silent update - don't respond!
                                            {
                                                let mut peers_lock = peers.lock().await;
                                                if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                    p.touch();
                                                    // 🌐 Direct путь жив — сбросим miss streak и
                                                    // вернёмся на direct, если были на relay.
                                                    // Only an ACK that really came from the peer's own address proves the
                                                    // direct path; one forwarded by a relay arrives from the relay's address.
                                                    let own_ip = |a: &str| a.parse::<SocketAddr>().map(|x| x.ip()).ok()
                                                        .or_else(|| a.split(':').next().and_then(|h| h.parse().ok()));
                                                    let from_peer = own_ip(&p.addr) == Some(from.ip())
                                                        || p.data_addr.as_deref().and_then(|a| own_ip(a)) == Some(from.ip());
                                                    if from_peer {
                                                        if p.use_relay {
                                                            println!("[transport] 🛰→📡 peer {} direct restored, leaving relay",
                                                                     hex::encode(&p.id.0[..8]));
                                                        }
                                                        p.direct_miss_streak = 0;
                                                        p.use_relay = false;
                                                    }
                                                }
                                            }
                                            {
                                                let mut tunnels_lock = tunnels.lock().await;
                                                tunnels_lock.update_activity(&peer_id);
                                            }
                                        }
                                        // Stream layer messages (0x10-0x16)
                                        0x10..=0x16 => {
                                            eprintln!("[transport] 🔔🔔🔔 STREAM FRAME DETECTED! type: {:#04x}, len: {}",
                                                     decrypted[0], decrypted.len());

                                            // Parse StreamFrame
                                            if let Some(stream_frame) = StreamFrame::from_bytes(&decrypted) {
                                                let stream_id = stream_frame.header.stream_id;
                                                eprintln!("[transport] 🔔 Stream frame parsed: stream_id={}, msg_type={:?}",
                                                         stream_id, stream_frame.header.msg_type);

                                                // Handle in stream registry
                                                let mut streams_lock = streams.lock().await;

                                                // If stream doesn't exist and this is a SYN, create it (incoming connection)
                                                if !streams_lock.get_stream(stream_id).is_some() {
                                                    if stream_frame.header.msg_type == crate::dataplane::stream::StreamMsgType::Syn {
                                                        println!("[transport] 🌊🌊🌊 INCOMING SYN for stream {} from peer {}",
                                                                 stream_id, hex::encode(&peer_id.0[..8]));

                                                        // Create new stream with SAME ID for incoming connection
                                                        let result = streams_lock.create_stream_with_id(peer_id, stream_id);
                                                        println!("[transport] ✅ Stream created: {:?}", result);
                                                    } else {
                                                        println!("[transport] ⚠️  Received frame for unknown stream {} (type: {:?})",
                                                                 stream_id, stream_frame.header.msg_type);
                                                        continue;
                                                    }
                                                }

                                                if let Some(stream) = streams_lock.get_stream_mut(stream_id) {
                                                    // Handle frame and get optional response
                                                    if let Some(response_frame) = stream.handle_frame(&stream_frame) {
                                                        eprintln!("[transport] 📤 Sending response frame: stream_id={}, type={:?}",
                                                                 stream_id, response_frame.header.msg_type);
                                                        // Queue response for sending
                                                        drop(streams_lock);

                                                        // Send response immediately
                                                        let peer_for_send = {
                                                            let peers_lock = peers.lock().await;
                                                            peers_lock.get(&peer_id).cloned()
                                                        };

                                                        if let Some(p) = peer_for_send {
                                                            let frame_bytes = response_frame.to_bytes();
                                                            eprintln!("[transport] 📦 Response frame: {} bytes", frame_bytes.len());
                                                            let enc_lock = encryption.lock().await;
                                                            if let Ok(resp_data) = enc_lock.encrypt(&p, &frame_bytes) {
                                                                let addr = p.data_addr.as_ref().unwrap_or(&p.addr);
                                                                eprintln!("[transport] 🔒 Encrypted response: {} bytes, sending to {}",
                                                                         resp_data.len(), addr);
                                                                match socket.send_to(&resp_data, addr).await {
                                                                    Ok(n) => {
                                                                        eprintln!("[transport] ✅ Sent {} bytes of response", n);
                                                                    }
                                                                    Err(e) => {
                                                                        eprintln!("[transport] ❌ Failed to send response: {}", e);
                                                                    }
                                                                }
                                                            } else {
                                                                eprintln!("[transport] ❌ Failed to encrypt response");
                                                            }
                                                        } else {
                                                            eprintln!("[transport] ⚠️  Peer not found for response");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        // Control commands (0x20-0x2F)
                                        0x20 | 0x21 | 0x22 | 0x23 => {
                                            use crate::netlayer::packet::PacketType;

                                            let cmd_type = match decrypted[0] {
                                                0x20 => Some(PacketType::StartExitHandler),
                                                0x21 => Some(PacketType::ExitHandlerStarted),
                                                0x22 => Some(PacketType::ExitHandlerError),
                                                0x23 => Some(PacketType::StopExitHandler),
                                                _ => None,
                                            };

                                            if let Some(cmd) = cmd_type {
                                                println!("[transport] 🎛️  CONTROL COMMAND: {:?} from {}",
                                                         cmd, crate::util::mask_hash_id(&peer_id));

                                                match cmd {
                                                    PacketType::StartExitHandler => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  🎛️  REQUEST: Запрос на запуск Exit Node Handler         ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();

                                                        // Get peer info from peers map
                                                        let peer_for_send = {
                                                            let peers_lock = peers.lock().await;
                                                            peers_lock.get(&peer_id).cloned()
                                                        };

                                                        let peer_addr = peer_for_send.as_ref()
                                                            .and_then(|p| p.data_addr.as_ref().map(String::from))
                                                            .unwrap_or_else(|| peer_for_send.as_ref().map(|p| p.addr.clone()).unwrap_or_else(|| "unknown".to_string()));

                                                        // Отправляем запрос в Exit Handler Manager (если есть канал)
                                                        if let Some(ref tx) = exit_handler_tx {
                                                            let _ = tx.send(ExitHandlerRequest {
                                                                peer_id: peer_id.clone(),
                                                                peer_addr: peer_addr.clone(),
                                                            }).await;
                                                            println!("[transport] 📤 Запрос отправлен в Exit Handler Manager");
                                                        } else {
                                                            println!("[transport] ⚠️  Exit Handler Manager не настроен!");
                                                        }

                                                        // Отправляем подтверждение (если есть peer info)
                                                        if let Some(p) = peer_for_send {

                                                            let ack_pkt = NetPacket::new(
                                                                PacketType::ExitHandlerStarted,
                                                                peer_id,
                                                                true,
                                                                vec![]
                                                            );
                                                            let ack_bytes = ack_pkt.to_bytes();
                                                            let enc_lock = encryption.lock().await;
                                                            if let Ok(encrypted_ack) = enc_lock.encrypt(&p, &ack_bytes) {
                                                                let addr = p.data_addr.as_ref().unwrap_or(&p.addr);
                                                                let _ = socket.send_to(&encrypted_ack, addr).await;
                                                                println!("[transport] 📤 Отправлен ExitHandlerStarted ACK");
                                                            }
                                                        }
                                                    }
                                                    PacketType::ExitHandlerStarted => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  ✅ Exit Node Handler на ноде {} ГОТОВ!          ║",
                                                                 crate::util::mask_hash_id(&peer_id));
                                                        println!("║     Теперь можете использовать SOCKS5 Proxy!              ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();
                                                    }
                                                    PacketType::ExitHandlerError => {
                                                        println!("[transport] ❌ Ошибка запуска Exit Node Handler на ноде {}",
                                                                 crate::util::mask_hash_id(&peer_id));
                                                    }
                                                    PacketType::StopExitHandler => {
                                                        println!("[transport] 🛑 Запрос на остановку Exit Node Handler от {}",
                                                                 crate::util::mask_hash_id(&peer_id));
                                                    }
                                                    _ => {}
                                                }
                                                {
                                                    let mut peers_lock = peers.lock().await;
                                                    if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                        p.touch();
                                                    }
                                                }
                                                {
                                                    let mut tunnels_lock = tunnels.lock().await;
                                                    tunnels_lock.update_activity(&peer_id);
                                                }
                                            }
                                        }
                                        // HTTP Proxy control commands (0x30-0x3F)
                                        0x30 | 0x31 | 0x32 | 0x33 | 0x34 | 0x35 | 0x36 | 0x37 => {
                                            use crate::netlayer::packet::PacketType;

                                            let cmd_type = match decrypted[0] {
                                                0x30 => Some(PacketType::StartProxyGateway),
                                                0x31 => Some(PacketType::ProxyGatewayStarted),
                                                0x32 => Some(PacketType::ProxyGatewayError),
                                                0x33 => Some(PacketType::StopProxyGateway),
                                                0x34 => Some(PacketType::StartSocks5Gateway),
                                                0x35 => Some(PacketType::Socks5GatewayStarted),
                                                0x36 => Some(PacketType::Socks5GatewayError),
                                                0x37 => Some(PacketType::StopSocks5Gateway),
                                                _ => None,
                                            };

                                            if let Some(cmd) = cmd_type {
                                                println!("[transport] 🌐 PROXY CONTROL COMMAND: {:?} from {}",
                                                         cmd, crate::util::mask_hash_id(&peer_id));

                                                match cmd {
                                                    PacketType::StartProxyGateway => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  🌐 REQUEST: Запрос на запуск HTTP Proxy Gateway          ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();

                                                        // Get peer info from peers map
                                                        let peer_for_send = {
                                                            let peers_lock = peers.lock().await;
                                                            peers_lock.get(&peer_id).cloned()
                                                        };

                                                        let peer_addr = peer_for_send.as_ref()
                                                            .and_then(|p| p.data_addr.as_ref().map(String::from))
                                                            .unwrap_or_else(|| peer_for_send.as_ref().map(|p| p.addr.clone()).unwrap_or_else(|| "unknown".to_string()));
                                                        let short_id = hex::encode(&peer_id.0[..8]);

                                                        if let Some(ref tx) = proxy_gateway_tx {
                                                            let _ = tx.send(ProxyGatewayRequest {
                                                                peer_id: peer_id.clone(),
                                                                peer_addr: peer_addr.clone(),
                                                                short_id: short_id.clone(),
                                                            }).await;
                                                            println!("[transport] 📤 Запрос отправлен в Proxy Gateway Manager для ноды {}", short_id);
                                                        } else {
                                                            println!("[transport] ⚠️  Proxy Gateway Manager не настроен!");
                                                            println!("[transport] 📝 Пожалуйста, запустите вручную: proxy-gateway");
                                                        }

                                                        // Отправляем подтверждение (если есть peer info)
                                                        if let Some(p) = peer_for_send {
                                                            let ack_pkt = NetPacket::new(
                                                                PacketType::ProxyGatewayStarted,
                                                                peer_id,
                                                                true,
                                                                vec![]
                                                            );
                                                            let ack_bytes = ack_pkt.to_bytes();
                                                            let enc_lock = encryption.lock().await;
                                                            if let Ok(encrypted_ack) = enc_lock.encrypt(&p, &ack_bytes) {
                                                                let addr_to_use = p.data_addr.as_ref().unwrap_or(&p.addr);
                                                                let _ = socket.send_to(&encrypted_ack, addr_to_use).await;
                                                                println!("[transport] 📤 Отправлен ProxyGatewayStarted ACK на {}", addr_to_use);
                                                            }
                                                        }
                                                    }
                                                    PacketType::ProxyGatewayStarted => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  ✅ ACK: HTTP Proxy Gateway запущен на удалённой ноде     ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();
                                                    }
                                                    PacketType::ProxyGatewayError => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  ❌ ERROR: Ошибка запуска HTTP Proxy Gateway                ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();
                                                    }
                                                    PacketType::StopProxyGateway => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  🛑 REQUEST: Запрос на остановку HTTP Proxy Gateway         ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();
                                                    }
                                                    // 🧦 SOCKS5 Proxy Gateway commands
                                                    PacketType::StartSocks5Gateway => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  🧦 REQUEST: Запрос на запуск SOCKS5 Proxy Gateway         ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();

                                                        let peer_for_send = {
                                                            let peers_lock = peers.lock().await;
                                                            peers_lock.get(&peer_id).cloned()
                                                        };

                                                        let peer_addr = peer_for_send.as_ref()
                                                            .and_then(|p| p.data_addr.as_ref().map(String::from))
                                                            .unwrap_or_else(|| peer_for_send.as_ref().map(|p| p.addr.clone()).unwrap_or_else(|| "unknown".to_string()));
                                                        let short_id = hex::encode(&peer_id.0[..8]);

                                                        if let Some(ref tx) = socks5_gateway_tx {
                                                            let _ = tx.send(ProxyGatewayRequest {
                                                                peer_id: peer_id.clone(),
                                                                peer_addr: peer_addr.clone(),
                                                                short_id: short_id.clone(),
                                                            }).await;
                                                            println!("[transport] 📤 Запрос отправлен в SOCKS5 Gateway Manager для ноды {}", short_id);
                                                        } else {
                                                            println!("[transport] ⚠️  SOCKS5 Gateway Manager не настроен!");
                                                            println!("[transport] 📝 Пожалуйста, запустите вручную: socks5-gateway");
                                                        }

                                                        if let Some(p) = peer_for_send {
                                                            let ack_pkt = NetPacket::new(
                                                                PacketType::Socks5GatewayStarted,
                                                                peer_id,
                                                                true,
                                                                vec![]
                                                            );
                                                            let ack_bytes = ack_pkt.to_bytes();
                                                            let enc_lock = encryption.lock().await;
                                                            if let Ok(encrypted_ack) = enc_lock.encrypt(&p, &ack_bytes) {
                                                                let addr_to_use = p.data_addr.as_ref().unwrap_or(&p.addr);
                                                                let _ = socket.send_to(&encrypted_ack, addr_to_use).await;
                                                                println!("[transport] 📤 Отправлен Socks5GatewayStarted ACK на {}", addr_to_use);
                                                            }
                                                        }
                                                    }
                                                    PacketType::Socks5GatewayStarted => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  ✅ ACK: SOCKS5 Proxy Gateway запущен на удалённой ноде    ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();
                                                    }
                                                    PacketType::Socks5GatewayError => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  ❌ ERROR: Ошибка запуска SOCKS5 Proxy Gateway               ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();
                                                    }
                                                    PacketType::StopSocks5Gateway => {
                                                        println!();
                                                        println!("╔════════════════════════════════════════════════════════════╗");
                                                        println!("║  🛑 REQUEST: Запрос на остановку SOCKS5 Proxy Gateway       ║");
                                                        println!("╚════════════════════════════════════════════════════════════╝");
                                                        println!();
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        // HTTP Proxy packets (0x40-0x4F)
                                        0x40 | 0x41 | 0x42 => {
                                            use crate::netlayer::packet::PacketType;

                                            let proxy_pkt_type = match decrypted[0] {
                                                0x40 => Some(PacketType::ProxyRequest),
                                                0x41 => Some(PacketType::ProxyResponse),
                                                0x42 => Some(PacketType::ProxyResponseFragment),
                                                _ => None,
                                            };

                                            if let Some(pkt_type) = proxy_pkt_type {
                                                println!("[transport] 🌐 PROXY PACKET: {:?} from {}",
                                                         pkt_type, crate::util::mask_hash_id(&peer_id));

                                                match pkt_type {
                                                    PacketType::ProxyRequest => {
                                                        // Это ProxyRequest - deserialize и передаём в gateway
                                                        // Пропускаем первый байт (0x40)
                                                        // ⚡ Используем bincode вместо JSON (в 3-5 раз быстрее)
                                                        if let Ok(proxy_req) = crate::proxy::ProxyRequest::from_bincode(&decrypted[1..]) {
                                                            println!("[transport] 📨 ProxyRequest #{}: {} {}",
                                                                     proxy_req.request_id, proxy_req.method, proxy_req.url);

                                                            // Отправляем в HttpProxyGateway через канал
                                                            if let Some(ref tx) = proxy_request_tx {
                                                                let peer_id_for_tx = peer_id.clone();
                                                                let _ = tx.send((peer_id, proxy_req)).await;
                                                                println!("[transport] 📤 ProxyRequest отправлен в HttpProxyGateway");
                                                            } else {
                                                                println!("[transport] ⚠️  HttpProxyGateway не подключён! Запустите: proxy-gateway");
                                                            }
                                                        } else {
                                                            eprintln!("[transport] ❌ Failed to parse ProxyRequest");
                                                        }
                                                    }
                                                    PacketType::ProxyResponse => {
                                                        // Это ProxyResponse - deserialize и передаём клиенту
                                                        // Пропускаем первый байт (0x41)
                                                        // ⚡ Используем bincode вместо JSON (в 3-5 раз быстрее)
                                                        if let Ok(proxy_resp) = crate::proxy::ProxyResponse::from_bincode(&decrypted[1..]) {
                                                            println!("[transport] 📨 ProxyResponse #{}: {} {} bytes",
                                                                     proxy_resp.request_id, proxy_resp.status, proxy_resp.body.len());

                                                            // Отправляем в HttpProxyClient через канал
                                                            if let Some(ref tx) = proxy_response_tx {
                                                                let peer_id_for_tx = peer_id.clone();
                                                                let _ = tx.send((peer_id, proxy_resp)).await;
                                                                println!("[transport] 📤 ProxyResponse отправлен в HttpProxyClient");
                                                            } else {
                                                                println!("[transport] ⚠️  HttpProxyClient не подключён!");
                                                            }
                                                        } else {
                                                            eprintln!("[transport] ❌ Failed to parse ProxyResponse");
                                                        }
                                                    }
                                                    PacketType::Socks5Request => {
                                                        // SOCKS5 CONNECT request
                                                        if let Ok(socks5_req) = serde_json::from_slice::<crate::socks5::Socks5ProxyRequest>(&decrypted[1..]) {
                                                            println!("[transport] 📨 Socks5Request #{}: {}:{}",
                                                                     socks5_req.request_id, socks5_req.target_host, socks5_req.target_port);

                                                            if let Some(ref tx) = socks5_request_tx {
                                                                let peer_id_for_tx = peer_id.clone();
                                                                let _ = tx.send((peer_id, socks5_req)).await;
                                                                println!("[transport] 📤 Socks5Request отправлен в ExitNodeHandler");
                                                            } else {
                                                                println!("[transport] ⚠️  Socks5ExitNodeHandler не подключён! Запустите: exit");
                                                            }
                                                        } else {
                                                            eprintln!("[transport] ❌ Failed to parse Socks5Request");
                                                        }
                                                    }
                                                    PacketType::Socks5Response => {
                                                        // SOCKS5 CONNECT response
                                                        if let Ok(socks5_resp) = serde_json::from_slice::<crate::socks5::Socks5ProxyResponse>(&decrypted[1..]) {
                                                            println!("[transport] 📨 Socks5Response #{}: status={}",
                                                                     socks5_resp.request_id, socks5_resp.status);

                                                            if let Some(ref tx) = socks5_response_tx {
                                                                let peer_id_for_tx = peer_id.clone();
                                                                let _ = tx.send((peer_id, socks5_resp)).await;
                                                                println!("[transport] 📤 Socks5Response отправлен в Socks5ProxyServer");
                                                            } else {
                                                                println!("[transport] ⚠️  Socks5ProxyServer не подключён!");
                                                            }
                                                        } else {
                                                            eprintln!("[transport] ❌ Failed to parse Socks5Response");
                                                        }
                                                    }
                                                    PacketType::Socks5TunnelData => {
                                                        // SOCKS5 tunnel data
                                                        if let Ok(tunnel_data) = serde_json::from_slice::<crate::socks5::Socks5TunnelData>(&decrypted[1..]) {
                                                            println!("[transport] 📨 Socks5TunnelData #{}: {} bytes, close={}",
                                                                     tunnel_data.tunnel_id, tunnel_data.data.len(), tunnel_data.close);

                                                            if let Some(ref tx) = socks5_tunnel_data_tx {
                                                                let peer_id_for_tx = peer_id.clone();
                                                                let _ = tx.send((peer_id, tunnel_data)).await;
                                                            } else {
                                                                println!("[transport] ⚠️  Socks5TunnelData канал не подключён!");
                                                            }
                                                        } else {
                                                            eprintln!("[transport] ❌ Failed to parse Socks5TunnelData");
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                                {
                                                    let mut peers_lock = peers.lock().await;
                                                    if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                        p.touch();
                                                    }
                                                }
                                                {
                                                    let mut tunnels_lock = tunnels.lock().await;
                                                    tunnels_lock.update_activity(&peer_id);
                                                }
                                            }
                                        }
                                        // P2P Tunnel packets (0x80-0x8F)
                                        0x80 | 0x81 | 0x82 | 0x83 | 0x84 | 0x85 | 0x86 => {
                                            println!("[transport] 🔗 P2P Tunnel packet from {}: type={:#04x}, len={}",
                                                     crate::util::mask_hash_id(&peer_id), decrypted[0], decrypted.len());

                                            // Forward to P2P Tunnel Manager
                                            let peer_id_for_tx = peer_id.clone();
                                            if let Some(ref tx) = p2p_tunnel_tx {
                                                let _ = tx.send((peer_id, decrypted)).await;
                                                println!("[transport] 📤 P2P tunnel packet forwarded to P2PTunnelManager");
                                            } else {
                                                println!("[transport] ⚠️  P2PTunnelManager not connected!");
                                            }

                                            {
                                                let mut peers_lock = peers.lock().await;
                                                if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                    p.touch();
                                                }
                                            }
                                            {
                                                let mut tunnels_lock = tunnels.lock().await;
                                                tunnels_lock.update_activity(&peer_id);
                                            }
                                        }
                                        // YTP packets (0x60-0x6F, 0x70 BatchedWagon)
                                        0x60 | 0x61 | 0x70 => {
                                            use crate::protocol::{Wagon, TrainAckMessage, Station, BatchedWagon};

                                            match decrypted[0] {
                                                0x60 => {
                                                    // YTP Wagon packet
                                                    // ⚡ ПАРСИМ ОДИН РАЗ вместо трёх!
                                                    let wagon_bytes = decrypted[1..].to_vec();

                                                    // 🎯 ПАРСИНГ №1 (единственный!) - теперь с move вместо borrow
                                                    let wagon: Option<Wagon> = Wagon::from_bytes(&wagon_bytes).ok();

                                                    if let Some(wagon) = wagon {
                                                        // Проверяем checksum
                                                        if !wagon.verify() {
                                                            println!("[transport] ⚠️  Wagon has invalid checksum! Dropping.");

                                                            // 🚂 Статистика checksum failures
                                                            let stats = get_wagon_stats();
                                                            stats.checksum_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                                            continue;
                                                        }

                                                        println!("[transport] 🚂 YTP Wagon from {} (train #{}, wagon {}/{}, {} KB)",
                                                                 crate::util::mask_hash_id(&peer_id),
                                                                 wagon.train_id,
                                                                 wagon.wagon_num + 1,
                                                                 wagon.total_wagons,
                                                                 wagon.cargo.len() / 1024
                                                        );

                                                        // Передаём в station для сборки (УЖЕ ДЕСЕРИАЛИЗОВАННЫЙ!)
                                                        if let Some(ref station_lock) = station {
                                                            let station_guard = station_lock.lock().await;
                                                            if let Some(ref st) = *station_guard {
                                                                match st.receive_wagon_parsed(peer_id, wagon).await {
                                                                Ok(Some(train_id)) => {
                                                                    println!("[transport] ✅ Train #{} assembled!", train_id);

                                                                    // 🔍 DEBUG: Пытаемся получить train
                                                                    eprintln!("[transport] 🔍 Calling get_train for #{}", train_id);

                                                                    // Получаем собранные данные
                                                                    if let Some(data) = st.get_train(train_id).await {
                                                                        println!("[transport] 📦 Train #{} delivered: {} MB",
                                                                                 train_id, data.len() / 1_000_000);
                                                                        eprintln!("[transport] ✅ Train #{} delivered: {} bytes", train_id, data.len());

                                                                        // ⚠️ ACK/NACK DISABLED per user directive
                                                                        // // 🔄 ПРОВЕРЯЕМ NACK (0x62 prefix)
                                                                        // if !data.is_empty() && data[0] == 0x62u8 {
                                                                        //     use crate::protocol::WagonNack;
                                                                        //
                                                                        //     if let Ok(nack) = serde_json::from_slice::<WagonNack>(&data[1..]) {
                                                                        //         println!("[transport] 🔄 Received NACK for train #{}: {} missing wagons",
                                                                        //                  nack.train_id, nack.missing_wagons.len());
                                                                        //
                                                                        //         // Отправляем NACK в gateway для обработки
                                                                        //         if let Some(ref tx) = nack_tx {
                                                                        //             let peer_id_for_tx = peer_id.clone();
                                                                        //             let _ = tx.send((peer_id, nack)).await;
                                                                        //             println!("[transport] 📤 NACK sent to gateway for retransmission");
                                                                        //         } else {
                                                                        //             println!("[transport] ⚠️  NACK channel not connected! Wagon retransmission disabled!");
                                                                        //         }
                                                                        //         continue;
                                                                        //     }
                                                                        // }

                                                                    // СНАЧАЛА проверяем HTTP Proxy (приоритет для CONNECT tunneling)
                                                                    if let Ok(tunnel_data) = crate::proxy::ProxyTunnelData::from_bincode(&data) {
                                                                            let tunnel_id = tunnel_data.tunnel_id;
                                                                            let data_len = tunnel_data.data.len();

                                                                            println!("[transport] 🚇 ProxyTunnelData #{}: {} bytes, close={}",
                                                                                     tunnel_id, data_len, tunnel_data.close);

                                                                            if let Some(ref tx) = proxy_tunnel_data_tx {
                                                                                let peer_id_for_tx = peer_id.clone();
                                                                                let _ = tx.send((peer_id, tunnel_data)).await;
                                                                                println!("[transport] 📤 ProxyTunnelData #{} sent to handler", tunnel_id);
                                                                            } else {
                                                                                println!("[transport] ⚠️  ProxyTunnelData channel not connected!");
                                                                            }
                                                                        }
                                                                        // Потом SOCKS5
                                                                        else if let Ok(socks5_tunnel) = serde_json::from_slice::<crate::socks5::Socks5TunnelData>(&data) {
                                                                            let tunnel_id = socks5_tunnel.tunnel_id;
                                                                            let data_len = socks5_tunnel.data.len();

                                                                            println!("[transport] 🚇 Socks5TunnelData #{}: {} bytes, close={}",
                                                                                     tunnel_id, data_len, socks5_tunnel.close);

                                                                            if let Some(ref tx) = socks5_tunnel_data_tx {
                                                                                let peer_id_for_tx = peer_id.clone();
                                                                                let _ = tx.send((peer_id, socks5_tunnel)).await;
                                                                                println!("[transport] 📤 SOCKS5 Tunnel data #{} sent to handler", tunnel_id);
                                                                            } else {
                                                                                println!("[transport] ⚠️  SOCKS5 Tunnel data channel not connected!");
                                                                            }
                                                                        }
                                                                        // Потом HTTP Proxy
                                                                        else if let Ok(proxy_req) = crate::proxy::ProxyRequest::from_bincode(&data) {
                                                                            let req_id = proxy_req.request_id;
                                                                            let url = proxy_req.url.clone();
                                                                            let method = proxy_req.method.clone();

                                                                            println!("[transport] 📨 ProxyRequest #{}: {} {}", req_id, method, url);

                                                                            // Отправляем в HttpProxyGateway
                                                                            if let Some(ref tx) = proxy_request_tx {
                                                                                let peer_id_for_tx = peer_id.clone();
                                                                                let _ = tx.send((peer_id, proxy_req)).await;
                                                                                println!("[transport] 📤 ProxyRequest #{} sent to gateway", req_id);
                                                                            }
                                                                        }
                                                                        // Пробуем десериализовать как ProxyResponse (для client)
                                                                        // ✅ Проверяем префикс 0x41
                                                                        else if data.len() > 1 && data[0] == 0x41u8 {
                                                                            // Убираем префикс 0x41
                                                                            let json_data = &data[1..];

                                                                            if let Ok(proxy_resp) = crate::proxy::ProxyResponse::from_bincode(json_data) {
                                                                                let req_id = proxy_resp.request_id;

                                                                                println!("[transport] 📨 ProxyResponse #{}: status={}, {} bytes",
                                                                                         req_id, proxy_resp.status, json_data.len());

                                                                                // Отправляем в HttpProxyClient
                                                                                if let Some(ref tx) = proxy_response_tx {
                                                                                    let peer_id_for_tx = peer_id.clone();
                                                                                    let _ = tx.send((peer_id, proxy_resp)).await;
                                                                                    println!("[transport] 📤 ProxyResponse #{} sent to client", req_id);
                                                                                }
                                                                            } else {
                                                                                eprintln!("[transport] ⚠️  ProxyResponse with 0x41 prefix but invalid JSON");
                                                                            }
                                                                        }
                                                                        // SOCKS5 ProxyRequest
                                                                        else if let Ok(socks5_req) = serde_json::from_slice::<crate::socks5::Socks5ProxyRequest>(&data) {
                                                                            let req_id = socks5_req.request_id;
                                                                            let target = format!("{}:{}", socks5_req.target_host, socks5_req.target_port);

                                                                            println!("[transport] 📨 Socks5Request #{}: {}", req_id, target);

                                                                            if let Some(ref tx) = socks5_request_tx {
                                                                                let peer_id_for_tx = peer_id.clone();
                                                                                let _ = tx.send((peer_id, socks5_req)).await;
                                                                                println!("[transport] 📤 Socks5Request #{} sent to ExitNodeHandler", req_id);
                                                                            }
                                                                        }
                                                                        // SOCKS5 ProxyResponse
                                                                        else if let Ok(socks5_resp) = serde_json::from_slice::<crate::socks5::Socks5ProxyResponse>(&data) {
                                                                            let req_id = socks5_resp.request_id;

                                                                            println!("[transport] 📨 Socks5Response #{}: status={}", req_id, socks5_resp.status);

                                                                            if let Some(ref tx) = socks5_response_tx {
                                                                                let peer_id_for_tx = peer_id.clone();
                                                                                let _ = tx.send((peer_id, socks5_resp)).await;
                                                                                println!("[transport] 📤 Socks5Response #{} sent to Socks5ProxyServer", req_id);
                                                                            }
                                                                        }
                                                                        // TUN Wagon (new!)
                                                                        else if let Ok(tun_wagon) = serde_json::from_slice::<crate::netlayer::tun_exit::TunWagon>(&data) {
                                                                            let conn_id = tun_wagon.connection_id.clone();

                                                                            println!("[transport] 📨 TunWagon: conn_id={}, packet_size={}, close={}",
                                                                                     conn_id, tun_wagon.packet.len(), tun_wagon.close);

                                                                            if let Some(ref tx) = tun_wagon_tx {
                                                                                let peer_id_for_tx = peer_id.clone();
                                                                                let _ = tx.send((peer_id, tun_wagon)).await;
                                                                                println!("[transport] 📤 TunWagon sent to TunExitHandler");
                                                                            } else {
                                                                                println!("[transport] ⚠️  TunExitHandler не подключён! Запустите: tun exit");
                                                                            }
                                                                        }
                                                                        // TUN Wagon Response (new!)
                                                                        else if let Ok(tun_resp) = serde_json::from_slice::<crate::netlayer::tun_exit::TunWagonResponse>(&data) {
                                                                            let conn_id = tun_resp.connection_id.clone();

                                                                            println!("[transport] 📨 TunWagonResponse: conn_id={}, data_size={}, close={}",
                                                                                     conn_id, tun_resp.data.len(), tun_resp.close);

                                                                            if let Some(ref tx) = tun_wagon_resp_tx {
                                                                                let peer_id_for_tx = peer_id.clone();
                                                                                let _ = tx.send((peer_id, tun_resp)).await;
                                                                                println!("[transport] 📤 TunWagonResponse sent to TUN Entry Node");
                                                                            } else {
                                                                                println!("[transport] ⚠️  TUN Entry Node не подключён! TunWagonResponse игнорируется");
                                                                            }
                                                                        }
                                                                        else {
                                                                            eprintln!("[transport] ⚠️  Unknown data type in train #{}", train_id);
                                                                        }
                                                                    } else {
                                                                        eprintln!("[transport] ❌ get_train returned None for train #{}!", train_id);
                                                                        eprintln!("[transport] ⚠️  Train was assembled but could NOT be extracted!");
                                                                        continue;
                                                                    }
                                                                }
                                                                Ok(None) => {
                                                                    // Поезд ещё собирается
                                                                }
                                                                Err(e) => {
                                                                    eprintln!("[transport] ❌ Station error: {}", e);
                                                                }
                                                            }
                                                            } // конец if let Some(ref st)
                                                        } else {
                                                            eprintln!("[transport] ⚠️  No station configured!");
                                                        }
                                                    } else {
                                                        eprintln!("[transport] ❌ Failed to parse YTP Wagon");
                                                    }
                                                }
                                                0x61 => {
                                                    // ⚠️ ACK/NACK DISABLED per user directive
                                                    // YTP ACK/NACK packet
                                                    // if let Ok(ack) = serde_json::from_slice::<TrainAckMessage>(&decrypted[1..]) {
                                                    //     println!("[transport] 📨 YTP ACK from {}: train #{:?}",
                                                    //              crate::util::mask_hash_id(&peer_id), ack.train_id());
                                                    //
                                                    //     match &ack {
                                                    //         TrainAckMessage::Complete { .. } => {
                                                    //             println!("[transport] ✅ Train #{} complete! ACK received from {}",
                                                    //                      ack.train_id(), crate::util::mask_hash_id(&peer_id));
                                                    //             // TODO: Отправить ACK в gateway для очистки sent_trains
                                                    //         }
                                                    //         TrainAckMessage::Missing { missing_wagons, total_wagons, .. } => {
                                                    //             println!("[transport] ⚠️  Train #{} missing wagons: {}/{}",
                                                    //                      ack.train_id(), missing_wagons.len(), total_wagons);
                                                    //             // TODO: Trigger express train for missing wagons
                                                    //         }
                                                    //         TrainAckMessage::Progress { progress_percent, .. } => {
                                                    //             println!("[transport] 📊 Train #{} progress: {:.1}%",
                                                    //                      ack.train_id(), progress_percent);
                                                    //         }
                                                    //     }
                                                    // } else {
                                                    //     eprintln!("[transport] ❌ Failed to parse YTP ACK");
                                                    // }
                                                    println!("[transport] ⚠️ ACK/NACK disabled - ignoring 0x61 packet");
                                                }
                                                0x70 => {
                                                    // ⚡ BatchedWagon - несколько пакетов в одном wagon
                                                    let batched_bytes = decrypted[1..].to_vec();

                                                    if let Ok(batched_wagon) = bincode::deserialize::<BatchedWagon>(&batched_bytes) {
                                                        let packet_count = batched_wagon.packets.len();
                                                        let wagon_id = batched_wagon.wagon_id;

                                                        println!("[transport] 📦 BatchedWagon #{} from {}: {} packets",
                                                                 wagon_id,
                                                                 crate::util::mask_hash_id(&peer_id),
                                                                 packet_count);

                                                        // ⚡ Сортируем пакеты по sequence number
                                                        let mut sorted_packets = batched_wagon.packets.clone();
                                                        sorted_packets.sort_by_key(|p| p.seq_num);

                                                        // ⚡ Обрабатываем каждый пакет по порядку
                                                        for packet in sorted_packets {
                                                            // Пытаемся распознать тип данных
                                                            let data = &packet.data;

                                                            // СНАЧАЛА проверяем HTTP Proxy (приоритет для CONNECT tunneling)
                                                            if let Ok(tunnel_data) = serde_json::from_slice::<crate::proxy::ProxyTunnelData>(data) {
                                                                let tunnel_id = tunnel_data.tunnel_id;
                                                                let data_len = tunnel_data.data.len();

                                                                println!("[transport] 🚇 [batched] ProxyTunnelData #{}: {} bytes, close={}",
                                                                         tunnel_id, data_len, tunnel_data.close);

                                                                if let Some(ref tx) = proxy_tunnel_data_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, tunnel_data)).await;
                                                                }
                                                            }
                                                            // Потом SOCKS5
                                                            else if let Ok(socks5_tunnel) = serde_json::from_slice::<crate::socks5::Socks5TunnelData>(data) {
                                                                let tunnel_id = socks5_tunnel.tunnel_id;
                                                                let data_len = socks5_tunnel.data.len();

                                                                println!("[transport] 🚇 [batched] Socks5TunnelData #{}: {} bytes, close={}",
                                                                         tunnel_id, data_len, socks5_tunnel.close);

                                                                if let Some(ref tx) = socks5_tunnel_data_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, socks5_tunnel)).await;
                                                                }
                                                            }
                                                            // SOCKS5 ProxyRequest
                                                            else if let Ok(socks5_req) = serde_json::from_slice::<crate::socks5::Socks5ProxyRequest>(data) {
                                                                let req_id = socks5_req.request_id;
                                                                let target = format!("{}:{}", socks5_req.target_host, socks5_req.target_port);

                                                                println!("[transport] 📨 [batched] Socks5Request #{}: {}", req_id, target);

                                                                if let Some(ref tx) = socks5_request_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, socks5_req)).await;
                                                                }
                                                            }
                                                            // SOCKS5 ProxyResponse
                                                            else if let Ok(socks5_resp) = serde_json::from_slice::<crate::socks5::Socks5ProxyResponse>(data) {
                                                                let req_id = socks5_resp.request_id;

                                                                println!("[transport] 📨 [batched] Socks5Response #{}: status={}", req_id, socks5_resp.status);

                                                                if let Some(ref tx) = socks5_response_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, socks5_resp)).await;
                                                                }
                                                            }
                                                            // HTTP Proxy Request
                                                            else if let Ok(proxy_req) = crate::proxy::ProxyRequest::from_bincode(data) {
                                                                let req_id = proxy_req.request_id;
                                                                let url = proxy_req.url.clone();

                                                                println!("[transport] 📨 [batched] ProxyRequest #{}: {}", req_id, url);

                                                                if let Some(ref tx) = proxy_request_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, proxy_req)).await;
                                                                }
                                                            }
                                                            // HTTP Proxy Response
                                                            else if let Ok(proxy_resp) = crate::proxy::ProxyResponse::from_bincode(data) {
                                                                let req_id = proxy_resp.request_id;

                                                                if let Some(ref tx) = proxy_response_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, proxy_resp)).await;
                                                                }
                                                            }
                                                            // HTTP Proxy Tunnel Data
                                                            else if let Ok(tunnel_data) = serde_json::from_slice::<crate::proxy::ProxyTunnelData>(data) {
                                                                if let Some(ref tx) = proxy_tunnel_data_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, tunnel_data)).await;
                                                                }
                                                            }
                                                            // TUN Wagon
                                                            else if let Ok(tun_wagon) = serde_json::from_slice::<crate::netlayer::tun_exit::TunWagon>(data) {
                                                                if let Some(ref tx) = tun_wagon_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, tun_wagon)).await;
                                                                }
                                                            }
                                                            // TUN Wagon Response
                                                            else if let Ok(tun_resp) = serde_json::from_slice::<crate::netlayer::tun_exit::TunWagonResponse>(data) {
                                                                if let Some(ref tx) = tun_wagon_resp_tx {
                                                                    let peer_id_for_tx = peer_id.clone();
                                                                    let _ = tx.send((peer_id, tun_resp)).await;
                                                                }
                                                            }
                                                            else {
                                                                eprintln!("[transport] ⚠️  [batched] Unknown data type in packet seq #{}", packet.seq_num);
                                                            }
                                                        }

                                                        println!("[transport] ✅ BatchedWagon #{} processed: {} packets delivered", wagon_id, packet_count);
                                                    } else {
                                                        eprintln!("[transport] ❌ Failed to parse BatchedWagon");
                                                    }
                                                }
                                                _ => {}
                                            }

                                            {
                                                let mut peers_lock = peers.lock().await;
                                                if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                    p.touch();
                                                }
                                            }
                                            {
                                                let mut tunnels_lock = tunnels.lock().await;
                                                tunnels_lock.update_activity(&peer_id);
                                            }
                                        }
                                        // PROBE message (0x50 = 'P') - текстовое сообщение для активации обратного канала
                                        // Peer Exchange packet (0xE0)

                                        0xE0 => {

                                            // Десериализуем список пиров

                                            if let Ok(peer_list) = bincode::deserialize::<Vec<(HashId, String)>>(&decrypted[1..]) {

                                                println!("[transport] 🔄 Received peer list ({} peers) from {}", 

                                                         peer_list.len(), crate::util::mask_hash_id(&peer_id));

                                                
                                                // Добавляем новых пиров

                                                for (new_peer_id, new_addr) in peer_list {

                                                    // Skip our own node ID
                                                    if new_peer_id == local_id {
                                                        continue;
                                                    }
                                                    if new_peer_id == peer_id {

                                                        continue;

                                                    }

                                                    

                                                    let mut peers_lock = peers.lock().await;

                                                    // Node Identity Binding Fix: unauthenticated
                                                    // peer-exchange hearsay carries no signing key at
                                                    // all — never let it introduce a pinned node_id.
                                                    // Only that node's OWN verified Hello may do that.
                                                    let is_pinned = transport.pinned_identities.read().await.contains_key(&new_peer_id);

                                                    if !peers_lock.contains_key(&new_peer_id) && !is_pinned {

                                                        println!("[transport] ➕ Adding new peer from list: {} @ {}",

                                                                 hex::encode(&new_peer_id.0[..8]), new_addr);

                                                        let new_peer = PeerInfo::new(new_peer_id, &new_addr);

                                                        peers_lock.insert(new_peer_id, new_peer);

                                                    }

                                                }

                                            } else {

                                                println!("[transport] ⚠️  Failed to deserialize peer list from {}", 

                                                         crate::util::mask_hash_id(&peer_id));

                                            }
                                        }

                                        // DHT RPC messages (0xF0-0xF7)

                                        0xF0 => {

                                            println!("[transport] 📡 DHT Ping from {}", crate::util::mask_hash_id(&peer_id));

                                            let pong_packet = vec![0xF1u8];

                                            if let Some(peer_info) = peer {

                                                let enc = encryption.lock().await;

                                                if let Ok(encrypted) = enc.encrypt(&peer_info, &pong_packet) {

                                                    let addr = peer_info.data_addr.as_ref().unwrap_or(&peer_info.addr);

                                                    let _ = socket.send_to(&encrypted, addr).await;

                                                }

                                            }

                                        }

                                        0xF1 => {

                                            println!("[transport] 📡 DHT Pong from {}", crate::util::mask_hash_id(&peer_id));

                                        }

                                        0xF2 => {

                                            println!("[transport] 🔍 DHT FindNode from {}", crate::util::mask_hash_id(&peer_id));

                                            // Извлекаем target из пакета (32 байта после 0xF2)

                                            if decrypted.len() >= 33 {

                                                let mut target_bytes = [0u8; 32];

                                                target_bytes.copy_from_slice(&decrypted[1..33]);

                                                let target = HashId(target_bytes);

                                                

                                                // Ищем ближайшие узлы в ktable

                                                let dht_lock = dht.lock().await;

                                                let closest = dht_lock.ktable.closest(&target, &local_id);

                                                

                                                // Преобразуем BucketPeer в (HashId, String)

                                                let nodes: Vec<(HashId, String)> = closest.iter()

                                                    .map(|p| (p.id.clone(), p.addr.clone()))

                                                    .collect();

                                                

                                                // Отправляем ответ

                                                if let Some(peer_info) = peer {

                                                    let mut resp_packet = vec![0xF3u8];

                                                    let data = serde_json::to_vec(&nodes)

                                                        .map_err(|e| println!("[transport] Failed to serialize: {}", e))

                                                        .ok();

                                                    if let Some(data) = data {

                                                        resp_packet.extend_from_slice(&data);

                                                        let enc = encryption.lock().await;

                                                        if let Ok(encrypted) = enc.encrypt(&peer_info, &resp_packet) {

                                                            let addr = peer_info.data_addr.as_ref().unwrap_or(&peer_info.addr);

                                                            let _ = socket.send_to(&encrypted, addr).await;

                                                        }

                                                    }

                                                }

                                            } else {

                                                println!("[transport] ⚠️  Invalid FIND_NODE packet (too short)");

                                            }

                                        }

                                        0xF3 => {

                                            println!("[transport] 📋 DHT FindNode Response from {}", crate::util::mask_hash_id(&peer_id));

                                        }

                                        0xF4 => {

                                            println!("[transport] 💾 DHT Store from {}", crate::util::mask_hash_id(&peer_id));

                                            // Формат: [0xF4][key:32][value...]

                                            if decrypted.len() >= 33 {

                                                let mut key_bytes = [0u8; 32];

                                                key_bytes.copy_from_slice(&decrypted[1..33]);

                                                let key = HashId(key_bytes);

                                                let value = decrypted[33..].to_vec();

                                                

                                                // Сохраняем в storage

                                                // "Valid hostile peer" audit (2026-09-15/16): the
                                                // bare .store() hardcodes origin="legacy" for EVERY
                                                // caller, so DhtStorage's own per-origin rate limit
                                                // (5 STORE/sec, then a 300s block) was shared across
                                                // the entire peer population instead of being per
                                                // sender — a single peer could exhaust the "legacy"
                                                // bucket and get every other peer's STORE requests
                                                // blocked too. Uses store_with_quota with the real,
                                                // cryptographically authenticated sender identity
                                                // (never the source IP/address — that isn't YANDI
                                                // identity) so the limit is local to this one peer.
                                                let mut dht_lock = dht.lock().await;

                                                if let Err(e) = dht_lock.storage.store_with_quota(key, value, hex::encode(&peer_id.0)) {
                                                    println!("[dht] ⛔ STORE rejected from {}: {}", crate::util::mask_hash_id(&peer_id), e);
                                                }

                                                

                                                // Отправляем ответ об успехе

                                                if let Some(peer_info) = peer {

                                                    let mut resp_packet = vec![0xF5u8, 1]; // 1 = success

                                                    let enc = encryption.lock().await;

                                                    if let Ok(encrypted) = enc.encrypt(&peer_info, &resp_packet) {

                                                        let addr = peer_info.data_addr.as_ref().unwrap_or(&peer_info.addr);

                                                        let _ = socket.send_to(&encrypted, addr).await;

                                                    }

                                                }

                                            } else {

                                                println!("[transport] ⚠️  Invalid STORE packet (too short)");

                                            }

                                        }

                                        0xF5 => {

                                            println!("[transport] ✅ DHT Store Response from {}", crate::util::mask_hash_id(&peer_id));

                                        }

                                        0xF6 => {

                                            println!("[transport] 🔎 DHT FindValue from {}", crate::util::mask_hash_id(&peer_id));

                                            // Формат: [0xF6][key:32]

                                            if decrypted.len() >= 33 {

                                                let mut key_bytes = [0u8; 32];

                                                key_bytes.copy_from_slice(&decrypted[1..33]);

                                                let key = HashId(key_bytes);

                                                

                                                // Ищем значение в storage

                                                // Same fix as STORE above: real per-peer origin,
                                                // never the shared "legacy" bucket. A rate-limited
                                                // lookup fails closed (treated as "not found")
                                                // rather than leaking whether a value exists.
                                                let mut dht_lock = dht.lock().await;

                                                let value = match dht_lock.storage.get_with_quota(&key, &hex::encode(&peer_id.0)) {
                                                    Ok(v) => v,
                                                    Err(e) => {
                                                        println!("[dht] ⛔ FIND_VALUE rejected from {}: {}", crate::util::mask_hash_id(&peer_id), e);
                                                        None
                                                    }
                                                };

                                                

                                                // Ищем ближайшие узлы если значения нет

                                                let closest_nodes: Vec<(HashId, String)> = if value.is_none() {

                                                    dht_lock.ktable.closest(&key, &local_id).iter()

                                                        .map(|p| (p.id.clone(), p.addr.clone()))

                                                        .collect()

                                                } else {

                                                    vec![]

                                                };

                                                

                                                // Отправляем ответ

                                                if let Some(peer_info) = peer {

                                                    let mut resp_packet = vec![0xF7u8];

                                                    let data = serde_json::to_vec(&(value, closest_nodes))

                                                        .map_err(|e| println!("[transport] Failed to serialize: {}", e))

                                                        .ok();

                                                    if let Some(data) = data {

                                                        resp_packet.extend_from_slice(&data);

                                                        let enc = encryption.lock().await;

                                                        if let Ok(encrypted) = enc.encrypt(&peer_info, &resp_packet) {

                                                            let addr = peer_info.data_addr.as_ref().unwrap_or(&peer_info.addr);

                                                            let _ = socket.send_to(&encrypted, addr).await;

                                                        }

                                                    }

                                                }

                                            } else {

                                                println!("[transport] ⚠️  Invalid FIND_VALUE packet (too short)");

                                            }

                                        }

                                        0xF7 => {

                                            println!("[transport] 📦 DHT FindValue Response from {}", crate::util::mask_hash_id(&peer_id));

                                        }

                                        // Port Update (0xF8) - peer port rotation notification
                                        0xF8 => {
                                            println!("[transport] 🔄 Port Update from {}", 
                                                     crate::util::mask_hash_id(&peer_id));
                                            
                                            if let Some(peer_info) = peer {
                                                if let Some(packet) = PortUpdatePacket::from_bytes(&decrypted) {
                                                    let mut peers_lock = peers.lock().await;
                                                    if let Some(peer) = peers_lock.get_mut(&peer_id) {
                                                        peer.update_remote_ports(
                                                            packet.discovery_port,
                                                            packet.data_port,
                                                            packet.sequence,
                                                            packet.loss_rate,
                                                            packet.rx_speed
                                                        );
                                                        println!("[transport] ✅ Updated peer {} ports: {}/{}",
                                                                 crate::util::mask_hash_id(&peer_id),
                                                                 packet.discovery_port, packet.data_port);
                                                    }
                                                }
                                            }
                                        }

                                        0x50 => {
                                            let msg = String::from_utf8_lossy(&decrypted);
                                            println!("[transport] 📨 Received PROBE message from {}",
                                                     crate::util::mask_hash_id(&peer_id));
                                            println!("[transport]    Message: {}", msg);

                                            // PROBE используется для активации NAT traversal
                                            // Обновляем активность пира
                                            {
                                                let mut peers_lock = peers.lock().await;
                                                if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                    p.touch();
                                                }
                                            }
                                            {
                                                let mut tunnels_lock = tunnels.lock().await;
                                                tunnels_lock.update_activity(&peer_id);
                                            }
                                        }
                                        // 🌐 NAT relay forwarding (0xA0).
                                        // Этот путь срабатывает ТОЛЬКО на relay-ноде:
                                        // отправитель шифрует обёртку для relay-пира, мы её
                                        // расшифровали → видим 0xA0 → парсим RelayDataPacket
                                        // и форвардим внутренний blob к target.data_addr.
                                        // На самой target-ноде relay-сервер шлёт сырые
                                        // [sender_id][nonce][ciphertext][tag] — это direct-формат,
                                        // он попадает в обычную ветку, не сюда.
                                        0xA0 => {
                                            use crate::netlayer::packet::RelayDataPacket;

                                            // 🛰  Mobile / non-relay не форвардит. Кто-то ошибся адресом.
                                            if !transport.can_serve_relay() {
                                                eprintln!("[relay] ⚠️  RelayData received, but this node is not a relay (caps=0b{:016b}) — dropping",
                                                          transport.capabilities);
                                                continue;
                                            }

                                            if decrypted.len() < 2 {
                                                eprintln!("[relay] ⚠️  RelayDataPacket too short");
                                                continue;
                                            }
                                            match bincode::deserialize::<RelayDataPacket>(&decrypted[1..]) {
                                                Ok(relay_pkt) if relay_pkt.source_peer != peer_id => {
                                                    eprintln!("[relay] ⛔ source {} does not match the authenticated sender {} — dropping",
                                                              hex::encode(&relay_pkt.source_peer.0[..8]), hex::encode(&peer_id.0[..8]));
                                                }
                                                Ok(relay_pkt) => {
                                                    let target_id = relay_pkt.target_peer;
                                                    let target = {
                                                        let peers_lock = peers.lock().await;
                                                        peers_lock.get(&target_id).cloned()
                                                    };
                                                    match target {
                                                        Some(t) => {
                                                            let target_addr = if let Some(ref a) = t.data_addr {
                                                                a.clone()
                                                            } else {
                                                                let ip = t.addr.split(':').next().unwrap_or(&t.addr);
                                                                format!("{}:{}", ip, t.remote_data_port)
                                                            };
                                                            let bytes = relay_pkt.data;
                                                            let bytes_len = bytes.len();
                                                            if let Err(e) = transport
                                                                .socket_manager
                                                                .data()
                                                                .await
                                                                .send_to(&bytes, &target_addr)
                                                                .await
                                                            {
                                                                eprintln!("[relay] ❌ forward to {} failed: {}",
                                                                          hex::encode(&target_id.0[..8]), e);
                                                            } else {
                                                                let mut rm = transport.relay_manager.lock().await;
                                                                if rm.get_session(relay_pkt.session_id).is_none() {
                                                                    rm.create_session(relay_pkt.source_peer, target_id);
                                                                }
                                                                let _ = rm.update_session_activity(
                                                                    relay_pkt.session_id,
                                                                    bytes_len,
                                                                );
                                                                println!("[relay] 🔁 forwarded {} B from {} to {} (session {})",
                                                                         bytes_len,
                                                                         hex::encode(&relay_pkt.source_peer.0[..8]),
                                                                         hex::encode(&target_id.0[..8]),
                                                                         relay_pkt.session_id);
                                                            }
                                                        }
                                                        None => {
                                                            eprintln!("[relay] ⚠️  target {} not in peer table — dropping",
                                                                      hex::encode(&target_id.0[..8]));
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    eprintln!("[relay] ❌ RelayDataPacket parse: {}", e);
                                                }
                                            }
                                        }
                                        // 🌐 MappingProbeReq (0xA2): peer спрашивает «какой у меня внешний адрес у тебя?»
                                        // Тело: пусто. Источник пакета (`from`) — это и есть наблюдаемый mapped addr.
                                        0xA2 => {
                                            // Формируем reply: [0xA3][len:2][addr_str]
                                            let observed = from.to_string();
                                            let mut reply = vec![0xA3u8];
                                            let bytes = observed.as_bytes();
                                            reply.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
                                            reply.extend_from_slice(bytes);
                                            let peer_for_reply = {
                                                let peers_lock = peers.lock().await;
                                                peers_lock.get(&peer_id).cloned()
                                            };
                                            if let Some(p) = peer_for_reply {
                                                let enc = {
                                                    let enc_lock = encryption.lock().await;
                                                    enc_lock.encrypt(&p, &reply)
                                                };
                                                if let Ok(data) = enc {
                                                    let dest = p.data_addr.clone()
                                                        .unwrap_or_else(|| p.addr.clone());
                                                    let _ = transport.socket_manager.data().await
                                                        .send_to(&data, &dest).await;
                                                    println!("[nat] 📡 reply to MappingProbeReq from {}: observed = {}",
                                                             hex::encode(&peer_id.0[..8]),
                                                             crate::util::mask_ipv4(&observed));
                                                }
                                            }
                                        }
                                        // 🌐 MappingProbeReply (0xA3): peer вернул нам observed адрес.
                                        // Тело: [len:2][addr_str].
                                        0xA3 => {
                                            if decrypted.len() < 3 {
                                                continue;
                                            }
                                            let len = u16::from_be_bytes([decrypted[1], decrypted[2]]) as usize;
                                            if decrypted.len() < 3 + len {
                                                continue;
                                            }
                                            let observed = String::from_utf8_lossy(&decrypted[3..3 + len]).to_string();
                                            println!("[nat] 📡 MappingProbeReply from {}: we appear as {}",
                                                     hex::encode(&peer_id.0[..8]),
                                                     crate::util::mask_ipv4(&observed));
                                            let mut probes = transport.mapping_probes.write().await;
                                            probes.insert(peer_id, observed.clone());

                                            // Считаем поведение: уникальные observed → EDM, одинаковые → EIM.
                                            // Достаточно ≥2 ответов от разных peer'ов.
                                            if probes.len() >= 2 {
                                                let unique: std::collections::HashSet<&String> =
                                                    probes.values().collect();
                                                let behavior = if unique.len() == 1 {
                                                    MappingBehavior::EndpointIndependent
                                                } else {
                                                    MappingBehavior::EndpointDependent
                                                };
                                                drop(probes);
                                                let mut current = transport.local_nat_mapping.write().await;
                                                if *current != behavior {
                                                    println!("[nat] 🔍 LOCAL NAT mapping behavior: {} → {}",
                                                             current.as_str(), behavior.as_str());
                                                    *current = behavior;
                                                }
                                            }
                                        }
                                        // 🌐 PUNCH_REQ (0xA4): A → introducer Z, тело [target:32].
                                        // Z должен переслать обоим участникам PUNCH_INTRO с mapped addr контрагента.
                                        0xA4 if decrypted.len() >= 33 => {
                                            // 🛰  Mobile / non-introducer не сводит. Игнор.
                                            if !transport.can_introduce_peers() {
                                                eprintln!("[punch] ⚠️  PUNCH_REQ received, but this node is not an introducer — dropping");
                                                continue;
                                            }
                                            let mut tid = [0u8; 32];
                                            tid.copy_from_slice(&decrypted[1..33]);
                                            let target_id = HashId(tid);
                                            let initiator_id = peer_id;

                                            // Z знает observed addr A — это `from`. Знает observed addr B — берём из peer table.
                                            let target_peer = {
                                                let peers_lock = peers.lock().await;
                                                peers_lock.get(&target_id).cloned()
                                            };
                                            let target = match target_peer {
                                                Some(t) => t,
                                                None => {
                                                    eprintln!("[punch] introducer: target {} unknown",
                                                              hex::encode(&target_id.0[..8]));
                                                    continue;
                                                }
                                            };
                                            let target_observed = target.data_addr.clone()
                                                .unwrap_or_else(|| target.addr.clone());
                                            let initiator_observed = from.to_string();

                                            // Сборка PUNCH_INTRO: [0xA5][peer_id:32][addr_len:2][addr_str]
                                            let build_intro = |pid: HashId, addr: &str| -> Vec<u8> {
                                                let mut out = vec![0xA5u8];
                                                out.extend_from_slice(&pid.0);
                                                let bytes = addr.as_bytes();
                                                out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
                                                out.extend_from_slice(bytes);
                                                out
                                            };

                                            // Шлём A: «вот B и его mapped addr».
                                            let intro_for_a = build_intro(target_id, &target_observed);
                                            if let Some(a_peer) = {
                                                let peers_lock = peers.lock().await;
                                                peers_lock.get(&initiator_id).cloned()
                                            } {
                                                let enc = {
                                                    let enc_lock = encryption.lock().await;
                                                    enc_lock.encrypt(&a_peer, &intro_for_a)
                                                };
                                                if let Ok(data) = enc {
                                                    let _ = transport.socket_manager.data().await
                                                        .send_to(&data, &initiator_observed).await;
                                                }
                                            }
                                            // Шлём B: «вот A и его mapped addr».
                                            let intro_for_b = build_intro(initiator_id, &initiator_observed);
                                            let enc = {
                                                let enc_lock = encryption.lock().await;
                                                enc_lock.encrypt(&target, &intro_for_b)
                                            };
                                            if let Ok(data) = enc {
                                                let _ = transport.socket_manager.data().await
                                                    .send_to(&data, &target_observed).await;
                                            }
                                            println!("[punch] 🤝 introduced {} ({}) ↔ {} ({})",
                                                     hex::encode(&initiator_id.0[..8]),
                                                     crate::util::mask_ipv4(&initiator_observed),
                                                     hex::encode(&target_id.0[..8]),
                                                     crate::util::mask_ipv4(&target_observed));
                                        }
                                        // 🌐 PUNCH_INTRO (0xA5): получили mapped addr контрагента — стреляем серией probe'ов.
                                        // Тело: [peer_id:32][addr_len:2][addr_str]
                                        0xA5 if decrypted.len() >= 35 => {
                                            let mut pid = [0u8; 32];
                                            pid.copy_from_slice(&decrypted[1..33]);
                                            let other_id = HashId(pid);
                                            let len = u16::from_be_bytes([decrypted[33], decrypted[34]]) as usize;
                                            if decrypted.len() < 35 + len { continue; }
                                            let addr = String::from_utf8_lossy(&decrypted[35..35 + len]).to_string();
                                            println!("[punch] 📩 intro received: punch {} at {}",
                                                     hex::encode(&other_id.0[..8]),
                                                     crate::util::mask_ipv4(&addr));

                                            // Burst 5 probes × 50 мс. Probe = encrypted [0xA1].
                                            let other_peer = {
                                                let peers_lock = peers.lock().await;
                                                peers_lock.get(&other_id).cloned()
                                            };
                                            if let Some(op) = other_peer {
                                                let enc_lock = encryption.lock().await;
                                                let probe_payload = vec![0xA1u8];
                                                let probe_enc = enc_lock.encrypt(&op, &probe_payload).ok();
                                                drop(enc_lock);
                                                if let Some(probe_enc) = probe_enc {
                                                    let socket_send = transport.socket_manager.data().await.clone();
                                                    let addr_clone = addr.clone();
                                                    tokio::spawn(async move {
                                                        for i in 0..5 {
                                                            let _ = socket_send.send_to(&probe_enc, &addr_clone).await;
                                                            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                                                            tracing::debug!("[punch] probe #{} → {}", i + 1, addr_clone);
                                                        }
                                                    });
                                                }
                                            }
                                        }
                                        // 🌐 PUNCH_PROBE (0xA1): прошёл сквозь NAT — direct путь открылся.
                                        // Обновляем data_addr на тот, с которого пришло, снимаем use_relay.
                                        0xA1 => {
                                            let new_addr = from.to_string();
                                            let mut peers_lock = peers.lock().await;
                                            if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                let was_relay = p.use_relay;
                                                p.data_addr = Some(new_addr.clone());
                                                p.use_relay = false;
                                                p.direct_miss_streak = 0;
                                                p.touch();
                                                if was_relay {
                                                    println!("[punch] ✅ direct path established with {} via {}",
                                                             hex::encode(&peer_id.0[..8]),
                                                             crate::util::mask_ipv4(&new_addr));
                                                }
                                            }
                                        }
                                        // Application message (or unrecognized)
                                        _ => {
                                            // Проверяем, является ли это текстовым сообщением (ASCII printable)
                                            let is_text_message = decrypted.iter().all(|&b| b >= 32 && b <= 126 || b == 10 || b == 13 || b == 9);

                                            if is_text_message {
                                                // Это текстовое приложение-уровня сообщение (от команды send)
                                                let msg = String::from_utf8_lossy(&decrypted);
                                                let short_id = hex::encode(&peer_id.0[..8]);
                                                let peer_ip = peer.as_ref()
                                                    .and_then(|p| p.data_addr.as_ref().map(String::as_str))
                                                    .or_else(|| peer.as_ref().map(|p| p.addr.as_str()))
                                                    .unwrap_or("unknown");
                                                println!("[transport] ✅ Received text message from {} (Short ID: {}, IP: {})",
                                                         crate::util::mask_hash_id(&peer_id), short_id, peer_ip);
                                                println!("[transport] 📩 Message: {}", msg);
                                            } else {
                                                // Неизвестный бинарный протокол
                                                eprintln!("[transport] ⚠️  Unknown message type: {:#04x}, len: {}, first 20 bytes: {:?}",
                                                         decrypted[0], decrypted.len(),
                                                         &decrypted[..decrypted.len().min(20)]);
                                                let short_id = hex::encode(&peer_id.0[..8]);
                                                let peer_ip = peer.as_ref()
                                                    .and_then(|p| p.data_addr.as_ref().map(String::as_str))
                                                    .or_else(|| peer.as_ref().map(|p| p.addr.as_str()))
                                                    .unwrap_or("unknown");
                                                println!("[transport] ✅ Decrypted {} bytes from {} (Short ID: {}, IP: {})",
                                                         decrypted.len(), crate::util::mask_hash_id(&peer_id), short_id, peer_ip);
                                                println!("[transport] 📩 Message: {}",
                                                         String::from_utf8_lossy(&decrypted));
                                            }

                                            {
                                                let mut peers_lock = peers.lock().await;
                                                if let Some(p) = peers_lock.get_mut(&peer_id) {
                                                    p.touch();
                                                }
                                            }
                                            {
                                                let mut tunnels_lock = tunnels.lock().await;
                                                tunnels_lock.update_activity(&peer_id);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                println!("[transport] ❌ Decryption failed: {}", e);
                                println!("[transport]    Possible mismatched session keys!");
                                println!("[transport] 🛡️  SECURITY: Packet dropped - invalid encryption");
                            }
                        }
                }
                Err(e) => {
                    println!("[transport] ❌ Socket error: {}", e);
                    continue;
                }
            }
        }
    }

    /// Send Hello Request to peer
    pub async fn send_hello_request(&self, addr: &str) -> Result<(), String> {
        println!("[transport] 📤 Sending HELLO_REQ to {}", addr);
        // куда стучаться по TCP, если UDP до этого узла не дойдёт
        if let (Some(c), Ok(sa)) = (tcp_carrier::global(), addr.parse::<std::net::SocketAddr>()) {
            c.hint(sa);
        }
        let ports = self.port_manager.current_state();

        // Get CID from node ID (first 8 bytes)
        let node_id = self.identity.node_id();
        let mut cid = [0u8; 8];
        cid.copy_from_slice(&node_id.0[..8]);

        // Get Ed25519 signing public key for self-certifying identity
        let public_key = self.identity.signing_public_key;

        // Ключ обмена — одноразовый, на это приглашение (подставляется ниже, когда известен номер приглашения)
        let x25519_public = [0u8; 32];

        // Create Hello Request
        let ipv6_virtual = self.identity.generate_ipv6_virtual();
        let ipv6_bytes: Option<[u8; 16]> = ipv6_virtual
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|addr| addr.octets());

        let mut hello_packet = HelloPacket::new_request(
            node_id,
            public_key,
            x25519_public,
            cid,
            self.capabilities, // ✅ Use actual capabilities from system monitoring
        );

        // Одноразовый ключ для этого приглашения: номер приглашения = nonce приветствия; секрет хранится до подтверждения и стирается при использовании
        hello_packet.x25519_public = {
            let mut encryption = self.encryption.lock().await;
            encryption.generate_hello_ephemeral(hello_packet.nonce)
        };

        // Add IPv6 virtual address
        if let Some(ipv6) = ipv6_bytes {
            hello_packet.ipv6_virtual = Some(ipv6);
        }

        // Advertise current active netlayer data port. Default 10000 remains open as fallback.
        hello_packet.discovery_endpoint = Some(format!("0.0.0.0:{}", ports.data_port));

        // Set P2P data endpoint (port 9998)
        hello_packet.p2p_data_addr = Some(format!("0.0.0.0:{}", crate::p2p::transport::configured_ports().0));

        // Generate temporary P2P X25519 key pair (TODO: move to P2PEncryptionManager)
        let p2p_private_key = x25519_dalek::EphemeralSecret::new(rand::thread_rng());
        let p2p_public_key = x25519_dalek::PublicKey::from(&p2p_private_key);
        hello_packet = hello_packet.with_p2p_x25519_public(p2p_public_key.to_bytes());


        // Set WAN address (for NAT traversal)
        if let Some(ref ext_ip) = *self.external_ip.read().await {
            hello_packet.wan_address = Some(format!("{}:{}", ext_ip, ports.data_port));
        }

        // Set LAN address from topology (for NAT traversal)
        if let Some(ref topo) = self.topology {
            if let Some(lan_ip) = topo.get_primary_lan_ip() {
                hello_packet.lan_address = Some(format!("{}:{}", lan_ip, ports.data_port));
            }
        }

        // Sign the packet with identity
        let challenge = hello_packet.challenge_data();
        let signature = self.identity.sign(&challenge)
            .map_err(|e| format!("Failed to sign Hello packet: {}", e))?;
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&signature);
        hello_packet.signature = crate::netlayer::packet::Signature(sig_bytes);

        // Serialize and send
        let data = hello_packet.to_bytes()
            .map_err(|e| format!("Failed to serialize Hello packet: {}", e))?;

        self.socket_manager.discovery().await.send_to(&data, addr)
            .await
            .map_err(|e| format!("Failed to send Hello packet: {}", e))?;

        println!("[transport] ✅ HELLO_REQ sent to {} ({} bytes)", addr, data.len());

        Ok(())
    }

    /// Ответить на приглашение: своим одноразовым ключом, сеанс поднимается сразу.
    /// Ошибка «встречное знакомство» — остаётся ключ рукопожатия узла с меньшим номером, подтверждение не отправляется.
    pub async fn send_hello_ack(&self, addr: &str, request_nonce: u64, peer_id: HashId, peer_ephemeral: [u8; 32]) -> Result<(), String> {
        println!("[transport] 📤 Sending HELLO_ACK to {}", addr);
        let ports = self.port_manager.current_state();

        // Get CID from node ID
        let node_id = self.identity.node_id();
        let mut cid = [0u8; 8];
        cid.copy_from_slice(&node_id.0[..8]);

        // Get Ed25519 signing public key for self-certifying identity
        let public_key = self.identity.signing_public_key;

        // Одноразовый ключ ответчика: общий секрет считается сразу, сеанс сохранён, секрет не хранится
        let x25519_public = {
            let mut encryption = self.encryption.lock().await;
            encryption.complete_hello_responder(peer_id, &peer_ephemeral)?
        };
        {
            let mut tunnels = self.tunnels.lock().await;
            tunnels.register_tunnel(peer_id);
        }

        // Create Hello Ack
        let ipv6_virtual = self.identity.generate_ipv6_virtual();
        let ipv6_bytes: Option<[u8; 16]> = ipv6_virtual
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|addr| addr.octets());

        let hello_packet = HelloPacket::new_ack_with_ipv6(
            node_id,
            public_key,
            x25519_public,
            cid,
            self.capabilities, // ✅ Use actual capabilities from system monitoring
            request_nonce,
            ipv6_bytes,
        );

        // Generate temporary P2P X25519 key pair for ACK
        let p2p_private_key_ack = x25519_dalek::EphemeralSecret::new(rand::thread_rng());
        let p2p_public_key_ack = x25519_dalek::PublicKey::from(&p2p_private_key_ack);
        let hello_packet = hello_packet.with_p2p_x25519_public(p2p_public_key_ack.to_bytes());

        // Set discovery endpoint
        let hello_packet = hello_packet
            .with_discovery_endpoint(format!("0.0.0.0:{}", ports.data_port))
            .with_p2p_data_addr(format!("0.0.0.0:{}", crate::p2p::transport::configured_ports().0));

        // Set WAN address (for NAT traversal)
        let mut hello_packet = hello_packet;
        if let Some(ref ext_ip) = *self.external_ip.read().await {
            hello_packet.wan_address = Some(format!("{}:{}", ext_ip, ports.data_port));
        }

        // Set LAN address from topology (for NAT traversal)
        if let Some(ref topo) = self.topology {
            if let Some(lan_ip) = topo.get_primary_lan_ip() {
                hello_packet.lan_address = Some(format!("{}:{}", lan_ip, ports.data_port));
            }
        }

        // Sign the packet with identity
        let challenge = hello_packet.challenge_data();
        let signature = self.identity.sign(&challenge)
            .map_err(|e| format!("Failed to sign Hello Ack packet: {}", e))?;
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&signature);
        hello_packet.signature = crate::netlayer::packet::Signature(sig_bytes);

        // Serialize and send
        let data = hello_packet.to_bytes()
            .map_err(|e| format!("Failed to serialize Hello packet: {}", e))?;

        self.socket_manager.discovery().await.send_to(&data, addr)
            .await
            .map_err(|e| format!("Failed to send Hello packet: {}", e))?;

        println!("[transport] ✅ HELLO_ACK sent to {} ({} bytes)", addr, data.len());

        Ok(())
    }

    /// Инициатор: пришло подтверждение. Сеанс поднимается из одноразового ключа приглашения с этим номером и ключа ответчика.
    pub async fn establish_session(&self, peer_id: HashId, request_nonce: u64, ack_ephemeral: [u8; 32]) -> Result<u64, String> {
        println!("[transport] 🔐 Establishing session with peer: {}",
                 hex::encode(&peer_id.0[..8]));

        let version = {
            let mut encryption = self.encryption.lock().await;
            match encryption.complete_hello_initiator(request_nonce, peer_id, &ack_ephemeral) {
                Ok(v) => v,
                Err(e) => {
                    println!("[transport] ⚠️  Session establishment failed: {}", e);
                    return Err(e);
                }
            }
        };

        // Регистрируем тоннель
        {
            let mut tunnels = self.tunnels.lock().await;
            tunnels.register_tunnel(peer_id);
        }

        println!("[transport] ✅ Session v{} and tunnel established for peer: {}",
                 version, hex::encode(&peer_id.0[..8]));
        Ok(version)
    }

    /// Send encrypted data to peer on port 10000
    /// Если ключ сеанса с узлом состарился — отправить ему новое приглашение к рукопожатию (не чаще раза в минуту). Старый ключ ещё принимает пакеты в пути.
    async fn rekey_if_due(&self, peer: &PeerInfo) {
        let due = { self.encryption.lock().await.needs_rekey(peer) };
        if !due {
            return;
        }
        {
            let mut asked = self.rekey_requested.lock().await;
            if asked.get(&peer.id).map(|t| t.elapsed() < std::time::Duration::from_secs(60)).unwrap_or(false) {
                return;
            }
            asked.insert(peer.id, Instant::now());
            if asked.len() > 4096 {
                asked.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(120));
            }
        }
        println!("[transport] 🔑 session key with {} is old — asking for a new one", hex::encode(&peer.id.0[..8]));
        if let Err(e) = self.send_hello_request(&peer.addr).await {
            println!("[transport] ⚠️  rekey request failed: {}", e);
        }
    }

    /// Сколько пакетов осталось до потолка, нужно ли менять ключ — для проверок.
    pub async fn session_needs_rekey(&self, peer: &PeerInfo) -> bool {
        self.encryption.lock().await.needs_rekey(peer)
    }

    pub async fn send_encrypted(&self, peer_id: HashId, data: &[u8]) -> Result<(), String> {
        println!("[transport] 📤 [SEND-START] Preparing to send {} bytes to peer: {}",
                 data.len(), hex::encode(&peer_id.0[..8]));

        let peer = {
            let peers = self.peers.lock().await;
            peers.get(&peer_id).cloned()
        };

        let peer = match peer {
            Some(p) => p,
            None => return Err(format!("Peer not found: {}", hex::encode(&peer_id.0[..8]))),
        };

        // Старый ключ (час работы или очень много пакетов): договариваемся о новом заранее, не дожидаясь потолка
        self.rekey_if_due(&peer).await;

        // 📡 WS shortcut: peer подключён через WS-туннель → шлём шифрованный wagon в его mpsc.
        // Не идём в UDP-сокет вообще.
        let ws_sender = {
            let ws = self.ws_outgoing.lock().await;
            ws.get(&peer_id).cloned()
        };
        if let Some(tx) = ws_sender {
            let encrypted = {
                let encryption = self.encryption.lock().await;
                encryption.encrypt(&peer, data)
                    .map_err(|e| format!("WS encrypt failed: {}", e))?
            };
            tx.send(encrypted).await
                .map_err(|e| format!("WS channel closed: {}", e))?;
            return Ok(());
        }

        // 🌐 NAT dispatch: если peer за NAT и direct до него не работает —
        // заворачиваем в RelayDataPacket и шлём через любой Public peer.
        // Поведение управляется флагом peer.use_relay (выставляется heartbeat-логикой);
        // если флаг не выставлен, идём direct (как раньше).
        if peer.use_relay {
            if let Some(relay_peer) = self.pick_relay(&peer_id).await {
                return self.send_via_relay(&peer, &relay_peer, data).await;
            }
            // Нет доступного relay — деградируем в direct, чтобы не падать молча.
            eprintln!("[transport] ⚠️  Peer {} нуждается в relay, но Public peer не найден — пробую direct",
                      hex::encode(&peer_id.0[..8]));
        }

        // Use data_addr if available, otherwise extract from addr
        let data_endpoint = if let Some(ref addr) = peer.data_addr {
            addr.clone()
        } else {
            let ip = peer.addr.split(':').next().unwrap_or(&peer.addr);
            format!("{}:{}", ip, peer.remote_data_port)
        };

        // Encrypt data
        let encrypted = {
            let encryption = self.encryption.lock().await;
            match encryption.encrypt(&peer, data) {
                Ok(enc) => enc,
                Err(e) => {
                    eprintln!("[transport] ❌ [ENCRYPTION FAILED] peer={}, error={}",
                             hex::encode(&peer_id.0[..8]), e);
                    return Err(format!("Encryption failed: {}", e));
                }
            }
        };

        println!("[transport] 📤 [SEND-ENCRYPTED] Encrypted to {} bytes, sending to: {}",
                 encrypted.len(), crate::util::mask_ipv4(&data_endpoint));

        // Send on SEND socket (separate from recv socket!)
        self.socket_manager.data().await.send_to(&encrypted, &data_endpoint)
            .await
            .map_err(|e| format!("Failed to send encrypted data: {}", e))?;

        self.tx_bytes_counter.fetch_add(encrypted.len() as u64, Ordering::Relaxed);

        println!("[transport] ✅ [SEND-COMPLETE] Sent {} bytes to {} - socket should be free now",
                 encrypted.len(), crate::util::mask_ipv4(&data_endpoint));

        Ok(())
    }

    /// 📡 Mobile подключается к anchor'у по wss://. Используется при `--anchor-url`.
    /// Шлёт Hello, получает peer_id anchor'а, регистрирует ws_outgoing для маршрутизации.
    pub async fn connect_to_anchor_ws(
        self: &Arc<Self>,
        anchor_url: &str,
        anchor_fingerprint_hex: &str,
    ) -> Result<HashId, String> {
        use crate::netlayer::ws_transport::connect_to_anchor;

        let mut conn = connect_to_anchor(anchor_url, anchor_fingerprint_hex).await
            .map_err(|e| format!("WS connect: {}", e))?;
        println!("[ws-client] 🌐 connected to {}", conn.peer_addr);

        // 🔐 Hardening Step 2: попытка resume по сохранённому SessionToken (с session-key).
        // Если получится — пропускаем Hello/ECDH; иначе fallback на обычный Hello flow.
        let resume_candidate = {
            let store = self.paired_anchors.lock().await;
            // Ищем entry чей URL соответствует anchor_url и есть session_token с session_key.
            store.anchors.iter().find_map(|e| {
                if e.payload.anchor_url == anchor_url {
                    e.session.as_ref().and_then(|tok| {
                        if tok.is_expired() { return None; }
                        let sk = tok.session_key()?;
                        Some((e.payload.anchor_id, tok.session_id, tok.resume_secret().ok()?, sk))
                    })
                } else {
                    None
                }
            })
        };

        if let Some((anchor_id, session_id, resume_secret, session_key)) = resume_candidate {
            use crate::netlayer::pairing::{
                compute_resume_mac, decode_resume_ack, encode_resume, ResumeStatus,
            };
            // Restore session-key на mobile-стороне — нужно чтобы расшифровать 0xC1 ACK.
            {
                let mut enc = self.encryption.lock().await;
                let _ = enc.restore_session(anchor_id, session_key);
            }
            // Регистрируем peer заранее, чтобы send_encrypted/decrypt смогли его найти.
            let mut peer = PeerInfo::new(anchor_id, &conn.peer_addr);
            peer.touch();
            self.peers.lock().await.insert(anchor_id, peer);
            self.ws_outgoing.lock().await.insert(anchor_id, conn.outgoing.clone());

            let my_node_id = self.identity.node_id();
            let mac = compute_resume_mac(&resume_secret, session_id, &conn.peer_addr);
            let resume_bytes = encode_resume(my_node_id, session_id, &conn.peer_addr, &mac);
            if let Err(e) = conn.outgoing.send(resume_bytes).await {
                self.ws_outgoing.lock().await.remove(&anchor_id);
                return Err(format!("RESUME send: {}", e));
            }
            println!("[ws-client] 🔁 RESUME sent (session {:#x})", session_id);

            // Ждём 0xC1 ACK (encrypted). Один decrypt-проход.
            let ack_bytes = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                conn.incoming.recv(),
            ).await
                .map_err(|_| "RESUME_ACK timeout".to_string())?
                .ok_or_else(|| "RESUME_ACK: connection closed".to_string())?;

            // Сначала попробуем encrypted (если anchor вернул encrypted ACK).
            let plain_ack = {
                let mut enc = self.encryption.lock().await;
                match enc.decrypt_by_peer_id(&ack_bytes) {
                    Ok((_, p)) => Some(p),
                    Err(_) => None,
                }
            };
            let plain_ack = plain_ack.unwrap_or_else(|| ack_bytes.clone());

            match decode_resume_ack(&plain_ack) {
                Ok((ResumeStatus::Ok, _)) => {
                    println!("[ws-client] ✅ RESUME accepted, skipping Hello");

                    // Spawn decrypt pump — тот же что и в Hello-flow.
                    let transport_clone = self.clone();
                    tokio::spawn(async move {
                        while let Some(bytes) = conn.incoming.recv().await {
                            let dec = {
                                let mut enc = transport_clone.encryption.lock().await;
                                enc.decrypt_by_peer_id(&bytes)
                            };
                            match dec {
                                Ok((sender_id, plain)) => {
                                    if sender_id != anchor_id {
                                        eprintln!("[ws-client] dropped: packet from {} on a connection bound to {}",
                                                  hex::encode(&sender_id.0[..8]), hex::encode(&anchor_id.0[..8]));
                                        continue;
                                    }
                                    if let Some(p) = transport_clone.peers.lock().await.get_mut(&sender_id) {
                                        p.touch();
                                    }
                                    let handled = transport_clone
                                        .dispatch_decrypted_wagon(sender_id, &plain, WagonSource::Ws)
                                        .await;
                                    if !handled {
                                        println!("[ws-client/resume] 📥 wagon {} B from {} (0x{:02x})",
                                                 plain.len(),
                                                 hex::encode(&sender_id.0[..8]),
                                                 plain.first().copied().unwrap_or(0));
                                    }
                                }
                                Err(e) => eprintln!("[ws-client/resume] decrypt failed: {}", e),
                            }
                        }
                        transport_clone.ws_outgoing.lock().await.remove(&anchor_id);
                        println!("[ws-client/resume] 🛑 anchor connection closed");
                    });
                    return Ok(anchor_id);
                }
                Ok((status, _)) => {
                    eprintln!("[ws-client] RESUME rejected: {:?}; fallback на Hello", status);
                    // Удалим то что заранее зарегистрировали — Hello-flow зарегистрирует заново.
                    self.ws_outgoing.lock().await.remove(&anchor_id);
                    // Conn потерян — нужно переподключиться. Сообщаем caller'у что нужен retry.
                    return Err(format!("RESUME status {:?}, reconnect for Hello", status));
                }
                Err(e) => {
                    eprintln!("[ws-client] RESUME_ACK decode failed: {}", e);
                    self.ws_outgoing.lock().await.remove(&anchor_id);
                    return Err(format!("RESUME_ACK decode: {}", e));
                }
            }
        }

        // Шлём Hello (тот же формат что по UDP discovery).
        let my_node_id = self.identity.node_id();
        let mut cid8 = [0u8; 8];
        cid8.copy_from_slice(&my_node_id.0[..8]);
        let hello = HelloPacket::new_request(
            my_node_id,
            self.identity.signing_public_key,
            self.identity.public_key,
            cid8,
            self.capabilities,
        );
        let hello = self.sign_hello(hello)?;
        let hello_bytes = hello.to_bytes()
            .map_err(|e| format!("Hello to_bytes: {}", e))?;
        conn.outgoing.send(hello_bytes).await
            .map_err(|e| format!("Hello send: {}", e))?;
        println!("[ws-client] 👋 Hello sent");

        // Anchor должен ответить Hello-Ack — извлекаем его node_id из этого ответа.
        let ack_bytes = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            conn.incoming.recv(),
        ).await
            .map_err(|_| "Hello-Ack timeout".to_string())?
            .ok_or_else(|| "Hello-Ack: connection closed".to_string())?;
        let ack = HelloPacket::from_bytes(&ack_bytes)
            .map_err(|e| format!("Hello-Ack parse: {}", e))?;
        if let Err(e) = P2PTransport::verify_peer_handshake_static(&ack) {
            return Err(format!("Hello-Ack rejected: {}", e));
        }
        let anchor_id = ack.node_id;
        println!("[ws-client] 👋 Hello-Ack from {}", hex::encode(&anchor_id.0[..8]));

        // Регистрируем anchor как peer и проставляем ws_outgoing.
        let mut peer = PeerInfo::new(anchor_id, &conn.peer_addr);
        peer.caps_bits = ack.capabilities;
        peer.jurisdiction = ack.jurisdiction.clone();
        peer.touch();
        self.peers.lock().await.insert(anchor_id, peer);
        self.ws_outgoing.lock().await.insert(anchor_id, conn.outgoing.clone());

        // 🚇 Integration Step 2: pump для входящих encrypted wagon'ов от anchor'а.
        // Декриптим → `dispatch_decrypted_wagon` (общий с UDP).
        let transport_clone = self.clone();
        tokio::spawn(async move {
            while let Some(bytes) = conn.incoming.recv().await {
                let dec = {
                    let mut enc = transport_clone.encryption.lock().await;
                    enc.decrypt_by_peer_id(&bytes)
                };
                match dec {
                    Ok((sender_id, plain)) => {
                        if sender_id != anchor_id {
                            eprintln!("[ws-client] dropped: packet from {} on a connection bound to {}",
                                      hex::encode(&sender_id.0[..8]), hex::encode(&anchor_id.0[..8]));
                            continue;
                        }
                        if let Some(p) = transport_clone.peers.lock().await.get_mut(&sender_id) {
                            p.touch();
                        }
                        let handled = transport_clone
                            .dispatch_decrypted_wagon(sender_id, &plain, WagonSource::Ws)
                            .await;
                        if !handled {
                            println!("[ws-client] 📥 wagon {} B from {} (first byte 0x{:02x}) — not dispatched",
                                     plain.len(),
                                     hex::encode(&sender_id.0[..8]),
                                     plain.first().copied().unwrap_or(0));
                        }
                    }
                    Err(e) => {
                        eprintln!("[ws-client] decrypt failed: {}", e);
                    }
                }
            }
            transport_clone.ws_outgoing.lock().await.remove(&anchor_id);
            println!("[ws-client] 🛑 anchor connection closed");
        });

        Ok(anchor_id)
    }

    /// 🛰  Узнаём свою роль через capabilities bits.
    pub fn is_mobile(&self) -> bool {
        use crate::netlayer::packet::hello_caps::MOBILE;
        (self.capabilities & MOBILE) != 0
    }

    pub fn is_anchor(&self) -> bool {
        use crate::netlayer::packet::hello_caps::ANCHOR;
        (self.capabilities & ANCHOR) != 0
    }

    pub fn can_serve_relay(&self) -> bool {
        use crate::netlayer::packet::hello_caps::{RELAY, MOBILE};
        (self.capabilities & RELAY) != 0 && (self.capabilities & MOBILE) == 0
    }

    pub fn can_introduce_peers(&self) -> bool {
        use crate::netlayer::packet::hello_caps::{INTRODUCER, MOBILE};
        (self.capabilities & INTRODUCER) != 0 && (self.capabilities & MOBILE) == 0
    }

    /// 🌐 Инициировать UDP hole punching через introducer (любой Public peer != target).
    /// Шлём ему PUNCH_REQ(target_id), он перешлёт PUNCH_INTRO обоим — серия probe'ов запустится.
    /// Если наш local_nat_mapping = EDM, hole punching почти гарантированно не сработает,
    /// поэтому пропускаем и оставляем relay.
    pub async fn initiate_hole_punch(&self, target_id: HashId) -> Result<(), String> {
        let mapping = *self.local_nat_mapping.read().await;
        if matches!(mapping, MappingBehavior::EndpointDependent) {
            return Err("local NAT is EDM (symmetric) — hole punch likely useless".to_string());
        }
        let introducer = self.pick_relay(&target_id).await
            .ok_or_else(|| "no public introducer available".to_string())?;

        let mut req = vec![0xA4u8];
        req.extend_from_slice(&target_id.0);

        // Шлём introducer'у directly, без relay-обёртки (он точно достижим).
        let encrypted = {
            let enc = self.encryption.lock().await;
            enc.encrypt(&introducer, &req)
                .map_err(|e| format!("encrypt PUNCH_REQ: {}", e))?
        };
        let dest = introducer.data_addr.clone()
            .unwrap_or_else(|| introducer.addr.clone());
        self.socket_manager.data().await.send_to(&encrypted, &dest).await
            .map_err(|e| format!("send PUNCH_REQ: {}", e))?;

        println!("[punch] 🚀 PUNCH_REQ sent to introducer {} (target {})",
                 hex::encode(&introducer.id.0[..8]),
                 hex::encode(&target_id.0[..8]));
        Ok(())
    }

    /// 🌐 Выбрать relay для peer'а: peer который объявил бит RELAY в caps_bits и не Mobile.
    /// Если biт нет — fallback на старый критерий (Public/MultiHomed по NatStatus).
    /// Возвращает PeerInfo выбранного relay или None.
    async fn pick_relay(&self, target_id: &HashId) -> Option<PeerInfo> {
        let peers = self.peers.lock().await;
        // Приоритет 1: peer с явным битом RELAY и без MOBILE.
        let by_caps = peers.values()
            .find(|p| p.id != *target_id && p.can_serve_relay());
        if let Some(p) = by_caps {
            return Some(p.clone());
        }
        // Приоритет 2 (legacy fallback): peer с public-IP по NatStatus.
        // Используется только если ни один peer ещё не успел объявить себя через caps.
        peers
            .values()
            .find(|p| p.id != *target_id
                  && p.nat_status.can_accept_direct_connection()
                  && !p.is_mobile())
            .cloned()
    }

    /// 🌐 Отправить data через relay: оборачиваем в RelayDataPacket с magic 0xA0,
    /// шифруем для relay-peer'а, отправляем на его data_addr.
    /// Внутренний `data` — это plaintext, который сначала шифруем для конечного target'а,
    /// иначе relay видел бы плейн.
    async fn send_via_relay(
        &self,
        target: &PeerInfo,
        relay: &PeerInfo,
        data: &[u8],
    ) -> Result<(), String> {
        // Внутреннее шифрование — для конечного target'а.
        let inner_encrypted = {
            let encryption = self.encryption.lock().await;
            encryption.encrypt(target, data)
                .map_err(|e| format!("Inner encryption (for target) failed: {}", e))?
        };

        // Сессия relay: одна на пару (source, target).
        let session_id = {
            let mut rm = self.relay_manager.lock().await;
            if let Some(s) = rm.get_session_by_peer(&target.id) {
                s.session_id
            } else {
                rm.create_session(self.identity.node_id(), target.id)
            }
        };

        let relay_pkt = crate::netlayer::packet::RelayDataPacket::new(
            session_id,
            self.identity.node_id(),
            target.id,
            inner_encrypted,
        );
        let mut wrapper = vec![0xA0u8];
        let pkt_bytes = bincode::serialize(&relay_pkt)
            .map_err(|e| format!("RelayDataPacket serialize: {}", e))?;
        wrapper.extend_from_slice(&pkt_bytes);

        // Внешнее шифрование — для relay'я (чтобы скрыть метаданные).
        let outer_encrypted = {
            let encryption = self.encryption.lock().await;
            encryption.encrypt(relay, &wrapper)
                .map_err(|e| format!("Outer encryption (for relay) failed: {}", e))?
        };

        let relay_endpoint = if let Some(ref addr) = relay.data_addr {
            addr.clone()
        } else {
            let ip = relay.addr.split(':').next().unwrap_or(&relay.addr);
            format!("{}:{}", ip, relay.remote_data_port)
        };

        self.socket_manager.data().await.send_to(&outer_encrypted, &relay_endpoint)
            .await
            .map_err(|e| format!("send via relay failed: {}", e))?;

        self.tx_bytes_counter.fetch_add(outer_encrypted.len() as u64, Ordering::Relaxed);

        println!("[relay] 🛰  → relay {} (session {}, target {}, inner={} B, outer={} B)",
                 hex::encode(&relay.id.0[..8]),
                 session_id,
                 hex::encode(&target.id.0[..8]),
                 data.len(),
                 outer_encrypted.len());

        Ok(())
    }

    pub async fn get_web_metrics(&self) -> WebTransportMetrics {
        let peers = self.get_peers().await;
        let peer_ids: Vec<HashId> = peers.iter().map(|peer| peer.id).collect();

        let station = {
            let station_guard = self.station.lock().await;
            station_guard.as_ref().cloned()
        };

        let station_snapshot = if let Some(station) = station {
            station.get_snapshot(&peer_ids).await
        } else {
            crate::protocol::station::StationSnapshot::default()
        };

        let current_rx = self.rx_bytes_counter.load(Ordering::Relaxed);
        let current_tx = self.tx_bytes_counter.load(Ordering::Relaxed);
        let peer_rx_estimate = peers.first().map(|peer| peer.peer_tx_speed as f64).unwrap_or(0.0);
        let peer_path0_loss_pct = peers.first()
            .map(|peer| peer.peer_loss_rate as f64 / 10.0)
            .unwrap_or(0.0);
        let wagon_stats = get_wagon_stats();
        let wagon_sent_total = wagon_stats.sent_total.load(Ordering::Relaxed);
        let wagon_recv_total = wagon_stats.recv_total.load(Ordering::Relaxed);
        let wagon_drop_total = wagon_stats.dropped.load(Ordering::Relaxed);
        let wagon_checksum_failed_total = wagon_stats.checksum_failed.load(Ordering::Relaxed);
        let wagon_retrans_total = wagon_stats.retransmitted.load(Ordering::Relaxed);
        let wagon_drop_crc_pct = if wagon_recv_total + wagon_drop_total + wagon_checksum_failed_total > 0 {
            (wagon_drop_total + wagon_checksum_failed_total) as f64 * 100.0
                / (wagon_recv_total + wagon_drop_total + wagon_checksum_failed_total) as f64
        } else {
            0.0
        };

        let avg_rtt_ms = {
            let stream_ids = self.stream_list().await;
            if stream_ids.is_empty() {
                0
            } else {
                let mut total_rtt = 0u64;
                let mut count = 0u64;
                for stream_id in stream_ids {
                    if let Ok(stats) = self.stream_stats(stream_id).await {
                        total_rtt += stats.rtt_ms as u64;
                        count += 1;
                    }
                }
                if count > 0 { (total_rtt / count) as u32 } else { 0 }
            }
        };

        let mut cache = self.web_metrics_cache.lock().await;
        if cache.rx_bytes == 0 && cache.tx_bytes == 0 && cache.wagons_received == 0 {
            *cache = WebMetricsCache {
                sampled_at: Instant::now(),
                rx_bytes: current_rx,
                tx_bytes: current_tx,
                wagons_received: station_snapshot.total_wagons_received,
                clone_hits: station_snapshot.total_path0_loss_events,
                path0_loss_events: station_snapshot.total_path0_loss_events,
                evictions_total: station_snapshot.total_evicted_trains,
            };

            return WebTransportMetrics {
                peer_rx_estimate,
                avg_rtt_ms,
                peer_path0_loss_pct,
                active_trains: station_snapshot.active_trains,
                depot_bytes: station_snapshot.current_bytes,
                delivered_cache_size: station_snapshot.delivered_cache_size,
                evictions_total: station_snapshot.total_evicted_trains,
                timeout_total: station_snapshot.total_timeout_trains,
                cleanup_total: station_snapshot.total_completed_cleanups,
                total_wagons: station_snapshot.total_wagons_received,
                total_clone_hits: station_snapshot.total_path0_loss_events,
                wagon_sent_total,
                wagon_recv_total,
                wagon_drop_total,
                wagon_checksum_failed_total,
                wagon_retrans_total,
                wagon_drop_crc_pct,
                peer_count: peers.len(),
                ..Default::default()
            };
        }

        let now = Instant::now();
        let elapsed = now.duration_since(cache.sampled_at).as_secs_f64();

        let rx_delta = current_rx.saturating_sub(cache.rx_bytes);
        let tx_delta = current_tx.saturating_sub(cache.tx_bytes);
        let wagons_delta = station_snapshot.total_wagons_received.saturating_sub(cache.wagons_received);
        let clone_delta = station_snapshot.total_path0_loss_events.saturating_sub(cache.clone_hits);
        let path0_loss_delta = station_snapshot.total_path0_loss_events.saturating_sub(cache.path0_loss_events);
        let evictions_delta = station_snapshot.total_evicted_trains.saturating_sub(cache.evictions_total);

        let rx_speed = if elapsed > 0.0 { rx_delta as f64 / elapsed } else { 0.0 };
        let tx_speed = if elapsed > 0.0 { tx_delta as f64 / elapsed } else { 0.0 };
        let wagons_per_sec = if elapsed > 0.0 { wagons_delta as f64 / elapsed } else { 0.0 };
        let path0_loss_incoming_pct = if wagons_delta > 0 {
            path0_loss_delta as f64 * 100.0 / wagons_delta as f64
        } else {
            0.0
        };
        let clone_hit_pct = if wagons_delta > 0 {
            clone_delta as f64 * 100.0 / wagons_delta as f64
        } else {
            0.0
        };
        let clone_hit_rate = if elapsed > 0.0 { clone_delta as f64 / elapsed } else { 0.0 };

        *cache = WebMetricsCache {
            sampled_at: now,
            rx_bytes: current_rx,
            tx_bytes: current_tx,
            wagons_received: station_snapshot.total_wagons_received,
            clone_hits: station_snapshot.total_path0_loss_events,
            path0_loss_events: station_snapshot.total_path0_loss_events,
            evictions_total: station_snapshot.total_evicted_trains,
        };

        WebTransportMetrics {
            rx_speed,
            tx_speed,
            peer_rx_estimate,
            avg_rtt_ms,
            path0_loss_incoming_pct,
            peer_path0_loss_pct,
            clone_hit_pct,
            clone_hit_rate,
            wagons_per_sec,
            active_trains: station_snapshot.active_trains,
            depot_bytes: station_snapshot.current_bytes,
            delivered_cache_size: station_snapshot.delivered_cache_size,
            evictions_total: station_snapshot.total_evicted_trains,
            evictions_delta,
            timeout_total: station_snapshot.total_timeout_trains,
            cleanup_total: station_snapshot.total_completed_cleanups,
            total_wagons: station_snapshot.total_wagons_received,
            total_clone_hits: station_snapshot.total_path0_loss_events,
            wagon_sent_total,
            wagon_recv_total,
            wagon_drop_total,
            wagon_checksum_failed_total,
            wagon_retrans_total,
            wagon_drop_crc_pct,
            peer_count: peers.len(),
        }
    }

    /// Subscribe to Hello events
    ///
    /// Returns a receiver that gets all Hello events
    /// Send peer list to peer for network discovery

    pub async fn send_peer_list(&self, peer_id: HashId) -> Result<(), String> {

        let peers = self.get_peers().await;

        // Собираем список пиров (NodeID + адрес)

        let peer_list: Vec<(HashId, String)> = peers.iter()

            .map(|p| (p.id.clone(), p.addr.clone()))

            .collect();

        

        println!("[transport] 🔄 Sending peer list ({} peers) to {}", 

                 peer_list.len(), hex::encode(&peer_id.0[..8]));

        

        // Сериализуем список

        let data = bincode::serialize(&peer_list)

            .map_err(|e| format!("Failed to serialize peer list: {}", e))?;

        

        // Отправляем как пакет типа 0xE0

        let mut packet = vec![0xE0u8];

        packet.extend_from_slice(&data);

        

        self.send_encrypted(peer_id, &packet).await

    }

    /// Send DHT RPC Ping to peer

    pub async fn dht_rpc_ping(&self, peer_id: HashId) -> Result<(), String> {

        let packet = vec![0xF0u8];

        self.send_encrypted(peer_id, &packet).await

    }



    /// Send DHT RPC Pong to peer

    pub async fn dht_rpc_pong(&self, peer_id: HashId) -> Result<(), String> {

        let packet = vec![0xF1u8];

        self.send_encrypted(peer_id, &packet).await

    }



    /// Send DHT RPC FindNode request

    pub async fn dht_rpc_find_node(&self, peer_id: HashId, target: HashId) -> Result<Vec<(HashId, String)>, String> {
        use crate::dht::messages::{DhtQuery, DhtQueryType};
        use crate::dht::kademlia::K;
        
        let query = DhtQuery {
            request_id: 0,
            query_type: DhtQueryType::FindNode,
            key: target,
            value: None,
            limit: K as u8,
        };
        
        let response = self.send_dht_query(peer_id, &query).await?;
        Ok(response.nodes)
    }

    /// Send DHT RPC FindNode response

    pub async fn dht_rpc_find_node_response(&self, peer_id: HashId, nodes: Vec<(HashId, String)>) -> Result<(), String> {

        let mut packet = vec![0xF3u8];

        let data = serde_json::to_vec(&nodes)

            .map_err(|e| format!("Failed to serialize nodes: {}", e))?;

        packet.extend_from_slice(&data);

        self.send_encrypted(peer_id, &packet).await

    }



    /// Send DHT RPC Store request

    pub async fn dht_rpc_store(&self, peer_id: HashId, key: HashId, value: Vec<u8>) -> Result<(), String> {

        let mut packet = vec![0xF4u8];

        packet.extend_from_slice(key.as_ref());

        packet.extend_from_slice(&value);

        self.send_encrypted(peer_id, &packet).await

    }


    /// Send DHT RPC Store response

    pub async fn dht_rpc_store_response(&self, peer_id: HashId, success: bool) -> Result<(), String> {

        let mut packet = vec![0xF5u8];

        packet.push(if success { 1 } else { 0 });

        self.send_encrypted(peer_id, &packet).await

    }

    /// Send DHT RPC FindValue request

    pub async fn dht_rpc_find_value(&self, peer_id: HashId, key: HashId) -> Result<(), String> {

        let mut packet = vec![0xF6u8];

        packet.extend_from_slice(key.as_ref());

        self.send_encrypted(peer_id, &packet).await

    }

    /// Send DHT RPC FindValue response

    pub async fn dht_rpc_find_value_response(&self, peer_id: HashId, value: Option<Vec<u8>>, nodes: Vec<(HashId, String)>) -> Result<(), String> {

        let mut packet = vec![0xF7u8];

        let data = serde_json::to_vec(&(value, nodes))

            .map_err(|e| format!("Failed to serialize: {}", e))?;

        packet.extend_from_slice(&data);

        self.send_encrypted(peer_id, &packet).await

    }

    pub fn subscribe_hello(&self) -> tokio::sync::broadcast::Receiver<HelloEvent> {
        self.hello_tx.subscribe()
    }

    /// Get the internal Hello event sender (for advanced usage)
    pub fn hello_sender(&self) -> tokio::sync::broadcast::Sender<HelloEvent> {
        self.hello_tx.clone()
    }

    /// Take the proxy response sender (returns None if already taken or not set)
    /// This is used by HttpProxyClient to connect to the response channel
    pub async fn take_proxy_response_channel(&self) -> Option<tokio::sync::mpsc::Sender<(HashId, crate::proxy::ProxyResponse)>> {
        // This doesn't make sense - we need a RECEIVER in the client, not sender
        // Let's think differently...
        None // Placeholder
    }

    /// Set proxy response channel (for HttpProxyClient)
    pub fn set_proxy_response_channel(&self, tx: tokio::sync::mpsc::Sender<(HashId, crate::proxy::ProxyResponse)>) {
        // We need to modify proxy_response_tx but it's in Arc<Mutex<...>>
        // This won't work directly...
        // TODO: Need to refactor proxy_response_tx to be Arc<Mutex<Option<...>>>
    }

    /// 🔄 Set NACK channel (for Wagon Retransmission)
    pub fn set_nack_channel(&self, tx: tokio::sync::mpsc::Sender<(HashId, crate::protocol::WagonNack)>) {
        // We need to modify nack_tx but it's in Arc<Mutex<...>>
        // TODO: Need to refactor nack_tx to be Arc<Mutex<Option<...>>>
    }

    /// Set YTP Station
    pub async fn set_station(&self, station: Arc<crate::protocol::Station>) {
        let mut station_lock = self.station.lock().await;
        *station_lock = Some(station);
        println!("[transport] 🚂 Station registered");
    }

    /// Get peer info
    pub async fn get_peer(&self, peer_id: HashId) -> Option<PeerInfo> {
        let peers = self.peers.lock().await;
        peers.get(&peer_id).cloned()
    }

    /// Get all peers
    /// Сколько приветствий отклонил допуск (ограничения на наплыв) с запуска.
    pub async fn hellos_refused(&self) -> u64 {
        self.admission.lock().await.rejected_total()
    }

    /// Сколько приветствий пропустил допуск.
    pub async fn hellos_admitted(&self) -> u64 {
        self.admission.lock().await.allowed_total()
    }

    pub async fn get_peers(&self) -> Vec<PeerInfo> {
        let peers = self.peers.lock().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let timeout_ms = (crate::netlayer::peer::PEER_ONLINE_TIMEOUT_SECS * 1000) as u128;
        peers.values()
            .filter(|p| now.saturating_sub(p.last_seen) <= timeout_ms)
            .cloned()
            .collect()
    }

    /// Get peers as HashMap (for synchronization with P2P transport)
    pub async fn get_peers_map(&self) -> std::collections::HashMap<HashId, PeerInfo> {
        let peers = self.peers.lock().await;
        peers.clone()
    }

    /// 🚇 Integration Step 1: единая точка обработки декриптнутых wagon'ов.
    ///
    /// Вызывается из:
    /// - `data_listener` после `decrypt_by_peer_id` (UDP-канал)
    /// - WS-server pump (`anchor` принимает от mobile)
    /// - WS-client pump (`mobile` принимает от anchor)
    ///
    /// Возвращает `true` если пакет обработан (вызывающий должен `continue` без
    /// дальнейшего match'инга). `false` — пакет не относится к моим integration-веткам,
    /// caller продолжает legacy-обработку (chat, proxy, heartbeat и т.п.).
    ///
    /// Step 1 placeholder. Step 3 — наполнение circuit-веткой. Возвращает true если
    /// пакет обработан integration-логикой (caller skip'ает legacy match).
    pub(crate) async fn dispatch_decrypted_wagon(
        self: &Arc<Self>,
        sender: HashId,
        plaintext: &[u8],
        source: WagonSource,
    ) -> bool {
        if plaintext.is_empty() {
            return false;
        }
        match plaintext[0] {
            // 🔐 Step 4: RESUME (0xC0). Mobile запрашивает восстановление session по session_id+HMAC.
            crate::netlayer::pairing::PKT_RESUME => {
                let _ = source;
                self.handle_resume_packet(sender, plaintext).await;
                true
            }
            // 🔐 Step 4: RESUME_ACK (0xC1). Mobile получает ответ от anchor'а.
            crate::netlayer::pairing::PKT_RESUME_ACK => {
                let _ = source;
                self.handle_resume_ack(sender, plaintext).await;
                true
            }
            // 🆕 Hardening Step 3: SESSION_ISSUE (0xC2). Mobile получает session_token
            // от anchor'а после успешного pair'инга. Сохраняем в PairedAnchorStore.
            crate::netlayer::pairing::PKT_SESSION_ISSUE => {
                let _ = source;
                self.handle_session_issue(sender, plaintext).await;
                true
            }
            // Цепочки через несколько узлов (0xD9..0xDC): построение, ячейки, закрытие.
            crate::hops::PKT_CREATE | crate::hops::PKT_CREATED | crate::hops::PKT_CELL | crate::hops::PKT_DESTROY => {
                let _ = source;
                crate::hops_net::on_packet(&sender.0, plaintext);
                true
            }
            // Записи «узел доступен через ретрансляторы» (0xDD).
            crate::relay_net::PKT_VIA => {
                let _ = source;
                crate::relay_net::on_packet(&sender.0, plaintext);
                true
            }
            // Обмен карточками узлов (0xD8): списки сети по странам.
            crate::network_offers::PKT_OFFERS => {
                let _ = source;
                crate::network_offers::on_packet(&sender.0, plaintext);
                true
            }
            _ => false,
        }
    }

    /// Hardening Step 3 — mobile-сторона обработки 0xC2 SESSION_ISSUE.
    /// Anchor прислал нам SessionToken (после QR-pair'инга). Сохраняем в store.
    async fn handle_session_issue(self: &Arc<Self>, sender: HashId, plaintext: &[u8]) {
        use crate::netlayer::pairing::decode_session_issue;
        let (tok, _sk) = match decode_session_issue(plaintext) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[session-issue] decode failed from {}: {}",
                          hex::encode(&sender.0[..8]), e);
                return;
            }
        };
        let mut store = self.paired_anchors.lock().await;
        // Anchor должен уже быть в store (mobile делал pair-import заранее).
        // Если нет — добавим минимальный entry.
        let exists = store.anchors.iter().any(|e| e.payload.anchor_id == sender);
        if !exists {
            // Создаём placeholder с anchor_id и пустыми остальными полями. URL и fingerprint
            // mobile получит позже (через QR-import или /pair/qr).
            use crate::netlayer::pairing::{PairedAnchorEntry, PairingPayload};
            store.anchors.push(PairedAnchorEntry {
                payload: PairingPayload {
                    anchor_id: sender,
                    anchor_x25519_hex: String::new(),
                    fingerprint_hex: String::new(),
                    anchor_url: String::new(),
                },
                session: None,
                preference: u32::MAX,
            });
        }
        let updated = store.set_session(&sender, tok.clone());
        let path = crate::netlayer::pairing::default_paired_anchors_path();
        if let Err(e) = store.save(&path) {
            eprintln!("[session-issue] persist failed: {}", e);
        }
        println!("[session-issue] ✅ session {:#x} from {} saved (existed={})",
                 tok.session_id, hex::encode(&sender.0[..8]), updated);
    }

    /// Step 4 / Hardening Step 2 — anchor-сторона обработки 0xC0.
    /// Lookup session_id во всех paired-токенах (ключ store'а — pubkey_hex клиента),
    /// verify HMAC, refresh TTL, восстанавливаем session-key в EncryptionManager
    /// (если он был сохранён в SessionToken на момент pair'инга), затем отвечаем
    /// 0xC1 RESUME_ACK encrypted (status=Ok). Если session-key не сохранён —
    /// шлём ACK plaintext (legacy-токен из старого pair'инга), mobile может его
    /// прочитать без сессии.
    async fn handle_resume_packet(self: &Arc<Self>, sender: HashId, plaintext: &[u8]) {
        use crate::netlayer::pairing::{
            decode_resume, encode_resume_ack, verify_resume_mac, ResumeStatus,
            DEFAULT_SESSION_TTL_SECS,
        };
        let (embedded_node_id, session_id, addr, mac) = match decode_resume(plaintext) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[resume] decode failed: {}", e);
                return;
            }
        };
        // Если sender ещё не известен (plaintext pre-Hello flow в WS-server'е), берём
        // node_id из payload. Иначе ожидаем что embedded совпадает с sender'ом — иначе reject.
        let resolved_sender = if sender == HashId([0u8; 32]) {
            embedded_node_id
        } else {
            if sender != embedded_node_id {
                eprintln!("[resume] sender mismatch: ws={}, packet={}",
                          hex::encode(&sender.0[..8]),
                          hex::encode(&embedded_node_id.0[..8]));
                return;
            }
            sender
        };
        let sender = resolved_sender;
        let mut status = ResumeStatus::Unknown;
        let mut hit_pubkey: Option<String> = None;
        let mut saved_session_key: Option<[u8; 32]> = None;
        {
            let store = self.paired_clients.lock().await;
            for (pk, tok) in store.clients.iter() {
                if tok.session_id == session_id {
                    if tok.is_expired() {
                        status = ResumeStatus::Expired;
                        hit_pubkey = Some(pk.clone());
                        break;
                    }
                    let secret = match tok.resume_secret() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if verify_resume_mac(&secret, session_id, &addr, &mac) {
                        status = ResumeStatus::Ok;
                        hit_pubkey = Some(pk.clone());
                        saved_session_key = tok.session_key();
                    } else {
                        status = ResumeStatus::BadMac;
                        hit_pubkey = Some(pk.clone());
                    }
                    break;
                }
            }
        }
        match (status, hit_pubkey.clone()) {
            (ResumeStatus::Ok, Some(pk)) => {
                let path = crate::netlayer::pairing::default_paired_clients_path();
                let mut store = self.paired_clients.lock().await;
                store.refresh(&pk, DEFAULT_SESSION_TTL_SECS);
                if let Err(e) = store.save(&path) {
                    eprintln!("[resume] persist failed: {}", e);
                }
                println!("[resume] ✅ session {:#x} from {} ({}): verified, TTL refreshed",
                         session_id, hex::encode(&sender.0[..8]), addr);
            }
            (ResumeStatus::BadMac, _) => {
                eprintln!("[resume] ❌ session {:#x} from {}: BadMac",
                          session_id, hex::encode(&sender.0[..8]));
            }
            (ResumeStatus::Expired, _) => {
                eprintln!("[resume] ⌛ session {:#x} from {}: expired",
                          session_id, hex::encode(&sender.0[..8]));
            }
            (ResumeStatus::Unknown, _) => {
                eprintln!("[resume] ❓ session {:#x} from {}: unknown",
                          session_id, hex::encode(&sender.0[..8]));
            }
            _ => {}
        }

        // 🔐 Hardening Step 2: 0xC1 RESUME_ACK через encrypted-channel когда session-key
        // resume есть, иначе — plaintext (legacy fallback для старых токенов без session-key).
        let ack_bytes = encode_resume_ack(status, None);
        if let Some(session_key) = saved_session_key {
            if matches!(status, ResumeStatus::Ok) {
                let mut enc = self.encryption.lock().await;
                let v = enc.restore_session(sender, session_key);
                drop(enc);
                println!("[resume] 🔁 session-key restored v{} for {}",
                         v, hex::encode(&sender.0[..8]));
                if let Err(e) = self.send_encrypted(sender, &ack_bytes).await {
                    eprintln!("[resume-ack] encrypted send failed: {}; fallback plaintext", e);
                    let _ = self.send_plaintext_best_effort(sender, &ack_bytes).await;
                } else {
                    println!("[resume-ack] 🔒 0xC1 sent encrypted to {}",
                             hex::encode(&sender.0[..8]));
                }
                return;
            }
        }
        // Fallback: send plaintext ACK. Mobile должна уметь это переварить (см. handle_resume_ack).
        let _ = self.send_plaintext_best_effort(sender, &ack_bytes).await;
    }

    /// Hardening Step 2: лучший доступный канал доставки plaintext-пакета
    /// (WS-outgoing если есть, иначе UDP data-socket с last-known address).
    /// Используется только для resume-ACK fallback'а; основной путь — encrypted.
    async fn send_plaintext_best_effort(self: &Arc<Self>, peer_id: HashId, bytes: &[u8]) -> Result<(), String> {
        if let Some(out) = self.ws_outgoing.lock().await.get(&peer_id).cloned() {
            out.send(bytes.to_vec()).await
                .map_err(|e| format!("ws send: {}", e))?;
            return Ok(());
        }
        let dest = {
            let peers = self.peers.lock().await;
            let p = peers.get(&peer_id)
                .ok_or_else(|| format!("peer {} not in table", hex::encode(&peer_id.0[..8])))?;
            p.data_addr.clone().unwrap_or_else(|| p.addr.clone())
        };
        self.socket_manager.data().await.send_to(bytes, &dest).await
            .map_err(|e| format!("udp send to {}: {}", dest, e))?;
        Ok(())
    }

    /// Step 4 — mobile-сторона обработки 0xC1. Если status Ok и есть new_secret — обновить
    /// `PairedAnchorStore`. На текущем шаге — только лог; сам store на mobile-стороне
    /// подключим в Step 6 (auto-reconnect).
    async fn handle_resume_ack(self: &Arc<Self>, sender: HashId, plaintext: &[u8]) {
        use crate::netlayer::pairing::decode_resume_ack;
        match decode_resume_ack(plaintext) {
            Ok((status, secret)) => {
                println!("[resume-ack] from {}: status={:?}, new_secret={}",
                         hex::encode(&sender.0[..8]),
                         status,
                         secret.is_some());
                // TODO Step 6: обновить PairedAnchorStore::set_session с новым secret.
            }
            Err(e) => {
                eprintln!("[resume-ack] decode failed: {}", e);
            }
        }
    }

    /// 🆕 Hardening Step 6: выбрать exit-кандидата с учётом `--exit-jurisdiction`.
    /// Если флаг задан — возвращает anchor'ов в этой стране (preference из jurisdiction_index
    /// и peer-table). Если не задан — возвращает все anchor'ы (sort by last_seen DESC).
    /// Caller (circuit builder) берёт первого как exit-hop.
    pub async fn pick_exit_candidates(&self, prefer_country: Option<&str>) -> Vec<PeerInfo> {
        let want = prefer_country
            .map(|c| c.to_string())
            .or_else(crate::netlayer::packet::exit_jurisdiction);
        match want.as_deref() {
            Some(c) if !c.is_empty() => self.find_anchors_by_jurisdiction(c).await,
            _ => self.find_anchors_by_jurisdiction("").await,
        }
    }

    /// 🌍 Iter 3 / Hardening Step 5: Найти anchor-узлы (caps & ANCHOR != 0) с заданной jurisdiction.
    /// `country` — ISO-3166 alpha-2 (case-insensitive). Возвращает peer'ов отсортированных по
    /// last_seen DESC. Если `country` пуст — игнорирует фильтр и возвращает всех anchor'ов.
    ///
    /// Step 5: при недостатке локальных результатов (<3) дополняет из
    /// `jurisdiction_index` (root-level индекс, обновляется при приёме Hello-ов).
    /// Index-entry'и оборачиваются в синтетический PeerInfo (без caps), чтобы caller
    /// мог хотя бы достучаться по addr'у.
    pub async fn find_anchors_by_jurisdiction(&self, country: &str) -> Vec<PeerInfo> {
        use crate::netlayer::packet::hello_caps::ANCHOR;
        let needle = country.to_uppercase();
        let mut out: Vec<PeerInfo> = self.peers.lock().await.values()
            .filter(|p| (p.caps_bits & ANCHOR) != 0)
            .filter(|p| {
                if needle.is_empty() {
                    return true;
                }
                p.jurisdiction
                    .as_deref()
                    .map(|j| j.eq_ignore_ascii_case(&needle))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));

        // Step 5: добиваем из jurisdiction_index если результатов мало.
        if !needle.is_empty() && out.len() < 3 {
            let already: std::collections::HashSet<HashId> = out.iter().map(|p| p.id).collect();
            for entry in self.jurisdiction_index.lookup(&needle) {
                if already.contains(&entry.node_id) { continue; }
                let mut synthetic = PeerInfo::new(entry.node_id, &entry.addr);
                synthetic.caps_bits = ANCHOR; // index хранит только anchor'ов
                synthetic.jurisdiction = Some(needle.clone());
                out.push(synthetic);
                if out.len() >= 8 { break; }
            }
        }
        out
    }

    /// Set external IP address (for NAT traversal)
    pub async fn set_external_ip(&self, ip: Option<String>) {
        *self.external_ip.write().await = ip;
    }

    /// Set network topology (for NAT traversal)
    pub fn set_topology(&mut self, topology: Option<NetworkTopology>) {
        self.topology = topology;
    }


    /// Find peer by short ID (first N bytes of node ID in hex)
    /// Accepts 4, 6, or 8 bytes (8, 12, or 16 hex characters)
    pub fn find_peer_by_short_id(&self, short_id: &str) -> Option<HashId> {
        use tokio::task::block_in_place;

        // This is a sync method but needs async access to peers
        // In real usage, this should be made async or restructured
        // For now, we'll use block_in_place as a workaround
        block_in_place(|| {
            let rt = tokio::runtime::Handle::try_current();
            if rt.is_err() {
                return None;
            }

            // Convert short_id hex to bytes
            let short_bytes = match hex::decode(short_id) {
                Ok(bytes) => bytes,
                Err(_) => return None,
            };

            // Accept 4, 6, or 8 bytes (8, 12, or 16 hex chars)
            if short_bytes.len() != 4 && short_bytes.len() != 6 && short_bytes.len() != 8 {
                eprintln!("   ⚠️  Invalid SHORT_ID length: {} bytes (expected 4, 6, or 8)", short_bytes.len());
                return None;
            }

            // Search for peer with matching short_id
            let peers_future = self.peers.lock();
            let peers = rt.unwrap().block_on(peers_future);

            for (peer_id, _peer) in peers.iter() {
                // Compare first N bytes based on input length
                if peer_id.0[..short_bytes.len()] == short_bytes[..] {
                    return Some(peer_id.clone());
                }
            }

            None
        })
    }

    /// Get node identity
    pub fn identity(&self) -> &Arc<NodeIdentity> {
        &self.identity
    }

    // ========== Two-Phase Peer Verification (Stage 1.3) ==========
    // The instance-method version (verify_peer_phase1/phase2/verify_peer_handshake)
    // was superseded by verify_peer_handshake_static below and had no callers
    // left anywhere in the codebase — removed as dead code.

    /// Static version: Full handshake verification (can be called without &self)
    pub fn verify_peer_handshake_static(hello_packet: &HelloPacket) -> Result<(), String> {
        if !hello_packet.verify_node_name() {
            return Err("Phase 1 FAILED: node_name mismatch".to_string());
        }
        println!("[transport] ✅ Phase 1: Self-certifying identity verified");

        // the node id must be bound to the signing key (or be a legacy id before the sunset)
        let now_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if !crate::util::types::id_acceptable(&hello_packet.node_id.0, &hello_packet.public_key, now_s) {
            return Err("Phase 1 FAILED: node_id is not bound to the signing key".to_string());
        }

        let challenge = hello_packet.challenge_data();
        if !NodeIdentity::verify_node(&hello_packet.node_name, &hello_packet.public_key, &hello_packet.signature.0, &challenge) {
            return Err("Phase 2 FAILED: Invalid signature".to_string());
        }
        
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let time_diff = if now > hello_packet.timestamp { now - hello_packet.timestamp } else { hello_packet.timestamp - now };

        // Reverted to the original symmetric 5-minute window (was briefly
        // tightened to 30s for future-dated timestamps during dead-code
        // cleanup). That tightening was never requested and risked
        // rejecting legitimate handshakes between machines with real
        // clock drift — a live regression risk with no real security
        // upside, since Barrier 3's nonce cache (check_replay) is what
        // actually stops replay now, not this coarse timestamp window.
        if time_diff > 5 * 60 {
            return Err(format!("Timestamp too old: diff={}s", time_diff));
        }
        
        println!("[transport] ✅ Phase 2: Signature verified");
        println!("[transport] 🎉 Handshake verified for peer {}", hex::encode(&hello_packet.node_name.0[..8]));
        Ok(())
    }

    /// Get Station (для TcpTunnelExitHandler)
    pub async fn get_station(&self) -> Arc<crate::protocol::Station> {
        self.station.lock().await
            .as_ref()
            .unwrap()
            .clone()
    }

    /// Get local discovery address
    pub fn discovery_addr(&self) -> SocketAddr {
        self.discovery_socket.local_addr()
            .unwrap_or_else(|_| format!("0.0.0.0:{}", DEFAULT_DISCOVERY_PORT).parse().unwrap())
    }

    /// Get local data address
    pub fn data_addr(&self) -> SocketAddr {
        self.data_recv_socket.local_addr()
            .unwrap_or_else(|_| format!("0.0.0.0:{}", DEFAULT_DATA_PORT).parse().unwrap())
    }

    /// Get current active discovery address (rotated port if any)
    pub fn active_discovery_addr(&self) -> SocketAddr {
        let state = self.port_manager.current_state();
        format!("0.0.0.0:{}", state.discovery_port)
            .parse()
            .unwrap_or_else(|_| self.discovery_addr())
    }

    /// Get current active data address (rotated port if any)
    pub fn active_data_addr(&self) -> SocketAddr {
        let state = self.port_manager.current_state();
        format!("0.0.0.0:{}", state.data_port)
            .parse()
            .unwrap_or_else(|_| self.data_addr())
    }

    /// Get TUN wagon tx channel clone (for sending TunWagons to exit node)
    pub fn tun_wagon_tx(&self) -> Option<mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagon)>> {
        self.tun_wagon_tx.clone()
    }

    /// Set TUN wagon tx channel (called from main.rs)
    pub fn set_tun_wagon_channel(&mut self, tx: mpsc::Sender<(HashId, crate::netlayer::tun_exit::TunWagon)>) {
        self.tun_wagon_tx = Some(tx);
    }

    /// Node Identity Binding Fix: load the owner's pinned node_id -> signing
    /// pubkey bindings (from `trusted_peers.json` via `PeerDirectory`).
    /// A Hello later claiming one of these node_ids with any other key is
    /// rejected before it can touch PeerInfo/DHT/online state. Peers not in
    /// this map keep the existing, unchanged discovery behavior.
    pub async fn set_pinned_identities(&self, pins: HashMap<HashId, [u8; 32]>) {
        println!("[transport] 🔒 Loaded {} pinned node identity binding(s)", pins.len());
        *self.pinned_identities.write().await = pins;
    }

    /// Bootstrap - connect to all nodes in the list
    pub async fn bootstrap(&self, bootstrap_addrs: Vec<String>, external_ip: Option<String>) -> Result<(), String> {
        println!("[bootstrap] 🚀 Starting bootstrap with {} nodes", bootstrap_addrs.len());

        if external_ip.is_some() {
            println!("[bootstrap] 🌐 External IP detected: will filter own address");
        }

        if bootstrap_addrs.is_empty() {
            println!("[bootstrap] ℹ️  No bootstrap nodes configured, waiting for incoming connections");
            return Ok(());
        }

        let mut connected = 0;
        let mut failed = 0;
        let mut skipped = 0;

        for addr in bootstrap_addrs {
            // Extract IP from address (format: IP:PORT)
            let peer_ip = addr.split(':').next().unwrap_or(&addr);

            // Skip own address (localhost)
            if addr.contains("127.0.0.1") || addr.contains("localhost") {
                println!("[bootstrap] ⏭️  Skipping localhost: {}", addr);
                skipped += 1;
                continue;
            }

            // Skip own external IP
            if let Some(ref ext_ip) = external_ip {
                if peer_ip == ext_ip {
                    println!("[bootstrap] ⏭️  Skipping own external IP: {}", addr);
                    skipped += 1;
                    continue;
                }
            }

            println!("[bootstrap] 📡 Connecting to {}...", addr);

            match self.send_hello_request(&addr).await {
                Ok(_) => {
                    println!("[bootstrap] ✅ Hello sent to {}", addr);
                    connected += 1;
                }
                Err(e) => {
                    println!("[bootstrap] ❌ Failed to connect to {}: {}", addr, e);
                    failed += 1;
                }
            }

            // Small delay between connections
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        println!("[bootstrap] 📊 Bootstrap complete: {} connected, {} failed, {} skipped",
                 connected, failed, skipped);

        Ok(())
    }

    // ========== DHT Integration ==========

    /// Find closest peers in DHT to target node ID
    pub async fn dht_find_closest(&self, target: HashId, count: usize) -> Vec<(HashId, String)> {
        let dht = self.dht.lock().await;
        dht.closest_n(&target, count)
    }

    /// Find specific peer by node ID in DHT
    /// Find specific peer by node ID in DHT (iterative network lookup)
    /// Find specific peer by node ID in DHT (iterative network lookup)
    pub async fn dht_find_node(&self, node_id: HashId) -> Option<String> {
        use std::collections::HashSet;
        use crate::dht::bucket::xor_distance;
        use crate::dht::kademlia::{ALPHA, K};
        
        // Start with K closest peers from local table
        let dht = self.dht.lock().await;
        let mut shortlist: Vec<(HashId, String)> = dht.closest_n(&node_id, K);
        drop(dht);
        
        let mut queried = HashSet::new();
        let mut closest = shortlist.clone();
        
        loop {
            // Select ALPHA unqueried nodes
            let to_query: Vec<(HashId, String)> = shortlist
                .iter()
                .filter(|(id, _)| !queried.contains(id))
                .take(ALPHA)
                .cloned()
                .collect();
            
            if to_query.is_empty() {
                break;
            }
            
            // Mark as queried
            for (id, _) in &to_query {
                queried.insert(id.clone());
            }
            
            // Query peers in parallel
            let mut found_nodes = Vec::new();
            for (peer_id, _) in to_query {
                // Send FIND_NODE RPC and get response
                match self.dht_rpc_find_node(peer_id.clone(), node_id.clone()).await {
                    Ok(nodes) => {
                        found_nodes.extend(nodes);
                        // Record success for adaptive alpha
                        let mut dht = self.dht.lock().await;
                        dht.record_rpc_success();
                    }
                    Err(e) => {
                        eprintln!("[transport] DHT find_node failed: {}", e);
                        // Record timeout/failure for adaptive alpha
                        let mut dht = self.dht.lock().await;
                        dht.record_rpc_timeout();
                    }
                }
            }
            
            // Merge results
            for node in found_nodes {
                if !shortlist.iter().any(|(id, _)| id == &node.0) {
                    shortlist.push(node);
                }
            }
            
            // Sort by XOR distance
            shortlist.sort_by(|(id_a, _), (id_b, _)| {
                xor_distance(id_a, &node_id).cmp(&xor_distance(id_b, &node_id))
            });
            shortlist.truncate(K);
            
            // Check convergence
            if shortlist.first().map(|(id, _)| id) == closest.first().map(|(id, _)| id) {
                break;
            }
            closest = shortlist.clone();
        }
        
        // Return address of found node if exists
        shortlist.into_iter().find(|(id, _)| id == &node_id).map(|(_, addr)| addr)
    }
    pub async fn dht_store(&self, key: HashId, value: Vec<u8>) {
        let mut dht = self.dht.lock().await;
        dht.store_value(key, value);
    }

    /// Retrieve value from local DHT storage
    pub async fn dht_get(&self, key: HashId) -> Option<Vec<u8>> {
        let mut dht = self.dht.lock().await;
        dht.get_value(&key)
    }

    /// Get DHT statistics
    pub async fn dht_stats(&self) -> (usize, usize) {
        let dht = self.dht.lock().await;
        let peer_count = dht.ktable.peer_count();
        let storage_count = dht.get_storage_stats();
        (peer_count, storage_count)
    }

    /// Cleanup expired DHT entries
    pub async fn dht_cleanup(&self) {
        let mut dht = self.dht.lock().await;
        dht.cleanup_storage();
        dht.ktable.cleanup_inactive_peers();
    }

    // ========== Stream Layer API ==========

    /// Open new reliable stream to peer
    pub async fn stream_open(&self, peer_id: HashId) -> Result<u32, String> {
        // Check peer exists
        let peers = self.peers.lock().await;
        if !peers.contains_key(&peer_id) {
            return Err(format!("Unknown peer: {}", hex::encode(&peer_id.0[..8])));
        }
        drop(peers);

        // Create stream with limits
        let stream_id = {
            let mut streams = self.streams.lock().await;
            streams.create_stream(peer_id)?
        };

        // Initiate connection (send SYN immediately)
        let syn_frame = {
            let mut streams = self.streams.lock().await;
            if let Some(stream) = streams.get_stream_mut(stream_id) {
                stream.connect()
            } else {
                return Err(format!("Stream {} not found after creation", stream_id));
            }
        };

        // Send SYN immediately
        let peer_info = {
            let peers_lock = self.peers.lock().await;
            peers_lock.get(&peer_id).cloned()
        };

        if let Some(peer) = peer_info {
            let frame_bytes = syn_frame.to_bytes();
            let encrypted = {
                let enc_lock = self.encryption.lock().await;
                enc_lock.encrypt(&peer, &frame_bytes)
            };

            if let Ok(data) = encrypted {
                let addr = peer.data_addr.as_ref().unwrap_or(&peer.addr);
                if let Err(e) = self.socket_manager.data().await.send_to(&data, addr).await {
                    return Err(format!("Failed to send SYN: {}", e));
                }
                println!("[transport] 📤 Sent SYN for stream {} to {}", stream_id, addr);
            }
        }

        println!("[transport] 🌊 Opening stream {} to peer {}",
                 stream_id, hex::encode(&peer_id.0[..8]));

        // Wait for stream to be established (timeout 10 seconds)
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(10);

        loop {
            {
                let streams = self.streams.lock().await;
                if let Some(stream) = streams.get_stream(stream_id) {
                    if stream.state == crate::dataplane::stream::StreamState::Established {
                        println!("[transport] ✅ Stream {} established", stream_id);
                        return Ok(stream_id);
                    }
                }
            }

            if start.elapsed() > timeout {
                return Err(format!("Stream {} connection timeout", stream_id));
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Write data to stream
    pub async fn stream_write(&self, stream_id: u32, data: &[u8]) -> Result<usize, String> {
        eprintln!("[transport] 📝 stream_write(): stream_id={}, data_len={}", stream_id, data.len());

        let mut streams = self.streams.lock().await;

        if let Some(stream) = streams.get_stream_mut(stream_id) {
            let result = stream.write(data);
            eprintln!("[transport] 📝 stream_write() result: {:?}", result);
            result
        } else {
            eprintln!("[transport] ❌ stream_write(): Stream {} not found", stream_id);
            Err(format!("Stream {} not found", stream_id))
        }
    }

    /// Read data from stream
    pub async fn stream_read(&self, stream_id: u32, buf: &mut [u8]) -> Result<usize, String> {
        let mut streams = self.streams.lock().await;

        if let Some(stream) = streams.get_stream_mut(stream_id) {
            let n = stream.read(buf);

            // SPAM REDUCED: only log when we actually read data
            if n > 0 {
                eprintln!("[transport] 📖 stream_read(): stream_id={}, read={}", stream_id, n);
            }

            Ok(n)
        } else {
            eprintln!("[transport] ❌ stream_read(): Stream {} not found", stream_id);
            Err(format!("Stream {} not found", stream_id))
        }
    }

    /// Close stream gracefully
    pub async fn stream_close(&self, stream_id: u32) -> Result<(), String> {
        let mut streams = self.streams.lock().await;

        if let Some(stream) = streams.get_stream_mut(stream_id) {
            if let Some(fin_frame) = stream.close() {
                // FIN will be sent by stream_manager_task
                println!("[transport] 🌊 Closing stream {}", stream_id);
            }
            Ok(())
        } else {
            Err(format!("Stream {} not found", stream_id))
        }
    }

    /// Get stream statistics
    pub async fn stream_stats(&self, stream_id: u32) -> Result<StreamStats, String> {
        let streams = self.streams.lock().await;

        if let Some(stream) = streams.get_stream(stream_id) {
            Ok(StreamStats {
                stream_id,
                state: format!("{:?}", stream.state),
                send_seq: stream.send_seq,
                recv_seq: stream.recv_seq,
                unacked: stream.unacked.len(),
                rtt_ms: stream.rtt_ms,
                available: stream.available(),
            })
        } else {
            Err(format!("Stream {} not found", stream_id))
        }
    }

    /// Get all stream IDs
    pub async fn stream_list(&self) -> Vec<u32> {
        let streams = self.streams.lock().await;
        streams.all_stream_ids()
    }

    /// Get stream state
    pub async fn stream_state(&self, stream_id: u32) -> Option<crate::dataplane::stream::StreamState> {
        let streams = self.streams.lock().await;
        streams.get_stream(stream_id).map(|s| s.state.clone())
    }

    /// Get stream peer ID
    pub async fn stream_peer_id(&self, stream_id: u32) -> Result<Option<HashId>, String> {
        let streams = self.streams.lock().await;
        Ok(streams.get_stream(stream_id).map(|s| s.peer_id))
    }

    /// Cleanup stale encryption sessions

    /// Send DHT query to a peer and wait for response

    /// Send DHT query to a peer and wait for response
    pub async fn send_dht_query(&self, peer_id: HashId, query: &crate::dht::messages::DhtQuery) -> Result<crate::dht::messages::DhtResponse, String> {
        use crate::dht::messages::DhtResponse;
        
        let request_id = self.dht.lock().await.next_request_id();
        
        // Create query with request_id
        let mut query_with_id = query.clone();
        query_with_id.request_id = request_id;
        
        // Register pending response
        let rx = self.dht.lock().await.register_pending(request_id);
        
        // Serialize query to binary
        let data = query_with_id.to_bytes();
        
        // Send encrypted
        self.send_encrypted(peer_id, &data).await?;
        
        // Wait for response with timeout
        match tokio::time::timeout(tokio::time::Duration::from_secs(5), rx).await {
            Ok(Ok(data)) => {
                // Deserialize response
                match DhtResponse::from_bytes(&data) {
                    Some(resp) => Ok(resp),
                    None => Err("Invalid response format".to_string()),
                }
            }
            Ok(Err(_)) => Err("Response channel closed".to_string()),
            Err(_) => Err("Request timeout".to_string()),
        }
    }
    pub async fn cleanup_sessions(&self) {
        self.encryption.lock().await.cleanup_stale_sessions();
    }
    /// Update control plane (State Manager)

    pub async fn update_control_plane(&self) {

        let telemetry = self.collect_telemetry().await;

        let mut state_manager = self.state_manager.lock().await;

        if let Some(new_state) = state_manager.update(telemetry) {

            tracing::info!("[state] Transport state changed to: {:?}", new_state);

            let actions = crate::state_manager::PolicyEngine::get_actions(new_state);

            self.apply_actions(actions).await;

        }

    }

    

    async fn collect_telemetry(&self) -> crate::state_manager::Telemetry {
        let mut telemetry = crate::state_manager::Telemetry::default();
        
        // Получаем активных пиров
        let active_peers = self.get_peers().await;
        
        if active_peers.is_empty() {
            return telemetry;
        }
        
        // Получаем статистику из Station
        let station_guard = self.station.lock().await;
        if let Some(station) = station_guard.as_ref() {
            let mut total_wagons = 0u64;
            let mut total_loss_events = 0u64;
            let mut peer_count = 0;
            
            for peer in &active_peers {
                let metrics = station.get_peer_metrics(peer.id).await;
                if metrics.total_wagons_received > 0 {
                    total_wagons += metrics.total_wagons_received;
                    total_loss_events += metrics.total_path0_loss_events;
                    peer_count += 1;
                }
            }
            
            // Вычисляем loss_rate (потери на Path0)
            if total_wagons > 0 {
                telemetry.loss_rate = total_loss_events as f64 / total_wagons as f64;
            }
            telemetry.peer_loss_rate = active_peers.first().map(|p| p.peer_loss_rate as f64 / 1000.0).unwrap_or(0.0);
            
            
            // tracing::debug!("[telemetry] loss_rate={:.2}%, peers={}", 
                           //telemetry.loss_rate * 100.0, peer_count);
        }
        
        telemetry
    }

    

    async fn apply_actions(&self, actions: Vec<crate::state_manager::Action>) {

        for action in actions {

            match action {

                crate::state_manager::Action::EnableDualPath => {

                    tracing::debug!("[state] Enabling DUAL-PATH");

                }

                crate::state_manager::Action::DisableDualPath => {

                    tracing::debug!("[state] Disabling DUAL-PATH");

                }

                crate::state_manager::Action::IncreasePadding(size) => {

                    tracing::debug!("[state] Increasing padding to {} bytes", size);

                }

                crate::state_manager::Action::ResetPadding => {

                    tracing::debug!("[state] Resetting padding");

                }

                crate::state_manager::Action::EnableJitter => {

                    tracing::debug!("[state] Enabling jitter");

                }

                crate::state_manager::Action::DisableJitter => {

                    tracing::debug!("[state] Disabling jitter");

                }

                crate::state_manager::Action::RotatePorts => {
                    tracing::info!("[state] Rotating ports (stealth mode) - DISABLED for testing");
                }

                crate::state_manager::Action::OptimizeLatency => {

                    tracing::debug!("[state] Optimizing latency");

                }

                crate::state_manager::Action::None => {}

            }

        }
    }

    /// Rotate UDP ports (for stealth mode)
    pub async fn rotate_ports(&self) -> Result<(), String> {
        let (new_discovery_port, new_data_port) = PortManager::generate_port_pair();
        
        // Create new sockets
        let (new_discovery, new_data) = Self::create_new_sockets(new_discovery_port, new_data_port).await?;
        let (new_discovery_stop_tx, new_discovery_stop_rx) = oneshot::channel();
        let (new_data_stop_tx, new_data_stop_rx) = oneshot::channel();
        let transport_arc = Arc::new(self.clone());

        Self::spawn_discovery_listener_task(
            transport_arc.clone(),
            new_discovery.clone(),
            Some(new_discovery_stop_rx),
        );
        Self::spawn_data_listener_task(
            transport_arc.clone(),
            new_data.clone(),
            Some(new_data_stop_rx),
        );
        
        // Notify active peers
        // Get current loss rate from State Manager
        let loss_rate = {
            let sm = self.state_manager.lock().await;
            (sm.get_current_loss_rate() * 1000.0) as u16
        };
        // Get current RX speed
        let rx_speed = {
            let current_rx = self.rx_bytes_counter.load(Ordering::Relaxed);
            let last_rx = *self.last_rx_bytes.lock().await;
            let elapsed = self.last_rx_time.lock().await.elapsed().as_secs_f64();
            if elapsed > 0.0 { (current_rx - last_rx) as u32 } else { 0 }
        };


        let active_peers: Vec<HashId> = {
            let peers_lock = self.peers.lock().await;
            peers_lock.keys().copied().collect()
        };
        
        let port_update_packet = PortUpdatePacket::new(
            new_discovery_port,
            new_data_port,
            0,
            loss_rate,
            rx_speed
        );
        let packet_bytes = port_update_packet.to_bytes();
        
        for peer_id in &active_peers {
            let _ = self.send_encrypted(*peer_id, &packet_bytes).await;
        }
        
        // Update socket manager
        self.socket_manager.rotate(new_discovery.clone(), new_data.clone(), new_data.clone()).await;
        self.port_manager
            .set_current_ports(new_discovery_port, new_data_port)
            .await;
        
        // Notify listeners
        let new_pair = SocketPair::new(new_discovery, new_data.clone(), new_data);
        let _ = self.port_rotation_tx.send(new_pair);

        let previous_rotated = {
            let mut listeners = self.active_rotated_listeners.lock().await;
            let previous = listeners.take();
            *listeners = Some(RotatedListenerHandles {
                discovery_port: new_discovery_port,
                data_port: new_data_port,
                discovery_stop: new_discovery_stop_tx,
                data_stop: new_data_stop_tx,
            });
            previous
        };

        if let Some(previous) = previous_rotated {
            let overlap = self.port_manager.overlap_duration();
            tokio::spawn(async move {
                tokio::time::sleep(overlap).await;
                let _ = previous.discovery_stop.send(());
                let _ = previous.data_stop.send(());
            });
        }
        
        tracing::info!("[state] ✅ Port rotation complete: {}/{}", new_discovery_port, new_data_port);
        Ok(())
    }

}


// ====================== PORT UPDATE PROTOCOL (для 10/10 ротации) ======================

#[derive(Debug, Clone)]
pub struct PortUpdatePacket {
    pub discovery_port: u16,
    pub data_port: u16,
    pub sequence: u64,
    pub loss_rate: u16, // loss_rate * 1000 (например, 47 = 4.7%)
    pub rx_speed: u32, // RX speed in bytes/sec (up to ~4GB/s)
    pub timestamp: u64,
}

impl PortUpdatePacket {
    pub fn new(discovery_port: u16, data_port: u16, sequence: u64, loss_rate: u16, rx_speed: u32) -> Self {
        Self {
            discovery_port,
            data_port,
            sequence,
            loss_rate,
            rx_speed,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(26);
        buf.push(0xF8); // Port Update message type
        buf.extend_from_slice(&self.discovery_port.to_be_bytes());
        buf.extend_from_slice(&self.data_port.to_be_bytes());
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        buf.extend_from_slice(&self.loss_rate.to_be_bytes());
        buf.extend_from_slice(&self.rx_speed.to_be_bytes());
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        buf
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 27 || data[0] != 0xF8 {
            return None;
        }
        let discovery_port = u16::from_be_bytes(data[1..3].try_into().ok()?);
        let data_port = u16::from_be_bytes(data[3..5].try_into().ok()?);
        let sequence = u64::from_be_bytes(data[5..13].try_into().ok()?);
        let loss_rate = u16::from_be_bytes(data[13..15].try_into().ok()?);
        let rx_speed = u32::from_be_bytes(data[15..19].try_into().ok()?);
        let timestamp = u64::from_be_bytes(data[19..27].try_into().ok()?);

        Some(Self {
            discovery_port,
            data_port,
            sequence,
            loss_rate,
            rx_speed,
            timestamp,
        })
    }
}

// ── Node Identity Binding Fix — Barrier 1 unit tests ────────────────────────
//
// Found by a live exploit: a Hello passing Ed25519 verification only proves
// internal self-consistency, never that this is the node_id we've pinned.
// `P2PTransport::identity_conflict` is the guard that closes that gap —
// tested here directly (pure function over a HashMap, no live socket/process
// needed) rather than via a real two-node run, which this codebase
// deliberately avoids in automated tests (constructing a real P2PTransport
// binds fixed, well-known ports that could collide with an actual running
// node — see the prior mandate's report).
#[cfg(test)]
mod identity_pin_tests {
    use super::*;

    fn key(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    #[test]
    fn resume_is_bound_to_the_node_id_of_the_paired_key() {
        let pk = [7u8; 32];
        let own = HashId(crate::util::types::derive_node_id(&pk));
        let other_bound = HashId(crate::util::types::derive_node_id(&[8u8; 32]));
        let legacy = HashId([9u8; 32]);
        assert!(P2PTransport::resume_node_matches(&hex::encode(pk), &own));
        assert!(!P2PTransport::resume_node_matches(&hex::encode(pk), &other_bound), "a bound id of another key is refused");
        assert!(P2PTransport::resume_node_matches(&hex::encode(pk), &legacy), "an old random id is still accepted before the sunset");
        assert!(!P2PTransport::resume_node_matches("zz", &own));
    }

    #[test]
    fn a_hello_signed_the_ws_way_is_accepted_and_tampering_is_not() {
        let id = crate::core::NodeIdentity::new();
        let mut h = HelloPacket::new_request(id.node_id(), id.signing_public_key, id.public_key, [0u8; 8], 0);
        let sig = id.sign(&h.challenge_data()).unwrap();
        let mut b = [0u8; 64];
        b.copy_from_slice(&sig);
        h.signature = crate::netlayer::packet::Signature(b);
        assert!(P2PTransport::verify_peer_handshake_static(&h).is_ok());
        h.capabilities ^= 1;
        assert!(P2PTransport::verify_peer_handshake_static(&h).is_err());
    }

    #[test]
    fn a_hello_claiming_a_bound_node_id_of_another_key_is_rejected() {
        let victim = crate::core::NodeIdentity::new();
        let attacker = crate::core::NodeIdentity::new();
        // the attacker signs a Hello that carries the victim's node id
        let mut h = HelloPacket::new_request(victim.node_id(), attacker.signing_public_key, attacker.public_key, [0u8; 8], 0);
        let sig = attacker.sign(&h.challenge_data()).unwrap();
        let mut b = [0u8; 64];
        b.copy_from_slice(&sig);
        h.signature = crate::netlayer::packet::Signature(b);
        assert!(P2PTransport::verify_peer_handshake_static(&h).is_err());
        // a new identity's id is bound to its own key
        assert!(crate::util::types::id_bound_to_key(&victim.node_id().0, &victim.signing_public_key));
    }

    #[test]
    fn unsigned_hello_and_ack_are_rejected() {
        let mut h = HelloPacket::new_request(
            HashId([1u8; 32]), [2u8; 32], [2u8; 32], [0u8; 8], 0,
        );
        assert!(P2PTransport::verify_peer_handshake_static(&h).is_err());
        h.node_id = HashId([3u8; 32]);
        assert!(P2PTransport::verify_peer_handshake_static(&h).is_err());
    }

    #[test]
    fn unpinned_node_id_is_never_a_conflict() {
        let pins: HashMap<HashId, [u8; 32]> = HashMap::new();
        let node_id = HashId([1u8; 32]);
        assert!(!P2PTransport::identity_conflict(&pins, node_id, &key(0xAA)));
    }

    #[test]
    fn pinned_node_id_with_matching_key_is_not_a_conflict() {
        let node_id = HashId([2u8; 32]);
        let mut pins = HashMap::new();
        pins.insert(node_id, key(0xBB));
        assert!(!P2PTransport::identity_conflict(&pins, node_id, &key(0xBB)));
    }

    #[test]
    fn pinned_node_id_with_a_different_key_is_a_conflict() {
        // The exact shape of the live exploit: X claims B's node_id with
        // its own, different, otherwise perfectly valid signing key.
        let node_id_b = HashId([3u8; 32]);
        let real_key_b = key(0xCC);
        let attacker_key_x = key(0xDD);
        let mut pins = HashMap::new();
        pins.insert(node_id_b, real_key_b);
        assert!(P2PTransport::identity_conflict(&pins, node_id_b, &attacker_key_x));
    }

    #[test]
    fn pinning_one_node_id_never_affects_another() {
        let node_id_b = HashId([4u8; 32]);
        let node_id_other = HashId([5u8; 32]);
        let mut pins = HashMap::new();
        pins.insert(node_id_b, key(0xEE));
        // A totally unrelated, unpinned node_id must remain unaffected —
        // pinning B must not accidentally restrict discovery of anyone else.
        assert!(!P2PTransport::identity_conflict(&pins, node_id_other, &key(0xFF)));
    }
}
