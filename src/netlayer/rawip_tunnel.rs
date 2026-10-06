// src/netlayer/rawip_tunnel.rs
//!
//! Simple RawIP Tunnel for Mobile Entry Nodes
//!
//! Accepts raw IP packets via TCP (port 10001) and forwards them to internet.
//! Simplified alternative to full YTP/TunWagon implementation.
//!
//! Protocol:
//!   - Entry node connects via TCP to port 10001
//!   - Sends raw IP packets (header + payload)
//!   - Exit node masquerades and forwards to internet
//!   - Responses sent back via same TCP connection

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

use crate::util::HashId;
use crate::protocol::Station;

/// Результат операции
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Заголовок RawIP пакета (4 байта: [magic, length_high, length_low, reserved])
#[derive(Debug, Clone)]
struct RawIpHeader {
    /// Magic byte (0x59 = 'Y')
    magic: u8,
    /// Длина IP пакета (big-endian, 2 bytes)
    length: u16,
    /// Reserved (для будущего использования)
    _reserved: u8,
}

impl RawIpHeader {
    const MAGIC: u8 = 0x59;  // 'Y' for YANDI
    const SIZE: usize = 4;

    /// Создать заголовок
    fn new(length: u16) -> Self {
        Self {
            magic: Self::MAGIC,
            length,
            _reserved: 0,
        }
    }

    /// Сериализовать в байты
    fn to_bytes(&self) -> [u8; Self::SIZE] {
        [
            self.magic,
            (self.length >> 8) as u8,
            (self.length & 0xFF) as u8,
            self._reserved,
        ]
    }

    /// Десериализовать из байтов
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }

        let magic = bytes[0];
        if magic != Self::MAGIC {
            println!("   ⚠️  Invalid magic byte: 0x{:02x}", magic);
            return None;
        }

        let length = u16::from_be_bytes([bytes[1], bytes[2]]);

        Some(Self {
            magic,
            length,
            _reserved: bytes[3],
        })
    }
}

/// TCP соединение для RawIP туннеля
#[derive(Debug, Clone)]
struct RawIpConnection {
    /// Peer адрес (entry node)
    peer_addr: SocketAddr,
    /// Последовательный номер (для отслеживания)
    seq_num: u64,
    /// Время создания
    created: std::time::Instant,
}

impl RawIpConnection {
    fn new(peer_addr: SocketAddr) -> Self {
        Self {
            peer_addr,
            seq_num: 0,
            created: std::time::Instant::now(),
        }
    }
}

/// RawIP Tunnel Handler
pub struct RawIpTunnel {
    /// Station для отправки данных (не используется, но сохраняем)
    _station: Arc<Station>,
    /// Активные соединения (peer_addr -> connection)
    connections: Arc<RwLock<HashMap<SocketAddr, RawIpConnection>>>,
    /// Порт для прослушивания
    listen_port: u16,
}

impl RawIpTunnel {
    /// Создать новый RawIP туннель
    pub fn new(station: Arc<Station>, listen_port: u16) -> Self {
        println!("🌐 [RAWIP] Creating RawIP Tunnel on port {}...", listen_port);

        Self {
            _station: station,
            connections: Arc::new(RwLock::new(HashMap::new())),
            listen_port,
        }
    }

    /// Запустить туннель
    pub async fn run(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.listen_port);
        let listener = TcpListener::bind(&addr).await?;
        println!("🌐 [RAWIP] Listening on {} for RawIP packets", addr);

        loop {
            match listener.accept().await {
                Ok((socket, peer_addr)) => {
                    println!("📡 [RAWIP] New connection from {}", peer_addr);

                    // Обрабатываем соединение в отдельном task
                    let connections = self.connections.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(socket, peer_addr, connections).await {
                            eprintln!("❌ [RAWIP] Connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("❌ [RAWIP] Accept error: {}", e);
                }
            }
        }
    }

    /// Обработать TCP соединение от entry node
    async fn handle_connection(
        mut socket: TcpStream,
        peer_addr: SocketAddr,
        connections: Arc<RwLock<HashMap<SocketAddr, RawIpConnection>>>,
    ) -> Result<()> {
        // Создаём соединение
        let mut conn = RawIpConnection::new(peer_addr);

        // Сохраняем соединение
        {
            let mut conns = connections.write().await;
            conns.insert(peer_addr, conn.clone());
            println!("💾 [RAWIP] Connection saved: {} (total: {})", peer_addr, conns.len());
        }

        let mut buffer = [0u8; 8192];  // Буфер для чтения

        loop {
            // Читаем заголовок (4 байта)
            match socket.read_exact(&mut buffer[..RawIpHeader::SIZE]).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    println!("🔚 [RAWIP] Connection closed by {}", peer_addr);
                    break;
                }
                Err(e) => {
                    eprintln!("❌ [RAWIP] Read header error: {}", e);
                    break;
                }
            }

            // Парсим заголовок
            let header = match RawIpHeader::from_bytes(&buffer[..RawIpHeader::SIZE]) {
                Some(h) => h,
                None => {
                    eprintln!("⚠️  [RAWIP] Invalid header, closing connection");
                    break;
                }
            };

            // Читаем IP пакет
            let packet_len = header.length as usize;
            if packet_len > buffer.len() {
                eprintln!("⚠️  [RAWIP] Packet too large: {} bytes", packet_len);
                break;
            }

            match socket.read_exact(&mut buffer[..packet_len]).await {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("❌ [RAWIP] Read packet error: {}", e);
                    break;
                }
            }

            let packet = &buffer[..packet_len];
            conn.seq_num += 1;

            println!("📦 [RAWIP] Packet #{} from {} ({} bytes)", conn.seq_num, peer_addr, packet_len);

            // Обрабатываем IP пакет
            if let Err(e) = Self::handle_ip_packet(packet, &mut socket, peer_addr).await {
                eprintln!("❌ [RAWIP] Handle packet error: {}", e);
            }
        }

        // Удаляем соединение
        {
            let mut conns = connections.write().await;
            conns.remove(&peer_addr);
            println!("🧹 [RAWIP] Connection removed: {} (total: {})", peer_addr, conns.len());
        }

        Ok(())
    }

    /// Обработать IP пакет
    async fn handle_ip_packet(
        packet: &[u8],
        _response_socket: &mut TcpStream,
        _peer_addr: SocketAddr,
    ) -> Result<()> {
        // Проверяем версию IP
        if packet.is_empty() {
            return Ok(());
        }

        let version = (packet[0] & 0xF0) >> 4;

        if version == 4 {
            Self::handle_ipv4_packet(packet).await?;
        } else if version == 6 {
            Self::handle_ipv6_packet(packet).await?;
        } else {
            println!("⚠️  [RAWIP] Unknown IP version: {}", version);
        }

        Ok(())
    }

    /// Обработать IPv4 пакет
    async fn handle_ipv4_packet(packet: &[u8]) -> Result<()> {
        // IPv4 header (минимум 20 байт)
        if packet.len() < 20 {
            return Ok(());
        }

        let header_len = ((packet[0] & 0x0F) as usize) * 4;
        if packet.len() < header_len {
            return Ok(());
        }

        let protocol = packet[9];  // Protocol (6 = TCP, 17 = UDP)

        let src_addr = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
        let dst_addr = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);

        match protocol {
            6 => println!("   📡 IPv4/TCP: {} -> {}", src_addr, dst_addr),
            17 => println!("   📡 IPv4/UDP: {} -> {}", src_addr, dst_addr),
            _ => println!("   📡 IPv4/proto={}: {} -> {}", protocol, src_addr, dst_addr),
        }

        // TODO: Реальное перенаправление в интернет через NAT
        // Для демо просто логируем

        Ok(())
    }

    /// Обработать IPv6 пакет
    async fn handle_ipv6_packet(packet: &[u8]) -> Result<()> {
        // IPv6 header (40 байт)
        if packet.len() < 40 {
            return Ok(());
        }

        let protocol = packet[6];  // Next header (6 = TCP, 17 = UDP)

        // Src IPv6 (bytes 8-23)
        let src_bytes: [u8; 16] = packet[8..24].try_into()
            .map_err(|_| "Invalid src IPv6")?;
        let src_addr = std::net::Ipv6Addr::from(src_bytes);

        // Dst IPv6 (bytes 24-39)
        let dst_bytes: [u8; 16] = packet[24..40].try_into()
            .map_err(|_| "Invalid dst IPv6")?;
        let dst_addr = std::net::Ipv6Addr::from(dst_bytes);

        match protocol {
            6 => println!("   📡 IPv6/TCP: {} -> {}", src_addr, dst_addr),
            17 => println!("   📡 IPv6/UDP: {} -> {}", src_addr, dst_addr),
            _ => println!("   📡 IPv6/proto={}: {} -> {}", protocol, src_addr, dst_addr),
        }

        // TODO: Реальное перенаправление в интернет через NAT
        // Для демо просто логируем

        Ok(())
    }
}
