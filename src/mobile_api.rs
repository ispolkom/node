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
/// Сигнал для звонка другому телефону: доставляется только если тот на связи прямо сейчас и нигде не хранится
/// (устаревший звонок не должен «звонить» через час). Узел передаёт байты как есть: они зашифрованы телефоном для телефона.
const FT_SEND_LIVE: u8 = 0x31;
const FT_LIVE_MSG: u8 = 0x13;
/// Ответ отправителю: адресата нет на связи (кадр: тип и 32 байта номера).
const FT_PEER_OFFLINE: u8 = 0x14;
/// Сигнал звонка небольшой (описание сеанса и кандидаты): больше не принимаем.
const MAX_LIVE: usize = 32 * 1024;

pub struct MobileState {
    chat: Arc<ChatManager>,
    p2p: Arc<crate::p2p::P2PTransport>,
    my_id: HashId,
}

static STATE: OnceLock<Arc<MobileState>> = OnceLock::new();
static BUS: OnceLock<tokio::sync::broadcast::Sender<Event>> = OnceLock::new();
static MAIL: Mutex<Option<Vec<Mail>>> = Mutex::new(None);
static LIVE: Mutex<Option<std::collections::HashMap<String, usize>>> = Mutex::new(None);
/// Когда устройство последний раз подключалось или отключалось (мс), для списка устройств на странице узла.
static LAST_SEEN: Mutex<Option<std::collections::HashMap<String, u64>>> = Mutex::new(None);
static LAST_TS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TUNNELS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

const MAX_TUNNELS: usize = 256;
const MAX_MAIL: usize = 5000;
const FT_PEER_STATUS: u8 = 0x12;

#[derive(Clone)]
enum Event {
    Node(Arc<ChatMessage>),
    Mail(Arc<Mail>),
    Live(Arc<Mail>),
    Presence(String, bool),
}

/// Сообщение от одного телефона владельца другому: узел только передаёт байты (они могут быть зашифрованы телефоном для телефона).
#[derive(Clone, Serialize, Deserialize)]
struct Mail {
    ts: u64,
    from: String,
    to: String,
    payload_b64: String,
}
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
    /// публичные ключи телефона (для сквозного шифрования между телефонами владельца), base64
    #[serde(default)]
    x25519_pub: Option<String>,
    #[serde(default)]
    ed25519_pub: Option<String>,
}

impl Device {
    /// Номер телефона как собеседника: 32 байта от хэша токена (токен не раскрывается).
    fn peer_id(&self) -> String {
        hex::encode(Sha256::digest([b"yandi-device:".as_slice(), self.token_hash.as_bytes()].concat()))
    }
}

fn bus() -> &'static tokio::sync::broadcast::Sender<Event> {
    BUS.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// Чат вызывает это после сохранения входящего сообщения: подключённые телефоны получат его сразу.
pub fn publish(msg: &ChatMessage) {
    let _ = bus().send(Event::Node(Arc::new(msg.clone())));
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

fn next_ts() -> u64 {
    use std::sync::atomic::Ordering::SeqCst;
    let mut cur = LAST_TS.load(SeqCst);
    loop {
        let t = now_ms().max(cur + 1);
        match LAST_TS.compare_exchange(cur, t, SeqCst, SeqCst) {
            Ok(_) => return t,
            Err(c) => cur = c,
        }
    }
}

fn mail_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("mobile_mail.json")
}

fn with_mail<R>(f: impl FnOnce(&mut Vec<Mail>) -> R) -> R {
    let mut g = MAIL.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| std::fs::read_to_string(mail_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    let r = f(list);
    if let Ok(s) = serde_json::to_string(&*list) {
        let p = mail_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = crate::util::private_file::write_private(&p, s.as_bytes());
    }
    r
}

/// Спаренные устройства и их состояние: для страницы узла (кто в сети, когда был виден).
pub fn devices_overview() -> Vec<serde_json::Value> {
    let seen = LAST_SEEN.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    with_devices(|d| d.clone())
        .into_iter()
        .map(|x| {
            let pid = x.peer_id();
            let n = live_count(&pid);
            json!({
                "id": pid[..16].to_string(),
                "name": x.name,
                "created_ms": x.created_ms,
                "online": n > 0,
                "connections": n,
                "last_seen_ms": seen.get(&pid),
                "has_keys": x.x25519_pub.is_some(),
            })
        })
        .collect()
}

/// Забыть устройство (его токен перестаёт действовать, очередь сообщений для него очищается). `id` — начало номера устройства, не короче 8 знаков.
pub fn remove_device(id: &str) -> bool {
    if id.len() < 8 {
        return false;
    }
    let Some(peer) = with_devices(|d| d.iter().map(|x| x.peer_id()).find(|p| p.starts_with(id))) else { return false };
    with_devices(|d| d.retain(|x| x.peer_id() != peer));
    with_mail(|l| l.retain(|m| m.to != peer && m.from != peer));
    true
}

fn device_by_peer(peer: &str) -> Option<Device> {
    with_devices(|d| d.iter().find(|x| x.peer_id() == peer).cloned())
}

fn live_count(peer: &str) -> usize {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(peer).copied()).unwrap_or(0)
}

fn set_live(peer: &str, up: bool) {
    LAST_SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(peer.to_string(), now_ms());
    let mut g = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(Default::default);
    let c = m.entry(peer.to_string()).or_insert(0);
    let was = *c > 0;
    if up {
        *c += 1;
    } else {
        *c = c.saturating_sub(1);
    }
    let now = *c > 0;
    if !now {
        m.remove(peer);
    }
    if was != now {
        let _ = bus().send(Event::Presence(peer.to_string(), now));
    }
}

/// Положить сообщение другому телефону владельца в его очередь и сразу показать, если он на связи.
fn deliver_mail(from: &str, to: &str, payload: &[u8]) -> bool {
    if payload.is_empty() || payload.len() > MAX_TEXT + 256 || device_by_peer(to).is_none() {
        return false;
    }
    let m = Mail { ts: next_ts(), from: from.to_string(), to: to.to_string(), payload_b64: base64_of(payload) };
    with_mail(|l| {
        l.push(m.clone());
        if l.len() > MAX_MAIL {
            let drop = l.len() - MAX_MAIL;
            l.drain(..drop);
        }
    });
    let _ = bus().send(Event::Mail(Arc::new(m)));
    true
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
    let mut name: String = req.device_name.trim().chars().take(64).collect();
    if name.is_empty() {
        name = "Телефон".to_string();
    }
    with_devices(|d| {
        if d.len() >= MAX_DEVICES {
            d.remove(0);
        }
        let th = token_hash(&token);
        if d.iter().any(|x| x.name == name || x.name.starts_with(&format!("{name} ("))) {
            name = format!("{name} ({})", &th[..4]);
        }
        d.push(Device { token_hash: th, name, created_ms: now_ms(), acked_ts: now_ms(), x25519_pub: None, ed25519_pub: None });
    });
    Json(json!({"token": token})).into_response()
}

#[derive(Clone)]
pub(crate) struct DeviceKey(pub(crate) String);

/// Номер устройства (как собеседника) по хэшу его токена.
pub(crate) fn device_peer_of(token_hash: &str) -> Option<String> {
    with_devices(|d| d.iter().find(|x| same(&x.token_hash, token_hash)).map(|x| x.peer_id()))
}

/// Это сопряжённое устройство этого узла?
pub(crate) fn is_device_peer(peer: &str) -> bool {
    device_by_peer(peer).is_some()
}

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

async fn info(State(st): State<Arc<MobileState>>, axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Json<serde_json::Value> {
    let id = hex::encode(st.my_id.0);
    let me = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.peer_id()));
    Json(json!({"node_id": id, "name": format!("YANDI {}", &id[..8]), "version": crate::VERSION, "device_id": me, "online": true}))
}

async fn contacts(State(st): State<Arc<MobileState>>, axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Json<serde_json::Value> {
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
    let me = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.peer_id()));
    for d in with_devices(|d| d.clone()) {
        let pid = d.peer_id();
        if Some(&pid) == me.as_ref() {
            continue;
        }
        out.push(json!({"peer_id": pid, "display_name": format!("📱 {}", d.name), "online": live_count(&pid) > 0, "is_manual": false}));
    }
    Json(json!({"contacts": out}))
}

fn msg_json(m: &ChatMessage) -> serde_json::Value {
    json!({"id": hex::encode(m.msg_id.0), "from_peer_id": hex::encode(m.from.0), "text": m.text, "ts_ms": m.timestamp})
}

async fn history(State(st): State<Arc<MobileState>>, Path(peer): Path<String>, Query(q): Query<std::collections::HashMap<String, String>>) -> Json<serde_json::Value> {
    let limit = q.get("limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(50).clamp(1, 500);
    if device_by_peer(&peer).is_some() {
        return Json(json!({"messages": []})); // переписка между телефонами хранится на самих телефонах
    }
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

async fn send(State(st): State<Arc<MobileState>>, axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path(peer): Path<String>, Json(req): Json<SendReq>) -> Response {
    if req.text.is_empty() || req.text.len() > MAX_TEXT {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if device_by_peer(&peer).is_some() {
        let Some(me) = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.peer_id())) else { return StatusCode::UNAUTHORIZED.into_response() };
        return if deliver_mail(&me, &peer, req.text.as_bytes()) { Json(json!({"ok": true})).into_response() } else { StatusCode::BAD_REQUEST.into_response() };
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
    if let Some(me) = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.peer_id())) {
        for m in with_mail(|l| l.iter().filter(|m| m.to == me && m.ts > acked).cloned().collect::<Vec<_>>()) {
            out.push((m.ts, json!({"id": m.ts, "from_peer_id": m.from, "payload_b64": m.payload_b64, "ts_ms": m.ts})));
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
    if let Some(me) = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.peer_id())) {
        with_mail(|l| l.retain(|m| !(m.to == me && m.ts <= max)));
    }
    StatusCode::OK
}

/// Выход в интернет через этот компьютер всегда доступен устройству с токеном; куда именно он идёт, решает владелец (внешний прокси в настройках).
async fn proxy_info() -> Json<serde_json::Value> {
    Json(json!({"running": true, "host": "этот компьютер", "port": crate::mobile_tls::configured_port(), "upstream": crate::upstream_proxy::active().is_some()}))
}

/// Публичный ключ телефона владельца (чтобы телефон шифровал сообщение для другого телефона сам); у обычных собеседников ключа нет — 404.
async fn pubkey(Path(peer): Path<String>) -> Response {
    match device_by_peer(&peer).and_then(|d| d.x25519_pub) {
        Some(k) => Json(json!({"x25519_pub": k})).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct KeysReq {
    ed25519_pub: String,
    x25519_pub: String,
}

async fn accept_pubkeys(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<KeysReq>) -> StatusCode {
    let ok = |k: &str| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(k).map(|b| b.len() == 32).unwrap_or(false)
    };
    if !ok(&req.x25519_pub) || !ok(&req.ed25519_pub) {
        return StatusCode::BAD_REQUEST;
    }
    with_devices(|d| {
        if let Some(x) = d.iter_mut().find(|x| same(&x.token_hash, &dev)) {
            x.x25519_pub = Some(req.x25519_pub);
            x.ed25519_pub = Some(req.ed25519_pub);
        }
    });
    StatusCode::OK
}

/// Настройка сервера звонков (coturn на этом же компьютере): `turn.json` в папке данных `{"port": 3478, "secret": "...", "host": "необязательно"}`.
#[derive(Deserialize)]
struct TurnConfig {
    port: u16,
    secret: String,
    #[serde(default)]
    host: Option<String>,
}

fn turn_config() -> Option<TurnConfig> {
    let s = std::fs::read_to_string(crate::util::data_dir::data_dir().join("turn.json")).ok()?;
    let c: TurnConfig = serde_json::from_str(&s).ok()?;
    (c.port > 0 && c.secret.len() >= 16).then_some(c)
}

/// Временные учётные данные для TURN (схема «REST API» у coturn: имя — срок действия и метка, пароль — HMAC-SHA1 от имени).
/// Постоянного пароля у телефона нет, а срок действия ограничен.
async fn turn_credentials(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let Some(c) = turn_config() else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "сервер звонков не настроен на узле"}))).into_response();
    };
    const TTL: u64 = 6 * 3600;
    let username = format!("{}:{}", now_ms() / 1000 + TTL, &dev[..dev.len().min(16)]);
    let Ok(mut mac) = Hmac::<sha1::Sha1>::new_from_slice(c.secret.as_bytes()) else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    Json(json!({"host": c.host, "port": c.port, "username": username, "credential": credential, "ttl": TTL})).into_response()
}

async fn ws(State(st): State<Arc<MobileState>>, axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, up: WebSocketUpgrade) -> Response {
    let Some(me) = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.peer_id())) else { return StatusCode::UNAUTHORIZED.into_response() };
    up.on_upgrade(move |sock| ws_session(st, me, sock))
}

fn frame(kind: u8, from: &[u8], ts: u64, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(45 + payload.len());
    f.push(kind);
    f.extend_from_slice(from);
    f.extend_from_slice(&(ts as i64).to_le_bytes());
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

fn chat_frame(m: &ChatMessage) -> Vec<u8> {
    frame(FT_CHAT_MSG, &m.from.0, m.timestamp, m.text.as_bytes())
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    let v = hex::decode(s).ok()?;
    v.try_into().ok()
}

async fn ws_session(st: Arc<MobileState>, me: String, mut sock: WebSocket) {
    use base64::Engine;
    set_live(&me, true);
    let mut rx = bus().subscribe();
    let mut seen: HashSet<HashId> = HashSet::new();
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(Event::Node(m)) => {
                    if m.from != st.my_id && seen.insert(m.msg_id) {
                        if sock.send(Message::Binary(chat_frame(&m))).await.is_err() { break; }
                    }
                }
                Ok(Event::Mail(m)) => {
                    if m.to == me {
                        if let (Some(from), Ok(p)) = (hex32(&m.from), base64::engine::general_purpose::STANDARD.decode(&m.payload_b64)) {
                            if sock.send(Message::Binary(frame(FT_CHAT_MSG, &from, m.ts, &p))).await.is_err() { break; }
                        }
                    }
                }
                Ok(Event::Live(m)) => {
                    if m.to == me {
                        if let (Some(from), Ok(p)) = (hex32(&m.from), base64::engine::general_purpose::STANDARD.decode(&m.payload_b64)) {
                            if sock.send(Message::Binary(frame(FT_LIVE_MSG, &from, m.ts, &p))).await.is_err() { break; }
                        }
                    }
                }
                Ok(Event::Presence(peer, up)) => {
                    if peer != me {
                        if let Some(id) = hex32(&peer) {
                            let mut f = vec![FT_PEER_STATUS];
                            f.extend_from_slice(&id);
                            f.push(up as u8);
                            if sock.send(Message::Binary(f)).await.is_err() { break; }
                        }
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
                    Some(FT_SEND_LIVE) if d.len() >= 37 => {
                        let mut id = [0u8; 32];
                        id.copy_from_slice(&d[1..33]);
                        let len = u32::from_le_bytes([d[33], d[34], d[35], d[36]]) as usize;
                        if len == 0 || len > MAX_LIVE || d.len() < 37 + len { continue; }
                        let to = hex::encode(id);
                        if device_by_peer(&to).is_none() || to == me { continue; }
                        if live_count(&to) == 0 {
                            let mut f = vec![FT_PEER_OFFLINE];
                            f.extend_from_slice(&id);
                            if sock.send(Message::Binary(f)).await.is_err() { break; }
                            continue;
                        }
                        let m = Mail { ts: next_ts(), from: me.clone(), to, payload_b64: base64::engine::general_purpose::STANDARD.encode(&d[37..37 + len]) };
                        let _ = bus().send(Event::Live(Arc::new(m)));
                    }
                    Some(FT_SEND_MSG) if d.len() >= 37 => {
                        let mut id = [0u8; 32];
                        id.copy_from_slice(&d[1..33]);
                        let len = u32::from_le_bytes([d[33], d[34], d[35], d[36]]) as usize;
                        if len == 0 || len > MAX_TEXT + 256 || d.len() < 37 + len { continue; }
                        if device_by_peer(&hex::encode(id)).is_some() {
                            // другому телефону владельца: байты передаются как есть
                            deliver_mail(&me, &hex::encode(id), &d[37..37 + len]);
                            continue;
                        }
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
    set_live(&me, false);
}

fn router(st: Arc<MobileState>) -> Router {
    let guarded = Router::new()
        .route("/mobile/info", get(info))
        .route("/mobile/contacts", get(contacts))
        .route("/mobile/chat/:peer", get(history).post(send))
        .route("/mobile/inbox", get(inbox))
        .route("/mobile/inbox/ack", post(inbox_ack))
        .route("/mobile/proxy/info", get(proxy_info))
        .route("/mobile/pubkey/:peer", get(pubkey))
        .route("/mobile/pubkeys", post(accept_pubkeys))
        .route("/mobile/turn", get(turn_credentials))
        .route("/mobile/files", get(crate::mobile_files::list).post(crate::mobile_files::start))
        .route("/mobile/files/:id", axum::routing::delete(crate::mobile_files::remove))
        .route("/mobile/files/:id/status", get(crate::mobile_files::status))
        .route("/mobile/files/:id/chunk/:idx", get(crate::mobile_files::get_chunk).put(crate::mobile_files::put_chunk))
        .route("/mobile/files/:id/done", post(crate::mobile_files::done))
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

/// Начало запроса `CONNECT host:port` — выход в интернет для телефона (только с токеном устройства).
pub fn is_connect_request(first: &[u8]) -> bool {
    first.starts_with(b"CONNECT ")
}

fn connect_target_and_token(head: &str) -> Option<(String, String)> {
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let mut it = first.split_whitespace();
    if it.next()? != "CONNECT" {
        return None;
    }
    let target = it.next()?.to_string();
    let mut token = String::new();
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        if k.trim().eq_ignore_ascii_case("proxy-authorization") {
            let v = v.trim();
            if let Some(t) = v.strip_prefix("Bearer ") {
                token = t.trim().to_string();
            } else if let Some(b) = v.strip_prefix("Basic ") {
                use base64::Engine;
                if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b.trim()) {
                    token = String::from_utf8_lossy(&raw).split_once(':').map(|(_, p)| p.to_string()).unwrap_or_default();
                }
            }
        }
    }
    Some((target, token))
}

/// Выход телефона в интернет через этот компьютер: проверяем токен устройства, соединяемся по правилам выхода узла (внешний прокси, если он
/// включён; адреса этого компьютера и домашней сети закрыты) и передаём байты в обе стороны.
pub async fn serve_connect<S>(mut tls: S, first: Vec<u8>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = first;
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > 8192 {
            return Ok(());
        }
        let mut tmp = [0u8; 2048];
        let n = tokio::time::timeout(Duration::from_secs(10), tls.read(&mut tmp)).await.map_err(|_| std::io::Error::other("timeout"))??;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let rest = buf[end..].to_vec();
    let reply = |code: &str| format!("HTTP/1.1 {code}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let Some((target, token)) = connect_target_and_token(&head) else { return tls.write_all(reply("400 Bad Request").as_bytes()).await };
    let h = token_hash(&token);
    if token.is_empty() || !with_devices(|d| d.iter().any(|x| same(&x.token_hash, &h))) {
        return tls.write_all(reply("407 Proxy Authentication Required").as_bytes()).await;
    }
    if TUNNELS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= MAX_TUNNELS {
        TUNNELS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        return tls.write_all(reply("503 Service Unavailable").as_bytes()).await;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            TUNNELS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _g = Guard;
    let mut out = match crate::exit_policy::connect_public(&target, Duration::from_secs(15)).await {
        Ok(s) => s,
        Err(e) => {
            let code = if e.kind() == std::io::ErrorKind::PermissionDenied { "403 Forbidden" } else { "502 Bad Gateway" };
            return tls.write_all(reply(code).as_bytes()).await;
        }
    };
    let _ = out.set_nodelay(true);
    tls.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await?;
    if !rest.is_empty() {
        out.write_all(&rest).await?;
    }
    let idle = Duration::from_secs(crate::mobile_tls::IDLE_SECS);
    let (mut tr, mut tw) = tokio::io::split(tls);
    let (mut or, mut ow) = out.into_split();
    let up = async {
        let mut b = vec![0u8; 16 * 1024];
        loop {
            match tokio::time::timeout(idle, tr.read(&mut b)).await {
                Ok(Ok(n)) if n > 0 => {
                    if ow.write_all(&b[..n]).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = ow.shutdown().await;
    };
    let down = async {
        let mut b = vec![0u8; 16 * 1024];
        loop {
            match tokio::time::timeout(idle, or.read(&mut b)).await {
                Ok(Ok(n)) if n > 0 => {
                    if tw.write_all(&b[..n]).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = tw.shutdown().await;
    };
    tokio::select! { _ = up => {}, _ = down => {} }
    Ok(())
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
    fn connect_requests_are_parsed() {
        assert!(is_connect_request(b"CONNECT a.com:443 HTTP/1.1\r\n"));
        let (t, k) = connect_target_and_token("CONNECT a.com:443 HTTP/1.1\r\nHost: a.com:443\r\nProxy-Authorization: Bearer abc\r\n\r\n").unwrap();
        assert_eq!((t.as_str(), k.as_str()), ("a.com:443", "abc"));
        let (_, k) = connect_target_and_token("CONNECT a.com:443 HTTP/1.1\r\nproxy-authorization: Basic eTphYmM=\r\n\r\n").unwrap();
        assert_eq!(k, "abc");
        assert!(connect_target_and_token("GET / HTTP/1.1\r\n\r\n").is_none());
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
