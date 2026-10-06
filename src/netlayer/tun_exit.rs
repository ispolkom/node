//! TUN Exit Node Handler
//!
//! Принимает YTP wagons от entry nodes через P2PTransport (порт 10000),
//! распаковывает TCP пакеты и отправляет в интернет

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use tokio::net::{TcpStream, UdpSocket as TokioUdpSocket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{RwLock, mpsc};
use serde::{Serialize, Deserialize};

use crate::util::HashId;
use crate::netlayer::transport::P2PTransport;
use crate::protocol::Station;

/// Тип для ошибок
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// TUN Wagon - данные для передачи через P2P (аналог Socks5TunnelData)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunWagon {
    /// Уникальный ID соединения (entry_peer + client_ip + client_port)
    pub connection_id: String,
    /// IPv6 пакет (полный, включая заголовки IPv6 + TCP + payload)
    pub packet: Vec<u8>,
    /// Флаг закрытия соединения
    pub close: bool,
}

/// TUN Wagon Response - ответ от exit node к entry node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunWagonResponse {
    /// Уникальный ID соединения
    pub connection_id: String,
    /// Данные ответа (TCP пакеты от интернета)
    pub data: Vec<u8>,
    /// Флаг закрытия соединения
    pub close: bool,
}

/// IPv6 заголовок
#[derive(Debug)]
struct IPv6Header {
    src_addr: Ipv6Addr,
    dst_addr: Ipv6Addr,
    next_header: u8,  // Protocol (TCP = 6, UDP = 17)
    payload_length: u16,
}

impl IPv6Header {
    /// Распарсить IPv6 заголовок
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 40 {
            return None;
        }

        // Version (4 bits) should be 6
        let version = data[0] >> 4;
        if version != 6 {
            return None;
        }

        let payload_length = u16::from_be_bytes([data[4], data[5]]);
        let next_header = data[6];

        // Skip Hop Limit (data[7])
        // Source IPv6 (16 bytes)
        let src_addr = Ipv6Addr::from([
            data[8], data[9], data[10], data[11],
            data[12], data[13], data[14], data[15],
            data[16], data[17], data[18], data[19],
            data[20], data[21], data[22], data[23],
        ]);

        // Destination IPv6 (16 bytes)
        let dst_addr = Ipv6Addr::from([
            data[24], data[25], data[26], data[27],
            data[28], data[29], data[30], data[31],
            data[32], data[33], data[34], data[35],
            data[36], data[37], data[38], data[39],
        ]);

        Some(IPv6Header {
            src_addr,
            dst_addr,
            next_header,
            payload_length,
        })
    }
}

/// TCP заголовок (упрощённый)
#[derive(Debug)]
struct TcpHeader {
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
}

impl TcpHeader {
    /// Распарсить TCP заголовок из bytes
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 20 {
            return None;
        }

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);
        let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let data_offset = (data[12] >> 4) * 4;
        let flags = data[13];
        let window = u16::from_be_bytes([data[14], data[15]]);

        // Валидация
        if data_offset < 20 || data_offset as usize > data.len() {
            return None;
        }

        Some(TcpHeader {
            src_port,
            dst_port,
            seq,
            ack,
            flags,
            window,
        })
    }

    /// Проверить флаг SYN
    fn is_syn(&self) -> bool {
        self.flags & 0x02 != 0
    }

    /// Проверить флаг ACK
    fn is_ack(&self) -> bool {
        self.flags & 0x10 != 0
    }

    /// Проверить флаг FIN
    fn is_fin(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Проверить флаг RST
    fn is_rst(&self) -> bool {
        self.flags & 0x04 != 0
    }

    /// Проверить флаг PSH (data)
    fn is_psh(&self) -> bool {
        self.flags & 0x08 != 0
    }
}

/// TCP соединение с клиентом и внешним сервером
#[derive(Debug)]
struct TcpConnection {
    /// Entry node peer ID
    entry_peer: HashId,
    /// Адрес клиента (fd00::2:12345)
    client_addr: Ipv6Addr,
    /// Порт клиента
    client_port: u16,
    /// Реальный socket в интернет
    external_socket: Option<TcpStream>,
    /// Адрес внешнего сервера
    dest_addr: SocketAddr,
    /// Время создания
    created: std::time::Instant,
}

impl TcpConnection {
    fn new(entry_peer: HashId, client_addr: Ipv6Addr, client_port: u16,
           dest_addr: SocketAddr) -> Self {
        Self {
            entry_peer,
            client_addr,
            client_port,
            external_socket: None,
            dest_addr,
            created: std::time::Instant::now(),
        }
    }

    /// Установить внешний socket
    fn set_socket(&mut self, socket: TcpStream) {
        self.external_socket = Some(socket);
    }

    /// Получить socket для чтения
    fn take_socket(&mut self) -> Option<TcpStream> {
        self.external_socket.take()
    }

    /// Закрыть соединение
    async fn close(&mut self) {
        if let Some(mut socket) = self.external_socket.take() {
            let _ = socket.shutdown().await;
        }
    }
}

/// TUN Exit Node Handler
pub struct TunExitHandler {
    /// P2P Transport reference (для отправки ответов entry node)
    transport: Arc<P2PTransport>,
    /// Station для отправки данных обратно через YTP
    station: Arc<Station>,
    /// Активные TCP соединения
    /// Ключ: (entry_peer, client_ip, client_port)
    connections: Arc<RwLock<HashMap<(HashId, Ipv6Addr, u16), TcpConnection>>>,
    /// Channel для получения TunWagon от entry nodes
    wagon_rx: mpsc::Receiver<(HashId, TunWagon)>,
    /// Флаг остановки
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl TunExitHandler {
    /// Создать новый TUN Exit Handler
    pub async fn new(
        transport: Arc<P2PTransport>,
        wagon_rx: mpsc::Receiver<(HashId, TunWagon)>
    ) -> Result<Self> {
        println!("🌍 [TUN EXIT] Creating TUN Exit Node Handler...");
        println!("   📡 Using P2PTransport (port 10000) for traffic");

        // Создаём Station для отправки данных обратно через YTP
        let station = Station::with_defaults(
            transport.identity().node_id(),
            transport.clone()
        );

        Ok(Self {
            transport,
            station: Arc::new(station),
            connections: Arc::new(RwLock::new(HashMap::new())),
            wagon_rx,
            running: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        })
    }

    /// Запустить обработчик
    pub async fn run(&mut self) -> Result<()> {
        println!("🌍 [TUN EXIT] Starting TUN Exit Node Handler...");
        println!("💡 Waiting for TUN wagons from entry nodes via P2PTransport...");

        while self.running.load(std::sync::atomic::Ordering::Relaxed) {
            // Receive TunWagon from channel
            match self.wagon_rx.recv().await {
                Some((entry_peer, wagon)) => {
                    println!("📨 [TUN EXIT] Received wagon from {}", hex::encode(&entry_peer.0[..8]));
                    println!("   📦 Connection ID: {}", wagon.connection_id);
                    println!("   📦 Packet size: {} bytes, close: {}", wagon.packet.len(), wagon.close);

                    // Process wagon
                    if let Err(e) = self.handle_wagon(entry_peer, wagon).await {
                        eprintln!("   ❌ Error handling wagon: {}", e);
                    }
                }
                None => {
                    println!("🛑 [TUN EXIT] Channel closed");
                    break;
                }
            }
        }

        println!("🛑 [TUN EXIT] Handler stopped");
        Ok(())
    }

    /// Обработать TunWagon от entry node
    async fn handle_wagon(&self, entry_peer: HashId, wagon: TunWagon) -> Result<()> {
        // кто может выходить в интернет через этот узел — выбор владельца (по умолчанию только доверенные узлы)
        if !crate::exit_policy::may_exit(&entry_peer) {
            crate::exit_policy::refuse_log(&entry_peer, "туннель пакетов");
            return Ok(());
        }
        // wagon.packet = полный IPv6 пакет (включая заголовки IPv6 + TCP + payload)
        let data = &wagon.packet;

        println!("   🔍 Parsing IPv6 packet ({} bytes)...", data.len());

        // Парсим IPv6 заголовок
        let ipv6_header = match IPv6Header::parse(data) {
            Some(header) => header,
            None => {
                println!("   ⚠️  Not a valid IPv6 packet");
                return Ok(());
            }
        };

        println!("   ✅ IPv6: {} -> {}", ipv6_header.src_addr, ipv6_header.dst_addr);

        // Проверяем что это TCP
        if ipv6_header.next_header != 6 {
            println!("   ⚠️  Not TCP (protocol={})", ipv6_header.next_header);
            return Ok(());
        }

        // Extract TCP payload (skip IPv6 header = 40 bytes)
        let tcp_data = data.get(40..).ok_or("Invalid TCP data")?;

        // Парсим TCP заголовок
        let tcp_header = match TcpHeader::parse(tcp_data) {
            Some(header) => header,
            None => {
                println!("   ⚠️  Not a valid TCP packet");
                return Ok(());
            }
        };

        println!("   ✅ TCP: {}:{} -> {}:{}",
                 ipv6_header.src_addr, tcp_header.src_port,
                 ipv6_header.dst_addr, tcp_header.dst_port);

        // Определяем тип пакета и обрабатываем
        if tcp_header.is_syn() && !tcp_header.is_ack() {
            println!("   🆕 SYN packet - creating new connection...");
            self.handle_syn(entry_peer, ipv6_header.src_addr, tcp_header.dst_port, wagon.connection_id).await?;
        } else if tcp_header.is_psh() || (tcp_header.is_ack() && !tcp_header.is_syn()) {
            println!("   📦 Data/ACK packet");
            self.handle_data(entry_peer, ipv6_header.src_addr, tcp_header.src_port, tcp_data).await?;
        } else if tcp_header.is_fin() || tcp_header.is_rst() || wagon.close {
            println!("   🔚 FIN/RST packet - closing connection");
            self.handle_close(entry_peer, ipv6_header.src_addr, tcp_header.src_port).await?;
        } else {
            println!("   ℹ️  Other TCP flags: 0x{:02x}", tcp_header.flags);
        }

        Ok(())
    }

    /// Обработать SYN - создать новое соединение
    async fn handle_syn(&self, entry_peer: HashId, client_addr: Ipv6Addr, client_port: u16,
                       connection_id: String) -> Result<()> {
        // TODO: Определить dest_addr на основе dst_addr из IPv6
        // Для демо подключаемся к google.com:443
        let dest_addr = "142.250.74.46:443".parse::<SocketAddr>()
            .map_err(|e| format!("Invalid dest address: {}", e))?;

        println!("   🔗 Creating TCP connection to {}", dest_addr);

        // Создаём socket
        let socket = TcpStream::connect(dest_addr).await
            .map_err(|e| format!("Failed to connect: {}", e))?;

        println!("   ✅ Connected to {}", dest_addr);

        // Создаём соединение (без entry_udp_addr - больше не нужен)
        let conn = TcpConnection::new(entry_peer, client_addr, client_port, dest_addr);

        // Запускаем task для чтения из socket и отправки обратно entry node
        let station_clone = self.station.clone();
        let client_addr_clone = client_addr;
        let client_port_clone = client_port;
        let dest_addr_clone = dest_addr;
        let entry_peer_clone = entry_peer;
        let connection_id_clone = connection_id;
        let connections_clone = self.connections.clone();

        tokio::spawn(async move {
            println!("   🔄 Started reverse handler for {}:{} -> {}", client_addr_clone, client_port_clone, dest_addr_clone);
            if let Err(e) = Self::reverse_path_handler(
                socket,
                station_clone,
                client_addr_clone,
                client_port_clone,
                entry_peer_clone,
                connection_id_clone,
                dest_addr_clone,
                connections_clone
            ).await {
                eprintln!("   ❌ Reverse handler error: {}", e);
            }
        });

        // Сохраняем соединение (без socket, так как он забран task'ом)
        let mut connections = self.connections.write().await;
        connections.insert((entry_peer, client_addr, client_port), conn);

        println!("   💾 Connection saved to table");
        println!("   ✅ Total connections: {}", connections.len());

        Ok(())
    }

    /// Обработать данные - отправить в существующий socket
    async fn handle_data(&self, entry_peer: HashId, client_addr: Ipv6Addr, client_port: u16,
                        tcp_data: &[u8]) -> Result<()> {
        let connections = self.connections.read().await;

        let key = (entry_peer, client_addr, client_port);
        if let Some(_conn) = connections.get(&key) {
            // TODO: Extract actual TCP payload data and send it to socket
            // Для демо просто логируем
            println!("   📤 Data would be sent ({} bytes)", tcp_data.len());
        } else {
            println!("   ⚠️  Connection not found");
        }

        Ok(())
    }

    /// Reverse path handler - читает из real socket и отправляет обратно entry node через P2P
    async fn reverse_path_handler(
        mut socket: TcpStream,
        station: Arc<Station>,
        client_addr: Ipv6Addr,
        client_port: u16,
        entry_peer: HashId,
        connection_id: String,
        dest_addr: SocketAddr,
        connections: Arc<RwLock<HashMap<(HashId, Ipv6Addr, u16), TcpConnection>>>,
    ) -> Result<()> {
        let mut buf = [0u8; 8192];

        loop {
            match socket.read(&mut buf).await {
                Ok(0) => {
                    println!("   🔚 Connection closed by {}", dest_addr);

                    // Send close signal back
                    let response = TunWagonResponse {
                        connection_id: connection_id.clone(),
                        data: vec![],
                        close: true,
                    };

                    if let Ok(resp_bytes) = serde_json::to_vec(&response) {
                        println!("   📤 Sending close signal to entry node");

                        // Отправляем через Station (YTP train)
                        if let Err(e) = station.send_train(entry_peer.clone(), resp_bytes).await {
                            eprintln!("   ❌ Failed to send close signal: {}", e);
                        } else {
                            println!("   ✅ Close signal sent via YTP");
                        }
                    }

                    break;
                }
                Ok(n) => {
                    println!("   📨 Read {} bytes from {}", n, dest_addr);

                    // Создаём ответ
                    let response = TunWagonResponse {
                        connection_id: connection_id.clone(),
                        data: buf[..n].to_vec(),
                        close: false,
                    };

                    // Сериализуем и отправляем через YTP
                    if let Ok(resp_bytes) = serde_json::to_vec(&response) {
                        println!("   📤 Sending response: {} bytes", resp_bytes.len());

                        // Отправляем через Station (YTP train)
                        if let Err(e) = station.send_train(entry_peer.clone(), resp_bytes).await {
                            eprintln!("   ❌ Failed to send response: {}", e);
                            break;
                        } else {
                            println!("   ✅ Response sent via YTP");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("   ❌ Error reading from socket: {}", e);
                    break;
                }
            }
        }

        // Удаляем соединение из таблицы
        let mut connections_lock = connections.write().await;
        connections_lock.remove(&(entry_peer, client_addr, client_port));
        println!("   🧹 Connection removed from table");

        Ok(())
    }

    /// Обработать закрытие - закрыть socket
    async fn handle_close(&self, entry_peer: HashId, client_addr: Ipv6Addr, client_port: u16) -> Result<()> {
        let mut connections = self.connections.write().await;

        let key = (entry_peer, client_addr, client_port);
        if let Some(mut conn) = connections.remove(&key) {
            println!("   🔚 Closing connection to {}", conn.dest_addr);
            conn.close().await;
            println!("   ✅ Connection closed");
            println!("   ✅ Remaining connections: {}", connections.len());
        } else {
            println!("   ⚠️  Connection not found");
        }

        Ok(())
    }

    /// Очистить старые соединения
    pub async fn cleanup_old_connections(&self, max_age: std::time::Duration) {
        let mut connections = self.connections.write().await;
        let now = std::time::Instant::now();

        let initial_count = connections.len();

        connections.retain(|_, conn| {
            now.duration_since(conn.created) < max_age
        });

        let removed = initial_count - connections.len();

        if removed > 0 {
            println!("🧹 [TUN EXIT] Cleaned up {} old connections", removed);
        }
    }
}
