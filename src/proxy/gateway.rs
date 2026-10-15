// src/proxy/gateway.rs
//! HTTP Proxy Gateway (Reverse Proxy)
//! ==================================
//!
//! Receives proxy requests via P2P and makes real HTTPS requests to target servers

use crate::netlayer::P2PTransport;
use crate::proxy::{ProxyRequest, ProxyResponse, ProxyTunnelData};
use crate::util::HashId;
use crate::protocol::Station;
use std::sync::Arc;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use anyhow::{Result, Context};
use tracing::{info, error, debug, warn};
use reqwest::Client as HttpClient;

/// Пределы выхода в интернет: одновременных запросов всего и от одного узла, туннелей от одного узла, размер ответа в памяти.
const MAX_INFLIGHT_TOTAL: usize = 128;
const MAX_INFLIGHT_PER_PEER: usize = 16;
const MAX_TUNNELS_PER_PEER: usize = 32;
const MAX_RESPONSE_BODY: usize = 8 * 1024 * 1024;

static INFLIGHT: std::sync::Mutex<Option<HashMap<HashId, usize>>> = std::sync::Mutex::new(None);

/// Место в очереди выхода: занято до конца обработки запроса. None — узел или весь выход загружены.
struct InflightSlot(HashId);

impl InflightSlot {
    fn take(peer: HashId) -> Option<Self> {
        let mut g = INFLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        let total: usize = m.values().sum();
        let mine = m.entry(peer).or_insert(0);
        if total >= MAX_INFLIGHT_TOTAL || *mine >= MAX_INFLIGHT_PER_PEER {
            if *mine == 0 {
                m.remove(&peer);
            }
            return None;
        }
        *mine += 1;
        Some(Self(peer))
    }
}

impl Drop for InflightSlot {
    fn drop(&mut self) {
        let mut g = INFLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = g.as_mut() {
            if let Some(n) = m.get_mut(&self.0) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    m.remove(&self.0);
                }
            }
        }
    }
}

/// Helper function to send tunnel data
async fn send_tunnel_data(
    station: &Arc<Station>,
    target_node: HashId,
    tunnel_data: ProxyTunnelData,
) -> Result<()> {
    let data_bytes = tunnel_data.encode();

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
    /// Active tunnels: tunnel_id -> bounded writer queue
    active_tunnels: Arc<Mutex<HashMap<(HashId, u64), crate::netlayer::tunnel_writer::TunnelWriter>>>,
    /// CONNECT, который уже принят, но туннель ещё не создан (идёт соединение): учитывается в квоте, иначе параллельные запросы
    /// все видят «свободно» до await и превышают предел или заменяют друг друга
    connecting: Arc<std::sync::Mutex<std::collections::HashSet<(HashId, u64)>>>,
}

/// Бронь места под CONNECT; снимается при выходе из функции.
struct ConnectSlot {
    set: Arc<std::sync::Mutex<std::collections::HashSet<(HashId, u64)>>>,
    key: (HashId, u64),
}

impl Drop for ConnectSlot {
    fn drop(&mut self) {
        self.set.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
    }
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
            connecting: Arc::new(std::sync::Mutex::new(Default::default())),
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
            let Some(slot) = InflightSlot::take(source_node) else {
                warn!("⚠️  Proxy request #{} dropped: too many in flight from {}", request.request_id, hex::encode(&source_node.0[..8]));
                let busy = ProxyResponse { request_id: request.request_id, status: 503, headers: vec![], body: b"exit busy".to_vec() };
                let _ = gateway.send_response(source_node, busy).await;
                continue;
            };

            tokio::spawn(async move {
                let _slot = slot;
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
            connecting: self.connecting.clone(),
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
        let mut http_response = self.make_http_request(&request).await?;

        // тело читаем кусками и не больше предела: ответ не раздувает память узла
        let status = http_response.status().as_u16();
        let headers = self.extract_headers(&http_response);
        let mut body = Vec::new();
        while let Some(chunk) = http_response.chunk().await? {
            if body.len() + chunk.len() > MAX_RESPONSE_BODY {
                let big = ProxyResponse { request_id: request.request_id, status: 502, headers: vec![], body: b"response too large".to_vec() };
                self.send_response(source_node, big).await?;
                return Ok(());
            }
            body.extend_from_slice(&chunk);
        }
        let proxy_response = ProxyResponse { request_id: request.request_id, status, headers, body };

        debug!("✅ Response: {} {} bytes", proxy_response.status, proxy_response.body.len());

        // Send response via P2P
        self.send_response(source_node, proxy_response).await?;

        Ok(())
    }

    /// Handle CONNECT request - establish TCP tunnel
    async fn handle_connect(&self, source_node: HashId, request: ProxyRequest) -> Result<()> {
        use tokio::io::AsyncReadExt;

        // Parse host:port from URL (format: "goodwin.su:443" or "example.com:443")
        let addr_str = request.url.clone();

        // туннелей от одного узла не больше предела; повторный номер поверх живого туннеля не принимается
        let key = (source_node, request.request_id);
        let reserved = {
            let tunnels = self.active_tunnels.lock().await;
            let mut pending = self.connecting.lock().unwrap_or_else(|e| e.into_inner());
            let mine = tunnels.keys().filter(|(n, _)| *n == source_node).count() + pending.iter().filter(|(n, _)| *n == source_node).count();
            if mine >= MAX_TUNNELS_PER_PEER || tunnels.contains_key(&key) || pending.contains(&key) {
                false
            } else {
                pending.insert(key);
                true
            }
        };
        if !reserved {
            let busy = ProxyResponse { request_id: request.request_id, status: 429, headers: vec![], body: Vec::new() };
            self.send_response(source_node, busy).await?;
            return Ok(());
        }
        let slot = ConnectSlot { set: self.connecting.clone(), key };

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

        // Приёмник данных клиента (writer) публикуется ДО подтверждения: клиент начинает передачу сразу после 200, и первые байты
        // не должны находить пустую таблицу
        let tunnel_id = request.request_id;
        let transport = self.transport.clone();
        let source_node_clone = source_node;

        // Split TCP stream
        let (mut read_half, write_half) = target_stream.into_split();

        let mut writer_closed;
        {
            let mut tunnels = self.active_tunnels.lock().await;
            // соединения разных узлов — раздельно: номер туннеля выбирает сам узел, чужой номер не даёт доступа к чужому соединению
            let credit = crate::netlayer::flow::credit_sender(self.station.clone(), crate::netlayer::tunnel_wire::HTTP_CONNECT, source_node, tunnel_id);
            let writer = crate::netlayer::tunnel_writer::spawn_stream_writer_credit(write_half, self.active_tunnels.clone(), (source_node, tunnel_id), Some(credit));
            writer_closed = writer.subscribe();
            tunnels.insert((source_node, tunnel_id), writer);
            drop(slot); // бронь снята: место теперь занято самим туннелем
            info!("🚇 Tunnel #{}: write_half stored in active_tunnels", tunnel_id);
        }

        // Send "200 Connection Established" back to client
        let proxy_response = ProxyResponse {
            request_id: request.request_id,
            status: 200,
            headers: vec![
                ("Connection".to_string(), "keep-alive".to_string()),
            ],
            body: Vec::new(),
        };

        if let Err(e) = self.send_response(source_node, proxy_response).await {
            // ответ не ушёл: туннель никому не нужен, writer снимается (иначе занимал бы место до простоя)
            self.active_tunnels.lock().await.remove(&(source_node, tunnel_id));
            return Err(e);
        }
        info!("📤 Sent 200 Connection Established");

        info!("🚇 Starting tunnel #{} bi-directional forwarding", tunnel_id);

        let active_tunnels_for_reader = self.active_tunnels.clone();

        // Task: Read from target and send to client via YTP
        let transport_to_client = transport.clone();
        let station_to_client = self.station.clone(); // ✅ Добавляем!
        let flow = crate::netlayer::flow::register(crate::netlayer::tunnel_wire::HTTP_CONNECT, source_node, tunnel_id);
        tokio::spawn(async move {
            let mut cursor = crate::netlayer::stream_order::Cursor::new();
            let mut buf = vec![0u8; 64 * 1024]; // ⚡ 64 KB buffer for tunnel data
            loop {
                match tokio::select! {
                    _ = writer_closed.changed() => Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "tunnel writer closed")),
                    result = read_half.read(&mut buf) => result,
                } {
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
                            byte_offset: cursor.end(),
                        };
                        let _ = tokio::time::timeout(Duration::from_secs(2), send_tunnel_data(&station_to_client, source_node_clone, tunnel_data)).await;
                        break;
                    }
                    Ok(n) => {
                        debug!("🚇 Tunnel #{}: Read {} bytes from server", tunnel_id, n);

                        let Ok(byte_offset) = cursor.block(n) else {
                            active_tunnels_for_reader.lock().await.remove(&(source_node_clone, tunnel_id));
                            break;
                        };
                        // регулятор скорости получателя: не читаем дальше, пока клиент не готов принять
                        let waited = tokio::select! {
                            _ = writer_closed.changed() => Err(()),
                            r = flow.window.wait_until(byte_offset + n as u64, crate::netlayer::flow::stall_timeout()) => r,
                        };
                        if waited.is_err() {
                            error!("❌ Tunnel #{}: receiver window stalled or tunnel closed", tunnel_id);
                            active_tunnels_for_reader.lock().await.remove(&(source_node_clone, tunnel_id));
                            break;
                        }
                        let tunnel_data = ProxyTunnelData {
                            byte_offset,
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
                                byte_offset: cursor.end(),
                            };
                            let _ = tokio::time::timeout(Duration::from_secs(2), send_tunnel_data(&station_to_client, source_node_clone, close_data)).await;
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
                            byte_offset: cursor.end(),
                        };
                        let _ = tokio::time::timeout(Duration::from_secs(2), send_tunnel_data(&station_to_client, source_node_clone, tunnel_data)).await;
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Handle incoming tunnel data from client (async method - same as client's)
    async fn handle_tunnel_data(
        active_tunnels: Arc<Mutex<HashMap<(HashId, u64), crate::netlayer::tunnel_writer::TunnelWriter>>>,
        tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>>>>,
    ) {
        let rx = tunnel_rx.lock().await.take();
        if let Some(mut rx) = rx {
            while let Some((source_node, tunnel_data)) = rx.recv().await {
                let key = (source_node, tunnel_data.tunnel_id);
                let mut tunnels = active_tunnels.lock().await;
                if let Some(writer) = tunnels.get(&key) {
                    // Admission is synchronous. A stalled TCP writer can exhaust only its own queue.
                    if writer.enqueue_offset(tunnel_data.data, tunnel_data.close, tunnel_data.byte_offset).is_err() {
                        warn!("[rx] tunnel #{} closed: delivery queue exhausted", tunnel_data.tunnel_id);
                        tunnels.remove(&key); // abort writer and wake paired reader
                    }
                }
            }
        }
    }

    /// Send tunnel data packet via YTP
    async fn send_tunnel_data_packet(
        &self,
        target_node: HashId,
        tunnel_data: ProxyTunnelData,
    ) -> Result<()> {
        let data_bytes = tunnel_data.encode();

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

    /// 🚂 Send response via YTP (DUAL-PATH)
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

        // ✅ Используем station.send_train() - DUAL-PATH теперь!
        let train_id = self.station.send_train(target_node, packet).await
            .map_err(|e| anyhow::anyhow!("Failed to send response: {}", e))?;

        info!("✅ Response sent via YTP DUAL-PATH! Train #{}", train_id);

        Ok(())
    }
}

#[cfg(test)]
mod limits_tests {
    use super::*;

    #[test]
    fn inflight_slots_are_limited_per_peer_and_released() {
        let a = HashId([0xA1; 32]);
        let b = HashId([0xB2; 32]);
        let held: Vec<_> = (0..MAX_INFLIGHT_PER_PEER).map(|_| InflightSlot::take(a).expect("в пределах")).collect();
        assert!(InflightSlot::take(a).is_none(), "сверх предела узла — отказ");
        assert!(InflightSlot::take(b).is_some(), "другой узел не страдает");
        drop(held);
        assert!(InflightSlot::take(a).is_some(), "после завершения место свободно");
    }
}
