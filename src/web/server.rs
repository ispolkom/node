// src/web/server.rs
//!
//! # YANDI Web Server
//!
//! Локальный HTTP сервер для управления нодой

use axum::{
    body::Body,
    extract::{Path as PathExtractor, State, Multipart},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::{get, post, put, delete},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use tracing::{debug, info, error};

use crate::{MdnsService, DiscoveredNode};
use crate::core::profile::UserProfile;
use crate::netlayer::relay::{RelayManager, RelaySession, RelaySessionStatus};
use crate::web::media_api;

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
    /// Кэш обнаруженных нод (обновляется в фоне)
    pub discovered_nodes: Arc<Mutex<Vec<DiscoveredNode>>>,
    /// Информация о текущей ноде
    pub node_info: NodeInfo,
    /// Статус работы P2P ноды (true = online, false = offline)
    pub node_running: Arc<std::sync::atomic::AtomicBool>,
    /// Proxy response channel (for HTTP Proxy Client)
    pub proxy_resp_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::proxy::ProxyResponse)>>>>,
    /// Proxy tunnel data channel (for CONNECT bi-directional tunneling)
    pub proxy_tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::proxy::ProxyTunnelData)>>>>,
    /// Активный HTTP Proxy Client (хранится чтобы остановить)
    pub active_proxy_client: Arc<Mutex<Option<crate::proxy::HttpProxyClient>>>,
    /// Активный SOCKS5 Proxy Server (хранится чтобы остановить)
    pub active_socks5_proxy: Arc<Mutex<Option<crate::socks5::Socks5ProxyServer>>>,
    /// SOCKS5 proxy response channel
    pub socks5_resp_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::socks5::Socks5ProxyResponse)>>>>,
    /// SOCKS5 tunnel data channel
    pub socks5_tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::socks5::Socks5TunnelData)>>>>,
    /// Chat Manager для P2P чата
    pub chat_manager: Arc<Mutex<Option<std::sync::Arc<crate::communication::ChatManager>>>>,
    /// File Transfer Manager для чанкованной передачи файлов
    pub file_transfer_manager: Arc<Mutex<Option<std::sync::Arc<crate::communication::FileTransferManager>>>>,
    /// P2P Tunnel Manager для чистых P2P тоннелей
    pub p2p_tunnel_manager: Arc<Mutex<Option<crate::p2p_tunnel::P2PTunnelManager>>>,
    /// Group Manager для групп
    pub group_manager: Arc<Mutex<Option<std::sync::Arc<crate::communication::groups::GroupManager>>>>,
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
        let (media_signal_bus, _) = tokio::sync::broadcast::channel(256);
        Self {
            port,
            state: AppState {
                mdns: None,
                transport: None,
                p2p_transport: None,
                active_proxies: Arc::new(Mutex::new(std::collections::HashMap::new())),
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
                group_manager: Arc::new(Mutex::new(None)),
                media_manager: Arc::new(tokio::sync::Mutex::new(None)),
                media_signal_bus,
                incoming_calls: Arc::new(Mutex::new(std::collections::HashMap::new())),
                incoming_video_calls: Arc::new(Mutex::new(std::collections::HashMap::new())),
                auth_state: crate::web::auth::load_auth_state(),
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
                group_manager: Arc::new(Mutex::new(None)),
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
        proxy_resp_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::proxy::ProxyResponse)>>>>,
        proxy_tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::proxy::ProxyTunnelData)>>>>,
    ) -> Self {
        self.state.proxy_resp_rx = proxy_resp_rx;
        self.state.proxy_tunnel_rx = proxy_tunnel_rx;
        self
    }

    /// Установить SOCKS5 proxy channels
    pub fn with_socks5_channels(
        mut self,
        socks5_resp_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::socks5::Socks5ProxyResponse)>>>>,
        socks5_tunnel_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(crate::util::HashId, crate::socks5::Socks5TunnelData)>>>>,
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
    pub fn with_chat_manager(mut self, chat_manager: std::sync::Arc<crate::communication::ChatManager>) -> Self {
        self.state.chat_manager = Arc::new(Mutex::new(Some(chat_manager)));
        self
    }

    /// Установить File Transfer Manager
    pub fn with_file_transfer_manager(mut self, file_transfer_manager: std::sync::Arc<crate::communication::FileTransferManager>) -> Self {
        self.state.file_transfer_manager = Arc::new(Mutex::new(Some(file_transfer_manager)));
        self
    }

    /// Установить P2P Tunnel Manager
    pub fn with_p2p_tunnel_manager(mut self, tunnel_manager: crate::p2p_tunnel::P2PTunnelManager) -> Self {
        self.state.p2p_tunnel_manager = Arc::new(Mutex::new(Some(tunnel_manager)));
        self
    }

    /// Установить Group Manager
    

        pub fn with_media_manager(mut self, media_manager: crate::media::session::MediaSessionManager) -> Self {
        self.state.media_manager = Arc::new(tokio::sync::Mutex::new(Some(media_manager)));
        self
    }

    /// Set Group Manager for groups
    pub fn with_group_manager(mut self, group_manager: std::sync::Arc<crate::communication::groups::GroupManager>) -> Self {
    

        self.state.group_manager = Arc::new(Mutex::new(Some(group_manager)));
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
            .map_err(|e| format!("Server error: {}", e))?;

        Ok(())
    }

    /// Создать роутер
    fn create_router(&self, state_for_auth: AppState) -> Router<AppState> {
        Router::new()
            // HTML pages (protected)
            .route("/", get(index_handler))
            // кто может выходить в интернет через этот узел (выбор владельца)
            .merge(crate::exit_policy::router())
            // карточки узлов и списки сети по странам
            // доверенные узлы: визитка этого узла, добавить/убрать узел
            .merge(crate::web::peers::router())
            .merge(crate::network_offers::router())
            .merge(crate::hops_net::router())
            .merge(crate::relay_net::router())
            .merge(crate::supervisor::router())
            .route("/contacts", get(contacts_handler))
            .route("/gateways", get(gateways_handler))
            .route("/settings", get(settings_handler))
            .route("/chat", get(chat_handler))
            .route("/relay", get(relay_handler))
            .route("/groups", get(groups_handler))
            .route("/group-chat", get(group_chat_handler))
            // Static files (protected — served only to logged-in users)
            .route("/style.css", get(style_handler))
            .route("/chat.css", get(chat_css_handler))
            .route("/app.js", get(script_handler))
            .route("/speed.js", get(speed_script_handler))
            .route("/ui/voice-call.js", get(voice_call_handler))
            .route("/ui/video-call.js", get(video_call_handler))
            .route("/media/*path", get(media_handler))
            // API endpoints (protected)
            .route("/api/status", get(api_status))
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
            // Relay API endpoints
            .route("/api/relay/status", get(api_relay_status))
            .route("/api/relay/sessions", get(api_relay_sessions))
            .route("/api/relay/server/start", post(api_relay_server_start))
            .route("/api/relay/server/stop", post(api_relay_server_stop))
            .route("/api/relay/connect/:short_id", post(api_relay_connect))
            .route("/api/gateway/start", post(api_gateway_start))
            .route("/api/gateway/stop", post(api_gateway_stop))
            .route("/api/contacts", get(api_contacts_get).post(api_contacts_post).delete(api_contacts_delete))
            .route("/api/contacts/export", get(api_contacts_export))
            .route("/api/gateways", get(api_gateways_get).post(api_gateways_post).delete(api_gateways_delete))
            .route("/api/settings", get(api_settings_get).put(api_settings_put))
            // Chat API endpoints
            .route("/api/chat/send/:peer_id", post(api_chat_send))
            .route("/api/chat/history/:peer_id", get(api_chat_history))
            .route("/api/chat/clear/:peer_id", post(api_chat_clear))
            .route("/api/chat/edit/:peer_id", axum::routing::patch(api_chat_edit))
            .route("/api/chat/delete/:peer_id", axum::routing::delete(api_chat_delete))
            .route("/api/chats", get(api_chats_list))
            // Groups API endpoints
            .route("/api/groups", get(api_groups_get).post(api_groups_post))
            .route("/api/groups/sync", post(api_groups_sync))
            .route("/api/groups/dht/status", get(api_groups_dht_status))
            .route("/api/groups/:group_id", get(api_groups_get_one).delete(api_groups_delete).put(api_groups_put))
            .route("/api/groups/:group_id/publish", post(api_groups_publish))
            .route("/api/groups/:group_id/leave", post(api_groups_leave))
            .route("/api/groups/:group_id/members", post(api_groups_add_member).delete(api_groups_remove_member))
            // Group Chat API endpoints
            .route("/api/group-chat/send/:group_id", post(api_group_chat_send))
            .route("/api/group-chat/history/:group_id", get(api_group_chat_history))
            .route("/api/group-chat/clear/:group_id", post(api_group_chat_clear))
            // File Transfer API endpoints
            .route("/api/files/upload", post(api_files_upload))
            .route("/api/files/send-chunk/:peer_id", post(api_files_send_chunk))
            .route("/api/files/send-file/:peer_id", post(api_files_send_direct))
            .route("/api/files/send/:peer_id", post(api_files_send_direct))
            .route("/api/files/send/:peer_id/:filename", post(api_files_send_uploaded))
            .route("/api/files/content/:file_id/:filename", get(api_files_content))
            .route("/api/files/status/:file_id/:filename", get(api_files_status))
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
            .route("/api/media/ws/:peer_id", get(media_api::media_websocket_handler))
            .route("/api/media/incoming-call", get(media_api::get_incoming_call))
            .route("/api/media/call/:call_id/accept", post(media_api::accept_call))
            .route("/api/media/call/:call_id/reject", post(media_api::reject_call))
            .route("/api/media/call/end", post(media_api::end_active_call))
            // Video call endpoints
            .route("/api/media/video/call/start", post(media_api::start_video_call))
            .route("/api/media/video/incoming-call", get(media_api::get_incoming_video_call))
            .route("/api/media/video/call/:call_id/accept", post(media_api::accept_video_call))
            .route("/api/media/video/call/:call_id/reject", post(media_api::reject_video_call))
            .route("/api/media/video/call/end", post(media_api::end_active_video_call))
            // Pairing endpoints
            .route("/pair/qr", get(pair_qr_handler))
            .route("/pair/qr.json", get(pair_qr_json_handler))
            .route("/pair/issue", post(pair_issue_handler))
            // Apply auth middleware to all routes above
            .route_layer(axum::middleware::from_fn_with_state(
                state_for_auth,
                auth_middleware,
            ))
            // Public routes — not protected by auth middleware
            .route("/login", get(login_page_handler))
            .route("/setup", get(setup_page_handler))
            .route("/api/auth/login", post(api_auth_login))
            .route("/api/auth/setup", post(api_auth_setup))
            .route("/api/auth/logout", get(api_auth_logout))
            .route("/api/auth/rebind", post(api_auth_rebind))
            .route("/api/auth/recover", post(api_auth_recover))
            .layer(axum::middleware::from_fn(host_guard))
            .layer(TraceLayer::new_for_http())
    }
}

/// Можно ли обращаться к веб-интерфейсу под таким заголовком `Host`. Интерфейс слушает только этот компьютер;
/// чужой сайт через подмену DNS (DNS rebinding) заставляет браузер ходить на 127.0.0.1 под своим именем — такие запросы отклоняем.
pub fn host_allowed(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        // [::1]:порт
        match rest.split_once(']') { Some((ip, tail)) if tail.is_empty() || tail.starts_with(':') => ip, _ => return false }
    } else {
        host.rsplit_once(':').map(|(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { host }).unwrap_or(host)
    };
    matches!(name.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1")
}

async fn host_guard(req: axum::extract::Request, next: Next) -> Response {
    let ok = req.headers().get("host").and_then(|v| v.to_str().ok()).map(host_allowed).unwrap_or(false);
    if !ok {
        return (StatusCode::FORBIDDEN, "forbidden host").into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod host_guard_tests {
    use super::{host_allowed, media_name_ok};

    #[test]
    fn media_names_cannot_leave_the_media_directory() {
        for ok in ["ring.mp3", "sounds/ring.wav"] {
            assert!(media_name_ok(ok), "{ok}");
        }
        for bad in ["", "../secret", "a/../../b", "/etc/passwd", "..\\x", "a\0b", "./.."] {
            assert!(!media_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_this_computer_names_pass() {
        for ok in ["localhost", "localhost:8080", "127.0.0.1:9000", "127.0.0.1", "[::1]:8080", "[::1]", "LOCALHOST:1"] {
            assert!(host_allowed(ok), "{ok}");
        }
        for bad in ["", "evil.com", "evil.com:8080", "localhost.evil.com", "127.0.0.1.evil.com:80", "evil.com:127.0.0.1",
                    "192.168.1.5:8080", "[::2]:80", "[::1]x", "localhost:80:80", "0.0.0.0:80", "127.0.0.2"] {
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

async fn api_auth_login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Response {
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
    match crate::web::auth::verify_login(&body.login_password) {
        Ok(true) => {
            state.auth_state.record_login_success();
            let token = state.auth_state.create_session(body.remember_me);
            let cookie = crate::web::auth::make_session_cookie(&token, body.remember_me);
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("Set-Cookie", cookie.parse().unwrap());
            (StatusCode::OK, resp_headers, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Ok(false) => {
            state.auth_state.record_login_failure();
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Неверный пароль"})),
            ).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        ).into_response(),
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

async fn api_auth_setup(
    State(state): State<AppState>,
    Json(body): Json<SetupRequest>,
) -> Response {
    match crate::web::auth::setup_auth(&state.auth_state, &body.login_password, &body.login_password_repeat, &body.master_password, &body.master_password_repeat) {
        Ok(_master_key) => {
            let token = state.auth_state.create_session(false);
            let cookie = crate::web::auth::make_session_cookie(&token, false);
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("Set-Cookie", cookie.parse().unwrap());
            (StatusCode::OK, resp_headers, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        ).into_response(),
    }
}

async fn api_auth_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let cookie_str = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if let Some(token) = crate::web::auth::extract_session_token(cookie_str) {
        state.auth_state.invalidate_session(&token);
    }
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert("Set-Cookie", crate::web::auth::clear_session_cookie().parse().unwrap());
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
    // Verify login password first
    match crate::web::auth::verify_login(&body.login_password) {
        Ok(true) => { state.auth_state.record_login_success(); }
        Ok(false) => {
            state.auth_state.record_login_failure();
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Неверный пароль"})),
            ).into_response();
        }
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        ).into_response(),
    }

    match crate::web::auth::rebind_to_machine(&state.auth_state, &body.master_password) {
        Ok(()) => {
            let token = state.auth_state.create_session(body.remember_me);
            let session_cookie = crate::web::auth::make_session_cookie(&token, body.remember_me);
            let mut resp_headers = HeaderMap::new();
            resp_headers.append("Set-Cookie", session_cookie.parse().unwrap());
            resp_headers.append("Set-Cookie", "yandi_rebind=; Path=/; Max-Age=0".parse().unwrap());
            (StatusCode::OK, resp_headers, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        ).into_response(),
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
    let auth_state = state.auth_state.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::web::auth::recover_login(&auth_state, &body.recovery_code, &body.new_login_password).map(|_| body.remember_me)
    })
    .await
    .unwrap_or_else(|_| Err("Внутренняя ошибка".to_string()));
    match result {
        Ok(remember_me) => {
            state.auth_state.record_login_success();
            let token = state.auth_state.create_session(remember_me);
            let cookie = crate::web::auth::make_session_cookie(&token, remember_me);
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("Set-Cookie", cookie.parse().unwrap());
            (StatusCode::OK, resp_headers, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Err(e) => {
            state.auth_state.record_login_failure();
            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e}))).into_response()
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

async fn relay_handler() -> Html<&'static str> {
    Html(include_str!("ui/relay.html"))
}

async fn groups_handler() -> Html<&'static str> {
    Html(include_str!("ui/groups.html"))
}

async fn group_chat_handler() -> Html<&'static str> {
    Html(include_str!("ui/group-chat.html"))
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
    let is_running = state.node_running.load(std::sync::atomic::Ordering::Relaxed);
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
        "wagon_retrans_total": metrics.wagon_retrans_total,
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
    let self_short_id: Option<String> = state.transport.as_ref()
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
    let mdns_nodes: Vec<DiscoveredNode> = state.discovered_nodes.lock().await
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
            let already_in_mdns = mdns_nodes.iter()
                .any(|n| n.short_id == peer_cid);

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
    let all_nodes: Vec<serde_json::Value> = mdns_nodes.iter()
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
    PathExtractor(short_id): PathExtractor<String>
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
    PathExtractor(short_id): PathExtractor<String>
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
    let gateway_peer = match peers.iter().find(|p| &p.id.0[..8] == short_id_bytes.as_slice()) {
        Some(peer) => peer,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer with short ID {} not found", short_id)
            }));
        }
    };

    info!("🌐 Starting HTTP Proxy through gateway: {} ({})", short_id, gateway_peer.addr);

    // Проверяем - уже есть активный прокси?
    if state.active_proxies.lock().await.len() > 0 {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Proxy already running. Stop it first."
        }));
    }

    // Берём receivers
    let resp_rx = {
        let mut rx_lock = state.proxy_resp_rx.lock().await;
        rx_lock.take()
    };

    let tunnel_rx = {
        let mut rx_lock = state.proxy_tunnel_rx.lock().await;
        rx_lock.take()
    };

    if resp_rx.is_none() || tunnel_rx.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Proxy channels not available. May be already in use by CLI?"
        }));
    }

    // Создаём HttpProxyClient
    use crate::proxy::HttpProxyClient;

    let http_port = crate::core::get_config().ports.http_proxy;
    let mut proxy_config = crate::proxy::ProxyConfig::default();
    proxy_config.client_listen_addr = format!("127.0.0.1:{http_port}");
    let http_proxy = HttpProxyClient::new(
        transport.clone(),
        gateway_peer.id.clone()
    ).with_config(proxy_config);

    let http_proxy = http_proxy.with_response_channel(resp_rx.unwrap());
    let http_proxy = http_proxy.with_tunnel_data_channel(tunnel_rx.unwrap());

    // Регистрируем Station
    transport.set_station(http_proxy.station.clone()).await;

    // Запускаем прокси в фоне
    tokio::spawn(async move {
        if let Err(e) = http_proxy.start().await {
            eprintln!("❌ HTTP Proxy error: {}", e);
        }
    });

    // 🚨 ОТПРАВЛЯЕМ 0x30 ПАКЕТ НА GATEWAY ДЛЯ АВТОЗАПУСКА!
    info!("📤 Sending StartProxyGateway (0x30) packet to gateway {}", short_id);
    let cmd_bytes = vec![0x30u8]; // StartProxyGateway

    if let Err(e) = transport.send_encrypted(gateway_peer.id, &cmd_bytes).await {
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

    state.active_proxies.lock().await.insert(short_id.clone(), proxy_info);

    info!("✅ HTTP Proxy started on 127.0.0.1:{} through gateway {}", http_port, short_id);

    Json(serde_json::json!({
        "status": "success",
        "message": format!("Proxy started through {}", short_id),
        "local_port": 8080,
        "gateway": short_id
    }))
}

/// Остановить proxy
async fn api_proxy_stop(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>
) -> impl IntoResponse {
    // Удаляем из active_proxies
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
    PathExtractor(short_id): PathExtractor<String>
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
    let auto_country: Option<Option<String>> = if short_id == "auto" || short_id == "hops" || short_id == "rules" {
        Some(None)
    } else {
        short_id.strip_prefix("auto-").or_else(|| short_id.strip_prefix("hops-")).or_else(|| short_id.strip_prefix("rules-")).and_then(crate::network_offers::normalise_country).map(Some)
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
        match peers.iter().find(|p| &p.id.0[..8] == short_id_bytes.as_slice()) {
            Some(peer) => {
                info!("🧦 Starting SOCKS5 Proxy through gateway: {} ({})", short_id, peer.addr);
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

    // Берём receivers
    let resp_rx = {
        let mut rx_lock = state.socks5_resp_rx.lock().await;
        rx_lock.take()
    };

    let tunnel_rx = {
        let mut rx_lock = state.socks5_tunnel_rx.lock().await;
        rx_lock.take()
    };

    if resp_rx.is_none() || tunnel_rx.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "SOCKS5 channels not available. May be already in use by CLI?"
        }));
    }

    // Прокси для браузера — ТОЛЬКО на этом компьютере и со своим паролем узла. Раньше он открывался на внешнем адресе с общим для
    // всех паролем «yandi123»: любой в интернете мог гнать через него свой трафик в чужой выход.
    let socks_port = crate::core::get_config().ports.mobile_gateway;
    let socks_password = match crate::exit_policy::local_proxy_password() {
        Ok(p) => p,
        Err(e) => return Json(serde_json::json!({"status": "error", "message": format!("Не удалось создать пароль прокси: {e}")})),
    };

    // Создаём Socks5ProxyServer с авторизацией
    use crate::socks5::{Socks5ProxyServer, Socks5Config};

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
        listen_addr,  // ✅ Бинд на конкретный внешний IP
        auth_required: true,  // ✅ Обязательная авторизация
        username: Some("yandi".to_string()),
        password: Some(socks_password.clone()),
        enable_udp: false,  // UDP не поддерживаем в P2P режиме
    };

    let socks5_proxy = Socks5ProxyServer::new(socks5_config, transport.clone());

    let socks5_proxy = socks5_proxy
        .with_response_channel(resp_rx.unwrap())
        .with_tunnel_data_channel(tunnel_rx.unwrap());
    let socks5_proxy = match (&gateway_peer, &auto_country) {
        (Some(gw), _) => socks5_proxy.with_exit_node(gw.id.clone()),
        (None, Some(c)) => {
            let pool = crate::exit_select::start_pool(transport.clone(), c.clone());
            crate::exit_select::start_verifier(pool.clone(), listen_addr, socks_password.clone());
            let p = socks5_proxy.with_exit_pool(pool);
            let p = if rules_mode { p.with_rules(crate::route_rules::Rules::load()) } else { p };
            if hops_mode { p.with_hops() } else { p }
        }
        (None, None) => return Json(serde_json::json!({"status": "error", "message": "choose a gateway or automatic selection"})),
    };

    // Регистрируем Station
    transport.set_station(socks5_proxy.station.clone()).await;

    // Запускаем прокси в фоне (переменная перемещается в spawn)
    tokio::spawn(async move {
        if let Err(e) = socks5_proxy.run().await {
            eprintln!("❌ SOCKS5 Proxy error: {}", e);
        }
    });

    // 🚨 ОТПРАВЛЯЕМ 0x34 ПАКЕТ НА GATEWAY ДЛЯ АВТОЗАПУСКА SOCKS5 EXIT NODE! (в режиме auto это делает очередь выходов)
    if let Some(gateway_peer) = &gateway_peer {
        info!("📤 Sending StartSocks5Gateway (0x34) packet to gateway {}", short_id);
        let cmd_bytes = vec![0x34u8]; // StartSocks5Gateway

        if let Err(e) = transport.send_encrypted(gateway_peer.id, &cmd_bytes).await {
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

    state.active_proxies.lock().await.insert(short_id.clone(), proxy_info);

    info!("✅ SOCKS5 Proxy started on 127.0.0.1:{} through gateway {}", socks_port, short_id);

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

async fn api_socks5_personal(State(state): State<AppState>, Json(b): Json<PersonalBody>) -> impl IntoResponse {
    let err = |m: &str| Json(serde_json::json!({"status": "error", "message": m}));
    let Some(transport) = state.transport.as_ref() else { return err("P2P transport not available in standalone mode") };
    let hop = |h: &PersonalHop| -> Option<crate::hops::PathHop> {
        let id: [u8; 32] = hex::decode(&h.node).ok()?.try_into().ok()?;
        let kb: [u8; 32] = hex::decode(&h.key).ok()?.try_into().ok()?;
        Some(crate::hops::PathHop { id, key: ed25519_dalek::VerifyingKey::from_bytes(&kb).ok()? })
    };
    let Some(gateway) = hop(&PersonalHop { node: b.node.clone(), key: b.key.clone() }) else { return err("неверный номер или ключ шлюза") };
    let mut path = vec![];
    if let Some(r) = &b.relay {
        let Some(r) = hop(r) else { return err("неверный номер или ключ ретранслятора") };
        path.push(r);
    } else if let Some(r) = {
        // ретранслятор не указан: ищем по объявленной записи шлюза (подпись проверена ключом из сопряжения), берём связанного с нами
        let connected: std::collections::HashSet<String> = transport.get_peers().await.iter().map(|p| p.id.to_hex()).collect();
        let (cards, _) = crate::network_offers::directory_snapshot(None);
        crate::relay_net::lookup_relays(&b.node.to_ascii_lowercase(), &b.key.to_ascii_lowercase())
            .into_iter()
            .filter(|id| connected.contains(id))
            .find_map(|id| cards.iter().find(|c| c.node_id == id).and_then(|c| hop(&PersonalHop { node: c.node_id.clone(), key: c.key.clone() })))
    } {
        path.push(r);
    }
    path.push(gateway);
    let Some(secret) = hex::decode(&b.secret).ok().and_then(|v| <[u8; 32]>::try_from(v).ok()) else { return err("неверный секрет устройств") };
    if state.active_proxies.lock().await.len() > 0 {
        return err("Proxy already running. Stop it first.");
    }
    let socks_port = crate::core::get_config().ports.mobile_gateway;
    let socks_password = match crate::exit_policy::local_proxy_password() {
        Ok(p) => p,
        Err(e) => return err(&format!("Не удалось создать пароль прокси: {e}")),
    };
    use crate::socks5::{Socks5ProxyServer, Socks5Config};
    let Ok(listen_addr) = format!("127.0.0.1:{socks_port}").parse() else { return err("bad bind address") };
    let cfg = Socks5Config { listen_addr, auth_required: true, username: Some("yandi".to_string()), password: Some(socks_password.clone()), enable_udp: false };
    let proxy = Socks5ProxyServer::new(cfg, transport.clone()).with_personal(path, secret);
    tokio::spawn(async move {
        if let Err(e) = proxy.run().await {
            eprintln!("❌ SOCKS5 Proxy error: {}", e);
        }
    });
    state.active_proxies.lock().await.insert(
        "personal".to_string(),
        ProxyInfo { short_id: "personal".to_string(), proxy_type: ProxyType::Socks5, local_port: socks_port, started_at: chrono::Utc::now().to_rfc3339() },
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
    PathExtractor(short_id): PathExtractor<String>
) -> impl IntoResponse {
    // Удаляем из active_proxies
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
    let socks5_proxies: Vec<&ProxyInfo> = proxies.values()
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

// ═══════════════════════════════════════════════════════════════
// RELAY API HANDLERS
// ═══════════════════════════════════════════════════════════════

/// Получить статус relay сервера
async fn api_relay_status(State(state): State<AppState>) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    }

    let transport = state.transport.as_ref().unwrap();
    let relay_manager = transport.relay_manager.lock().await;

    let stats = relay_manager.get_stats();

    Json(serde_json::json!({
        "status": "success",
        "relay_server": relay_manager.is_relay_server(),
        "active_sessions": stats.active_sessions,
        "total_bytes_forwarded": stats.total_bytes_forwarded,
        "total_packets_forwarded": stats.total_packets_forwarded
    }))
}

/// Получить список активных relay сессий
async fn api_relay_sessions(State(state): State<AppState>) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    }

    let transport = state.transport.as_ref().unwrap();
    let relay_manager = transport.relay_manager.lock().await;

    let sessions: Vec<serde_json::Value> = relay_manager.get_active_sessions()
        .iter()
        .map(|s| {
            serde_json::json!({
                "session_id": s.session_id,
                "source_peer": hex::encode(&s.source_peer.0[..8]),
                "target_peer": hex::encode(&s.target_peer.0[..8]),
                "status": format!("{:?}", s.status),
                "bytes_forwarded": s.bytes_forwarded,
                "packets_forwarded": s.packets_forwarded,
                "idle_time_secs": s.idle_time().as_secs()
            })
        })
        .collect();

    Json(serde_json::json!({
        "status": "success",
        "sessions": sessions
    }))
}

/// Включить режим relay сервера
async fn api_relay_server_start(State(state): State<AppState>) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    }

    let transport = state.transport.as_ref().unwrap();
    let mut relay_manager = transport.relay_manager.lock().await;

    relay_manager.set_relay_server_mode(true);

    info!("🌐 Relay server mode ENABLED via Web UI");

    Json(serde_json::json!({
        "status": "success",
        "message": "Relay server mode enabled"
    }))
}

/// Выключить режим relay сервера
async fn api_relay_server_stop(State(state): State<AppState>) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    }

    let transport = state.transport.as_ref().unwrap();
    let mut relay_manager = transport.relay_manager.lock().await;

    relay_manager.set_relay_server_mode(false);

    info!("🌐 Relay server mode DISABLED via Web UI");

    Json(serde_json::json!({
        "status": "success",
        "message": "Relay server mode disabled"
    }))
}

/// Подключиться к пиру через relay сервер
async fn api_relay_connect(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>
) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
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
    let target_peer = match peers.iter().find(|p| &p.id.0[..8] == short_id_bytes.as_slice()) {
        Some(peer) => peer,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Peer with short ID {} not found", short_id)
            }));
        }
    };

    info!("🔌 Connecting to peer {} via relay", short_id);

    // Создаём relay сессию
    let my_id = transport.identity().node_id();
    let session_id = {
        let mut manager = transport.relay_manager.lock().await;
        manager.create_session(my_id, target_peer.id)
    };

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

    let request_bytes = match serde_json::to_vec(&request) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to serialize relay request: {}", e)
            }));
        }
    };

    let mut packet = vec![0x60u8]; // RelayConnectRequest
    packet.extend_from_slice(&request_bytes);

    if let Err(e) = transport.send_encrypted(target_peer.id, &packet).await {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to send relay request: {}", e)
        }));
    }

    info!("✅ Relay connection request sent to {}", short_id);

    Json(serde_json::json!({
        "status": "success",
        "message": format!("Relay connection request sent to {}", short_id),
        "session_id": session_id
    }))
}

/// Запустить Gateway mode (на этой ноде)
async fn api_gateway_start(State(state): State<AppState>) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available in standalone mode"
        }));
    }

    // TODO: Запустить HttpProxyGateway
    info!("Request to start Gateway mode");

    Json(serde_json::json!({
        "status": "success",
        "message": "Gateway mode started",
        "port": 8080
    }))
}

/// Остановить Gateway mode
async fn api_gateway_stop(State(state): State<AppState>) -> impl IntoResponse {
    // TODO: Остановить HttpProxyGateway
    info!("Request to stop Gateway mode");

    Json(serde_json::json!({
        "status": "success",
        "message": "Gateway mode stopped"
    }))
}

async fn api_start() -> impl IntoResponse {
    // TODO: Реально запустить ноду
    Json(serde_json::json!({
        "status": "success",
        "message": "Node started"
    }))
}

async fn api_stop() -> impl IntoResponse {
    // TODO: Реально остановить ноду
    Json(serde_json::json!({
        "status": "success",
        "message": "Node stopped"
    }))
}

async fn api_contacts_get(State(state): State<AppState>) -> impl IntoResponse {
    // Загружаем из contacts.json
    let contacts_path = "contacts.json";

    // Если файл не существует, возвращаем пустой список
    if !std::path::Path::new(contacts_path).exists() {
        return Json(serde_json::json!({
            "contacts": []
        }));
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
                            if let Some(short_id) = contact["short_id"].as_str() {
                                let is_online = online_peers.contains(short_id);
                                contact["online"] = serde_json::json!(is_online);
                            }
                        }
                    }
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
        Err(_) => {
            Json(serde_json::json!({
                "contacts": []
            }))
    }
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
    State(state): State<AppState>,
    axum::extract::Json(payload): axum::extract::Json<ContactRequest>
) -> impl IntoResponse {
    // Загружаем текущие контакты
    let contacts_path = "contacts.json";
    let mut contacts: Vec<serde_json::Value> = Vec::new();

    if std::path::Path::new(contacts_path).exists() {
        if let Ok(content) = tokio::fs::read_to_string(contacts_path).await {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(arr) = data.get("contacts").and_then(|v| v.as_array()) {
                    contacts = arr.clone();
                }
            }
        }
    }

    // Проверяем, это новый контакт или редактирование
    if let Some(id) = &payload.id {
        // Редактирование существующего контакта
        if let Some(contact) = contacts.iter_mut().find(|c| {
            c.get("id").and_then(|v| v.as_str()) == Some(id)
        }) {
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

    // Сохраняем в файл
    let data = serde_json::json!({
        "contacts": contacts
    });

    if let Ok(content) = serde_json::to_string_pretty(&data) {
        let _ = tokio::fs::write(contacts_path, content).await;
    }

    Json(serde_json::json!({
        "status": "success",
        "message": "Contact saved"
    }))
}

#[derive(Deserialize)]
struct DeleteContactRequest {
    id: String,
}

async fn api_contacts_delete(
    axum::extract::Json(payload): axum::extract::Json<DeleteContactRequest>
) -> impl IntoResponse {
    // Загружаем текущие контакты
    let contacts_path = "contacts.json";
    let mut contacts: Vec<serde_json::Value> = Vec::new();

    if std::path::Path::new(contacts_path).exists() {
        if let Ok(content) = tokio::fs::read_to_string(contacts_path).await {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(arr) = data.get("contacts").and_then(|v| v.as_array()) {
                    contacts = arr.clone();
                }
            }
        }
    }

    // Удаляем контакт
    contacts.retain(|c| {
        c.get("id").and_then(|v| v.as_str()) != Some(payload.id.as_str())
    });

    // Сохраняем в файл
    let data = serde_json::json!({
        "contacts": contacts
    });

    if let Ok(content) = serde_json::to_string_pretty(&data) {
        let _ = tokio::fs::write(contacts_path, content).await;
    }

    Json(serde_json::json!({
        "status": "success",
        "message": "Contact deleted"
    }))
}

/// Export contacts as a JSON file download (only short_id and name fields)
async fn api_contacts_export() -> impl IntoResponse {
    let contacts_path = "contacts.json";

    let contacts: Vec<serde_json::Value> = if std::path::Path::new(contacts_path).exists() {
        match tokio::fs::read_to_string(contacts_path).await {
            Ok(content) => serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|v| v.get("contacts").and_then(|c| c.as_array()).cloned())
                .unwrap_or_default()
                .into_iter()
                .map(|c| serde_json::json!({
                    "short_id": c.get("short_id").and_then(|v| v.as_str()).unwrap_or(""),
                    "name":     c.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                }))
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
        .header("Content-Disposition", "attachment; filename=\"yandi_contacts.json\"")
        .body(payload)
        .unwrap()
}

async fn api_gateways_get(State(state): State<AppState>) -> impl IntoResponse {
    // Загружаем из gateways.json
    let gateways_path = "gateways.json";

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

async fn api_gateways_post(
    axum::extract::Json(payload): axum::extract::Json<GatewayRequest>
) -> impl IntoResponse {
    // Загружаем текущие шлюзы
    let gateways_path = "gateways.json";
    let mut gateways: Vec<serde_json::Value> = Vec::new();

    if std::path::Path::new(gateways_path).exists() {
        if let Ok(content) = tokio::fs::read_to_string(gateways_path).await {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(arr) = data.get("gateways").and_then(|v| v.as_array()) {
                    gateways = arr.clone();
                }
            }
        }
    }

    // Проверяем, это новый шлюз или редактирование
    if let Some(id) = &payload.id {
        // Редактирование существующего шлюза
        if let Some(gateway) = gateways.iter_mut().find(|g| {
            g.get("id").and_then(|v| v.as_str()) == Some(id)
        }) {
            gateway["name"] = serde_json::json!(payload.name);
            gateway["short_id"] = serde_json::json!(payload.short_id);
            gateway["country"] = serde_json::json!(payload.country);
        }
    } else {
        // Добавление нового шлюза
        let new_gateway = serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "name": payload.name,
            "short_id": payload.short_id,
            "country": payload.country,
            "connected": false
        });
        gateways.push(new_gateway);
    }

    // Сохраняем в файл
    let data = serde_json::json!({
        "gateways": gateways
    });

    if let Ok(content) = serde_json::to_string_pretty(&data) {
        let _ = tokio::fs::write(gateways_path, content).await;
    }

    Json(serde_json::json!({
        "status": "success",
        "message": "Gateway saved"
    }))
}

async fn api_gateways_delete(
    axum::extract::Json(payload): axum::extract::Json<DeleteContactRequest>
) -> impl IntoResponse {
    // Загружаем текущие шлюзы
    let gateways_path = "gateways.json";
    let mut gateways: Vec<serde_json::Value> = Vec::new();

    if std::path::Path::new(gateways_path).exists() {
        if let Ok(content) = tokio::fs::read_to_string(gateways_path).await {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(arr) = data.get("gateways").and_then(|v| v.as_array()) {
                    gateways = arr.clone();
                }
            }
        }
    }

    // Удаляем шлюз
    gateways.retain(|g| {
        g.get("id").and_then(|v| v.as_str()) != Some(payload.id.as_str())
    });

    // Сохраняем в файл
    let data = serde_json::json!({
        "gateways": gateways
    });

    if let Ok(content) = serde_json::to_string_pretty(&data) {
        let _ = tokio::fs::write(gateways_path, content).await;
    }

    Json(serde_json::json!({
        "status": "success",
        "message": "Gateway deleted"
    }))
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
    axum::extract::Json(settings): axum::extract::Json<serde_json::Value>
) -> impl IntoResponse {
    use crate::update_config;

    // Получаем текущую конфигурацию
    let mut config = crate::get_config();

    // Обновляем порты если указаны (только 1..=65535; раньше число молча обрезалось до 16 бит)
    let port_of = |v: &serde_json::Value| v.as_u64().filter(|p| (1..=65535).contains(p)).map(|p| p as u16);
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
                    None => return Json(serde_json::json!({"status": "error", "message": format!("{key}: port must be a number from 1 to 65535")})),
                }
            }
        }
    }

    // Обновляем серверные настройки если указаны (адрес — настоящий IP, уровень журнала — из известных)
    if let Some(server) = settings.get("server") {
        if let Some(bind_address) = server.get("bind_address").and_then(|v| v.as_str()) {
            if bind_address.parse::<std::net::IpAddr>().is_err() {
                return Json(serde_json::json!({"status": "error", "message": "bind_address must be an IP address"}));
            }
            config.server.bind_address = bind_address.to_string();
        }
        if let Some(log_level) = server.get("log_level").and_then(|v| v.as_str()) {
            if !["error", "warn", "info", "debug", "trace"].contains(&log_level) {
                return Json(serde_json::json!({"status": "error", "message": "log_level must be error, warn, info, debug or trace"}));
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
async fn api_node_stop(State(state): State<AppState>) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available in standalone mode"
        }));
    }

    // Устанавливаем статус offline
    state.node_running.store(false, std::sync::atomic::Ordering::Relaxed);

    // TODO: Вызвать transport.shutdown() для закрытия сокетов
    // Пока меняем только статус

    Json(serde_json::json!({
        "status": "success",
        "message": "Node stopped",
        "node_status": "offline"
    }))
}

/// Запустить P2P ноду
async fn api_node_start(State(state): State<AppState>) -> impl IntoResponse {
    if state.transport.is_none() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available in standalone mode"
        }));
    }

    // Устанавливаем статус online
    state.node_running.store(true, std::sync::atomic::Ordering::Relaxed);

    // TODO: Вызвать transport.restart() для переоткрытия сокетов
    // Пока меняем только статус

    Json(serde_json::json!({
        "status": "success",
        "message": "Node started",
        "node_status": "online"
    }))
}

// === Chat API Endpoints ===

/// Отправить сообщение
async fn api_chat_send(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "Chat manager not initialized"));

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

    // Подготовить attachment если есть
    let attachment = req.attachment.map(|att| crate::communication::FileAttachment {
        filename: att.filename,
        size: att.size,
        mime_type: att.mime_type,
        data: att.data,
        file_ref: att.file_ref.map(|file_ref| crate::communication::FileReference {
            file_id: file_ref.file_id,
            total_chunks: file_ref.total_chunks,
            local_name: file_ref.local_name,
        }),
    });

    // Отправить сообщение
    match chat_manager.send_message_with_attachment(peer_hash, req.text, attachment).await {
        Ok(msg) => {
            Json(serde_json::json!({
                "status": "success",
                "message": "Message sent",
                "msg_id": hex::encode(&msg.msg_id.0[..8]),
                "timestamp": msg.timestamp
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to send message: {}", e)
            }))
        }
    }
}

/// Загрузить историю чата
async fn api_chat_history(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "Chat manager not initialized"));

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

    // println!("🔍 Loading history for short_id: {}, peer_hash: {}", peer_id, hex::encode(&peer_hash.0[..8]));
    // Загрузить историю (последние 100 сообщений)
    match chat_manager.load_history(&peer_hash, 100) {
        Ok(messages) => {
            // Конвертировать HashId в hex строку для JSON (полные 32 байта)
            let messages_json: Vec<serde_json::Value> = messages.into_iter().map(|msg| {
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
                    "attachment": msg.attachment
                })
            }).collect();

            Json(serde_json::json!({
                "status": "success",
                "peer_id": peer_id,
                "messages": messages_json
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to load history: {}", e)
            }))
        }
    }
}

/// Очистить историю чата
async fn api_chat_clear(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "Chat manager not initialized"));

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
    match chat_manager.clear_history(&peer_hash) {
        Ok(_) => {
            Json(serde_json::json!({
                "status": "success",
                "message": "Chat history cleared"
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to clear history: {}", e)
            }))
        }
    }
}

/// Редактировать сообщение
async fn api_chat_edit(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    Json(req): Json<EditMessageRequest>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "Chat manager not initialized"));

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

    // Редактировать сообщение
    match chat_manager.edit_message(&peer_hash, &msg_id, req.text) {
        Ok(_) => {
            Json(serde_json::json!({
                "status": "success",
                "message": "Message edited"
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to edit message: {}", e)
            }))
        }
    }
}

/// Удалить сообщение
async fn api_chat_delete(
    State(state): State<AppState>,
    PathExtractor(peer_id): PathExtractor<String>,
    Json(req): Json<DeleteMessageRequest>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "Chat manager not initialized"));

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
        match chat_manager.delete_message_for_everyone(&peer_hash, &msg_id).await {
            Ok(_) => {
                Json(serde_json::json!({
                    "status": "success",
                    "message": "Message deleted for everyone"
                }))
            }
            Err(e) => {
                Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Failed to delete message: {}", e)
                }))
            }
        }
    } else {
        // Удалить только у себя
        match chat_manager.delete_message_local(&peer_hash, &msg_id) {
            Ok(_) => {
                Json(serde_json::json!({
                    "status": "success",
                    "message": "Message deleted locally"
                }))
            }
            Err(e) => {
                Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Failed to delete message: {}", e)
                }))
            }
        }
    }
}

/// Получить список всех чатов
async fn api_chats_list(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let chat_manager = state.chat_manager.lock().await;
    let chat_manager = chat_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "Chat manager not initialized"));

    if let Err(_) = chat_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Chat manager not available"
        }));
    }

    let chat_manager = chat_manager.unwrap();

    // Получить список чатов
    match chat_manager.list_chats() {
        Ok(peer_ids) => {
            let chats: Vec<String> = peer_ids.iter()
                .map(|id| hex::encode(&id.0[..8]))
                .collect();

            Json(serde_json::json!({
                "status": "success",
                "chats": chats
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to list chats: {}", e)
            }))
        }
    }
}

/// Request для отправки сообщения
#[derive(Deserialize)]
struct SendMessageRequest {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment: Option<MessageAttachment>,
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
    let safe_filename = format!("{}_{timestamp}", filename.replace("/", "_").replace("\\", "_"));
    let file_path = uploads_dir.join(&safe_filename);

    // Сохраняем файл
    if let Err(e) = tokio::fs::write(&file_path, file_data.clone()).await {
        error!("❌ Failed to save file: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to save file: {}", e)
        }));
    }

    let file_size = file_data.len();

    info!("📤 File uploaded: {} ({} bytes) -> {:?}", filename, file_size, file_path);

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
    let temp_path = temp_dir.join(format!("{}__{}", transfer_id, sanitize_storage_filename(&filename)));
    if let Err(e) = std::fs::write(&temp_path, &file_data) {
        error!("❌ Failed to write file to disk: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to write file to disk: {}", e)
        }));
    }

    info!("📤 Sending file '{}' ({} bytes) to {} directly from memory",
        filename, file_size, crate::util::mask_hash_id(&peer_hash));

    match file_transfer_manager.start_file_transfer_from_disk_with_id(
        peer_hash,
        Some(transfer_id.clone()),
        filename.clone(),
        temp_path,
        mime_type.clone()
    ).await {
        Ok(file_id) => {
            info!("✅ File transfer started: {} (file_id: {})", filename, file_id);
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
static CHUNK_UPLOADS: LazyLock<TokioMutex<HashMap<String, PendingFileUpload>>> = LazyLock::new(|| TokioMutex::new(HashMap::new()));

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

    if chunk_data.len() > BROWSER_UPLOAD_CHUNK_SIZE {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!(
                "Chunk too large: got {} bytes, max {} bytes",
                chunk_data.len(),
                BROWSER_UPLOAD_CHUNK_SIZE
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
        format!("{}_{}",
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
    let max_chunks = (crate::communication::MAX_FILE_TRANSFER_SIZE as usize / BROWSER_UPLOAD_CHUNK_SIZE + 1) as u32;
    if total_chunks == 0 || total_chunks > max_chunks || file_size > crate::communication::MAX_FILE_TRANSFER_SIZE {
        return Json(serde_json::json!({"status": "error", "message": "File is larger than the allowed size"}));
    }
    // the id is used in a file name: only plain characters
    if upload_id.is_empty() || upload_id.len() > 80 || !upload_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Json(serde_json::json!({"status": "error", "message": "Invalid file_id"}));
    }

    // Сохраняем чанк сразу в temp-файл
    let mut uploads = CHUNK_UPLOADS.lock().await;
    // forget abandoned uploads (and their temp files), refuse too many at once
    uploads.retain(|_, u| {
        let alive = u.started.elapsed() < PENDING_UPLOAD_TTL;
        if !alive {
            let _ = std::fs::remove_file(&u.temp_path);
        }
        alive
    });
    if !uploads.contains_key(&upload_id) && uploads.len() >= MAX_PENDING_UPLOADS {
        return Json(serde_json::json!({"status": "error", "message": "Too many unfinished uploads"}));
    }
    let mut created = false;

    let upload = uploads.entry(upload_id.clone()).or_insert_with(|| {
        created = true;
        PendingFileUpload {
            filename: filename.clone(),
            mime_type: mime_type.clone(),
            total_chunks,
            temp_path: uploads_dir.join(format!("{}__{}", upload_id, sanitize_storage_filename(&filename))),
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

    let write_result = (|| -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&upload.temp_path)?;
        let offset = (chunk_index as u64) * (BROWSER_UPLOAD_CHUNK_SIZE as u64);
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&chunk_data)?;
        Ok(())
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

    info!("📦 Received chunk {}/{} for '{}' ({} bytes)",
        chunk_index + 1, total_chunks, upload.filename, chunk_data.len());

    // Проверяем: все ли чанки получены?
    let is_complete = upload.received_count == upload.total_chunks;
    let file_id = upload_id.clone();
    let received_count = upload.received_count;
    let upload_filename = upload.filename.clone();
    let upload_mime = upload.mime_type.clone();
    let temp_path = upload.temp_path.clone();
    let upload_total_chunks = upload.total_chunks;

    drop(uploads);

    if created {
        if let Err(e) = file_transfer_manager.register_streaming_transfer(
            peer_hash,
            Some(file_id.clone()),
            upload_filename.clone(),
            temp_path.clone(),
            upload_mime.clone(),
            file_size,
            upload_total_chunks,
        ).await {
            error!("❌ Failed to register streaming transfer: {}", e);
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to register streaming transfer: {}", e)
            }));
        }
    }

    if let Err(e) = file_transfer_manager.send_streaming_chunk(peer_hash, &file_id, chunk_index, &chunk_data).await {
        error!("❌ Failed to stream chunk via P2P: {}", e);
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to stream chunk via P2P: {}", e)
        }));
    }

    if is_complete {
        let file_size = match std::fs::metadata(&temp_path) {
            Ok(meta) => meta.len(),
            Err(e) => {
                error!("❌ Failed to stat temp upload file: {}", e);
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Failed to stat temp upload file: {}", e)
                }));
            }
        };

        info!("✅ All chunks received! File size: {} bytes", file_size);

        match file_transfer_manager.finalize_streaming_transfer(peer_hash, &file_id).await {
            Ok(_) => {
                info!("✅ Streaming P2P transfer finalized for file {}", upload_filename);
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
    let file_data = match tokio::fs::read(&file_path).await {
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

    info!("📤 Sending file '{}' ({} bytes) to {} with chunking",
        filename, file_data.len(), crate::util::mask_hash_id(&peer_hash));

    // Начать чанкованную передачу файла
    let transfer_id = format!(
        "{}_{}",
        hex::encode(&peer_hash.0[..8]),
        uuid::Uuid::new_v4().simple()
    );
    match file_transfer_manager.start_file_transfer_from_disk_with_id(
        peer_hash,
        Some(transfer_id),
        filename.clone(),
        file_path.clone(),
        mime_type
    ).await {
        Ok(file_id) => {
            info!("✅ File transfer started: {} (file_id: {})", filename, file_id);
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

fn sanitize_storage_filename(filename: &str) -> String {
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
    let is_reserved = matches!(trimmed.to_ascii_lowercase().as_str(),
        "con" | "prn" | "aux" | "nul" |
        "com1" | "com2" | "com3" | "com4" | "com5" | "com6" | "com7" | "com8" | "com9" |
        "lpt1" | "lpt2" | "lpt3" | "lpt4" | "lpt5" | "lpt6" | "lpt7" | "lpt8" | "lpt9"
    );

    if trimmed.is_empty() || is_reserved {
        "file".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn resolve_local_file_path(file_id: &str, filename: &str) -> Option<std::path::PathBuf> {
    // SEC-07: validate file_id — only alphanumeric, underscores, and hyphens
    if file_id.is_empty() || !file_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
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

    match std::fs::read(&file_path) {
        Ok(data) => {
            let content_type = mime_guess::from_path(&filename)
                .first_or_octet_stream()
                .to_string();
            let data_len = data.len();
            let is_streaming_media = content_type.starts_with("audio/") || content_type.starts_with("video/");

            if let Some(range_header) = headers
                .get(axum::http::header::RANGE)
                .and_then(|v| v.to_str().ok())
            {
                debug!(
                    "📡 File content request: file_id={}, filename={}, range={}, media={}",
                    file_id,
                    filename,
                    range_header,
                    is_streaming_media
                );

                if !is_streaming_media {
                    if let Some((start, mut end)) = parse_single_http_range(range_header, data_len) {
                        if start >= data_len {
                            return Response::builder()
                                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                                .header(axum::http::header::CONTENT_RANGE, format!("bytes */{}", data_len))
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
                                .header(axum::http::header::CONTENT_TYPE, content_type.clone())
                                .header(axum::http::header::ACCEPT_RANGES, "bytes")
                                .header(axum::http::header::CONTENT_LENGTH, chunk.len().to_string())
                                .header(axum::http::header::CONTENT_RANGE, format!("bytes {}-{}/{}", start, end, data_len))
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
    let tunnel_manager = tunnel_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "P2P Tunnel manager not initialized"));

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
        Ok(_tunnel) => {
            Json(serde_json::json!({
                "status": "success",
                "message": format!("P2P tunnel request sent to {}", short_id),
                "peer": short_id,
                "tunnel_type": format!("{:?}", tunnel_type)
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to start tunnel: {}", e)
            }))
        }
    }
}

/// Stop P2P tunnel with peer
async fn api_tunnel_stop(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    let tunnel_manager = state.p2p_tunnel_manager.lock().await;
    let tunnel_manager = tunnel_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "P2P Tunnel manager not initialized"));

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
        Ok(_) => {
            Json(serde_json::json!({
                "status": "success",
                "message": format!("P2P tunnel with {} closed", short_id)
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to close tunnel: {}", e)
            }))
        }
    }
}

/// List all active P2P tunnels
async fn api_tunnel_list(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let tunnel_manager = state.p2p_tunnel_manager.lock().await;
    let tunnel_manager = tunnel_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "P2P Tunnel manager not initialized"));

    if let Err(_) = tunnel_manager {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P Tunnel manager not available"
        }));
    }

    let tunnel_manager = tunnel_manager.unwrap();

    // Get tunnel list
    let tunnels = tunnel_manager.list_tunnels().await;

    let tunnel_list: Vec<serde_json::Value> = tunnels.iter()
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
    let tunnel_manager = tunnel_manager.as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "P2P Tunnel manager not initialized"));

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
        None => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("No active tunnel with {}", short_id)
            }))
        }
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
    let peers_json: Vec<_> = peers.into_iter().map(|peer| {
        serde_json::json!({
            "short_id": hex::encode(&peer.id.0[..8]),
            "node_id": hex::encode(&peer.id.0),
            "discovery_addr": peer.addr,
            "data_addr": peer.p2p_data_addr,
            "nat_status": peer.nat_status.as_str(),
        })
    }).collect();

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
        && std::path::Path::new(path).components().all(|c| matches!(c, std::path::Component::Normal(_)))
}

async fn media_handler(PathExtractor(path): PathExtractor<String>) -> impl IntoResponse {
    if !media_name_ok(&path) {
        return (StatusCode::NOT_FOUND, [(axum::http::header::CONTENT_TYPE, "text/plain")], Vec::<u8>::new());
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
            (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, content_type)], data)
        }
        Err(_) => {
            (StatusCode::NOT_FOUND, [(axum::http::header::CONTENT_TYPE, "text/plain")], Vec::<u8>::new())
        }
    }
}
// === Groups API Handlers ===

/// Получить список всех групп пользователя
async fn api_groups_get(State(state): State<AppState>) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };

    let groups = group_manager.get_my_groups().await;
    
    let groups_json: Vec<serde_json::Value> = groups.into_iter().map(|g| {
        let members: Vec<serde_json::Value> = g.members.values().map(|m| {
            serde_json::json!({
                "node_id": hex::encode(&m.node_id.0),
                "short_id": m.short_id,
                "nickname": m.nickname,
                "role": format!("{:?}", m.role),
                "joined_at": m.joined_at
            })
        }).collect();
        
        serde_json::json!({
            "id": g.id.to_hex(),
            "name": g.name,
            "description": g.description,
            "member_count": g.members.len(),
            "members": members,
            "settings": {
                "is_private": g.settings.is_private,
                "is_encrypted": g.settings.is_encrypted,
                "max_members": g.settings.max_members,
                "allow_files": g.settings.allow_files,
                "allow_voice": g.settings.allow_voice
            },
            "created_at": g.created_at,
            "version": g.version
        })
    }).collect();
    
    Json(serde_json::json!({
        "status": "success",
        "groups": groups_json
    }))
}

/// Создать новую группу
async fn api_groups_post(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let description = payload.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let is_private = payload.get("is_private").and_then(|v| v.as_bool()).unwrap_or(true);
    let is_encrypted = payload.get("is_encrypted").and_then(|v| v.as_bool()).unwrap_or(true);
    
    if name.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Group name is required"
        }));
    }
    
    // Получаем ID текущей ноды
    let my_node_id = if let Some(transport) = &state.transport {
        transport.identity().node_id()
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    let settings = crate::communication::groups::GroupSettings {
        is_private,
        is_encrypted,
        ..Default::default()
    };
    
    let group = group_manager.create_group(name, description, my_node_id, settings).await;
    let group_hash = group.id.to_hex();
    
    if let Some(transport) = &state.transport {
        if let Err(e) = group_manager.store_group_in_dht_signed(&group, transport, &transport.identity()).await {
            tracing::warn!("Failed to store group in DHT: {}", e);
        }
    }
    
    Json(serde_json::json!({
        "status": "success",
        "message": "Group created",
        "group_hash": group_hash,
        "group": {
            "id": group_hash,
            "name": group.name,
            "description": group.description,
            "member_count": group.members.len()
        }
    }))
}

/// Получить информацию о конкретной группе
async fn api_groups_get_one(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let group = match group_manager.get_group(&group_id).await {
        Some(g) => g,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group not found"
            }));
        }
    };
    
    let members: Vec<serde_json::Value> = group.members.values().map(|m| {
        serde_json::json!({
            "node_id": hex::encode(&m.node_id.0),
            "short_id": m.short_id,
            "nickname": m.nickname,
            "role": format!("{:?}", m.role),
            "joined_at": m.joined_at,
            "last_seen": m.last_seen
        })
    }).collect();
    
    Json(serde_json::json!({
        "status": "success",
        "id": group.id.to_hex(),
        "name": group.name,
        "description": group.description,
        "members": members,
        "member_count": members.len(),
        "settings": {
            "is_private": group.settings.is_private,
            "is_encrypted": group.settings.is_encrypted,
            "max_members": group.settings.max_members,
            "allow_files": group.settings.allow_files,
            "allow_voice": group.settings.allow_voice
        },
        "created_by": hex::encode(&group.created_by.0),
        "created_at": group.created_at,
        "version": group.version
    }))
}

/// Добавить участника в группу
async fn api_groups_add_member(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let short_id = payload.get("short_id").and_then(|v| v.as_str()).unwrap_or("");
    let role_str = payload.get("role").and_then(|v| v.as_str()).unwrap_or("Member");
    
    if short_id.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Short ID is required"
        }));
    }
    
    // Находим пира по short_id
    let peer_id = if let Some(transport) = &state.transport {
        match transport.find_peer_by_short_id(short_id) {
            Some(id) => id,
            None => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer with short ID {} not found", short_id)
                }));
            }
        }
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    let role = match role_str {
        "Admin" => crate::communication::groups::GroupRole::Admin,
        "Moderator" => crate::communication::groups::GroupRole::Moderator,
        "Member" => crate::communication::groups::GroupRole::Member,
        _ => crate::communication::groups::GroupRole::Member,
    };
    
    // Получаем ID текущей ноды (кто добавляет)
    let my_node_id = if let Some(transport) = &state.transport {
        transport.identity().node_id()
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    let member = crate::communication::groups::GroupMember::new(peer_id, short_id.to_string(), role);
    
    match group_manager.add_member(&group_id, member, &my_node_id).await {
        Ok(()) => {
            // Если есть DHT transport, обновляем группу в DHT
            if let Some(transport) = &state.transport {
                if let Some(group) = group_manager.get_group(&group_id).await {
                    let _ = group_manager.store_group_in_dht_signed(&group, transport, &transport.identity()).await;
                }
            }
            
            Json(serde_json::json!({
                "status": "success",
                "message": "Member added"
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": e
            }))
        }
    }
}

/// Удалить участника из группы
async fn api_groups_remove_member(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let short_id = payload.get("short_id").and_then(|v| v.as_str()).unwrap_or("");
    
    if short_id.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Short ID is required"
        }));
    }
    
    // Находим пира по short_id
    let peer_id = if let Some(transport) = &state.transport {
        match transport.find_peer_by_short_id(short_id) {
            Some(id) => id,
            None => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Peer with short ID {} not found", short_id)
                }));
            }
        }
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    // Получаем ID текущей ноды (кто удаляет)
    let my_node_id = if let Some(transport) = &state.transport {
        transport.identity().node_id()
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    match group_manager.remove_member(&group_id, &peer_id, &my_node_id).await {
        Ok(()) => {
            // Если есть DHT transport, обновляем группу в DHT
            if let Some(transport) = &state.transport {
                if let Some(group) = group_manager.get_group(&group_id).await {
                    let _ = group_manager.store_group_in_dht_signed(&group, transport, &transport.identity()).await;
                }
            }
            
            Json(serde_json::json!({
                "status": "success",
                "message": "Member removed"
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": e
            }))
        }
    }
}

// === Group Chat API Handlers ===

/// Отправить сообщение в группу
async fn api_group_chat_send(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    
    if text.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Message text is required"
        }));
    }
    
    // Получаем ID текущей ноды (отправитель)
    let my_node_id = if let Some(transport) = &state.transport {
        transport.identity().node_id()
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    match group_manager.send_message(&group_id, my_node_id, crate::communication::groups::GroupMessageType::Text(text)).await {
        Ok(msg) => {
            Json(serde_json::json!({
                "status": "success",
                "message": "Message sent",
                "msg_id": hex::encode(&msg.msg_id.0[..8]),
                "timestamp": msg.timestamp
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": e
            }))
        }
    }
}

/// Получить историю сообщений группы
async fn api_group_chat_history(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let limit = 100; // TODO: из query параметра
    let messages = group_manager.get_messages(&group_id, limit).await;
    
    let messages_json: Vec<serde_json::Value> = messages.into_iter().map(|msg| {
        serde_json::json!({
            "msg_id": hex::encode(&msg.msg_id.0),
            "from": hex::encode(&msg.from.0),
            "from_short": hex::encode(&msg.from.0[..8]),
            "timestamp": msg.timestamp,
            "text": match &msg.msg_type {
                crate::communication::groups::GroupMessageType::Text(t) => t,
                _ => "",
            },
            "msg_type": format!("{:?}", msg.msg_type),
            "edited_at": msg.edited_at,
            "deleted": msg.deleted
        })
    }).collect();
    
    Json(serde_json::json!({
        "status": "success",
        "group_id": group_id_hex,
        "messages": messages_json,
        "count": messages_json.len()
    }))
}

/// Очистить историю сообщений группы
async fn api_group_chat_clear(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    match group_manager.clear_history(&group_id).await {
        Ok(()) => {
            Json(serde_json::json!({
                "status": "success",
                "message": "Chat history cleared"
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": e
            }))
        }
    }
}

// === Profile API Handlers ===

/// Get current user profile
async fn api_profile_get(State(state): State<AppState>) -> impl IntoResponse {
    let short_id = state.node_info.short_id.clone();
    
    match UserProfile::load() {
        Ok(Some(profile)) => {
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
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to load profile: {}", e)
            }))
        }
    }
}

/// Update current user profile
async fn api_profile_put(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let short_id = state.node_info.short_id.clone();
    let display_name = payload.get("display_name").and_then(|v| v.as_str()).map(String::from);
    let avatar = payload.get("avatar").and_then(|v| v.as_str()).map(String::from);
    
    let mut profile = match UserProfile::load() {
        Ok(Some(p)) => p,
        _ => UserProfile::new(short_id),
    };
    
    profile.update(display_name, avatar);
    
    match profile.save() {
        Ok(()) => {
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
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to save profile: {}", e)
            }))
        }
    }
}

/// Get profile by short_id (from contacts cache)
async fn api_profile_get_by_short_id(
    State(state): State<AppState>,
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    let contacts_path = "contacts.json";
    
    if !std::path::Path::new(contacts_path).exists() {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Contact not found"
        }));
    }
    
    match tokio::fs::read_to_string(contacts_path).await {
        Ok(content) => {
            match serde_json::from_str::<serde_json::Value>(&content) {
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
                Err(_) => {
                    Json(serde_json::json!({
                        "status": "error",
                        "message": "Failed to parse contacts"
                    }))
                }
            }
        }
        Err(_) => {
            Json(serde_json::json!({
                "status": "error",
                "message": "Failed to read contacts"
            }))
        }
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
                }.to_string();
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
        return Json(serde_json::json!({"status": "error", "message": "Avatar is larger than 4 MB"}));
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

    // Save file
    let avatar_path = avatar_dir.join(format!("{}.{}", short_id, file_ext));
    if let Err(e) = tokio::fs::write(&avatar_path, &file_data).await {
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to save avatar: {}", e)
        }));
    }
    
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
async fn api_avatar_get(
    PathExtractor(short_id): PathExtractor<String>,
) -> impl IntoResponse {
    let avatar_dir = dirs::home_dir()
        .expect("No home directory")
        .join(".yandi/avatars");

    // the name comes from the address: only a plain hex id may be used to build a file name
    if short_id.is_empty() || short_id.len() > 64 || !short_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::NOT_FOUND.into_response();
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
                    return (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, content_type)], data).into_response();
                }
                Err(_) => continue,
            }
        }
    }
    
    let default_avatar = include_bytes!("ui/media/default-avatar.png");
    (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "image/png")], default_avatar.to_vec()).into_response()
}


/// Synchronize groups with DHT
async fn api_groups_sync(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let transport = match &state.transport {
        Some(t) => t,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available for DHT sync"
            }));
        }
    };
    
    let local_groups = group_manager.get_my_groups().await;
    let mut synced_count = 0;
    
    for group in &local_groups {
        if let Err(e) = group_manager.store_group_in_dht_signed(group, transport, &transport.identity()).await {
            tracing::warn!("Failed to sync group {} to DHT: {}", group.id.to_hex(), e);
        } else {
            synced_count += 1;
        }
    }
    
    Json(serde_json::json!({
        "status": "success",
        "found_groups": 0,
        "new_groups": 0,
        "updated_groups": synced_count,
        "message": format!("Synced {} groups to DHT", synced_count)
    }))
}


/// Get DHT status for groups
async fn api_groups_dht_status(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let local_groups = group_manager.get_my_groups().await;
    let local_count = local_groups.len();
    
    Json(serde_json::json!({
        "status": "success",
        "groups_in_dht": 0,
        "local_groups": local_count,
        "last_sync": chrono::Utc::now().to_rfc3339(),
        "active_peers": 0
    }))
}


/// Publish group to DHT
async fn api_groups_publish(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let transport = match &state.transport {
        Some(t) => t,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "P2P transport not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let group = match group_manager.get_group(&group_id).await {
        Some(g) => g,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group not found"
            }));
        }
    };
    
    match group_manager.store_group_in_dht_signed(&group, transport, &transport.identity()).await {
        Ok(()) => {
            Json(serde_json::json!({
                "status": "success",
                "message": format!("Group {} published to DHT", group_id_hex)
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to publish group: {}", e)
            }))
        }
    }
}


/// Delete group
async fn api_groups_delete(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let groups_dir = dirs::home_dir()
        .expect("No home directory")
        .join(".yandi/data/groups");
    
    let group_dir = groups_dir.join(&group_id_hex);
    if group_dir.exists() {
        let _ = tokio::fs::remove_dir_all(group_dir).await;
    }
    
    Json(serde_json::json!({
        "status": "success",
        "message": format!("Group {} deleted", group_id_hex)
    }))
}


/// Update group (PUT)
async fn api_groups_put(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let is_private = payload.get("is_private").and_then(|v| v.as_bool());
    
    let my_node_id = if let Some(transport) = &state.transport {
        transport.identity().node_id()
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    if let Some(is_private_val) = is_private {
        let _ = group_manager.update_settings(&group_id, &my_node_id, |s| {
            s.is_private = is_private_val;
        }).await;
    }
    
    Json(serde_json::json!({
        "status": "success",
        "message": "Group updated"
    }))
}


/// Leave group
async fn api_groups_leave(
    State(state): State<AppState>,
    PathExtractor(group_id_hex): PathExtractor<String>,
) -> impl IntoResponse {
    let group_manager = state.group_manager.lock().await;
    let group_manager = match group_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "Group manager not available"
            }));
        }
    };
    
    let group_id = match crate::communication::groups::GroupId::from_hex(&group_id_hex) {
        Ok(id) => id,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Invalid group ID: {}", e)
            }));
        }
    };
    
    let my_node_id = if let Some(transport) = &state.transport {
        transport.identity().node_id()
    } else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "P2P transport not available"
        }));
    };
    
    match group_manager.remove_member(&group_id, &my_node_id, &my_node_id).await {
        Ok(()) => {
            Json(serde_json::json!({
                "status": "success",
                "message": "Left group successfully"
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to leave group: {}", e)
            }))
        }
    }
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
async fn current_pairing_payload(state: &AppState) -> Result<crate::netlayer::pairing::PairingPayload, String> {
    let transport = state.transport.as_ref().ok_or_else(|| "P2P transport not available".to_string())?;
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
    let port = ws_bind.split(':').last().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8443);
    let host = if state.node_info.external_ip != "unknown" && !state.node_info.external_ip.is_empty() {
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

async fn pair_qr_json_handler(State(state): State<AppState>) -> impl IntoResponse {
    match current_pairing_payload(&state).await {
        Ok(p) => (StatusCode::OK, Json(serde_json::json!({
            "status": "ok",
            "payload": p,
            "qr_string": p.to_qr_string(),
        }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "status": "error",
            "message": e,
        }))).into_response(),
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
            return (StatusCode::INTERNAL_SERVER_ERROR,
                    format!("qr encode failed: {}", e)).into_response();
        }
    };
    let svg = qr.render::<qrcode::render::svg::Color>()
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
    session_id: Option<String>,
    resume_secret_hex: Option<String>,
    expires_at: Option<u64>,
    /// hex(32B) session-key — mobile сохраняет одновременно с resume_secret.
    session_key_hex: Option<String>,
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
    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin = headers.get("origin").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !origin.is_empty() {
        // the Origin's host must be one of this computer's names (a name that merely CONTAINS "localhost" is not)
        let origin_host = origin.split_once("://").map(|(_, rest)| rest.split('/').next().unwrap_or("")).unwrap_or("");
        let ok = host_allowed(origin_host);
        if !ok {
            return (StatusCode::FORBIDDEN, Json(PairIssueResponse {
                status: "error".into(),
                session_id: None, resume_secret_hex: None, expires_at: None, session_key_hex: None,
                message: Some(format!("origin {} blocked (host {})", origin, host)),
            })).into_response();
        }
    }

    let transport = match state.transport.as_ref() {
        Some(t) => t,
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(PairIssueResponse {
                status: "error".into(),
                session_id: None, resume_secret_hex: None, expires_at: None, session_key_hex: None,
                message: Some("P2P transport not available".into()),
            })).into_response();
        }
    };

    // the key is stored and later used to look the client up: exactly 32 bytes of hex, nothing else
    if req.client_pubkey_hex.len() != 64 || !req.client_pubkey_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return (StatusCode::BAD_REQUEST, Json(PairIssueResponse {
            status: "error".into(),
            session_id: None, resume_secret_hex: None, expires_at: None, session_key_hex: None,
            message: Some("client_pubkey_hex must be 64 hex characters".into()),
        })).into_response();
    }
    let req = PairIssueRequest { client_pubkey_hex: req.client_pubkey_hex.to_ascii_lowercase(), ..req };

    let ttl = req.ttl_secs.unwrap_or(crate::netlayer::pairing::DEFAULT_SESSION_TTL_SECS);

    // Генерируем session_key прямо здесь (32 случайных байта). Anchor должен
    // отдать тот же session_key мобилке через 0xC2 при первом encrypted connect'е.
    use rand::RngCore;
    let mut session_key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut session_key);

    let tok = crate::netlayer::pairing::SessionToken::new_with_session_key(ttl, &session_key);

    // Сохраняем в paired_clients store (key = client_pubkey_hex).
    let mut store = transport.paired_clients.lock().await;
    store.clients.insert(req.client_pubkey_hex.clone(), tok.clone());
    let path = crate::netlayer::pairing::default_paired_clients_path();
    if let Err(e) = store.save(&path) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(PairIssueResponse {
            status: "error".into(),
            session_id: None, resume_secret_hex: None, expires_at: None, session_key_hex: None,
            message: Some(format!("persist failed: {}", e)),
        })).into_response();
    }

    info!("[pair/issue] issued session {:#x} for pubkey {}",
          tok.session_id, &req.client_pubkey_hex[..16.min(req.client_pubkey_hex.len())]);

    (StatusCode::OK, Json(PairIssueResponse {
        status: "ok".into(),
        session_id: Some(format!("{:#x}", tok.session_id)),
        resume_secret_hex: Some(tok.resume_secret_hex.clone()),
        expires_at: Some(tok.expires_at),
        session_key_hex: Some(hex::encode(session_key)),
        message: None,
    })).into_response()
}
