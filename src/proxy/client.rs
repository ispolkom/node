// src/proxy/client.rs
//! HTTP Proxy Client (Local Proxy)
//! ===============================
//!
//! Listens on localhost:8080 and forwards requests via P2P to gateway node

use crate::netlayer::P2PTransport;
use crate::proxy::{ProxyRequest, ProxyResponse, ProxyTunnelData, ProxyConfig, UrlMapper};
use crate::util::HashId;
use crate::protocol::{Station, WagonNack, NackReason};
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use std::io::ErrorKind;
use tokio::sync::{Mutex, RwLock};
use anyhow::{Result, Context};
use tracing::{info, error, debug, warn};

fn is_normal_tunnel_shutdown(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::NotConnected
    )
}

/// Helper function to send tunnel data to gateway
async fn send_tunnel_data_to_gateway(
    station: &Arc<Station>,
    gateway_node: HashId,
    tunnel_data: ProxyTunnelData,
) -> Result<()> {
    // ⚡ Используем bincode вместо JSON (в 3-5 раз быстрее)
    let data_bytes = tunnel_data.to_bincode()
        .map_err(|e| anyhow::anyhow!("Failed to serialize tunnel data: {}", e))?;

    // ✅ Используем ОДНУ И ТУ ЖЕ Station (не создаём новую!)
    station.send_train(gateway_node, data_bytes).await
        .map_err(|e| anyhow::anyhow!("Failed to send tunnel data: {}", e))?;

    Ok(())
}

/// Отправка данных туннеля шлюзу (прямо, станцией).
async fn send_tunnel_data_dispatch(
    station: &Arc<Station>,
    _transport: &Arc<P2PTransport>,
    gateway_node: HashId,
    tunnel_data: ProxyTunnelData,
) -> Result<()> {
    send_tunnel_data_to_gateway(station, gateway_node, tunnel_data).await
}

/// 🚂 Состояние сборки поезда на клиенте (DUAL-PATH с deduplication)
#[derive(Debug)]
struct TrainReassemblyState {
    /// ID поезда
    train_id: u64,

    /// Общее количество wagon-ов
    total_wagons: u16,

    /// 📦 Wagons с Path0 (deduplication)
    path0_wagons: HashMap<u16, Vec<u8>>,

    /// 📦 Wagons с Path1 (deduplication)
    path1_wagons: HashMap<u16, Vec<u8>>,

    /// ✅ Собранные wagon-ы (уже взяты из любого пути)
    assembled: HashSet<u16>,

    /// 📦 Полный train (собранный из данных wagon-ов)
    train_data: Vec<u8>,

    /// Время получения последнего wagon-а
    last_wagon_time: Instant,

    /// NACK уже отправлен?
    nack_sent: bool,

    /// Target node (откуда получили train)
    source_node: HashId,
}

impl TrainReassemblyState {
    /// Создать новое состояние сборки
    fn new(train_id: u64, total_wagons: u16, source_node: HashId) -> Self {
        Self {
            train_id,
            total_wagons,
            path0_wagons: HashMap::new(),
            path1_wagons: HashMap::new(),
            assembled: HashSet::new(),
            train_data: Vec::new(),
            last_wagon_time: Instant::now(),
            nack_sent: false,
            source_node,
        }
    }

    /// 🔄 Добавить wagon из определённого пути (с deduplication)
    fn add_wagon(&mut self, wagon_num: u16, wagon_data: Vec<u8>, path_id: u8) {
        // Сохраняем wagon в соответствующий путь
        match path_id {
            0 => {
                self.path0_wagons.insert(wagon_num, wagon_data);
            }
            1 => {
                self.path1_wagons.insert(wagon_num, wagon_data);
            }
            _ => {
                warn!("⚠️ Unknown path_id: {}", path_id);
                return;
            }
        }

        self.last_wagon_time = Instant::now();
    }

    /// 🔧 Пытается собрать train из обоих путей
    /// Возвращает true если train собран полностью
    fn try_assemble(&mut self) -> bool {
        let mut complete = true;
        let mut assembled_data = Vec::new();

        for wagon_num in 0..self.total_wagons {
            // ✅ Пропускаем уже собранные
            if self.assembled.contains(&wagon_num) {
                continue;
            }

            // 🔄 Priority: Path0 → Path1 → Missing
            let wagon_data = self.path0_wagons
                .get(&wagon_num)
                .or_else(|| self.path1_wagons.get(&wagon_num));

            if let Some(data) = wagon_data {
                // ✅ Собрали wagon из любого пути
                self.assembled.insert(wagon_num);
                assembled_data.extend_from_slice(data);
            } else {
                // ❌ Wagon missing в обоих путях
                complete = false;
            }
        }

        if complete {
            // ✅ TRAIN COMPLETE!
            self.train_data = assembled_data;
            true
        } else {
            false
        }
    }

    /// Отметить wagon как полученный (legacy, для совместимости)
    fn mark_received(&mut self, wagon_num: u16) {
        self.assembled.insert(wagon_num);
        self.last_wagon_time = Instant::now();
    }

    /// Проверить все ли wagon-ы получены
    fn is_complete(&self) -> bool {
        self.assembled.len() as u16 == self.total_wagons
    }

    /// Получить количество полученных wagon-ов
    fn received_count(&self) -> usize {
        self.assembled.len()
    }

    /// Получить список потерянных wagon-ов (нет ни в Path0, ни в Path1)
    fn missing_wagons(&self) -> Vec<u16> {
        let mut missing = Vec::new();
        for i in 0..self.total_wagons {
            if !self.assembled.contains(&i) {
                // Проверяем есть ли в любом пути
                let has_in_path0 = self.path0_wagons.contains_key(&i);
                let has_in_path1 = self.path1_wagons.contains_key(&i);

                if !has_in_path0 && !has_in_path1 {
                    missing.push(i);
                }
            }
        }
        missing
    }

    /// Получить собранные данные
    fn get_train_data(&self) -> Vec<u8> {
        self.train_data.clone()
    }

    /// Проверить нужно ли отправлять NACK
    fn should_send_nack(&self) -> bool {
        if self.nack_sent {
            return false; // Уже отправляли
        }

        let elapsed = self.last_wagon_time.elapsed();
        let received_pct = (self.received_count() as f64 / self.total_wagons as f64) * 100.0;

        // ⚡ Timeout: 1 секунда без новых wagon-ов (было 5 сек!)
        if elapsed > Duration::from_secs(1) {
            return true;
        }

        // ⚡ Threshold: получили 50%+ но не все (было 90%!)
        if received_pct >= 50.0 && !self.is_complete() {
            return true;
        }

        false
    }
}

/// HTTP Proxy Client (runs on client node)
///
/// Listens on localhost:8080 and forwards HTTP requests via P2P network
pub struct HttpProxyClient {
    transport: Arc<P2PTransport>,
    gateway_node: HashId,
    config: ProxyConfig,
    pending_requests: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<ProxyResponse>>>>,
    next_request_id: Arc<RwLock<u64>>,
    response_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>>>>,
    tunnel_data_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>>>>,
    /// Active tunnels: tunnel_id -> write half
    active_tunnels: Arc<Mutex<HashMap<u64, tokio::net::tcp::OwnedWriteHalf>>>,
    pub station: Arc<Station>,
    /// Состояния сборки поездов: train_id -> TrainReassemblyState
    train_reassembly: Arc<Mutex<HashMap<u64, TrainReassemblyState>>>,
}

impl HttpProxyClient {
    /// Create new HTTP proxy client
    pub fn new(transport: Arc<P2PTransport>, gateway_node: HashId) -> Self {
        let station = Station::with_defaults(
            transport.identity().node_id(),
            transport.clone()
        );

        Self {
            transport,
            gateway_node,
            config: ProxyConfig::default(),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            next_request_id: Arc::new(RwLock::new(1)),
            response_rx: Arc::new(Mutex::new(None)),
            tunnel_data_rx: Arc::new(Mutex::new(None)),
            active_tunnels: Arc::new(Mutex::new(HashMap::new())),
            station: Arc::new(station),
            train_reassembly: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set response channel
    pub fn with_response_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>) -> Self {
        self.response_rx = Arc::new(Mutex::new(Some(rx)));
        self
    }

    /// Set tunnel data channel
    pub fn with_tunnel_data_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>) -> Self {
        self.tunnel_data_rx = Arc::new(Mutex::new(Some(rx)));
        self
    }

    /// Set custom config
    pub fn with_config(mut self, config: ProxyConfig) -> Self {
        self.config = config;
        self
    }

    /// Start the proxy server
    pub async fn start(&self) -> Result<()> {
        let addr = &self.config.client_listen_addr;
        info!("🌐 HTTP Proxy Client listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .context(format!("Failed to bind to {}", addr))?;

        info!("✅ HTTP Proxy ready! Configure browser: HTTP proxy = {}", addr);
        info!("📡 Gateway node: {}", hex::encode(&self.gateway_node.0[..8]));

        // Subscribe to P2P packets for responses
        let transport_clone = self.transport.clone();
        let pending_clone = self.pending_requests.clone();
        let response_rx_clone = self.response_rx.clone();
        tokio::spawn(async move {
            Self::handle_responses(transport_clone, pending_clone, response_rx_clone).await;
        });

        // Subscribe to tunnel data packets
        let active_tunnels_clone = self.active_tunnels.clone();
        let tunnel_rx_clone = self.tunnel_data_rx.clone();
        tokio::spawn(async move {
            Self::handle_tunnel_data(active_tunnels_clone, tunnel_rx_clone).await;
        });

        // 🔄 Start NACK monitoring task (каждую секунду проверяет timeout)
        let reassembly_clone = self.train_reassembly.clone();
        let station_clone = self.station.clone(); // ✅ Передаём station вместо transport
        tokio::spawn(async move {
            Self::nack_monitor_task(reassembly_clone, station_clone).await;
        });

        // Accept incoming connections
        loop {
            match listener.accept().await {
                Ok((mut stream, client_addr)) => {
                    debug!("📥 New connection from {}", client_addr);

                    // Проверяем аутентификацию если настроен токен
                    if let Some(token) = &self.config.auth_token {
                        if !Self::authenticate_socket(&mut stream, token).await {
                            warn!("❌ Auth failed from {}", client_addr);
                            continue;
                        }
                        debug!("✅ Auth successful from {}", client_addr);
                    }

                    let client = self.clone_for_handler();
                    tokio::spawn(async move {
                        if let Err(e) = client.handle_connection(stream).await {
                            error!("❌ Error handling connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("❌ Error accepting connection: {}", e);
                }
            }
        }
    }

    /// Clone for handler (clone necessary fields)
    fn clone_for_handler(&self) -> Self {
        Self {
            transport: self.transport.clone(),
            gateway_node: self.gateway_node,
            config: self.config.clone(),
            pending_requests: self.pending_requests.clone(),
            next_request_id: self.next_request_id.clone(),
            response_rx: self.response_rx.clone(),
            tunnel_data_rx: self.tunnel_data_rx.clone(),
            active_tunnels: self.active_tunnels.clone(),
            station: self.station.clone(),
            train_reassembly: self.train_reassembly.clone(),
        }
    }

    /// Проверить аутентификацию сокета
    async fn authenticate_socket(socket: &mut tokio::net::TcpStream, token: &str) -> bool {
        use tokio::io::AsyncReadExt;
        use tokio::time::{timeout, Duration};

        // Читать первую строку (токен)
        let mut buf = vec![0u8; 1024];
        let read_result = timeout(Duration::from_secs(5), socket.read(&mut buf)).await;

        match read_result {
            Ok(Ok(n)) => {
                if n == 0 {
                    return false;
                }
                let client_token = String::from_utf8_lossy(&buf[..n]);
                let client_token = client_token.trim();
                client_token == token
            }
            _ => false,
        }
    }

    /// Handle single connection
    async fn handle_connection(&self, mut stream: tokio::net::TcpStream) -> Result<()> {
        // Read HTTP request
        let request_data = Self::read_http_request(&mut stream).await?;

        // Parse request line
        let request_line = Self::get_request_line(&request_data)?;
        debug!("📨 Request: {}", request_line);

        // Convert to real URL (now returns tuple: method, url)
        let (method, target_url) = UrlMapper::parse_request_line(&request_line)
            .context("Failed to parse target URL")?;

        debug!("🎯 Method: {}, URL: {}", method, target_url);

        // Check if this is CONNECT (HTTPS tunneling)
        if method == "CONNECT" {
            return self.handle_connect_tunnel(stream, target_url).await;
        }

        // Extract headers
        let headers = Self::parse_headers(&request_data)?;

        // Generate request ID
        let request_id = {
            let mut id = self.next_request_id.write().await;
            let current = *id;
            *id = id.wrapping_add(1);
            current
        };

        // Create oneshot channel for response
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Store pending request
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id, tx);
        }

        // Create proxy request
        let proxy_request = ProxyRequest {
            request_id,
            url: target_url.clone(),
            method,
            headers,
            body: Vec::new(),
        };

        // Serialize and send via YTP
        let request_bytes = proxy_request.to_bincode()
            .map_err(|e| anyhow::anyhow!("Failed to serialize request: {}", e))?;

        debug!("📤 Sending ProxyRequest via YTP to gateway ({} bytes)", request_bytes.len());

        // Отправляем как поезд через YTP!
        self.station.send_train(self.gateway_node, request_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to send YTP train: {}", e))?;

        debug!("✅ ProxyRequest sent via YTP");

        // Wait for response (with timeout)
        let timeout = tokio::time::Duration::from_secs(self.config.timeout_secs);

        let response = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                error!("❌ Response channel error: {}", e);
                return Err(anyhow::anyhow!("Response channel error"));
            }
            Err(_) => {
                warn!("⏱️  Request timeout");
                // Remove from pending
                let mut pending = self.pending_requests.lock().await;
                pending.remove(&request_id);
                return Err(anyhow::anyhow!("Request timeout"));
            }
        };

        // Send response back to browser
        Self::send_http_response(&mut stream, &response).await?;

        debug!("✅ Request completed");

        Ok(())
    }

    /// Read HTTP request from stream
    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;

        let mut buffer = vec![0u8; 64 * 1024]; // ⚡ 64 KB buffer for HTTP requests
        let mut total_read = 0;

        loop {
            let n = stream.read(&mut buffer[total_read..]).await
                .context("Failed to read from socket")?;

            if n == 0 {
                break;
            }

            total_read += n;

            // Check if we have complete headers (double CRLF)
            if total_read >= 4 {
                let window = &buffer[total_read.saturating_sub(4)..total_read];
                if window == b"\r\n\r\n" {
                    break;
                }
            }

            if total_read >= buffer.len() {
                break;
            }
        }

        buffer.truncate(total_read);
        Ok(buffer)
    }

    /// Extract request line from HTTP request
    fn get_request_line(request_data: &[u8]) -> Result<String> {
        let request_str = String::from_utf8_lossy(request_data);
        let first_line = request_str
            .lines()
            .next()
            .context("Empty request")?;

        Ok(first_line.to_string())
    }

    /// Parse HTTP headers
    fn parse_headers(request_data: &[u8]) -> Result<Vec<(String, String)>> {
        let request_str = String::from_utf8_lossy(request_data);
        let mut headers = Vec::new();

        for line in request_str.lines().skip(1) {
            if line.is_empty() {
                break;
            }

            if let Some(colon_pos) = line.find(':') {
                let name = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.push((name, value));
            }
        }

        Ok(headers)
    }

    /// Send HTTP response to browser
    async fn send_http_response(
        stream: &mut tokio::net::TcpStream,
        response: &ProxyResponse,
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        // Check if this is a CONNECT response (empty body, special status text)
        let is_connect = response.headers.iter()
            .any(|(k, v)| k.to_lowercase() == "connection" && v.to_lowercase() == "keep-alive")
            && response.body.is_empty();

        // Build HTTP response
        let status_text = if is_connect {
            "Connection Established"
        } else {
            Self::status_to_text(response.status)
        };

        let mut response_text = format!(
            "HTTP/1.1 {} {}\r\n",
            response.status,
            status_text
        );

        // Add headers
        for (name, value) in &response.headers {
            response_text.push_str(&format!("{}: {}\r\n", name, value));
        }

        response_text.push_str("\r\n");

        // Send headers
        stream.write_all(response_text.as_bytes()).await
            .context("Failed to send response headers")?;

        // Send body (skip for CONNECT)
        if !response.body.is_empty() {
            stream.write_all(&response.body).await
                .context("Failed to send response body")?;
        }

        Ok(())
    }

    /// Handle CONNECT tunnel (HTTPS)
    async fn handle_connect_tunnel(
        &self,
        mut stream: tokio::net::TcpStream,
        target_url: String,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        info!("🔐 Starting CONNECT tunnel to {}", target_url);

        // Generate request ID
        let request_id = {
            let mut id = self.next_request_id.write().await;
            let current = *id;
            *id = id.wrapping_add(1);
            current
        };

        // Create CONNECT request
        let proxy_request = ProxyRequest {
            request_id,
            url: target_url.clone(),
            method: "CONNECT".to_string(),
            headers: vec![],
            body: Vec::new(),
        };

        // Serialize request
        let request_bytes = proxy_request.to_bincode()
            .map_err(|e| anyhow::anyhow!("Failed to serialize CONNECT request: {}", e))?;

        // 🔄 IMPORTANT: Create oneshot and register in pending_requests BEFORE sending!
        // This prevents race condition where response arrives before we're ready to receive it.
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id, tx);
        }

        debug!("📤 Sending CONNECT request via YTP to gateway");

        // Now send the request (response handler is already registered)
        self.station.send_train(self.gateway_node, request_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to send CONNECT request: {}", e))?;

        debug!("✅ CONNECT request sent (response handler registered)");

        // Wait for 200 Connection Established response
        let timeout = tokio::time::Duration::from_secs(self.config.timeout_secs);
        let proxy_response = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                error!("❌ Response channel error: {}", e);
                return Err(anyhow::anyhow!("Response channel error"));
            }
            Err(_) => {
                warn!("⏱️  CONNECT request timeout");
                return Err(anyhow::anyhow!("CONNECT request timeout"));
            }
        };

        // Check if we got 200
        if proxy_response.status != 200 {
            error!("❌ CONNECT failed with status {}", proxy_response.status);
            Self::send_http_response(&mut stream, &proxy_response).await?;
            return Err(anyhow::anyhow!("CONNECT failed"));
        }

        // Send "200 Connection Established" to browser
        Self::send_http_response(&mut stream, &proxy_response).await?;
        info!("✅ Sent 200 Connection Established to browser");

        // Start bi-directional tunneling
        info!("🚇 Starting bi-directional tunneling");

        let station_clone = self.station.clone(); // ✅ Клонируем station вместо transport
        let transport_for_circuit = self.transport.clone();
        let gateway_node = self.gateway_node;
        let tunnel_id = request_id;

        // Split browser stream
        let (mut read_half, write_half) = stream.into_split();

        // Store write_half in active_tunnels
        {
            let mut tunnels = self.active_tunnels.lock().await;
            tunnels.insert(tunnel_id, write_half);
            info!("🚇 Tunnel #{}: write_half stored in active_tunnels", tunnel_id);
        }

        let active_tunnels_for_reader = self.active_tunnels.clone();

        // Task 1: Read from browser and send to gateway via YTP
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024]; // ⚡ 64 KB buffer for tunnel data
            loop {
                match read_half.read(&mut buf).await {
                    Ok(0) => {
                        info!("🚇 Tunnel #{}: Browser closed connection", tunnel_id);
                        {
                            let mut tunnels = active_tunnels_for_reader.lock().await;
                            tunnels.remove(&tunnel_id);
                        }
                        // Send close packet to gateway
                        let tunnel_data = ProxyTunnelData {
                            tunnel_id,
                            data: Vec::new(),
                            close: true,
                        };
                        let _ = send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, tunnel_data).await;
                        break;
                    }
                    Ok(n) => {
                        debug!("🚇 Tunnel #{}: Read {} bytes from browser", tunnel_id, n);

                        let tunnel_data = ProxyTunnelData {
                            tunnel_id,
                            data: buf[..n].to_vec(),
                            close: false,
                        };

                        if let Err(e) = send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, tunnel_data).await {
                            error!("❌ Failed to send tunnel data: {}", e);
                            {
                                let mut tunnels = active_tunnels_for_reader.lock().await;
                                tunnels.remove(&tunnel_id);
                            }
                            // ✅ FIX: Send close packet to gateway on error
                            let close_data = ProxyTunnelData {
                                tunnel_id,
                                data: Vec::new(),
                                close: true,
                            };
                            let _ = send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, close_data).await;
                            break;
                        }
                    }
                    Err(e) => {
                        error!("❌ Error reading from browser: {}", e);
                        {
                            let mut tunnels = active_tunnels_for_reader.lock().await;
                            tunnels.remove(&tunnel_id);
                        }
                        // ✅ FIX: Send close packet to gateway on read error
                        let tunnel_data = ProxyTunnelData {
                            tunnel_id,
                            data: Vec::new(),
                            close: true,
                        };
                        let _ = send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, tunnel_data).await;
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Convert status code to text
    fn status_to_text(status: u16) -> &'static str {
        match status {
            200 => "OK",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Unknown",
        }
    }

    /// Handle incoming tunnel data from gateway
    async fn handle_tunnel_data(
        active_tunnels: Arc<Mutex<HashMap<u64, tokio::net::tcp::OwnedWriteHalf>>>,
        tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>>>>,
    ) {
        info!("🚇 Tunnel data handler started");

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
        while let Some((_source_node, tunnel_data)) = rx.recv().await {
            let tunnel_id = tunnel_data.tunnel_id;
            let data_len = tunnel_data.data.len();
            let is_close = tunnel_data.close;

            debug!("🚇 Received tunnel data #{}: {} bytes, close={}",
                   tunnel_id, data_len, is_close);

            // ✅ FIX: Используем get_mut() вместо remove() чтобы устранить race condition!
            let mut tunnels = active_tunnels.lock().await;

            if let Some(write_half) = tunnels.get_mut(&tunnel_id) {
                use tokio::io::AsyncWriteExt;

                if is_close {
                    info!("🚇 Tunnel #{}: close packet received", tunnel_id);
                    // Close the connection to browser
                    let _ = write_half.shutdown().await;
                    // Remove tunnel from active list
                    tunnels.remove(&tunnel_id);
                    continue;
                }

                if !tunnel_data.data.is_empty() {
                    if let Err(e) = write_half.write_all(&tunnel_data.data).await {
                        if is_normal_tunnel_shutdown(&e) {
                            debug!("🚇 Tunnel #{}: browser side already closed ({})", tunnel_id, e);
                        } else {
                            error!("❌ Error writing to browser: {}", e);
                        }
                        let _ = write_half.shutdown().await;
                        tunnels.remove(&tunnel_id);
                        continue;
                    }

                    debug!("✅ Tunnel #{}: wrote {} bytes to browser", tunnel_id, data_len);

                    // Flush to ensure data is sent
                    if let Err(e) = write_half.flush().await {
                        if is_normal_tunnel_shutdown(&e) {
                            debug!("🚇 Tunnel #{}: browser flush skipped, socket closed ({})", tunnel_id, e);
                        } else {
                            error!("❌ Error flushing browser tunnel #{}: {}", tunnel_id, e);
                        }
                        let _ = write_half.shutdown().await;
                        tunnels.remove(&tunnel_id);
                        continue;
                    }
                }
            } else {
                debug!("🚇 Tunnel #{}: data arrived after local close, dropping", tunnel_id);
            }
        }

        warn!("⚠️  Tunnel data channel closed");
    }

    /// Handle incoming P2P responses
    async fn handle_responses(
        _transport: Arc<P2PTransport>,
        pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<ProxyResponse>>>>,
        response_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>>>>,
    ) {
        info!("📡 P2P response handler started");

        // Get the receiver - take ownership
        let mut rx_opt = {
            let mut rx_lock = response_rx.lock().await;
            rx_lock.take()
        };

        let mut rx = match rx_opt {
            Some(r) => r,
            None => {
                warn!("⚠️  No response channel configured!");
                return;
            }
        };

        // Listen for responses
        while let Some((_source_node, response)) = rx.recv().await {
            let request_id = response.request_id;
            let status = response.status;
            let body_len = response.body.len();

            debug!("📨 Received ProxyResponse #{}: {} {} bytes",
                   request_id, status, body_len);

            // Find pending request
            let sender = {
                let mut pending_lock = pending.lock().await;
                pending_lock.remove(&request_id)
            };

            if let Some(tx) = sender {
                // Send response to waiting request
                if tx.send(response).is_err() {
                    error!("❌ Failed to send response to waiting request");
                } else {
                    debug!("✅ Response delivered to request #{}", request_id);
                }
            } else {
                warn!("⚠️  No pending request found for response #{}", request_id);
            }
        }
    }

    /// 🔄 NACK Monitor Task - проверяет все собираемые поезда каждую секунду
    /// и отправляет NACK если нужно
    async fn nack_monitor_task(
        train_reassembly: Arc<Mutex<HashMap<u64, TrainReassemblyState>>>,
        station: Arc<Station>,  // ✅ Используем Station вместо Transport
    ) {
        use tokio::time::interval;
        use std::time::Duration;

        let mut ticker = interval(Duration::from_secs(1));

        loop {
            ticker.tick().await;

            let mut reassembly = train_reassembly.lock().await;

            // Проверяем каждый train
            let mut trains_to_remove = Vec::new();

            for (train_id, state) in reassembly.iter_mut() {
                // Проверяем нужно ли отправлять NACK
                if state.should_send_nack() {
                    let missing = state.missing_wagons();

                    if !missing.is_empty() {
                        info!("🔄 Sending NACK for train #{}: {} missing wagons (got {}/{})",
                             train_id,
                             missing.len(),
                             state.received_count(),
                             state.total_wagons);

                        // Создаём NACK
                        let elapsed = state.last_wagon_time.elapsed();
                        let reason = if elapsed > Duration::from_secs(1) { // ⚡ Было 5 сек!
                            NackReason::Timeout
                        } else {
                            NackReason::Threshold
                        };

                        let nack = WagonNack::new(*train_id, missing.clone(), reason);

                        // ✅ Используем переданную station (не создаём новую!)
                        // Сериализуем NACK с префиксом 0x62 (NACK packet type)
                        let nack_bytes = serde_json::to_vec(&nack).unwrap_or_default();
                        let mut packet = vec![0x62u8]; // Префикс NACK
                        packet.extend_from_slice(&nack_bytes);

                        // Отправляем source node
                        if let Err(e) = station.send_train(state.source_node, packet).await {
                            error!("❌ Failed to send NACK for train #{}: {}", train_id, e);
                        } else {
                            info!("✅ NACK sent for train #{} (missing: {:?})", train_id,
                                 &missing[..missing.len().min(10)]); // Показываем первые 10
                        }

                        // Помечаем что NACK отправлен
                        state.nack_sent = true;
                    }
                }

                // Если поезд собран полностью - удаляем через 5 секунд
                if state.is_complete() && state.last_wagon_time.elapsed() > Duration::from_secs(5) {
                    trains_to_remove.push(*train_id);
                }

                // Если поезд устарел (60 секунд) - удаляем
                if state.last_wagon_time.elapsed() > Duration::from_secs(60) {
                    warn!("⏱️  Train #{} timeout after 60s, removing", train_id);
                    trains_to_remove.push(*train_id);
                }
            }

            // Удаляем завершённые поезда
            for train_id in trains_to_remove {
                reassembly.remove(&train_id);
                debug!("🧹 Removed completed/expired train #{}", train_id);
            }
        }
    }
}
