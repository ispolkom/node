//! Программный вход для приложения на телефоне: `/mobile/*` по HTTPS и сокет сообщений (договорённость — docs/CLIENT_WIRE.md).
//!
//! Это тонкий слой поверх готового чата узла: сам чат, шифрование между узлами и доставка остаются как есть. Слой работает на том
//! же TLS-входе, что и прокси для телефона (`mobile_tls`): первый байт 0x05 — прокси, запрос `/mobile/...` — сюда, всё прочее — сайт-
//! маскировка. Отпечаток сертификата телефон берёт из QR (его показывает страница настроек).
//!
//! Доступ: разовый код спаривания (6 цифр, живёт 5 минут, 5 неверных попыток сжигают его) меняется на токен устройства; на узле хранится
//! только хэш токена. Входящие для устройства — это сообщения от других людей новее последнего подтверждённого (подтверждение хранится в
//! файле устройств), поэтому они переживают перезапуск узла.
//!
//! Между телефоном и узлом шифрует TLS; между узлами — обычное сквозное шифрование чата. Своего слоя шифрования от телефона до
//! собеседника здесь пока нет: ключей `/mobile/pubkey` узел не выдаёт, приложение шлёт обычный текст внутри TLS.
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::communication::{ChatManager, ChatMessage};
use crate::util::HashId;

const PAIRING_TTL: Duration = Duration::from_secs(300);
const PAIRING_MAX_FAILS: u32 = 5;
const MAX_DEVICES: usize = 16;
const MAX_TEXT: usize = 64 * 1024;

const FT_PING: u8 = 0x01;
const FT_PONG: u8 = 0x02;
const FT_CHAT_MSG: u8 = 0x10;
const FT_SEND_MSG: u8 = 0x30;

pub struct MobileState {
    chat: Arc<ChatManager>,
    p2p: Arc<crate::p2p::P2PTransport>,
    my_id: HashId,
}

static STATE: OnceLock<Arc<MobileState>> = OnceLock::new();
static BUS: OnceLock<tokio::sync::broadcast::Sender<Arc<ChatMessage>>> = OnceLock::new();
static PAIRING: Mutex<Option<Pairing>> = Mutex::new(None);
static DEVICES: Mutex<Option<Vec<Device>>> = Mutex::new(None);

struct Pairing {
    code: String,
    expires: Instant,
    fails: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct Device {
    /// sha256 токена, hex: сам токен на узле не хранится
    token_hash: String,
    name: String,
    created_ms: u64,
    /// всё, что от других людей и не новее этой метки (мс), телефон уже получил
    acked_ts: u64,
}

fn bus() -> &'static tokio::sync::broadcast::Sender<Arc<ChatMessage>> {
    BUS.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// Чат вызывает это после сохранения входящего сообщения: подключённые телефоны получат его сразу.
pub fn publish(msg: &ChatMessage) {
    let _ = bus().send(Arc::new(msg.clone()));
}

pub fn init(chat: Arc<ChatManager>, p2p: Arc<crate::p2p::P2PTransport>, my_id: HashId) {
    let _ = STATE.set(Arc::new(MobileState { chat, p2p, my_id }));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn devices_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("mobile_devices.json")
}

fn with_devices<R>(f: impl FnOnce(&mut Vec<Device>) -> R) -> R {
    let mut g = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| std::fs::read_to_string(devices_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    let r = f(list);
    if let Ok(s) = serde_json::to_string_pretty(&*list) {
        let p = devices_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = crate::util::private_file::write_private(&p, s.as_bytes());
    }
    r
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Новый разовый код спаривания (6 цифр); предыдущий перестаёт действовать.
pub fn issue_pairing_code() -> String {
    use rand::Rng;
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
    *PAIRING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pairing { code: code.clone(), expires: Instant::now() + PAIRING_TTL, fails: 0 });
    code
}

/// Содержимое QR для приложения.
pub fn pairing_qr_json(host: &str, port: u16, fingerprint_hex: &str) -> String {
    json!({"host": host, "port": port, "pairing_code": issue_pairing_code(), "tls_fingerprint": fingerprint_hex, "tls": true}).to_string()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[derive(Deserialize)]
struct PairReq {
    pairing_code: String,
    #[serde(default)]
    device_name: String,
}

async fn pair(Json(req): Json<PairReq>) -> Response {
    let ok = {
        let mut g = PAIRING.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_mut() {
            Some(p) if Instant::now() < p.expires && p.fails < PAIRING_MAX_FAILS => {
                if same(&p.code, req.pairing_code.trim()) {
                    *g = None;
                    true
                } else {
                    p.fails += 1;
                    false
                }
            }
            _ => false,
        }
    };
    if !ok {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "bad or expired pairing code"}))).into_response();
    }
    use rand::RngCore;
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let name: String = req.device_name.chars().take(64).collect();
    with_devices(|d| {
        if d.len() >= MAX_DEVICES {
            d.remove(0);
        }
        d.push(Device { token_hash: token_hash(&token), name, created_ms: now_ms(), acked_ts: now_ms() });
    });
    Json(json!({"token": token})).into_response()
}

#[derive(Clone)]
struct DeviceKey(String);

async fn auth(Query(q): Query<std::collections::HashMap<String, String>>, mut req: Request, next: Next) -> Response {
    let bearer = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::to_string);
    let Some(token) = bearer.or_else(|| q.get("token").cloned()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let h = token_hash(&token);
    if !with_devices(|d| d.iter().any(|x| same(&x.token_hash, &h))) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    req.extensions_mut().insert(DeviceKey(h));
    next.run(req).await
}

async fn resolve_hex(st: &MobileState, id: &str) -> Option<HashId> {
    if let Ok(h) = HashId::from_hex(id) {
        return Some(h);
    }
    st.p2p.find_peer_by_short_id(id).await
}

async fn info(State(st): State<Arc<MobileState>>) -> Json<serde_json::Value> {
    let id = hex::encode(st.my_id.0);
    Json(json!({"node_id": id, "name": format!("YANDI {}", &id[..8]), "version": crate::VERSION}))
}

async fn contacts(State(st): State<Arc<MobileState>>) -> Json<serde_json::Value> {
    let known: Vec<HashId> = st.p2p.list_peers().await.into_iter().map(|p| p.id).collect();
    let raw: serde_json::Value = std::fs::read_to_string("contacts.json").ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({"contacts": []}));
    let mut out = Vec::new();
    for c in raw["contacts"].as_array().cloned().unwrap_or_default() {
        let short = c["short_id"].as_str().unwrap_or("").to_string();
        if short.is_empty() {
            continue;
        }
        let full = known.iter().find(|k| hex::encode(&k.0[..8]) == short);
        out.push(json!({
            "peer_id": full.map(|k| hex::encode(k.0)).unwrap_or_else(|| short.clone()),
            "display_name": c["name"].as_str().unwrap_or(&short),
            "online": full.is_some(),
            "is_manual": true,
        }));
    }
    Json(json!({"contacts": out}))
}

fn msg_json(m: &ChatMessage) -> serde_json::Value {
    json!({"id": hex::encode(m.msg_id.0), "from_peer_id": hex::encode(m.from.0), "text": m.text, "ts_ms": m.timestamp})
}

async fn history(State(st): State<Arc<MobileState>>, Path(peer): Path<String>, Query(q): Query<std::collections::HashMap<String, String>>) -> Json<serde_json::Value> {
    let limit = q.get("limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(50).clamp(1, 500);
    let Some(h) = resolve_hex(&st, &peer).await else { return Json(json!({"messages": []})) };
    let mut msgs = st.chat.load_history(&h, limit).unwrap_or_default();
    msgs.sort_by_key(|m| m.timestamp);
    let skip = msgs.len().saturating_sub(limit);
    Json(json!({"messages": msgs.iter().skip(skip).map(msg_json).collect::<Vec<_>>()}))
}

#[derive(Deserialize)]
struct SendReq {
    text: String,
}

async fn send(State(st): State<Arc<MobileState>>, Path(peer): Path<String>, Json(req): Json<SendReq>) -> Response {
    if req.text.is_empty() || req.text.len() > MAX_TEXT {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(h) = resolve_hex(&st, &peer).await else { return StatusCode::NOT_FOUND.into_response() };
    match st.chat.send_message(h, req.text).await {
        Ok(m) => Json(json!({"id": hex::encode(m.msg_id.0), "ts_ms": m.timestamp})).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Сообщения других людей, пришедшие после подтверждённого устройством. Номер сообщения — его время (мс).
async fn inbox(State(st): State<Arc<MobileState>>, axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Query(q): Query<std::collections::HashMap<String, String>>) -> Json<serde_json::Value> {
    let limit = q.get("limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(200).clamp(1, 1000);
    let acked = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.acked_ts).unwrap_or(0));
    let mut out: Vec<(u64, serde_json::Value)> = Vec::new();
    for peer in st.chat.list_chats().unwrap_or_default() {
        for m in st.chat.load_history(&peer, 200).unwrap_or_default() {
            if m.from != st.my_id && m.timestamp > acked {
                out.push((m.timestamp, json!({"id": m.timestamp, "from_peer_id": hex::encode(m.from.0), "payload_b64": base64_of(m.text.as_bytes()), "ts_ms": m.timestamp})));
            }
        }
    }
    out.sort_by_key(|(t, _)| *t);
    out.truncate(limit);
    Json(json!({"messages": out.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}))
}

fn base64_of(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[derive(Deserialize)]
struct AckReq {
    ids: Vec<u64>,
}

async fn inbox_ack(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<AckReq>) -> StatusCode {
    let Some(max) = req.ids.iter().copied().max() else { return StatusCode::OK };
    with_devices(|d| {
        if let Some(x) = d.iter_mut().find(|x| same(&x.token_hash, &dev)) {
            x.acked_ts = x.acked_ts.max(max);
        }
    });
    StatusCode::OK
}

async fn proxy_info() -> Json<serde_json::Value> {
    Json(json!({"available": crate::mobile_tls::configured_port().is_some(), "kind": "socks5-over-tls", "port": crate::mobile_tls::configured_port()}))
}

async fn no_pubkey() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn accept_pubkeys() -> StatusCode {
    StatusCode::OK
}

async fn not_ready() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::NOT_IMPLEMENTED, Json(json!({"error": "files from the phone are not supported yet"})))
}

async fn ws(State(st): State<Arc<MobileState>>, up: WebSocketUpgrade) -> Response {
    up.on_upgrade(move |sock| ws_session(st, sock))
}

fn chat_frame(m: &ChatMessage) -> Vec<u8> {
    let p = m.text.as_bytes();
    let mut f = Vec::with_capacity(45 + p.len());
    f.push(FT_CHAT_MSG);
    f.extend_from_slice(&m.from.0);
    f.extend_from_slice(&(m.timestamp as i64).to_le_bytes());
    f.extend_from_slice(&(p.len() as u32).to_le_bytes());
    f.extend_from_slice(p);
    f
}

async fn ws_session(st: Arc<MobileState>, mut sock: WebSocket) {
    let mut rx = bus().subscribe();
    let mut seen: HashSet<HashId> = HashSet::new();
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(m) => {
                    if m.from != st.my_id && seen.insert(m.msg_id) {
                        if sock.send(Message::Binary(chat_frame(&m))).await.is_err() { break; }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            inc = sock.recv() => {
                let Some(Ok(msg)) = inc else { break };
                let Message::Binary(d) = msg else { if matches!(msg, Message::Close(_)) { break } else { continue } };
                match d.first().copied() {
                    Some(FT_PING) => { if sock.send(Message::Binary(vec![FT_PONG])).await.is_err() { break; } }
                    Some(FT_SEND_MSG) if d.len() >= 37 => {
                        let mut id = [0u8; 32];
                        id.copy_from_slice(&d[1..33]);
                        let len = u32::from_le_bytes([d[33], d[34], d[35], d[36]]) as usize;
                        if len == 0 || len > MAX_TEXT || d.len() < 37 + len { continue; }
                        let text = String::from_utf8_lossy(&d[37..37 + len]).to_string();
                        let chat = st.chat.clone();
                        // отправка может ждать сеть — не держим сокет
                        tokio::task::spawn(async move { let _ = chat.send_message(HashId(id), text).await; });
                    }
                    _ => {}
                }
            }
        }
    }
}

fn router(st: Arc<MobileState>) -> Router {
    let guarded = Router::new()
        .route("/mobile/info", get(info))
        .route("/mobile/contacts", get(contacts))
        .route("/mobile/chat/:peer", get(history).post(send))
        .route("/mobile/inbox", get(inbox))
        .route("/mobile/inbox/ack", post(inbox_ack))
        .route("/mobile/proxy/info", get(proxy_info))
        .route("/mobile/pubkey/:peer", get(no_pubkey))
        .route("/mobile/pubkeys", post(accept_pubkeys))
        .route("/mobile/files", get(not_ready).post(not_ready))
        .route("/mobile/ws", get(ws))
        .layer(middleware::from_fn(auth));
    Router::new().route("/mobile/pair", post(pair)).merge(guarded).with_state(st)
}

/// Поток, у которого уже прочитанное начало возвращается читателю первым.
pub struct Prepended<S> {
    head: std::io::Cursor<Vec<u8>>,
    inner: S,
}

impl<S> Prepended<S> {
    pub fn new(head: Vec<u8>, inner: S) -> Self {
        Self { head: std::io::Cursor::new(head), inner }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prepended<S> {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        let pos = self.head.position() as usize;
        let rest = &self.head.get_ref()[pos..];
        if !rest.is_empty() {
            let n = rest.len().min(buf.remaining());
            buf.put_slice(&rest[..n]);
            self.head.set_position((pos + n) as u64);
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prepended<S> {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, b: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, b)
    }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Похож ли начальный кусок на запрос к `/mobile/...`.
pub fn is_mobile_request(first: &[u8]) -> bool {
    let line = first.split(|b| *b == b'\r' || *b == b'\n').next().unwrap_or(&[]);
    let line = String::from_utf8_lossy(line);
    let mut it = line.split_whitespace();
    matches!((it.next(), it.next()), (Some("GET" | "POST" | "PUT" | "DELETE"), Some(p)) if p.starts_with("/mobile/"))
}

/// Обслужить одно TLS-соединение как HTTP (включая переход на сокет). Узел не готов — отвечаем 503.
pub async fn serve_connection<S>(stream: S, first: Vec<u8>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    use hyper_util::rt::TokioIo;
    use hyper_util::service::TowerToHyperService;
    let Some(st) = STATE.get().cloned() else {
        let mut s = stream;
        use tokio::io::AsyncWriteExt;
        return s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    };
    let io = TokioIo::new(Prepended::new(first, stream));
    let svc = TowerToHyperService::new(router(st));
    hyper::server::conn::http1::Builder::new()
        .serve_connection(io, svc)
        .with_upgrades()
        .await
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_requests_are_recognised() {
        assert!(is_mobile_request(b"POST /mobile/pair HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(is_mobile_request(b"GET /mobile/ws?token=a HTTP/1.1\r\n"));
        assert!(!is_mobile_request(b"GET / HTTP/1.1\r\n"));
        assert!(!is_mobile_request(b"GET /mobilex HTTP/1.1\r\n"));
        assert!(!is_mobile_request(&[0x05, 0x01, 0x00]));
    }

    #[test]
    fn code_compare_is_exact() {
        assert!(same("123456", "123456"));
        assert!(!same("123456", "123457"));
        assert!(!same("123456", "12345"));
    }

    #[test]
    fn chat_frame_has_the_layout_the_app_expects() {
        let m = ChatMessage::new(HashId([7; 32]), HashId([9; 32]), "hi".into());
        let f = chat_frame(&m);
        assert_eq!(f[0], FT_CHAT_MSG);
        assert_eq!(&f[1..33], &[7u8; 32]);
        assert_eq!(u32::from_le_bytes([f[41], f[42], f[43], f[44]]), 2);
        assert_eq!(&f[45..], b"hi");
    }
}
