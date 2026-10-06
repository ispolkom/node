// src/netlayer/cli.rs
//! P2P CLI Commands
//! ================
//!
//! Интерактивные команды для управления P2P нодой

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{info, error, warn, debug};

use crate::netlayer::P2PTransport;
use crate::util::HashId;
use crate::proxy::ProxyResponse;
use crate::socks5::{Socks5ProxyResponse, Socks5ProxyRequest, Socks5TunnelData};
use crate::netlayer::YandiTunManager;
use crate::netlayer::tun_exit::TunExitHandler;
use crate::netlayer::relay::{RelayManager, RelaySession, RelaySessionStatus};
use crate::netlayer::nat::NatStatus;

/// CLI менеджер для управления нодой
pub struct P2PCli {
    transport: Arc<P2PTransport>,
    running: Arc<AtomicBool>,
    proxy_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>>>>,
    proxy_req_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyRequest)>>>>,
    proxy_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyTunnelData)>>>>,
    // 🔄 NACK channel (для wagon retransmission)
    nack_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::protocol::WagonNack)>>>>,
    // SOCKS5 channels
    socks5_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyResponse)>>>>,
    socks5_req_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyRequest)>>>>,
    socks5_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5TunnelData)>>>>,
    // TUN wagon channel
    tun_wagon_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::netlayer::tun_exit::TunWagon)>>>>,
    tun_wagon_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::netlayer::tun_exit::TunWagonResponse)>>>>,
    // TUN Manager
    tun_manager: Arc<RwLock<Option<YandiTunManager>>>,
    // P2P Tunnel Manager
    p2p_tunnel_manager: Option<crate::p2p_tunnel::P2PTunnelManager>,
}

impl P2PCli {
    /// Создать новый CLI менеджер
    pub fn new(transport: Arc<P2PTransport>) -> Self {
        Self {
            transport,
            running: Arc::new(AtomicBool::new(true)),
            proxy_resp_rx: Arc::new(tokio::sync::Mutex::new(None)),
            proxy_req_rx: Arc::new(tokio::sync::Mutex::new(None)),
            proxy_tunnel_rx: Arc::new(tokio::sync::Mutex::new(None)),
            nack_rx: Arc::new(tokio::sync::Mutex::new(None)), // 🔄 NACK channel
            socks5_resp_rx: Arc::new(tokio::sync::Mutex::new(None)),
            socks5_req_rx: Arc::new(tokio::sync::Mutex::new(None)),
            socks5_tunnel_rx: Arc::new(tokio::sync::Mutex::new(None)),
            tun_wagon_rx: Arc::new(tokio::sync::Mutex::new(None)),
            tun_wagon_resp_rx: Arc::new(tokio::sync::Mutex::new(None)),
            tun_manager: Arc::new(RwLock::new(None)),
            p2p_tunnel_manager: None,
        }
    }

    /// Set proxy response channel
    pub fn with_proxy_response_channel(mut self, rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>>>>) -> Self {
        self.proxy_resp_rx = rx;
        self
    }

    /// Set proxy request channel (for gateway)
    pub fn with_proxy_request_channel(mut self, rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyRequest)>>>>) -> Self {
        self.proxy_req_rx = rx;
        self
    }

    /// Set proxy tunnel data channel (for CONNECT)
    pub fn with_proxy_tunnel_data_channel(mut self, rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyTunnelData)>>>>) -> Self {
        self.proxy_tunnel_rx = rx;
        self
    }

    /// Set NACK channel (для wagon retransmission)
    pub fn with_nack_channel(mut self, rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::protocol::WagonNack)>>>>) -> Self {
        self.nack_rx = rx;
        self
    }

    /// Set SOCKS5 response channel
    pub fn with_socks5_response_channel(mut self, rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyResponse)>>>>) -> Self {
        self.socks5_resp_rx = rx;
        self
    }

    /// Set SOCKS5 request channel (for exit node)
    pub fn with_socks5_request_channel(mut self, rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyRequest)>>>>) -> Self {
        self.socks5_req_rx = rx;
        self
    }

    /// Set SOCKS5 tunnel data channel (for CONNECT)
    pub fn with_socks5_tunnel_data_channel(mut self, rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5TunnelData)>>>>) -> Self {
        self.socks5_tunnel_rx = rx;
        self
    }

    /// Set TUN wagon channel
    pub fn with_tun_wagon_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, crate::netlayer::tun_exit::TunWagon)>) -> Self {
        self.tun_wagon_rx = Arc::new(tokio::sync::Mutex::new(Some(rx)));
        self
    }

    /// Set TUN wagon response channel
    pub fn with_tun_wagon_response_channel(mut self, rx: tokio::sync::mpsc::Receiver<(HashId, crate::netlayer::tun_exit::TunWagonResponse)>) -> Self {
        self.tun_wagon_resp_rx = Arc::new(tokio::sync::Mutex::new(Some(rx)));
        self
    }

    /// Set TUN manager
    pub fn with_tun_manager(mut self, manager: YandiTunManager) -> Self {
        self.tun_manager = Arc::new(RwLock::new(Some(manager)));
        self
    }

    /// Set P2P Tunnel manager
    pub fn with_p2p_tunnel_manager(mut self, manager: crate::p2p_tunnel::P2PTunnelManager) -> Self {
        self.p2p_tunnel_manager = Some(manager);
        self
    }

    /// Запустить CLI в отдельной задаче
    pub fn spawn(self) {
        let running = self.running.clone();
        let transport = self.transport.clone();
        let tun_manager = self.tun_manager.clone();
        let proxy_resp_rx = self.proxy_resp_rx.clone();
        let proxy_req_rx = self.proxy_req_rx.clone();
        let proxy_tunnel_rx = self.proxy_tunnel_rx.clone();
        let nack_rx = self.nack_rx.clone(); // 🔄 NACK channel
        let socks5_resp_rx = self.socks5_resp_rx.clone();
        let socks5_req_rx = self.socks5_req_rx.clone();
        let socks5_tunnel_rx = self.socks5_tunnel_rx.clone();
        let tun_wagon_rx = self.tun_wagon_rx.clone();
        let tun_wagon_resp_rx = self.tun_wagon_resp_rx.clone();
        let p2p_tunnel_manager = self.p2p_tunnel_manager.clone();

        println!("🎮 CLI команды:");
        println!("   hello <IP:PORT>    - отправить Hello запрос");
        println!("   send <SHORT_ID>    - отправить зашифрованное сообщение");
        println!("   peers              - показать известных пиров с Short ID");
        println!("   socks5 <SHORT_ID>  - запустить SOCKS5 Proxy через пира");
        println!("   socks5 auto [NL]   - быстро: выход выбирается сам, страна по желанию");
        println!("   socks5 hops [NL]   - анонимно: цепочка через три узла");
        println!("   proxy <SHORT_ID>   - запустить HTTP Proxy (DPI bypass) через пира");
        println!("   proxy-gateway      - запустить HTTP Proxy Gateway (на exit node)");
        println!("   relay-server       - включить режим relay сервера");
        println!("   relay-connect <ID> - подключиться к пиру через relay");
        println!("   relay-list         - показать активные relay сессии");
        println!("   tun link <ID>      - связать TUN с exit node (entry node)");
        println!("   tun exit           - открыть внешний трафик (exit node)");
        println!("   exit               - запустить Exit Node Handler (SOCKS5/Proxy)");
        println!("   help               - показать справку");
        println!("   quit               - остановить ноду");
        println!();

        tokio::spawn(async move {
            // Читаем команды из stdin
            use tokio::io::AsyncBufReadExt;
            let stdin = tokio::io::stdin();
            let mut reader = tokio::io::BufReader::new(stdin);
            let mut line = String::new();

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                print!("> ");
                use std::io::Write;
                let _ = std::io::stdout().flush();

                line.clear();

                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        // EOF - нормальное завершение
                        println!("\n👋 Получен EOF");
                        break;
                    }
                    Ok(n) => {
                        let cmd = line.trim();
                        if cmd.is_empty() {
                            continue;
                        }

                        if let Err(e) = Self::handle_command(
                            cmd,
                            &transport,
                            tun_manager.clone(),
                            proxy_resp_rx.clone(),
                            proxy_req_rx.clone(),
                            proxy_tunnel_rx.clone(),
                            nack_rx.clone(), // 🔄 NACK channel
                            socks5_resp_rx.clone(),
                            socks5_req_rx.clone(),
                            socks5_tunnel_rx.clone(),
                            tun_wagon_rx.clone(),
                            tun_wagon_resp_rx.clone(),
                            p2p_tunnel_manager.clone()
                        ).await {
                            println!("❌ Ошибка команды: {}", e);
                        }
                    }
                    Err(e) => {
                        // Игнорируем ошибки чтения и продолжаем
                        eprintln!("[cli] ❌ Ошибка чтения (kind={:?}): {}", e.kind(), e);
                        break;
                    }
                }
            }

            println!("👋 CLI остановлен");
        });
    }

    /// Обработать команду
    async fn handle_command(
        cmd: &str,
        transport: &P2PTransport,
        tun_manager: Arc<RwLock<Option<YandiTunManager>>>,
        proxy_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>>>>,
        proxy_req_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyRequest)>>>>,
        proxy_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyTunnelData)>>>>,
        nack_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::protocol::WagonNack)>>>>, // 🔄 NACK channel
        socks5_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyResponse)>>>>,
        socks5_req_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5ProxyRequest)>>>>,
        socks5_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, Socks5TunnelData)>>>>,
        tun_wagon_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::netlayer::tun_exit::TunWagon)>>>>,
        tun_wagon_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::netlayer::tun_exit::TunWagonResponse)>>>>,
        p2p_tunnel_manager: Option<crate::p2p_tunnel::P2PTunnelManager>,
    ) -> Result<(), String> {
        let parts: Vec<&str> = cmd.split_whitespace().collect();

        if parts.is_empty() {
            return Ok(());
        }

        match parts[0] {
            "hello" => {
                if parts.len() < 2 {
                    println!("Использование: hello <IP:PORT>");
                    println!("Пример: hello 89.123.45.67:9000");
                    return Ok(());
                }

                let addr = parts[1];
                println!("📤 Отправка Hello запроса на {}...", addr);

                transport.send_hello_request(addr).await?;

                println!("✅ Hello запрос отправлен! Ожидайте ответа...");
            }

            "send" => {
                if parts.len() < 2 {
                    println!("Использование: send <SHORT_ID> <MESSAGE>");
                    println!("Пример: send 8283219e Hello from Russia!");
                    println!();
                    println!("Используйте 'peers' чтобы узнать Short ID пира (первые 8 байт)");
                    return Ok(());
                }

                let short_id_hex = parts[1];

                // Парсим короткий ID (8 байт = 16 hex символов)
                let short_id_bytes = hex::decode(short_id_hex)
                    .map_err(|_| format!("Неверный формат Short ID: {}", short_id_hex))?;

                if short_id_bytes.len() != 8 {
                    return Err(format!("Short ID должен быть 8 байт (16 hex), получено: {}", short_id_bytes.len()));
                }

                // Ищем пира по короткому ID
                let peers = transport.get_peers().await;
                let peer = peers.iter()
                    .find(|p| &p.id.0[..8] == short_id_bytes.as_slice())
                    .ok_or_else(|| format!("Пир с Short ID {} не найден", short_id_hex))?;

                let message = if parts.len() > 2 {
                    parts[2..].join(" ")
                } else {
                    "Hello from YANDI node!".to_string()
                };

                println!("📤 Отправка зашифрованного сообщения пиру {}...", crate::util::mask_hash_id(&peer.id));
                println!("   Текст: {}", message);

                transport.send_encrypted(peer.id, message.as_bytes()).await?;

                println!("✅ Сообщение отправлено через порт 10000!");
            }

            "peers" => {
                println!("📋 Известные пиры:");

                let peers = transport.get_peers().await;

                if peers.is_empty() {
                    println!("   (нет известных пиров)");
                } else {
                    for (i, peer) in peers.iter().enumerate() {
                        let short_id = hex::encode(&peer.id.0[..8]);
                        let nat_status = peer.get_nat_status();
                        let nat_icon = match nat_status {
                            NatStatus::Public => "🌐",
                            NatStatus::BehindNat => "🔒",
                            NatStatus::MultiHomed => "🔄",
                            _ => "❓",
                        };
                        println!("   {}. {} (Short ID: {}) {}", i + 1, peer.addr, short_id, nat_icon);
                        println!("      Node ID: {}", crate::util::mask_hash_id(&peer.id));
                        println!("      NAT: {}", nat_status.as_str());
                        if let Some(ref data_addr) = peer.data_addr {
                            // Не маскируем data_addr - нужен для отладки
                            println!("      Data endpoint: {}", data_addr);
                        }
                    }
                }

                println!();
            }

            "socks5" => {
                Self::handle_socks5_command(&parts, transport, socks5_resp_rx, socks5_tunnel_rx.clone()).await?
            }

            "proxy" => {
                Self::handle_proxy_command(&parts, transport, proxy_resp_rx, proxy_tunnel_rx.clone()).await?
            }

            "proxy-gateway" => {
                Self::handle_proxy_gateway_command(transport, proxy_req_rx, proxy_tunnel_rx.clone(), nack_rx.clone()).await?
            }

            "socks5-gateway" => {
                Self::handle_socks5_gateway_command(transport, socks5_req_rx, socks5_tunnel_rx.clone(), nack_rx.clone()).await?
            }

            "relay-server" => {
                Self::handle_relay_server_command(transport).await?
            }

            "relay-connect" => {
                if parts.len() < 2 {
                    println!("Использование: relay-connect <SHORT_ID>");
                    println!("Пример: relay-connect 0021944b");
                    println!();
                    println!("Подключается к пиру через публичный relay сервер");
                    println!("Используется для пиров за NAT");
                    return Ok(());
                }

                let short_id_hex = parts[1];
                Self::handle_relay_connect_command(transport, short_id_hex).await?
            }

            "relay-list" => {
                Self::handle_relay_list_command(transport).await?
            }

            "tcp" => {
                if parts.len() < 2 {
                    println!("Использование: tcp <SHORT_ID>");
                    println!("Пример: tcp 0021944b");
                    println!();
                    println!("Запускает Raw TCP Tunnel over HTTP CONNECT на 127.0.0.1:9090");
                    println!("весь трафик будет проксироваться через указанного пира (exit node)");
                    println!();
                    println!("⚡ Это экспериментальная функция для максимальной скорости!");
                    println!("   Убирает SOCKS5 и JSON слои, отправляя raw bytes в YTP wagon");
                    println!();
                    println!("Используйте 'peers' чтобы узнать Short ID пира");
                    return Ok(());
                }

                let short_id_hex = parts[1];

                // Парсим короткий ID
                let short_id_bytes = hex::decode(short_id_hex)
                    .map_err(|_| format!("Неверный формат Short ID: {}", short_id_hex))?;

                if short_id_bytes.len() != 8 {
                    return Err(format!("Short ID должен быть 8 байт (16 hex), получено: {}", short_id_bytes.len()));
                }

                // Ищем пира по короткому ID
                let peers = transport.get_peers().await;
                let exit_peer = peers.iter()
                    .find(|p| &p.id.0[..8] == short_id_bytes.as_slice())
                    .ok_or_else(|| format!("Пир с Short ID {} не найден", short_id_hex))?;

                println!("🚇 Запуск Raw TCP Tunnel...");
                println!("   HTTP Proxy на 127.0.0.1:9090 → Exit Node {} ({})", exit_peer.addr, short_id_hex);
                println!();

                // Создаем TcpTunnel
                use crate::protocol::TcpTunnel;

                let transport_for_tcp = transport.clone();
                let exit_peer_id = exit_peer.id;

                let tcp_tunnel = TcpTunnel::new(
                    std::sync::Arc::new(transport_for_tcp),
                    exit_peer_id,
                    9090  // Порт 9090 как в плане
                );

                println!("✅ Raw TCP Tunnel запускается на http://127.0.0.1:9090");
                println!("📡 Configure browser: HTTP Proxy = 127.0.0.1:9090");
                println!();

                tokio::spawn(async move {
                    if let Err(e) = tcp_tunnel.run().await {
                        eprintln!("❌ TCP Tunnel Error: {}", e);
                    }
                });

                println!("✅ TCP Tunnel активен!");
                println!("⚡ Raw bytes → YTP wagon → Exit Node → Интернет");
                println!();
            }

            "tunnel" => {
                if parts.len() < 2 {
                    println!("Использование: tunnel <start|stop|status|list>");
                    println!("   tunnel start <SHORT_ID> [type]  - создать P2P тоннель");
                    println!("   tunnel stop <SHORT_ID>           - закрыть P2P тоннель");
                    println!("   tunnel status                   - показать статус тоннелей");
                    println!("   tunnel list                     - список активных тоннелей");
                    println!();
                    println!("Типы тоннелей:");
                    println!("   voice       - 📞 Голосовой звонок (VoIP)");
                    println!("   video       - 📹 Видеосвязь");
                    println!("   file        - 📎 Передача файлов P2P");
                    println!("   gaming      - 🎮 Игры P2P");
                    println!("   generic     - 🔗 Универсальный P2P тоннель");
                    println!();
                    println!("ПРИМЕР:");
                    println!("   tunnel start 0021944b voice    # Голосовой звонок");
                    println!("   tunnel stop 0021944b");
                    println!();
                    println!("💡 Это ЧИСТЫЙ P2P тоннель БЕЗ выхода в интернет!");
                    println!("   Используется для direct peer-to-peer связи.");
                    return Ok(());
                }

                match parts[1] {
                    "start" => {
                        if parts.len() < 3 {
                            println!("❌ Не указан SHORT_ID!");
                            println!("   Использование: tunnel start <SHORT_ID> [type]");
                            println!("   Пример: tunnel start 0021944b voice");
                            return Ok(());
                        }

                        let short_id = parts[2];

                        // Определить тип тоннеля
                        let tunnel_type = match parts.get(3) {
                            Some(&"voice") => crate::p2p_tunnel::TunnelType::Voice,
                            Some(&"video") => crate::p2p_tunnel::TunnelType::Video,
                            Some(&"file") => crate::p2p_tunnel::TunnelType::FileTransfer,
                            Some(&"gaming") => crate::p2p_tunnel::TunnelType::Gaming,
                            _ => crate::p2p_tunnel::TunnelType::Generic,
                        };

                        println!("🔗 Запрос P2P тоннеля с {} (type: {:?})",
                            short_id, tunnel_type);

                        // Найти peer по short_id
                        let peer_id = match transport.find_peer_by_short_id(short_id) {
                            Some(id) => id,
                            None => {
                                println!("❌ Пир с SHORT_ID={} не найден!", short_id);
                                println!("   Запустите 'peers' чтобы увидеть список пиров");
                                return Ok(());
                            }
                        };

                        // Получить менеджер
                        let tunnel_manager = p2p_tunnel_manager.as_ref()
                            .ok_or("P2P Tunnel Manager не инициализирован!")?;

                        // Создать тоннель
                        match tunnel_manager.request_tunnel(peer_id, tunnel_type).await {
                            Ok(_) => {
                                println!("✅ Запрос на тоннель отправлен!");
                                println!("   Ожидание подтверждения от {}...", short_id);
                            }
                            Err(e) => {
                                println!("❌ Ошибка создания тоннеля: {}", e);
                            }
                        }
                    }

                    "stop" => {
                        if parts.len() < 3 {
                            println!("❌ Не указан SHORT_ID!");
                            println!("   Использование: tunnel stop <SHORT_ID>");
                            return Ok(());
                        }

                        let short_id = parts[2];

                        println!("🔒 Закрытие P2P тоннеля с {}", short_id);

                        // Найти peer
                        let peer_id = match transport.find_peer_by_short_id(short_id) {
                            Some(id) => id,
                            None => {
                                println!("❌ Пир с SHORT_ID={} не найден!", short_id);
                                return Ok(());
                            }
                        };

                        // Получить менеджер
                        let tunnel_manager = p2p_tunnel_manager.as_ref()
                            .ok_or("P2P Tunnel Manager не инициализирован!")?;

                        // Закрыть тоннель
                        match tunnel_manager.close_tunnel(peer_id).await {
                            Ok(_) => {
                                println!("✅ Тоннель с {} закрыт!", short_id);
                            }
                            Err(e) => {
                                println!("❌ Ошибка закрытия тоннеля: {}", e);
                            }
                        }
                    }

                    "status" => {
                        let tunnel_manager = p2p_tunnel_manager.as_ref()
                            .ok_or("P2P Tunnel Manager не инициализирован!")?;

                        let tunnels = tunnel_manager.list_tunnels().await;

                        if tunnels.is_empty() {
                            println!("⚠️  Нет активных P2P тоннелей");
                        } else {
                            println!("📊 Активные P2P тоннели:");
                            for tunnel in tunnels {
                                println!("   🆔 {}", hex::encode(&tunnel.peer.0[..8]));
                                println!("      Type: {:?}", tunnel.tunnel_type);
                                println!("      Status: {:?}", tunnel.status);
                                println!("      Sent: {} bytes", tunnel.bytes_sent);
                                println!("      Received: {} bytes", tunnel.bytes_received);
                                println!();
                            }
                        }
                    }

                    "list" => {
                        let tunnel_manager = p2p_tunnel_manager.as_ref()
                            .ok_or("P2P Tunnel Manager не инициализирован!")?;

                        let tunnels = tunnel_manager.list_tunnels().await;

                        if tunnels.is_empty() {
                            println!("⚠️  Нет активных P2P тоннелей");
                        } else {
                            println!("🔗 Активные P2P тоннели:");
                            for tunnel in tunnels {
                                let icon = match tunnel.tunnel_type {
                                    crate::p2p_tunnel::TunnelType::Voice => "📞",
                                    crate::p2p_tunnel::TunnelType::Video => "📹",
                                    crate::p2p_tunnel::TunnelType::FileTransfer => "📎",
                                    crate::p2p_tunnel::TunnelType::Gaming => "🎮",
                                    _ => "🔗",
                                };

                                println!("   {} {} - {:?}",
                                    icon,
                                    hex::encode(&tunnel.peer.0[..8]),
                                    tunnel.tunnel_type
                                );
                            }
                        }
                    }

                    _ => {
                        println!("❌ Неизвестная команда: {}", parts[1]);
                        println!("   Доступно: start, stop, status, list");
                    }
                }
            }

            "exit" => {
                println!("🚪 Запуск SOCKS5 Exit Node Handler...");
                println!();
                println!("   Эта нода будет принимать входящие CONNECT запросы");
                println!("   от SOCKS5 Proxy клиентов и проксировать трафик");
                println!("   в интернет.");
                println!();

                use crate::socks5::ExitNodeHandler;

                // Take receivers
                let socks5_req = { socks5_req_rx.lock().await.take() };
                let socks5_tunnel = { socks5_tunnel_rx.lock().await.take() };

                if socks5_req.is_none() || socks5_tunnel.is_none() {
                    println!("⚠️  Ошибка: SOCKS5 каналы не настроены!");
                    return Ok(());
                }

                let exit_handler = ExitNodeHandler::new(std::sync::Arc::new(transport.clone()))
                    .with_request_channel(socks5_req.unwrap())
                    .with_tunnel_data_channel(socks5_tunnel.unwrap());

                println!("✅ SOCKS5 Exit Node запускается...");

                tokio::spawn(async move {
                    if let Err(e) = exit_handler.run().await {
                        eprintln!("❌ SOCKS5 Exit Node error: {}", e);
                    }
                });

                println!("✅ SOCKS5 Exit Node запущен в фоне!");
            }

            "tcp-exit" => {
                println!("🚇 Запуск TCP Tunnel Exit Handler...");
                println!();
                println!("   Эта нода будет принимать входящие HTTP/HTTPS CONNECT запросы");
                println!("   от TcpTunnel клиентов и проксировать трафик в интернет.");
                println!("   Raw TCP bytes → YTP wagons → Internet");
                println!();

                use crate::protocol::TcpTunnelExitHandler;

                let transport_clone = std::sync::Arc::new(transport.clone());

                println!("✅ TCP Tunnel Exit Handler запускается...");

                tokio::spawn(async move {
                    let exit_handler = TcpTunnelExitHandler::new(transport_clone).await;
                    if let Err(e) = exit_handler.run().await {
                        eprintln!("❌ TCP Tunnel Exit error: {}", e);
                    }
                });

                println!("✅ TCP Tunnel Exit Handler запущен в фоне!");
                println!("   Теперь entry nodes могут использовать эту ноду как gateway.");
            }

            "rawip-exit" => {
                println!("🚀 Запуск RawIP Tunnel Exit Handler...");
                println!();
                println!("   Эта нода будет принимать сырые IP пакеты через TCP (порт 10001)");
                println!("   от Entry Nodes и проксировать их в интернет.");
                println!("   IP packets (RawIP protocol) → Internet");
                println!();

                use crate::netlayer::RawIpTunnel;

                let transport_clone = std::sync::Arc::new(transport.clone());

                println!("✅ RawIP Tunnel запускается...");

                tokio::spawn(async move {
                    let station_clone = {
                        let station_opt = transport_clone.station.lock().await;
                        station_opt.as_ref().unwrap().clone()
                    };

                    let rawip_tunnel = RawIpTunnel::new(station_clone, 10001);
                    if let Err(e) = rawip_tunnel.run().await {
                        eprintln!("❌ RawIP Tunnel error: {}", e);
                    }
                });

                println!("✅ RawIP Tunnel запущен на порту 10001!");
                println!("   Теперь мобильные приложения могут подключаться.");
            }

            "tun" => {
                if parts.len() < 2 {
                    println!("Использование: tun <init|status|link|exit|info>");
                    println!("   tun init                - создать TUN устройства");
                    println!("   tun status              - показать статус TUN устройств");
                    println!("   tun link <SHORT_ID>     - связать TUN с exit node (на entry node)");
                    println!("   tun exit                - открыть внешний трафик (на exit node)");
                    println!("   tun info                - показать информацию о TUN устройствах");
                    println!();
                    println!("ПРИМЕР:");
                    println!("   Нода 2 (exit): tun exit");
                    println!("   Нода 1 (entry): tun link 0021944bf8ffc764");
                    return Ok(());
                }

                match parts[1] {
                    "init" => {
                        println!("🌐 Инициализация TUN устройств...");

                        use crate::netlayer::YandiTunManager;

                        match YandiTunManager::new() {
                            Ok(mut manager) => {
                                // Создаём оба устройства
                                if let Err(e) = manager.create_client_device() {
                                    eprintln!("   ⚠️  Ошибка создания yandi_client: {}", e);
                                }
                                if let Err(e) = manager.create_p2p_device() {
                                    eprintln!("   ⚠️  Ошибка создания yandi_p2p: {}", e);
                                }

                                // Запускаем устройства
                                if let Err(e) = manager.start_all().await {
                                    eprintln!("   ⚠️  Ошибка запуска TUN устройств: {}", e);
                                } else {
                                    println!("   ✅ TUN устройства запущены");
                                    println!("   📱 yandi_client: fd00::1/64");
                                    println!("   🔗 yandi_p2p: fc00:1234:5678:1::1/64");
                                }

                                // Сохраняем в tun_manager
                                let mut tun_mgr = tun_manager.write().await;
                                *tun_mgr = Some(manager);
                            }
                            Err(e) => {
                                eprintln!("   ❌ Ошибка создания TUN менеджера: {}", e);
                            }
                        }
                    }

                    "status" => {
                        let manager = tun_manager.read().await;
                        if manager.is_some() {
                            println!("✅ TUN менеджер инициализирован");
                        } else {
                            println!("⚠️  TUN менеджер НЕ инициализирован");
                            println!("   Запустите: tun init");
                        }
                    }

                    "link" => {
                        // Check if SHORT_ID provided
                        if parts.len() < 3 {
                            println!("❌ Не указан SHORT_ID exit node!");
                            println!("   Использование: tun link <SHORT_ID>");
                            println!();
                            println!("   Шаг 1: На exit node запустите: tun exit");
                            println!("   Шаг 2: Здесь: peers  (чтобы увидеть SHORT_ID)");
                            println!("   Шаг 3: Здесь: tun link <SHORT_ID>");
                            return Ok(());
                        }

                        let short_id = parts[2];

                        println!("🔗 Связывание TUN с exit node: {}", short_id);
                        let mut manager = tun_manager.write().await;

                        if manager.is_none() {
                            println!("❌ TUN менеджер не инициализирован!");
                            println!("   Запустите: tun init");
                            return Ok(());
                        }

                        let mgr = manager.as_mut().unwrap();

                        // Найти peer по short_id
                        let peer_id = match transport.find_peer_by_short_id(short_id) {
                            Some(id) => id,
                            None => {
                                println!("❌ Пир с SHORT_ID={} не найден!", short_id);
                                println!("   Запустите 'peers' чтобы увидеть список пиров");
                                return Ok(());
                            }
                        };

                        println!("   ✅ Найден пир: {}", short_id);

                        // Подключаем каналы к client device
                        if let Some(client) = mgr.client_mut() {
                            client.set_transport(Arc::new(transport.clone()));
                            client.set_exit_node(peer_id.clone());

                            // Подключаем tun_wagon_tx канал
                            if let Some(tx) = transport.tun_wagon_tx() {
                                client.set_tun_wagon_channel(tx);
                                println!("   ✅ TunWagon канал подключён к yandi_client");
                            } else {
                                println!("   ⚠️  TunWagon канал не найден в P2PTransport!");
                            }

                            println!("   ✅ yandi_client связан с P2P transport");
                            println!("   🎯 Exit peer: {}", short_id);
                        }

                        // Подключаем p2p device
                        if let Some(p2p) = mgr.p2p_mut() {
                            p2p.set_transport(Arc::new(transport.clone()));
                            println!("   ✅ yandi_p2p связан с P2P transport");
                        }

                        // Подключаем tun_wagon_resp_rx канал
                        let tun_wagon_resp_rx = {
                            let mut rx_lock = tun_wagon_resp_rx.lock().await;
                            rx_lock.take()
                        };

                        if let Some(rx) = tun_wagon_resp_rx {
                            mgr.set_tun_wagon_response_channel(rx);
                            println!("   ✅ TunWagonResponse канал подключён");

                            // Запускаем обработчик
                            if let Err(e) = mgr.start_tun_wagon_response_handler().await {
                                println!("   ⚠️  Ошибка запуска обработчика: {}", e);
                            } else {
                                println!("   ✅ TunWagonResponse обработчик запущен");
                            }
                        } else {
                            println!("   ⚠️  TunWagonResponse канал не настроен!");
                        }

                        println!("✅ TUN устройства связаны с exit node {}!", short_id);
                        println!("💡 Трафик пойдёт через {}", short_id);
                        println!("💡 Теперь можно настраивать маршрутизацию: ip route add ...");
                    }

                    "exit" => {
                        println!("🌍 Запуск TUN Exit Node...");

                        // Извлекаем TUN wagon channel
                        let wagon_rx = {
                            let mut rx_lock = tun_wagon_rx.lock().await;
                            rx_lock.take().ok_or("TUN wagon channel already taken")?
                        };

                        let transport_clone = Arc::new(transport.clone());

                        tokio::spawn(async move {
                            match TunExitHandler::new(transport_clone, wagon_rx).await {
                                Ok(mut tun_exit) => {
                                    if let Err(e) = tun_exit.run().await {
                                        eprintln!("❌ TUN Exit Node error: {}", e);
                                    }
                                }
                                Err(e) => {
                                    eprintln!("❌ Failed to create TUN Exit Handler: {}", e);
                                }
                            }
                        });

                        println!("✅ TUN Exit Node запускается в фоне...");
                        println!("💡 Ожидание TUN wagons через P2PTransport (порт 10000)...");
                    }

                    "info" => {
                        let manager = tun_manager.read().await;
                        if manager.is_none() {
                            println!("⚠️  TUN менеджер не инициализирован");
                            println!("   Запустите: tun init");
                            return Ok(());
                        }

                        let mgr = manager.as_ref().unwrap();
                        println!("📱 TUN устройства:");
                        if let Some(client) = mgr.client() {
                            println!("   yandi_client: {}", client.ipv6_addr);
                        }
                        if let Some(p2p) = mgr.p2p() {
                            println!("   yandi_p2p: {}", p2p.ipv6_addr);
                        }
                    }

                    _ => {
                        println!("❌ Неизвестная команда: {}", parts[1]);
                        println!("   Доступно: init, status, link, exit, info");
                    }
                }
            }

            "help" => {
                println!("🎮 Доступные команды:");
                println!("   hello <IP:PORT>    - отправить Hello запрос на ноду");
                println!("   send <SHORT_ID>    - отправить зашифрованное сообщение");
                println!("   peers              - список известных пиров с Short ID и NAT статусом");
                println!("   socks5 <SHORT_ID>  - запустить SOCKS5 Proxy через пира");
                println!("   proxy <SHORT_ID>   - запустить HTTP Proxy (DPI bypass) через пира");
                println!("   proxy-gateway      - запустить HTTP Proxy Gateway (на exit node)");
                println!("   relay-server       - включить режим relay сервера");
                println!("   relay-connect <ID> - подключиться к пиру через relay");
                println!("   relay-list         - показать активные relay сессии");
                println!("   tcp <SHORT_ID>     - запустить TCP Tunnel через пира (entry node)");
                println!("   tcp-exit           - запустить TCP Tunnel Exit Handler (gateway)");
                println!("   rawip-exit         - запустить RawIP Tunnel (порт 10001) для мобильных приложений");
                println!("   tunnel <CMD>       - управление P2P тоннелями (voice/video/file)");
                println!("   tun init           - создать TUN устройства");
                println!("   tun link <ID>      - связать TUN с exit node (entry node)");
                println!("   tun exit           - открыть внешний трафик (exit node)");
                println!("   exit               - запустить Exit Node Handler (SOCKS5/Proxy)");
                println!("   help               - показать эту справку");
                println!("   quit               - остановить ноду");
                println!();
            }

            "quit" | "q" => {
                println!("🛑 Остановка ноды...");
                transport.hello_sender().clone(); // Keep alive for shutdown
                std::process::exit(0);
            }

            _ => {
                println!("❌ Неизвестная команда: {}", parts[0]);
                println!("   Введите 'help' для списка команд");
            }
        }

        Ok(())
    }

    /// Остановить CLI
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    /// Handle proxy command
    async fn handle_proxy_command(
        parts: &[&str],
        transport: &P2PTransport,
        proxy_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, ProxyResponse)>>>>,
        proxy_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyTunnelData)>>>>,
    ) -> Result<(), String> {
        if parts.len() < 2 {
            println!("Использование: proxy <SHORT_ID>");
            println!("Пример: proxy 0021944b");
            println!();
            println!("Запускает HTTP Proxy на 127.0.0.1:8080");
            println!("весь трафик будет проксироваться через указанного пира (gateway node)");
            println!();
            println!("Для обхода DPI:");
            println!("  - Браузер делает HTTP запрос на localhost:8080");
            println!("  - Формат: http://localhost:8080/youtube.com/watch?v=xxx");
            println!("  - Proxy конвертирует в HTTPS и отправляет через P2P");
            println!("  - Gateway делает реальный запрос от своего имени");
            println!();
            println!("Используйте 'peers' чтобы узнать Short ID пира");
            return Ok(());
        }

        let short_id_hex = parts[1];

        // Парсим короткий ID
        let short_id_bytes = hex::decode(short_id_hex)
            .map_err(|_| format!("Неверный формат Short ID: {}", short_id_hex))?;

        if short_id_bytes.len() != 8 {
            return Err(format!("Short ID должен быть 8 байт (16 hex), получено: {}", short_id_bytes.len()));
        }

        // Ищем пира по короткому ID
        let peers = transport.get_peers().await;
        let gateway_peer = peers.iter()
            .find(|p| &p.id.0[..8] == short_id_bytes.as_slice())
            .ok_or_else(|| format!("Пир с Short ID {} не найден", short_id_hex))?;

        println!("🌐 Запуск HTTP Proxy (DPI bypass)...");
        println!("   1. Запуск локального HTTP Proxy на 127.0.0.1:8080");
        println!("   2. Gateway node: {} ({})", gateway_peer.addr, short_id_hex);
        println!();

        // 1. ЗАПУСКАЕМ ЛОКАЛЬНЫЙ HTTP PROXY НА CLIENT NODE
        use crate::proxy::HttpProxyClient;

        let transport_for_proxy = transport.clone();
        let gateway_node_id = gateway_peer.id;

        let mut http_proxy = HttpProxyClient::new(
            std::sync::Arc::new(transport_for_proxy),
            gateway_node_id
        );

        // Take the response receiver
        let resp_rx = {
            let mut rx_lock = proxy_resp_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = resp_rx {
            http_proxy = http_proxy.with_response_channel(rx);
            println!("✅ Response channel connected");
        } else {
            println!("⚠️  No response channel available - responses won't be delivered!");
        }

        // Take the tunnel data receiver
        let tunnel_rx = {
            let mut rx_lock = proxy_tunnel_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = tunnel_rx {
            http_proxy = http_proxy.with_tunnel_data_channel(rx);
            println!("✅ Tunnel data channel connected");
        } else {
            println!("⚠️  No tunnel data channel - CONNECT won't work!");
        }

        // Регистрируем Station в transport для обработки YTP пакетов
        transport.set_station(http_proxy.station.clone()).await;
        println!("🚂 YTP Station registered in transport");

        println!("✅ Локальный HTTP Proxy запускается на 127.0.0.1:8080");
        println!();
        println!("📝 Настройка браузера:");
        println!("   - HTTP Proxy: 127.0.0.1:8080");
        println!("   - NO authentication");
        println!("   - URL формат: http://localhost:8080/domain.com/path");
        println!();

        tokio::spawn(async move {
            if let Err(e) = http_proxy.start().await {
                eprintln!("❌ HTTP Proxy Error: {}", e);
            }
        });

        // 2. ОТПРАВЛЯЕМ КОМАНДУ НА GATEWAY NODE
        println!("📤 Отправка запроса на Gateway Node...");

        let cmd_bytes = vec![0x30u8]; // StartProxyGateway

        if let Err(e) = transport.send_encrypted(gateway_peer.id, &cmd_bytes).await {
            println!("❌ Не удалось отправить команду: {}", e);
            return Err(format!("Failed to send StartProxyGateway command: {}", e));
        }

        println!("✅ Запрос отправлен на ноду {}", short_id_hex);
        println!("⏳ Ожидание подтверждения от Gateway Node...");
        println!();
        println!("✅ HTTP Proxy запущен! Готов к приему запросов.");
        println!();

        Ok(())
    }

    /// Handle proxy-gateway command
    async fn handle_proxy_gateway_command(
        transport: &P2PTransport,
        proxy_req_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyRequest)>>>>,
        proxy_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::proxy::ProxyTunnelData)>>>>,
        nack_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::protocol::WagonNack)>>>>,
    ) -> Result<(), String> {
        println!("🌐 Запуск HTTP Proxy Gateway...");
        println!();
        println!("   Эта нода будет принимать HTTP proxy запросы через P2P,");
        println!("   делать реальные HTTPS запросы к целевым серверам");
        println!("   и возвращать ответы обратно через P2P.");
        println!();
        println!("   Gateway node будет делать запросы ОТ СВОЕГО ИМЕНИ.");
        println!();

        use crate::proxy::HttpProxyGateway;

        let mut gateway = HttpProxyGateway::new(std::sync::Arc::new(transport.clone()));

        // Take the request receiver
        let req_rx = {
            let mut rx_lock = proxy_req_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = req_rx {
            gateway = gateway.with_request_channel(rx);
            println!("✅ Request channel connected");
        } else {
            println!("⚠️  No request channel available - requests won't be received!");
        }

        // Take the tunnel data receiver
        let tunnel_rx = {
            let mut rx_lock = proxy_tunnel_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = tunnel_rx {
            gateway = gateway.with_tunnel_data_channel(rx);
            println!("✅ Tunnel data channel connected");
        } else {
            println!("⚠️  No tunnel data channel - CONNECT won't work!");
        }

        // 🔄 Take the NACK receiver (для wagon retransmission)
        let nack_rx_chan = {
            let mut rx_lock = nack_rx.lock().await;
            rx_lock.take()
        };

        if let Some(nack_rx) = nack_rx_chan {
            println!("✅ NACK channel connected - wagon retransmission enabled");

            // Запускаем NACK handler task
            let gateway_clone = gateway.clone_for_handler();
            tokio::spawn(async move {
                Self::handle_nack_packets(gateway_clone, nack_rx).await;
            });
        } else {
            println!("⚠️  No NACK channel - wagon retransmission disabled!");
        }

        // Регистрируем Station в transport для обработки YTP пакетов
        transport.set_station(gateway.station.clone()).await;
        println!("🚂 YTP Station registered in transport");

        println!("✅ HTTP Proxy Gateway запущен!");
        println!("   Ожидание proxy запросов через P2P...");

        tokio::spawn(async move {
            if let Err(e) = gateway.run().await {
                eprintln!("❌ Proxy Gateway Error: {}", e);
            }
        });

        println!();

        Ok(())
    }

    /// 🔄 Обработка NACK пакетов - повторная отправка потерянных wagon-ов
    async fn handle_nack_packets(
        gateway: crate::proxy::HttpProxyGateway,
        mut nack_rx: tokio::sync::mpsc::Receiver<(HashId, crate::protocol::WagonNack)>,
    ) {
        use crate::protocol::WagonNack;

        println!("🔄 NACK Handler started - listening for retransmission requests");

        while let Some((source_node, nack)) = nack_rx.recv().await {
            info!("🔄 Received NACK from {} for train #{}: {} missing wagons",
                  hex::encode(&source_node.0[..8]), nack.train_id, nack.missing_wagons.len());

            // Получаем отправленные wagons из хранилища
            let sent_trains = gateway.sent_trains.clone();
            let mut trains_lock = sent_trains.lock().await;

            if let Some(sent_train) = trains_lock.get_mut(&nack.train_id) {
                info!("📦 Found train #{} in storage, retransmitting {} wagons",
                      nack.train_id, nack.missing_wagons.len());

                let mut retransmitted = 0;
                for wagon_num in &nack.missing_wagons {
                    if let Some(wagon_bytes) = sent_train.get_wagon(*wagon_num) {
                        // Повторно отправляем wagon
                        let target_node = sent_train.target_node;
                        let mut packet = vec![0x60u8]; // YTP Wagon prefix
                        packet.extend_from_slice(wagon_bytes);

                        if let Err(e) = gateway.transport.send_encrypted(source_node, &packet).await {
                            error!("❌ Failed to retransmit wagon #{} of train #{}: {}",
                                   wagon_num, nack.train_id, e);
                        } else {
                            debug!("✅ Retransmitted wagon #{} of train #{} ({} bytes)",
                                   wagon_num, nack.train_id, wagon_bytes.len());
                            retransmitted += 1;
                        }
                    } else {
                        warn!("⚠️  Wagon #{} not found in train #{} storage",
                              wagon_num, nack.train_id);
                    }
                }

                info!("✅ Retransmitted {} wagons for train #{}", retransmitted, nack.train_id);
            } else {
                warn!("⚠️  Train #{} not found in storage (may have expired)", nack.train_id);
            }
        }

        warn!("⚠️  NACK channel closed");
    }

    /// Handle socks5 command - start SOCKS5 proxy client
    async fn handle_socks5_command(
        parts: &[&str],
        transport: &P2PTransport,
        socks5_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::socks5::Socks5ProxyResponse)>>>>,
        socks5_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::socks5::Socks5TunnelData)>>>>,
    ) -> Result<(), String> {
        if parts.len() < 2 {
            println!("Использование: socks5 <SHORT_ID> [--username USER] [--password PASS]");
            println!();
            println!("ПРИМЕРЫ:");
            println!("   socks5 0021944bf8ffc764");
            println!("   socks5 0021944bf8ffc764 --username admin --password secret");
            println!();
            println!("SOCKS5 proxy запускается на 127.0.0.1:1080 (только этот компьютер)");
            println!("Авторизация: username yandi, пароль — свой у каждого узла (показывается при запуске)");
            return Ok(());
        }

        let short_id_hex = parts[1];
        // «auto [СТРАНА]» — выходы выбираются сами по каталогу карточек и чередуются
        // «hops [СТРАНА]» — то же, но соединения идут цепочкой через три узла (анонимно)
        let hops_mode = short_id_hex == "hops";
        let auto = short_id_hex == "auto" || hops_mode;
        let auto_country = if auto { parts.get(2).filter(|c| !c.starts_with("--")).and_then(|c| crate::network_offers::normalise_country(c)) } else { None };

        // Парсим аргументы
        let mut username = Some("yandi".to_string());
        // свой пароль узла (раньше — общий для всех «yandi123»)
        let mut password = Some(crate::exit_policy::local_proxy_password().map_err(|e| format!("пароль прокси: {e}"))?);

        let mut i = if auto && auto_country.is_some() { 3 } else { 2 };
        while i < parts.len() {
            match parts[i] {
                "--username" => {
                    if i + 1 < parts.len() {
                        username = Some(parts[i + 1].to_string());
                        i += 2;
                    } else {
                        println!("⚠️  --username требует значение");
                        i += 1;
                    }
                }
                "--password" => {
                    if i + 1 < parts.len() {
                        password = Some(parts[i + 1].to_string());
                        i += 2;
                    } else {
                        println!("⚠️  --password требует значение");
                        i += 1;
                    }
                }
                _ => {
                    println!("⚠️  Неизвестный аргумент: {}", parts[i]);
                    i += 1;
                }
            }
        }

        if auto {
            return Self::start_socks5_auto(transport, socks5_resp_rx, socks5_tunnel_rx, username, password, auto_country, hops_mode).await;
        }

        // Парсим short_id
        let short_id_bytes = hex::decode(short_id_hex)
            .map_err(|e| format!("Invalid short ID: {}", e))?;

        if short_id_bytes.len() != 8 {
            return Err("Short ID must be 8 bytes (16 hex chars)".to_string());
        }

        // Ищем пира по short_id
        let peers = transport.get_peers().await;
        let gateway_peer = peers.iter()
            .find(|p| &p.id.0[..8] == short_id_bytes.as_slice())
            .ok_or_else(|| format!("Peer with short ID {} not found", short_id_hex))?;

        println!("🧦 Запуск SOCKS5 Proxy через {} ({})...", short_id_hex, gateway_peer.addr);
        println!();

        use crate::socks5::{Socks5ProxyServer, Socks5Config};

        let config = Socks5Config {
            listen_addr: "127.0.0.1:1080".parse().unwrap(),  // только этот компьютер (раньше — все адреса с общим паролем)
            auth_required: true,  // ✅ Обязательная авторизация
            username,
            password,
            enable_udp: false,  // UDP не поддерживаем в P2P режиме
        };

        println!("🧦 SOCKS5 Proxy Configuration:");
        println!("   Listen Addr: {}", config.listen_addr);
        println!("   Auth Required: {}", config.auth_required);
        if let Some(ref user) = config.username {
            println!("   Username: {}", user);
        }
        if let Some(ref pw) = config.password {
            println!("   Password: {}", pw);
        }
        println!();

        let mut socks5_proxy = Socks5ProxyServer::new(config, std::sync::Arc::new(transport.clone()));

        // Take the response receiver
        let resp_rx = {
            let mut rx_lock = socks5_resp_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = resp_rx {
            socks5_proxy = socks5_proxy.with_response_channel(rx);
            println!("✅ SOCKS5 Response channel connected");
        } else {
            println!("⚠️  No SOCKS5 response channel available!");
        }

        // Take the tunnel data receiver
        let tunnel_rx = {
            let mut rx_lock = socks5_tunnel_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = tunnel_rx {
            socks5_proxy = socks5_proxy.with_tunnel_data_channel(rx);
            println!("✅ SOCKS5 Tunnel data channel connected");
        } else {
            println!("⚠️  No SOCKS5 tunnel data channel - CONNECT won't work!");
        }

        // Устанавливаем exit node
        socks5_proxy = socks5_proxy.with_exit_node(gateway_peer.id.clone());

        // Регистрируем Station
        transport.set_station(socks5_proxy.station.clone()).await;

        println!("🚂 YTP Station registered for SOCKS5");
        println!();

        // Запускаем прокси в фоне
        tokio::spawn(async move {
            if let Err(e) = socks5_proxy.run().await {
                eprintln!("❌ SOCKS5 Proxy Error: {}", e);
            }
        });

        // ОТПРАВЛЯЕМ КОМАНДУ НА GATEWAY NODE
        println!("📤 Отправка запроса на Gateway Node...");

        let cmd_bytes = vec![0x34u8]; // StartSocks5Gateway

        if let Err(e) = transport.send_encrypted(gateway_peer.id, &cmd_bytes).await {
            println!("❌ Не удалось отправить команду: {}", e);
            return Err(format!("Failed to send StartSocks5Gateway command: {}", e));
        }

        println!("✅ Запрос отправлен на ноду {}", short_id_hex);
        println!("⏳ Ожидание подтверждения от Gateway Node...");
        println!();
        println!("✅ SOCKS5 Proxy запущен на 0.0.0.0:1080!");
        println!("   Готов к приему SOCKS5 запросов.");
        println!();

        Ok(())
    }

    /// SOCKS5 на этом компьютере с автоматическим выбором выхода: узлы из каталога, которые выпускают в интернет и с которыми
    /// есть связь, чередуются; страна — по желанию (`socks5 auto NL`).
    async fn start_socks5_auto(
        transport: &P2PTransport,
        socks5_resp_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::socks5::Socks5ProxyResponse)>>>>,
        socks5_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::socks5::Socks5TunnelData)>>>>,
        username: Option<String>,
        password: Option<String>,
        country: Option<String>,
        hops_mode: bool,
    ) -> Result<(), String> {
        use crate::socks5::{Socks5ProxyServer, Socks5Config};
        let config = Socks5Config { listen_addr: "127.0.0.1:1080".parse().unwrap(), auth_required: true, username, password: password.clone(), enable_udp: false };
        println!("🧦 SOCKS5 на 127.0.0.1:1080, выходы выбираются сами{}", country.as_deref().map(|c| format!(" (страна {c})")).unwrap_or_default());
        if let Some(pw) = &password {
            println!("   Пароль: {pw}");
        }
        let pool = crate::exit_select::start_pool(Arc::new(transport.clone()), country);
        if let Some(pw) = &password {
            crate::exit_select::start_verifier(pool.clone(), config.listen_addr, pw.clone());
        }
        let mut proxy = Socks5ProxyServer::new(config, Arc::new(transport.clone())).with_exit_pool(pool);
        if hops_mode {
            proxy = proxy.with_hops();
            println!("   Анонимно: соединения идут цепочкой через три узла");
        } else {
            println!("   Быстро: соединения идут напрямую через один выход");
        }
        if let Some(rx) = socks5_resp_rx.lock().await.take() {
            proxy = proxy.with_response_channel(rx);
        }
        if let Some(rx) = socks5_tunnel_rx.lock().await.take() {
            proxy = proxy.with_tunnel_data_channel(rx);
        }
        transport.set_station(proxy.station.clone()).await;
        tokio::spawn(async move {
            if let Err(e) = proxy.run().await {
                eprintln!("❌ SOCKS5 Proxy Error: {}", e);
            }
        });
        println!("✅ Готов. Пока в каталоге нет подходящих выходов, соединения будут отклоняться.");
        Ok(())
    }

    /// Handle socks5-gateway command - start SOCKS5 exit node
    async fn handle_socks5_gateway_command(
        transport: &P2PTransport,
        socks5_req_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::socks5::Socks5ProxyRequest)>>>>,
        socks5_tunnel_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::socks5::Socks5TunnelData)>>>>,
        _nack_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<(HashId, crate::protocol::WagonNack)>>>>,
    ) -> Result<(), String> {
        println!("🧦 Запуск SOCKS5 Exit Node (Gateway)...");
        println!();
        println!("   Эта нода будет принимать SOCKS5 запросы через P2P,");
        println!("   делать реальные TCP соединения к целевым серверам");
        println!("   и возвращать данные обратно через P2P.");
        println!();
        println!("   Exit node будет делать соединения ОТ СВОЕГО ИМЕНИ.");
        println!();

        use crate::socks5::ExitNodeHandler;

        let mut exit_node = ExitNodeHandler::new(std::sync::Arc::new(transport.clone()));

        // Take the request receiver
        let req_rx = {
            let mut rx_lock = socks5_req_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = req_rx {
            exit_node = exit_node.with_request_channel(rx);
            println!("✅ SOCKS5 Request channel connected");
        } else {
            println!("⚠️  No SOCKS5 request channel available!");
        }

        // Take the tunnel data receiver
        let tunnel_rx = {
            let mut rx_lock = socks5_tunnel_rx.lock().await;
            rx_lock.take()
        };

        if let Some(rx) = tunnel_rx {
            exit_node = exit_node.with_tunnel_data_channel(rx);
            println!("✅ SOCKS5 Tunnel data channel connected");
        } else {
            println!("⚠️  No SOCKS5 tunnel data channel!");
        }

        // Регистрируем Station в transport
        transport.set_station(exit_node.station.clone()).await;
        println!("🚂 YTP Station registered in transport");

        println!("✅ SOCKS5 Exit Node запущен!");
        println!("   Ожидание SOCKS5 запросов через P2P...");

        tokio::spawn(async move {
            if let Err(e) = exit_node.run().await {
                eprintln!("❌ SOCKS5 Exit Node Error: {}", e);
            }
        });

        println!();

        Ok(())
    }

    /// Handle relay-server command - enable relay server mode
    async fn handle_relay_server_command(transport: &P2PTransport) -> Result<(), String> {
        println!("🌐 Включение режима Relay Server...");
        println!();
        println!("   Эта нода будет ретранслировать трафик между пирами за NAT");
        println!("   Публичный IP требуется для работы в качестве relay сервера");
        println!();

        // Получаем relay manager из транспорта
        let mut relay_manager = transport.relay_manager.lock().await;
        relay_manager.set_relay_server_mode(true);

        println!("✅ Relay Server режим ВКЛЮЧЕН!");
        println!("   Нода готова принимать relay-соединения");
        println!();

        Ok(())
    }

    /// Handle relay-connect command - connect to peer via relay
    async fn handle_relay_connect_command(transport: &P2PTransport, short_id_hex: &str) -> Result<(), String> {
        // Парсим short_id
        let short_id_bytes = hex::decode(short_id_hex)
            .map_err(|_| format!("Неверный формат Short ID: {}", short_id_hex))?;

        if short_id_bytes.len() != 8 {
            return Err(format!("Short ID должен быть 8 байт (16 hex), получено: {}", short_id_bytes.len()));
        }

        // Ищем пира по short_id
        let peers = transport.get_peers().await;
        let target_peer = peers.iter()
            .find(|p| &p.id.0[..8] == short_id_bytes.as_slice())
            .ok_or_else(|| format!("Пир с Short ID {} не найден", short_id_hex))?;

        println!("🔌 Подключение к пиру {} через Relay...", short_id_hex);
        println!("   Peer: {} ({})", target_peer.addr, short_id_hex);
        println!("   NAT статус: {}", target_peer.get_nat_status().as_str());

        // Проверяем, есть ли у нас relay manager
        let relay_manager = transport.relay_manager.lock().await;

        if !relay_manager.is_relay_server() {
            println!("⚠️  Предупреждение: Эта нода не является relay сервером");
            println!("   Для подключения нужна публичная нода с relay-server");
            println!();
            println!("   Сначала найдите публичную ноду и включите на ней relay-server");
            drop(relay_manager);
            return Ok(());
        }

        // Создаем relay сессию
        let my_id = transport.identity().node_id();
        let session_id = {
            let mut manager = transport.relay_manager.lock().await;
            manager.create_session(my_id, target_peer.id)
        };

        println!("🔗 Relay сессия #{} создана", session_id);

        // Отправляем запрос на подключение
        use crate::netlayer::packet::RelayConnectRequest;

        let request = RelayConnectRequest {
            source_peer: my_id,
            target_peer: target_peer.id,
            session_id,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        let request_bytes = serde_json::to_vec(&request)
            .map_err(|e| format!("Failed to serialize relay request: {}", e))?;

        let mut packet = vec![0x60u8]; // RelayConnectRequest
        packet.extend_from_slice(&request_bytes);

        transport.send_encrypted(target_peer.id, &packet).await?;

        println!("📤 Запрос на relay-соединение отправлен пиру {}", short_id_hex);
        println!("⏳ Ожидание ответа...");

        Ok(())
    }

    /// Handle relay-list command - show active relay sessions
    async fn handle_relay_list_command(transport: &P2PTransport) -> Result<(), String> {
        let relay_manager = transport.relay_manager.lock().await;

        if !relay_manager.is_relay_server() {
            println!("ℹ️  Relay server режим НЕ АКТИВЕН");
            println!("   Включите: relay-server");
        } else {
            println!("🌐 Relay Server режим АКТИВЕН");
        }

        let stats = relay_manager.get_stats();

        println!();
        println!("📊 Статистика Relay:");
        println!("   Активных сессий: {}", stats.active_sessions);
        println!("   Всего передано: {} байт", stats.total_bytes_forwarded);
        println!("   Всего пакетов: {}", stats.total_packets_forwarded);
        println!();

        let active_sessions = relay_manager.get_active_sessions();

        if active_sessions.is_empty() {
            println!("   (нет активных relay сессий)");
        } else {
            println!("📋 Активные relay сессии:");
            for (i, session) in active_sessions.iter().enumerate() {
                println!("   {}. Сессия #{}", i + 1, session.session_id);
                println!("      От: {}", hex::encode(&session.source_peer.0[..8]));
                println!("      Кому: {}", hex::encode(&session.target_peer.0[..8]));
                println!("      Статус: {:?}", session.status);
                println!("      Передано: {} байт", session.bytes_forwarded);
                println!("      Бездействие: {:?}", session.idle_time());
                println!();
            }
        }

        drop(relay_manager);
        Ok(())
    }
}