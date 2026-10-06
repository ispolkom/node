// src/protocol/tcp_tunnel.rs
//! Raw TCP Tunnel over HTTP CONNECT
//! =================================
//!
//! Упаковывает raw TCP bytes прямо в wagon БЕЗ JSON и SOCKS5

use std::sync::Arc;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};
use anyhow::{Result, anyhow};

use crate::protocol::Station;
use crate::netlayer::P2PTransport;
use crate::util::HashId;
use tracing::{info, error, debug, warn};

/// Connection ID (уникальный для каждого TCP соединения)
type ConnectionId = u64;

/// Raw TCP Tunnel - упаковывает байты прямо в wagon
pub struct TcpTunnel {
    /// Station для отправки wagon и получения ответов через callback
    station: Arc<Station>,

    /// Transport для P2P коммуникации
    transport: Arc<P2PTransport>,

    /// Exit node (куда отправлять в интернет)
    exit_node: HashId,

    /// Порт для прослушивания
    port: u16,

    /// Активные соединения: connection_id -> response_tx
    /// 🚇 Глобальная мапа для маршрутизации ответов от callback
    active_connections: Arc<Mutex<HashMap<ConnectionId, mpsc::Sender<Vec<u8>>>>>,

    /// Следующий connection_id
    next_connection_id: Arc<Mutex<ConnectionId>>,

    /// Callback уже установлен?
    callback_set: Arc<Mutex<bool>>,
}

impl TcpTunnel {
    /// Создать новый TCP tunnel
    pub fn new(transport: Arc<P2PTransport>, exit_node: HashId, port: u16) -> Self {
        info!("🚇 Creating TCP Tunnel on port {} (exit node: {})",
              port, hex::encode(&exit_node.0[..8]));

        // Создаем собственную Station (как в Socks5ProxyServer)
        let station = Station::with_defaults(
            transport.identity().node_id(),
            transport.clone()
        );

        Self {
            station: Arc::new(station),
            transport,
            exit_node,
            port,
            active_connections: Arc::new(Mutex::new(HashMap::new())),
            next_connection_id: Arc::new(Mutex::new(1)),
            callback_set: Arc::new(Mutex::new(false)),
        }
    }

    /// Запустить TCP tunnel
    pub async fn run(&self) -> Result<()> {
        // 🚇 Устанавливаем глобальный callback ОДИН РАЗ
        {
            let mut set = self.callback_set.lock().await;
            if !*set {
                self.setup_global_callback().await;
                *set = true;
            }
        }

        let addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&addr).await?;

        info!("✅ TCP Tunnel listening on http://{}", addr);
        info!("📡 Configure browser: HTTP Proxy = {}", addr);

        loop {
            match listener.accept().await {
                Ok((stream, client_addr)) => {
                    debug!("📥 New TCP connection from {}", client_addr);

                    let tunnel = self.clone_for_handler();
                    tokio::spawn(async move {
                        if let Err(e) = tunnel.handle_connection(stream).await {
                            error!("❌ Error handling TCP client {}: {}", client_addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("❌ Error accepting TCP connection: {}", e);
                }
            }
        }
    }

    /// 🚇 Установить глобальный callback для маршрутизации ответов
    async fn setup_global_callback(&self) {
        let connections = self.active_connections.clone();
        let station = self.station.clone();

        station.set_data_callback(move |source_id: HashId, raw_bytes: Vec<u8>| {
            debug!("📨 Global callback received {} bytes from {}",
                   raw_bytes.len(), hex::encode(&source_id.0[..8]));

            // 🔑 Используем source_id как connection_id
            let conn_id = connection_id_from_hash_id(source_id);

            // Ищем соединение в HashMap
            let connections_clone = connections.clone();
            tokio::spawn(async move {
                let conns = connections_clone.lock().await;
                if let Some(tx) = conns.get(&conn_id) {
                    debug!("📤 Routing to connection #{}", conn_id);
                    tx.blocking_send(raw_bytes).ok();
                } else {
                    warn!("⚠️  No connection found for source_id {} (conn_id: {})",
                          hex::encode(&source_id.0[..8]), conn_id);
                }
            });
        }).await;

        info!("✅ Global callback registered for TCP Tunnel");
    }

    /// Clone for handler
    fn clone_for_handler(&self) -> Self {
        Self {
            station: self.station.clone(),
            transport: self.transport.clone(),
            exit_node: self.exit_node,
            port: self.port,
            active_connections: self.active_connections.clone(),
            next_connection_id: self.next_connection_id.clone(),
            callback_set: self.callback_set.clone(),
        }
    }

    /// Обработать одно соединение с двунаправленным туннелем
    async fn handle_connection(&self, mut stream: TcpStream) -> Result<()> {
        // Читаем HTTP request
        let mut buffer = vec![0u8; 4096];
        let n = stream.read(&mut buffer).await?;
        let request = String::from_utf8_lossy(&buffer[..n]);

        debug!("📨 Received request:\n{}", request.lines().take(3).collect::<Vec<_>>().join("\n"));

        // Разделяем stream на read/write half
        let (mut read_half, write_half) = stream.into_split();

        // Генерируем уникальный connection_id
        let conn_id = {
            let mut id = self.next_connection_id.lock().await;
            let cid = *id;
            *id += 1;
            cid
        };

        info!("🔗 Connection #{} established", conn_id);

        // 🚇 Создаём channel для получения ответов от глобального callback
        let (response_tx, mut response_rx) = mpsc::channel::<Vec<u8>>(100);

        // Регистрируем соединение в глобальной мапе
        {
            let mut conns = self.active_connections.lock().await;
            conns.insert(conn_id, response_tx);
            debug!("✅ Connection #{} registered in global map", conn_id);
        }

        // Запускаем task для записи ответов в stream
        let conn_id_clone = conn_id;
        let active_conns = self.active_connections.clone();
        tokio::spawn(async move {
            let mut write_half = write_half;
            let mut counter = 0;
            while let Some(data) = response_rx.recv().await {
                counter += 1;
                debug!("📤 [Conn#{}] Writing {} bytes to browser (msg #{})",
                       conn_id_clone, data.len(), counter);
                if let Err(e) = write_half.write_all(&data).await {
                    error!("❌ [Conn#{}] Error writing to stream: {}", conn_id_clone, e);
                    break;
                }
            }
            debug!("🔚 [Conn#{}] Response writer closed (total: {} msgs)",
                   conn_id_clone, counter);

            // Удаляем соединение из мапы
            let mut conns = active_conns.lock().await;
            conns.remove(&conn_id_clone);
            debug!("🗑️  [Conn#{}] Removed from global map", conn_id_clone);
        });

        // Проверяем тип запроса
        if request.starts_with("CONNECT") {
            // HTTPS запрос - туннелируем raw bytes
            self.handle_connect(read_half, &request, &buffer[..n]).await
        } else {
            // HTTP запрос - absolute URL
            self.handle_http_proxy(read_half, &request, &buffer[..n]).await
        }
    }

    /// Обработать CONNECT запрос (HTTPS)
    async fn handle_connect(&self, mut read_half: OwnedReadHalf, request: &str, initial_data: &[u8]) -> Result<()> {
        // Парсим hostname:port
        let target = request
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .ok_or_else(|| anyhow!("Invalid CONNECT request"))?;

        info!("🔌 CONNECT request to {}", target);

        // Запускаем чтение raw bytes С начальными данными (CONNECT запрос)
        self.tunnel_raw_bytes_with_initial(read_half, initial_data).await
    }

    /// Обработать HTTP прокси запрос (absolute URL)
    async fn handle_http_proxy(&self, mut read_half: OwnedReadHalf, request: &str, _initial_data: &[u8]) -> Result<()> {
        // Парсим absolute URL: GET http://example.com/path HTTP/1.1
        let first_line = request.lines().next()
            .ok_or_else(|| anyhow!("Empty request"))?;

        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(anyhow!("Invalid HTTP request"));
        }

        // Парсим URL: http://example.com:80/path
        let url_str = parts[1];
        let url = url::Url::parse(url_str)
            .map_err(|e| anyhow!("Failed to parse URL {}: {}", url_str, e))?;

        let host = url.host_str()
            .ok_or_else(|| anyhow!("No host in URL"))?;
        let port = url.port().unwrap_or(80);

        let target = format!("{}:{}", host, port);
        info!("🌐 HTTP proxy request to {} (full URL: {})", target, url_str);

        // 🚇 НЕ ПЕРЕПИСЫВАЕМ запрос! Отправляем как есть с абсолютным URL!
        // Шлюз сам перепишет его перед отправкой в интернет
        self.tunnel_raw_bytes_with_initial(read_half, request.as_bytes()).await
    }

    /// Туннелировать raw bytes (ГЛАВНАЯ ФУНКЦИЯ!)
    async fn tunnel_raw_bytes(&self, mut read_half: OwnedReadHalf) -> Result<()> {
        self.tunnel_raw_bytes_with_initial(read_half, &[]).await
    }

    /// Туннелировать raw bytes с начальными данными (для HTTP proxy)
    async fn tunnel_raw_bytes_with_initial(&self, mut read_half: OwnedReadHalf, initial_data: &[u8]) -> Result<()> {
        let mut buffer = vec![0u8; 16 * 1024]; // 16KB buffer

        // Сначала отправляем начальные данные (если есть)
        if !initial_data.is_empty() {
            debug!("📤 Sending initial {} bytes", initial_data.len());
            self.station.send_train(self.exit_node, initial_data.to_vec()).await?;
        }

        loop {
            // Читаем raw bytes из браузера
            let n = match read_half.read(&mut buffer).await {
                Ok(0) => {
                    debug!("🔚 Client closed connection");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    error!("❌ Read error: {}", e);
                    break;
                }
            };

            if n == 0 {
                break;
            }

            debug!("📥 Read {} bytes from browser", n);

            // ⏰ Получаем Unix timestamp (milliseconds)
            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            debug!("⏰ Timestamp: {} ms", timestamp_ms);

            // 📦 Отправляем через Station (send_train уже использует batching!)
            // Raw bytes БЕЗ ИЗМЕНЕНИЙ!
            self.station.send_train(self.exit_node, buffer[..n].to_vec()).await?;

            debug!("📤 Sent {} bytes via YTP", n);
        }

        debug!("✅ Tunnel closed");
        Ok(())
    }
}

/// Преобразовать HashId в ConnectionId
fn connection_id_from_hash_id(hash_id: HashId) -> u64 {
    // Используем первые 8 байт HashId как connection_id
    let bytes = &hash_id.0[..8];
    u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}
