// src/main.rs
//! YANDI - Main Entry Point
//! ========================
//!
//! P2P network node

use yandi::{
    NodeIdentity, init_logging, LogLevel, NetworkMetrics,
    OSDetector, ExternalIpService, NetworkTopology, NodeIntrospection,
    SystemInfo,
    mask_hash_id, mask_ipv6, mask_ipv4, mask_public_key,
    P2PTransport, HelloEvent, P2PCli, BootstrapConfig,
    YandiTunManager, init_config, get_config
};


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use anyhow::anyhow;

    {
        let argv: Vec<String> = std::env::args().collect();
        // `yandi testnet up|status|down|reset` — тренировочная сеть из нескольких копий узла на этом компьютере
        if argv.get(1).map(|a| a.as_str()) == Some("testnet") {
            let code = yandi::testnet::run_cli(argv[2..].to_vec()).await;
            std::process::exit(code);
        }
        // `yandi bootstrap keygen|show|verify|add|from-card|remove|sign` — инструменты владельца списка входных узлов
        if argv.get(1).map(|a| a.as_str()) == Some("bootstrap") {
            let (code, text) = yandi::bootstrap::tool::run(argv[2..].to_vec());
            println!("{text}");
            std::process::exit(code);
        }
    }

    // Инициализируем конфигурацию
    init_config()
        .map_err(|e| anyhow::anyhow!("Failed to init config: {}", e))?;

    let config = get_config();

    // Initialize logging
    init_logging(LogLevel::Info)
        .map_err(|e| anyhow::anyhow!("Failed to init logging: {}", e))?;

    // Initialize metrics
    let metrics = NetworkMetrics::new();

    println!("🚀 YANDI v2 v0.2.0");
    println!("{}\n", "=".repeat(32));
    println!("📋 Configuration:");
    println!("   Discovery:      {}", config.ports.discovery);
    println!("   Data:           {}", config.ports.data);
    println!("   Mobile Gateway: {}", config.ports.mobile_gateway);
    println!("   Mobile P2P:     {}", config.ports.mobile_p2p);
    println!("   HTTP Proxy:     {}", config.ports.http_proxy);
    println!("   Web UI:         {}", config.ports.web_ui);
    println!();

    // 1. Detect OS
    let os_detector = OSDetector;
    os_detector.print_info();

    // 2. Detect network interfaces
    let topology = NetworkTopology::detect()
        .map_err(|e| anyhow!("Failed to detect network topology: {}", e))?;

    // 3. Detect external IP
    let ip_service = ExternalIpService::new();
    // страна по внешнему адресу — для карточки узла в сети (город и провайдер никуда не публикуются)
    let mut ip_country: Option<String> = None;
    let external_ip = match ip_service.get_external_ip().await {
        Ok(ip) => {
            match ip_service.get_detailed_ip_info().await {
                Ok(info) => {
                    // Info already printed in get_detailed_ip_info
                    ip_country = yandi::network_offers::normalise_country(&info.country);
                }
                Err(e) => {
                    println!("   ⚠️  Could not get detailed IP info: {}", e);
                }
            }
            Some(ip)
        }
        Err(e) => {
            println!("⚠️  Failed to detect external IP: {}", e);
            None
        }
    };

    // 3a. Detect external IPv6 (if available)
    let external_ipv6 = match ip_service.get_external_ipv6().await {
        Ok(ip) => {
            Some(ip)
        }
        Err(e) => {
            // IPv6 not available - this is normal
            None
        }
    };

    println!();

    // 4. Системный мониторинг ресурсов
    let mut sys_info = SystemInfo::gather();
    sys_info.display();

    // Измеряем задержку сети
    sys_info.measure_network_latency();

    println!();

    // 5. CLI override роли. --lite форсирует Mobile (lite-клиент через свой anchor),
    //    --anchor форсирует Anchor (домашний ПК с serve-ролями), без флага — авто.
    let args: Vec<String> = std::env::args().collect();
    let forced_role = if args.iter().any(|a| a == "--lite" || a == "--mobile") {
        Some(yandi::netlayer::node_introspection::NodeRole::Mobile)
    } else if args.iter().any(|a| a == "--anchor") {
        Some(yandi::netlayer::node_introspection::NodeRole::Anchor)
    } else {
        None
    };

    // 5b. Mobile / lite-клиент: --anchor-url wss://host:port/ + --anchor-fp <hex>
    //     При наличии — мобилка после старта подключается к anchor'у через WS-over-TLS
    //     и регистрирует его как peer'а в основном transport'е. Fingerprint обязателен —
    //     это TLS-pin, без него нельзя верифицировать self-signed cert anchor'а.
    let arg_value = |flag: &str| -> Option<String> {
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == flag {
                return it.next().cloned();
            }
            if let Some(rest) = a.strip_prefix(&format!("{}=", flag)) {
                return Some(rest.to_string());
            }
        }
        None
    };
    let anchor_url = arg_value("--anchor-url");
    let anchor_fp = arg_value("--anchor-fp");

    // 5b-h1. Hardening Step 1: --ws-bind <addr> переопределяет bind WS-сервера.
    //        Приоритет: CLI > yandi-config.yaml > default 0.0.0.0:8443.
    if let Some(ws_bind) = arg_value("--ws-bind") {
        println!("🌐 WS-bind override (CLI): {}", ws_bind);
        yandi::core::set_ws_bind_override(ws_bind);
    }

    // 5c. 🌍 Iter 3 / Hardening Step 6: self-claim jurisdiction (ISO-3166 alpha-2).
    //     `--jurisdiction XX` или alias `--my-jurisdiction XX`. Не валидируется.
    //     Вшивается в Hello-пакеты (TLV) — используется foreign-exit selection.
    let self_juris = arg_value("--jurisdiction").or_else(|| arg_value("--my-jurisdiction"));
    let claimed_country = self_juris.as_deref().and_then(yandi::network_offers::normalise_country);
    if let Some(j) = self_juris {
        let trimmed: String = j.chars().take(8).collect();
        println!("🌍 Jurisdiction self-claim: {}", trimmed);
        yandi::netlayer::packet::set_node_jurisdiction(trimmed);
    }

    // 5c-h6. 🆕 Hardening Step 6: `--exit-jurisdiction XX` — фильтр exit-кандидатов
    //        при построении circuit'а. Сохраняем глобально, потребитель — circuit builder.
    if let Some(j) = arg_value("--exit-jurisdiction") {
        let trimmed: String = j.chars().take(8).collect();
        println!("🌍 Exit-jurisdiction preference: {}", trimmed);
        yandi::netlayer::packet::set_exit_jurisdiction(trimmed);
    }

    // 5c-h6b. 🆕 Hardening Step 6: `--anchor-store <path>` — override paired_anchors.json.
    if let Some(p) = arg_value("--anchor-store") {
        println!("🗂  paired_anchors.json override: {}", p);
        yandi::netlayer::packet::set_anchor_store_override(p);
    }

    // 5c-h3. 🆕 Hardening Step 3: --import-pairing '<qr-string>' — mobile импортирует
    //        PairingPayload из QR-строки, добавляет в paired_anchors.json и выходит.
    //        Запускать ОТДЕЛЬНО от обычного yandi-старта; после import можно перезапускать
    //        без флага и mobile подключится автоматически через watchdog.
    if let Some(qr) = arg_value("--import-pairing") {
        match yandi::netlayer::pairing::PairingPayload::from_qr_string(&qr) {
            Ok(payload) => {
                let path = yandi::netlayer::pairing::default_paired_anchors_path();
                let mut store = yandi::netlayer::pairing::PairedAnchorStore::load_or_default(&path);
                store.add_or_update(payload.clone(), 0);
                match store.save(&path) {
                    Ok(_) => {
                        println!("✅ Pairing imported:");
                        println!("   anchor_id:       {}", hex::encode(&payload.anchor_id.0[..8]));
                        println!("   anchor_url:      {}", payload.anchor_url);
                        println!("   TLS fingerprint: {}", payload.fingerprint_hex);
                        println!("   store path:      {:?}", path);
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to persist paired_anchors.json: {}", e);
                        return Err(anyhow!("persist pairing: {}", e));
                    }
                }
            }
            Err(e) => {
                eprintln!("❌ Failed to parse QR-string: {}", e);
                return Err(anyhow!("parse pairing payload: {}", e));
            }
        }
    }

    // 5. Node self-introspection
    let introspection = NodeIntrospection::detect_with_override(
        &topology,
        external_ip.clone(),
        external_ipv6.clone(),
        &sys_info,
        forced_role,
    ).map_err(|e| anyhow!("Failed to perform node introspection: {}", e))?;

    // 6. Load auth state (before identity — master_key will be available for other components)
    let auth_state = yandi::load_auth_state();
    {
        use std::sync::atomic::Ordering;
        if !auth_state.is_setup.load(Ordering::Relaxed) {
            println!("🔐 Auth: первый запуск — откройте Web UI для первичной настройки");
        } else if auth_state.needs_rebind.load(Ordering::Relaxed) {
            println!("🔐 Auth: ключ не открыт на этом устройстве — нужно восстановление (yandi-keys recover), ничего не изменено");
        } else {
            println!("🔐 Auth: мастер-ключ загружен");
        }
    }

    // 7. Load or create identity (используем discovery порт из конфига)
    let config = get_config();
    let discovery_port = config.ports.discovery;

    // First run (no auth.json): the person creates the login password and the master password in the browser BEFORE the identity is
    // made, because the identity is stored under the keys that this creates. The setup server stops as soon as it has succeeded.
    #[cfg(unix)]
    if !auth_state.is_setup.load(std::sync::atomic::Ordering::Relaxed) {
        let port = config.ports.web_ui;
        println!("🔐 Первый запуск: откройте http://127.0.0.1:{port} и создайте пароль входа и мастер-пароль");
        if let Err(e) = yandi::web::first_setup::run(auth_state.clone(), port).await {
            eprintln!("🔐 Первичная настройка не запустилась: {e}");
            std::process::exit(2);
        }
        println!("🔐 Ключи созданы; продолжаем запуск ноды");
    }
    // A failure to open an EXISTING identity is an error and stops the node here; it never creates a new identity or touches the file.
    #[cfg(unix)]
    let identity = {
        let root = auth_state.get_master_key();
        match NodeIdentity::load_or_create_with_root(discovery_port, root.as_ref()) {
            Ok(identity) => identity,
            Err(e) => {
                eprintln!("🔐 Identity: {} — {}", e.category(), e);
                eprintln!("   Nothing was created or changed. See docs/KEY_RECOVERY.md (yandi-keys status / recover).");
                std::process::exit(2);
            }
        }
    };
    #[cfg(not(unix))]
    let identity = NodeIdentity::load_or_create(discovery_port);

    let identity_for_transport = identity.clone(); // Клон для транспортов
    let identity_for_web = identity.clone(); // Clone for Web UI/mDNS
    let identity_for_socks5 = identity.clone(); // Clone for SOCKS5 auto-start

    println!("📋 Node Information:");
    println!("   Node ID:        {}", mask_hash_id(&identity.node_id()));
    println!("   Port:            {}", discovery_port);
    println!("   CID:            {}", hex::encode(&identity.node_id().0[..8]));

    // Show cryptographic keys (masked)
    println!();
    println!("🔑 Cryptographic Keys:");
    println!("   X25519 Public:  {}", mask_public_key(&identity.public_key));
    println!("   Ed25519 Public: {}", mask_public_key(&identity.signing_public_key));
    println!("   Private Keys:   🔒 HIDDEN (never logged)");

    // TUN менеджер будет инициализирован позже через CLI команду "tun init"
    let tun_manager: Option<YandiTunManager> = None;

    // Show IPv6 virtual address (masked)
    println!();
    println!("🌐 Virtual Addresses:");
    let ipv6_virtual = identity.generate_ipv6_virtual();
    println!("   IPv6 Virtual:    {}", mask_ipv6(&ipv6_virtual));
    println!("   IPv6 Short:      {}", identity.get_ipv6_short());

    // Show best bind IP
    println!();
    println!("🌍 Network Configuration:");
    let best_ip = topology.get_best_bind_ip();
    let best_ip_str = best_ip.to_string();
    println!("   Bind IP:        {}", mask_ipv4(&best_ip_str));

    println!();
    println!("⏳ Starting P2P node...");
    println!();

    // 7. Capabilities включают роль: anchor/mobile/relay/introducer биты выставляются здесь.
    let capabilities = introspection.capabilities.capabilities_bits;
    println!("🎯 Node Role:        {}", introspection.role.name());
    println!("   Capabilities:    0b{:016b}", capabilities);
    println!("   Power level:      {}", sys_info.power);
    println!("   Load limit:       {}%", (sys_info.power.load_multiplier() * 100.0) as u32);
    println!("   Max connections:  {}", sys_info.power.max_connections());
    println!("   Max bandwidth:    {} Mbps", sys_info.power.max_bandwidth_mbps());
    println!();

    // 8. Create Exit Handler Manager channel
    use tokio::sync::mpsc;
    let (exit_tx, mut exit_rx) = mpsc::channel::<yandi::netlayer::transport::ExitHandlerRequest>(100);

    // 8.2 Create Proxy Gateway auto-start channel
    let (proxy_gateway_tx, mut proxy_gateway_rx) = mpsc::channel::<yandi::netlayer::transport::ProxyGatewayRequest>(100);

    // 8.25 Create SOCKS5 Gateway auto-start channel (аналог Proxy Gateway)
    let (socks5_gateway_tx, mut socks5_gateway_rx) = mpsc::channel::<yandi::netlayer::transport::ProxyGatewayRequest>(100);

    // 8.5 Create Proxy Gateway channel (for receiving requests on gateway node)
    let (proxy_tx, proxy_rx) = mpsc::channel::<(yandi::util::HashId, yandi::proxy::ProxyRequest)>(100);
    let proxy_rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(proxy_rx)));

    // 8.6 Create Proxy Client channel (for receiving responses on client node)
    let (proxy_resp_tx, proxy_resp_rx) = mpsc::channel::<(yandi::util::HashId, yandi::proxy::ProxyResponse)>(100);
    let proxy_resp_rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(proxy_resp_rx)));

    // 8.7 Create Proxy Tunnel Data channel (for CONNECT bi-directional tunneling)
    let (proxy_tunnel_tx, proxy_tunnel_rx) = mpsc::channel::<(yandi::util::HashId, yandi::proxy::ProxyTunnelData)>(100);
    let proxy_tunnel_rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(proxy_tunnel_rx)));

    // 8.75 Create NACK channel (для wagon retransmission на gateway)
    let (nack_tx, nack_rx) = mpsc::channel::<(yandi::util::HashId, yandi::protocol::WagonNack)>(100);
    let nack_rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(nack_rx)));

    // 8.8 Create SOCKS5 channels (аналогично HTTP Proxy)
    let (socks5_req_tx, socks5_req_rx) = mpsc::channel::<(yandi::util::HashId, yandi::socks5::Socks5ProxyRequest)>(100);
    let socks5_req_rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(socks5_req_rx)));

    let (socks5_resp_tx, socks5_resp_rx) = mpsc::channel::<(yandi::util::HashId, yandi::socks5::Socks5ProxyResponse)>(100);
    let socks5_resp_rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(socks5_resp_rx)));

    let (socks5_tunnel_tx, socks5_tunnel_rx) = mpsc::channel::<(yandi::util::HashId, yandi::socks5::Socks5TunnelData)>(100);
    let socks5_tunnel_rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(socks5_tunnel_rx)));

    // 8.9 Create TUN wagon channel (для TUN exit node)
    let (tun_wagon_tx, mut tun_wagon_rx) = mpsc::channel::<(yandi::util::HashId, yandi::netlayer::tun_exit::TunWagon)>(100);

    // 8.10 Create TUN wagon response channel (для TUN entry node - получения данных от exit node)
    let (tun_wagon_resp_tx, mut tun_wagon_resp_rx) = mpsc::channel::<(yandi::util::HashId, yandi::netlayer::tun_exit::TunWagonResponse)>(100);

    // 8.11 Create P2P Tunnel packet channel (для P2P тоннелей)
    let (p2p_tunnel_tx, mut p2p_tunnel_rx) = mpsc::channel::<(yandi::util::HashId, Vec<u8>)>(100);

    // 8.12 Create Chat packet channel (для чата)
    let (chat_packet_tx, mut chat_packet_rx) = mpsc::channel::<(yandi::util::HashId, yandi::communication::CommPacket)>(10000);
    let (group_packet_tx, mut group_packet_rx) = mpsc::channel::<(yandi::util::HashId, yandi::communication::GroupPacket)>(10000);
    let (media_signal_tx, mut media_signal_rx) = mpsc::channel::<(yandi::util::HashId, yandi::p2p::P2PPacketType, Vec<u8>)>(1024);

    // Клонируем каналы для второго транспорта
    let chat_packet_tx_for_p2p = chat_packet_tx.clone();
    let group_packet_tx_for_p2p = group_packet_tx.clone();
    let p2p_tunnel_tx_for_p2p = p2p_tunnel_tx.clone();

    // 9. Create P2P Transport with exit handler and proxy channels
    let transport = P2PTransport::with_handlers(
        identity_for_transport.clone(),
        capabilities,
        Some(exit_tx),
        Some(proxy_gateway_tx), // 🚀 Auto-start gateway
        Some(proxy_tx),
        Some(proxy_resp_tx),
        Some(proxy_tunnel_tx),
        Some(nack_tx), // 🔄 NACK channel
        // SOCKS5 handlers
        Some(socks5_gateway_tx), // 🧦 SOCKS5 Gateway auto-start
        Some(socks5_req_tx),
        Some(socks5_resp_tx),
        Some(socks5_tunnel_tx),
        // TUN wagon handler
        Some(tun_wagon_tx),
        Some(tun_wagon_resp_tx),
        // P2P tunnel handler
        Some(p2p_tunnel_tx),
        // Chat handler
        Some(chat_packet_tx),
        Some(group_packet_tx),
        // P2P communication transport is independent from netlayer
        None,
        None,  // external_ip
        None,  // topology
        None,  // relay_request_tx
        None,  // relay_response_tx
        None,  // relay_data_tx
        config.ports.discovery,
        config.ports.data,
    ).await
        .map_err(|e| anyhow!("Failed to create P2P transport: {}", e))?;

    println!("📡 Transport Info:");
    println!("   Fallback discovery: {}", transport.discovery_addr());
    println!("   Fallback data:      {}", transport.data_addr());
    println!("   Active discovery:   {}", transport.active_discovery_addr());
    println!("   Active data:        {}", transport.active_data_addr());
    println!();

    // 9.1 Mobile-сценарий: подключиться к anchor'у по wss://. Происходит после
    // создания transport'а, чтобы Hello-Ack от anchor'а сразу попал в peer-таблицу.
    // Если флагов нет — это обычный (Anchor / standalone) запуск, ничего не делаем.
    if let Some(url) = anchor_url.as_deref() {
        match anchor_fp.as_deref() {
            Some(fp) => {
                println!("📱 [mobile] Подключаюсь к anchor'у: {}", url);
                println!("            TLS pin (sha256): {}", fp);
                match transport.connect_to_anchor_ws(url, fp).await {
                    Ok(anchor_id) => {
                        println!("✅ [mobile] Привязан к anchor'у {} через WS-over-TLS",
                                 hex::encode(&anchor_id.0[..8]));
                    }
                    Err(e) => {
                        eprintln!("❌ [mobile] WS-link к {} провалился: {}", url, e);
                    }
                }
            }
            None => {
                eprintln!("⚠️  --anchor-url задан без --anchor-fp <sha256-hex>; pinning обязателен, пропускаю.");
            }
        }
    }

    // 9.5 Create P2P Comm Transport (port 9999, MTU 65536) - для чата, файлов, звонков
    let p2p_transport = yandi::p2p::P2PTransport::with_handlers(
        identity_for_transport,
        capabilities,
        Some(chat_packet_tx_for_p2p),
        Some(p2p_tunnel_tx_for_p2p),
        Some(media_signal_tx),
        // адрес, который узел называет соседям для переписки: заданный владельцем в настройках (network.public_ip) важнее найденного
        config.network.public_ip.clone().or_else(|| external_ip.clone()).unwrap_or_else(|| "0.0.0.0".to_string()),
    ).await
        .map_err(|e| anyhow!("Failed to create P2P Comm transport: {}", e))?;

    println!("📡 P2P Comm Transport Info:");
    println!("   Discovery: {}", p2p_transport.discovery_addr());
    println!("   Data:      {}", p2p_transport.data_addr());
    println!("   MTU:       65536 bytes (64 KB)");
    println!();

    // P2P Bootstrap: connect to peers on port 9001
    let bootstrap_path = std::path::PathBuf::from("nodes/bootstrap.json");
    // подписанный список входных узлов (скачанный или сохранённый) + локальный файл владельца
    let effective_bootstrap = yandi::bootstrap::effective_config(&bootstrap_path).await;
    {
        let config = effective_bootstrap.clone();
        // SEC-10: register pinned Ed25519 fingerprints before connecting
        let fps = config.fingerprint_map();
        if !fps.is_empty() {
            p2p_transport.set_bootstrap_fingerprints(fps);
        }

        let p2p_nodes: Vec<String> = config.get_enabled_nodes()
            .iter()
            .filter_map(|n| {
                let parts: Vec<&str> = n.address.split(":").collect();
                if parts.len() == 2 {
                    Some(format!("{}:9001", parts[0]))
                } else {
                    None
                }
            })
            .collect();

        for addr in p2p_nodes {
            println!("[P2P] 🔗 Connecting to {}:9001...", addr);
            if let Err(e) = p2p_transport.send_hello_request(&addr).await {
                println!("[P2P] ⚠️  Failed to connect to {}: {}", addr, e);
            } else {
                println!("[P2P] ✅ Hello sent to {}", addr);
            }
        }
    }

    // 8.5 Start mDNS announcer
    let short_id = hex::encode(&identity_for_web.node_id().0[..8]);
    let short_id_clone = short_id.clone();
    let full_node_id = identity_for_web.node_id();
    let role = format!("{:?}", introspection.role);

    println!("📡 Starting mDNS announcer...");
    let config = get_config();
    let web_ui_port = config.ports.web_ui;
    let data_port = config.ports.data;

    let mut mdns_service = yandi::MdnsService::new()
        .map_err(|e| anyhow!("Failed to create mDNS service: {}", e))?
        .with_announcer(
            short_id.clone(),
            web_ui_port,      // Admin/Web UI port
            discovery_port,   // Discovery port
            data_port,        // Data port
            role.clone(),
        )
        .map_err(|e| anyhow!("Failed to enable mDNS announcer: {}", e))?
        .with_browser()
        .map_err(|e| anyhow!("Failed to enable mDNS browser: {}", e))?;

    // копия из тренировочной сети не объявляет себя в домашней сети (имя yandi.local — у настоящего узла)
    if !yandi::testnet::active() {
        mdns_service.start().await
            .map_err(|e| anyhow!("Failed to start mDNS service: {}", e))?;
    }

    let mdns_hostname = format!("{}.local", short_id);
    println!("✅ mDNS: This node is now accessible as:");
    println!("   🌐 yandi.local    → Web UI (port {})", web_ui_port);
    println!("   🌐 {}  → Web UI (port {})", mdns_hostname, web_ui_port);
    println!();

    // 8.6 Start Web Server in background
    let external_ip_str = external_ip.clone().unwrap_or_else(|| "unknown".to_string());
    let virtual_ipv6 = identity_for_web.generate_ipv6_virtual();
    let ipv6_short = identity_for_web.get_ipv6_short();

    let node_info = yandi::NodeInfo {
        is_local: true,
        short_id: short_id_clone.clone(),
        cid: short_id_clone,
        node_id: hex::encode(&full_node_id.0), // Полный 64-символьный HashId
        role,
        external_ip: external_ip_str,
        virtual_ipv6: virtual_ipv6.to_string(),
        ipv6_short,
        discovery_port: discovery_port,
        data_port: data_port,
        web_port: web_ui_port,
    };

    let transport_for_web = transport.clone();
    let mdns_for_web = std::sync::Arc::new(mdns_service);
    let proxy_resp_rx_for_web = proxy_resp_rx.clone();
    let proxy_tunnel_rx_for_web = proxy_tunnel_rx.clone();
    let socks5_resp_rx_for_web = socks5_resp_rx.clone();
    let socks5_tunnel_rx_for_web = socks5_tunnel_rx.clone();

    // Создать ChatManager (использует мастер-ключ если доступен)
    let mut chat_manager = {
        let mk = auth_state.get_master_key();
        let result = if let Some(master_key) = mk {
            yandi::communication::ChatManager::new_with_master_key(
                identity_for_web.node_id(), p2p_transport.clone(), master_key,
            )
        } else {
            yandi::communication::ChatManager::new(identity_for_web.node_id(), p2p_transport.clone())
        };
        match result {
            Ok(cm) => cm,
            Err(e) => { eprintln!("❌ Failed to create ChatManager: {}", e); std::process::exit(1); }
        }
    };

    println!("💬 Chat Manager initialized");

    // Создать FileTransferManager (использует P2P Comm Transport для больших чанков!)
    let file_transfer_manager = std::sync::Arc::new(
        yandi::communication::FileTransferManager::new(
            identity_for_web.node_id(),
            p2p_transport.clone()
        )
    );

    println!("📤 File Transfer Manager initialized");

    // Create GroupManager
    let group_manager = std::sync::Arc::new(
        yandi::communication::groups::GroupManager::new()
    );

    println!("👥 Group Manager initialized");


    // Link ChatManager and FileTransferManager
    chat_manager.set_file_transfer_manager(file_transfer_manager.clone());

    let chat_manager = std::sync::Arc::new(chat_manager);
    chat_manager.clone().spawn_delivery_timeout_task();

    // Spawn Chat packet handler
    let chat_manager_clone = chat_manager.clone();
    tokio::spawn(async move {
        println!("💬 Chat packet handler started");

        while let Some((peer_id, comm_packet)) = chat_packet_rx.recv().await {
            if let Err(e) = chat_manager_clone.handle_comm_packet(peer_id, comm_packet).await {
                eprintln!("❌ [Chat] Error handling packet: {}", e);
            }
        }

        println!("💬 Chat packet handler stopped");
    });

    // Создать P2PTunnelManager - используем P2P Transport (порт 9998)
    let p2p_tunnel_manager = yandi::p2p_tunnel::P2PTunnelManager::new(
        identity_for_web.node_id(),
        p2p_transport.clone()  // ✅ P2P Transport на порту 9998
    );

    println!("🔗 P2P Tunnel Manager initialized");

    // Spawn P2P Tunnel packet handler
    let p2p_tunnel_manager_clone = p2p_tunnel_manager.clone();
    tokio::spawn(async move {
        println!("🔗 P2P Tunnel packet handler started");

        while let Some((peer_id, packet_bytes)) = p2p_tunnel_rx.recv().await {
            if let Err(e) = p2p_tunnel_manager_clone.handle_packet(peer_id, packet_bytes).await {
                eprintln!("❌ [P2P Tunnel] Error handling packet: {}", e);
            }
        }

        println!("🔗 P2P Tunnel packet handler stopped");
    });

    // Карточка узла для списков сети (телефоны — только клиенты, карточек не публикуют).
    {
        let host = config.network.public_ip.clone().or_else(|| external_ip.clone());
        let addresses: Vec<String> = host.into_iter().map(|h| if h.contains(':') { format!("[{h}]:{discovery_port}") } else { format!("{h}:{discovery_port}") }).collect();
        let p2p_port = yandi::p2p::transport::configured_ports().1;
        // визитка этого узла (открытые сведения для доверенных узлов) и стук к доверенным узлам, которых нет в сети
        {
            let p2p_addresses: Vec<String> = config.network.public_ip.clone().or_else(|| external_ip.clone()).into_iter().map(|h| if h.contains(':') { format!("[{h}]:{p2p_port}") } else { format!("{h}:{p2p_port}") }).collect();
            yandi::web::peers::install_my_card(&identity.node_id().0, &identity.signing_public_key, addresses.clone(), p2p_addresses);
            yandi::web::peers::install_p2p(p2p_transport.clone());
            yandi::web::peers::install_transport(transport.clone());
            yandi::web::peers::apply_saved().await;
            yandi::web::peers::spawn_reconnect_loop();
        }
        if introspection.role != yandi::netlayer::node_introspection::NodeRole::Mobile {
            let (country, source) = match (&claimed_country, &ip_country) {
                (Some(c), _) => (Some(c.clone()), "claimed"),
                (None, Some(c)) => (Some(c.clone()), "ip_lookup"),
                _ => (None, "unknown"),
            };
            let power = match sys_info.power { yandi::util::NodePower::Low => "low", yandi::util::NodePower::Medium => "medium", yandi::util::NodePower::High => "high" };
            let a = yandi::network_offers::SelfAssessment {
                country,
                country_source: source,
                public_ip: introspection.capabilities.has_public_ip,
                power,
                cpu_cores: sys_info.cpu.logical_cores as u32,
                ram_gb: (sys_info.memory.total_mb / 1024) as u32,
                latency_ms: sys_info.network_latency_ms.map(|l| l.round() as u32),
                addr: addresses.clone(),
            };
            let key = ed25519_dalek::SigningKey::from_bytes(&identity.signing_private_key);
            // свой внешний адрес задан владельцем — не следим; иначе при смене адреса перевыпускаем карточки и визитку
            let watch_ip = config.network.public_ip.is_none();
            let node_id_for_card = identity.node_id().0;
            let pubkey_for_card = identity.signing_public_key;
            let (disc_port, p2p_port_for_card) = (discovery_port, p2p_port);
            let on_change: std::sync::Arc<dyn Fn(String) + Send + Sync> = std::sync::Arc::new(move |host: String| {
                let fmt = |port: u16| if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
                yandi::web::peers::install_my_card(&node_id_for_card, &pubkey_for_card, vec![fmt(disc_port)], vec![fmt(p2p_port_for_card)]);
            });
            yandi::network_offers::start(transport.clone(), identity.node_id().0, key, a, watch_ip, Some(on_change));
            yandi::reciprocity::start();
        }
        // вход для телефона по TLS (если владелец включил): байты — в локальный SOCKS5-прокси узла
        if let Some(tls_port) = yandi::mobile_tls::configured_port() {
            let socks_port = config.ports.mobile_gateway;
            let node_hex = hex::encode(identity.node_id().0);
            tokio::spawn(async move {
                if let Err(e) = yandi::mobile_tls::serve(tls_port, socks_port, node_hex).await {
                    eprintln!("[mobile-tls] {e}");
                }
            });
        }
        yandi::relay_net::start(transport.clone(), identity.node_id().0, ed25519_dalek::SigningKey::from_bytes(&identity.signing_private_key), std::env::var("YANDI_CLIENT_ONLY").map(|v| v == "1").unwrap_or(false));
        yandi::hops_net::start(transport.clone(), identity.node_id().0, ed25519_dalek::SigningKey::from_bytes(&identity.signing_private_key));
    }

    // Запустить CLI
    let p2p_tunnel_manager_for_cli = p2p_tunnel_manager.clone();
    let proxy_rx_for_cli = proxy_rx.clone();
    let proxy_tunnel_rx_for_cli = proxy_tunnel_rx.clone();
    let nack_rx_for_cli = nack_rx.clone();
    let socks5_req_rx_for_cli = socks5_req_rx.clone();
    let socks5_tunnel_rx_for_cli = socks5_tunnel_rx.clone();
    let cli = P2PCli::new(transport.clone())
        .with_proxy_response_channel(proxy_resp_rx)
        .with_proxy_request_channel(proxy_rx_for_cli)
        .with_proxy_tunnel_data_channel(proxy_tunnel_rx_for_cli)
        .with_nack_channel(nack_rx_for_cli) // 🔄 NACK channel
        .with_socks5_response_channel(socks5_resp_rx)
        .with_socks5_request_channel(socks5_req_rx_for_cli)
        .with_socks5_tunnel_data_channel(socks5_tunnel_rx_for_cli)
        .with_tun_wagon_channel(tun_wagon_rx)
        .with_tun_wagon_response_channel(tun_wagon_resp_rx)
        .with_p2p_tunnel_manager(p2p_tunnel_manager_for_cli);
    cli.spawn();

    // Keep the application running
    let config = get_config();
    let web_ui_port = config.ports.web_ui;
    let http_proxy_port = config.ports.http_proxy;
    let (media_signal_bus, _) = tokio::sync::broadcast::channel::<yandi::web::media_api::MediaSignalEvent>(256);

    // Shared incoming calls queues (written by signal loop, read by web server)
    let incoming_calls: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, yandi::web::server::IncomingCallInfo>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let incoming_calls_for_signal = incoming_calls.clone();
    let incoming_video_calls: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, yandi::web::server::IncomingCallInfo>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let incoming_video_calls_for_signal = incoming_video_calls.clone();

    let media_signal_bus_forward = media_signal_bus.clone();
    tokio::spawn(async move {
        while let Some((peer_id, packet_type, payload)) = media_signal_rx.recv().await {
            if let Ok(text) = String::from_utf8(payload.clone()) {
                // Handle VoiceCallRequest: store as incoming voice call
                if packet_type == yandi::p2p::P2PPacketType::VoiceCallRequest {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                    if let Some(info) = yandi::web::media_api::incoming_from_packet(&peer_id, &text, now) {
                        yandi::web::media_api::queue_incoming(&mut *incoming_calls_for_signal.lock().await, info, now);
                        println!("📞 Incoming call stored in queue (VoiceCallRequest)");
                    }
                }

                // Handle VideoCallRequest: store as incoming video call
                if packet_type == yandi::p2p::P2PPacketType::VideoCallRequest {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                    if let Some(info) = yandi::web::media_api::incoming_from_packet(&peer_id, &text, now) {
                        yandi::web::media_api::queue_incoming(&mut *incoming_video_calls_for_signal.lock().await, info, now);
                        println!("📞 Incoming call stored in queue (VideoCallRequest)");
                    }
                }

                // Always forward to signal bus for WebSocket delivery
                let _ = media_signal_bus_forward.send(yandi::web::media_api::MediaSignalEvent {
                    from_peer_id: hex::encode(&peer_id.0[..8]),
                    payload: text,
                    packet_type: format!("{:?}", packet_type),
                });
            }
        }
    });

    tokio::spawn(async move {
        let media_manager = yandi::media::session::MediaSessionManager::new();
        let web_server = yandi::WebServer::with_transport(web_ui_port, transport_for_web)
            .with_auth_state(auth_state)
            .with_node_info(node_info)
            .with_proxy_channels(proxy_resp_rx_for_web, proxy_tunnel_rx_for_web)
            .with_socks5_channels(socks5_resp_rx_for_web, socks5_tunnel_rx_for_web)
            .with_chat_manager(chat_manager)
            .with_file_transfer_manager(file_transfer_manager)
            .with_p2p_transport(p2p_transport)
            .with_p2p_tunnel_manager(p2p_tunnel_manager)
            .with_media_manager(media_manager)
            .with_media_signal_bus(media_signal_bus)
            .with_incoming_calls(incoming_calls)
            .with_incoming_video_calls(incoming_video_calls)
            .with_group_manager(group_manager.clone())
            .with_mdns(mdns_for_web).await;

        if let Err(e) = web_server.run().await {
            eprintln!("❌ Web server error: {}", e);
        }
    });

    println!("🌐 Web UI running on http://127.0.0.1:{}", web_ui_port);
    println!("   Proxy will run on port {} (use CLI: proxy <SHORT_ID>)", http_proxy_port);
    println!();

    // Subscribe to Hello events
    let mut hello_rx = transport.subscribe_hello();

    // Spawn Exit Handler Manager task
    let transport_for_exit = transport.clone();
    tokio::spawn(async move {
        println!("🚪 Exit Handler Manager started");

        while let Some(request) = exit_rx.recv().await {
            println!();
            println!("╔════════════════════════════════════════════════════════════╗");
            println!("║  📨 ЗАПРОС: Запрос на открытие интернета                     ║");
            println!("║     от ноды: {}                              ║",
                     mask_hash_id(&request.peer_id));
            println!("╚════════════════════════════════════════════════════════════╝");
            println!();

            // Запускаем ExitNodeHandler на ЭТОЙ ноде (НЛ)
            // NOTE: Теперь Exit Node Handler запускается через CLI команду "exit"
            // Каналы должны быть созданы при запуске ноды

            println!("⚠️  Для запуска Exit Node Handler используйте CLI команду: exit");
            println!();
            println!("╔════════════════════════════════════════════════════════════╗");
            println!("║  ✅ ОТВЕТ: Интернет доступ открыт!                            ║");
            println!("╚════════════════════════════════════════════════════════════╝");
            println!();

            // Отправляем ответ обратно (ExitHandlerStarted = 0x21)
            let resp_bytes = vec![0x21u8];
            if let Err(e) = transport_for_exit.send_encrypted(request.peer_id, &resp_bytes).await {
                eprintln!("❌ Не удалось отправить ответ: {}", e);
            }
        }
    });

    // 🚀 Spawn Proxy Gateway Auto-Start Manager task (ТОЛЬКО на Gateway нодах!)
    if introspection.role == yandi::netlayer::node_introspection::NodeRole::Anchor {
        let transport_for_proxy_gateway = transport.clone();
        let proxy_rx_clone = proxy_rx.clone();
        let proxy_tunnel_rx_clone = proxy_tunnel_rx.clone();
        let nack_rx_clone = nack_rx.clone();

        tokio::spawn(async move {
            println!("🌐 Proxy Gateway Auto-Start Manager started (Gateway mode)");

            // Берём receivers для proxy gateway (ОДИН РАЗ!)
            let req_rx = {
                let mut rx_lock = proxy_rx_clone.lock().await;
                rx_lock.take()
            };

            let tunnel_rx = {
                let mut rx_lock = proxy_tunnel_rx_clone.lock().await;
                rx_lock.take()
            };

            let nack_rx_chan = {
                let mut rx_lock = nack_rx_clone.lock().await;
                rx_lock.take()
            };

            // Создаём HttpProxyGateway ОДИН РАЗ (он работает для всех нод)
            if req_rx.is_none() {
                println!("⚠️  No proxy request channel! Gateway auto-start disabled.");
                return;
            }

            use yandi::proxy::HttpProxyGateway;

            let mut gateway = HttpProxyGateway::new(transport_for_proxy_gateway.clone());
            gateway = gateway.with_request_channel(req_rx.unwrap());

            if let Some(tunnel_rx) = tunnel_rx {
                gateway = gateway.with_tunnel_data_channel(tunnel_rx);
            }

            // TODO: NACK handler - нужен публичный метод в HttpProxyGateway
            // if let Some(nack_rx) = nack_rx_chan {
            //     // Запускаем NACK handler
            // }

            // Регистрируем Station
            transport_for_proxy_gateway.set_station(gateway.station.clone()).await;

            println!("🌐 HttpProxyGateway initialized and ready for auto-start");

            // Флаг: запущен ли уже gateway
            let gateway_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            // Теперь слушаем запросы от нод
            while let Some(request) = proxy_gateway_rx.recv().await {
                println!();
                println!("╔════════════════════════════════════════════════════════════╗");
                println!("║  🌐 ЗАПРОС: Автозапуск HTTP Proxy Gateway                   ║");
                println!("║     от ноды: {}                              ║",
                         mask_hash_id(&request.peer_id));
                println!("║     Short ID: {}                                ║",
                         request.short_id);
                println!("╚════════════════════════════════════════════════════════════╝");
                println!();

                // Gateway уже запущен! Просто логируем
                println!("[proxy-gateway-auto] ✅ HTTP Proxy Gateway активен для ноды {}!",
                         request.short_id);
                println!();
                println!("╔════════════════════════════════════════════════════════════╗");
                println!("║  ✅ GATEWAY ACTIVATED! Интернет доступен для ноды {}      ║", request.short_id);
                println!("╚════════════════════════════════════════════════════════════╝");
                println!();

                // Запускаем gateway.run() только ОДИН РАЗ (после первого запроса)
                if !gateway_started.load(std::sync::atomic::Ordering::Relaxed) {
                    gateway_started.store(true, std::sync::atomic::Ordering::Relaxed);

                    // Клонируем gateway через clone_for_handler()
                    let gateway_run = gateway.clone_for_handler();

                    tokio::spawn(async move {
                        if let Err(e) = gateway_run.run().await {
                            eprintln!("[proxy-gateway-auto] ❌ Gateway error: {}", e);
                        }
                    });
                }
            }
        });
    } else {
        println!("ℹ️  Proxy Gateway Auto-Start disabled (non-Gateway node)");
        // На клиентских нодах receivers остаются доступными для CLI команды "proxy"
    }

    // 🧦 Spawn SOCKS5 Gateway Auto-Start Manager task (ТОЛЬКО на Gateway нодах!)
    if introspection.role == yandi::netlayer::node_introspection::NodeRole::Anchor {
        let transport_for_socks5_gateway = transport.clone();
        let socks5_req_rx_clone = socks5_req_rx.clone();
        let socks5_tunnel_rx_clone = socks5_tunnel_rx.clone();
        let nack_rx_clone = nack_rx.clone();

        tokio::spawn(async move {
            println!("🧦 SOCKS5 Gateway Auto-Start Manager started (Gateway mode)");

            // Берём receivers для SOCKS5 gateway (ОДИН РАЗ!)
            let req_rx = {
                let mut rx_lock = socks5_req_rx_clone.lock().await;
                rx_lock.take()
            };

            let tunnel_rx = {
                let mut rx_lock = socks5_tunnel_rx_clone.lock().await;
                rx_lock.take()
            };

            // Создаём SOCKS5 Exit Node Handler ОДИН РАЗ
            if req_rx.is_none() {
                println!("⚠️  No SOCKS5 request channel! Gateway auto-start disabled.");
                return;
            }

            use yandi::socks5::ExitNodeHandler;

            let mut socks5_gateway = ExitNodeHandler::new(transport_for_socks5_gateway.clone());
            socks5_gateway = socks5_gateway.with_request_channel(req_rx.unwrap());

            if let Some(tunnel_rx) = tunnel_rx {
                socks5_gateway = socks5_gateway.with_tunnel_data_channel(tunnel_rx);
            }

            // Регистрируем Station
            transport_for_socks5_gateway.set_station(socks5_gateway.station.clone()).await;

            println!("🧦 SOCKS5 Exit Node Handler initialized and ready for auto-start");

            // Флаг: запущен ли уже gateway
            let gateway_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            // Теперь слушаем запросы от нод
            while let Some(request) = socks5_gateway_rx.recv().await {
                println!();
                println!("╔════════════════════════════════════════════════════════════╗");
                println!("║  🧦 ЗАПРОС: Автозапуск SOCKS5 Proxy Gateway                  ║");
                println!("║     от ноды: {}                              ║",
                         mask_hash_id(&request.peer_id));
                println!("║     Short ID: {}                                ║",
                         request.short_id);
                println!("╚════════════════════════════════════════════════════════════╝");
                println!();

                // Gateway уже запущен! Просто логируем
                println!("[socks5-gateway-auto] ✅ SOCKS5 Proxy Gateway активен для ноды {}!",
                         request.short_id);
                println!();
                println!("╔════════════════════════════════════════════════════════════╗");
                println!("║  ✅ SOCKS5 GATEWAY ACTIVATED! Интернет доступен для ноды {} ║", request.short_id);
                println!("╚════════════════════════════════════════════════════════════╝");
                println!();

                // Запускаем gateway.run() только ОДИН РАЗ (после первого запроса)
                if !gateway_started.load(std::sync::atomic::Ordering::Relaxed) {
                    gateway_started.store(true, std::sync::atomic::Ordering::Relaxed);

                    // Клонируем gateway через clone_for_handler()
                    let socks5_gateway_run = socks5_gateway.clone_for_handler();

                    tokio::spawn(async move {
                        if let Err(e) = socks5_gateway_run.run().await {
                            eprintln!("[socks5-gateway-auto] ❌ SOCKS5 Gateway error: {}", e);
                        }
                    });
                }
            }
        });
    } else {
        println!("ℹ️  SOCKS5 Gateway Auto-Start disabled (non-Gateway node)");
    }

    // Spawn Hello event handler
    let transport_clone = transport.clone();
    tokio::spawn(async move {
        println!("📨 Hello event listener started");

        while let Ok(event) = hello_rx.recv().await {
            match event {
                HelloEvent::Request { from, packet } => {
                    println!("📨 Received HELLO_REQ from {}", mask_ipv4(&from.to_string()));
                    println!("   Node ID: {}", mask_hash_id(&packet.node_id));
                    println!("   CID:     {}", hex::encode(&packet.cid));

                    // Ответ: сеанс поднимается внутри (одноразовый ключ ответчика), затем уходит подтверждение
                    if let Err(e) = transport_clone.send_hello_ack(&from.to_string(), packet.nonce, packet.node_id, packet.x25519_public).await {
                        if e.contains("crossing handshake") {
                            println!("   🤝 Встречное знакомство: остаётся рукопожатие узла с меньшим номером");
                        } else {
                            println!("   ❌ Failed to send ACK: {}", e);
                        }
                    } else {
                        println!("   ✅ Sent HELLO_ACK (session established)");

                        // 🔥 ACTIVATE REVERSE CHANNEL: Send probe message to open port 10000
                        println!("   🔄 Activating reverse channel...");

                        let probe_msg = format!("PROBE:{}", hex::encode(&packet.node_id.0[..8]));
                        if let Err(e) = transport_clone.send_encrypted(packet.node_id, probe_msg.as_bytes()).await {
                            println!("   ⚠️  Failed to send probe: {}", e);
                        } else {
                            println!("   ✅ Probe sent to activate reverse channel");
                        }
                    }
                }
                HelloEvent::Ack { from, packet } => {
                    println!("📨 Received HELLO_ACK from {}", mask_ipv4(&from.to_string()));
                    println!("   Node ID: {}", mask_hash_id(&packet.node_id));

                    // Establish session (номер приглашения = nonce подтверждения)
                    match transport_clone.establish_session(packet.node_id, packet.nonce, packet.x25519_public).await {
                        Ok(v) => println!("   🔒 Session v{} established", v),
                        Err(e) => {
                            if e.contains("duplicate") {
                                println!("   ⚠️  Duplicate handshake, ignoring");
                            } else {
                                println!("   ❌ Session error: {}", e);
                            }
                        }
                    }

                    // 🔥 ACTIVATE REVERSE CHANNEL: Send probe message to open port 10000
                    println!("   🔄 Activating reverse channel...");

                    let probe_msg = format!("PROBE:{}", hex::encode(&packet.node_id.0[..8]));
                    if let Err(e) = transport_clone.send_encrypted(packet.node_id, probe_msg.as_bytes()).await {
                        println!("   ⚠️  Failed to send probe: {}", e);
                    } else {
                        println!("   ✅ Probe sent to activate reverse channel");
                    }

                    // 🔥🔥 ПРОВЕРЯЕМ: ЕСЛИ ЭТО EXIT NODE - ЗАПУСКАЕМ EXIT HANDLER + SOCKS5
                    // Проверяем bootstrap config - это exit node?
                    let bootstrap_path = std::path::PathBuf::from("nodes/bootstrap.json");
                    if let Ok(config) = BootstrapConfig::load_from_file(&bootstrap_path) {
                        let peer_cid = hex::encode(&packet.node_id.0[..8]);

                        // Auto-start DISABLED - exit node handler now starts on demand via control command
                        // // Ищем ноду в bootstrap по IP
                        // for node in &config.nodes {
                        //     if node.address.contains(&from.to_string()) {
                        //         if node.role.as_ref().map(|r| r.as_str()) == Some("exit") {
                        //             println!("🧦 [auto-socks5] Exit node connected! Starting SOCKS5 Proxy...");
                        //
                        //             let transport_for_socks5 = transport_clone.clone();
                        //             tokio::spawn(async move {
                        //                 use yandi::Socks5ProxyServer;
                        //                 use yandi::Socks5Config;
                        //
                        //                 let socks5_config = Socks5Config::default();
                        //                 let socks5_server = Socks5ProxyServer::new(
                        //                     socks5_config,
                        //                     std::sync::Arc::new(transport_for_socks5)
                        //                 );
                        //
                        //                 if let Err(e) = socks5_server.with_exit_node(packet.node_id).run().await {
                        //                     eprintln!("❌ [auto-socks5] SOCKS5 failed: {}", e);
                        //                 }
                        //             });
                        //             break;
                        //         }
                        //     }
                        // }
                    }
                }
            }
        }
    });

    println!();
    println!("✅ YANDI v2 is ready!");
    println!();

    // ===== BOOTSTRAP: Автоматическое подключение к нодам =====
    let bootstrap_path = std::path::PathBuf::from("nodes/bootstrap.json");

    // Подписанный список входных узлов + локальный файл владельца (загружены выше)
    let bootstrap_config: Result<BootstrapConfig, String> = Ok(effective_bootstrap.clone());
    // раз в 6 часов список обновляется: новые входные узлы подключаются без перезапуска
    {
        let transport_for_bootstrap = transport.clone();
        let external_ip_for_bootstrap = external_ip.clone();
        let bootstrap_path_for_refresh = bootstrap_path.clone();
        tokio::spawn(async move {
            let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                let cfg = yandi::bootstrap::effective_config(&bootstrap_path_for_refresh).await;
                let fresh: Vec<String> = cfg.get_enabled_nodes().iter().map(|n| n.address.clone()).filter(|a| known.insert(a.clone())).collect();
                if !fresh.is_empty() {
                    let _ = transport_for_bootstrap.bootstrap(fresh, external_ip_for_bootstrap.clone()).await;
                }
            }
        });
    }

    match bootstrap_config {
        Ok(config) => {
            if config.connect_on_startup {
                println!("🚀 Bootstrap: auto-connecting to nodes...");
                println!();

                let bootstrap_nodes: Vec<String> = config.get_enabled_nodes()
                    .iter()
                    .map(|n| n.address.clone())
                    .collect();

                if !bootstrap_nodes.is_empty() {
                    // Запускаем bootstrap с фильтрацией своего IP
                    if let Err(e) = transport.bootstrap(bootstrap_nodes, external_ip).await {
                        println!("⚠️  Bootstrap failed: {}", e);
                    }

                    println!();
                    println!("⏳ Waiting for peer responses...");
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                } else {
                    println!("ℹ️  No bootstrap nodes configured");
                    println!("   Edit {:?} to add nodes", bootstrap_path);
                }
            }
        }
        Err(e) => {
            println!("⚠️  Failed to load bootstrap config: {}", e);
            println!("   Will wait for incoming connections...");
        }
    }

    println!();
    println!("💡 SOCKS5 Proxy через P2P:");
    println!("   На exit node: exit");
    println!("   На entry node: socks5 <SHORT_ID>");
    println!("   Пример: socks5 0021944bf8ffc764");
    println!();
    println!("💡 Доступные команды:");
    println!();
    println!("   proxy <SHORT_ID>  - запустить HTTP Proxy через пира");
    println!("   proxy-gateway      - запустить HTTP Proxy Gateway");
    // 🚂 Запускаем периодическую печать wagon statistics (каждые 30 секунд)
    // 🔇 Спам отключен
    /*
    use yandi::netlayer::transport::get_wagon_stats;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            get_wagon_stats().print_stats();
        }
    });
    */

    // 🧹 Periodic session cleanup (every 60 seconds)
    let transport_cleanup = transport.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        // transport_cleanup.cleanup_sessions().await; // DISABLED: Keep sessions alive
        }
    });

    // Keep the application running
    // Ctrl+C в терминале или обычная команда остановки (SIGTERM: systemd, `yandi testnet down`) — одинаково аккуратная остановка
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r?,
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    println!("\n👋 Shutting down...");

    // Очистка TUN устройств
    if let Some(ref manager) = tun_manager {
        println!("🧹 Очистка TUN устройств...");
        if let Err(e) = manager.cleanup_all() {
            eprintln!("   ⚠️  Ошибка очистки: {}", e);
        } else {
            println!("   ✅ TUN устройства остановлены");
        }
    }

    Ok(())
}
