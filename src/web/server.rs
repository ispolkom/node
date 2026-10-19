// src/web/server.rs
//!
//! # YANDI Web Server
//!
//! Локальный HTTP сервер для управления нодой

use axum::{
    body::Body,
    extract::{Multipart, Path as PathExtractor, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::{delete, get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use std::future::IntoFuture;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info};

use crate::core::profile::UserProfile;
use crate::netlayer::relay::{RelayManager, RelaySession, RelaySessionStatus};
use crate::web::media_api;
use crate::{DiscoveredNode, MdnsService};

/// Информация о ноде для веб-интерфейса
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Является ли эта нода локальной (относительно пользователя)
    pub is_local: bool,
    /// Short ID (первые 8 байт Node ID)
    pub short_id: String,
    /// CID (Connection ID) - тоже Short ID
    pub cid: String,
    /// Полный Node ID (маскированный)
    pub node_id: String,
    /// Роль ноды
    pub role: String,
    /// Внешний IP адрес (маскированный)
    pub external_ip: String,
    /// Виртуальный IPv6 адрес (маскированный)
    pub virtual_ipv6: String,
    /// IPv6 Short (упрощённый)
    pub ipv6_short: String,
    /// P2P порт
    pub discovery_port: u16,
    /// Data порт
    pub data_port: u16,
    /// Web порт
    pub web_port: u16,
}

impl Default for NodeInfo {
    fn default() -> Self {
        Self {
            is_local: false,
            short_id: "unknown".to_string(),
            cid: "unknown".to_string(),
            node_id: "unknown".to_string(),
            role: "Unknown".to_string(),
            external_ip: "unknown".to_string(),
            virtual_ipv6: "unknown".to_string(),
            ipv6_short: "unknown".to_string(),
            discovery_port: 9000,
            data_port: 10000,
            web_port: 8080,
        }
    }
}

impl NodeInfo {
    /// Значение по умолчанию для standalone режима
    pub fn default_standalone() -> Self {
        Self {
            is_local: true,
            short_id: "standalone".to_string(),
            cid: "standalone".to_string(),
            node_id: "standalone".to_string(),
            role: "Web Only".to_string(),
            external_ip: "127.0.0.1".to_string(),
            virtual_ipv6: "fc00::".to_string(),
            ipv6_short: "::".to_string(),
            discovery_port: 9000,
            data_port: 10000,
            web_port: 8080,
        }
    }
}

/// Входящий голосовой вызов (ожидает принятия/отклонения)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IncomingCallInfo {
    pub call_id: String,
    pub from_short_id: String,
    pub from_display_name: String,
    pub received_at: u64,
}

/// Состояние веб-сервера
#[derive(Clone)]
pub struct AppState {
    /// mDNS сервис для обнаружения нод
    pub mdns: Option<Arc<crate::MdnsService>>,
    /// P2P Transport (опционально, для интеграции с основной нодой) - netlayer transport
    pub transport: Option<Arc<crate::P2PTransport>>,
    /// P2P Communication Transport (опционально) - новый transport для Chat, Files (port 9998)
    pub p2p_transport: Option<Arc<crate::p2p::P2PTransport>>,
    /// Список активных proxy подключений
    pub active_proxies: Arc<Mutex<std::collections::HashMap<String, ProxyInfo>>>,
    /// Задачи работающих прокси (слушатель и перекачка каналов): остановка прекращает их, а не только стирает запись
    pub proxy_tasks: Arc<Mutex<std::collections::HashMap<String, Vec<tokio::task::AbortHandle>>>>,
    /// Кэш обнаруженных нод (обновляется в фоне)
    pub discovered_nodes: Arc<Mutex<Vec<DiscoveredNode>>>,
    /// Информация о текущей ноде
    pub node_info: NodeInfo,
    /// Статус работы P2P ноды (true = online, false = offline)
    pub node_running: Arc<std::sync::atomic::AtomicBool>,
    /// Proxy response channel (for HTTP Proxy Client)
    pub proxy_resp_rx: Arc<
        Mutex<
            Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::proxy::ProxyResponse)>>,
        >,
    >,
    /// Proxy tunnel data channel (for CONNECT bi-directional tunneling)
    pub proxy_tunnel_rx: Arc<
        Mutex<
            Option<
                tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::proxy::ProxyTunnelData)>,
            >,
        >,
    >,
    /// Активный HTTP Proxy Client (хранится чтобы остановить)
    pub active_proxy_client: Arc<Mutex<Option<crate::proxy::HttpProxyClient>>>,
    /// Активный SOCKS5 Proxy Server (хранится чтобы остановить)
    pub active_socks5_proxy: Arc<Mutex<Option<crate::socks5::Socks5ProxyServer>>>,
    /// SOCKS5 proxy response channel
    pub socks5_resp_rx: Arc<
        Mutex<
            Option<
                tokio::sync::mpsc::Receiver<(
                    crate::util::HashId,
                    crate::socks5::Socks5ProxyResponse,
                )>,
            >,
        >,
    >,
    /// SOCKS5 tunnel data channel
    pub socks5_tunnel_rx: Arc<
        Mutex<
            Option<
                tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::socks5::Socks5TunnelData)>,
            >,
        >,
    >,
    /// Chat Manager для P2P чата
    pub chat_manager: Arc<Mutex<Option<std::sync::Arc<crate::communication::ChatManager>>>>,
    /// File Transfer Manager для чанкованной передачи файлов
    pub file_transfer_manager:
        Arc<Mutex<Option<std::sync::Arc<crate::communication::FileTransferManager>>>>,
    /// P2P Tunnel Manager для чистых P2P тоннелей
    pub p2p_tunnel_manager: Arc<Mutex<Option<crate::p2p_tunnel::P2PTunnelManager>>>,
    /// Media session manager for voice/video calls
    pub media_manager: Arc<tokio::sync::Mutex<Option<crate::media::session::MediaSessionManager>>>,
    /// Media signaling bus for forwarding P2P signaling into WebSocket sessions
    pub media_signal_bus: tokio::sync::broadcast::Sender<crate::web::media_api::MediaSignalEvent>,
    /// Очередь входящих голосовых вызовов (call_id → info)
    pub incoming_calls: Arc<Mutex<std::collections::HashMap<String, IncomingCallInfo>>>,
    /// Очередь входящих видеозвонков (call_id → info)
    pub incoming_video_calls: Arc<Mutex<std::collections::HashMap<String, IncomingCallInfo>>>,
    /// Auth state (session store + master key + setup flags)
    pub auth_state: crate::web::auth::AuthState,
}

/// Информация об активном proxy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyInfo {
    pub short_id: String,
    pub proxy_type: ProxyType,
    pub local_port: u16,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProxyType {
    Http,
    Socks5,
}

pub struct WebServer {
    port: u16,
    state: AppState,
}

impl WebServer {
    /// Создать новый веб-сервер (standalone режим)
    pub fn new(port: u16) -> Self {
        Self::new_with_auth(port, crate::web::auth::load_auth_state())
    }

    fn new_with_auth(port: u16, auth_state: crate::web::auth::AuthState) -> Self {
        let (media_signal_bus, _) = tokio::sync::broadcast::channel(256);
        Self {
            port,
            state: AppState {
                mdns: None,
                transport: None,
                p2p_transport: None,
                active_proxies: Arc::new(Mutex::new(std::collections::HashMap::new())),
                proxy_tasks: Arc::new(Mutex::new(std::collections::HashMap::new())),
                discovered_nodes: Arc::new(Mutex::new(Vec::new())),
                node_info: NodeInfo::default_standalone(),
                node_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                proxy_resp_rx: Arc::new(Mutex::new(None)),
                proxy_tunnel_rx: Arc::new(Mutex::new(None)),
                active_proxy_client: Arc::new(Mutex::new(None)),
                active_socks5_proxy: Arc::new(Mutex::new(None)),
                socks5_resp_rx: Arc::new(Mutex::new(None)),
                socks5_tunnel_rx: Arc::new(Mutex::new(None)),
                chat_manager: Arc::new(Mutex::new(None)),
                file_transfer_manager: Arc::new(Mutex::new(None)),
                p2p_tunnel_manager: Arc::new(Mutex::new(None)),
                media_manager: Arc::new(tokio::sync::Mutex::new(None)),
                media_signal_bus,
                incoming_calls: Arc::new(Mutex::new(std::collections::HashMap::new())),
                incoming_video_calls: Arc::new(Mutex::new(std::collections::HashMap::new())),
                auth_state,
            },
        }
    }

    /// Создать веб-сервер с привязкой к P2P transport
    pub fn with_transport(port: u16, transport: Arc<crate::P2PTransport>) -> Self {
        let (media_signal_bus, _) = tokio::sync::broadcast::channel(256);
        Self {
            port,
            state: AppState {
                mdns: None,
                transport: Some(transport),
                p2p_transport: None,
                active_proxies: Arc::new(Mutex::new(std::collections::HashMap::new())),
                proxy_tasks: Arc::new(Mutex::new(std::collections::HashMap::new())),
                discovered_nodes: Arc::new(Mutex::new(Vec::new())),
                node_info: NodeInfo::default(),
                node_running: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                proxy_resp_rx: Arc::new(Mutex::new(None)),
                proxy_tunnel_rx: Arc::new(Mutex::new(None)),
                active_proxy_client: Arc::new(Mutex::new(None)),
                active_socks5_proxy: Arc::new(Mutex::new(None)),
                socks5_resp_rx: Arc::new(Mutex::new(None)),
                socks5_tunnel_rx: Arc::new(Mutex::new(None)),
                chat_manager: Arc::new(Mutex::new(None)),
                file_transfer_manager: Arc::new(Mutex::new(None)),
                p2p_tunnel_manager: Arc::new(Mutex::new(None)),
                media_manager: Arc::new(tokio::sync::Mutex::new(None)),
                media_signal_bus,
                incoming_calls: Arc::new(Mutex::new(std::collections::HashMap::new())),
                incoming_video_calls: Arc::new(Mutex::new(std::collections::HashMap::new())),
                auth_state: crate::web::auth::load_auth_state(),
            },
        }
    }

    /// Установить proxy channels
    pub fn with_proxy_channels(
        mut self,
        proxy_resp_rx: Arc<
            Mutex<
                Option<
                    tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::proxy::ProxyResponse)>,
                >,
            >,
        >,
        proxy_tunnel_rx: Arc<
            Mutex<
                Option<
                    tokio::sync::mpsc::Receiver<(
                        crate::util::HashId,
                        crate::proxy::ProxyTunnelData,
                    )>,
                >,
            >,
        >,
    ) -> Self {
        self.state.proxy_resp_rx = proxy_resp_rx;
        self.state.proxy_tunnel_rx = proxy_tunnel_rx;
        self
    }

    /// Установить SOCKS5 proxy channels
    pub fn with_socks5_channels(
        mut self,
        socks5_resp_rx: Arc<
            Mutex<
                Option<
                    tokio::sync::mpsc::Receiver<(
                        crate::util::HashId,
                        crate::socks5::Socks5ProxyResponse,
                    )>,
                >,
            >,
        >,
        socks5_tunnel_rx: Arc<
            Mutex<
                Option<
                    tokio::sync::mpsc::Receiver<(
                        crate::util::HashId,
                        crate::socks5::Socks5TunnelData,
                    )>,
                >,
            >,
        >,
    ) -> Self {
        self.state.socks5_resp_rx = socks5_resp_rx;
        self.state.socks5_tunnel_rx = socks5_tunnel_rx;
        self
    }

    /// Установить P2P Communication Transport (port 9998)
    pub fn with_p2p_transport(mut self, p2p_transport: Arc<crate::p2p::P2PTransport>) -> Self {
        self.state.p2p_transport = Some(p2p_transport);
        self
    }

    /// Установить media signaling bus
    pub fn with_media_signal_bus(
        mut self,
        media_signal_bus: tokio::sync::broadcast::Sender<crate::web::media_api::MediaSignalEvent>,
    ) -> Self {
        self.state.media_signal_bus = media_signal_bus;
        self
    }

    /// Установить mDNS сервис
    pub async fn with_mdns(mut self, mdns: Arc<crate::MdnsService>) -> Self {
        self.state.mdns = Some(mdns.clone());

        // Запустить background task для обновления кэша нод
        let discovered_cache = self.state.discovered_nodes.clone();
        let mdns_for_cache = mdns;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
            loop {
                interval.tick().await;

                let nodes = mdns_for_cache.get_discovered().await;
                let node_count = nodes.len();
                *discovered_cache.lock().await = nodes;
                // info!("🔄 mDNS cache updated: {} nodes", node_count);  // 🔇 Silenced spam
            }
        });

        self
    }

    /// Установить информацию о ноде
    pub fn with_node_info(mut self, node_info: NodeInfo) -> Self {
        self.state.node_info = node_info;
        self
    }

    /// Установить Chat Manager
    pub fn with_chat_manager(
        mut self,
        chat_manager: std::sync::Arc<crate::communication::ChatManager>,
    ) -> Self {
        self.state.chat_manager = Arc::new(Mutex::new(Some(chat_manager)));
        self
    }

    /// Установить File Transfer Manager
    pub fn with_file_transfer_manager(
        mut self,
        file_transfer_manager: std::sync::Arc<crate::communication::FileTransferManager>,
    ) -> Self {
        self.state.file_transfer_manager = Arc::new(Mutex::new(Some(file_transfer_manager)));
        self
    }

    /// Установить P2P Tunnel Manager
    pub fn with_p2p_tunnel_manager(
        mut self,
        tunnel_manager: crate::p2p_tunnel::P2PTunnelManager,
    ) -> Self {
        self.state.p2p_tunnel_manager = Arc::new(Mutex::new(Some(tunnel_manager)));
        self
    }

    /// Установить Group Manager

    pub fn with_media_manager(
        mut self,
        media_manager: crate::media::session::MediaSessionManager,
    ) -> Self {
        self.state.media_manager = Arc::new(tokio::sync::Mutex::new(Some(media_manager)));
        self
    }


    /// Установить pre-loaded AuthState (вместо авто-загрузки в конструкторе)
    pub fn with_auth_state(mut self, auth_state: crate::web::auth::AuthState) -> Self {
        self.state.auth_state = auth_state;
        self
    }

    /// Установить очередь входящих звонков (shared с main.rs signal loop)
    pub fn with_incoming_calls(
        mut self,
        incoming_calls: Arc<Mutex<std::collections::HashMap<String, IncomingCallInfo>>>,
    ) -> Self {
        self.state.incoming_calls = incoming_calls;
        self
    }

    pub fn with_incoming_video_calls(
        mut self,
        incoming_video_calls: Arc<Mutex<std::collections::HashMap<String, IncomingCallInfo>>>,
    ) -> Self {
        self.state.incoming_video_calls = incoming_video_calls;
        self
    }

    /// Запустить сервер (блокирующий)
    pub async fn run(self) -> Result<(), String> {
        let port = self.port;
        let state = self.state.clone();
        let app = self.create_router(state.clone()).with_state(state);

        // На Linux оболочка работает через локальный сокет владельца; TCP-порт для браузера не открывается.
        #[cfg(target_os = "linux")]
        {
            let socket = super::ui_socket::Endpoint::bind(&crate::util::data_dir::data_dir())
                .map_err(|_| "Cannot start UI socket: unsafe permissions, occupied socket, or unavailable data directory".to_string())?;
            if std::env::var_os("YANDI_LOCAL_TCP").is_some() {
                // только тренировочная сеть (testnet): рядом с сокетом открывается и петля; на боевом узле переменной нет
                let addr = format!("127.0.0.1:{}", port);
                let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;
                info!("🌐 Тренировочная сеть: управление ещё и по http://{}", addr);
                let tcp = axum::serve(listener, app.clone()).into_future();
                tokio::select! {
                    result = tcp => result.map_err(|e| format!("Server error: {e}"))?,
                    result = socket.serve(app) => result.map_err(|_| "UI socket listener stopped".to_string())?,
                }
            } else {
                info!("🌐 Оболочка подключается через локальный сокет (TCP-порта нет)");
                socket
                    .serve(app)
                    .await
                    .map_err(|_| "UI socket listener stopped".to_string())?;
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let addr = format!("127.0.0.1:{}", port);
            info!("🌐 Web UI available at: http://{}", addr);
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    tokio::signal::ctrl_c()
                        .await
                        .expect("failed to install CTRL+C handler");
                })
                .await
                .map_err(|e| format!("Server error: {e}"))?;
        }

        Ok(())
    }

    /// Создать роутер
    fn create_router(&self, state_for_auth: AppState) -> Router<AppState> {
        Router::new()
            // HTML pages (protected)
            .route("/", get(index_handler))
            // кто может выходить в интернет через этот узел (выбор владельца)
            .merge(crate::exit_policy::router())
            .merge(crate::guest::router())
            .merge(crate::mail_policy::router())
            // закрытая среда узла: пароль владельца, блокировка, смена пароля
            .merge(vault_router())
            .merge(crate::upstream_proxy::router())
            // карточки узлов и списки сети по странам
            // доверенные узлы: визитка этого узла, добавить/убрать узел
            .merge(crate::web::peers::router())
            .merge(crate::network_offers::router())
            .merge(crate::hops_net::router())
            .merge(crate::supervisor::router())
            .route("/contacts", get(contacts_handler))
            .route("/gateways", get(gateways_handler))
            .route("/settings", get(settings_handler))
            .route("/chat", get(chat_handler))
            .route("/groups", get(groups_handler))
            // Static files (protected — served only to logged-in users)
            .route("/style.css", get(style_handler))
            .route("/chat.css", get(chat_css_handler))
            .route("/app.js", get(script_handler))
            .route("/speed.js", get(speed_script_handler))
            .route("/ui/voice-call.js", get(voice_call_handler))
            .route("/ui/phone-call.js", get(phone_call_js_handler))
            .route("/ui/video-call.js", get(video_call_handler))
            .route("/media/*path", get(media_handler))
            // API endpoints (protected)
            .route("/api/status", get(api_status))
            .route("/api/admin/session", get(api_admin_session_get).post(api_admin_session_post).delete(api_admin_session_delete))
            .route("/api/admin/password", post(api_admin_password_post))
            .route("/api/alerts", get(api_alerts))
            .route("/api/alerts/ack", post(api_alerts_ack))
            .route("/api/notices", get(api_notices_get))
            .route("/api/notices/ack", post(api_notices_ack))
            .route("/api/update/status", get(|| async { Json(crate::updates::status_json()) }))
            .route("/api/update/check", post(|| async { Json(crate::updates::check_now().await) }))
            .route("/api/speed", get(api_speed))
            .route("/api/nodes", get(api_nodes))
            .route("/api/connect/:short_id", post(api_connect))
            .route("/api/proxy/start/:short_id", post(api_proxy_start))
            .route("/api/proxy/stop/:short_id", post(api_proxy_stop))
            .route("/api/proxy/status", get(api_proxy_status))
            // SOCKS5 Proxy API endpoints
            .route("/api/socks5/start/:short_id", post(api_socks5_start))
            .route("/api/socks5/stop/:short_id", post(api_socks5_stop))
            .route("/api/socks5/status", get(api_socks5_status))
            .route("/api/socks5/personal", post(api_socks5_personal))
            .route("/api/gateway/start", post(api_gateway_start))
            .route("/api/gateway/stop", post(api_gateway_stop))
            .route(
                "/api/contacts",
                get(api_contacts_get)
                    .post(api_contacts_post)
                    .delete(api_contacts_delete),
            )
            .route("/api/contacts/export", get(api_contacts_export))
            .route("/api/contacts/local", post(api_contacts_local))
            .route("/api/network/relay", get(api_network_relay).post(api_network_relay_set))
            .route(
                "/api/gateways",
                get(api_gateways_get)
                    .post(api_gateways_post)
                    .delete(api_gateways_delete),
            )
            .route("/api/settings", get(api_settings_get).put(api_settings_put))
            // Chat API endpoints
            .route("/api/chat/send/:peer_id", post(api_chat_send))
            .route("/api/chat/history/:peer_id", get(api_chat_history))
            .route("/api/chat/clear/:peer_id", post(api_chat_clear))
            .route("/api/chat/react/:peer_id", post(api_chat_react))
            .route(
                "/api/chat/edit/:peer_id",
                axum::routing::patch(api_chat_edit),
            )
            .route(
                "/api/chat/delete/:peer_id",
                axum::routing::delete(api_chat_delete),
            )
            .route("/api/chats", get(api_chats_list))
            // File Transfer API endpoints
            .route("/api/files/upload", post(api_files_upload))
            .route("/api/files/send-chunk/:peer_id", post(api_files_send_chunk))
            .route("/api/files/send-file/:peer_id", post(api_files_send_direct))
            .route("/api/files/send/:peer_id", post(api_files_send_direct))
            .route(
                "/api/files/send/:peer_id/:filename",
                post(api_files_send_uploaded),
            )
            .route(
                "/api/files/content/:file_id/:filename",
                get(api_files_content),
            )
            .route(
                "/api/files/status/:file_id/:filename",
                get(api_files_status),
            )
            // P2P Tunnel API endpoints
            .route("/api/tunnel/start/:short_id", post(api_tunnel_start))
            .route("/api/tunnel/stop/:short_id", post(api_tunnel_stop))
            .route("/api/tunnel/list", get(api_tunnel_list))
            .route("/api/tunnel/status/:short_id", get(api_tunnel_status))
            // P2P Communication API endpoints (port 9998)
            .route("/api/p2p/peers", get(api_p2p_peers))
            .route("/api/p2p/status", get(api_p2p_status))
            .route("/api/p2p/sync", post(api_p2p_sync))
            // Node control endpoints
            .route("/api/node/stop", post(api_node_stop))
            .route("/api/node/start", post(api_node_start))
            // Client configuration endpoint (for mobile app)
            .route("/api/config", get(api_config_get))
            .route("/api/profile/me", get(api_profile_get).put(api_profile_put))
            .route("/api/profile/:short_id", get(api_profile_get_by_short_id))
            .route("/api/profile/avatar", post(api_profile_avatar))
            .route("/api/avatar/:short_id", get(api_avatar_get))
            // Media API endpoints for voice/video calls
            .route("/api/media/call/start", post(media_api::start_call))
            .route("/api/media/call/:call_id/end", delete(media_api::end_call))
            .route("/api/media/call/:call_id", get(media_api::get_call_info))
            .route("/api/media/calls", get(media_api::list_calls))
            .route(
                "/api/media/ws/:peer_id",
                get(media_api::media_websocket_handler),
            )
            .route(
                "/api/media/incoming-call",
                get(media_api::get_incoming_call),
            )
            .route(
                "/api/media/call/:call_id/accept",
                post(media_api::accept_call),
            )
            .route(
                "/api/media/call/:call_id/reject",
                post(media_api::reject_call),
            )
            .route("/api/media/call/end", post(media_api::end_active_call))
            // Video call endpoints
            .route(
                "/api/media/video/call/start",
                post(media_api::start_video_call),
            )
            .route(
                "/api/media/video/incoming-call",
                get(media_api::get_incoming_video_call),
            )
            .route(
                "/api/media/video/call/:call_id/accept",
                post(media_api::accept_video_call),
            )
            .route(
                "/api/media/video/call/:call_id/reject",
                post(media_api::reject_video_call),
            )
            .route(
                "/api/media/video/call/end",
                post(media_api::end_active_video_call),
            )
            // Pairing endpoints
            .route("/api/mobile/pairing", get(mobile_pairing_handler))
            .route("/api/mobile/devices", get(mobile_devices_handler))
            .route("/api/mobile/other-devices", get(mobile_other_devices_handler))
            .route("/api/complaints", get(complaints_list_handler))
            .route("/api/complaints/:id", get(complaint_get_handler))
            .route("/api/complaints/:id/reply", post(complaint_reply_handler))
            .route("/api/complaints/:id/status", post(complaint_status_handler))
            .route("/api/complaints/:id/measure", post(complaint_measure_handler))
            .route("/api/mobile/phone-action", post(mobile_phone_action_handler))
            .route("/api/mobile/devices/:id/promote", post(mobile_device_promote_handler))
            .route("/api/mobile/devices/:id/revoke-branch", post(mobile_device_revoke_branch_handler))
            .route("/api/mobile/invite-policy", get(mobile_invite_policy_get).post(mobile_invite_policy_post))
            .route(
                "/api/mobile/devices/:id",
                axum::routing::delete(mobile_device_remove_handler),
            )
            // Groups: created and deleted only here (this computer); content is encrypted by the phones
            // Calls phone ↔ this page (signals relayed by the node, media browser ↔ phone directly)
            .route("/api/mobile/call/events", get(mobile_call_events))
            .route("/api/mobile/call/signal", post(mobile_call_signal))
            .route("/api/mobile/call/turn", get(mobile_call_turn))
            .route("/api/mobile/call/log", post(mobile_call_log))
            .route(
                "/api/mobile/groups",
                get(crate::mobile_groups::pc_list),
            )
            // удаление группы — только с повторным вводом пароля узла (открытая сессия админки не даёт права удалить)
            .route("/api/mobile/groups/:gid/delete", post(api_mobile_group_delete))
            // группы на компьютере: узел как участник (свой ключ, своя лента); видно только то, куда узел вошёл сам
                        .route("/api/groups", get(api_groups))
            .route("/api/groups/create", post(api_groups_create))
            .route("/api/groups/:gid/join", post(api_groups_join))
            .route("/api/groups/:gid/request", post(api_groups_request))
            .route("/api/groups/:gid/messages", get(api_groups_messages))
            .route("/api/groups/:gid/send", post(api_groups_send))
            .route("/api/groups/:gid/rules", post(api_groups_rules))
            .route("/api/groups/:gid/leave", post(api_groups_leave))
            .route("/api/groups/:gid/forget", post(api_groups_forget))
            .route("/api/groups/:gid/info", get(api_groups_info))
            .route("/api/groups/:gid/mod", post(api_groups_mod))
            .route("/api/groups/:gid/settings", post(api_groups_settings))
            .route("/api/groups/:gid/autoclean", post(api_groups_autoclean))
            .route("/api/groups/:gid/profile", post(api_groups_profile))
            .route("/api/groups/:gid/vote", post(api_groups_vote))
            .route("/api/groups/:gid/avatar/:alias", get(api_groups_member_avatar))
            .route("/api/groups/:gid/transfer", post(api_groups_transfer))
            // файлы групп от компьютера: тело запроса — сам файл (до 200 МБ), шифрование и нарезка — в узле
            .route(
                "/api/groups/:gid/upload",
                post(api_groups_upload).layer(axum::extract::DefaultBodyLimit::max(crate::mobile_group_files::MAX_GROUP_FILE as usize + (1 << 20))),
            )
            .route("/api/groups/:gid/file/:seq", get(api_groups_file))
            .route("/api/groups/:gid/invite-code", post(api_groups_invite_code))
            .route("/api/groups/:gid/invite-device", post(api_groups_invite_device))
            .route("/api/groups/:gid/answer", post(api_groups_answer))
            .route("/api/groups/devices", get(api_groups_devices))
            .route("/pair/qr", get(pair_qr_handler))
            .route("/pair/qr.json", get(pair_qr_json_handler))
            .route("/pair/issue", post(pair_issue_handler))
            // пока закрытая среда заперта, расшифрованное содержимое владельца (группы, жалобы, файлы переписки) не отдаётся
            .route_layer(axum::middleware::from_fn(locked_guard))
            // админские маршруты (жалобы, сопряжение, настройки узла) — только в сеансе администратора (после auth: он слой снаружи)
            .route_layer(axum::middleware::from_fn(admin_guard))
            // Apply auth middleware to all routes above
            .route_layer(axum::middleware::from_fn_with_state(
                state_for_auth,
                auth_middleware,
            ))
            // Public routes — not protected by auth middleware
            .route("/login", get(login_page_handler))
            .route("/setup", get(setup_page_handler))
            .route("/api/auth/login", post(api_auth_login))
            .route("/api/auth/state", get(|State(state): State<AppState>| async move {
                crate::web::auth::ui_auth_state(&state.auth_state)
            }))
            .route("/api/auth/setup", post(api_auth_setup))
            .route("/api/auth/logout", get(api_auth_logout))
            .route("/api/auth/rebind", post(api_auth_rebind))
            .route("/api/auth/recover", post(api_auth_recover))
            .layer(axum::middleware::from_fn(host_guard))
            .layer(TraceLayer::new_for_http())
    }
}

#[cfg(unix)]
fn vault_router() -> Router<AppState> {
    crate::closed_api::router().merge(crate::backup_api::router())
}

#[cfg(not(unix))]
fn vault_router() -> Router<AppState> {
    Router::new()
}

/// Можно ли обращаться к веб-интерфейсу под таким заголовком `Host`. Интерфейс слушает только этот компьютер;
/// чужой сайт через подмену DNS (DNS rebinding) заставляет браузер ходить на 127.0.0.1 под своим именем — такие запросы отклоняем.
pub fn host_allowed(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        // [::1]:порт
        match rest.split_once(']') {
            Some((ip, tail)) if tail.is_empty() || tail.starts_with(':') => ip,
            _ => return false,
        }
    } else {
        host.rsplit_once(':')
            .map(|(h, port)| {
                if port.chars().all(|c| c.is_ascii_digit()) {
                    h
                } else {
                    host
                }
            })
            .unwrap_or(host)
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

async fn host_guard(req: axum::extract::Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(host_allowed)
        .unwrap_or(false);
    if !ok {
        return (StatusCode::FORBIDDEN, "forbidden host").into_response();
    }
    let h = req.headers();
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok());
    let upgrade = get("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    if !request_origin_ok(req.method().as_str(), upgrade, get("host"), get("origin"), get("sec-fetch-site")) {
        return (StatusCode::FORBIDDEN, "forbidden origin").into_response();
    }
    next.run(req).await
}

/// Защита от подделки запросов чужой страницей (CSRF), в том числе с другого порта того же компьютера: изменяющие запросы (всё, кроме
/// GET/HEAD/OPTIONS) и подключение веб-сокета принимаются, только если браузер говорит, что запрос с этой же страницы узла: `Origin` —
/// ровно этот адрес и порт, `Sec-Fetch-Site` — не `cross-site`/`same-site`. Без этих заголовков (не браузер: curl, стенды) — пропускаем,
/// их и так защищает сессия.
fn request_origin_ok(method: &str, websocket: bool, host: Option<&str>, origin: Option<&str>, fetch_site: Option<&str>) -> bool {
    let unsafe_method = !matches!(method, "GET" | "HEAD" | "OPTIONS");
    if !unsafe_method && !websocket {
        return true;
    }
    if let Some(site) = fetch_site {
        if site.eq_ignore_ascii_case("cross-site") || site.eq_ignore_ascii_case("same-site") {
            return false;
        }
    }
    match origin {
        None => true,
        Some(o) => {
            let Some(host) = host else { return false };
            let o = o.trim();
            o == format!("http://{host}") || o == format!("https://{host}")
        }
    }
}

#[cfg(test)]
mod origin_guard_tests {
    use super::request_origin_ok;

    #[test]
    fn foreign_pages_cannot_change_anything() {
        let host = Some("127.0.0.1:9999");
        // своя страница
        assert!(request_origin_ok("POST", false, host, Some("http://127.0.0.1:9999"), Some("same-origin")));
        // чужая страница на другом порту того же компьютера (тот же «сайт», другой источник)
        assert!(!request_origin_ok("POST", false, host, Some("http://127.0.0.1:18080"), Some("same-site")));
        assert!(!request_origin_ok("POST", false, host, Some("http://127.0.0.1:18080"), None));
        assert!(!request_origin_ok("DELETE", false, host, Some("https://evil.example"), Some("cross-site")));
        assert!(!request_origin_ok("POST", false, host, Some("null"), None), "Origin: null (песочница, file://)");
        assert!(!request_origin_ok("POST", false, host, None, Some("cross-site")), "без Origin, но браузер говорит «чужой сайт»");
        // веб-сокет с чужой страницы — тоже нет
        assert!(!request_origin_ok("GET", true, host, Some("http://127.0.0.1:18080"), None));
        assert!(request_origin_ok("GET", true, host, Some("http://127.0.0.1:9999"), None));
        // чтение страниц и не-браузеры (curl, стенды) не затронуты
        assert!(request_origin_ok("GET", false, host, Some("http://evil.example"), Some("cross-site")));
        assert!(request_origin_ok("POST", false, host, None, None));
        // имя localhost вместо адреса — своя страница, если совпадает с Host
        assert!(request_origin_ok("POST", false, Some("localhost:9999"), Some("http://localhost:9999"), None));
    }
}

#[cfg(test)]
mod host_guard_tests {
    use super::{host_allowed, media_name_ok};

    /// How many bytes a handler written like ours (`while let Ok(Some(field))`, `field.bytes()`) manages to buffer from a 3 MB upload.
    async fn buffered_from_3mb(limit: Option<usize>) -> usize {
        use axum::{extract::Multipart, routing::post, Router};
        use tower::ServiceExt;
        async fn take(mut m: Multipart) -> String {
            let mut total = 0usize;
            while let Ok(Some(f)) = m.next_field().await {
                match f.bytes().await {
                    Ok(b) => total += b.len(),
                    Err(_) => break,
                }
            }
            total.to_string()
        }
        let mut app = Router::new().route("/", post(take));
        if let Some(n) = limit {
            app = app.layer(axum::extract::DefaultBodyLimit::max(n));
        }
        let mut body =
            b"--B\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\n\r\n"
                .to_vec();
        body.extend(std::iter::repeat(0u8).take(3 * 1024 * 1024));
        body.extend_from_slice(b"\r\n--B--\r\n");
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "multipart/form-data; boundary=B")
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        String::from_utf8_lossy(&bytes).parse().unwrap_or(0)
    }

    /// The upload handlers buffer a whole multipart field, so they rely on the framework's default body limit (2 MB), which
    /// must refuse a 3 MB upload before the handler can buffer it. If a route ever raises the limit explicitly, this documents
    /// that the limit is what protects memory.
    #[tokio::test]
    async fn the_default_body_limit_stops_a_3mb_multipart_upload_and_a_raised_limit_lets_it_through(
    ) {
        assert_eq!(
            buffered_from_3mb(None).await,
            0,
            "the default limit refuses a 3 MB upload"
        );
        assert_eq!(
            buffered_from_3mb(Some(10 * 1024 * 1024)).await,
            3 * 1024 * 1024,
            "a raised limit lets it through"
        );
    }

    #[test]
    fn media_names_cannot_leave_the_media_directory() {
        for ok in ["ring.mp3", "sounds/ring.wav"] {
            assert!(media_name_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "../secret",
            "a/../../b",
            "/etc/passwd",
            "..\\x",
            "a\0b",
            "./..",
        ] {
            assert!(!media_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_this_computer_names_pass() {
        for ok in [
            "localhost",
            "localhost:8080",
            "127.0.0.1:9000",
            "127.0.0.1",
            "[::1]:8080",
            "[::1]",
            "LOCALHOST:1",
        ] {
            assert!(host_allowed(ok), "{ok}");
        }
        for bad in [
            "",
            "evil.com",
            "evil.com:8080",
            "localhost.evil.com",
            "127.0.0.1.evil.com:80",
            "evil.com:127.0.0.1",
            "192.168.1.5:8080",
            "[::2]:80",
            "[::1]x",
            "localhost:80:80",
            "0.0.0.0:80",
            "127.0.0.2",
        ] {
            assert!(!host_allowed(bad), "{bad}");
        }
    }
}

// === Auth Middleware ===

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    use std::sync::atomic::Ordering;

    let auth = &state.auth_state;

    #[cfg(target_os = "linux")]
    if super::ui_socket::is_local(&req) {
        // Local owner authorisation does not unlock the vault or bypass StepUp.
        return next.run(req).await;
    }

    // First run — redirect to setup
    if !auth.is_setup.load(Ordering::Relaxed) {
        return Redirect::to("/setup").into_response();
    }

    // Check session cookie
    let cookie_str = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = crate::web::auth::extract_session_token(cookie_str);

    if let Some(t) = token {
        if auth.verify_session(&t) {
            return next.run(req).await;
        }
    }

    // No valid session — redirect to login (with rebind flag if needed)
    if auth.needs_rebind.load(Ordering::Relaxed) {
        Redirect::to("/login?rebind=1").into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}

// === Auth Page Handlers ===

fn grp_reply(r: Result<serde_json::Value, String>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

async fn api_groups() -> Response {
    grp_reply(crate::group_client::list())
}

#[derive(Deserialize)]
struct MyGroupCreate {
    name: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    about: String,
    kind: String,
}

async fn api_groups_create(Json(b): Json<MyGroupCreate>) -> Response {
    grp_reply(crate::group_client::create(&b.name, &b.topic, &b.about, &b.kind).map(|id| serde_json::json!({"id": id})))
}

#[derive(Deserialize)]
struct MyGroupJoin {
    #[serde(default)]
    code: String,
    #[serde(default)]
    name: String,
}

async fn api_groups_join(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupJoin>) -> Response {
    let code = if b.code.trim().is_empty() { None } else { Some(b.code.trim().to_string()) };
    grp_reply(crate::group_client::join(&gid, code.as_deref(), &b.name).map(|_| serde_json::json!({"ok": true})))
}

async fn api_groups_request(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupJoin>) -> Response {
    grp_reply(crate::group_client::request_invite(&gid, &b.name).map(|_| serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
struct MyGroupAfter {
    #[serde(default)]
    after: u64,
}

async fn api_groups_messages(axum::extract::Path(gid): axum::extract::Path<String>, axum::extract::Query(q): axum::extract::Query<MyGroupAfter>) -> Response {
    grp_reply(crate::group_client::messages(&gid, q.after))
}

#[derive(Deserialize)]
struct MyGroupSend {
    text: String,
    /// ответ-цитата: номер записи группы, псевдоним автора и выдержка текста
    #[serde(default)]
    reply: Option<MyGroupReply>,
}

#[derive(Deserialize)]
struct MyGroupReply {
    seq: u64,
    #[serde(default)]
    author: String,
    #[serde(default)]
    text: String,
}

async fn api_groups_send(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupSend>) -> Response {
    let text = b.text.trim().to_string();
    if text.is_empty() || text.len() > 8000 {
        return grp_reply(Err("пустое или слишком длинное сообщение".into()));
    }
    // ссылка — отдельный вид записи, чтобы группа могла её запретить (как в приложении)
    let is_link = {
        let l = text.to_lowercase();
        l.contains("http://") || l.contains("https://") || l.contains("www.")
    };
    let kind = if is_link { crate::group_client::KIND_LINK } else { crate::group_client::KIND_TEXT };
    let mut body = serde_json::json!({"t": text});
    if let Some(r) = b.reply {
        // поле «r» в записи группы: s — номер цитируемой записи, a — автор, t — выдержка (не длиннее 200 знаков)
        body["r"] = serde_json::json!({
            "s": r.seq,
            "a": r.author.chars().take(64).collect::<String>(),
            "t": r.text.chars().take(200).collect::<String>(),
        });
    }
    grp_reply(crate::group_client::send(&gid, kind, body).map(|seq| serde_json::json!({"seq": seq})))
}

#[derive(Deserialize)]
struct MyGroupRules {
    deny: Vec<u8>,
}

async fn api_groups_rules(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupRules>) -> Response {
    grp_reply(crate::group_client::set_rules(&gid, &b.deny).map(|_| serde_json::json!({"ok": true})))
}

async fn api_groups_leave(axum::extract::Path(gid): axum::extract::Path<String>) -> Response {
    grp_reply(crate::group_client::leave(&gid).map(|_| serde_json::json!({"ok": true})))
}

async fn api_groups_info(axum::extract::Path(gid): axum::extract::Path<String>) -> Response {
    grp_reply(crate::group_client::info(&gid))
}

#[derive(Deserialize)]
struct MyGroupMod {
    action: String,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    seq: Option<u64>,
}

async fn api_groups_mod(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupMod>) -> Response {
    grp_reply(crate::group_client::moderate(&gid, &b.action, b.alias.as_deref(), b.seq).map(|_| serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
struct MyGroupSettings {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    about: Option<String>,
}

#[derive(Deserialize)]
struct MyGroupAutoClean {
    days: u32,
}

async fn api_groups_autoclean(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupAutoClean>) -> Response {
    grp_reply(crate::group_client::set_auto_clean(&gid, b.days).map(|d| serde_json::json!({"ok": true, "days": d})))
}

#[derive(Deserialize)]
struct MyGroupVote {
    seq: u64,
    v: i64,
}

/// Картинка визитки участника группы (маленький PNG) для ленты и списка участников.
async fn api_groups_member_avatar(axum::extract::Path((gid, alias)): axum::extract::Path<(String, String)>) -> Response {
    match crate::group_client::member_avatar(&gid, &alias) {
        Ok(Some(png)) => ([(axum::http::header::CONTENT_TYPE, "image/png"), (axum::http::header::CACHE_CONTROL, "no-store")], png).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Монетка одобрения: v = 1 или -1.
async fn api_groups_vote(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupVote>) -> Response {
    grp_reply(crate::group_client::vote(&gid, b.seq, b.v).map(|_| serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
struct MyGroupShare {
    share: bool,
}

/// Показывать ли ник и аватар компьютера в группе (по желанию, из единого профиля узла).
async fn api_groups_profile(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupShare>) -> Response {
    grp_reply(crate::group_client::share_card(&gid, b.share).map(|_| serde_json::json!({"ok": true})))
}

async fn api_groups_settings(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupSettings>) -> Response {
    grp_reply(crate::group_client::update_settings(&gid, b.name.as_deref(), b.topic.as_deref(), b.about.as_deref()).map(|_| serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
struct MyGroupTransfer {
    alias: String,
}

async fn api_groups_transfer(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupTransfer>) -> Response {
    grp_reply(crate::group_client::transfer_owner(&gid, &b.alias).map(|_| serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
struct MyGroupUploadQ {
    #[serde(default)]
    name: String,
}

async fn api_groups_upload(axum::extract::Path(gid): axum::extract::Path<String>, axum::extract::Query(q): axum::extract::Query<MyGroupUploadQ>, body: axum::body::Bytes) -> Response {
    grp_reply(crate::group_client::upload_file(&gid, &q.name, body.to_vec()).await.map(|seq| serde_json::json!({"seq": seq})))
}

/// Файл из записи группы: расшифровывается и проверяется в узле; отдаётся как чужое вложение (безопасные типы — на месте, остальное — скачать;
/// `nosniff` и `sandbox`, чтобы файл не выполнился в источнике страницы администратора).
async fn api_groups_file(axum::extract::Path((gid, seq)): axum::extract::Path<(String, u64)>) -> Response {
    match crate::group_client::download_file(&gid, seq).await {
        Ok((name, data)) => {
            let content_type = safe_served_type(&name);
            let disposition = served_disposition(&content_type, &name);
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", content_type)
                .header("Content-Disposition", disposition)
                .header("X-Content-Type-Options", "nosniff")
                .header("Content-Security-Policy", "sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'")
                .header("Cache-Control", "no-store")
                .body(axum::body::Body::from(data))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

async fn api_groups_invite_code(axum::extract::Path(gid): axum::extract::Path<String>) -> Response {
    grp_reply(crate::group_client::invite_code(&gid).map(|v| serde_json::json!({"code": v["code"], "group_id": gid, "invite": format!("YANDI-GROUP-1:{}:{}", gid, v["code"].as_str().unwrap_or(""))})))
}

#[derive(Deserialize)]
struct MyGroupPeer {
    peer: String,
}

async fn api_groups_invite_device(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupPeer>) -> Response {
    grp_reply(crate::group_client::invite_device(&gid, &b.peer).await.map(|_| serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
struct MyGroupAnswer {
    rid: String,
    accept: bool,
    #[serde(default)]
    never: bool,
}

async fn api_groups_answer(axum::extract::Path(gid): axum::extract::Path<String>, Json(b): Json<MyGroupAnswer>) -> Response {
    grp_reply(crate::group_client::answer_request(&gid, &b.rid, b.accept, b.never).await.map(|_| serde_json::json!({"ok": true})))
}

async fn api_groups_devices() -> Response {
    grp_reply(Ok(crate::group_client::devices()))
}

async fn api_groups_forget(axum::extract::Path(gid): axum::extract::Path<String>) -> Response {
    grp_reply(crate::group_client::forget_local(&gid).map(|_| serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
struct StepUpDelete {
    password: String,
}

/// Повторная проверка пароля владельца перед разрушительным действием (удаление групп). `Err` — готовый ответ: пароль не передан
/// (403 `password_required`), неверен (401), слишком частые попытки (429). Попытки считаются тем же счётчиком, что и вход.
async fn require_step_up(state: &AppState, password: Option<&str>) -> Result<(), Response> {
    let Some(password) = password else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"status": "error", "error": "password_required", "message": "для этого действия нужен пароль узла"})),
        )
            .into_response());
    };
    if let Some(remaining) = state.auth_state.login_backoff_remaining() {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": format!("Слишком много попыток, подождите {}с", remaining.as_secs().max(1))})),
        )
            .into_response());
    }
    match crate::web::auth::verify_login_counted(password) {
        Ok(true) => {
            state.auth_state.record_login_success();
            Ok(())
        }
        Ok(false) => {
            state.auth_state.record_login_failure();
            Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Неверный пароль"}))).into_response())
        }
        Err(e) => Err(counted_login_error(e)),
    }
}

/// То же для действий администратора: проверяется админ-пароль с его собственным бюджетом попыток.
async fn require_admin_step_up(password: &str) -> Result<(), Response> {
    let password = password.to_string();
    match tokio::task::spawn_blocking(move || crate::web::auth::verify_admin_counted(&password)).await {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Неверный пароль администратора"}))).into_response()),
        Ok(Err(e)) => Err(counted_login_error(e)),
        Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Внутренняя ошибка"}))).into_response()),
    }
}

/// Удаление группы телефонов: пароль узла спрашивается заново (забытая открытой админка, дети, гости не удалят группу).
/// Попытки считаются тем же счётчиком, что и вход: перебор пароля через эту ручку упирается в ту же задержку.
async fn api_mobile_group_delete(State(state): State<AppState>, axum::extract::Path(gid): axum::extract::Path<String>, Json(body): Json<StepUpDelete>) -> Response {
    // удаление чужой группы — управление узлом: нужен сеанс администратора (его проверил admin_guard) и ЕЩЁ РАЗ админ-пароль
    if let Err(resp) = require_admin_step_up(&body.password).await {
        return resp;
    }
    crate::mobile_groups::pc_delete(axum::extract::Path(gid)).await
}

async fn login_page_handler() -> Html<&'static str> {
    Html(include_str!("ui/login.html"))
}

async fn setup_page_handler() -> Html<&'static str> {
    Html(include_str!("ui/setup.html"))
}

// === Auth API Handlers ===

#[derive(Deserialize)]
struct LoginRequest {
    login_password: String,
    #[serde(default)]
    remember_me: bool,
}

/// Ответ на отказ счётчика проверки пароля: закрыт вход паролем (403), счёт не сохранён или сбой проверки (500).
fn counted_login_error(e: crate::web::auth::CountedLogin) -> Response {
    use crate::web::auth::CountedLogin::*;
    match e {
        Locked => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Слишком много неверных паролей: войдите по сид-фразе (восстановление доступа)"})),
        ).into_response(),
        Storage(m) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Вход временно невозможен: {m}")})),
        ).into_response(),
        Failed(m) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": m}))).into_response(),
    }
}

async fn api_auth_login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> Response {
    // Brute-force throttle: reject before even hashing the password if
    // we're still cooling down from prior failures (see AuthState::
    // login_backoff_remaining — the web UI has no rate limiting otherwise
    // and defaults to binding 0.0.0.0).
    if let Some(remaining) = state.auth_state.login_backoff_remaining() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": format!("Слишком много попыток, подождите {}с", remaining.as_secs().max(1))})),
        ).into_response();
    }
    // все проверки пароля идут через один счёт: пять неверных подряд — дальше пароль не принимается совсем (даже верный)
    match crate::web::auth::verify_login_counted(&body.login_password) {
        Ok(true) => {
            state.auth_state.record_login_success();
            let token = state.auth_state.create_session(body.remember_me);
            let cookie = crate::web::auth::make_session_cookie(&token, body.remember_me);
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("Set-Cookie", cookie.parse().unwrap());
            (
                StatusCode::OK,
                resp_headers,
                Json(serde_json::json!({"ok": true})),
            )
                .into_response()
        }
        Ok(false) => {
            state.auth_state.record_login_failure();
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Неверный пароль"})),
            )
                .into_response()
        }
        Err(e) => counted_login_error(e),
    }
}

#[derive(Deserialize)]
struct SetupRequest {
    login_password: String,
    #[serde(default)]
    login_password_repeat: String,
    master_password: String,
    #[serde(default)]
    master_password_repeat: String,
}

async fn api_auth_setup(State(state): State<AppState>, Json(body): Json<SetupRequest>) -> Response {
    match crate::web::auth::setup_auth(
        &state.auth_state,
        &body.login_password,
        &body.login_password_repeat,
        &body.master_password,
        &body.master_password_repeat,
    ) {
        Ok(_master_key) => {
            let token = state.auth_state.create_session(false);
            let cookie = crate::web::auth::make_session_cookie(&token, false);
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("Set-Cookie", cookie.parse().unwrap());
            (
                StatusCode::OK,
                resp_headers,
                Json(serde_json::json!({"ok": true})),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
    }
}

async fn api_auth_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let cookie_str = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if let Some(token) = crate::web::auth::extract_session_token(cookie_str) {
        state.auth_state.invalidate_session(&token);
    }
    crate::web::admin::end(); // выход закрывает и сеанс администратора
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        "Set-Cookie",
        crate::web::auth::clear_session_cookie().parse().unwrap(),
    );
    resp_headers.insert("Location", "/login".parse().unwrap());
    (StatusCode::FOUND, resp_headers).into_response()
}

#[derive(Deserialize)]
struct RebindRequest {
    login_password: String,
    master_password: String,
    #[serde(default)]
    remember_me: bool,
}

async fn api_auth_rebind(
    State(state): State<AppState>,
    Json(body): Json<RebindRequest>,
) -> Response {
    // Same brute-force throttle as api_auth_login — rebind also accepts a
    // login password guess and was equally unthrottled.
    if let Some(remaining) = state.auth_state.login_backoff_remaining() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": format!("Слишком много попыток, подождите {}с", remaining.as_secs().max(1))})),
        ).into_response();
    }
    // Verify login password first (тот же счёт попыток, что и у входа)
    match crate::web::auth::verify_login_counted(&body.login_password) {
        Ok(true) => {
            state.auth_state.record_login_success();
        }
        Ok(false) => {
            state.auth_state.record_login_failure();
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Неверный пароль"})),
            )
                .into_response();
        }
        Err(e) => return counted_login_error(e),
    }

    match crate::web::auth::rebind_to_machine(&state.auth_state, &body.master_password) {
        Ok(()) => {
            let token = state.auth_state.create_session(body.remember_me);
            let session_cookie = crate::web::auth::make_session_cookie(&token, body.remember_me);
            let mut resp_headers = HeaderMap::new();
            resp_headers.append("Set-Cookie", session_cookie.parse().unwrap());
            resp_headers.append(
                "Set-Cookie",
                "yandi_rebind=; Path=/; Max-Age=0".parse().unwrap(),
            );
            (
                StatusCode::OK,
                resp_headers,
                Json(serde_json::json!({"ok": true})),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct RecoverRequest {
    recovery_code: String,
    new_login_password: String,
    #[serde(default)]
    remember_me: bool,
}

/// Forgot the login password: the recovery code (the "master key" written down at creation) sets a new one.
async fn api_auth_recover(
    State(state): State<AppState>,
    Json(body): Json<RecoverRequest>,
) -> Response {
    // the same brute-force throttle as the login: a wrong code counts as a failed attempt
    if let Some(remaining) = state.auth_state.login_backoff_remaining() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": format!("Слишком много попыток, подождите {}с", remaining.as_secs().max(1))})),
        ).into_response();
    }
    // новый пароль проверяется целиком до любых действий; сид-фраза проходит счёт закрытой среды (двадцать неверных стирают её), и только
    // потом заменяется пароль входа — отдельного проверочного пути для сида без лимита нет
    #[cfg(unix)]
    if let Err(e) = crate::web::auth::check_new_password(&body.new_login_password) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response();
    }
    #[cfg(unix)]
    if let Some(Err((code, text))) = crate::closed_api::recover_vault_if_configured(body.recovery_code.clone(), body.new_login_password.clone()).await {
        state.auth_state.record_login_failure();
        return (code, Json(serde_json::json!({"error": text}))).into_response();
    }
    let auth_state = state.auth_state.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::web::auth::recover_login(&auth_state, &body.recovery_code, &body.new_login_password)
            .map(|_| body.remember_me)
    })
    .await
    .unwrap_or_else(|_| Err("Внутренняя ошибка".to_string()));
    match result {
        Ok(remember_me) => {
            crate::web::auth::reset_login_failures(); // вход по сид-фразе снимает запрет входа паролем
            crate::web::admin::end();
            // сид-фраза владельца возвращает общий пароль: забытый админ-пароль не запирает узел; не удалось — не говорим «успех»
            if let Err(e) = crate::web::admin::clear_password() {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("Пароль входа изменён, но админ-пароль не сброшен: {e}. Повторите восстановление или удалите файл admin_auth.json.")})),
                )
                    .into_response();
            }
            state.auth_state.record_login_success();
            let token = state.auth_state.create_session(remember_me);
            let cookie = crate::web::auth::make_session_cookie(&token, remember_me);
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("Set-Cookie", cookie.parse().unwrap());
            (
                StatusCode::OK,
                resp_headers,
                Json(serde_json::json!({"ok": true})),
            )
                .into_response()
        }
        Err(e) => {
            state.auth_state.record_login_failure();
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": e})),
            )
                .into_response()
        }
    }
}

// === HTML Handlers ===

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("ui/index.html"))
}

async fn contacts_handler() -> Html<&'static str> {
    Html(include_str!("ui/contacts.html"))
}

async fn gateways_handler() -> Html<&'static str> {
    Html(include_str!("ui/gateways.html"))
}

async fn settings_handler() -> Html<&'static str> {
    Html(include_str!("ui/settings.html"))
}

async fn chat_handler() -> Html<&'static str> {
    Html(include_str!("ui/chat.html"))
}

async fn groups_handler() -> Html<&'static str> {
    Html(include_str!("ui/groups.html"))
}


// === Static File Handlers ===

async fn style_handler() -> impl IntoResponse {
    let css = include_str!("ui/style.css");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/css; charset=utf-8")
        .body(css.to_owned())
        .unwrap()
}

async fn chat_css_handler() -> impl IntoResponse {
    let css = include_str!("ui/chat.css");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/css; charset=utf-8")
        .body(css.to_owned())
        .unwrap()
}

async fn script_handler() -> impl IntoResponse {
    let js = include_str!("ui/app.js");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/javascript; charset=utf-8")
        .body(js.to_owned())
        .unwrap()
}

async fn speed_script_handler() -> impl IntoResponse {
    let js = include_str!("ui/speed.js");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/javascript; charset=utf-8")
        .body(js.to_owned())
        .unwrap()
}

// === API Handlers ===

/// Получить статус ноды
async fn api_status(State(state): State<AppState>) -> impl IntoResponse {
    let proxy_count = state.active_proxies.lock().await.len();
    let info = &state.node_info;

    // Проверяем реальный статус P2P ноды
    let is_running = state
        .node_running
        .load(std::sync::atomic::Ordering::Relaxed);
    let status = if is_running { "online" } else { "offline" };

    // Получаем NAT статус из транспорта
    let nat_status = if let Some(transport) = &state.transport {
        // Пытаемся определить NAT статус этой ноды
        // Для простоты покажем "Public" если есть внешний IP
        if info.external_ip != "unknown" && info.external_ip != "127.0.0.1" {
            "Public"
        } else {
            "BehindNAT"
        }
    } else {
        "Unknown"
    };

    let response = serde_json::json!({
        "is_local": info.is_local,
        "short_id": info.short_id,
        "cid": info.cid,
        "node_id": info.node_id,
        "role": info.role,
        "external_ip": info.external_ip,
        "virtual_ipv6": info.virtual_ipv6,
        "ipv6_short": info.ipv6_short,
        "discovery_port": info.discovery_port,
        "data_port": info.data_port,
        "web_port": info.web_port,
        "status": status,
        "nat_status": nat_status,
        "active_proxies": proxy_count,
        // внешние прокси из настроек: добавлено и сколько отвечает по последней проверке (null — среда заперта)
        "upstream_added": crate::upstream_proxy::summary().map(|s| s.0),
        "upstream_working": crate::upstream_proxy::summary().map(|s| s.1),
        "mode": if state.transport.is_some() { &info.role } else { "Web UI only" },
    });

    Json(response)
}

/// Получить скорость (входящую и исходящую)
async fn api_speed(State(state): State<AppState>) -> impl IntoResponse {
    let mut metrics = crate::netlayer::transport::WebTransportMetrics::default();

    if let Some(transport) = &state.transport {
        metrics = transport.get_web_metrics().await;
    }

    Json(serde_json::json!({
        "rx_speed": metrics.rx_speed,
        "rx_speed_mbps": metrics.rx_speed / 1024.0 / 1024.0,
        "rx_speed_kbps": metrics.rx_speed / 1024.0,
        "tx_speed": metrics.tx_speed,
        "tx_speed_mbps": metrics.tx_speed / 1024.0 / 1024.0,
        "tx_speed_kbps": metrics.tx_speed / 1024.0,
        "peer_rx_estimate": metrics.peer_rx_estimate,
        "peer_rx_estimate_mbps": metrics.peer_rx_estimate / 1024.0 / 1024.0,
        "peer_rx_estimate_kbps": metrics.peer_rx_estimate / 1024.0,
        "avg_rtt_ms": metrics.avg_rtt_ms,
        "loss_incoming": metrics.path0_loss_incoming_pct,
        "loss_outgoing": metrics.peer_path0_loss_pct,
        "path0_loss_incoming": metrics.path0_loss_incoming_pct,
        "peer_path0_loss": metrics.peer_path0_loss_pct,
        "clone_hit_pct": metrics.clone_hit_pct,
        "clone_hit_rate": metrics.clone_hit_rate,
        "wagons_per_sec": metrics.wagons_per_sec,
        "active_trains": metrics.active_trains,
        "depot_bytes": metrics.depot_bytes,
        "delivered_cache_size": metrics.delivered_cache_size,
        "evictions_total": metrics.evictions_total,
        "evictions_delta": metrics.evictions_delta,
        "timeout_total": metrics.timeout_total,
        "cleanup_total": metrics.cleanup_total,
        "total_wagons": metrics.total_wagons,
        "total_clone_hits": metrics.total_clone_hits,
        "wagon_sent_total": metrics.wagon_sent_total,
        "wagon_recv_total": metrics.wagon_recv_total,
        "wagon_drop_total": metrics.wagon_drop_total,
        "wagon_checksum_failed_total": metrics.wagon_checksum_failed_total,
        "wagon_drop_crc_pct": metrics.wagon_drop_crc_pct,
        "peer_count": metrics.peer_count,
    }))
}

/// Получить список всех нод (mDNS + P2P)
async fn api_nodes(State(state): State<AppState>) -> impl IntoResponse {
    use std::collections::HashMap;

    // 0. Узнаём собственный short_id чтобы НЕ выводить себя в списке —
    //    локальная нода не может подключиться к самой себе, а пользователь
    //    мог бы нажать «proxy» на собственной записи и ждать чудо.
    let self_short_id: Option<String> = state
        .transport
        .as_ref()
        .map(|t| hex::encode(&t.identity().node_id().0[..8]))
        .or_else(|| {
            // standalone-режим (без transport'а): берём из node_info.
            if state.node_info.short_id != "unknown" && state.node_info.short_id != "standalone" {
                Some(state.node_info.short_id.clone())
            } else {
                None
            }
        });
    let is_self = |sid: &str| self_short_id.as_deref().map(|s| s == sid).unwrap_or(false);

    // 1. Получаем mDNS ноды из кэша (отфильтровываем себя).
    let mdns_nodes: Vec<DiscoveredNode> = state
        .discovered_nodes
        .lock()
        .await
        .iter()
        .filter(|n| !is_self(&n.short_id))
        .cloned()
        .collect();

    // 2. Получаем P2P пиры из transport (без self).
    let mut p2p_peers = Vec::new();
    if let Some(transport) = &state.transport {
        let peers = transport.get_peers().await;
        for peer in peers {
            let peer_cid = hex::encode(&peer.id.0[..8]);
            if is_self(&peer_cid) {
                continue;
            }
            let already_in_mdns = mdns_nodes.iter().any(|n| n.short_id == peer_cid);

            if !already_in_mdns {
                p2p_peers.push(serde_json::json!({
                    "short_id": peer_cid,
                    "hostname": format!("{}.local", peer_cid),
                    "admin_port": 9999,
                    "role": "P2P Peer",
                    "nat_status": peer.get_nat_status().as_str(),
                    "discovery_port": 9000,
                    "data_port": 10000,
                    "source": "p2p",
                    "connected": true
                    // ❌ УБРАЛ IP адрес для безопасности!
                }));
            }
        }
    }

    // 3. Объединяем mDNS и P2P ноды
    let all_nodes: Vec<serde_json::Value> = mdns_nodes
        .iter()
        .map(|n| {
            serde_json::json!({
                "short_id": n.short_id,
                "hostname": n.hostname,
                "admin_port": n.admin_port,
                "role": n.role,
                "nat_status": "Unknown", // mDNS не даёт NAT статус
                "discovery_port": n.discovery_port,
                "data_port": n.data_port,
                "source": "mdns",
                "connected": true
            })
        })
        .chain(p2p_peers.into_iter())
        .collect();

    let response = serde_json::json!({
        "nodes": all_nodes,
        "total": all_nodes.len(),
        "mdns_count": mdns_nodes.len(),
        "p2p_count": all_nodes.len() - mdns_nodes.len(),
        "self_short_id": self_short_id
    });

    Json(response)
}

/// Подключиться к ноде
async fn api_connect(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available in standalone mode"
        }));
    }

    // TODO: Реализовать подключение через transport
    info!("Request to connect to node: {}", short_id);

    Json(serde_json::json!({
        "status": "success",
        "message": format!("Connecting to {}", short_id)
    }))
}

/// Запустить HTTP/SOCKS5 proxy через ноду
async fn api_proxy_start(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available in standalone mode"
        }));
    }

    let transport = state.transport.as_ref().unwrap();

    // Парсим short_id
    let short_id_bytes = match hex::decode(&short_id) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid short ID: {}", e)
            }));
        }
    };

    if short_id_bytes.len() != 8 {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Short ID must be 8 bytes (16 hex chars)"
        }));
    }

    // Ищем пира по short_id
    let peers = transport.get_peers().await;
    let gateway_peer = match peers
        .iter()
        .find(|p| &p.id.0[..8] == short_id_bytes.as_slice())
    {
        Some(peer) => peer,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer with short ID {} not found", short_id)
            }));
        }
    };

    info!(
        "🌐 Starting HTTP Proxy through gateway: {} ({})",
        short_id, gateway_peer.addr
    );

    // Проверяем - уже есть активный прокси?
    if state.active_proxies.lock().await.len() > 0 {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Proxy already running. Stop it first."
        }));
    }

    // Приёмники остаются в состоянии узла; прокси получает каналы-перекачки (после остановки можно запустить снова)
    let channels_present = state.proxy_resp_rx.lock().await.is_some() && state.proxy_tunnel_rx.lock().await.is_some();
    if !channels_present {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Proxy channels not available. May be already in use by CLI?"
        }));
    }
    let (resp_rx, resp_pump) = pump_channel(state.proxy_resp_rx.clone());
    let (tunnel_rx, tunnel_pump) = pump_channel(state.proxy_tunnel_rx.clone());

    // Создаём HttpProxyClient
    use crate::proxy::HttpProxyClient;

    let http_port = crate::core::get_config().ports.http_proxy;
    let mut proxy_config = crate::proxy::ProxyConfig::default();
    proxy_config.client_listen_addr = format!("127.0.0.1:{http_port}");
    let http_proxy =
        HttpProxyClient::new(transport.clone(), gateway_peer.id.clone()).with_config(proxy_config);

    let http_proxy = http_proxy.with_response_channel(resp_rx);
    let http_proxy = http_proxy.with_tunnel_data_channel(tunnel_rx);

    // Регистрируем Station
    transport.set_station(http_proxy.station.clone()).await;

    // Запускаем прокси в фоне
    let listener_task = tokio::spawn(async move {
        if let Err(e) = http_proxy.start().await {
            eprintln!("❌ HTTP Proxy error: {}", e);
        }
    });
    state
        .proxy_tasks
        .lock()
        .await
        .insert(short_id.clone(), vec![listener_task.abort_handle(), resp_pump, tunnel_pump]);

    // 🚨 ОТПРАВЛЯЕМ 0x30 ПАКЕТ НА GATEWAY ДЛЯ АВТОЗАПУСКА!
    info!(
        "📤 Sending StartProxyGateway (0x30) packet to gateway {}",
        short_id
    );
    let cmd_bytes = vec![0x30u8]; // StartProxyGateway

    if let Err(e) = transport.send_encrypted(gateway_peer.id, &cmd_bytes).await {
        abort_proxy_tasks(&state, &short_id).await;
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to send StartProxyGateway command: {}", e)
        }));
    }

    info!("✅ StartProxyGateway command sent to {}", short_id);

    // НЕ сохраняем в state (нельзя клонировать)
    // Вместо этого просто сохраняем metadata

    // Добавляем в active_proxies
    let proxy_info = ProxyInfo {
        short_id: short_id.clone(),
        proxy_type: ProxyType::Http,
        local_port: http_port,
        started_at: chrono::Utc::now().to_rfc3339(),
    };

    state
        .active_proxies
        .lock()
        .await
        .insert(short_id.clone(), proxy_info);

    info!(
        "✅ HTTP Proxy started on 127.0.0.1:{} through gateway {}",
        http_port, short_id
    );

    Json(serde_json::json!({
        "status": "success",
        "message": format!("Proxy started through {}", short_id),
        "local_port": http_port,
        "gateway": short_id
    }))
}

/// Передаёт сообщения из общего приёмника узла в канал, который забирает прокси. Приёмник остаётся в состоянии,
/// поэтому после остановки прокси его можно запустить снова (раньше приёмник уходил в прокси навсегда).
fn pump_channel<T: Send + 'static>(
    src: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<T>>>>,
) -> (tokio::sync::mpsc::Receiver<T>, tokio::task::AbortHandle) {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let task = tokio::spawn(async move {
        loop {
            let item = {
                let mut guard = src.lock().await;
                match guard.as_mut() {
                    Some(r) => r.recv().await,
                    None => None,
                }
            };
            match item {
                Some(v) => {
                    if tx.send(v).await.is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    });
    (rx, task.abort_handle())
}

/// Прекратить задачи прокси этого шлюза (слушатель закрывается, порт освобождается).
async fn abort_proxy_tasks(state: &AppState, short_id: &str) {
    if let Some(handles) = state.proxy_tasks.lock().await.remove(short_id) {
        for h in handles {
            h.abort();
        }
    }
}

/// Остановить proxy
async fn api_proxy_stop(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    // Прекращаем слушатель и перекачку каналов, затем стираем запись
    abort_proxy_tasks(&state, &short_id).await;
    let removed = state.active_proxies.lock().await.remove(&short_id);

    if removed.is_some() {
        info!("🛑 HTTP Proxy stopped for gateway {}", short_id);

        Json(serde_json::json!({
            "status": "success",
            "message": format!("Proxy to {} stopped", short_id)
        }))
    } else {
        Json(serde_json::json!({
            "status": "error",
            "message": format!("No active proxy found for {}", short_id)
        }))
    }
}

/// Получить статус прокси
async fn api_proxy_status(State(state): State<AppState>) -> impl IntoResponse {
    let proxies = state.active_proxies.lock().await;

    if proxies.is_empty() {
        Json(serde_json::json!({
            "active": false,
            "proxies": []
        }))
    } else {
        let proxy_list: Vec<&ProxyInfo> = proxies.values().collect();
        Json(serde_json::json!({
            "active": true,
            "proxies": proxy_list
        }))
    }
}

// ═══════════════════════════════════════════════════════════════
// SOCKS5 PROXY API HANDLERS
// ═══════════════════════════════════════════════════════════════

/// Запустить SOCKS5 Proxy через gateway
async fn api_socks5_start(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available in standalone mode"
        }));
    }

    let transport = state.transport.as_ref().unwrap();

    // «auto» или «auto-NL» — выходы выбираются сами по каталогу карточек и чередуются
    // «hops» / «hops-NL» — то же, но соединения идут цепочкой через несколько узлов
    // «rules» / «rules-NL» — только нужные сайты (правила `route_rules`) идут через выход (быстро), остальные напрямую
    let rules_mode = short_id == "rules" || short_id.starts_with("rules-");
    let hops_mode = short_id == "hops" || short_id.starts_with("hops-");
    let auto_country: Option<Option<String>> =
        if short_id == "auto" || short_id == "hops" || short_id == "rules" {
            Some(None)
        } else {
            short_id
                .strip_prefix("auto-")
                .or_else(|| short_id.strip_prefix("hops-"))
                .or_else(|| short_id.strip_prefix("rules-"))
                .and_then(crate::network_offers::normalise_country)
                .map(Some)
        };
    let gateway_peer = if auto_country.is_some() {
        None
    } else {
        // Парсим short_id
        let short_id_bytes = match hex::decode(&short_id) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Invalid short ID: {}", e)
                }));
            }
        };

        if short_id_bytes.len() != 8 {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Short ID must be 8 bytes (16 hex chars)"
            }));
        }

        // Ищем пира по short_id
        let peers = transport.get_peers().await;
        match peers
            .iter()
            .find(|p| &p.id.0[..8] == short_id_bytes.as_slice())
        {
            Some(peer) => {
                info!(
                    "🧦 Starting SOCKS5 Proxy through gateway: {} ({})",
                    short_id, peer.addr
                );
                Some(peer.clone())
            }
            None => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer with short ID {} not found", short_id)
                }));
            }
        }
    };

    // Проверяем - уже есть активный прокси?
    if state.active_proxies.lock().await.len() > 0 {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Proxy already running. Stop it first."
        }));
    }

    // Приёмники остаются в состоянии узла; прокси получает каналы-перекачки (после остановки можно запустить снова)
    let channels_present = state.socks5_resp_rx.lock().await.is_some() && state.socks5_tunnel_rx.lock().await.is_some();
    if !channels_present {
        return Json(serde_json::json!({
            "status": "error",
            "message": "SOCKS5 channels not available. May be already in use by CLI?"
        }));
    }
    let (resp_rx, resp_pump) = pump_channel(state.socks5_resp_rx.clone());
    let (tunnel_rx, tunnel_pump) = pump_channel(state.socks5_tunnel_rx.clone());

    // Прокси для браузера — ТОЛЬКО на этом компьютере и со своим паролем узла. Раньше он открывался на внешнем адресе с общим для
    // всех паролем «yandi123»: любой в интернете мог гнать через него свой трафик в чужой выход.
    let socks_port = crate::core::get_config().ports.mobile_gateway;
    let socks_password = match crate::exit_policy::local_proxy_password() {
        Ok(p) => p,
        Err(e) => {
            return Json(
                serde_json::json!({"status": "error", "message": format!("Не удалось создать пароль прокси: {e}")}),
            )
        }
    };

    // Создаём Socks5ProxyServer с авторизацией
    use crate::socks5::{Socks5Config, Socks5ProxyServer};

    let bind_addr = format!("127.0.0.1:{socks_port}");
    let listen_addr = match bind_addr.parse() {
        Ok(addr) => addr,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid bind address {}: {}", bind_addr, e)
            }));
        }
    };

    info!("🧦 SOCKS5 Proxy binding to: {}", listen_addr);

    let socks5_config = Socks5Config {
        listen_addr,         // ✅ Бинд на конкретный внешний IP
        auth_required: true, // ✅ Обязательная авторизация
        username: Some("yandi".to_string()),
        password: Some(socks_password.clone()),
        enable_udp: false, // UDP не поддерживаем в P2P режиме
    };

    let socks5_proxy = Socks5ProxyServer::new(socks5_config, transport.clone());

    let socks5_proxy = socks5_proxy
        .with_response_channel(resp_rx)
        .with_tunnel_data_channel(tunnel_rx);
    let socks5_proxy = match (&gateway_peer, &auto_country) {
        (Some(gw), _) => socks5_proxy.with_exit_node(gw.id.clone()),
        (None, Some(c)) => {
            let pool = crate::exit_select::start_pool(transport.clone(), c.clone());
            crate::exit_select::start_verifier(pool.clone(), listen_addr, socks_password.clone());
            let p = socks5_proxy.with_exit_pool(pool);
            let p = if rules_mode {
                p.with_rules(crate::route_rules::Rules::load())
            } else {
                p
            };
            if hops_mode {
                p.with_hops()
            } else {
                p
            }
        }
        (None, None) => {
            return Json(
                serde_json::json!({"status": "error", "message": "choose a gateway or automatic selection"}),
            )
        }
    };

    // Регистрируем Station
    transport.set_station(socks5_proxy.station.clone()).await;

    // Запускаем прокси в фоне (переменная перемещается в spawn)
    let listener_task = tokio::spawn(async move {
        if let Err(e) = socks5_proxy.run().await {
            eprintln!("❌ SOCKS5 Proxy error: {}", e);
        }
    });
    state
        .proxy_tasks
        .lock()
        .await
        .insert(short_id.clone(), vec![listener_task.abort_handle(), resp_pump, tunnel_pump]);

    // 🚨 ОТПРАВЛЯЕМ 0x34 ПАКЕТ НА GATEWAY ДЛЯ АВТОЗАПУСКА SOCKS5 EXIT NODE! (в режиме auto это делает очередь выходов)
    if let Some(gateway_peer) = &gateway_peer {
        info!(
            "📤 Sending StartSocks5Gateway (0x34) packet to gateway {}",
            short_id
        );
        let cmd_bytes = vec![0x34u8]; // StartSocks5Gateway

        if let Err(e) = transport.send_encrypted(gateway_peer.id, &cmd_bytes).await {
            abort_proxy_tasks(&state, &short_id).await;
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to send StartSocks5Gateway command: {}", e)
            }));
        }
    }

    info!("✅ StartSocks5Gateway command sent to {}", short_id);

    // Добавляем в active_proxies
    let proxy_info = ProxyInfo {
        short_id: short_id.clone(),
        proxy_type: ProxyType::Socks5,
        local_port: socks_port,
        started_at: chrono::Utc::now().to_rfc3339(),
    };

    state
        .active_proxies
        .lock()
        .await
        .insert(short_id.clone(), proxy_info);

    info!(
        "✅ SOCKS5 Proxy started on 127.0.0.1:{} through gateway {}",
        socks_port, short_id
    );

    Json(serde_json::json!({
        "status": "success",
        "message": format!("SOCKS5 proxy started through {}", short_id),
        "listen_addr": format!("127.0.0.1:{socks_port}"),
        "local_port": socks_port,
        "auth_required": true,
        "username": "yandi",
        "password": socks_password,
        "gateway": short_id
    }))
}

/// Прокси через личный шлюз: свой компьютер (дома или за NAT), к которому идут цепочкой, возможно через ретранслятор.
/// Тело: `{"node": "<номер шлюза hex>", "key": "<ключ шлюза hex>", "secret": "<секрет устройств hex>",
///         "relay": {"node": "...", "key": "..."} }` (ретранслятор — по желанию; ключи — из сопряжения).
#[derive(serde::Deserialize)]
struct PersonalHop {
    node: String,
    key: String,
}

#[derive(serde::Deserialize)]
struct PersonalBody {
    node: String,
    key: String,
    secret: String,
    relay: Option<PersonalHop>,
}

async fn api_socks5_personal(
    State(state): State<AppState>,
    Json(b): Json<PersonalBody>,
) -> impl IntoResponse {
    let err = |m: &str| Json(serde_json::json!({"status": "error", "message": m}));
    let Some(transport) = state.transport.as_ref() else {
        return err("P2P transport not available in standalone mode");
    };
    let hop = |h: &PersonalHop| -> Option<crate::hops::PathHop> {
        let id: [u8; 32] = hex::decode(&h.node).ok()?.try_into().ok()?;
        let kb: [u8; 32] = hex::decode(&h.key).ok()?.try_into().ok()?;
        Some(crate::hops::PathHop {
            id,
            key: ed25519_dalek::VerifyingKey::from_bytes(&kb).ok()?,
        })
    };
    let Some(gateway) = hop(&PersonalHop {
        node: b.node.clone(),
        key: b.key.clone(),
    }) else {
        return err("неверный номер или ключ шлюза");
    };
    let mut path = vec![];
    if let Some(r) = &b.relay {
        let Some(r) = hop(r) else {
            return err("неверный номер или ключ ретранслятора");
        };
        path.push(r);
    } else if let Some(r) = {
        // ретранслятор не указан: ищем по объявленной записи шлюза (подпись проверена ключом из сопряжения), берём связанного с нами
        let connected: std::collections::HashSet<String> = transport
            .get_peers()
            .await
            .iter()
            .map(|p| p.id.to_hex())
            .collect();
        let (cards, _) = crate::network_offers::directory_snapshot(None);
        crate::relay_net::lookup_relays(&b.node.to_ascii_lowercase(), &b.key.to_ascii_lowercase())
            .into_iter()
            .filter(|id| connected.contains(id))
            .find_map(|id| {
                cards.iter().find(|c| c.node_id == id).and_then(|c| {
                    hop(&PersonalHop {
                        node: c.node_id.clone(),
                        key: c.key.clone(),
                    })
                })
            })
    } {
        path.push(r);
    }
    path.push(gateway);
    let Some(secret) = hex::decode(&b.secret)
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
    else {
        return err("неверный секрет устройств");
    };
    if state.active_proxies.lock().await.len() > 0 {
        return err("Proxy already running. Stop it first.");
    }
    let socks_port = crate::core::get_config().ports.mobile_gateway;
    let socks_password = match crate::exit_policy::local_proxy_password() {
        Ok(p) => p,
        Err(e) => return err(&format!("Не удалось создать пароль прокси: {e}")),
    };
    use crate::socks5::{Socks5Config, Socks5ProxyServer};
    let Ok(listen_addr) = format!("127.0.0.1:{socks_port}").parse() else {
        return err("bad bind address");
    };
    let cfg = Socks5Config {
        listen_addr,
        auth_required: true,
        username: Some("yandi".to_string()),
        password: Some(socks_password.clone()),
        enable_udp: false,
    };
    let proxy = Socks5ProxyServer::new(cfg, transport.clone()).with_personal(path, secret);
    let personal_task = tokio::spawn(async move {
        if let Err(e) = proxy.run().await {
            eprintln!("❌ SOCKS5 Proxy error: {}", e);
        }
    });
    state.proxy_tasks.lock().await.insert("personal".to_string(), vec![personal_task.abort_handle()]);
    state.active_proxies.lock().await.insert(
        "personal".to_string(),
        ProxyInfo {
            short_id: "personal".to_string(),
            proxy_type: ProxyType::Socks5,
            local_port: socks_port,
            started_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    Json(serde_json::json!({
        "status": "success", "message": "SOCKS5 proxy started through the personal gateway",
        "listen_addr": format!("127.0.0.1:{socks_port}"), "local_port": socks_port,
        "auth_required": true, "username": "yandi", "password": socks_password, "gateway": "personal"
    }))
}

/// Остановить SOCKS5 proxy
async fn api_socks5_stop(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    // Прекращаем слушатель и перекачку каналов, затем стираем запись
    abort_proxy_tasks(&state, &short_id).await;
    let removed = state.active_proxies.lock().await.remove(&short_id);

    // Очищаем active_socks5_proxy
    {
        let mut active = state.active_socks5_proxy.lock().await;
        *active = None;
    }

    if removed.is_some() {
        info!("🛑 SOCKS5 Proxy stopped for gateway {}", short_id);

        Json(serde_json::json!({
            "status": "success",
            "message": format!("SOCKS5 proxy to {} stopped", short_id)
        }))
    } else {
        Json(serde_json::json!({
            "status": "error",
            "message": format!("No active SOCKS5 proxy found for {}", short_id)
        }))
    }
}

/// Получить статус SOCKS5 прокси
async fn api_socks5_status(State(state): State<AppState>) -> impl IntoResponse {
    let proxies = state.active_proxies.lock().await;
    let external_ip = &state.node_info.external_ip;

    // Фильтруем только SOCKS5 прокси
    let socks5_proxies: Vec<&ProxyInfo> = proxies
        .values()
        .filter(|p| matches!(p.proxy_type, ProxyType::Socks5))
        .collect();

    if socks5_proxies.is_empty() {
        Json(serde_json::json!({
            "active": false,
            "proxies": [],
            "auth_required": true,
            "username": "yandi",
            "listen_addr": format!("127.0.0.1:{}", crate::core::get_config().ports.mobile_gateway)
        }))
    } else {
        Json(serde_json::json!({
            "active": true,
            "proxies": socks5_proxies,
            "auth_required": true,
            "username": "yandi",
            "listen_addr": format!("127.0.0.1:{}", crate::core::get_config().ports.mobile_gateway)
        }))
    }
}

/// Ретрансляция: узел сам служит ретранслятором по запросам других узлов (если не запрещено) и сам пользуется ретрансляторами,
/// когда до него нельзя достучаться. Здесь — состояние и один переключатель «служить ретранслятором для других».
async fn api_network_relay(State(state): State<AppState>) -> impl IntoResponse {
    let mut j = crate::relay_net::status_json();
    if let Some(t) = &state.transport {
        let st = t.relay_manager.lock().await.get_stats();
        j["forwarding"] = serde_json::json!({
            "active_sessions": st.active_sessions,
            "bytes": st.total_bytes_forwarded,
            "packets": st.total_packets_forwarded,
        });
    }
    Json(j)
}

#[derive(Deserialize)]
struct RelaySetRequest {
    enabled: bool,
}

async fn api_network_relay_set(Json(b): Json<RelaySetRequest>) -> impl IntoResponse {
    match crate::relay_net::set_relay_enabled(b.enabled) {
        Ok(()) => Json(serde_json::json!({"status": "success", "relay_enabled": b.enabled})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("не удалось сохранить: {e}")})),
    }
}

// ═══════════════════════════════════════════════════════════════
// GATEWAY API HANDLERS
// ═══════════════════════════════════════════════════════════════

/// Запустить Gateway mode (на этой ноде)
async fn api_gateway_start() -> impl IntoResponse {
    // честно: включение режима шлюза из страницы не реализовано (раньше отвечали «успех» и ничего не делали)
    (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({"status": "error", "message": "Режим шлюза со страницы пока не включается (не реализовано)"})))
}

/// Остановить Gateway mode
async fn api_gateway_stop() -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({"status": "error", "message": "Режим шлюза со страницы пока не выключается (не реализовано)"})))
}

/// Телефоны владельца, сопряжённые с этим узлом, — собеседники веб-чата (переписка шифруется между узлом и телефоном, см. mobile_self).
/// `short_id` у них — полный номер устройства (64 знака): по нему работают история и отправка.
/// Устройства узла, которые не в контактах: новые, скрытые и заблокированные. Для них — «добавить в контакты», «скрыть», «заблокировать».
async fn mobile_other_devices_handler() -> impl IntoResponse {
    crate::mobile_self::init_phone_status();
    let list: Vec<serde_json::Value> = crate::mobile_api::devices_overview()
        .into_iter()
        .filter_map(|d| {
            let full = d["peer_id"].as_str()?.to_string();
            let status = if crate::mobile_self::is_blocked_phone(&full) {
                "blocked"
            } else if crate::mobile_self::is_hidden_phone(&full) {
                "hidden"
            } else if crate::mobile_self::is_saved_phone(&full) {
                return None; // это контакт
            } else {
                "new"
            };
            Some(serde_json::json!({"peer_id": full, "name": d["name"], "online": d["online"], "status": status}))
        })
        .collect();
    Json(serde_json::json!({"devices": list}))
}

#[derive(Deserialize)]
struct PhoneActionReq {
    peer: String,
    action: String,
}

async fn mobile_phone_action_handler(Json(b): Json<PhoneActionReq>) -> Response {
    match crate::mobile_self::phone_action(&b.peer, &b.action) {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

fn append_phone_contacts(data: &mut serde_json::Value) {
    crate::mobile_self::init_phone_status();
    if let Some(arr) = data["contacts"].as_array_mut() {
        for d in crate::mobile_api::devices_overview() {
            let Some(full) = d["peer_id"].as_str() else { continue };
            if crate::mobile_self::is_hidden_phone(full) || crate::mobile_self::is_blocked_phone(full) || !crate::mobile_self::is_saved_phone(full) {
                continue; // «Удалить» на странице — спрятан; заблокированные и не добавленные в контакты — в списке «Другие устройства узла»
            }
            // глобальное — то, что человек задал себе (ник и картинка его профиля); локальное — как я его назвал и какую картинку поставил.
            // Локальное главнее; без него — его собственный ник; без ника — имя устройства.
            let (nick, avatar_hash) = crate::device_profile::public(full);
            let local_name = crate::local_overrides::name(full).unwrap_or_default();
            let device_name = d["name"].as_str().unwrap_or("Телефон");
            let name = crate::local_overrides::display_name(&local_name, &nick, &format!("📱 {device_name}"));
            arr.push(serde_json::json!({
                "id": format!("phone-{}", &full[..16]),
                "name": name,
                "local_name": local_name,
                "global_name": nick,
                "short_id": full,
                "online": d["online"],
                "is_phone": true,
                // картинка именно этого телефона (локальная, иначе его собственная), а не картинка ПК
                "avatar_id": full,
                "avatar_local": crate::local_overrides::has_avatar(full),
                "avatar_global": !avatar_hash.is_empty(),
            }));
        }
    }
}

async fn api_contacts_get(State(state): State<AppState>) -> impl IntoResponse {
    // Загружаем из contacts.json
    let contacts_path_owned = crate::util::data_dir::data_dir().join("contacts.json").display().to_string();
    let contacts_path = contacts_path_owned.as_str();

    // Если файл не существует, возвращаем пустой список
    if !std::path::Path::new(contacts_path).exists() {
        let mut data = serde_json::json!({ "contacts": [] });
        append_phone_contacts(&mut data);
        return Json(data);
    }

    // Читаем файл
    match tokio::fs::read_to_string(contacts_path).await {
        Ok(content) => {
            // Парсим JSON
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(mut data) => {
                    // Получаем список онлайн пиров из transport
                    let mut online_peers = std::collections::HashSet::new();
                    if let Some(transport) = &state.transport {
                        let peers = transport.get_peers().await;
                        for peer in peers {
                            let short_id = hex::encode(&peer.id.0[..8]);
                            online_peers.insert(short_id);
                        }
                    }
                    // Обновляем поле online для каждого контакта
                    if let Some(contacts_arr) = data["contacts"].as_array_mut() {
                        for contact in contacts_arr.iter_mut() {
                            if let Some(short_id) = contact["short_id"].as_str().map(str::to_string) {
                                let is_online = online_peers.contains(&short_id);
                                contact["online"] = serde_json::json!(is_online);
                                // локальная картинка есть — интерфейс покажет «Как он сам» (снять её)
                                contact["avatar_local"] = serde_json::json!(crate::local_overrides::has_avatar(&short_id));
                                if let Some(n) = crate::profile_sync::peer_nick(&short_id) {
                                    contact["nick"] = serde_json::json!(n);
                                }
                            }
                        }
                    }
                    append_phone_contacts(&mut data);
                    Json(data)
                }
                Err(_) => {
                    // Если ошибка парсинга, возвращаем пустой список
                    Json(serde_json::json!({
                        "contacts": []
                    }))
                }
            }
        }
        Err(_) => Json(serde_json::json!({
            "contacts": []
        })),
    }
}

#[derive(Deserialize)]
struct ContactRequest {
    name: String,
    short_id: String,
    #[serde(default)]
    id: Option<String>,
}

async fn api_contacts_post(
    State(_state): State<AppState>,
    axum::extract::Json(payload): axum::extract::Json<ContactRequest>,
) -> impl IntoResponse {
    let contacts_path_owned = crate::util::data_dir::data_dir().join("contacts.json").display().to_string();
    let contacts_path = contacts_path_owned.as_str();
    // телефон владельца в contacts.json не хранится: «Переименовать» задаёт ему локальное имя (его ник у него самого не меняется)
    if payload.id.as_deref().is_some_and(|i| i.starts_with("phone-")) {
        return match crate::local_overrides::set(&payload.short_id, Some(&payload.name), None) {
            Ok(()) => Json(serde_json::json!({"status": "success", "message": "Contact saved"})),
            Err(e) => Json(serde_json::json!({"status": "error", "message": e})),
        };
    }
    let _guard = CONTACTS_LOCK.lock().await;
    let mut contacts = match load_contacts_for_edit(contacts_path).await {
        Ok(c) => c,
        Err(e) => return Json(serde_json::json!({"status": "error", "message": e})),
    };

    // Проверяем, это новый контакт или редактирование
    if let Some(id) = &payload.id {
        // Редактирование существующего контакта
        if let Some(contact) = contacts
            .iter_mut()
            .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(id))
        {
            contact["name"] = serde_json::json!(payload.name);
            contact["short_id"] = serde_json::json!(payload.short_id);
        }
    } else {
        // Добавление нового контакта
        let new_contact = serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "name": payload.name,
            "short_id": payload.short_id,
            "online": false // TODO: проверить онлайн статус через P2P
        });
        contacts.push(new_contact);
    }

    if let Err(e) = save_contacts(contacts_path, &contacts).await {
        return Json(serde_json::json!({"status": "error", "message": e}));
    }

    Json(serde_json::json!({
        "status": "success",
        "message": "Contact saved"
    }))
}

#[derive(Deserialize)]
struct LocalOverrideRequest {
    /// номер человека (телефон — 64 знака, контакт — 16)
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// PNG/JPEG/GIF в base64; пустая строка — убрать свою картинку этому человеку
    #[serde(default)]
    avatar_b64: Option<String>,
}

/// Локальные имя и картинка контакта (видны только владельцу узла; человеку ничего не отправляется).
async fn api_contacts_local(axum::extract::Json(b): axum::extract::Json<LocalOverrideRequest>) -> impl IntoResponse {
    match crate::local_overrides::set(&b.id, b.name.as_deref(), b.avatar_b64.as_deref()) {
        Ok(()) => Json(serde_json::json!({"status": "success"})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"status": "error", "message": e}))).into_response(),
    }
}

#[derive(Deserialize)]
struct DeleteContactRequest {
    id: String,
}

async fn api_contacts_delete(
    axum::extract::Json(payload): axum::extract::Json<DeleteContactRequest>,
) -> impl IntoResponse {
    let contacts_path_owned = crate::util::data_dir::data_dir().join("contacts.json").display().to_string();
    let contacts_path = contacts_path_owned.as_str();

    // телефон владельца: он не в contacts.json (список строит узел из сопряжённых устройств) — прячем из чата
    if let Some(prefix) = payload.id.strip_prefix("phone-") {
        crate::mobile_self::hide_phone(prefix);
        return Json(serde_json::json!({"status": "success", "message": "Contact deleted"}));
    }

    let _guard = CONTACTS_LOCK.lock().await;
    let mut contacts = match load_contacts_for_edit(contacts_path).await {
        Ok(c) => c,
        Err(e) => return Json(serde_json::json!({"status": "error", "message": e})),
    };

    // локальные имя и картинка удаляемого контакта стираются вместе с ним
    for c in contacts.iter().filter(|c| c.get("id").and_then(|v| v.as_str()) == Some(payload.id.as_str())) {
        if let Some(sid) = c.get("short_id").and_then(|v| v.as_str()) {
            crate::local_overrides::forget(sid);
        }
    }
    // Удаляем контакт (id в файле бывает строкой или числом)
    contacts.retain(|c| match c.get("id") {
        Some(serde_json::Value::String(s)) => s != &payload.id,
        Some(v) => v.to_string() != payload.id,
        None => true,
    });

    if let Err(e) = save_contacts(contacts_path, &contacts).await {
        return Json(serde_json::json!({"status": "error", "message": e}));
    }

    Json(serde_json::json!({
        "status": "success",
        "message": "Contact deleted"
    }))
}

/// Export contacts as a JSON file download (only short_id and name fields)
async fn api_contacts_export() -> impl IntoResponse {
    let contacts_path_owned = crate::util::data_dir::data_dir().join("contacts.json").display().to_string();
    let contacts_path = contacts_path_owned.as_str();

    let contacts: Vec<serde_json::Value> = if std::path::Path::new(contacts_path).exists() {
        match tokio::fs::read_to_string(contacts_path).await {
            Ok(content) => serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|v| v.get("contacts").and_then(|c| c.as_array()).cloned())
                .unwrap_or_default()
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "short_id": c.get("short_id").and_then(|v| v.as_str()).unwrap_or(""),
                        "name":     c.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let payload = serde_json::to_string_pretty(&serde_json::json!({ "contacts": contacts }))
        .unwrap_or_else(|_| r#"{"contacts":[]}"#.to_string());

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json; charset=utf-8")
        .header(
            "Content-Disposition",
            "attachment; filename=\"yandi_contacts.json\"",
        )
        .body(payload)
        .unwrap()
}

async fn api_gateways_get(State(state): State<AppState>) -> impl IntoResponse {
    // Загружаем из gateways.json
    let gateways_path_owned = crate::util::data_dir::data_dir().join("gateways.json").display().to_string();
    let gateways_path = gateways_path_owned.as_str();

    // Если файл не существует, возвращаем пустой список
    if !std::path::Path::new(gateways_path).exists() {
        return Json(serde_json::json!({
            "gateways": []
        }));
    }

    // Читаем файл
    let gateways_json = match tokio::fs::read_to_string(gateways_path).await {
        Ok(content) => content,
        Err(_) => {
            return Json(serde_json::json!({
                "gateways": []
            }));
        }
    };

    // Парсим JSON
    let mut data: serde_json::Value = match serde_json::from_str(&gateways_json) {
        Ok(d) => d,
        Err(_) => {
            return Json(serde_json::json!({
                "gateways": []
            }));
        }
    };

    // Получаем список peers из P2P transport
    let mut online_peers = std::collections::HashSet::new();
    if let Some(transport) = &state.transport {
        let peers = transport.get_peers().await;
        for peer in peers {
            let short_id = hex::encode(&peer.id.0[..8]);
            online_peers.insert(short_id);
        }
    }

    // Получаем статус активного прокси
    let mut active_proxy_short_id = None;
    let proxies = state.active_proxies.lock().await;
    if proxies.len() > 0 {
        if let Some(proxy) = proxies.values().next() {
            active_proxy_short_id = Some(proxy.short_id.clone());
        }
    }

    // Обновляем статус connected для каждого gateway
    if let Some(gateways_arr) = data["gateways"].as_array_mut() {
        for gw in gateways_arr.iter_mut() {
            if let Some(short_id) = gw["short_id"].as_str() {
                // Клонируем short_id чтобы избежать удержания immutable borrow
                let short_id = short_id.to_string();

                // Проверяем онлайн-статус
                let is_online = online_peers.contains(&short_id);
                gw["connected"] = serde_json::json!(is_online);

                // Проверяем активен ли прокси для этого gateway
                let has_active_proxy = active_proxy_short_id.as_deref() == Some(short_id.as_str());
                gw["proxy_active"] = serde_json::json!(has_active_proxy);
            }
        }
    }

    Json(data)
}

#[derive(Deserialize)]
struct GatewayRequest {
    name: String,
    short_id: String,
    country: String,
    #[serde(default)]
    id: Option<String>,
}

/// Прочитать список шлюзов строго: нет файла — пусто; файл есть, но не читается или не разбирается — ошибка (его не затираем).
fn load_gateways(path: &str) -> Result<Vec<serde_json::Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let data: serde_json::Value = serde_json::from_str(&content).map_err(|e| format!("gateways.json повреждён: {e}"))?;
            // структура проверяется: «gateways» не массив — повреждение, а не пустой список (иначе следующая запись затёрла бы документ)
            // корень обязан быть объектом с полем gateways; иначе (массив, чужое имя поля) это не «пустой список», а чужой документ
            match data.get("gateways") {
                None => Err("gateways.json повреждён: нет поля gateways".to_string()),
                Some(v) => match v.as_array() {
                    Some(a) if a.iter().all(|x| x.is_object()) => Ok(a.clone()),
                    _ => Err("gateways.json повреждён: поле gateways должно быть списком объектов".to_string()),
                },
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("gateways.json не читается: {e}")),
    }
}

/// Изменить список шлюзов: один замок на чтение-изменение-запись (два окна не затирают друг друга), строгое чтение, атомарная запись.
fn update_gateways(path: &str, f: impl FnOnce(&mut Vec<serde_json::Value>)) -> Result<(), String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut gateways = load_gateways(path)?;
    f(&mut gateways);
    let content = serde_json::to_string_pretty(&serde_json::json!({ "gateways": gateways })).map_err(|e| e.to_string())?;
    crate::util::private_file::write_private(std::path::Path::new(path), content.as_bytes()).map_err(|e| format!("не удалось записать gateways.json: {e}"))
}

async fn api_gateways_post(
    axum::extract::Json(payload): axum::extract::Json<GatewayRequest>,
) -> axum::response::Response {
    let r = update_gateways(&crate::util::data_dir::data_dir().join("gateways.json").display().to_string(), |gateways| {
        // Проверяем, это новый шлюз или редактирование
        if let Some(id) = &payload.id {
            // Редактирование существующего шлюза
            if let Some(gateway) = gateways.iter_mut().find(|g| g.get("id").and_then(|v| v.as_str()) == Some(id)) {
                gateway["name"] = serde_json::json!(payload.name);
                gateway["short_id"] = serde_json::json!(payload.short_id);
                gateway["country"] = serde_json::json!(payload.country);
            }
        } else {
            // Добавление нового шлюза
            gateways.push(serde_json::json!({
                "id": uuid::Uuid::new_v4().to_string(),
                "name": payload.name,
                "short_id": payload.short_id,
                "country": payload.country,
                "connected": false
            }));
        }
    });
    match r {
        Ok(()) => Json(serde_json::json!({"status": "success", "message": "Gateway saved"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"status": "error", "message": e}))).into_response(),
    }
}

async fn api_gateways_delete(
    axum::extract::Json(payload): axum::extract::Json<DeleteContactRequest>,
) -> axum::response::Response {
    let r = update_gateways(&crate::util::data_dir::data_dir().join("gateways.json").display().to_string(), |gateways| {
        gateways.retain(|g| g.get("id").and_then(|v| v.as_str()) != Some(payload.id.as_str()));
    });
    match r {
        Ok(()) => Json(serde_json::json!({"status": "success", "message": "Gateway deleted"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"status": "error", "message": e}))).into_response(),
    }
}

async fn api_settings_get() -> impl IntoResponse {
    // Загружаем конфигурацию YANDI
    let config = crate::get_config();

    Json(serde_json::json!({
        "node_mode": {
            "p2p_enabled": true,
            "gateway_enabled": true
        },
        "gateway": {
            "auto_start": true,
            "multi_port": false,
            "max_clients": 11
        },
        "network": {
            "discovery_port": config.ports.discovery,
            "data_port": config.ports.data,
            "mobile_gateway": config.ports.mobile_gateway,
            "mobile_p2p": config.ports.mobile_p2p,
            "http_proxy": config.ports.http_proxy,
            "web_ui": config.ports.web_ui
        },
        "server": {
            "bind_address": config.server.bind_address,
            "log_level": config.server.log_level
        }
    }))
}

async fn api_settings_put(
    axum::extract::Json(settings): axum::extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    use crate::update_config;

    // Получаем текущую конфигурацию
    let mut config = crate::get_config();

    // Обновляем порты если указаны (только 1..=65535; раньше число молча обрезалось до 16 бит)
    let port_of = |v: &serde_json::Value| {
        v.as_u64()
            .filter(|p| (1..=65535).contains(p))
            .map(|p| p as u16)
    };
    if let Some(network) = settings.get("network") {
        for (key, slot) in [
            ("discovery_port", &mut config.ports.discovery),
            ("data_port", &mut config.ports.data),
            ("mobile_gateway", &mut config.ports.mobile_gateway),
            ("mobile_p2p", &mut config.ports.mobile_p2p),
            ("http_proxy", &mut config.ports.http_proxy),
            ("web_ui", &mut config.ports.web_ui),
        ] {
            if let Some(v) = network.get(key) {
                match port_of(v) {
                    Some(p) => *slot = p,
                    None => {
                        return Json(
                            serde_json::json!({"status": "error", "message": format!("{key}: port must be a number from 1 to 65535")}),
                        )
                    }
                }
            }
        }
    }

    // Обновляем серверные настройки если указаны (адрес — настоящий IP, уровень журнала — из известных)
    if let Some(server) = settings.get("server") {
        if let Some(bind_address) = server.get("bind_address").and_then(|v| v.as_str()) {
            if bind_address.parse::<std::net::IpAddr>().is_err() {
                return Json(
                    serde_json::json!({"status": "error", "message": "bind_address must be an IP address"}),
                );
            }
            config.server.bind_address = bind_address.to_string();
        }
        if let Some(log_level) = server.get("log_level").and_then(|v| v.as_str()) {
            if !["error", "warn", "info", "debug", "trace"].contains(&log_level) {
                return Json(
                    serde_json::json!({"status": "error", "message": "log_level must be error, warn, info, debug or trace"}),
                );
            }
            config.server.log_level = log_level.to_string();
        }
    }

    // Сохраняем конфигурацию
    match update_config(config) {
        Ok(_) => {
            info!("✅ Settings updated via Web UI");

            Json(serde_json::json!({
                "status": "success",
                "message": "Settings saved. Restart node to apply port changes.",
                "requires_restart": true
            }))
        }
        Err(e) => {
            error!("❌ Failed to save settings: {}", e);

            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to save settings: {}", e)
            }))
        }
    }
}

/// Остановить P2P ноду
async fn api_node_stop() -> impl IntoResponse {
    // честно: остановить транспорт из страницы узел пока не умеет (раньше кнопка лишь меняла флаг и показывала «офлайн», а соединения жили)
    (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({"status": "error", "message": "Остановить узел со страницы пока нельзя: соединения не закрываются. Узел останавливается службой: sudo systemctl stop yandi-node"})))
}

/// Запустить P2P ноду
async fn api_node_start() -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({"status": "error", "message": "Запуск со страницы не поддерживается: узел запускается службой: sudo systemctl start yandi-node"})))
}

// === Chat API Endpoints ===

/// Отправить сообщение
async fn api_chat_send(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Chat manager not initialized",
        )
    });

    if let Err(_) = chat_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Chat manager not available"
        }));
    }

    let chat_manager = chat_manager.unwrap();

    // телефон владельца (сопряжён с этим узлом): сообщение шифруется ключом телефона и уходит в его очередь, не в сеть узлов
    if crate::mobile_api::is_device_peer(&peer_id) {
        if let Some(att) = req.attachment {
            // файл уже загружен страницей (send-chunk положил его в uploads): узел шифрует его для телефона
            let Some(file_ref) = att.file_ref else {
                return Json(serde_json::json!({"status": "error", "message": "Телефону файл отправляется только загрузкой по кускам"}));
            };
            let Some(path) = resolve_local_file_path(&file_ref.file_id, &att.filename) else {
                return Json(serde_json::json!({"status": "error", "message": "Загруженный файл не найден на узле"}));
            };
            let attachment = crate::communication::FileAttachment {
                filename: att.filename,
                size: att.size,
                mime_type: att.mime_type,
                data: None,
                file_ref: Some(crate::communication::FileReference { file_id: file_ref.file_id, total_chunks: file_ref.total_chunks, local_name: file_ref.local_name }),
            };
            return match crate::mobile_api::pc_send_file_to_device(&peer_id, &path, attachment, req.text).await {
                Ok(msg) => Json(serde_json::json!({"status": "success", "message": "Message sent", "msg_id": hex::encode(&msg.msg_id.0[..8]), "timestamp": msg.timestamp})),
                Err(e) => Json(serde_json::json!({"status": "error", "message": e})),
            };
        }
        let reply = req.reply_to.map(|r| crate::communication::ReplyRef { msg_id: r.msg_id, text: r.text });
        return match crate::mobile_api::pc_send_to_device_reply(&peer_id, req.text, reply).await {
            Ok(msg) => Json(serde_json::json!({"status": "success", "message": "Message sent", "msg_id": hex::encode(&msg.msg_id.0[..8]), "timestamp": msg.timestamp})),
            Err(e) => Json(serde_json::json!({"status": "error", "message": e})),
        };
    }

    // Попытаться распарсить peer_id (поддерживает полный HashId или short_id)
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        // Полный HashId (64 hex символа)
        id
    } else {
        // Возможно это short_id (16 hex символов) - найти через dedicated p2p transport
        if let Some(transport) = &state.p2p_transport {
            if let Some(id) = transport.find_peer_by_short_id(&peer_id).await {
                id
            } else {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer not found: {}", peer_id)
                }));
            }
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // Подготовить attachment если есть
    let attachment = req
        .attachment
        .map(|att| crate::communication::FileAttachment {
            filename: att.filename,
            size: att.size,
            mime_type: att.mime_type,
            data: att.data,
            file_ref: att
                .file_ref
                .map(|file_ref| crate::communication::FileReference {
                    file_id: file_ref.file_id,
                    total_chunks: file_ref.total_chunks,
                    local_name: file_ref.local_name,
                }),
        });

    // Отправить сообщение
    let reply = req.reply_to.map(|r| crate::communication::ReplyRef { msg_id: r.msg_id, text: r.text });
    match chat_manager
        .send_message_full(peer_hash, req.text, attachment, reply)
        .await
    {
        Ok(msg) => Json(serde_json::json!({
            "status": "success",
            "message": "Message sent",
            "msg_id": hex::encode(&msg.msg_id.0[..8]),
            "timestamp": msg.timestamp
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to send message: {}", e)
        })),
    }
}

#[derive(Deserialize)]
struct ReactRequest {
    /// номер сообщения (hex, 64 знака — как в истории)
    msg_id: String,
    /// смайл из набора; пустой — снять
    emoji: String,
}

/// Поставить или снять смайл на сообщение личного чата (повтор того же смайла снимает).
async fn api_chat_react(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    Json(req): Json<ReactRequest>,
) -> impl IntoResponse {
    let err = |m: &str| Json(serde_json::json!({"status": "error", "message": m}));
    if !(req.emoji.is_empty() || crate::chat_reactions::SET.contains(&req.emoji.as_str())) {
        return err("такого смайла нет");
    }
    if req.msg_id.len() != 64 || !req.msg_id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return err("неверный номер сообщения");
    }
    // собеседник: полный номер или короткий
    let peer_hash = match crate::util::HashId::from_hex(&peer_id) {
        Ok(id) => id,
        Err(_) => match &state.p2p_transport {
            Some(t) => match t.find_peer_by_short_id(&peer_id).await {
                Some(id) => id,
                None => return err("собеседник не найден"),
            },
            None => return err("сеть недоступна"),
        },
    };
    let short = hex::encode(&peer_hash.0[..8]);
    let current = crate::chat_reactions::of_peer(&short).get(&req.msg_id).map(|r| r.0.clone()).unwrap_or_default();
    let next = if current == req.emoji { String::new() } else { req.emoji.clone() };
    let sent = if crate::mobile_api::is_device_peer(&peer_id) || crate::mobile_api::is_device_peer(&hex::encode(peer_hash.0)) {
        crate::mobile_api::pc_react_device(&hex::encode(peer_hash.0), &req.msg_id, &next)
    } else {
        let chat = state.chat_manager.lock().await;
        match chat.as_ref() {
            Some(c) => c
                .send_service(peer_hash, crate::chat_reactions::envelope(&req.msg_id, &next))
                .await
                .map_err(|e| format!("не удалось отправить: {e}")),
            None => Err("чат недоступен".to_string()),
        }
    };
    match sent {
        Ok(()) => {
            crate::chat_reactions::set_me(&short, &req.msg_id, &next);
            Json(serde_json::json!({"status": "success", "emoji": next}))
        }
        Err(e) => err(&e),
    }
}

/// Загрузить историю чата
async fn api_chat_history(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Chat manager not initialized",
        )
    });

    if let Err(_) = chat_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Chat manager not available"
        }));
    }

    let chat_manager = chat_manager.unwrap();

    // Попытаться распарсить peer_id (поддерживает полный HashId или short_id)
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        // Полный HashId (64 hex символа)
        id
    } else {
        // Возможно это short_id (16 hex символов) - найти через dedicated p2p transport
        if let Some(transport) = &state.p2p_transport {
            if let Some(id) = transport.find_peer_by_short_id(&peer_id).await {
                id
            } else {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer not found: {}", peer_id)
                }));
            }
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // переписка с телефоном владельца открыта на странице — «прочитано» по его сообщениям
    if crate::mobile_api::is_device_peer(&peer_id) {
        crate::mobile_api::pc_mark_read(&peer_id);
    }
    // println!("🔍 Loading history for short_id: {}, peer_hash: {}", peer_id, hex::encode(&peer_hash.0[..8]));
    // Загрузить историю (последние 100 сообщений)
    let reactions = crate::chat_reactions::of_peer(&hex::encode(&peer_hash.0[..8]));
    match chat_manager.load_history(&peer_hash, 100) {
        Ok(messages) => {
            // Конвертировать HashId в hex строку для JSON (полные 32 байта)
            let messages_json: Vec<serde_json::Value> = messages
                .into_iter()
                .filter(|msg| !crate::profile_sync::is_service(&msg.text)) // служебные сообщения профиля в переписке не показываем
                .map(|msg| {
                    serde_json::json!({
                        "msg_id": hex::encode(&msg.msg_id.0),  // Полный HashId (32 байта)
                        "from": hex::encode(&msg.from.0),      // Полный HashId (32 байта)
                        "to": hex::encode(&msg.to.0),          // Полный HashId (32 байта)
                        "timestamp": msg.timestamp,
                        "text": msg.text,
                        "encrypted": msg.encrypted,
                        "status": format!("{:?}", msg.status),
                        "edited": msg.edited,
                        "edit_timestamp": msg.edit_timestamp,
                        "attachment": msg.attachment,
                        "reply_to": msg.reply_to,
                        "reactions": reactions.get(&hex::encode(&msg.msg_id.0)).map(|(me, peer)| serde_json::json!({"me": me, "peer": peer}))
                    })
                })
                .collect();

            Json(serde_json::json!({
                "status": "success",
                "peer_id": peer_id,
                "messages": messages_json
            }))
        }
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to load history: {}", e)
        })),
    }
}

/// Очистить историю чата
async fn api_chat_clear(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Chat manager not initialized",
        )
    });

    if let Err(_) = chat_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Chat manager not available"
        }));
    }

    let chat_manager = chat_manager.unwrap();

    // Попытаться распарсить peer_id (поддерживает полный HashId или short_id)
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        // Полный HashId (64 hex символа)
        id
    } else {
        // Возможно это short_id (16 hex символов) - найти через dedicated p2p transport
        if let Some(transport) = &state.p2p_transport {
            if let Some(id) = transport.find_peer_by_short_id(&peer_id).await {
                id
            } else {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer not found: {}", peer_id)
                }));
            }
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // Очистить историю
    match chat_manager.clear_history(&peer_hash).await {
        Ok(_) => Json(serde_json::json!({
            "status": "success",
            "message": "Chat history cleared"
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to clear history: {}", e)
        })),
    }
}

/// Редактировать сообщение
async fn api_chat_edit(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    Json(req): Json<EditMessageRequest>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Chat manager not initialized",
        )
    });

    if let Err(_) = chat_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Chat manager not available"
        }));
    }

    let chat_manager = chat_manager.unwrap();

    // Попытаться распарсить peer_id
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        id
    } else {
        if let Some(transport) = &state.p2p_transport {
            if let Some(id) = transport.find_peer_by_short_id(&peer_id).await {
                id
            } else {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer not found: {}", peer_id)
                }));
            }
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // Распарсить msg_id
    let msg_id = match crate::util::HashId::from_hex(&req.msg_id) {
        Ok(id) => id,
        Err(_) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Invalid msg_id"
            }));
        }
    };

    // телефон владельца: правка уходит и ему (конверт правки), а не только в историю узла
    if crate::mobile_api::is_device_peer(&peer_id) {
        return match crate::mobile_api::pc_edit_for_device(&peer_id, &msg_id, req.text) {
            Ok(()) => Json(serde_json::json!({"status": "success", "message": "Message edited"})),
            Err(e) => Json(serde_json::json!({"status": "error", "message": e})),
        };
    }
    // Редактировать сообщение
    match chat_manager.edit_message(&peer_hash, &msg_id, req.text) {
        Ok(_) => Json(serde_json::json!({
            "status": "success",
            "message": "Message edited"
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to edit message: {}", e)
        })),
    }
}

/// Удалить сообщение
async fn api_chat_delete(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    Json(req): Json<DeleteMessageRequest>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Chat manager not initialized",
        )
    });

    if let Err(_) = chat_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Chat manager not available"
        }));
    }

    let chat_manager = chat_manager.unwrap();

    // Попытаться распарсить peer_id
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        id
    } else {
        if let Some(transport) = &state.p2p_transport {
            if let Some(id) = transport.find_peer_by_short_id(&peer_id).await {
                id
            } else {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer not found: {}", peer_id)
                }));
            }
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // Распарсить msg_id
    let msg_id = match crate::util::HashId::from_hex(&req.msg_id) {
        Ok(id) => id,
        Err(_) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Invalid msg_id"
            }));
        }
    };

    // Удалить сообщение
    if req.for_everyone {
        // Удалить для всех (отправить запрос на удаление)
        match chat_manager
            .delete_message_for_everyone(&peer_hash, &msg_id)
            .await
        {
            Ok(_) => Json(serde_json::json!({
                "status": "success",
                "message": "Message deleted for everyone"
            })),
            Err(e) => Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to delete message: {}", e)
            })),
        }
    } else {
        // Удалить только у себя
        match chat_manager.delete_message_local(&peer_hash, &msg_id).await {
            Ok(_) => Json(serde_json::json!({
                "status": "success",
                "message": "Message deleted locally"
            })),
            Err(e) => Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to delete message: {}", e)
            })),
        }
    }
}

/// Пути с расшифрованным содержимым владельца: при запертой среде отвечают 423 (служебная доставка идёт внутри узла и не страдает).
fn is_owner_content_path(path: &str) -> bool {
    ["/api/groups", "/api/files/content"]
        .iter()
        .any(|p| path == *p || path.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
}

/// Звонки: начать, принять, входящие, список и сокет сигнализации требуют открытой среды; завершить и отклонить можно всегда (очистка).
fn is_locked_media_path(path: &str) -> bool {
    // и звонки через /api/media/, и звонки телефонов владельца: события (расшифрованные сигналы), TURN и запись звонка в чат при запертой среде закрыты;
    // /api/mobile/call/signal остаётся доступным — через него отправляется «положить трубку»
    (path.starts_with("/api/media/") && !path.ends_with("/end") && !path.ends_with("/reject"))
        || matches!(path, "/api/mobile/call/events" | "/api/mobile/call/turn" | "/api/mobile/call/log")
}

/// Админские маршруты без сеанса администратора: 403 `admin_required` (интерфейс просит админ-пароль).
async fn admin_guard(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let token = req.headers().get(crate::web::admin::HEADER).and_then(|v| v.to_str().ok());
    if !crate::web::admin::allowed(req.method(), req.uri().path(), token) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "admin_required", "message": "нужен пароль администратора"}))).into_response();
    }
    next.run(req).await
}

#[derive(Deserialize)]
struct AdminSessionBody {
    password: String,
}

/// Открыть админ-панель: пароль проверяется тем же счётчиком попыток, что и вход (пять неверных закрывают вход паролем).
async fn api_admin_session_post(State(state): State<AppState>, Json(b): Json<AdminSessionBody>) -> Response {
    match crate::web::auth::verify_admin_counted(&b.password) {
        Ok(true) => {
            // токен знает только вошедший клиент; он нужен на каждом админском запросе
            let token = crate::web::admin::start();
            Json(serde_json::json!({"ok": true, "token": token, "ttl_secs": crate::web::admin::TTL.as_secs()})).into_response()
        }
        Ok(false) => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Неверный пароль администратора"}))).into_response(),
        Err(e) => counted_login_error(e),
    }
}

#[derive(Deserialize)]
struct AdminPasswordBody {
    current: String,
    new: String,
    #[serde(default)]
    repeat: String,
}

/// Назначить или сменить отдельный админ-пароль. Нужен действующий сеанс (маршрут админский) и текущий админ-пароль ещё раз.
async fn api_admin_password_post(Json(b): Json<AdminPasswordBody>) -> Response {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": m}))).into_response();
    if b.new != b.repeat {
        return bad("Пароли не совпадают");
    }
    if let Err(e) = crate::web::auth::check_new_password(&b.new) {
        return bad(&e);
    }
    match crate::web::auth::verify_admin_counted(&b.current) {
        Ok(true) => {}
        Ok(false) => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Неверный текущий пароль администратора"}))).into_response(),
        Err(e) => return counted_login_error(e),
    }
    let new = b.new.clone();
    match tokio::task::spawn_blocking(move || crate::web::admin::set_password(&new)).await {
        Ok(Ok(())) => Json(serde_json::json!({"ok": true, "separate": true})).into_response(),
        Ok(Err(e)) => bad(&e),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Внутренняя ошибка"}))).into_response(),
    }
}

async fn api_admin_session_get(headers: HeaderMap) -> Json<serde_json::Value> {
    let token = headers.get(crate::web::admin::HEADER).and_then(|v| v.to_str().ok());
    let left = crate::web::admin::remaining_for(token);
    Json(serde_json::json!({"active": left.is_some(), "separate": crate::web::admin::has_separate(), "remaining_secs": left.map(|d| d.as_secs()).unwrap_or(0), "ttl_secs": crate::web::admin::TTL.as_secs()}))
}

async fn api_admin_session_delete(headers: HeaderMap) -> Json<serde_json::Value> {
    // закрыть сеанс может только его владелец: чужой клиент не выгонит администратора
    let token = headers.get(crate::web::admin::HEADER).and_then(|v| v.to_str().ok());
    if crate::web::admin::remaining_for(token).is_some() {
        crate::web::admin::end();
    }
    Json(serde_json::json!({"ok": true}))
}

/// Счётчики для колокольчика в шапке оболочки. **Только числа** (ни текста, ни имён, ни времени): они не раскрывают содержимого, поэтому
/// доступны и при запертой закрытой среде. `messages` — сообщения, ожидающие владельца ПК на узле; `notices` — непрочитанные извещения админ-панели.
async fn api_alerts() -> Json<serde_json::Value> {
    #[cfg(unix)]
    let locked = crate::closed_api::is_locked();
    #[cfg(not(unix))]
    let locked = false;
    Json(serde_json::json!({
        "messages": crate::mobile_api::pc_inbox_pending(),
        "notices": crate::web::admin::view(None)["unread"].as_u64().unwrap_or(0),
        "missed_calls": crate::mobile_self::missed_calls(),
        "locked": locked,
    }))
}

/// Владелец открыл чат: пропущенные звонки просмотрены (счётчик сбрасывается).
async fn api_alerts_ack() -> Json<serde_json::Value> {
    crate::mobile_self::clear_missed_calls();
    Json(serde_json::json!({"ok": true}))
}

async fn api_notices_get(headers: HeaderMap) -> Json<serde_json::Value> {
    let token = headers.get(crate::web::admin::HEADER).and_then(|v| v.to_str().ok());
    Json(crate::web::admin::view(token))
}

#[derive(Deserialize)]
struct NoticesAck {
    kinds: Vec<String>,
}

async fn api_notices_ack(Json(b): Json<NoticesAck>) -> Json<serde_json::Value> {
    crate::web::admin::ack(&b.kinds);
    Json(serde_json::json!({"ok": true}))
}

async fn locked_guard(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    #[cfg(unix)]
    if (is_owner_content_path(req.uri().path()) || is_locked_media_path(req.uri().path())) && crate::closed_api::is_locked() {
        return (StatusCode::LOCKED, Json(serde_json::json!({"status": "locked", "message": "закрытая среда заперта: введите пароль"}))).into_response();
    }
    next.run(req).await
}

/// Получить список всех чатов
async fn api_chats_list(State(state): State<AppState>) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Chat manager not initialized",
        )
    });

    if let Err(_) = chat_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Chat manager not available"
        }));
    }

    let chat_manager = chat_manager.unwrap();

    // пока закрытая среда заперта, даже список собеседников (имена файлов истории) не отдаётся: это метаданные владельца
    #[cfg(unix)]
    if crate::closed_api::is_locked() {
        return Json(serde_json::json!({"status": "locked", "chats": [], "message": "закрытая среда заперта"}));
    }

    // Получить список чатов
    match chat_manager.list_chats() {
        Ok(peer_ids) => {
            let chats: Vec<String> = peer_ids.iter().map(|id| hex::encode(&id.0[..8])).collect();

            Json(serde_json::json!({
                "status": "success",
                "chats": chats
            }))
        }
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to list chats: {}", e)
        })),
    }
}

/// Request для отправки сообщения
#[derive(Deserialize)]
struct SendMessageRequest {
    text: String,
    /// ответ-цитата: на какое сообщение отвечают
    #[serde(default)]
    reply_to: Option<ReplyToRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment: Option<MessageAttachment>,
}

#[derive(Deserialize)]
struct ReplyToRequest {
    msg_id: String,
    text: String,
}

/// Attachment в запросе
#[derive(Deserialize)]
struct MessageAttachment {
    filename: String,
    size: u64,
    #[serde(rename = "mime_type")]
    mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>, // Base64 encoded
    #[serde(skip_serializing_if = "Option::is_none")]
    file_ref: Option<MessageFileReference>,
}

#[derive(Deserialize)]
struct MessageFileReference {
    file_id: String,
    total_chunks: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_name: Option<String>,
}

/// Request for sending file
#[derive(Deserialize)]
struct SendFileRequest {
    filename: String,
    size: u64,
    #[serde(rename = "mime_type")]
    mime_type: String,
    data: String, // Base64 encoded
}

/// Request for editing message
#[derive(Deserialize)]
struct EditMessageRequest {
    msg_id: String,
    text: String,
}

/// Request for deleting message
#[derive(Deserialize)]
struct DeleteMessageRequest {
    msg_id: String,
    for_everyone: bool,
}

// === File Transfer API Handlers ===

/// Upload file to local server (before P2P transfer)
/// Сохраняет файл в /uploads/ и возвращает имя для дальнейшей отправки
async fn api_files_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Создаём директорию для загрузок
    let uploads_dir = crate::communication::files_dir("uploads");
    if let Err(e) = tokio::fs::create_dir_all(&uploads_dir).await {
        error!("❌ Failed to create uploads directory: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to create uploads directory: {}", e)
        }));
    }

    // Обрабатываем multipart
    let mut filename = String::new();
    let mut file_data: Vec<u8> = Vec::new();
    let mut mime_type = String::from("application/octet-stream");

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break, // Конец
            Err(e) => {
                error!("❌ Failed to read multipart field: {}", e);
                return Json(serde_json::json!({
                    "status": "error",
                    "message": "Failed to read multipart"
                }));
            }
        };

        let name = field.name().unwrap_or("").to_string();

        if name == "file" {
            // Имя файла из заголовка
            if let Some(field_filename) = field.file_name() {
                filename = field_filename.to_string();
            }

            // MIME тип
            if let Some(field_mime) = field.content_type() {
                mime_type = field_mime.to_string();
            }

            // Читаем данные файла
            match field.bytes().await {
                Ok(data) => {
                    file_data = data.to_vec();
                }
                Err(e) => {
                    error!("❌ Failed to read file data: {}", e);
                    return Json(serde_json::json!({
                        "status": "error",
                        "message": "Failed to read file data"
                    }));
                }
            }
        }
    }

    if filename.is_empty() || file_data.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "No file data received"
        }));
    }

    // Генерируем уникальное имя файла (чтобы избежать коллизий)
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let safe_filename = format!(
        "{}_{timestamp}",
        filename.replace("/", "_").replace("\\", "_")
    );
    let file_path = uploads_dir.join(&safe_filename);

    // Сохраняем файл
    if let Err(e) = crate::file_vault::write_all(&file_path, &file_data) {
        error!("❌ Failed to save file: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to save file: {}", e)
        }));
    }

    let file_size = file_data.len();

    info!(
        "📤 File uploaded ({} bytes)",
        file_size
    );

    Json(serde_json::json!({
        "status": "success",
        "message": "File uploaded successfully",
        "filename": safe_filename,
        "original_filename": filename,
        "size": file_size,
        "mime_type": mime_type
    }))
}

/// Send file to peer directly (streams multipart → P2P, no disk save!)
/// Принимает файл через multipart, чанкует прямо в памяти, отправляет P2P
async fn api_files_send_direct(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let file_transfer_manager = {
        let ftm_lock = state.file_transfer_manager.lock().await;
        match ftm_lock.as_ref() {
            Some(ftm) => ftm.clone(),
            None => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": "File transfer manager not available"
                }));
            }
        }
    };

    let p2p_transport = match &state.p2p_transport {
        Some(t) => t,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // Попытаться распарсить peer_id
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        id
    } else {
        // Попытка найти через short_id
        if let Some(id) = p2p_transport.find_peer_by_short_id(&peer_id).await {
            id
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer not found: {}", peer_id)
            }));
        }
    };

    // Обрабатываем multipart - получаем файл в память
    let mut filename = String::new();
    let mut file_data: Vec<u8> = Vec::new();
    let mut mime_type = String::from("application/octet-stream");

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                error!("❌ Failed to read multipart field: {}", e);
                return Json(serde_json::json!({
                    "status": "error",
                    "message": "Failed to read multipart"
                }));
            }
        };

        let name = field.name().unwrap_or("").to_string();

        if name == "file" {
            if let Some(field_filename) = field.file_name() {
                filename = field_filename.to_string();
            }

            if let Some(field_mime) = field.content_type() {
                mime_type = field_mime.to_string();
            }

            // Читаем данные файла прямо в память
            match field.bytes().await {
                Ok(data) => {
                    file_data = data.to_vec();
                }
                Err(e) => {
                    error!("❌ Failed to read file data: {}", e);
                    return Json(serde_json::json!({
                        "status": "error",
                        "message": "Failed to read file data"
                    }));
                }
            }
        }
    }

    if filename.is_empty() || file_data.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "No file data received"
        }));
    }

    let file_size = file_data.len();
    let temp_dir = crate::communication::files_dir("uploads");
    if let Err(e) = std::fs::create_dir_all(&temp_dir) {
        error!("❌ Failed to create uploads directory: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to create uploads directory: {}", e)
        }));
    }
    let transfer_id = format!(
        "{}_{}",
        hex::encode(&peer_hash.0[..8]),
        uuid::Uuid::new_v4().simple()
    );
    let temp_path = temp_dir.join(format!(
        "{}__{}",
        transfer_id,
        sanitize_storage_filename(&filename)
    ));
    if let Err(e) = crate::file_vault::write_all(&temp_path, &file_data) {
        error!("❌ Failed to write file to disk: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to write file to disk: {}", e)
        }));
    }

    info!(
        "📤 Sending file ({} bytes) to {} directly from memory",
        file_size,
        crate::util::mask_hash_id(&peer_hash)
    );

    match file_transfer_manager
        .start_file_transfer_from_disk_with_id(
            peer_hash,
            Some(transfer_id.clone()),
            filename.clone(),
            temp_path,
            mime_type.clone(),
        )
        .await
    {
        Ok(file_id) => {
            info!(
                "✅ File transfer started (file_id: {})",
                file_id
            );
            Json(serde_json::json!({
                "status": "success",
                "message": "File transfer started",
                "file_id": file_id,
                "filename": filename,
                "size": file_size
            }))
        }
        Err(e) => {
            error!("❌ Failed to start file transfer: {}", e);
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to start file transfer: {}", e)
            }))
        }
    }
}

/// Accumulate chunks from browser and start P2P transfer when complete
/// Это именно то, как делают Telegram/WhatsApp - чанкование на клиенте!
use std::collections::HashMap;
use std::sync::LazyLock;
use tokio::sync::Mutex as TokioMutex;

const BROWSER_UPLOAD_CHUNK_SIZE: usize = crate::communication::FILE_TRANSFER_CHUNK_SIZE;
/// Телефону владельца файл не идёт по UDP между узлами (узел сам шифрует его и кладёт в очередь телефона), поэтому страница может слать
/// крупные куски: меньше запросов — в сотни раз быстрее. Держим ниже общего предела тела запроса (2 МБ).
const PHONE_UPLOAD_CHUNK_SIZE: usize = 1024 * 1024;

/// Накопитель чанков файлов от браузера
#[derive(Debug, Clone)]
struct PendingFileUpload {
    filename: String,
    mime_type: String,
    total_chunks: u32,
    temp_path: std::path::PathBuf,
    received_chunks: Vec<bool>,
    received_count: u32,
    started: std::time::Instant,
}

/// Most unfinished browser uploads at once, and how long an unfinished one is kept.
const MAX_PENDING_UPLOADS: usize = 32;
const PENDING_UPLOAD_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Глобальное хранилище незавершённых загрузок
static CHUNK_UPLOADS: LazyLock<TokioMutex<HashMap<String, PendingFileUpload>>> =
    LazyLock::new(|| TokioMutex::new(HashMap::new()));

/// Принять чанк файла от браузера и писать сразу во временный файл на диск.
/// Когда все чанки получены - запустить P2P передачу из этого temp-файла.
async fn api_files_send_chunk(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let file_transfer_manager = {
        let ftm_lock = state.file_transfer_manager.lock().await;
        match ftm_lock.as_ref() {
            Some(ftm) => ftm.clone(),
            None => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": "File transfer manager not available"
                }));
            }
        }
    };

    let p2p_transport = match &state.p2p_transport {
        Some(t) => t,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // Попытаться распарсить peer_id
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        id
    } else {
        // Попытка найти через short_id
        if let Some(id) = p2p_transport.find_peer_by_short_id(&peer_id).await {
            id
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer not found: {}", peer_id)
            }));
        }
    };

    // Парсим multipart
    let mut chunk_data: Option<Vec<u8>> = None;
    let mut filename = String::new();
    let mut mime_type = String::from("application/octet-stream");
    let mut file_size: Option<u64> = None;
    let mut chunk_index: Option<u32> = None;
    let mut total_chunks: Option<u32> = None;
    let mut existing_file_id: Option<String> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                error!("❌ Failed to read multipart field: {}", e);
                return Json(serde_json::json!({
                    "status": "error",
                    "message": "Failed to read multipart"
                }));
            }
        };

        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "file" => {
                if let Ok(data) = field.bytes().await {
                    chunk_data = Some(data.to_vec());
                }
            }
            "filename" => {
                if let Ok(s) = field.text().await {
                    filename = s;
                }
            }
            "mime_type" => {
                if let Ok(s) = field.text().await {
                    mime_type = s;
                }
            }
            "file_size" => {
                if let Ok(s) = field.text().await {
                    file_size = s.parse().ok();
                }
            }
            "chunk_index" => {
                if let Ok(s) = field.text().await {
                    chunk_index = s.parse().ok();
                }
            }
            "total_chunks" => {
                if let Ok(s) = field.text().await {
                    total_chunks = s.parse().ok();
                }
            }
            "file_id" => {
                if let Ok(s) = field.text().await {
                    existing_file_id = Some(s);
                }
            }
            _ => {}
        }
    }

    // Валидация
    let chunk_data = match chunk_data {
        Some(d) => d,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "No chunk data received"
            }));
        }
    };

    let chunk_limit = if crate::mobile_api::is_device_peer(&peer_id) { PHONE_UPLOAD_CHUNK_SIZE } else { BROWSER_UPLOAD_CHUNK_SIZE };
    if chunk_data.len() > chunk_limit {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!(
                "Chunk too large: got {} bytes, max {} bytes",
                chunk_data.len(),
                chunk_limit
            )
        }));
    }

    let chunk_index = match chunk_index {
        Some(i) => i,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "No chunk_index provided"
            }));
        }
    };

    let total_chunks = match total_chunks {
        Some(t) => t,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "No total_chunks provided"
            }));
        }
    };

    let file_size = match file_size {
        Some(size) => size,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "No file_size provided"
            }));
        }
    };

    // Генерируем или получаем file_id
    let upload_id = if let Some(ref fid) = existing_file_id {
        fid.clone()
    } else {
        // Новый upload - генерируем ID на основе peer + timestamp
        format!(
            "{}_{}",
            hex::encode(&peer_hash.0[..8]),
            chrono::Utc::now().timestamp_millis()
        )
    };

    let uploads_dir = crate::communication::files_dir("uploads");
    if let Err(e) = std::fs::create_dir_all(&uploads_dir) {
        error!("❌ Failed to create uploads directory: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to create uploads directory: {}", e)
        }));
    }

    // limits before anything is allocated: the numbers come from the browser request
    // телефону владельца — до 1 ГБ (предел хранилища файлов телефонов), другим узлам — 200 МБ (сетевой предел передачи между узлами)
    let max_size = if crate::mobile_api::is_device_peer(&peer_id) { crate::mobile_files::MAX_FILE } else { crate::communication::MAX_FILE_TRANSFER_SIZE };
    let max_chunks = (max_size as usize / chunk_limit + 1) as u32;
    if total_chunks == 0
        || total_chunks > max_chunks
        || file_size > max_size
    {
        return Json(
            serde_json::json!({"status": "error", "message": "File is larger than the allowed size"}),
        );
    }
    // the id is used in a file name: only plain characters
    if upload_id.is_empty()
        || upload_id.len() > 80
        || !upload_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Json(serde_json::json!({"status": "error", "message": "Invalid file_id"}));
    }

    // Сохраняем чанк сразу в temp-файл
    let mut uploads = CHUNK_UPLOADS.lock().await;
    // forget abandoned uploads (and their temp files), refuse too many at once
    uploads.retain(|_, u| {
        let alive = u.started.elapsed() < PENDING_UPLOAD_TTL;
        if !alive {
            let _ = std::fs::remove_file(&u.temp_path);
            let _ = std::fs::remove_dir_all(upload_parts_dir(&u.temp_path));
        }
        alive
    });
    if !uploads.contains_key(&upload_id) && uploads.len() >= MAX_PENDING_UPLOADS {
        return Json(
            serde_json::json!({"status": "error", "message": "Too many unfinished uploads"}),
        );
    }
    let mut created = false;

    let upload = uploads.entry(upload_id.clone()).or_insert_with(|| {
        created = true;
        PendingFileUpload {
            filename: filename.clone(),
            mime_type: mime_type.clone(),
            total_chunks,
            temp_path: uploads_dir.join(format!(
                "{}__{}",
                upload_id,
                sanitize_storage_filename(&filename)
            )),
            received_chunks: vec![false; total_chunks as usize],
            received_count: 0,
            started: std::time::Instant::now(),
        }
    });

    if upload.total_chunks != total_chunks {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("total_chunks mismatch: expected {}, got {}", upload.total_chunks, total_chunks)
        }));
    }

    let idx = chunk_index as usize;
    if idx >= upload.received_chunks.len() {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("chunk_index out of range: {}", chunk_index)
        }));
    }

    // каждая часть — отдельный запечатанный файл; открытого файла на диске нет, целый запечатанный файл собирается по завершении
    let write_result = (|| -> std::io::Result<()> {
        let dir = upload_parts_dir(&upload.temp_path);
        std::fs::create_dir_all(&dir)?;
        crate::file_vault::write_all(&dir.join(chunk_index.to_string()), &chunk_data)
    })();

    if let Err(e) = write_result {
        error!("❌ Failed to write upload chunk to disk: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to write upload chunk: {}", e)
        }));
    }

    if !upload.received_chunks[idx] {
        upload.received_chunks[idx] = true;
        upload.received_count += 1;
    }

    info!(
        "📦 Received chunk {}/{} ({} bytes)",
        chunk_index + 1,
        total_chunks,
        chunk_data.len()
    );

    // Проверяем: все ли чанки получены?
    let is_complete = upload.received_count == upload.total_chunks;
    let file_id = upload_id.clone();
    let received_count = upload.received_count;
    let upload_filename = upload.filename.clone();
    let upload_mime = upload.mime_type.clone();
    let temp_path = upload.temp_path.clone();
    let upload_total_chunks = upload.total_chunks;

    drop(uploads);

    // все части на месте: собираем запечатанный файл (с него идут повторы недостающих кусков)
    if is_complete {
        let parts: Vec<std::path::PathBuf> =
            (0..upload_total_chunks).map(|i| upload_parts_dir(&temp_path).join(i.to_string())).collect();
        let tp = temp_path.clone();
        let size = file_size;
        let built = tokio::task::spawn_blocking(move || {
            let r = crate::file_vault::assemble(&parts, &tp, size);
            // принятые части удаляются только после успешной сборки; при отказе (диск, замок) они остаются для повтора последней части
            if r.is_ok() {
                let _ = std::fs::remove_dir_all(upload_parts_dir(&tp));
            } else {
                let _ = std::fs::remove_file(&tp); // недособранный итоговый файл
            }
            r
        })
        .await;
        match built {
            Ok(Ok(())) => {}
            other => {
                let why = match other { Ok(Err(e)) => e.to_string(), Err(e) => e.to_string(), Ok(Ok(())) => String::new() };
                // запись о загрузке остаётся: повтор последней части снова запустит сборку из сохранённых частей
                error!("❌ Failed to assemble upload: {}", why);
                return Json(serde_json::json!({"status": "error", "message": format!("Failed to assemble upload: {}", why)}));
            }
        }
    }

    // телефон владельца: по сети узлов не шлём; когда файл собран, кладём его в uploads — отправит api_chat_send (зашифровав для телефона)
    if crate::mobile_api::is_device_peer(&peer_id) {
        if !is_complete {
            return Json(serde_json::json!({"status": "success", "message": "Chunk stored", "file_id": file_id, "received": received_count, "complete": false}));
        }
        let stored = uploads_dir.join(format!("{}__{}", file_id, sanitize_storage_filename(&upload_filename)));
        if let Err(e) = std::fs::rename(&temp_path, &stored) {
            return Json(serde_json::json!({"status": "error", "message": format!("Failed to store upload: {}", e)}));
        }
        CHUNK_UPLOADS.lock().await.remove(&file_id);
        let size = crate::file_vault::plain_len(&stored).unwrap_or(file_size);
        return Json(serde_json::json!({"status": "success", "message": "File transfer completed", "file_id": file_id, "size": size, "complete": true}));
    }

    if created {
        if let Err(e) = file_transfer_manager
            .register_streaming_transfer(
                peer_hash,
                Some(file_id.clone()),
                upload_filename.clone(),
                temp_path.clone(),
                upload_mime.clone(),
                file_size,
                upload_total_chunks,
            )
            .await
        {
            error!("❌ Failed to register streaming transfer: {}", e);
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to register streaming transfer: {}", e)
            }));
        }
    }

    if let Err(e) = file_transfer_manager
        .send_streaming_chunk(peer_hash, &file_id, chunk_index, &chunk_data)
        .await
    {
        error!("❌ Failed to stream chunk via P2P: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to stream chunk via P2P: {}", e)
        }));
    }

    if is_complete {
        let file_size = match crate::file_vault::plain_len(&temp_path) {
            Ok(len) => len,
            Err(e) => {
                error!("❌ Failed to stat temp upload file: {}", e);
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Failed to stat temp upload file: {}", e)
                }));
            }
        };

        info!("✅ All chunks received! File size: {} bytes", file_size);

        match file_transfer_manager
            .finalize_streaming_transfer(peer_hash, &file_id)
            .await
        {
            Ok(_) => {
                info!(
                    "✅ Streaming P2P transfer finalized for file {}",
                    file_id
                );
                CHUNK_UPLOADS.lock().await.remove(&file_id);
                return Json(serde_json::json!({
                    "status": "success",
                    "message": "File transfer completed",
                    "file_id": file_id,
                    "size": file_size,
                    "complete": true
                }));
            }
            Err(e) => {
                error!("❌ Failed to finalize streaming transfer: {}", e);
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Failed to finalize transfer: {}", e)
                }));
            }
        }
    } else {
        // Ещё не все чанки получены
        return Json(serde_json::json!({
            "status": "success",
            "message": format!("Chunk {} received", chunk_index + 1),
            "file_id": file_id,
            "chunk_index": chunk_index,
            "received": received_count,
            "total": upload_total_chunks,
            "complete": false
        }));
    }
}

/// Send uploaded file to peer (from /uploads directory)
async fn api_files_send_uploaded(
    State(state): State<AppState>,
    PathExtractor((peer_id, filename)): PathExtractor<(String, String)>,
) -> impl IntoResponse {
    let file_transfer_manager = {
        let ftm_lock = state.file_transfer_manager.lock().await;
        match ftm_lock.as_ref() {
            Some(ftm) => ftm.clone(),
            None => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": "File transfer manager not available"
                }));
            }
        }
    };

    let transport = match &state.p2p_transport {
        Some(t) => t,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };

    // Попытаться распарсить peer_id
    let peer_hash = if let Ok(id) = crate::util::HashId::from_hex(&peer_id) {
        id
    } else {
        // Попытка найти через short_id
        if let Some(id) = transport.find_peer_by_short_id(&peer_id).await {
            id
        } else {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer not found: {}", peer_id)
            }));
        }
    };

    // Читаем файл из uploads
    let file_path = crate::communication::files_dir("uploads").join(&filename);
    let file_data = match crate::file_vault::read_all(&file_path) {
        Ok(data) => data,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("File not found: {}", e)
            }));
        }
    };

    // MIME тип
    let mime_type = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();

    info!(
        "📤 Sending file ({} bytes) to {} with chunking",
        file_data.len(),
        crate::util::mask_hash_id(&peer_hash)
    );

    // Начать чанкованную передачу файла
    let transfer_id = format!(
        "{}_{}",
        hex::encode(&peer_hash.0[..8]),
        uuid::Uuid::new_v4().simple()
    );
    match file_transfer_manager
        .start_file_transfer_from_disk_with_id(
            peer_hash,
            Some(transfer_id),
            filename.clone(),
            file_path.clone(),
            mime_type,
        )
        .await
    {
        Ok(file_id) => {
            info!(
                "✅ File transfer started (file_id: {})",
                file_id
            );
            Json(serde_json::json!({
                "status": "success",
                "message": "File transfer started",
                "file_id": file_id,
                "filename": filename,
                "size": file_data.len()
            }))
        }
        Err(e) => {
            error!("❌ Failed to start file transfer: {}", e);
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to start file transfer: {}", e)
            }))
        }
    }
}

pub(crate) fn sanitize_storage_filename(filename: &str) -> String {
    // Strip path components — take only the file name (SEC-08)
    let name = std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");

    // Allow only safe ASCII characters; reject null bytes and unicode path separators
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    // Strip leading dots (hidden file trick) and trailing dots/spaces (Windows)
    let trimmed = sanitized.trim_start_matches('.');
    let trimmed = trimmed.trim_end_matches(|c: char| c == '.' || c == ' ');

    // Reject Windows reserved names
    let is_reserved = matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    );

    if trimmed.is_empty() || is_reserved {
        "file".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

/// Тип, с которым файл из чата отдаётся браузеру. Имя файла присылает собеседник, а отдаём мы его с того же адреса, что и страницу узла:
/// `x.html`/`x.svg` как text/html или image/svg+xml выполнили бы чужой скрипт с правами владельца (выпустить QR сопряжения, читать и
/// слать сообщения). Поэтому показывать «на месте» можно только картинки растровых форматов, звук и видео; всё остальное — как
/// application/octet-stream (браузер скачает, а не откроет).
pub(crate) fn safe_served_type(filename: &str) -> String {
    let guessed = mime_guess::from_path(filename).first_or_octet_stream().to_string();
    let inline_ok = matches!(guessed.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp")
        || guessed.starts_with("audio/")
        || guessed.starts_with("video/");
    if inline_ok {
        guessed
    } else {
        "application/octet-stream".to_string()
    }
}

/// inline — только для безопасных типов из `safe_served_type`; остальное скачивается (имя — только безопасные символы).
pub(crate) fn served_disposition(content_type: &str, filename: &str) -> String {
    let inline_ok = content_type.starts_with("image/") || content_type.starts_with("audio/") || content_type.starts_with("video/");
    if inline_ok {
        "inline".to_string()
    } else {
        let name: String = filename.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
        format!("attachment; filename=\"{}\"", name)
    }
}

pub(crate) fn resolve_local_file_path(file_id: &str, filename: &str) -> Option<std::path::PathBuf> {
    // SEC-07: validate file_id — only alphanumeric, underscores, and hyphens
    if file_id.is_empty()
        || !file_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }

    let stored_name = format!("{}__{}", file_id, sanitize_storage_filename(filename));
    let base_dirs = [
        crate::communication::files_dir("downloads"),
        crate::communication::files_dir("uploads"),
    ];

    for base in &base_dirs {
        let candidate = base.join(&stored_name);
        // Canonicalize resolves symlinks and ".." — then verify the result
        // is still inside the base directory (path traversal guard)
        if let Ok(canonical) = std::fs::canonicalize(&candidate) {
            if canonical.starts_with(base) {
                return Some(canonical);
            }
        }
    }

    None
}

fn parse_single_http_range(range_header: &str, data_len: usize) -> Option<(usize, usize)> {
    let range_value = range_header.strip_prefix("bytes=")?.trim();
    if range_value.is_empty() || range_value.contains(',') {
        return None;
    }

    let (start_raw, end_raw) = range_value.split_once('-')?;
    let start_raw = start_raw.trim();
    let end_raw = end_raw.trim();

    if start_raw.is_empty() {
        let suffix_len = end_raw.parse::<usize>().ok()?.min(data_len);
        let start = data_len.saturating_sub(suffix_len);
        let end = data_len.saturating_sub(1);
        return Some((start, end));
    }

    let start = start_raw.parse::<usize>().ok()?;
    let end = if end_raw.is_empty() {
        data_len.saturating_sub(1)
    } else {
        end_raw.parse::<usize>().ok()?
    };

    Some((start, end))
}

/// Папка с отдельными запечатанными частями загрузки, пока файл не собран.
fn upload_parts_dir(temp: &std::path::Path) -> std::path::PathBuf {
    let name = temp.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    temp.with_file_name(format!(".{name}.parts"))
}

async fn api_files_content(
    headers: HeaderMap,
    PathExtractor((file_id, filename)): PathExtractor<(String, String)>,
) -> impl IntoResponse {
    let file_path = match resolve_local_file_path(&file_id, &filename) {
        Some(path) => path,
        None => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(Body::empty())
                .unwrap();
        }
    };

    match crate::file_vault::read_all(&file_path) {
        Ok(data) => {
            let content_type = safe_served_type(&filename);
            let data_len = data.len();
            let is_streaming_media =
                content_type.starts_with("audio/") || content_type.starts_with("video/");

            if let Some(range_header) = headers
                .get(axum::http::header::RANGE)
                .and_then(|v| v.to_str().ok())
            {
                debug!(
                    "📡 File content request: file_id={}, range={}, media={}",
                    file_id, range_header, is_streaming_media
                );

                if !is_streaming_media {
                    if let Some((start, mut end)) = parse_single_http_range(range_header, data_len)
                    {
                        if start >= data_len {
                            return Response::builder()
                                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                                .header(
                                    axum::http::header::CONTENT_RANGE,
                                    format!("bytes */{}", data_len),
                                )
                                .header(axum::http::header::ACCEPT_RANGES, "bytes")
                                .header("Cache-Control", "no-store")
                                .body(Body::empty())
                                .unwrap();
                        }

                        end = end.min(data_len.saturating_sub(1));
                        if end >= start {
                            let chunk = data[start..=end].to_vec();
                            return Response::builder()
                                .status(StatusCode::PARTIAL_CONTENT)
                                .header("X-Content-Type-Options", "nosniff")
                                .header("Content-Security-Policy", "sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'")
                                .header("Content-Disposition", served_disposition(&content_type, &filename))
                                .header(axum::http::header::CONTENT_TYPE, content_type.clone())
                                .header(axum::http::header::ACCEPT_RANGES, "bytes")
                                .header(axum::http::header::CONTENT_LENGTH, chunk.len().to_string())
                                .header(
                                    axum::http::header::CONTENT_RANGE,
                                    format!("bytes {}-{}/{}", start, end, data_len),
                                )
                                .header("Cache-Control", "no-store")
                                .body(Body::from(chunk))
                                .unwrap();
                        }
                    }
                } else {
                    debug!(
                        "📡 Serving full media file instead of partial range response: {}",
                        filename
                    );
                }
            }

            Response::builder()
                .status(StatusCode::OK)
                .header("X-Content-Type-Options", "nosniff")
                .header("Content-Security-Policy", "sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'")
                .header("Content-Disposition", served_disposition(&content_type, &filename))
                .header(axum::http::header::CONTENT_TYPE, content_type)
                .header(axum::http::header::CONTENT_LENGTH, data_len.to_string())
                .header("Cache-Control", "no-store")
                .body(Body::from(data))
                .unwrap()
        }
        Err(_) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("Content-Type", "text/plain; charset=utf-8")
            .header("Cache-Control", "no-store")
            .body(Body::empty())
            .unwrap(),
    }
}

/// Check whether a transferred file is available locally (for the receiver's UI).
async fn api_files_status(
    PathExtractor((file_id, filename)): PathExtractor<(String, String)>,
) -> impl IntoResponse {
    let exists = resolve_local_file_path(&file_id, &filename).is_some();
    Json(serde_json::json!({
        "file_id": file_id,
        "filename": filename,
        "exists": exists,
    }))
}

// === P2P Tunnel API Handlers ===

/// Start P2P tunnel with peer
async fn api_tunnel_start(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let tunnel_manager = state.p2p_tunnel_manager.lock().await;
    let tunnel_manager = tunnel_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "P2P Tunnel manager not initialized",
        )
    });

    if let Err(_) = tunnel_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Tunnel manager not available"
        }));
    }

    let tunnel_manager = tunnel_manager.unwrap();

    // Parse tunnel type from payload (default: Generic)
    let tunnel_type = match payload.get("type").and_then(|t| t.as_str()) {
        Some("voice") => crate::p2p_tunnel::TunnelType::Voice,
        Some("video") => crate::p2p_tunnel::TunnelType::Video,
        Some("file") => crate::p2p_tunnel::TunnelType::FileTransfer,
        Some("gaming") => crate::p2p_tunnel::TunnelType::Gaming,
        _ => crate::p2p_tunnel::TunnelType::Generic,
    };

    // Find peer by short_id
    let transport_opt = state.transport.clone();
    if transport_opt.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    }

    let transport = transport_opt.unwrap();
    let peer_id = match transport.find_peer_by_short_id(&short_id) {
        Some(id) => id,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer {} not found", short_id)
            }));
        }
    };

    // Request tunnel
    match tunnel_manager.request_tunnel(peer_id, tunnel_type).await {
        Ok(_tunnel) => Json(serde_json::json!({
            "status": "success",
            "message": format!("P2P tunnel request sent to {}", short_id),
            "peer": short_id,
            "tunnel_type": format!("{:?}", tunnel_type)
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to start tunnel: {}", e)
        })),
    }
}

/// Stop P2P tunnel with peer
async fn api_tunnel_stop(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    let tunnel_manager = state.p2p_tunnel_manager.lock().await;
    let tunnel_manager = tunnel_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "P2P Tunnel manager not initialized",
        )
    });

    if let Err(_) = tunnel_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Tunnel manager not available"
        }));
    }

    let tunnel_manager = tunnel_manager.unwrap();

    // Find peer by short_id
    let transport_opt = state.transport.clone();
    if transport_opt.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    }

    let transport = transport_opt.unwrap();
    let peer_id = match transport.find_peer_by_short_id(&short_id) {
        Some(id) => id,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer {} not found", short_id)
            }));
        }
    };

    // Close tunnel
    match tunnel_manager.close_tunnel(peer_id).await {
        Ok(_) => Json(serde_json::json!({
            "status": "success",
            "message": format!("P2P tunnel with {} closed", short_id)
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to close tunnel: {}", e)
        })),
    }
}

/// List all active P2P tunnels
async fn api_tunnel_list(State(state): State<AppState>) -> impl IntoResponse {
    let tunnel_manager = state.p2p_tunnel_manager.lock().await;
    let tunnel_manager = tunnel_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "P2P Tunnel manager not initialized",
        )
    });

    if let Err(_) = tunnel_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Tunnel manager not available"
        }));
    }

    let tunnel_manager = tunnel_manager.unwrap();

    // Get tunnel list
    let tunnels = tunnel_manager.list_tunnels().await;

    let tunnel_list: Vec<serde_json::Value> = tunnels
        .iter()
        .map(|t| {
            serde_json::json!({
                "peer": hex::encode(&t.peer.0[..8]),
                "tunnel_type": format!("{:?}", t.tunnel_type),
                "status": format!("{:?}", t.status),
                "bytes_sent": t.bytes_sent,
                "bytes_received": t.bytes_received,
                "created_at": t.created_at
            })
        })
        .collect();

    Json(serde_json::json!({
        "status": "success",
        "tunnels": tunnel_list,
        "count": tunnel_list.len()
    }))
}

/// Get status of P2P tunnel with peer
async fn api_tunnel_status(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    let tunnel_manager = state.p2p_tunnel_manager.lock().await;
    let tunnel_manager = tunnel_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "P2P Tunnel manager not initialized",
        )
    });

    if let Err(_) = tunnel_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Tunnel manager not available"
        }));
    }

    let tunnel_manager = tunnel_manager.unwrap();

    // Find peer by short_id
    let transport_opt = state.transport.clone();
    if transport_opt.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    }

    let transport = transport_opt.unwrap();
    let peer_id = match transport.find_peer_by_short_id(&short_id) {
        Some(id) => id,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer {} not found", short_id)
            }));
        }
    };

    // Get tunnel info
    match tunnel_manager.get_tunnel(&peer_id).await {
        Some(tunnel) => {
            let info = tunnel.info().await;
            Json(serde_json::json!({
                "status": "success",
                "tunnel": {
                    "peer": short_id,
                    "tunnel_type": format!("{:?}", info.tunnel_type),
                    "tunnel_status": format!("{:?}", info.status),
                    "bytes_sent": info.bytes_sent,
                    "bytes_received": info.bytes_received,
                    "created_at": info.created_at
                }
            }))
        }
        None => Json(serde_json::json!({
            "status": "error",
            "message": format!("No active tunnel with {}", short_id)
        })),
    }
}

// === P2P Communication API Handlers (port 9998) ===

/// GET /api/p2p/status - Get P2P transport status
async fn api_p2p_status(State(state): State<AppState>) -> impl IntoResponse {
    let p2p_transport_opt = state.p2p_transport;

    if p2p_transport_opt.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Communication transport not available"
        }));
    }

    let p2p_transport = p2p_transport_opt.unwrap();

    // Get transport info
    let node_id = p2p_transport.node_id();
    let short_id = p2p_transport.short_id();
    let data_addr = p2p_transport.data_addr();

    Json(serde_json::json!({
        "status": "success",
        "p2p_transport": {
            "node_id": hex::encode(&node_id.0[..8]),
            "short_id": short_id,
            "data_addr": data_addr,
            "mtu": 65536,
            "description": "P2P Communication transport (Chat, Files, Voice, Video)"
        }
    }))
}

/// GET /api/p2p/peers - Get list of P2P peers
async fn api_p2p_peers(State(state): State<AppState>) -> impl IntoResponse {
    let p2p_transport_opt = state.p2p_transport;

    if p2p_transport_opt.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Communication transport not available"
        }));
    }

    let p2p_transport = p2p_transport_opt.unwrap();

    let peers = p2p_transport.list_peers().await;
    let peers_json: Vec<_> = peers
        .into_iter()
        .map(|peer| {
            serde_json::json!({
                "short_id": hex::encode(&peer.id.0[..8]),
                "node_id": hex::encode(&peer.id.0),
                "discovery_addr": peer.addr,
                "data_addr": peer.p2p_data_addr,
                "nat_status": peer.nat_status.as_str(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "status": "success",
        "peers": peers_json,
        "message": "P2P peers discovered on the dedicated P2P transport"
    }))
}

/// POST /api/p2p/sync - kept for UI compatibility, no-op for isolated transport
async fn api_p2p_sync(State(state): State<AppState>) -> impl IntoResponse {
    let p2p_transport_opt = state.p2p_transport.clone();

    if p2p_transport_opt.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Communication transport not available"
        }));
    }

    let p2p_transport = p2p_transport_opt.unwrap();
    let peers_count = p2p_transport.list_peers().await.len();

    Json(serde_json::json!({
        "status": "success",
        "message": "P2P transport is isolated; peer sync from netlayer is disabled",
        "peers_count": peers_count
    }))
}

/// GET /api/config - Получить клиентскую конфигурацию (для мобилки)
async fn api_config_get() -> impl IntoResponse {
    let config = crate::get_config();
    let client_config = config.to_client_config();

    Json(serde_json::json!({
        "status": "success",
        "config": {
            "discovery_port": client_config.discovery_port,
            "gateway_port": client_config.gateway_port,
            "p2p_port": client_config.p2p_port,
            "http_proxy_port": client_config.http_proxy_port,
        },
        "server": {
            "bind_address": config.server.bind_address,
            "public_ip": config.network.public_ip,
        },
        "version": crate::VERSION
    }))
}

// === Media files handler ===
/// Only plain relative names below the media directory: no `..`, no absolute paths, no backslashes.
fn media_name_ok(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

async fn media_handler(PathExtractor(path): PathExtractor<String>) -> impl IntoResponse {
    if !media_name_ok(&path) {
        return (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            Vec::<u8>::new(),
        );
    }
    let media_path = format!("ui/media/{}", path);
    match std::fs::read(media_path) {
        Ok(data) => {
            let content_type = if path.ends_with(".mp3") {
                "audio/mpeg"
            } else if path.ends_with(".wav") {
                "audio/wav"
            } else {
                "application/octet-stream"
            };
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, content_type)],
                data,
            )
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            Vec::<u8>::new(),
        ),
    }
}
// === Groups API Handlers ===











// === Group Chat API Handlers ===







// === Profile API Handlers ===

/// Get current user profile
async fn api_profile_get(State(state): State<AppState>) -> impl IntoResponse {
    let short_id = state.node_info.short_id.clone();

    match UserProfile::load() {
        Ok(Some(profile)) => Json(serde_json::json!({
            "status": "success",
            "profile": {
                "display_name": profile.display_name,
                "avatar": profile.avatar,
                "short_id": profile.short_id,
                "updated_at": profile.updated_at
            }
        })),
        Ok(None) => {
            let profile = UserProfile::new(short_id);
            Json(serde_json::json!({
                "status": "success",
                "profile": {
                    "display_name": profile.display_name,
                    "avatar": profile.avatar,
                    "short_id": profile.short_id,
                    "updated_at": profile.updated_at
                }
            }))
        }
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to load profile: {}", e)
        })),
    }
}

/// Удалить файлы аватара своего узла (кроме расширения `keep`): иначе после смены png→jpg старый файл отдаётся первым,
/// а «удалённый» аватар остаётся на диске.
fn remove_avatar_files(short_id: &str, keep: Option<&str>) {
    let Some(home) = dirs::home_dir() else { return };
    remove_avatar_files_in(&home.join(".yandi/avatars"), short_id, keep);
}

fn remove_avatar_files_in(dir: &std::path::Path, short_id: &str, keep: Option<&str>) {
    if short_id.is_empty() || !short_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return;
    }
    for ext in ["png", "jpg", "gif"] {
        if keep == Some(ext) {
            continue;
        }
        let _ = std::fs::remove_file(dir.join(format!("{short_id}.{ext}")));
    }
}

/// Один писатель на файл контактов: чтение–изменение–запись без потерь при одновременных запросах.
static CONTACTS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Имя контакта из приложения на телефоне: завести контакт или переименовать (общий список узла, его видят все устройства и ПК).
pub async fn contacts_upsert(short_id: &str, name: &str) -> Result<(), String> {
    let name: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(64).collect();
    if name.is_empty() || short_id.len() != 16 || !short_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("неверное имя или номер".into());
    }
    let short_id = short_id.to_ascii_lowercase();
    let _guard = CONTACTS_LOCK.lock().await;
    let mut contacts = load_contacts_for_edit(&crate::util::data_dir::data_dir().join("contacts.json").display().to_string()).await?;
    match contacts.iter_mut().find(|c| c.get("short_id").and_then(|v| v.as_str()) == Some(short_id.as_str())) {
        Some(c) => c["name"] = serde_json::json!(name),
        None => contacts.push(serde_json::json!({"id": uuid::Uuid::new_v4().to_string(), "name": name, "short_id": short_id, "online": false})),
    }
    save_contacts(&crate::util::data_dir::data_dir().join("contacts.json").display().to_string(), &contacts).await
}


/// Список контактов для изменения. Нет файла — пустой список; файл есть, но не читается или не разбирается — ошибка
/// (раньше такой файл считался пустым и затем перезаписывался одним новым контактом).
async fn load_contacts_for_edit(path: &str) -> Result<Vec<serde_json::Value>, String> {
    if !std::path::Path::new(path).exists() {
        return Ok(Vec::new());
    }
    let content = tokio::fs::read_to_string(path).await.map_err(|e| format!("не удалось прочитать список контактов: {e}"))?;
    let data: serde_json::Value = serde_json::from_str(&content).map_err(|_| "файл контактов повреждён, список не менялся".to_string())?;
    match data.get("contacts").and_then(|v| v.as_array()) {
        Some(arr) => Ok(arr.clone()),
        None => Err("файл контактов в неизвестном формате, список не менялся".to_string()),
    }
}

/// Записать список контактов целиком и атомарно (временный файл и переименование), ошибка диска возвращается.
async fn save_contacts(path: &str, contacts: &[serde_json::Value]) -> Result<(), String> {
    let content = serde_json::to_string_pretty(&serde_json::json!({ "contacts": contacts })).map_err(|e| e.to_string())?;
    let path = path.to_string();
    tokio::task::spawn_blocking(move || crate::util::private_file::write_private(std::path::Path::new(&path), content.as_bytes()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("не удалось сохранить список контактов: {e}"))
}

/// Update current user profile
async fn api_profile_put(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let short_id = state.node_info.short_id.clone();
    let display_name = payload
        .get("display_name")
        .and_then(|v| v.as_str())
        .map(String::from);
    let avatar = payload
        .get("avatar")
        .and_then(|v| v.as_str())
        .map(String::from);

    let mut profile = match UserProfile::load() {
        Ok(Some(p)) => p,
        _ => UserProfile::new(short_id),
    };

    let avatar_cleared = avatar.as_deref() == Some("");
    profile.update(display_name, avatar);

    match profile.save() {
        Ok(()) => {
            if avatar_cleared {
                remove_avatar_files(&state.node_info.short_id, None);
            }
            Json(serde_json::json!({
            "status": "success",
            "message": "Profile updated",
            "profile": {
                "display_name": profile.display_name,
                "avatar": profile.avatar,
                "short_id": profile.short_id,
                "updated_at": profile.updated_at
            }
        }))
        }
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to save profile: {}", e)
        })),
    }
}

/// Get profile by short_id (from contacts cache)
fn phone_profile(short_id: &str) -> Option<serde_json::Value> {
    // телефон владельца в contacts.json не хранится: профиль строим из списка сопряжённых устройств
    let mut data = serde_json::json!({ "contacts": [] });
    append_phone_contacts(&mut data);
    data["contacts"].as_array()?.iter().find(|c| c["short_id"].as_str() == Some(short_id)).map(|c| {
        serde_json::json!({
            "status": "success",
            "profile": {
                "display_name": c.get("name").and_then(|n| n.as_str()).unwrap_or(short_id),
                "avatar": c.get("avatar"),
                "short_id": short_id,
                "online": c.get("online").and_then(|o| o.as_bool()).unwrap_or(false)
            }
        })
    })
}

async fn api_profile_get_by_short_id(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    if let Some(found) = phone_profile(&short_id) {
        return Json(found);
    }
    let contacts_path_owned = crate::util::data_dir::data_dir().join("contacts.json").display().to_string();
    let contacts_path = contacts_path_owned.as_str();

    if !std::path::Path::new(contacts_path).exists() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Contact not found"
        }));
    }

    match tokio::fs::read_to_string(contacts_path).await {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(data) => {
                if let Some(contacts) = data.get("contacts").and_then(|c| c.as_array()) {
                    for contact in contacts {
                        if contact.get("short_id").and_then(|s| s.as_str()) == Some(&short_id) {
                            return Json(serde_json::json!({
                                "status": "success",
                                "profile": {
                                    "display_name": contact.get("name").and_then(|n| n.as_str()).unwrap_or(&short_id),
                                    "avatar": contact.get("avatar"),
                                    "short_id": short_id,
                                    "online": contact.get("online").and_then(|o| o.as_bool()).unwrap_or(false)
                                }
                            }));
                        }
                    }
                }
                Json(serde_json::json!({
                    "status": "error",
                    "message": "Contact not found"
                }))
            }
            Err(_) => Json(serde_json::json!({
                "status": "error",
                "message": "Failed to parse contacts"
            })),
        },
        Err(_) => Json(serde_json::json!({
            "status": "error",
            "message": "Failed to read contacts"
        })),
    }
}

/// Upload avatar for current user
async fn api_profile_avatar(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let short_id = state.node_info.short_id.clone();
    let avatar_dir = dirs::home_dir()
        .expect("No home directory")
        .join(".yandi/avatars");

    // Create directory if not exists
    if let Err(e) = tokio::fs::create_dir_all(&avatar_dir).await {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to create avatar directory: {}", e)
        }));
    }

    let mut file_data: Vec<u8> = Vec::new();
    let mut file_ext = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "avatar" {
            if let Some(content_type) = field.content_type() {
                file_ext = match content_type {
                    "image/png" => "png",
                    "image/jpeg" => "jpg",
                    "image/gif" => "gif",
                    _ => "png",
                }
                .to_string();
            }
            match field.bytes().await {
                Ok(data) => file_data = data.to_vec(),
                Err(e) => {
                    return Json(serde_json::json!({
                        "status": "error",
                        "message": format!("Failed to read file: {}", e)
                    }));
                }
            }
        }
    }

    if file_data.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "No file uploaded"
        }));
    }
    if file_data.len() > 4 * 1024 * 1024 {
        return Json(
            serde_json::json!({"status": "error", "message": "Avatar is larger than 4 MB"}),
        );
    }
    // the type comes from the file itself, not from what the browser claims
    file_ext = if file_data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if file_data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if file_data.starts_with(b"GIF8") {
        "gif"
    } else {
        return Json(serde_json::json!({"status": "error", "message": "Avatar must be a PNG, JPEG or GIF image"}));
    }.to_string();

    // картинка пересобирается из пикселей (метаданных нет) и сводится к маленькому PNG — так её принимают и телефоны
    let file_data = match crate::mobile_profile::sanitize_avatar(&file_data) {
        Some(png) => {
            file_ext = "png".to_string();
            png
        }
        None => {
            return Json(serde_json::json!({"status": "error", "message": "Avatar could not be read as an image"}));
        }
    };

    // Save file
    let avatar_path = avatar_dir.join(format!("{}.{}", short_id, file_ext));
    if let Err(e) = tokio::fs::write(&avatar_path, &file_data).await {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to save avatar: {}", e)
        }));
    }

    // файл другого типа от прошлой загрузки отдавался бы первым — убираем
    remove_avatar_files(&short_id, Some(file_ext.as_str()));

    // Update profile with avatar path
    let mut profile = match UserProfile::load() {
        Ok(Some(p)) => p,
        _ => UserProfile::new(short_id.clone()),
    };

    let avatar_url = format!("/api/avatar/{}", short_id);
    profile.update(None, Some(avatar_url.clone()));
    if let Err(e) = profile.save() {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to update profile: {}", e)
        }));
    }

    // TODO: Broadcast avatar hash to peers

    Json(serde_json::json!({
        "status": "success",
        "message": "Avatar uploaded",
        "avatar_url": avatar_url
    }))
}

/// Get avatar by short_id
async fn api_avatar_get(PathExtractor(short_id): PathExtractor<String>) -> impl IntoResponse {
    let avatar_dir = dirs::home_dir()
        .expect("No home directory")
        .join(".yandi/avatars");

    // the name comes from the address: only a plain hex id may be used to build a file name
    if short_id.is_empty()
        || short_id.len() > 64
        || !short_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return StatusCode::NOT_FOUND.into_response();
    }

    // сначала локальная картинка, которую владелец поставил этому человеку, потом картинка, заданная им самим (профиль устройства)
    if let Some(png) = crate::local_overrides::avatar(&short_id) {
        return (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "image/png")], png).into_response();
    }
    if short_id.len() == 64 {
        if let Some(png) = crate::device_profile::avatar(&short_id) {
            return (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "image/png")], png).into_response();
        }
    }

    for ext in ["png", "jpg", "gif"] {
        let path = avatar_dir.join(format!("{}.{}", short_id, ext));
        if path.exists() {
            match tokio::fs::read(&path).await {
                Ok(data) => {
                    let content_type = match ext {
                        "png" => "image/png",
                        "jpg" => "image/jpeg",
                        "gif" => "image/gif",
                        _ => "image/png",
                    };
                    return (
                        StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, content_type)],
                        data,
                    )
                        .into_response();
                }
                Err(_) => continue,
            }
        }
    }

    let default_avatar = include_bytes!("ui/media/default-avatar.png");
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "image/png")],
        default_avatar.to_vec(),
    )
        .into_response()
}













/// Звонки телефон ↔ страница (сигналы через узел, звук и видео напрямую браузер ↔ телефон)
async fn phone_call_js_handler() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/javascript; charset=utf-8")
        .body(include_str!("ui/phone-call.js").to_owned())
        .unwrap()
}

/// Voice call script handler
async fn voice_call_handler() -> impl IntoResponse {
    let js = include_str!("ui/voice-call.js");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/javascript; charset=utf-8")
        .body(js.to_owned())
        .unwrap()
}

/// Video call script handler
async fn video_call_handler() -> impl IntoResponse {
    let js = include_str!("ui/video-call.js");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/javascript; charset=utf-8")
        .body(js.to_owned())
        .unwrap()
}

// ================== Hardening Step 3: pairing endpoints ==================

/// Загрузить или создать PairingPayload для текущей ноды. Anchor отдаёт payload
/// (anchor_id, x25519_pub_hex, TLS-fingerprint, anchor_url) в JSON или PNG-QR.
async fn current_pairing_payload(
    state: &AppState,
) -> Result<crate::netlayer::pairing::PairingPayload, String> {
    let transport = state
        .transport
        .as_ref()
        .ok_or_else(|| "P2P transport not available".to_string())?;
    let identity = transport.identity();
    let node_id = identity.node_id();
    let node_id_hex = hex::encode(&node_id.0[..8]);
    let x25519_pub_hex = hex::encode(&identity.public_key);

    // Подгружаем TLS-fingerprint из ~/.yandi/tls/.
    let tls = crate::netlayer::tls_cert::TlsIdentity::load_or_generate_default(&node_id_hex)
        .map_err(|e| format!("load TLS identity: {}", e))?;
    let fingerprint_hex = tls.fingerprint_hex.clone();

    // anchor_url: берём публичный IP если он есть, иначе host из конфига, иначе localhost.
    // Берём порт из ws-bind override / config.
    let ws_bind = crate::core::effective_ws_bind();
    let port = ws_bind
        .split(':')
        .last()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8443);
    let host =
        if state.node_info.external_ip != "unknown" && !state.node_info.external_ip.is_empty() {
            state.node_info.external_ip.clone()
        } else {
            "127.0.0.1".to_string()
        };
    let anchor_url = format!("wss://{}:{}/", host, port);

    Ok(crate::netlayer::pairing::PairingPayload {
        anchor_id: node_id,
        anchor_x25519_hex: x25519_pub_hex,
        fingerprint_hex,
        anchor_url,
    })
}

/// Сигналы звонков от телефонов для страницы чата (страница опрашивает раз в секунду; пока опрашивает — узел не отвечает «не на связи»).
async fn mobile_call_events(axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>) -> impl IntoResponse {
    let after = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let (events, last) = crate::mobile_self::poll_calls(after);
    let names: std::collections::HashMap<String, String> = crate::mobile_api::devices_overview()
        .into_iter()
        .filter_map(|d| Some((d["peer_id"].as_str()?.to_string(), d["name"].as_str().unwrap_or("Телефон").to_string())))
        .collect();
    let list: Vec<serde_json::Value> = events
        .into_iter()
        .map(|(seq, from, text)| serde_json::json!({"seq": seq, "from": from, "name": names.get(&from).cloned().unwrap_or_else(|| "Телефон".into()), "text": text}))
        .collect();
    let mut resp = Json(serde_json::json!({"events": list, "last": last})).into_response();
    resp.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    resp
}

#[derive(Deserialize)]
struct CallSignalReq {
    peer: String,
    text: String,
}

async fn mobile_call_signal(Json(req): Json<CallSignalReq>) -> impl IntoResponse {
    match crate::mobile_api::pc_call_signal(&req.peer, &req.text) {
        Ok(()) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": e})),
    }
}

#[derive(Deserialize)]
struct CallLogReq {
    peer: String,
    #[serde(default)]
    outgoing: bool,
    text: String,
}

async fn mobile_call_log(Json(req): Json<CallLogReq>) -> impl IntoResponse {
    match crate::mobile_api::pc_log_call(&req.peer, req.outgoing, &req.text) {
        Ok(()) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": e})),
    }
}

async fn mobile_call_turn() -> impl IntoResponse {
    Json(serde_json::json!({"turn": crate::mobile_api::pc_turn()}))
}

/// Устройства, сопряжённые с этим узлом: кто в сети, сколько соединений, когда был виден.
async fn mobile_devices_handler() -> impl IntoResponse {
    let mut resp =
        Json(serde_json::json!({"devices": crate::mobile_api::devices_overview()})).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

/// Забыть устройство. Если оно владеет группами, удаление уносит и их (журналы, файлы), поэтому нужен повторный ввод пароля узла,
/// как при прямом удалении группы; устройство без групп забывается без пароля.
/// Повысить приглашённое устройство до пользователя (оно сможет приглашать само). Админский маршрут.
async fn mobile_device_promote_handler(axum::extract::Path(id): axum::extract::Path<String>) -> Response {
    match crate::mobile_api::promote_device(&id) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response(),
        Err(m) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"status": "error", "message": m}))).into_response(),
    }
}

/// Отозвать устройство вместе со всеми, кого оно пригласило (и кого пригласили они). Админский маршрут.
async fn mobile_device_revoke_branch_handler(axum::extract::Path(id): axum::extract::Path<String>) -> Response {
    let removed = crate::mobile_api::remove_branch(&id);
    if removed == 0 {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"status": "error", "message": "нет такого устройства"}))).into_response();
    }
    (StatusCode::OK, Json(serde_json::json!({"status": "ok", "removed": removed}))).into_response()
}

async fn mobile_invite_policy_get() -> Response {
    Json(serde_json::to_value(crate::mobile_api::invite_policy()).unwrap_or_default()).into_response()
}

async fn mobile_invite_policy_post(Json(p): Json<crate::mobile_api::InvitePolicy>) -> Response {
    match crate::mobile_api::set_invite_policy(p) {
        Ok(()) => Json(serde_json::to_value(crate::mobile_api::invite_policy()).unwrap_or_default()).into_response(),
        Err(m) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": m}))).into_response(),
    }
}

async fn mobile_device_remove_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> Response {
    // права проверяются над полным номером: по сокращению владение группами не находится
    let Some(id) = crate::mobile_api::resolve_device(&id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"status": "error", "message": "нет такого устройства"}))).into_response();
    };
    let owned = crate::mobile_groups::owned_by(&id);
    if !owned.is_empty() {
        let password = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("password").and_then(|p| p.as_str()).map(str::to_string));
        if password.is_none() {
            let names: Vec<String> = owned.iter().map(|(_, name, _)| name.clone()).collect();
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"status": "error", "error": "password_required", "message": "устройство владеет группами; для удаления нужен пароль узла", "groups": names})),
            )
                .into_response();
        }
        if let Err(resp) = require_step_up(&state, password.as_deref()).await {
            return resp;
        }
    }
    if crate::mobile_api::remove_device(&id) {
        (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"status": "error", "message": "нет такого устройства"}))).into_response()
    }
}

/// QR для приложения на телефоне: адрес узла, порт TLS-входа, отпечаток сертификата и новый разовый код (живёт 5 минут).
fn complaint_err(c: StatusCode, m: String) -> Response {
    (c, Json(serde_json::json!({"error": m}))).into_response()
}

async fn complaints_list_handler() -> Response {
    Json(serde_json::json!({"complaints": crate::complaints::list()})).into_response()
}

async fn complaint_get_handler(axum::extract::Path(id): axum::extract::Path<String>) -> Response {
    match crate::complaints::get(&id) {
        Some(v) => Json(v).into_response(),
        None => complaint_err(StatusCode::NOT_FOUND, "жалобы нет".into()),
    }
}

#[derive(Deserialize)]
struct ComplaintReply {
    /// `reporter` — ответить заявителю, `target` — нарушителю (только если это пользователь)
    side: String,
    text: String,
}

async fn complaint_reply_handler(axum::extract::Path(id): axum::extract::Path<String>, Json(b): Json<ComplaintReply>) -> Response {
    let Some(c) = crate::complaints::find(&id) else { return complaint_err(StatusCode::NOT_FOUND, "жалобы нет".into()) };
    let to = match b.side.as_str() {
        "reporter" => c.reporter.clone(),
        "target" if c.target_kind == "user" => c.target_id.clone(),
        _ => return complaint_err(StatusCode::BAD_REQUEST, "адресат: заявитель или нарушитель-пользователь".into()),
    };
    if let Err((code, m)) = crate::complaints::add_message(&id, &b.side, "admin", &b.text) {
        return complaint_err(code, m);
    }
    // доставка от имени узла; если телефона уже нет, запись в журнале остаётся
    let delivered = crate::mobile_api::pc_admin_message(&to, &b.text).await.is_ok();
    Json(serde_json::json!({"ok": true, "delivered": delivered})).into_response()
}

#[derive(Deserialize)]
struct ComplaintStatus {
    status: String,
}

async fn complaint_status_handler(axum::extract::Path(id): axum::extract::Path<String>, Json(b): Json<ComplaintStatus>) -> Response {
    match crate::complaints::set_status(&id, &b.status) {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err((c, m)) => complaint_err(c, m),
    }
}

#[derive(Deserialize)]
struct ComplaintMeasure {
    /// `warn` | `restrict` | `remove` | `delete_group`
    kind: String,
    #[serde(default)]
    note: String,
    /// для `restrict`: на сколько часов (1–720)
    #[serde(default)]
    hours: u64,
    /// пароль узла: нужен мерам, которые удаляют группы (`delete_group`, и `remove` устройства-владельца групп)
    #[serde(default)]
    password: Option<String>,
}

async fn complaint_measure_handler(State(state): State<AppState>, axum::extract::Path(id): axum::extract::Path<String>, Json(b): Json<ComplaintMeasure>) -> Response {
    let Some(c) = crate::complaints::find(&id) else { return complaint_err(StatusCode::NOT_FOUND, "жалобы нет".into()) };
    let is_user = c.target_kind == "user";
    // меры, уничтожающие группы, требуют того же повторного ввода пароля, что и прямое удаление группы
    let destroys_groups = match b.kind.as_str() {
        "delete_group" => !is_user,
        "remove" => {
            let full = crate::mobile_api::resolve_device(&c.target_id).unwrap_or_else(|| c.target_id.clone());
            is_user && !crate::mobile_groups::owned_by(&full).is_empty()
        }
        _ => false,
    };
    if destroys_groups {
        if let Err(resp) = require_step_up(&state, b.password.as_deref()).await {
            return resp;
        }
    }
    let until = if b.kind == "restrict" { crate::complaints::now_ms() + b.hours.clamp(1, 720) * 3600 * 1000 } else { 0 };
    let result: Result<(), String> = match b.kind.as_str() {
        "warn" if is_user => {
            let text = if b.note.trim().is_empty() { "Предупреждение: ваше поведение нарушает правила узла. При повторении будут применены ограничения.".to_string() } else { b.note.clone() };
            let _ = crate::complaints::add_message(&id, "target", "admin", &text);
            crate::mobile_api::pc_admin_message(&c.target_id, &text).await.map_err(|e| format!("предупреждение записано, доставить не удалось: {e}"))
        }
        "restrict" if is_user => {
            let text = format!("Ограничение на {} ч.: создание групп и отправка файлов недоступны. {}", b.hours.clamp(1, 720), b.note);
            let _ = crate::complaints::add_message(&id, "target", "admin", &text);
            let _ = crate::mobile_api::pc_admin_message(&c.target_id, &text).await;
            Ok(())
        }
        "remove" if is_user => {
            if crate::mobile_api::remove_device(&c.target_id) { Ok(()) } else { Err("устройство не найдено или не удалено".into()) }
        }
        "delete_group" if !is_user => crate::mobile_groups::admin_delete_group(&c.target_id),
        _ => return complaint_err(StatusCode::BAD_REQUEST, "эта мера к этой жалобе не применима".into()),
    };
    if let Err(e) = crate::complaints::add_measure(&id, &b.kind, &b.note, until) {
        return complaint_err(e.0, e.1);
    }
    match result {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(m) => Json(serde_json::json!({"ok": true, "warning": m})).into_response(),
    }
}

async fn mobile_pairing_handler(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(port) = crate::mobile_tls::configured_port() else {
        return (StatusCode::CONFLICT, Json(serde_json::json!({"status": "error", "message": "Вход для телефона выключен: задайте порт в mobile_tls.json ({\"port\": 443}) или YANDI_MOBILE_TLS_PORT и перезапустите узел"}))).into_response();
    };
    let Some(transport) = state.transport.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status": "error", "message": "Транспорт не готов"})),
        )
            .into_response();
    };
    let node_hex = hex::encode(&transport.identity().node_id().0[..8]);
    let fp = match crate::netlayer::tls_cert::TlsIdentity::load_or_generate_default(&node_hex) {
        Ok(t) => t.fingerprint_hex,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"status": "error", "message": e.to_string()})),
            )
                .into_response()
        }
    };
    let host = q
        .get("host")
        .cloned()
        .filter(|h| !h.is_empty() && h.len() < 256)
        .unwrap_or_else(|| {
            if state.node_info.external_ip != "unknown" && !state.node_info.external_ip.is_empty() {
                state.node_info.external_ip.clone()
            } else {
                "127.0.0.1".to_string()
            }
        });
    // ?role=user — код для постороннего пользователя узла (без личной переписки владельца), по умолчанию телефон владельца
    let role = q.get("role").map(String::as_str).unwrap_or("owner");
    let text = crate::mobile_api::pairing_qr_json(&host, port, &fp, role);
    let svg = match qrcode::QrCode::new(text.as_bytes()) {
        Ok(qr) => qr
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(256, 256)
            .build(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"status": "error", "message": e.to_string()})),
            )
                .into_response()
        }
    };
    let mut resp = Json(serde_json::json!({"status": "ok", "host": host, "port": port, "fingerprint": fp, "expires_in_secs": 300, "qr_text": text, "qr_svg": svg})).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

async fn pair_qr_json_handler(State(state): State<AppState>) -> impl IntoResponse {
    match current_pairing_payload(&state).await {
        Ok(p) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ok",
                "payload": p,
                "qr_string": p.to_qr_string(),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "status": "error",
                "message": e,
            })),
        )
            .into_response(),
    }
}

async fn pair_qr_handler(State(state): State<AppState>) -> impl IntoResponse {
    let payload = match current_pairing_payload(&state).await {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }
    };
    let qr_string = payload.to_qr_string();
    // Рендерим QR в SVG (qrcode crate с default-features off → доступно svg-render через
    // встроенный SvgBuilder). Если по каким-то причинам недоступен — отдадим plain text.
    let qr = match qrcode::QrCode::new(qr_string.as_bytes()) {
        Ok(q) => q,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("qr encode failed: {}", e),
            )
                .into_response();
        }
    };
    let svg = qr
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .build();
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/svg+xml; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(svg)
        .unwrap()
        .into_response()
}

#[derive(Debug, Deserialize)]
struct PairIssueRequest {
    /// Hex-кодированный публичный ключ мобилки (Ed25519 32B).
    client_pubkey_hex: String,
    /// Опциональный TTL в секундах. По умолчанию DEFAULT_SESSION_TTL_SECS (7 дней).
    #[serde(default)]
    ttl_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
struct PairIssueResponse {
    status: String,
    /// Номер выданного права (не секрет). Секретов в ответе нет: рабочих ключей и секретов возобновления больше не существует.
    session_id: Option<String>,
    expires_at: Option<u64>,
    message: Option<String>,
}

async fn pair_issue_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PairIssueRequest>,
) -> impl IntoResponse {
    // 🔐 Защита от случайных POST'ов: проверяем что запрос идёт с того же хоста
    // (origin/referer указывают на нашу же web-UI) ИЛИ присутствует pre-shared
    // X-Pair-Token из конфига. В первом приближении — Origin/Host check.
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !origin.is_empty() {
        // the Origin's host must be one of this computer's names (a name that merely CONTAINS "localhost" is not)
        let origin_host = origin
            .split_once("://")
            .map(|(_, rest)| rest.split('/').next().unwrap_or(""))
            .unwrap_or("");
        let ok = host_allowed(origin_host);
        if !ok {
            return (
                StatusCode::FORBIDDEN,
                Json(PairIssueResponse {
                    status: "error".into(),
                    session_id: None,
                    expires_at: None,
                    message: Some(format!("origin {} blocked (host {})", origin, host)),
                }),
            )
                .into_response();
        }
    }

    let transport = match state.transport.as_ref() {
        Some(t) => t,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(PairIssueResponse {
                    status: "error".into(),
                    session_id: None,
                    expires_at: None,
                    message: Some("P2P transport not available".into()),
                }),
            )
                .into_response();
        }
    };

    // the key is stored and later used to look the client up: exactly 32 bytes of hex, nothing else
    if req.client_pubkey_hex.len() != 64
        || !req.client_pubkey_hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(PairIssueResponse {
                status: "error".into(),
                session_id: None,
                expires_at: None,
                message: Some("client_pubkey_hex must be 64 hex characters".into()),
            }),
        )
            .into_response();
    }
    let req = PairIssueRequest {
        client_pubkey_hex: req.client_pubkey_hex.to_ascii_lowercase(),
        ..req
    };

    let ttl = req
        .ttl_secs
        .unwrap_or(crate::netlayer::pairing::DEFAULT_SESSION_TTL_SECS);

    // Право клиента — запись без секретов (docs/RESUME_REDESIGN.md): кому и до какого срока. Срок ограничен годом.
    let ttl = ttl.min(365 * 24 * 3600);
    let path = crate::netlayer::pairing::default_paired_clients_path();
    let pk = req.client_pubkey_hex.clone();
    // транзакция над файлом: блокировка → строгое чтение → изменение копии → запись; заблокированное или переполненное хранилище не
    // меняется, память обновляется только после успешной записи
    let issued = tokio::task::spawn_blocking(move || {
        crate::netlayer::pairing::PairedClientStore::update(&path, |st| {
            st.gc_expired();
            if !st.clients.contains_key(&pk) && st.clients.len() >= crate::netlayer::pairing::MAX_CLIENTS {
                anyhow::bail!("слишком много выданных прав");
            }
            Ok(st.issue(pk.clone(), ttl))
        })
    })
    .await;
    let tok = match issued {
        Ok(Ok(g)) => g,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(PairIssueResponse { status: "error".into(), session_id: None, expires_at: None, message: Some(format!("persist failed: {}", e)) }),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(PairIssueResponse { status: "error".into(), session_id: None, expires_at: None, message: Some("internal error".into()) }),
            )
                .into_response();
        }
    };

    info!(
        "[pair/issue] issued grant {} for pubkey {}",
        tok.session_id_hex,
        &req.client_pubkey_hex[..16.min(req.client_pubkey_hex.len())]
    );

    (
        StatusCode::OK,
        Json(PairIssueResponse {
            status: "ok".into(),
            session_id: Some(tok.session_id_hex.clone()),
            expires_at: Some(tok.expires_at),
            message: None,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod locked_guard_tests {
    use super::is_owner_content_path as p;

    #[test]
    fn owner_content_paths_are_matched_exactly() {
        assert!(p("/api/groups"));
        assert!(p("/api/groups/abc/messages"));
        assert!(!p("/api/complaints/7/reply"), "жалобы — служебные данные узла под админ-сеансом, а не личное содержимое под паролем среды");
        assert!(p("/api/files/content/id/name.txt"));
        assert!(!p("/api/groupsx"));
        assert!(!p("/api/mobile/groups"));
        assert!(!p("/api/auth/login"));
    }
}

#[cfg(test)]
mod served_file_tests {
    use super::{safe_served_type, served_disposition};

    #[test]
    fn a_file_from_a_contact_is_never_served_as_active_content() {
        for name in ["evil.html", "evil.htm", "evil.svg", "evil.xhtml", "evil.xml", "evil.js", "evil.mjs", "evil.pdf", "x.html.txt.html"] {
            let t = safe_served_type(name);
            assert_eq!(t, "application/octet-stream", "{name} -> {t}");
            assert!(served_disposition(&t, name).starts_with("attachment;"), "{name}");
        }
    }

    #[test]
    fn pictures_sound_and_video_still_open_in_place() {
        for (name, ty) in [("a.png", "image/png"), ("a.jpg", "image/jpeg"), ("a.mp3", "audio/mpeg"), ("a.mp4", "video/mp4")] {
            assert_eq!(safe_served_type(name), ty);
            assert_eq!(served_disposition(ty, name), "inline");
        }
    }

    #[test]
    fn the_download_name_cannot_inject_header_syntax() {
        let d = served_disposition("application/octet-stream", "a\"; b=\r\nX: y.zip");
        assert!(!d.contains('\r') && !d.contains('\n') && d.matches('"').count() == 2, "{d}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod ui_socket_parity_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn real_handlers_match_tcp_without_weakening_tcp_auth_or_step_up() {
        let auth = crate::web::auth::AuthState::default();
        auth.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
        let cookie = format!("yandi_session={}", auth.create_session(false));
        let web = WebServer::new_with_auth(0, auth.clone());
        let app = web.create_router(web.state.clone()).with_state(web.state.clone());
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let endpoint = crate::web::ui_socket::Endpoint::bind(&data).unwrap();
        let unix = tokio::spawn(endpoint.serve(app.clone()));
        let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp_listener.local_addr().unwrap();
        let tcp = tokio::spawn(axum::serve(tcp_listener, app).into_future());
        let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let base = format!("http://{addr}");
        assert_eq!(client.get(format!("{base}/api/status")).send().await.unwrap().status(), StatusCode::SEE_OTHER);
        // A caller-supplied header is not the connection capability.
        assert_eq!(client.get(format!("{base}/api/status")).header("x-yandi-local-owner", "true").send().await.unwrap().status(), StatusCode::SEE_OTHER);
        assert_eq!(client.post(format!("{base}/api/node/start")).header("cookie", &cookie).header("origin", "http://evil.example").send().await.unwrap().status(), StatusCode::FORBIDDEN);
        for (method, path, body) in [("GET", "/api/auth/state", ""), ("GET", "/api/status", ""), ("GET", "/api/speed", ""), ("GET", "/api/network/relay", ""), ("GET", "/api/vault/status", ""), ("POST", "/api/node/start", ""), ("POST", "/api/mobile/groups/0123456789abcdef0123456789abcdef/delete", "{\"password\":\"wrong\"}")] {
            if path.ends_with("/delete") {
                // Exercise the actual StepUp backoff without reading the owner's key files.
                for _ in 0..10 { auth.record_login_failure(); }
            }
            let response = client.request(method.parse().unwrap(), format!("{base}{path}")).header("cookie", &cookie).header("content-type", "application/json").body(body).send().await.unwrap();
            let expected_status = response.status().as_u16();
            let expected: serde_json::Value = response.json().await.unwrap();
            let mut stream = tokio::net::UnixStream::connect(data.join("run/yandi-ui.sock")).await.unwrap();
            let request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut raw = Vec::new();
            tokio::time::timeout(std::time::Duration::from_secs(3), stream.read_to_end(&mut raw)).await.unwrap().unwrap();
            let text = String::from_utf8(raw).unwrap();
            let (head, payload) = text.split_once("\r\n\r\n").unwrap();
            assert!(head.starts_with(&format!("HTTP/1.1 {expected_status}")), "{path}: {head}");
            assert_eq!(serde_json::from_str::<serde_json::Value>(payload).unwrap(), expected, "{path}");
            // удаление группы — действие администратора: без его сеанса отказ одинаков и по TCP, и по сокету (права сокета сеанс не заменяют)
            if path.ends_with("/delete") { assert_eq!(expected_status, 403, "peer credentials must not replace the admin session"); assert_eq!(expected["error"], "admin_required"); }
        }
        unix.abort(); tcp.abort();
        let _ = unix.await; let _ = tcp.await;
    }
}

#[cfg(test)]
mod contacts_and_avatar_tests {
    use super::*;

    #[tokio::test]
    async fn contacts_list_is_saved_atomically_and_a_broken_file_is_an_error_not_an_empty_list() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("contacts.json");
        let p = path.to_str().unwrap();
        // нет файла — пустой список
        assert!(load_contacts_for_edit(p).await.unwrap().is_empty());
        // запись и чтение
        save_contacts(p, &[serde_json::json!({"id": "1", "name": "a"})]).await.unwrap();
        assert_eq!(load_contacts_for_edit(p).await.unwrap().len(), 1);
        // повреждённый файл: ошибка, и файл не тронут
        std::fs::write(&path, b"{not json").unwrap();
        assert!(load_contacts_for_edit(p).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
        // чужой формат (нет ключа contacts) — тоже ошибка
        std::fs::write(&path, b"{\"x\": 1}").unwrap();
        assert!(load_contacts_for_edit(p).await.is_err());
        // ошибка диска возвращается, а не «успех»
        let bad = d.path().join("a_file");
        std::fs::write(&bad, b"file, not a directory").unwrap();
        let under_file = bad.join("contacts.json");
        assert!(save_contacts(under_file.to_str().unwrap(), &[]).await.is_err());
    }

    #[test]
    fn avatar_files_of_other_types_and_a_cleared_avatar_are_removed() {
        let d = tempfile::tempdir().unwrap();
        for ext in ["png", "jpg", "gif"] {
            std::fs::write(d.path().join(format!("abc123.{ext}")), b"x").unwrap();
        }
        std::fs::write(d.path().join("def456.png"), b"other node").unwrap();
        remove_avatar_files_in(d.path(), "abc123", Some("jpg"));
        assert!(d.path().join("abc123.jpg").exists());
        assert!(!d.path().join("abc123.png").exists() && !d.path().join("abc123.gif").exists());
        remove_avatar_files_in(d.path(), "abc123", None);
        assert!(!d.path().join("abc123.jpg").exists());
        assert!(d.path().join("def456.png").exists(), "чужой аватар не тронут");
        // имя с путём не выводит за каталог
        std::fs::write(d.path().join("keep.png"), b"x").unwrap();
        remove_avatar_files_in(d.path(), "../keep", None);
        assert!(d.path().join("keep.png").exists());
    }
}

#[cfg(test)]
mod gateways_file_tests {
    use super::*;

    #[test]
    fn gateways_are_changed_under_a_lock_and_a_broken_file_is_not_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gateways.json");
        let ps = p.to_str().unwrap();
        update_gateways(ps, |g| g.push(serde_json::json!({"id": "1"}))).unwrap();
        update_gateways(ps, |g| g.push(serde_json::json!({"id": "2"}))).unwrap();
        assert_eq!(load_gateways(ps).unwrap().len(), 2);
        // валидный JSON с неверной схемой — тоже повреждение, а не пустой список
        std::fs::write(&p, br#"{"gateways":{"saved":[]}}"#).unwrap();
        assert!(load_gateways(ps).is_err(), "gateways не массив");
        assert!(update_gateways(ps, |g| g.clear()).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), br#"{"gateways":{"saved":[]}}"#.to_vec(), "файл не затёрт");
        // корень-массив и объект без поля gateways — тоже чужой документ
        for body in [&br#"[{"id":"1"}]"#[..], &br#"{"gatways":[]}"#[..], &br#"{}"#[..]] {
            std::fs::write(&p, body).unwrap();
            assert!(update_gateways(ps, |g| g.clear()).is_err(), "{}", String::from_utf8_lossy(body));
            assert_eq!(std::fs::read(&p).unwrap(), body.to_vec(), "файл не затёрт");
        }
        std::fs::write(&p, b"{broken").unwrap();
        assert!(update_gateways(ps, |g| g.clear()).is_err(), "повреждённый список не заменяется пустым");
        assert_eq!(std::fs::read(&p).unwrap(), b"{broken");
    }
}

#[cfg(test)]
mod step_up_tests {
    use super::*;

    #[test]
    fn calls_need_an_open_environment_but_can_always_be_ended() {
        for p in ["/api/media/call/start", "/api/media/video/call/start", "/api/media/ws/abc", "/api/media/incoming-call", "/api/media/call/x/accept", "/api/media/calls"] {
            assert!(is_locked_media_path(p), "{p}");
        }
        for p in ["/api/mobile/call/events", "/api/mobile/call/turn", "/api/mobile/call/log"] {
            assert!(is_locked_media_path(p), "{p}");
        }
        for p in ["/api/mobile/call/signal", "/api/media/call/end", "/api/media/call/x/end", "/api/media/call/x/reject", "/api/media/video/call/end", "/api/status"] {
            assert!(!is_locked_media_path(p), "{p}");
        }
    }

    /// Меры жалобы, уничтожающие группу, требуют пароль узла: без него — 403, с неверным — отказ, группа цела.
    #[tokio::test]
    async fn deleting_a_group_through_a_complaint_needs_the_node_password() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::complaints::tests::fresh();
        let id = crate::complaints::submit(&"a".repeat(64), "group", &"b".repeat(32), "G", "abuse", "плохая группа", &[]).unwrap();
        let auth = crate::web::auth::AuthState::default();
        auth.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
        let web = WebServer::new_with_auth(0, auth);
        let measure = |password: Option<&str>| ComplaintMeasure { kind: "delete_group".into(), note: String::new(), hours: 0, password: password.map(String::from) };
        let r = complaint_measure_handler(State(web.state.clone()), axum::extract::Path(id.clone()), Json(measure(None))).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "без пароля мера не выполняется");
        let r = complaint_measure_handler(State(web.state.clone()), axum::extract::Path(id.clone()), Json(measure(Some("точно неверный пароль")))).await;
        assert_ne!(r.status(), StatusCode::OK, "с неверным паролем — тоже нет");
        let c = crate::complaints::find(&id).unwrap();
        assert!(c.measures.is_empty(), "мера в журнале не появилась");
    }
    /// Прямой DELETE устройства-владельца групп по сокращённому номеру не обходит пароль.
    #[tokio::test]
    async fn removing_a_group_owner_by_id_prefix_needs_the_node_password() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let peer = crate::mobile_api::test_add_device("prefix-owner-token");
        let gid = crate::mobile_groups::with_store(|s| {
            s.create("Группа префикса", "тест", crate::mobile_groups::Kind::Open, &peer, &[]).unwrap()
        });
        let auth = crate::web::auth::AuthState::default();
        auth.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
        let web = WebServer::new_with_auth(0, auth);
        let r = mobile_device_remove_handler(State(web.state.clone()), axum::extract::Path(peer[..8].to_string()), axum::body::Bytes::new()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "префикс не должен обходить пароль");
        assert!(!crate::mobile_groups::owned_by(&peer).is_empty(), "группа цела");
        crate::mobile_groups::with_store(|s| { let _ = s.delete(&gid); });
        crate::mobile_api::remove_device(&peer);
    }

}

#[cfg(test)]
mod admin_guard_tests {
    use super::*;
    use tower::ServiceExt;

    async fn status_of(app: &Router, path: &str, token: Option<&str>) -> StatusCode {
        let mut r = axum::http::Request::get(path);
        if let Some(t) = token {
            r = r.header(crate::web::admin::HEADER, t);
        }
        app.clone().oneshot(r.body(axum::body::Body::empty()).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn admin_routes_answer_403_without_the_sessions_token_and_work_with_it() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::web::admin::reset_for_test();
        let app: Router = Router::new()
            .route("/api/complaints", get(|| async { "ok" }))
            .route("/api/guest-pairing", get(|| async { "ok" }))
            .route("/api/devices/pairing", get(|| async { "ok" }))
            .route("/api/settings", get(|| async { "ok" }).put(|| async { "ok" }))
            .route("/api/status", get(|| async { "ok" }))
            .route("/api/chats", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(admin_guard));
        assert_eq!(status_of(&app, "/api/complaints", None).await, StatusCode::FORBIDDEN);
        assert_eq!(status_of(&app, "/api/guest-pairing", None).await, StatusCode::FORBIDDEN);
        assert_eq!(status_of(&app, "/api/devices/pairing", None).await, StatusCode::FORBIDDEN);
        // чтение настроек личное, запись — управление узлом
        assert_eq!(status_of(&app, "/api/settings", None).await, StatusCode::OK);
        let put = app.clone().oneshot(axum::http::Request::put("/api/settings").body(axum::body::Body::empty()).unwrap()).await.unwrap().status();
        assert_eq!(put, StatusCode::FORBIDDEN);
        // личные маршруты без сеанса работают как раньше
        assert_eq!(status_of(&app, "/api/status", None).await, StatusCode::OK);
        assert_eq!(status_of(&app, "/api/chats", None).await, StatusCode::OK);
        let token = crate::web::admin::start();
        assert_eq!(status_of(&app, "/api/complaints", Some(&token)).await, StatusCode::OK);
        assert_eq!(status_of(&app, "/api/complaints", None).await, StatusCode::FORBIDDEN, "другой клиент без токена не пользуется чужим сеансом");
        assert_eq!(status_of(&app, "/api/complaints", Some("чужой")).await, StatusCode::FORBIDDEN);
        crate::web::admin::end();
        assert_eq!(status_of(&app, "/api/complaints", Some(&token)).await, StatusCode::FORBIDDEN, "после выхода снова закрыто");
        crate::web::admin::reset_for_test();
    }
}
