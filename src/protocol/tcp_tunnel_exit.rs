// src/protocol/tcp_tunnel_exit.rs
//! TcpTunnel Exit Handler
//! ======================
//!
//! Обрабатывает raw bytes от клиентов и проксирует в интернет
//!
//! Архитектура:
//!
//! ```text
//! Client → YTP wagon (raw bytes) → Exit Handler → Internet
//!         ← YTP wagon (response)  ← Exit Handler ←
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use anyhow::{Result, anyhow};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::util::HashId;
use crate::protocol::Station;
use crate::netlayer::P2PTransport;
use tracing::{info, error, debug, warn};

/// TCP туннель от клиента к exit node
pub struct TcpTunnel {
    /// Клиент который создал туннель
    pub client_id: HashId,

    /// Уникальный ID туннеля
    pub tunnel_id: u64,

    /// Read half (читаем из интернета, отправляем клиенту)
    pub read_half: Option<OwnedReadHalf>,

    /// Write half (пишем в интернет)
    pub write_half: Option<OwnedWriteHalf>,

    /// Target (куда коннектимся в интернете)
    pub target: String,
}

impl TcpTunnel {
    /// Создать новый туннель
    pub fn new(client_id: HashId, tunnel_id: u64, target: String) -> Self {
        Self {
            client_id,
            tunnel_id,
            read_half: None,
            write_half: None,
            target,
        }
    }
}

/// TcpTunnel Exit Handler
/// Принимает raw bytes от клиентов через Station callback, проксирует в интернет
pub struct TcpTunnelExitHandler {
    /// Station для получения wagon'ов через callback и отправки ответов
    station: Arc<Station>,

    /// Transport для P2P коммуникации
    transport: Arc<P2PTransport>,

    /// Активные туннели: tunnel_id -> TcpTunnel
    active_tunnels: Arc<Mutex<HashMap<u64, TcpTunnel>>>,

    /// Маппинг client_id -> tunnel_id (для поиска туннеля по клиенту)
    client_tunnels: Arc<Mutex<HashMap<HashId, u64>>>,

    /// Следующий ID туннеля
    next_tunnel_id: Arc<Mutex<u64>>,
}

impl TcpTunnelExitHandler {
    /// Создать новый handler (использует существующую Station из transport)
    pub async fn new(
        transport: Arc<P2PTransport>,
    ) -> Self {
        // Получаем Station из transport (НЕ создаём новую!)
        let station = transport.get_station().await;

        Self {
            station,
            transport,
            active_tunnels: Arc::new(Mutex::new(HashMap::new())),
            client_tunnels: Arc::new(Mutex::new(HashMap::new())),
            next_tunnel_id: Arc::new(Mutex::new(1)),
        }
    }

    /// Запустить handler
    pub async fn run(self) -> Result<()> {
        info!("🚇 TcpTunnel Exit Handler started");

        // 🚇 Подписываемся на Station callback
        // Callback будет вызываться из Station::receive_wagon() когда придут wagon'ы от клиентов

        // Создаём handler для обработки (нужен Arc для callback)
        let handler = Arc::new(self);

        // Берём station ДО создания замыкания
        let station = handler.station.clone();

        station.set_data_callback(move |source_id: HashId, raw_bytes: Vec<u8>| {
            debug!("📨 Callback received {} bytes from client {}",
                   raw_bytes.len(), hex::encode(&source_id.0[..8]));

            // Запускаем обработку в отдельном task (callback - sync context)
            let handler = handler.clone();
            tokio::spawn(async move {
                if let Err(e) = handler.handle_wagon(source_id, raw_bytes).await {
                    error!("❌ Error handling wagon from {}: {}",
                           hex::encode(&source_id.0[..8]), e);
                }
            });
        }).await;

        info!("✅ Callback registered for exit handler");

        // Handler запущен и готов получать wagon'ы
        tokio::time::sleep(tokio::time::Duration::from_secs(u64::MAX)).await;

        Ok(())
    }

    /// Обработать wagon от клиента
    async fn handle_wagon(&self, client_id: HashId, data: Vec<u8>) -> Result<()> {
        // кто может выходить в интернет через этот узел — выбор владельца
        if !crate::exit_policy::may_exit(&client_id) {
            crate::exit_policy::refuse_log(&client_id, "TCP-туннель");
            return Ok(());
        }
        // Проверяем если это новый туннель или данные для существующего
        let data_str = String::from_utf8_lossy(&data);

        // Если это CONNECT запрос - всегда создаём новый туннель
        if data_str.starts_with("CONNECT ") {
            debug!("🔌 New CONNECT request from client {}", hex::encode(&client_id.0[..8]));
            self.create_tunnel(client_id, &data_str).await
        }
        // Если это HTTP запрос - всегда создаём новый туннель
        else if data_str.contains("HTTP/1.1\r\n") || data_str.contains("HTTP/1.0\r\n") {
            debug!("🌐 New HTTP request from client {}", hex::encode(&client_id.0[..8]));
            self.handle_http_request(client_id, data).await
        }
        // Иначе - это данные для существующего туннеля
        else {
            self.forward_to_tunnel(client_id, &data).await
        }
    }

    /// Создать новый туннель для CONNECT
    async fn create_tunnel(&self, client_id: HashId, request: &str) -> Result<()> {
        // Парсим target: CONNECT example.com:443
        let target = request
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .ok_or_else(|| anyhow!("Invalid CONNECT request"))?;

        info!("🔌 Creating tunnel to {}", target);

        // Коннектимся к target
        let stream = crate::exit_policy::connect_public(&target, std::time::Duration::from_secs(10)).await
            .map_err(|e| anyhow!("Failed to connect to {}: {}", target, e))?;

        stream.set_nodelay(true)?;

        // Разделяем на read/write half
        let (read_half, write_half) = stream.into_split();

        // Генерируем tunnel_id
        let tunnel_id = {
            let mut id = self.next_tunnel_id.lock().await;
            let tid = *id;
            *id += 1;
            tid
        };

        // Сохраняем туннель
        let mut tunnel = TcpTunnel::new(client_id, tunnel_id, target.to_string());
        tunnel.read_half = Some(read_half);
        tunnel.write_half = Some(write_half);

        {
            let mut tunnels = self.active_tunnels.lock().await;
            tunnels.insert(tunnel_id, tunnel);
        }

        // Сохраняем маппинг client_id -> tunnel_id
        {
            let mut client_tunnels = self.client_tunnels.lock().await;
            client_tunnels.insert(client_id, tunnel_id);
        }

        info!("✅ Tunnel #{} created to {} for client {}", tunnel_id, target,
              hex::encode(&client_id.0[..8]));

        // Отправляем 200 Connection Established обратно клиенту
        let response = b"HTTP/1.1 200 Connection Established\r\n\r\n";
        self.send_to_client(client_id, response.to_vec()).await?;

        // Запускаем task для чтения из туннеля и отправки обратно клиенту
        let tunnels = self.active_tunnels.clone();
        let station = self.station.clone();
        tokio::spawn(async move {
            Self::tunnel_reader(tunnel_id, client_id, tunnels, station).await;
        });

        Ok(())
    }

    /// Обработать HTTP запрос (absolute URL)
    async fn handle_http_request(&self, client_id: HashId, data: Vec<u8>) -> Result<()> {
        // Парсим HTTP запрос
        let request_str = String::from_utf8_lossy(&data);

        // Извлекаем хост из первой строки
        let first_line = request_str.lines().next()
            .ok_or_else(|| anyhow!("Empty HTTP request"))?;

        // GET http://example.com/path HTTP/1.1
        if !first_line.contains("://") {
            return Err(anyhow!("Invalid HTTP request (no absolute URL)"));
        }

        // Извлекаем хост (простой парсинг)
        let host = first_line
            .split("://")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.split(':').next())
            .ok_or_else(|| anyhow!("Cannot extract host from request"))?;

        let target = if first_line.contains(":443") {
            format!("{}:443", host)
        } else {
            format!("{}:80", host)
        };

        info!("🌐 HTTP request to {}", target);

        // Переписываем HTTP запрос (убираем схему)
        let modified_request = Self::modify_http_request(&request_str, &target)?;

        // Создаём туннель
        let stream = crate::exit_policy::connect_public(&target, std::time::Duration::from_secs(10)).await
            .map_err(|e| anyhow!("Failed to connect to {}: {}", target, e))?;

        stream.set_nodelay(true)?;

        let (read_half, write_half) = stream.into_split();

        let tunnel_id = {
            let mut id = self.next_tunnel_id.lock().await;
            let tid = *id;
            *id += 1;
            tid
        };

        let mut tunnel = TcpTunnel::new(client_id, tunnel_id, target.clone());
        tunnel.read_half = Some(read_half);
        tunnel.write_half = Some(write_half);

        {
            let mut tunnels = self.active_tunnels.lock().await;
            tunnels.insert(tunnel_id, tunnel);
        }

        // Сохраняем маппинг client_id -> tunnel_id
        {
            let mut client_tunnels = self.client_tunnels.lock().await;
            client_tunnels.insert(client_id, tunnel_id);
        }

        info!("✅ HTTP tunnel #{} created to {} for client {}", tunnel_id, target,
              hex::encode(&client_id.0[..8]));

        // Отправляем запрос в интернет
        {
            let mut tunnels = self.active_tunnels.lock().await;
            if let Some(tunnel) = tunnels.get_mut(&tunnel_id) {
                if let Some(write_half) = &mut tunnel.write_half {
                    write_half.write_all(modified_request.as_bytes()).await?;
                    info!("📤 Sent {} bytes to {}", modified_request.len(), target);
                }
            }
        }

        // Запускаем reader для ответа
        let tunnels = self.active_tunnels.clone();
        let station = self.station.clone();
        tokio::spawn(async move {
            Self::tunnel_reader(tunnel_id, client_id, tunnels, station).await;
        });

        Ok(())
    }

    /// Переписать HTTP запрос (убрать схему)
    fn modify_http_request(request: &str, _target: &str) -> Result<String> {
        let lines: Vec<&str> = request.lines().collect();

        if lines.is_empty() {
            return Err(anyhow!("Empty request"));
        }

        // Первая строка: GET http://example.com/path HTTP/1.1
        let first_line = lines[0];
        let parts: Vec<&str> = first_line.split_whitespace().collect();

        if parts.len() < 3 {
            return Err(anyhow!("Invalid HTTP request line"));
        }

        // Парсим URL
        let url_str = parts[1];
        let url = url::Url::parse(url_str)?;

        // Формируем новый путь
        let path = url.path();
        let query = url.query();
        let new_path = if let Some(q) = query {
            format!("{}?{}", path, q)
        } else {
            path.to_string()
        };

        // Пересобираем запрос
        let modified = format!(
            "{} {} HTTP/1.1\r\n{}",
            parts[0],
            new_path,
            lines[1..].join("\r\n")
        );

        Ok(modified)
    }

    /// Читать из туннеля и отправлять обратно клиенту через YTP
    async fn tunnel_reader(
        tunnel_id: u64,
        client_id: HashId,
        tunnels: Arc<Mutex<HashMap<u64, TcpTunnel>>>,
        station: Arc<Station>,
    ) {
        info!("🔄 tunnel_reader STARTED for tunnel #{} (client: {})",
              tunnel_id, hex::encode(&client_id.0[..8]));
        let mut buffer = vec![0u8; 16 * 1024];
        let mut counter = 0u32;

        loop {
            // Читаем из туннеля
            let n = {
                let mut tunnels_lock = tunnels.lock().await;
                let tunnel = tunnels_lock.get_mut(&tunnel_id);

                match tunnel {
                    Some(t) if t.read_half.is_some() => {
                        match t.read_half.as_mut().unwrap().read(&mut buffer).await {
                            Ok(0) => {
                                debug!("🔚 Tunnel #{} closed by remote", tunnel_id);
                                return;
                            }
                            Ok(n) => n,
                            Err(e) => {
                                error!("❌ Read error on tunnel #{}: {}", tunnel_id, e);
                                return;
                            }
                        }
                    }
                    _ => {
                        warn!("⚠️  Tunnel #{} not found", tunnel_id);
                        return;
                    }
                }
            };

            counter += 1;
            info!("📥 [Tunnel#{}] Read {} bytes from internet (msg #{})",
                  tunnel_id, n, counter);

            // 🚇 Отправляем обратно клиенту через Station (через YTP)
            if let Err(e) = station.send_train(client_id, buffer[..n].to_vec()).await {
                error!("❌ Failed to send response to client: {}", e);
                return;
            }

            info!("📤 [Tunnel#{}] Sent {} bytes to client {} via YTP (msg #{})",
                  tunnel_id, n, hex::encode(&client_id.0[..8]), counter);
        }

        info!("🔚 [Tunnel#{}] tunnel_reader CLOSED (total: {} msgs)",
              tunnel_id, counter);
    }

    /// Переслать данные в существующий туннель
    async fn forward_to_tunnel(&self, client_id: HashId, data: &[u8]) -> Result<()> {
        // Находим tunnel_id по client_id
        let tunnel_id = {
            let client_tunnels = self.client_tunnels.lock().await;
            match client_tunnels.get(&client_id) {
                Some(&tid) => tid,
                None => {
                    warn!("⚠️  No tunnel found for client {} (data: {} bytes)",
                          hex::encode(&client_id.0[..8]), data.len());
                    return Ok(());
                }
            }
        };

        // Пишем данные в туннель
        let mut tunnels = self.active_tunnels.lock().await;
        if let Some(tunnel) = tunnels.get_mut(&tunnel_id) {
            if let Some(write_half) = &mut tunnel.write_half {
                write_half.write_all(data).await
                    .map_err(|e| anyhow!("Failed to write to tunnel #{}: {}", tunnel_id, e))?;
                debug!("📤 Forwarded {} bytes to tunnel #{} ({})",
                       data.len(), tunnel_id, tunnel.target);
            } else {
                warn!("⚠️  Tunnel #{} has no write_half", tunnel_id);
            }
        } else {
            warn!("⚠️  Tunnel #{} not found (client: {})",
                  tunnel_id, hex::encode(&client_id.0[..8]));
        }

        Ok(())
    }

    /// Отправить данные клиенту через YTP
    async fn send_to_client(&self, client_id: HashId, data: Vec<u8>) -> Result<()> {
        // 🚇 Отправляем через Station, чтобы данные пришли через callback на клиенте
        let _train_id = self.station.send_train(client_id, data).await
            .map_err(|e| anyhow!("Failed to send to client: {}", e))?;
        Ok(())
    }
}
