// src/proxy/gateway.rs
//! HTTP Proxy Gateway (Reverse Proxy)
//! ==================================
//!
//! Receives proxy requests via P2P and makes real HTTPS requests to target servers

use crate::netlayer::P2PTransport;
use crate::proxy::{ProxyRequest, ProxyResponse, ProxyTunnelData};
use crate::util::HashId;
use crate::protocol::{Station, Wagon, WagonNack};
use std::sync::Arc;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use anyhow::{Result, Context};
use tracing::{info, error, debug, warn};
use reqwest::Client as HttpClient;

fn is_normal_tunnel_shutdown(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::NotConnected
    )
}

/// Хранилище отправленных wagon-ов для retransmission
#[derive(Debug)]
pub struct SentTrain {
    /// ID поезда
    train_id: u64,

    /// Wagon-ы: wagon_num → wagon data (сериализованный)
    wagons: HashMap<u16, Vec<u8>>,

    /// Время отправки
    sent_time: Instant,

    /// Target node (кому отправляли)
    pub target_node: HashId,
}

impl SentTrain {
    /// Создать новое хранилище для поезда
    pub fn new(train_id: u64, target_node: HashId) -> Self {
        Self {
            train_id,
            wagons: HashMap::new(),
            sent_time: Instant::now(),
            target_node,
        }
    }

    /// Добавить wagon (public для gateway)
    pub fn add_wagon(&mut self, wagon_num: u16, wagon_data: Vec<u8>) {
        self.wagons.insert(wagon_num, wagon_data);
    }

    /// Проверить устарел ли train (TTL 60 секунд)
    fn is_expired(&self) -> bool {
        self.sent_time.elapsed() > Duration::from_secs(60)
    }

    /// Получить wagon для переотправки
    pub fn get_wagon(&self, wagon_num: u16) -> Option<&Vec<u8>> {
        self.wagons.get(&wagon_num)
    }
}

/// Helper function to send tunnel data
async fn send_tunnel_data(
    station: &Arc<Station>,
    target_node: HashId,
    tunnel_data: ProxyTunnelData,
) -> Result<()> {
    // ⚡ Используем bincode вместо JSON (в 3-5 раз быстрее)
    let data_bytes = tunnel_data.to_bincode()
        .map_err(|e| anyhow::anyhow!("Failed to serialize tunnel data: {}", e))?;

    // ✅ Используем ОДНУ И ТУ ЖЕ Station (не создаём новую!)
    station.send_train(target_node, data_bytes).await
        .map_err(|e| anyhow::anyhow!("Failed to send tunnel data: {}", e))?;

    Ok(())
}

/// HTTP Proxy Gateway (runs on gateway/exit node)
///
/// Receives requests via P2P and makes real HTTPS requests
pub struct HttpProxyGateway {
    pub transport: Arc<P2PTransport>,
    http_client: HttpClient,
    pub station: Arc<Station>,
    request_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyRequest)>>>>,
    tunnel_data_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>>>>,
    /// Active tunnels: tunnel_id -> write half
    active_tunnels: Arc<Mutex<HashMap<(HashId, u64), tokio::net::tcp::OwnedWriteHalf>>>,
    /// Отправленные wagon-ы для retransmission: train_id → SentTrain
    pub sent_trains: Arc<Mutex<HashMap<u64, SentTrain>>>,
}

impl HttpProxyGateway {
    /// Create new HTTP proxy gateway
    pub fn new(transport: Arc<P2PTransport>) -> Self {
        let http_client = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        // Create YTP Station
        let station = Station::with_defaults(
            transport.identity().node_id(),
            transport.clone()
        );

        Self {
            transport,
            http_client,
            station: Arc::new(station),
            request_rx: Arc::new(Mutex::new(None)),
            tunnel_data_rx: Arc::new(Mutex::new(None)),
            active_tunnels: Arc::new(Mutex::new(HashMap::new())),
            sent_trains: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set request channel
    pub fn with_request_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, ProxyRequest)>) -> Self {
        self.request_rx = Arc::new(Mutex::new(Some(rx)));
        self
    }

    /// Set tunnel data channel
    pub fn with_tunnel_data_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>) -> Self {
        self.tunnel_data_rx = Arc::new(Mutex::new(Some(rx)));
        self
    }

    /// Start the gateway
    pub async fn run(&self) -> Result<()> {
        info!("🌐 HTTP Proxy Gateway started");
        info!("📡 Listening for proxy requests via P2P");

        // Start tunnel data handler
        let active_tunnels_clone = self.active_tunnels.clone();
        let tunnel_rx_clone = self.tunnel_data_rx.clone();
        tokio::spawn(async move {
            Self::handle_tunnel_data(active_tunnels_clone, tunnel_rx_clone).await;
        });

        // Start sent trains cleanup task (удаляем устаревшие trains каждые 30 секунд)
        let sent_trains_clone = self.sent_trains.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Self::cleanup_expired_trains(sent_trains_clone.clone()).await;
            }
        });

        // Get the receiver
        let mut rx_opt = {
            let mut rx_lock = self.request_rx.lock().await;
            rx_lock.take()
        };

        let mut rx = match rx_opt {
            Some(r) => r,
            None => {
                warn!("⚠️  No request channel configured!");
                return Ok(());
            }
        };

        info!("✅ Gateway ready to process requests");

        // Listen for incoming requests
        while let Some((source_node, request)) = rx.recv().await {
            let gateway = self.clone_for_handler();

            tokio::spawn(async move {
                if let Err(e) = gateway.handle_request(source_node, request).await {
                    error!("❌ Error handling proxy request: {}", e);
                }
            });
        }

        warn!("⚠️  Gateway request channel closed");
        Ok(())
    }

    /// Clone for handler
    pub fn clone_for_handler(&self) -> Self {
        Self {
            transport: self.transport.clone(),
            http_client: self.http_client.clone(),
            station: self.station.clone(),
            request_rx: self.request_rx.clone(),
            tunnel_data_rx: self.tunnel_data_rx.clone(),
            active_tunnels: self.active_tunnels.clone(),
            sent_trains: self.sent_trains.clone(),
        }
    }

    /// Handle proxy request
    pub async fn handle_request(&self, source_node: HashId, request: ProxyRequest) -> Result<()> {
        info!("📨 Proxy request #{} from {}", request.request_id, hex::encode(&source_node.0[..8]));
        // кто может выходить в интернет через этот узел — выбор владельца (по умолчанию только доверенные узлы)
        if !crate::exit_policy::may_exit(&source_node) {
            crate::exit_policy::refuse_log(&source_node, "HTTP-прокси");
            let refused = ProxyResponse { request_id: request.request_id, status: 403, headers: vec![], body: "exit not allowed for this node".as_bytes().to_vec() };
            self.send_response(source_node, refused).await?;
            return Ok(());
        }
        debug!("🎯 Target: {} {}", request.method, request.url);

        // Special handling for CONNECT (HTTPS tunneling)
        if request.method == "CONNECT" {
            info!("🔐 CONNECT request - starting TCP tunnel");
            return self.handle_connect(source_node, request).await;
        }

        // Make real HTTP request
        let http_response = self.make_http_request(&request).await?;

        // Build proxy response
        let proxy_response = ProxyResponse {
            request_id: request.request_id,
            status: http_response.status().as_u16(),
            headers: self.extract_headers(&http_response),
            body: http_response.bytes().await?.to_vec(),
        };

        debug!("✅ Response: {} {} bytes", proxy_response.status, proxy_response.body.len());

        // Send response via P2P
        self.send_response(source_node, proxy_response).await?;

        Ok(())
    }

    /// Handle CONNECT request - establish TCP tunnel
    async fn handle_connect(&self, source_node: HashId, request: ProxyRequest) -> Result<()> {
        use tokio::net::TcpStream;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::time::timeout;

        // Parse host:port from URL (format: "goodwin.su:443" or "example.com:443")
        let addr_str = request.url.clone();

        info!("🔌 Connecting to {} ...", addr_str);

        // Только адрес в интернете (не этот компьютер, не домашняя сеть), соединение — ровно с проверенным адресом
        let target_stream = match crate::exit_policy::connect_public(&addr_str, Duration::from_secs(10)).await {
            Ok(stream) => {
                info!("✅ Connected to {}", addr_str);
                stream
            }
            Err(e) => {
                let status = if e.kind() == std::io::ErrorKind::PermissionDenied { 403 } else if e.kind() == std::io::ErrorKind::TimedOut { 504 } else { 502 };
                error!("❌ CONNECT {} refused/failed: {}", addr_str, e);
                let proxy_response = ProxyResponse { request_id: request.request_id, status, headers: vec![], body: Vec::new() };
                self.send_response(source_node, proxy_response).await?;
                return Err(anyhow::anyhow!("Failed to connect: {}", e));
            }
        };

        // Send "200 Connection Established" back to client
        let proxy_response = ProxyResponse {
            request_id: request.request_id,
            status: 200,
            headers: vec![
                ("Connection".to_string(), "keep-alive".to_string()),
            ],
            body: Vec::new(),
        };

        self.send_response(source_node, proxy_response).await?;
        info!("📤 Sent 200 Connection Established");

        // Start bi-directional forwarding
        let tunnel_id = request.request_id;
        let transport = self.transport.clone();
        let source_node_clone = source_node;

        info!("🚇 Starting tunnel #{} bi-directional forwarding", tunnel_id);

        // Split TCP stream
        let (mut read_half, write_half) = target_stream.into_split();

        // Store write_half in active_tunnels
        {
            let mut tunnels = self.active_tunnels.lock().await;
            // соединения разных узлов — раздельно: номер туннеля выбирает сам узел, чужой номер не даёт доступа к чужому соединению
            tunnels.insert((source_node, tunnel_id), write_half);
            info!("🚇 Tunnel #{}: write_half stored in active_tunnels", tunnel_id);
        }

        let active_tunnels_for_reader = self.active_tunnels.clone();

        // Task: Read from target and send to client via YTP
        let transport_to_client = transport.clone();
        let station_to_client = self.station.clone(); // ✅ Добавляем!
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024]; // ⚡ 64 KB buffer for tunnel data
            loop {
                match read_half.read(&mut buf).await {
                    Ok(0) => {
                        info!("🚇 Tunnel #{}: Server closed connection", tunnel_id);
                        {
                            let mut tunnels = active_tunnels_for_reader.lock().await;
                            tunnels.remove(&(source_node_clone, tunnel_id));
                        }
                        // Send close packet
                        let tunnel_data = ProxyTunnelData {
                            tunnel_id,
                            data: Vec::new(),
                            close: true,
                        };
                        let _ = send_tunnel_data(&station_to_client, source_node_clone, tunnel_data).await;
                        break;
                    }
                    Ok(n) => {
                        debug!("🚇 Tunnel #{}: Read {} bytes from server", tunnel_id, n);

                        let tunnel_data = ProxyTunnelData {
                            tunnel_id,
                            data: buf[..n].to_vec(),
                            close: false,
                        };

                        if let Err(e) = send_tunnel_data(&station_to_client, source_node_clone, tunnel_data).await {
                            error!("❌ Failed to send tunnel data: {}", e);
                            {
                                let mut tunnels = active_tunnels_for_reader.lock().await;
                                tunnels.remove(&(source_node_clone, tunnel_id));
                            }
                            // ✅ FIX: Send close packet to client on error
                            let close_data = ProxyTunnelData {
                                tunnel_id,
                                data: Vec::new(),
                                close: true,
                            };
                            let _ = send_tunnel_data(&station_to_client, source_node_clone, close_data).await;
                            break;
                        }
                    }
                    Err(e) => {
                        error!("❌ Error reading from server: {}", e);
                        {
                            let mut tunnels = active_tunnels_for_reader.lock().await;
                            tunnels.remove(&(source_node_clone, tunnel_id));
                        }
                        // ✅ FIX: Send close packet to client on read error
                        let tunnel_data = ProxyTunnelData {
                            tunnel_id,
                            data: Vec::new(),
                            close: true,
                        };
                        let _ = send_tunnel_data(&station_to_client, source_node_clone, tunnel_data).await;
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Handle incoming tunnel data from client (async method - same as client's)
    async fn handle_tunnel_data(
        active_tunnels: Arc<Mutex<HashMap<(HashId, u64), tokio::net::tcp::OwnedWriteHalf>>>,
        tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>>>>,
    ) {
        info!("🚇 Tunnel data handler started (gateway)");

        // Get the receiver - take ownership
        let mut rx_opt = {
            let mut rx_lock = tunnel_rx.lock().await;
            rx_lock.take()
        };

        let mut rx = match rx_opt {
            Some(r) => r,
            None => {
                warn!("⚠️  No tunnel data channel configured!");
                return;
            }
        };

        // Listen for tunnel data packets
        while let Some((source_node, tunnel_data)) = rx.recv().await {
            let tunnel_id = tunnel_data.tunnel_id;
            let key = (source_node, tunnel_id);
            let data_len = tunnel_data.data.len();
            let is_close = tunnel_data.close;

            debug!("🚇 Received tunnel data #{}: {} bytes, close={}",
                   tunnel_id, data_len, is_close);

            // ✅ FIX: Используем get_mut() вместо remove() чтобы устранить race condition!
            let mut tunnels = active_tunnels.lock().await;

            if let Some(write_half) = tunnels.get_mut(&key) {
                use tokio::io::AsyncWriteExt;

                if is_close {
                    info!("🚇 Tunnel #{}: close packet received", tunnel_id);
                    // Close the connection to server
                    let _ = write_half.shutdown().await;
                    // Remove tunnel from active list
                    tunnels.remove(&key);
                    continue;
                }

                if !tunnel_data.data.is_empty() {
                    if let Err(e) = write_half.write_all(&tunnel_data.data).await {
                        if is_normal_tunnel_shutdown(&e) {
                            debug!("🚇 Tunnel #{}: upstream side already closed ({})", tunnel_id, e);
                        } else {
                            error!("❌ Error writing to server: {}", e);
                        }
                        let _ = write_half.shutdown().await;
                        tunnels.remove(&key);
                        continue;
                    }

                    debug!("✅ Tunnel #{}: wrote {} bytes to server", tunnel_id, data_len);

                    // Flush to ensure data is sent
                    if let Err(e) = write_half.flush().await {
                        if is_normal_tunnel_shutdown(&e) {
                            debug!("🚇 Tunnel #{}: upstream flush skipped, socket closed ({})", tunnel_id, e);
                        } else {
                            error!("❌ Error flushing server tunnel #{}: {}", tunnel_id, e);
                        }
                        let _ = write_half.shutdown().await;
                        tunnels.remove(&key);
                        continue;
                    }
                }
            } else {
                debug!("🚇 Tunnel #{}: data arrived after upstream close, dropping", tunnel_id);
            }
        }

        warn!("⚠️  Tunnel data channel closed");
    }

    /// Send tunnel data packet via YTP
    async fn send_tunnel_data_packet(
        &self,
        target_node: HashId,
        tunnel_data: ProxyTunnelData,
    ) -> Result<()> {
        // ⚡ Используем bincode вместо JSON (в 3-5 раз быстрее)
        let data_bytes = tunnel_data.to_bincode()
            .context("Failed to serialize tunnel data")?;

        debug!("📤 Sending tunnel data #{} ({} bytes)", tunnel_data.tunnel_id, data_bytes.len());

        self.station.send_train(target_node, data_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to send tunnel data: {}", e))?;

        Ok(())
    }

    /// Make real HTTP request to target
    async fn make_http_request(&self, request: &ProxyRequest) -> Result<reqwest::Response> {
        // Только адрес в интернете; запрос идёт ровно на проверенный адрес, переадресации не выполняются (их получает браузер,
        // и следующий запрос снова проходит проверку)
        let url = reqwest::Url::parse(&request.url).map_err(|e| anyhow::anyhow!("bad url {}: {}", request.url, e))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(anyhow::anyhow!("scheme not allowed: {}", url.scheme()));
        }
        let host = url.host_str().ok_or_else(|| anyhow::anyhow!("no host"))?.trim_matches(|c| c == '[' || c == ']').to_string();
        let port = url.port_or_known_default().ok_or_else(|| anyhow::anyhow!("no port"))?;
        let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
        let addrs = crate::exit_policy::resolve_public(&target).await.map_err(|e| anyhow::anyhow!("{}", e))?;
        let client = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&host, &addrs)
            .build()?;
        let mut http_req = match request.method.as_str() {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "HEAD" => client.head(url),
            _ => client.get(url),
        };

        // Add headers (filter out hop-by-hop headers)
        for (name, value) in &request.headers {
            let name_lower = name.to_lowercase();

            // Skip hop-by-hop headers
            if matches!(
                name_lower.as_str(),
                "connection" | "keep-alive" | "proxy-authenticate" |
                "proxy-authorization" | "te" | "trailers" |
                "transfer-encoding" | "upgrade"
            ) {
                continue;
            }

            // Skip proxy-related headers
            if matches!(
                name_lower.as_str(),
                "proxy-connection" | "proxy-authorization"
            ) {
                continue;
            }

            http_req = http_req.header(name, value);
        }

        // 🦎 Add User-Agent if not present (полный Firefox 122!)
        if !request.headers.iter().any(|(k, _)| k.to_lowercase() == "user-agent") {
            http_req = http_req.header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:122.0) Gecko/20100101 Firefox/122.0"
            );
        }

        // 🦎 Add critical Firefox headers if missing (X.com/YouTube требуют эти!)
        let header_names: Vec<&str> = request.headers.iter().map(|(k, _)| k.as_str()).collect();

        // Sec-Fetch-Site
        if !header_names.iter().any(|k| k.to_lowercase() == "sec-fetch-site") {
            http_req = http_req.header("Sec-Fetch-Site", "none");
        }

        // Sec-Fetch-Mode
        if !header_names.iter().any(|k| k.to_lowercase() == "sec-fetch-mode") {
            http_req = http_req.header("Sec-Fetch-Mode", "navigate");
        }

        // Sec-Fetch-User
        if !header_names.iter().any(|k| k.to_lowercase() == "sec-fetch-user") {
            http_req = http_req.header("Sec-Fetch-User", "?1");
        }

        // Sec-Fetch-Dest
        if !header_names.iter().any(|k| k.to_lowercase() == "sec-fetch-dest") {
            http_req = http_req.header("Sec-Fetch-Dest", "document");
        }

        // Accept-Encoding (важно для скорости!)
        if !header_names.iter().any(|k| k.to_lowercase() == "accept-encoding") {
            http_req = http_req.header("Accept-Encoding", "gzip, deflate, br");
        }

        // Upgrade-Insecure-Requests
        if !header_names.iter().any(|k| k.to_lowercase() == "upgrade-insecure-requests") {
            http_req = http_req.header("Upgrade-Insecure-Requests", "1");
        }

        // Send request
        let response = http_req.send().await
            .context(format!("Failed to fetch {}", request.url))?;

        Ok(response)
    }

    /// Extract headers from HTTP response
    fn extract_headers(&self, response: &reqwest::Response) -> Vec<(String, String)> {
        let mut headers = Vec::new();

        for (name, value) in response.headers() {
            // Filter out hop-by-hop headers
            let name_lower = name.as_str().to_lowercase();

            if matches!(
                name_lower.as_str(),
                "connection" | "keep-alive" | "transfer-encoding" |
                "upgrade" | "proxy-authenticate"
            ) {
                continue;
            }

            if let Ok(value_str) = value.to_str() {
                headers.push((name.as_str().to_string(), value_str.to_string()));
            }
        }

        headers
    }

    /// Cleanup expired sent trains (вызывается каждые 30 секунд)
    async fn cleanup_expired_trains(sent_trains: Arc<Mutex<HashMap<u64, SentTrain>>>) {
        let mut trains = sent_trains.lock().await;
        let before_count = trains.len();

        // Удаляем устаревшие trains
        trains.retain(|train_id, sent_train| {
            if sent_train.is_expired() {
                debug!("🧹 Cleaning up expired train #{}", train_id);
                false
            } else {
                true
            }
        });

        let after_count = trains.len();
        if before_count > after_count {
            info!("🧹 Cleaned up {} expired trains ({} → {})",
                 before_count - after_count, before_count, after_count);
        }
    }

    /// 🚂 Send response via YTP (DUAL-PATH с сохранением для NACK!)
    async fn send_response(&self, target_node: HashId, response: ProxyResponse) -> Result<()> {
        // ⚡ Сериализуем через bincode (в 3-5 раз быстрее JSON)
        let response_bytes = response.to_bincode()
            .map_err(|e| anyhow::anyhow!("Failed to serialize response: {}", e))?;

        info!("📤 Sending response #{} ({} MB, {} bytes)",
             response.request_id,
             response_bytes.len() / 1_000_000,
             response_bytes.len()
        );

        // Добавляем префикс ProxyResponse (0x41)
        let mut packet = vec![0x41u8];
        packet.extend_from_slice(&response_bytes);

        // 💾 СОХРАНЯЕМ packet для NACK fallback (до отправки!)
        let packet_clone = packet.clone();

        // ✅ Используем station.send_train() - DUAL-PATH теперь!
        let train_id = self.station.send_train(target_node, packet).await
            .map_err(|e| anyhow::anyhow!("Failed to send response: {}", e))?;

        // 💾 СОХРАНЯЕМ wagons для NACK fallback (если оба пути потеряли!)
        let wagon_size = crate::protocol::Wagon::PAYLOAD_SIZE;
        let mut sent_trains = self.sent_trains.lock().await;
        let sent_train = sent_trains.entry(train_id)
            .or_insert_with(|| SentTrain::new(train_id, target_node));

        // Разбиваем данные и сохраняем каждый wagon
        for (i, chunk) in packet_clone.chunks(wagon_size).enumerate() {
            // Создаём wagon для сериализации
            // Важно: используем те же параметры что и в Station::send_train
            let wagon = crate::protocol::Wagon::new(
                train_id,
                i as u32,
                crate::protocol::Train::calculate_wagon_count(&packet_clone) as u32,
                (i * wagon_size) as u64,
                chunk.to_vec(),
                0  // line_id не важен для storage, wagons одинаковые для обоих путей
            );

            match wagon.to_bytes() {
                Ok(wagon_bytes) => {
                    sent_train.add_wagon(i as u16, wagon_bytes);
                }
                Err(e) => {
                    warn!("⚠️  Failed to serialize wagon #{} for storage: {}", i, e);
                }
            }
        }

        info!("✅ Response sent via YTP DUAL-PATH! Train #{}", train_id);

        Ok(())
    }
}
