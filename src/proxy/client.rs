// src/proxy/client.rs
//! HTTP Proxy Client (Local Proxy)
//! ===============================
//!
//! Listens on localhost:8080 and forwards requests via P2P to gateway node

use crate::netlayer::P2PTransport;
use crate::proxy::{ProxyRequest, ProxyResponse, ProxyTunnelData, ProxyConfig, UrlMapper};
use crate::util::HashId;
use crate::protocol::Station;
use std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;
use tokio::sync::{Mutex, RwLock};
use anyhow::{Result, Context};
use tracing::{info, error, debug, warn};

/// Helper function to send tunnel data to gateway
async fn send_tunnel_data_to_gateway(
    station: &Arc<Station>,
    gateway_node: HashId,
    tunnel_data: ProxyTunnelData,
) -> Result<()> {
    let data_bytes = tunnel_data.encode();

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
    /// Active tunnels: tunnel_id -> bounded writer queue
    active_tunnels: Arc<Mutex<HashMap<u64, crate::netlayer::tunnel_writer::TunnelWriter>>>,
    /// Туннели в процессе установки: ранние данные шлюза ждут здесь публикации приёмника (см. `early_tunnel`)
    establishing: Arc<super::early_tunnel::EarlyTunnels>,
    pub station: Arc<Station>,
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
            establishing: super::early_tunnel::EarlyTunnels::new(),
            station: Arc::new(station),
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
        let gateway_for_responses = self.gateway_node;
        tokio::spawn(async move {
            Self::handle_responses(transport_clone, pending_clone, response_rx_clone, gateway_for_responses).await;
        });

        // Subscribe to tunnel data packets
        let active_tunnels_clone = self.active_tunnels.clone();
        let tunnel_rx_clone = self.tunnel_data_rx.clone();
        let gateway_for_tunnels = self.gateway_node;
        let establishing_clone = self.establishing.clone();
        tokio::spawn(async move {
            Self::handle_tunnel_data(active_tunnels_clone, tunnel_rx_clone, gateway_for_tunnels, establishing_clone).await;
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
            establishing: self.establishing.clone(),
            station: self.station.clone(),
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
            body: Self::header_end(&request_data).map(|e| request_data[e..].to_vec()).unwrap_or_default(),
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

    /// Границы тела запроса: сколько байт занимает заголовок вместе с пустой строкой.
    fn header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
    }

    /// Заявленная длина тела (`Content-Length`) в заголовке запроса.
    fn content_length(head: &[u8]) -> usize {
        String::from_utf8_lossy(head)
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
            .unwrap_or(0)
    }

    /// Read HTTP request from stream: заголовок целиком (граница ищется во всём прочитанном, а не только в последних четырёх
    /// байтах) и тело по `Content-Length` (не больше MAX_REQUEST_BODY). Тело без Content-Length (chunked) не поддерживается.
    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        const MAX_HEAD: usize = 64 * 1024;
        const MAX_REQUEST_BODY: usize = 1 << 20;

        let mut buffer: Vec<u8> = Vec::with_capacity(8 * 1024);
        let mut chunk = [0u8; 16 * 1024];
        let mut want: Option<usize> = None; // сколько всего байт нужно (заголовок + тело)

        loop {
            if let Some(w) = want {
                if buffer.len() >= w {
                    break;
                }
            }
            let n = stream.read(&mut chunk).await.context("Failed to read from socket")?;
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
            if want.is_none() {
                if let Some(end) = Self::header_end(&buffer) {
                    let len = Self::content_length(&buffer[..end]);
                    if len > MAX_REQUEST_BODY {
                        return Err(anyhow::anyhow!("request body too large"));
                    }
                    want = Some(end + len);
                } else if buffer.len() > MAX_HEAD {
                    return Err(anyhow::anyhow!("request header too large"));
                }
            }
        }

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

        // отметка до отправки: ответ шлюза и его первые данные могут прийти раньше, чем будет опубликован приёмник
        let _establishing = self.establishing.begin(request_id);

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
        let mut writer_closed;
        {
            let mut tunnels = self.active_tunnels.lock().await;
            let credit = crate::netlayer::flow::credit_sender(self.station.clone(), crate::netlayer::tunnel_wire::HTTP_CONNECT, gateway_node, tunnel_id);
            let writer = crate::netlayer::tunnel_writer::spawn_stream_writer_credit(write_half, self.active_tunnels.clone(), tunnel_id, Some(credit));
            writer_closed = writer.subscribe();
            // данные, пришедшие до публикации, передаются приёмнику по порядку (снимаем отметку под замком таблицы)
            for (data, close, offset) in self.establishing.take(tunnel_id) {
                if writer.enqueue_offset(data, close, offset).is_err() {
                    break;
                }
            }
            tunnels.insert(tunnel_id, writer);
            info!("🚇 Tunnel #{}: write_half stored in active_tunnels", tunnel_id);
        }

        let active_tunnels_for_reader = self.active_tunnels.clone();
        let flow = crate::netlayer::flow::register(crate::netlayer::tunnel_wire::HTTP_CONNECT, gateway_node, tunnel_id);

        // Task 1: Read from browser and send to gateway via YTP
        tokio::spawn(async move {
            let mut cursor = crate::netlayer::stream_order::Cursor::new();
            let mut buf = vec![0u8; 64 * 1024]; // ⚡ 64 KB buffer for tunnel data
            loop {
                match tokio::select! {
                    _ = writer_closed.changed() => Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "tunnel writer closed")),
                    result = read_half.read(&mut buf) => result,
                } {
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
                            byte_offset: cursor.end(),
                        };
                        let _ = tokio::time::timeout(Duration::from_secs(2), send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, tunnel_data)).await;
                        break;
                    }
                    Ok(n) => {
                        debug!("🚇 Tunnel #{}: Read {} bytes from browser", tunnel_id, n);

                        let Ok(byte_offset) = cursor.block(n) else {
                            active_tunnels_for_reader.lock().await.remove(&tunnel_id);
                            break;
                        };
                        // регулятор скорости получателя: не читаем дальше браузера, пока шлюз не готов принять
                        let waited = tokio::select! {
                            _ = writer_closed.changed() => Err(()),
                            r = flow.window.wait_until(byte_offset + n as u64, crate::netlayer::flow::stall_timeout()) => r,
                        };
                        if waited.is_err() {
                            error!("❌ Tunnel #{}: receiver window stalled or tunnel closed", tunnel_id);
                            active_tunnels_for_reader.lock().await.remove(&tunnel_id);
                            break;
                        }
                        let tunnel_data = ProxyTunnelData {
                            byte_offset,
                            tunnel_id,
                            data: buf[..n].to_vec(),
                            close: false,
                        };

                        if let Err(e) = tokio::select! {
                            _ = writer_closed.changed() => Err(anyhow::anyhow!("tunnel writer closed")),
                            result = send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, tunnel_data) => result,
                        } {
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
                                byte_offset: cursor.end(),
                            };
                            let _ = tokio::time::timeout(Duration::from_secs(2), send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, close_data)).await;
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
                            byte_offset: cursor.end(),
                        };
                        let _ = tokio::time::timeout(Duration::from_secs(2), send_tunnel_data_dispatch(&station_clone, &transport_for_circuit, gateway_node, tunnel_data)).await;
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
        active_tunnels: Arc<Mutex<HashMap<u64, crate::netlayer::tunnel_writer::TunnelWriter>>>,
        tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyTunnelData)>>>>,
        gateway: HashId,
        establishing: Arc<super::early_tunnel::EarlyTunnels>,
    ) {
        let rx = tunnel_rx.lock().await.take();
        if let Some(mut rx) = rx {
            while let Some((source_node, tunnel_data)) = rx.recv().await {
                if source_node != gateway {
                    warn!("[rx] tunnel #{}: данные не от шлюза, отброшены", tunnel_data.tunnel_id);
                    continue;
                }
                let key = tunnel_data.tunnel_id;
                let mut tunnels = active_tunnels.lock().await;
                if let Some(writer) = tunnels.get(&key) {
                    // Admission is synchronous. A stalled TCP writer can exhaust only its own queue.
                    if writer.enqueue_offset(tunnel_data.data, tunnel_data.close, tunnel_data.byte_offset).is_err() {
                        warn!("[rx] tunnel #{} closed: delivery queue exhausted", tunnel_data.tunnel_id);
                        tunnels.remove(&key); // abort writer and wake paired reader
                    }
                } else {
                    // туннель ещё устанавливается: данные ждут публикации приёмника (проверка — под замком таблицы, как и
                    // публикация, поэтому между ними кусок не потеряется)
                    establishing.offer(key, tunnel_data.data, tunnel_data.close, tunnel_data.byte_offset);
                }
            }
        }
    }

    /// Handle incoming P2P responses
    async fn handle_responses(
        _transport: Arc<P2PTransport>,
        pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<ProxyResponse>>>>,
        response_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>>>>,
        gateway: HashId,
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
        while let Some((source_node, response)) = rx.recv().await {
            if source_node != gateway {
                // ответ принимается только от шлюза, которому ушёл запрос: иначе любой узел подставит чужой ответ по номеру
                warn!("⚠️  ProxyResponse #{} не от шлюза, отброшен", response.request_id);
                continue;
            }
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

}

#[cfg(test)]
mod request_reading_tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    async fn read_from(parts: Vec<Vec<u8>>) -> Result<Vec<u8>> {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::task::spawn(async move {
            let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
            for p in parts {
                c.write_all(&p).await.unwrap();
                c.flush().await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        });
        let (mut s, _) = l.accept().await.unwrap();
        HttpProxyClient::read_http_request(&mut s).await
    }

    #[tokio::test]
    async fn the_body_of_a_post_is_read_whether_or_not_it_came_with_the_headers() {
        let head = b"POST http://example.org/x HTTP/1.1\r\nHost: example.org\r\nContent-Length: 11\r\n\r\n".to_vec();
        // заголовок и тело одним куском
        let all = [head.clone(), b"hello world".to_vec()].concat();
        let got = read_from(vec![all.clone()]).await.unwrap();
        assert_eq!(got, all);
        // заголовок отдельно, тело отдельно
        let got = read_from(vec![head.clone(), b"hello ".to_vec(), b"world".to_vec()]).await.unwrap();
        assert_eq!(got, all);
        let e = HttpProxyClient::header_end(&got).unwrap();
        assert_eq!(&got[e..], b"hello world");
    }
}

#[cfg(test)]
mod early_tunnel_data_tests {
    use super::*;

    #[tokio::test]
    async fn data_that_arrives_while_a_tunnel_is_establishing_is_kept_and_unknown_tunnels_are_dropped() {
        let gateway = HashId([9u8; 32]);
        let active: Arc<Mutex<HashMap<u64, crate::netlayer::tunnel_writer::TunnelWriter>>> = Arc::new(Mutex::new(HashMap::new()));
        let establishing = super::super::early_tunnel::EarlyTunnels::new();
        let _g = establishing.begin(5);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let rx = Arc::new(Mutex::new(Some(rx)));
        tokio::task::spawn(HttpProxyClient::handle_tunnel_data(active, rx, gateway, establishing.clone()));
        for off in 0..3u64 {
            tx.send((gateway, ProxyTunnelData { tunnel_id: 5, data: vec![off as u8; 10], close: false, byte_offset: off * 10 })).await.unwrap();
        }
        tx.send((gateway, ProxyTunnelData { tunnel_id: 6, data: vec![1; 10], close: false, byte_offset: 0 })).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!establishing.is_establishing(6), "неизвестный туннель ничего не занимает");
        let got = establishing.take(5);
        assert_eq!(got.len(), 3, "три куска ждали публикации приёмника");
        assert_eq!(got[2].2, 20, "порядок сохранён");
    }
}
