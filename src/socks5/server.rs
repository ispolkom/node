// src/socks5/server.rs
//! SOCKS5 Server Implementation
//! =============================
//!
//! SOCKS5 proxy server for traffic relay

use std::net::SocketAddr;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, RwLock};
use anyhow::{Result, anyhow};

use super::protocol::*;
use super::{Socks5Config, Socks5Error, Socks5ProxyRequest, Socks5ProxyResponse, Socks5TunnelData};
use crate::netlayer::P2PTransport;
use crate::util::HashId;
use crate::protocol::Station;
use tracing::{info, error, debug, warn};

/// SOCKS5 server (обычный TCP mode)
pub struct Socks5Server {
    config: Socks5Config,
}

/// SOCKS5 Proxy Server via P2P (аналог HttpProxyClient)
pub struct Socks5ProxyServer {
    config: Socks5Config,
    transport: Arc<P2PTransport>,
    exit_node_id: Option<HashId>,
    /// автоматический выбор выхода по каталогу (если задан — вместо одного постоянного)
    exit_pool: Option<Arc<crate::exit_select::ExitPool>>,
    /// соединения идут цепочкой через несколько узлов (вход → средний → выход), а не напрямую через выход
    use_hops: bool,
    /// правила по сайтам: часть трафика идёт напрямую, остальное — через выход
    rules: Option<Arc<crate::route_rules::Rules>>,
    /// личный шлюз: путь (вход → … → шлюз) и общий секрет устройств; соединения идут только им
    personal: Option<Arc<(Vec<crate::hops::PathHop>, [u8; 32])>>,
    next_request_id: Arc<RwLock<u64>>,
    /// Pending requests: request_id -> oneshot sender
    pending_requests: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Socks5ProxyResponse>>>>,
    /// Active tunnels: tunnel_id -> write half
    active_tunnels: Arc<Mutex<HashMap<u64, tokio::net::tcp::OwnedWriteHalf>>>,
    /// Channels for responses and tunnel data
    response_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyResponse)>>>>,
    tunnel_data_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5TunnelData)>>>>,
    pub station: Arc<Station>,
}

impl Socks5Server {
    /// Create new SOCKS5 server
    pub fn new(config: Socks5Config) -> Self {
        println!("[socks5] Creating SOCKS5 server on {}", config.listen_addr);
        Self { config }
    }

    /// Start SOCKS5 server
    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(&self.config.listen_addr).await
            .map_err(|e| anyhow!("Failed to bind SOCKS5 server: {}", e))?;

        println!("[socks5] SOCKS5 server listening on {}", self.config.listen_addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    println!("[socks5] New connection from {}", addr);
                    let config = self.config.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client(stream, addr, config).await {
                            eprintln!("[socks5] Error handling {}: {:?}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[socks5] Error accepting connection: {}", e);
                }
            }
        }
    }

    /// Handle single SOCKS5 client connection
    async fn handle_client(mut stream: TcpStream, client_addr: SocketAddr, config: Socks5Config) -> Result<()> {
        // Phase 1: Authentication selection
        // every handshake phase runs under a deadline: a connection that goes silent cannot hold the task
        let hs = std::time::Duration::from_secs(20);
        let auth_method = tokio::time::timeout(hs, Self::do_auth_selection(&mut stream, &config)).await
            .map_err(|_| anyhow!("SOCKS5 handshake timeout"))??;

        // Phase 2: Handle authentication (if required)
        if auth_method == Socks5AuthMethod::UserPass {
            tokio::time::timeout(hs, Self::do_username_password_auth(&mut stream, &config)).await
                .map_err(|_| anyhow!("SOCKS5 auth timeout"))??;
        }

        // Phase 3: Connection request
        let request = tokio::time::timeout(hs, Self::read_request(&mut stream)).await
            .map_err(|_| anyhow!("SOCKS5 request timeout"))??;

        println!("[socks5] Request from {}: {:?}", client_addr, request.command);

        // Phase 4: Execute command
        match request.command {
            Socks5Command::Connect => {
                Self::handle_connect(&mut stream, &request.address, &config).await?;
            }
            Socks5Command::Bind => {
                Self::handle_bind(&mut stream, &request.address, &config).await?;
            }
            Socks5Command::UdpAssociate => {
                if config.enable_udp {
                    Self::handle_udp_associate(&mut stream, &request.address, &config).await?;
                } else {
                    // UDP associate disabled
                    let response = Socks5Response::error(Socks5Error::CommandNotSupported, None);
                    stream.write_all(&response.to_bytes()).await?;
                    return Err(anyhow!("UDP associate not enabled"));
                }
            }
        }

        Ok(())
    }

    /// Phase 1: Authentication selection
    async fn do_auth_selection(stream: &mut TcpStream, config: &Socks5Config) -> Result<Socks5AuthMethod> {
        let mut buf = [0u8; 256];

        // Read client hello
        let n = stream.read(&mut buf).await?;
        let auth_select = Socks5AuthSelect::from_bytes(&buf[..n])?;

        // Select auth method
        let method = if config.auth_required {
            if auth_select.methods.contains(&Socks5AuthMethod::UserPass) {
                Socks5AuthMethod::UserPass
            } else {
                Socks5AuthMethod::NoAcceptable
            }
        } else {
            if auth_select.methods.contains(&Socks5AuthMethod::NoAuth) {
                Socks5AuthMethod::NoAuth
            } else if auth_select.methods.contains(&Socks5AuthMethod::UserPass) {
                Socks5AuthMethod::UserPass
            } else {
                Socks5AuthMethod::NoAcceptable
            }
        };

        // Send selection response
        let response = Socks5AuthResponse::new(method);
        stream.write_all(&response.to_bytes()).await?;

        if method == Socks5AuthMethod::NoAcceptable {
            return Err(anyhow!("No acceptable auth method"));
        }

        Ok(method)
    }

    /// Username/password authentication (RFC 1929)
    /// Возвращает «закрепление выхода»: имя `yandi.<начало номера узла>` просит именно этот выход (так узел проверяет страну выхода).
    async fn do_username_password_auth(stream: &mut TcpStream, config: &Socks5Config) -> Result<Option<String>> {
        let mut buf = [0u8; 512];

        let n = stream.read(&mut buf).await?;
        if n < 2 {
            return Err(anyhow!("Auth packet too short"));
        }

        let ulen = buf[1] as usize;
        if n < 2 + ulen {
            return Err(anyhow!("Username too long"));
        }

        let username = String::from_utf8_lossy(&buf[2..2+ulen]).to_string();

        let plen = buf[2+ulen] as usize;
        if n < 2 + ulen + 1 + plen {
            return Err(anyhow!("Password too long"));
        }

        let password = String::from_utf8_lossy(&buf[2+ulen+1..2+ulen+1+plen]).to_string();

        // Verify credentials
        let mut pin: Option<String> = None;
        let success = config.username.as_ref().zip(config.password.as_ref())
            .map(|(expected_user, expected_pass)| {
                let user_ok = username == *expected_user
                    || username.strip_prefix(expected_user.as_str()).and_then(|r| r.strip_prefix('.')).map(|p| {
                        let good = p.len() >= 8 && p.len() <= 64 && p.bytes().all(|c| c.is_ascii_hexdigit());
                        if good { pin = Some(p.to_ascii_lowercase()); }
                        good
                    }).unwrap_or(false);
                user_ok & bool::from(subtle::ConstantTimeEq::ct_eq(password.as_bytes(), expected_pass.as_bytes()))
            })
            .unwrap_or(false);

        // Send auth response
        stream.write_all(&[0x01, if success { 0x00 } else { 0x01 }]).await?;

        if !success {
            return Err(anyhow!("Invalid username or password"));
        }

        println!("[socks5] Authentication successful for user: {}", username);
        Ok(pin)
    }

    /// Read connection request
    async fn read_request(stream: &mut TcpStream) -> Result<Socks5Request> {
        let mut buf = [0u8; 512];

        let n = stream.read(&mut buf).await?;
        let request = Socks5Request::from_bytes(&buf[..n])?;

        Ok(request)
    }

    /// Handle CONNECT command
    async fn handle_connect(stream: &mut TcpStream, addr: &Socks5Address, _config: &Socks5Config) -> Result<()> {
        // Resolve domain if needed
        let target_addr = if let Some(socket_addr) = addr.to_socket_addr() {
            socket_addr
        } else {
            // Domain name - resolve it
            match addr {
                Socks5Address::Domain(domain, port) => {
                    // Use tokio DNS resolution
                    let addrs = tokio::net::lookup_host(format!("{}:{}", domain, port)).await?;
                    addrs.into_iter().next()
                        .ok_or_else(|| anyhow!("Failed to resolve domain: {}", domain))?
                }
                _ => return Err(anyhow!("Invalid address for connect")),
            }
        };

        // Connect to target
        let target_stream = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(target_addr)
        ).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                let response = Socks5Response::error(Socks5Error::HostUnreachable, None);
                stream.write_all(&response.to_bytes()).await?;
                return Err(anyhow!("Failed to connect to target: {}", e));
            }
            Err(_) => {
                let response = Socks5Response::error(Socks5Error::TtlExpired, None);
                stream.write_all(&response.to_bytes()).await?;
                return Err(anyhow!("Connection timeout"));
            }
        };

        // Send success response
        let local_addr = target_stream.local_addr()?;
        let bind_addr = Socks5Address::from_socket_addr(local_addr);
        let response = Socks5Response::success(bind_addr);
        stream.write_all(&response.to_bytes()).await?;

        println!("[socks5] Connected to {}", target_addr);

        // Relay data
        let (mut client_read, mut client_write) = stream.split();
        let (mut target_read, mut target_write) = target_stream.into_split();

        let client_to_target = tokio::io::copy(&mut client_read, &mut target_write);
        let target_to_client = tokio::io::copy(&mut target_read, &mut client_write);

        tokio::select! {
            result = client_to_target => {
                if let Err(e) = result {
                    eprintln!("[socks5] Client->Target error: {}", e);
                }
            }
            result = target_to_client => {
                if let Err(e) = result {
                    eprintln!("[socks5] Target->Client error: {}", e);
                }
            }
        }

        println!("[socks5] Connection closed");
        Ok(())
    }

    /// Handle BIND command (not commonly used)
    async fn handle_bind(stream: &mut TcpStream, _addr: &Socks5Address, _config: &Socks5Config) -> Result<()> {
        // BIND is for reverse connections - rarely used
        let response = Socks5Response::error(Socks5Error::CommandNotSupported, None);
        stream.write_all(&response.to_bytes()).await?;
        Err(anyhow!("BIND command not supported"))
    }

    /// Handle UDP ASSOCIATE command
    async fn handle_udp_associate(stream: &mut TcpStream, _addr: &Socks5Address, _config: &Socks5Config) -> Result<()> {
        // Bind UDP socket
        let udp_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await
            .map_err(|e| anyhow!("Failed to bind UDP: {}", e))?;

        let udp_addr = udp_socket.local_addr()?;
        let bind_addr = Socks5Address::from_socket_addr(udp_addr);

        // Send success response with UDP relay address
        let response = Socks5Response::success(bind_addr);
        stream.write_all(&response.to_bytes()).await?;

        println!("[socks5] UDP relay listening on {}", udp_addr);

        // Keep TCP connection alive and relay UDP packets
        // (Simplified - full implementation would handle UDP relay)
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;

        Ok(())
    }
}

impl Socks5ProxyServer {
    /// Create new P2P SOCKS5 proxy server
    pub fn new(config: Socks5Config, transport: Arc<P2PTransport>) -> Self {
        println!("[socks5-proxy] Creating P2P SOCKS5 proxy on {}", config.listen_addr);

        let station = Station::with_defaults(
            transport.identity().node_id(),
            transport.clone()
        );

        Self {
            config,
            transport,
            exit_node_id: None,
            exit_pool: None,
            use_hops: false,
            rules: None,
            personal: None,
            next_request_id: Arc::new(RwLock::new(1)),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            active_tunnels: Arc::new(Mutex::new(HashMap::new())),
            response_rx: Arc::new(Mutex::new(None)),
            tunnel_data_rx: Arc::new(Mutex::new(None)),
            station: Arc::new(station),
        }
    }

    /// Set response channel
    pub fn with_response_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyResponse)>) -> Self {
        self.response_rx = Arc::new(Mutex::new(Some(rx)));
        self
    }

    /// Set tunnel data channel
    pub fn with_tunnel_data_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, Socks5TunnelData)>) -> Self {
        self.tunnel_data_rx = Arc::new(Mutex::new(Some(rx)));
        self
    }

    /// Set exit node for all traffic
    pub fn with_exit_node(mut self, exit_node_id: HashId) -> Self {
        println!("[socks5-proxy] Using exit node: {}", hex::encode(&exit_node_id.0[..8]));
        self.exit_node_id = Some(exit_node_id);
        self
    }

    /// Выходы выбираются сами и чередуются (по каждому новому соединению)
    pub fn with_exit_pool(mut self, pool: Arc<crate::exit_select::ExitPool>) -> Self {
        self.exit_pool = Some(pool);
        self
    }

    /// Правила по сайтам (`route_rules`): «напрямую» — с этого компьютера, «через выход» — как выбран режим
    pub fn with_rules(mut self, rules: crate::route_rules::Rules) -> Self {
        self.rules = Some(Arc::new(rules));
        self
    }

    /// Все соединения — через личный шлюз по заданному пути (свой компьютер за NAT или дом)
    pub fn with_personal(mut self, path: Vec<crate::hops::PathHop>, secret: [u8; 32]) -> Self {
        self.personal = Some(Arc::new((path, secret)));
        self
    }

    /// Идти через цепочки из нескольких узлов (нужна очередь выходов)
    pub fn with_hops(mut self) -> Self {
        self.use_hops = true;
        self
    }

    /// Start P2P SOCKS5 proxy server
    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(&self.config.listen_addr).await
            .map_err(|e| anyhow!("Failed to bind SOCKS5 proxy server: {}", e))?;

        info!("🧦 SOCKS5 Proxy listening on {}", self.config.listen_addr);
        info!("📡 Exit node: {:?}", self.exit_node_id.map(|id| hex::encode(&id.0[..8])));

        // Subscribe to responses
        let transport_clone = self.transport.clone();
        let pending_clone = self.pending_requests.clone();
        let response_rx_clone = self.response_rx.clone();
        tokio::spawn(async move {
            Self::handle_responses(transport_clone, pending_clone, response_rx_clone).await;
        });

        // Subscribe to tunnel data
        let active_tunnels_clone = self.active_tunnels.clone();
        let tunnel_rx_clone = self.tunnel_data_rx.clone();
        tokio::spawn(async move {
            Self::handle_tunnel_data(active_tunnels_clone, tunnel_rx_clone).await;
        });

        // Accept incoming connections
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!("📥 New SOCKS5 connection from {}", addr);

                    let client = self.clone_for_handler();
                    tokio::spawn(async move {
                        if let Err(e) = client.handle_client_p2p(stream, addr).await {
                            error!("❌ Error handling SOCKS5 client {}: {}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("❌ Error accepting SOCKS5 connection: {}", e);
                }
            }
        }
    }

    /// Clone for handler
    fn clone_for_handler(&self) -> Self {
        Self {
            config: self.config.clone(),
            transport: self.transport.clone(),
            exit_node_id: self.exit_node_id,
            exit_pool: self.exit_pool.clone(),
            use_hops: self.use_hops,
            rules: self.rules.clone(),
            personal: self.personal.clone(),
            next_request_id: self.next_request_id.clone(),
            pending_requests: self.pending_requests.clone(),
            active_tunnels: self.active_tunnels.clone(),
            response_rx: self.response_rx.clone(),
            tunnel_data_rx: self.tunnel_data_rx.clone(),
            station: self.station.clone(),
        }
    }

    /// Handle incoming responses from exit node
    async fn handle_responses(
        _transport: Arc<P2PTransport>,
        pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Socks5ProxyResponse>>>>,
        response_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyResponse)>>>>,
    ) {
        let mut rx_opt = { response_rx.lock().await.take() };

        if let Some(mut rx) = rx_opt {
            while let Some((_source_node, response)) = rx.recv().await {
                debug!("📨 Received SOCKS5 response for request #{}", response.request_id);

                // Find pending request
                let sender_opt = {
                    let mut pending = pending.lock().await;
                    pending.remove(&response.request_id)
                };

                if let Some(sender) = sender_opt {
                    let _ = sender.send(response);
                } else {
                    warn!("⚠️  No pending request for #{}", response.request_id);
                }
            }
        }
    }

    /// Handle tunnel data from exit node
    async fn handle_tunnel_data(
        active_tunnels: Arc<Mutex<HashMap<u64, tokio::net::tcp::OwnedWriteHalf>>>,
        tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5TunnelData)>>>>,
    ) {
        let mut rx_opt = { tunnel_rx.lock().await.take() };

        if let Some(mut rx) = rx_opt {
            while let Some((_source_node, tunnel_data)) = rx.recv().await {
                let tunnel_id = tunnel_data.tunnel_id;

                // Check if tunnel should be closed
                if tunnel_data.close {
                    debug!("🔚 Tunnel #{} closed by exit node", tunnel_id);
                    // Remove from active tunnels
                    let mut tunnels = active_tunnels.lock().await;
                    tunnels.remove(&tunnel_id);
                    continue;
                }

                // Get write half for this tunnel (keep lock during write!)
                let write_result = {
                    let mut tunnels = active_tunnels.lock().await;

                    if let Some(write_half) = tunnels.get_mut(&tunnel_id) {
                        // Write data while holding lock
                        write_half.write_all(&tunnel_data.data).await
                            .map_err(|e| (e.to_string()))
                    } else {
                        Err("No active tunnel".to_string())
                    }
                };

                if let Err(e) = write_result {
                    error!("❌ Error writing to tunnel #{}: {}", tunnel_id, e);
                    // Remove broken tunnel
                    let mut tunnels = active_tunnels.lock().await;
                    tunnels.remove(&tunnel_id);
                }
            }
        }
    }

    /// Handle single P2P SOCKS5 client connection
    async fn handle_client_p2p(&self, mut client_stream: TcpStream, client_addr: SocketAddr) -> Result<()> {
        // Phase 1: Authentication selection
        let hs = std::time::Duration::from_secs(20); // a silent connection must not hold the task
        let auth_method = tokio::time::timeout(hs, Socks5Server::do_auth_selection(&mut client_stream, &self.config)).await
            .map_err(|_| anyhow!("SOCKS5 handshake timeout"))??;

        // Phase 2: Handle authentication (if required)
        let mut pin = None;
        if auth_method == Socks5AuthMethod::UserPass {
            pin = tokio::time::timeout(hs, Socks5Server::do_username_password_auth(&mut client_stream, &self.config)).await
                .map_err(|_| anyhow!("SOCKS5 auth timeout"))??;
        }

        // Phase 3: Connection request
        let request = tokio::time::timeout(hs, Socks5Server::read_request(&mut client_stream)).await
            .map_err(|_| anyhow!("SOCKS5 request timeout"))??;

        debug!("📨 SOCKS5 request from {}: {:?}", client_addr, request.command);

        // Phase 4: Execute command via P2P
        match request.command {
            Socks5Command::Connect => {
                // Take ownership for CONNECT
                self.handle_connect_p2p(client_stream, &request.address, client_addr, pin).await?;
            }
            Socks5Command::Bind => {
                let response = Socks5Response::error(Socks5Error::CommandNotSupported, None);
                client_stream.write_all(&response.to_bytes()).await?;
                return Err(anyhow!("BIND command not supported"));
            }
            Socks5Command::UdpAssociate => {
                let response = Socks5Response::error(Socks5Error::CommandNotSupported, None);
                client_stream.write_all(&response.to_bytes()).await?;
                return Err(anyhow!("UDP associate not supported in P2P mode"));
            }
        }

        Ok(())
    }

    /// Handle CONNECT command via P2P (аналог HttpProxyClient::handle_connect_tunnel)
    async fn handle_connect_p2p(&self, mut client_stream: TcpStream, target_addr: &Socks5Address, _client_addr: SocketAddr, pin: Option<String>) -> Result<()> {
        // ⚡ TCP NODELAY - critical for SOCKS5 performance!
        if let Err(e) = client_stream.set_nodelay(true) {
            error!("❌ Failed to set TCP_NODELAY on client stream: {}", e);
        } else {
            debug!("✅ TCP_NODELAY enabled for client");
        }

        // 1. Определяем exit node
        // правила по сайтам: «напрямую» — соединяемся сами (только адреса в интернете: в домашнюю сеть через прокси не пускаем)
        if let Some(rules) = &self.rules {
            let (host, port) = match target_addr {
                Socks5Address::Ipv4(ip, port) => (ip.to_string(), *port),
                Socks5Address::Domain(domain, port) => (domain.clone(), *port),
                Socks5Address::Ipv6(ip, port) => (ip.to_string(), *port),
            };
            if rules.decide(&host) == crate::route_rules::Route::Direct {
                let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
                return match crate::exit_policy::connect_public(&target, std::time::Duration::from_secs(10)).await {
                    Ok(mut remote) => {
                        let bind_addr = Socks5Address::Ipv4(std::net::Ipv4Addr::new(0, 0, 0, 0), 0);
                        client_stream.write_all(&Socks5Response::success(bind_addr).to_bytes()).await?;
                        tokio::spawn(async move {
                            let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut remote).await;
                        });
                        Ok(())
                    }
                    Err(e) => {
                        let code = match e.kind() {
                            std::io::ErrorKind::PermissionDenied => Socks5Error::ConnectionNotAllowed,
                            std::io::ErrorKind::TimedOut => Socks5Error::TtlExpired,
                            _ => Socks5Error::ConnectionRefused,
                        };
                        client_stream.write_all(&Socks5Response::error(code, None).to_bytes()).await?;
                        Err(anyhow!("Direct connect failed: {e}"))
                    }
                };
            }
        }

        // личный шлюз: путь задан явно, выбирать выход не надо
        if let Some(pg) = &self.personal {
            let (host, port) = match target_addr {
                Socks5Address::Ipv4(ip, port) => (ip.to_string(), *port),
                Socks5Address::Domain(domain, port) => (domain.clone(), *port),
                Socks5Address::Ipv6(ip, port) => (ip.to_string(), *port),
            };
            return match crate::hops_net::connect_personal(pg.0.clone(), &pg.1, &host, port).await {
                Ok((writer, reader)) => {
                    let bind_addr = Socks5Address::Ipv4(std::net::Ipv4Addr::new(0, 0, 0, 0), 0);
                    client_stream.write_all(&Socks5Response::success(bind_addr).to_bytes()).await?;
                    tokio::spawn(crate::hops_net::pump(client_stream, writer, reader));
                    Ok(())
                }
                Err(code) => {
                    client_stream.write_all(&Socks5Response::error(Socks5Error::from_reply_byte(code), None).to_bytes()).await?;
                    Err(anyhow!("Personal gateway connect failed: code={}", code))
                }
            };
        }

        let picked = self.exit_pool.as_ref().and_then(|p| {
            let hex = match &pin {
                Some(prefix) => p.pinned(prefix)?,
                None => p.next(crate::network_offers::now_secs())?,
            };
            let mut b = [0u8; 32];
            for i in 0..32 {
                b[i] = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
            }
            Some((HashId(b), hex))
        });
        let exit_hex = picked.as_ref().map(|(_, h)| h.clone());
        let exit_node = match picked {
            Some((id, _)) => id,
            None if self.exit_pool.is_some() => return Err(anyhow!("No exit available right now")),
            None => self.exit_node_id.ok_or_else(|| anyhow!("No exit node configured"))?,
        };
        let report = |ok: bool| {
            if let (Some(p), Some(h)) = (&self.exit_pool, &exit_hex) {
                if ok { p.worked(h) } else { p.failed(h, crate::network_offers::now_secs()) }
            }
        };

        // 2. Генерируем request_id
        let request_id = {
            let mut id = self.next_request_id.write().await;
            let current = *id;
            *id = id.wrapping_add(1);
            current
        };

        debug!("🔌 CONNECT request #{} to {:?}", request_id, target_addr);

        // 3. Парсим target address
        let (target_host, target_port) = match target_addr {
            Socks5Address::Ipv4(ip, port) => (ip.to_string(), *port),
            Socks5Address::Domain(domain, port) => (domain.clone(), *port),
            Socks5Address::Ipv6(ip, port) => (ip.to_string(), *port),
        };

        // цепочка через несколько узлов: соединение открывается выходом, данные идут слоями
        if self.use_hops {
            let result = crate::hops_net::connect(exit_node.0, &target_host, target_port).await;
            return match result {
                Ok((writer, reader)) => {
                    report(true);
                    let bind_addr = Socks5Address::Ipv4(std::net::Ipv4Addr::new(0, 0, 0, 0), 0);
                    client_stream.write_all(&Socks5Response::success(bind_addr).to_bytes()).await?;
                    tokio::spawn(crate::hops_net::pump(client_stream, writer, reader));
                    Ok(())
                }
                Err(code) => {
                    report(code != crate::hops_net::END_REFUSED);
                    let error = Socks5Error::from_reply_byte(code);
                    client_stream.write_all(&Socks5Response::error(error, None).to_bytes()).await?;
                    Err(anyhow!("Hop circuit connect failed: code={}", code))
                }
            };
        }

        // 4. Создаём Socks5ProxyRequest
        let proxy_request = Socks5ProxyRequest::new_connect(request_id, target_host.clone(), target_port);

        // 5. Создаём oneshot для ответа
        let (tx, rx) = tokio::sync::oneshot::channel();

        // 6. Сохраняем pending request
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id, tx);
        }

        // 7. Сериализуем и отправляем через YTP
        let request_bytes = serde_json::to_vec(&proxy_request)
            .map_err(|e| anyhow!("Failed to serialize SOCKS5 request: {}", e))?;

        debug!("📤 Sending SOCKS5 request via YTP to exit node");

        self.station.send_train_batched(exit_node, request_bytes).await
            .map_err(|e| { report(false); anyhow!("Failed to send SOCKS5 request: {}", e) })?;

        debug!("⏳ Waiting for SOCKS5 response...");

        // 8. Ждём ответа от exit node
        let proxy_response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            rx
        ).await
        .map_err(|_| { report(false); anyhow!("Timeout waiting for SOCKS5 response") })?
        .map_err(|_| { report(false); anyhow!("Failed to receive SOCKS5 response") })?;

        // 9. Проверяем статус (2 = выход не пускает нас: отдыхает; остальное — беда цели, а не выхода)
        report(proxy_response.status != 2);
        if !proxy_response.is_success() {
            let error = Socks5Error::from_reply_byte(proxy_response.status);
            let response = Socks5Response::error(error, None);
            client_stream.write_all(&response.to_bytes()).await?;
            return Err(anyhow!("Exit node failed to connect: status={}", proxy_response.status));
        }

        debug!("✅ Exit node connected successfully");

        // 10. Отправляем успешный SOCKS5 ответ клиенту
        let bind_addr = Socks5Address::Ipv4(std::net::Ipv4Addr::new(0, 0, 0, 0), 0);
        let response = Socks5Response::success(bind_addr);
        client_stream.write_all(&response.to_bytes()).await?;

        debug!("🚪 Starting tunnel #{}", request_id);

        // 11. Разделяем клиентский stream
        let (mut client_read, mut client_write) = client_stream.into_split();

        // 12. Сохраняем write half в active_tunnels
        {
            let mut tunnels = self.active_tunnels.lock().await;
            tunnels.insert(request_id, client_write);
        }

        // 13. Читаем данные от клиента и отправляем в туннель (в фоновом режиме!)
        let station_clone = self.station.clone();
        let exit_node_clone = exit_node;

        // Запускаем в фоновом режиме, чтобы handle_tunnel_data мог работать параллельно!
        tokio::spawn(async move {
            const BUFFER_SIZE: usize = 16 * 1024; // ⚡ 32 KB instead of 4 KB
            let mut buf = vec![0u8; BUFFER_SIZE];
            loop {
                match client_read.read(&mut buf).await {
                    Ok(0) => {
                        debug!("🔚 Client closed connection");
                        // Send close message
                        let tunnel_close = Socks5TunnelData::close(request_id);
                        let close_bytes = match serde_json::to_vec(&tunnel_close) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                error!("❌ Failed to serialize close message: {}", e);
                                break;
                            }
                        };
                        let send_res: std::result::Result<(), anyhow::Error> = station_clone.send_train(exit_node_clone, close_bytes).await
                            .map(|_| ())
                            .map_err(|e| anyhow!("send_train: {}", e));
                        if let Err(e) = send_res {
                            error!("❌ Failed to send close message: {}", e);
                        }
                        break;
                    }
                    Ok(n) => {
                        debug!("📤 Read {} bytes from client, sending to tunnel #{}", n, request_id);

                        // Send tunnel data
                        let tunnel_data = Socks5TunnelData::new(request_id, buf[..n].to_vec());
                        let data_bytes = match serde_json::to_vec(&tunnel_data) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                error!("❌ Failed to serialize tunnel data: {}", e);
                                break;
                            }
                        };

                        let send_res: std::result::Result<(), anyhow::Error> = station_clone.send_train(exit_node_clone, data_bytes).await
                            .map(|_| ())
                            .map_err(|e| anyhow!("send_train: {}", e));
                        if let Err(e) = send_res {
                            error!("❌ Failed to send tunnel data: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("❌ Error reading from client: {}", e);
                        break;
                    }
                }
            }
            debug!("🔚 Client-to-tunnel task finished for tunnel #{}", request_id);
        });

        debug!("🚇 Tunnel #{}: background task started, returning from handle_connect_p2p", request_id);

        Ok(())
    }
}
