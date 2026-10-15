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
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
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
const FT_PROFILE: u8 = 0x16;
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
/// Сколько неполученных писем может держать один отправитель и одна пара «отправитель → адресат»: один телефон не вытеснит чужую почту.
const MAX_MAIL_PER_SENDER: usize = 1500;
const MAX_MAIL_PER_PAIR: usize = 500;
/// Общий потолок очереди писем по размеру: файл очереди переписывается целиком, поэтому он не должен разрастаться до сотен МБ.
const MAX_MAIL_BYTES: usize = 24 * 1024 * 1024;
/// Письмо дольше этого не ждёт (конверт E4 всё равно недействителен через 30 суток).
const MAIL_TTL_MS: u64 = 30 * 24 * 3600 * 1000;
const FT_PEER_STATUS: u8 = 0x12;

#[derive(Clone)]
enum Event {
    Node(Arc<ChatMessage>),
    Mail(Arc<Mail>),
    Live(Arc<Mail>),
    Presence(String, bool),
    /// устройство забыто: его открытые сокеты закрываются сразу
    Revoked(String),
    /// профиль владельца изменился: телефоны подтянут его сами
    Profile,
}

/// Сообщить всем открытым сокетам телефонов, что ник или картинка профиля изменились.
pub fn announce_profile_changed() {
    let _ = bus().send(Event::Profile);
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
    /// роль, с которой сопрягается устройство по этому коду
    role: String,
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
    #[serde(default)]
    key_sig: Option<String>,
    /// последний раз, когда устройство было на связи (мс); хранится на диске, чтобы переживать перезапуск узла
    #[serde(default)]
    last_seen_ms: u64,
    /// `owner` — телефон владельца узла (всё), `user` — посторонний пользователь узла: свои почта, профиль, группы и жалобы
    #[serde(default = "owner_role")]
    role: String,
}

fn owner_role() -> String {
    "owner".to_string()
}

fn is_user_role(peer: &str) -> bool {
    with_devices(|d| d.iter().any(|x| x.peer_id() == peer && x.role == "user"))
}

/// Что недоступно роли `user`: личная переписка и контакты ПК, отправка от имени узла, профиль владельца.
fn owner_only_path(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    path == "/mobile/contacts"
        || path == "/mobile/inbox"
        || path == "/mobile/inbox/ack"
        || path.starts_with("/mobile/chat/")
        || path.starts_with("/mobile/files")
        || (path == "/mobile/profile" && method != Method::GET)
        || path == "/mobile/proxy/info"
}

impl Device {
    /// Когда устройство в последний раз проявляло активность: на связи / отключилось, иначе — когда сопрягли.
    fn last_activity_ms(&self) -> u64 {
        self.last_seen_ms.max(self.created_ms)
    }

    /// Номер телефона как собеседника: 32 байта от хэша токена (токен не раскрывается).
    fn peer_id(&self) -> String {
        hex::encode(Sha256::digest(
            [b"yandi-device:".as_slice(), self.token_hash.as_bytes()].concat(),
        ))
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
    spawn_pc_inbox_drain();
}

/// Сообщение телефона компьютеру, пришедшее, пока закрытая среда заперта: оно уже запечатано E2E ключом ПК (лежит в закрытой
/// среде), поэтому открытым текстом нигде не появляется и просто ждёт входа владельца.
#[derive(Clone, Serialize, Deserialize)]
struct PcMail {
    ts: u64,
    from: String,
    blob_b64: String,
}

static PC_INBOX: Mutex<Option<Vec<PcMail>>> = Mutex::new(None);
const PC_INBOX_MAX: usize = 1000;
const PC_INBOX_PER_DEVICE: usize = 200;

fn pc_inbox_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("pc_inbox.json")
}

fn with_pc_inbox<R>(f: impl FnOnce(&mut Vec<PcMail>) -> R) -> R {
    with_pc_inbox_saved(f).0
}

/// То же, но говорит, удалось ли записать очередь на диск.
fn with_pc_inbox_saved<R>(f: impl FnOnce(&mut Vec<PcMail>) -> R) -> (R, bool) {
    let mut g = PC_INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| {
        std::fs::read_to_string(pc_inbox_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    });
    let r = f(list);
    let saved = serde_json::to_vec(&*list).ok().map(|s| crate::util::private_file::write_private(&pc_inbox_path(), &s).is_ok()).unwrap_or(false);
    (r, saved)
}

/// Положить в ожидание. false — очередь переполнена (телефон увидит «не доставлено»).
fn queue_for_pc(device: &str, blob: &[u8]) -> bool {
    use base64::Engine;
    let ((queued, pos), saved) = with_pc_inbox_saved(|l| {
        if l.len() >= PC_INBOX_MAX || l.iter().filter(|m| m.from == device).count() >= PC_INBOX_PER_DEVICE {
            return (false, 0);
        }
        l.push(PcMail { ts: now_ms(), from: device.to_string(), blob_b64: base64::engine::general_purpose::STANDARD.encode(blob) });
        (true, l.len() - 1)
    });
    if queued && !saved {
        // на диск не легло — телефону «не доставлено», а не тихая потеря после перезапуска
        with_pc_inbox(|l| {
            if pos < l.len() {
                l.remove(pos);
            }
        });
        return false;
    }
    queued
}

/// Когда среда открыта, ожидающие сообщения обрабатываются как обычные входящие.
fn spawn_pc_inbox_drain() {
    crate::supervisor::supervise("pc-inbox-drain", crate::supervisor::Policy::restart(), move || async move {
        use base64::Engine;
        let mut swept = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let Some(st) = STATE.get().cloned() else { continue };
            if !swept {
                // один раз после старта: группы, чей владелец давно удалён, не висят в списках
                let live = live_owner_peers();
                if node_peer_hex().is_some() && live.len() > 1 {
                    let n = crate::mobile_groups::sweep_orphans(&live);
                    if n > 0 {
                        eprintln!("[groups] удалено групп без владельца: {n}");
                    }
                    swept = true;
                }
            }
            #[cfg(unix)]
            if crate::closed_api::is_locked() {
                continue;
            }
            // по одному: письмо уходит из очереди только после обработки, сбой сохранения оставляет его на повтор
            while let Some(m) = with_pc_inbox(|l| l.first().cloned()) {
                let done = if now_ms().saturating_sub(m.ts) > MAIL_TTL_MS {
                    true
                } else if let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(&m.blob_b64) {
                    phone_to_node(&st, &m.from, &blob)
                } else {
                    true
                };
                if !done {
                    break;
                }
                with_pc_inbox(|l| {
                    if l.first().map(|x| x.ts == m.ts && x.from == m.from && x.blob_b64 == m.blob_b64).unwrap_or(false) {
                        l.remove(0);
                    }
                });
            }
        }
    });
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn devices_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("mobile_devices.json")
}

/// Что сейчас лежит на диске: пишем файл только при изменении (проверка токена не должна писать диск при каждом запросе).
static DEVICES_ON_DISK: Mutex<Option<(std::path::PathBuf, String)>> = Mutex::new(None);

/// Применить `f` к списку устройств и сохранить, если он изменился. Вторым значением — удалось ли сохранить.
fn devices_apply<R>(f: impl FnOnce(&mut Vec<Device>) -> R) -> (R, bool) {
    let mut g = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| {
        std::fs::read_to_string(devices_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    });
    let r = f(list);
    let Ok(s) = serde_json::to_string_pretty(&*list) else {
        return (r, false);
    };
    let p = devices_path();
    let mut disk = DEVICES_ON_DISK.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(&*disk, Some((path, text)) if *path == p && *text == s) {
        return (r, true);
    }
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let saved = crate::util::private_file::write_private(&p, s.as_bytes()).is_ok();
    if saved {
        *disk = Some((p, s));
    }
    (r, saved)
}

fn with_devices<R>(f: impl FnOnce(&mut Vec<Device>) -> R) -> R {
    devices_apply(f).0
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Новый разовый код спаривания (6 цифр); предыдущий перестаёт действовать.
pub fn issue_pairing_code() -> String {
    issue_pairing_code_for("owner")
}

/// То же для роли: `owner` (телефон владельца) или `user` (посторонний пользователь узла).
pub fn issue_pairing_code_for(role: &str) -> String {
    use rand::Rng;
    // повышение — только явным «owner»: опечатка или неизвестное значение даёт меньше прав
    let role = if role == "owner" { "owner" } else { "user" }.to_string();
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
    *PAIRING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pairing {
        code: code.clone(),
        expires: Instant::now() + PAIRING_TTL,
        fails: 0,
        role,
    });
    code
}

/// Содержимое QR для приложения.
/// Содержимое QR и текста для приложения: `YANDI-PAIR-1:<base64 JSON>`. Непрозрачный blob (а не открытый JSON) — чтобы фильтры по
/// шаблону («pairing_code», голый IP) не цепляли приглашение, когда его пересылают в чатах/почте. Безопасность та же: внутри всё тот
/// же одноразовый код на 5 минут. Приложение понимает и новый blob, и старый открытый JSON.
pub fn pairing_qr_json(host: &str, port: u16, fingerprint_hex: &str, role: &str) -> String {
    use base64::Engine;
    let json = json!({"host": host, "port": port, "pairing_code": issue_pairing_code_for(role), "tls_fingerprint": fingerprint_hex, "tls": true}).to_string();
    format!(
        "YANDI-PAIR-1:{}",
        base64::engine::general_purpose::STANDARD.encode(json.as_bytes())
    )
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
    with_mail_saved(f).0
}

/// То же, но говорит, удалось ли записать очередь на диск.
fn with_mail_saved<R>(f: impl FnOnce(&mut Vec<Mail>) -> R) -> (R, bool) {
    let mut g = MAIL.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| {
        std::fs::read_to_string(mail_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    });
    let r = f(list);
    let saved = serde_json::to_string(&*list)
        .ok()
        .map(|s| {
            let p = mail_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            crate::util::private_file::write_private(&p, s.as_bytes()).is_ok()
        })
        .unwrap_or(false);
    (r, saved)
}

/// Спаренные устройства и их состояние: для страницы узла (кто в сети, когда был виден).
/// Открытые ключи подписи сопряжённых телефонов (только с проверенной связкой) и их имена.
pub fn device_ed_keys() -> Vec<([u8; 32], String)> {
    use base64::Engine;
    with_devices(|d| {
        d.iter()
            .filter_map(|x| {
                let raw = base64::engine::general_purpose::STANDARD.decode(x.key_sig.as_ref().and(x.ed25519_pub.as_deref())?).ok()?;
                Some((<[u8; 32]>::try_from(raw.as_slice()).ok()?, x.name.clone()))
            })
            .collect()
    })
}

pub fn devices_overview() -> Vec<serde_json::Value> {
    let seen = LAST_SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
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
                "last_seen_ms": seen.get(&pid).copied().unwrap_or(0).max(x.last_seen_ms),
                "has_keys": x.x25519_pub.is_some() && x.key_sig.is_some(),
                "peer_id": pid,
            })
        })
        .collect()
}

/// Забыть устройство (его токен перестаёт действовать, очередь сообщений для него очищается). `id` — начало номера устройства, не короче 8 знаков.
pub fn remove_device(id: &str) -> bool {
    if id.len() < 8 {
        return false;
    }
    let Some(peer) = with_devices(|d| d.iter().map(|x| x.peer_id()).find(|p| p.starts_with(id)))
    else {
        return false;
    };
    // отзыв подтверждается, только если новый список лёг на диск: иначе после перезапуска токен снова бы действовал
    let before = with_devices(|d| d.clone());
    let ((), saved) = devices_apply(|d| d.retain(|x| x.peer_id() != peer));
    if !saved {
        devices_apply(|d| *d = before);
        return false;
    }
    after_forget(peer);
    true
}

/// Всё, что следует за удалением устройства из списка: его почта, группы и открытые сокеты (вызывается вне замка списка устройств).
fn after_forget(peer: String) {
    with_mail(|l| l.retain(|m| m.to != peer && m.from != peer));
    crate::device_profile::forget(&peer);
    // группы, которыми владел этот человек, удаляются вместе с ним; бывшим участникам уходит извещение от имени узла
    let owned = crate::mobile_groups::owned_by(&peer);
    let others: Vec<(String, Vec<String>)> = if owned.is_empty() {
        Vec::new()
    } else {
        with_devices(|d| d.iter().map(|x| x.peer_id()).filter(|p| *p != peer).collect::<Vec<_>>())
            .into_iter()
            .map(|p| {
                let keys: Vec<String> = owned.iter().filter(|(gid, _, ks)| ks.contains(&crate::mobile_groups::member_key_for(gid, &p))).map(|(_, name, _)| name.clone()).collect();
                (p, keys)
            })
            .filter(|(_, names)| !names.is_empty())
            .collect()
    };
    crate::mobile_groups::forget_device(&peer);
    if !others.is_empty() {
        tokio::spawn(async move {
            for (device, names) in others {
                for name in names {
                    let _ = pc_admin_message(&device, &format!("Группа «{name}» удалена: её владелец больше не на узле. Это решение узла, а не ваше действие.")).await;
                }
            }
        });
    }
    let _ = bus().send(Event::Revoked(peer));
}

/// Полный номер устройства по его началу (не короче 8 знаков), как в списке устройств на странице узла.
pub(crate) fn device_peer_by_prefix(id: &str) -> Option<String> {
    if id.len() < 8 {
        return None;
    }
    with_devices(|d| d.iter().map(|x| x.peer_id()).find(|p| p.starts_with(id)))
}

/// Сопряжённые устройства: (номер, имя).
pub(crate) fn device_list() -> Vec<(String, String)> {
    with_devices(|d| d.iter().map(|x| (x.peer_id(), x.name.clone())).collect())
}

fn device_by_peer(peer: &str) -> Option<Device> {
    with_devices(|d| d.iter().find(|x| x.peer_id() == peer).cloned())
}

fn live_count(peer: &str) -> usize {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(peer).copied())
        .unwrap_or(0)
}

fn set_live(peer: &str, up: bool) {
    LAST_SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .insert(peer.to_string(), now_ms());
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
    drop(g);
    if was != now {
        // подключился или отключился: запоминаем на диске (раз в смену состояния, не на каждый кадр)
        let t = now_ms();
        with_devices(|d| {
            if let Some(x) = d.iter_mut().find(|x| x.peer_id() == peer) {
                x.last_seen_ms = t;
            }
        });
        let _ = bus().send(Event::Presence(peer.to_string(), now));
    }
}

/// Положить сообщение другому телефону владельца в его очередь и сразу показать, если он на связи.
fn deliver_mail(from: &str, to: &str, payload: &[u8]) -> bool {
    if payload.is_empty() || payload.len() > MAX_TEXT + 256 || device_by_peer(to).is_none() {
        return false;
    }
    let m = Mail {
        ts: next_ts(),
        from: from.to_string(),
        to: to.to_string(),
        payload_b64: base64_of(payload),
    };
    // отправитель (устройство или сам узел) проверяется под замком очереди: remove_device сначала убирает устройство, потом чистит его
    // письма, поэтому письмо забытого устройства либо не попадёт в очередь, либо будет вычищено вместе с остальными
    let (accepted, saved) = with_mail_saved(|l| (is_node_peer(from) || device_by_peer(from).is_some()) && mail_accept(l, m.clone(), now_ms()));
    if !accepted {
        return false; // очередь переполнена этим отправителем или вообще: новое письмо не принимаем, чужие старые не трогаем
    }
    if !saved {
        // на диск не легло: письмо существовало бы только в памяти и пропало при перезапуске — честно «не принято»
        with_mail(|l| l.retain(|x| !(x.ts == m.ts && x.from == m.from && x.to == m.to)));
        return false;
    }
    let _ = bus().send(Event::Mail(Arc::new(m)));
    true
}

/// Положить письмо в общую очередь. Просроченные письма убираются; переполнение решается отказом новому письму (с квотами на отправителя
/// и на пару), а не вытеснением чужих неполученных писем.
fn mail_accept(l: &mut Vec<Mail>, m: Mail, now: u64) -> bool {
    l.retain(|x| now.saturating_sub(x.ts) < MAIL_TTL_MS);
    let from_n = l.iter().filter(|x| x.from == m.from).count();
    let pair_n = l.iter().filter(|x| x.from == m.from && x.to == m.to).count();
    let bytes: usize = l.iter().map(|x| x.payload_b64.len()).sum();
    if l.len() >= MAX_MAIL || from_n >= MAX_MAIL_PER_SENDER || pair_n >= MAX_MAIL_PER_PAIR || bytes + m.payload_b64.len() > MAX_MAIL_BYTES {
        return false;
    }
    l.push(m);
    true
}

#[derive(Deserialize)]
struct PairReq {
    pairing_code: String,
    #[serde(default)]
    device_name: String,
}

async fn pair(Json(req): Json<PairReq>) -> Response {
    let granted: Option<String> = {
        let mut g = PAIRING.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_mut() {
            Some(p) if Instant::now() < p.expires && p.fails < PAIRING_MAX_FAILS => {
                if same(&p.code, req.pairing_code.trim()) {
                    let role = p.role.clone();
                    *g = None;
                    Some(role)
                } else {
                    p.fails += 1;
                    None
                }
            }
            _ => None,
        }
    };
    let Some(role) = granted else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bad or expired pairing code"})),
        )
            .into_response();
    };
    use rand::RngCore;
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let mut name: String = req.device_name.trim().chars().take(64).collect();
    if name.is_empty() || crate::content_policy::is_reserved_name(&name) {
        name = "Телефон".to_string();
    }
    // вытесняем того, кто дольше всех молчит, а не самого давно сопряжённого: давнее, но живое устройство не должно вылетать.
    // Выбор, удаление вытесняемого и добавление нового — одним действием под замком списка (параллельные сопряжения не дадут 17);
    // остальное «забывание» (группы, почта, сокеты) — после, вне замка
    let mut evicted: Option<String> = None;
    with_devices(|d| {
        if d.len() >= MAX_DEVICES {
            if let Some(i) = d.iter().enumerate().min_by_key(|(_, x)| x.last_activity_ms()).map(|(i, _)| i) {
                evicted = Some(d.remove(i).peer_id());
            }
        }
        let th = token_hash(&token);
        if d.iter()
            .any(|x| x.name == name || x.name.starts_with(&format!("{name} (")))
        {
            name = format!("{name} ({})", &th[..4]);
        }
        d.push(Device {
            token_hash: th,
            name,
            created_ms: now_ms(),
            acked_ts: now_ms(),
            x25519_pub: None,
            ed25519_pub: None,
            key_sig: None,
            last_seen_ms: 0,
            role,
        });
    });
    if let Some(peer) = evicted {
        after_forget(peer);
    }
    Json(json!({"token": token})).into_response()
}

#[derive(Clone)]
pub(crate) struct DeviceKey(pub(crate) String);

/// Сообщение телефона самому компьютеру (подписанный E4): расшифровать и положить в историю веб-чата.
/// true — письмо обработано (или не подлежит обработке); false — сохранить не удалось, его стоит повторить.
fn phone_to_node(st: &MobileState, from_device: &str, blob: &[u8]) -> bool {
    if crate::mobile_self::is_blocked_phone(from_device) {
        return true; // заблокированное устройство: ни сообщений, ни файлов, ни звонков компьютеру
    }
    let Ok(ed) = device_ed25519(from_device) else { return true };
    let (Some(text), Some(from)) = (crate::mobile_self::open(blob, from_device, &hex::encode(st.my_id.0), 1, &ed), hex32(from_device)) else { return true };
    let device = HashId(from);
    crate::mobile_self::unhide_phone(from_device);
    let text = match crate::mobile_self::classify(text) {
        crate::mobile_self::Envelope::Receipt { read, ids } => {
            let status = if read { crate::communication::MessageStatus::Read } else { crate::communication::MessageStatus::Delivered };
            for id in ids {
                if let Ok(mid) = HashId::from_hex(&id) {
                    let _ = st.chat.store_local_status(&device, &mid, status.clone());
                }
            }
            return true;
        }
        crate::mobile_self::Envelope::Edit { cmid, text } => {
            let _ = st.chat.edit_message(&device, &crate::mobile_self::incoming_id(from_device, &cmid), text);
            return true;
        }
        crate::mobile_self::Envelope::CapPing => {
            node_live_to_device(st, from_device, &crate::mobile_self::cap_ping());
            return true;
        }
        crate::mobile_self::Envelope::Ignored => return true,
        crate::mobile_self::Envelope::Reaction { cmid, emoji } => {
            let local = crate::chat_reactions::local_for_phone(from_device, &cmid);
            crate::chat_reactions::set_peer(&hex::encode(&device.0[..8]), &local, &emoji);
            return true;
        }
        crate::mobile_self::Envelope::Message { cmid, text, reply } => {
            // сообщение с номером: в историю под выведенным номером, телефону сразу «доставлено», «прочитано» — когда откроют страницу
            let local = crate::mobile_self::incoming_id(from_device, &cmid);
            crate::chat_reactions::remember_rid(&hex::encode(local.0), &cmid); // чтобы страница могла поставить на него смайл
            let mut msg = ChatMessage::new(device, st.my_id, text);
            msg.msg_id = local;
            msg.encrypted = true;
            msg.reply_to = reply.and_then(|(id, quote)| {
                // якорь: номер сообщения на узле; свои сообщения телефон называет своим номером (cmid) — выводим тот же номер, что при приёме
                let anchor = if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                    id
                } else {
                    hex::encode(crate::mobile_self::incoming_id(from_device, &id).0)
                };
                crate::communication::ReplyRef { msg_id: anchor, text: quote }.sanitized()
            });
            msg.mark_delivered();
            if st.chat.store_local_incoming(&device, &msg).is_err() {
                // не сохранили: номер письма возвращается из журнала повторов, иначе повтор отвергнется как «уже виденное»
                crate::mobile_self::release_replay(blob);
                return false; // квитанции «доставлено» не будет, письмо остаётся в очереди на повтор
            }
            crate::mobile_self::remember_unread(&local, from_device, &cmid);
            node_mail_to_device(st, from_device, &crate::mobile_self::receipt(false, &[cmid]));
            return true;
        }
        crate::mobile_self::Envelope::Plain(t) => t,
    };
    if text.starts_with(crate::mobile_self::FILE_MARKER) {
        // предложение файла: куски уже лежат на узле, расшифровать и показать на странице (может занять время — не держим сокет)
        let chat = st.chat.clone();
        let (node, dev, me) = (hex::encode(st.my_id.0), from_device.to_string(), st.my_id);
        tokio::spawn(async move {
            let mut receipt_id = None;
            let mut msg = match crate::mobile_self::receive_file(&node, &dev, &text).await {
                Ok(f) => {
                    let mut m = ChatMessage::new(HashId(from), me, String::new());
                    m.msg_id = crate::mobile_self::incoming_id(&dev, &f.receipt_id);
                    receipt_id = Some(f.receipt_id);
                    m.attachment = Some(f.attachment);
                    m
                }
                Err(e) => ChatMessage::new(HashId(from), me, format!("⚠️ Файл с телефона не принят: {e}")),
            };
            msg.encrypted = true;
            msg.mark_delivered();
            if chat.store_local_incoming(&HashId(from), &msg).is_ok() {
                if let Some(id) = receipt_id {
                    crate::mobile_self::remember_unread(&msg.msg_id, &dev, &id);
                    if let Ok(x) = device_x25519(&dev) {
                        let receipt = crate::mobile_self::receipt(false, &[id]);
                        if let Ok(blob) = crate::mobile_self::seal(&receipt, &x, &node, &dev, 1) {
                            deliver_mail(&node, &dev, &blob);
                        }
                    }
                }
            }
        });
        return true;
    }
    let mut msg = ChatMessage::new(HashId(from), st.my_id, text);
    msg.encrypted = true;
    msg.mark_delivered();
    st.chat.store_local_incoming(&HashId(from), &msg).is_ok()
}

/// Номер самого узла (hex): под ним компьютер участвует в группах как устройство.
pub(crate) fn node_peer_hex() -> Option<String> {
    STATE.get().map(|s| hex::encode(s.my_id.0))
}

/// Это номер самого узла (компьютер как собеседник телефона)?
pub(crate) fn is_node_peer(peer: &str) -> bool {
    STATE.get().map(|s| hex::encode(s.my_id.0) == peer).unwrap_or(false)
}

/// Открытый x25519 телефона — только из подписанной связки, которую узел проверил при регистрации.
fn device_x25519(peer: &str) -> Result<[u8; 32], String> {
    let dev = device_by_peer(peer).ok_or("нет такого устройства")?;
    dev.key_sig
        .as_ref()
        .and(dev.x25519_pub.as_deref())
        .and_then(|k| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(k).ok()
        })
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| "телефон ещё не прислал подписанные ключи — откройте на нём приложение".to_string())
}

fn device_ed25519(peer: &str) -> Result<[u8; 32], String> {
    use base64::Engine;
    let dev = device_by_peer(peer).ok_or("нет такого устройства")?;
    dev.key_sig.as_ref().and(dev.ed25519_pub.as_deref())
        .and_then(|k| base64::engine::general_purpose::STANDARD.decode(k).ok())
        .and_then(|b| b.try_into().ok()).ok_or_else(|| "нет закреплённого ключа подписи".into())
}

/// Файл со страницы узла телефону: куски шифруются своим ключом и ложатся в хранилище файлов, предложение — зашифрованной почтой.
pub async fn pc_send_file_to_device(peer: &str, path: &std::path::Path, attachment: crate::communication::FileAttachment, text: String) -> Result<ChatMessage, String> {
    let st = STATE.get().cloned().ok_or("узел ещё не готов")?;
    if crate::mobile_self::is_blocked_phone(peer) {
        return Err("устройство заблокировано".into());
    }
    let x = device_x25519(peer)?;
    let to = HashId(hex32(peer).ok_or("неверный номер устройства")?);
    let node = hex::encode(st.my_id.0);
    let offer = crate::mobile_self::send_file(&node, peer, path, &attachment.filename, &attachment.mime_type).await?;
    let mut msg = ChatMessage::new(st.my_id, to, String::new());
    let envelope = crate::mobile_self::wrap_message(&hex::encode(msg.msg_id.0), &offer);
    if !deliver_mail(&node, peer, &crate::mobile_self::seal(&envelope, &x, &node, peer, 1)?) {
        return Err("не удалось положить предложение файла в очередь телефона".into());
    }
    if !text.trim().is_empty() {
        let _ = pc_send_to_device(peer, text.clone()).await;
    }
    msg.attachment = Some(attachment);
    msg.encrypted = true;
    // «доставлено» — когда телефон скачал файл и удалил передачу с узла (mobile_files::remove)
    if let Some(tid) = crate::mobile_self::offer_tid(&offer) {
        crate::mobile_self::remember_file(&tid, peer, msg.msg_id);
    }
    st.chat.store_local_outgoing(&to, &msg).map_err(|e| e.to_string())?;
    Ok(msg)
}

/// Положить телефону служебный конверт от узла (зашифрован его ключом) в очередь.
fn node_mail_to_device(st: &MobileState, device: &str, text: &str) {
    if crate::mobile_self::is_blocked_phone(device) {
        return;
    }
    if let Ok(x) = device_x25519(device) {
        let node = hex::encode(st.my_id.0);
        if let Ok(blob) = crate::mobile_self::seal(text, &x, &node, device, 1) {
            deliver_mail(&node, device, &blob);
        }
    }
}

/// «Живой» сигнал телефону от узла (не хранится): ответ на пинг поддержки квитанций.
fn node_live_to_device(st: &MobileState, device: &str, text: &str) {
    if crate::mobile_self::is_blocked_phone(device) {
        return;
    }
    if let Ok(x) = device_x25519(device) {
        let node = hex::encode(st.my_id.0);
        let Ok(blob) = crate::mobile_self::seal(text, &x, &node, device, 2) else { return };
        let m = Mail { ts: next_ts(), from: node, to: device.to_string(), payload_b64: base64_of(&blob) };
        let _ = bus().send(Event::Live(Arc::new(m)));
    }
}

/// Итог звонка в историю чата на странице узла (как в приложении: «📞 Звонок · 01:23», «📞 Видеозвонок: Нет ответа»). Пишет страница, когда звонок
/// закончился; `outgoing` — звонила страница.
pub fn pc_log_call(peer: &str, outgoing: bool, text: &str) -> Result<(), String> {
    let st = STATE.get().cloned().ok_or("узел ещё не готов")?;
    if !is_device_peer(peer) {
        return Err("нет такого устройства".into());
    }
    let text: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
    if !text.starts_with('📞') {
        return Err("это не запись звонка".into());
    }
    let to = HashId(hex32(peer).ok_or("неверный номер устройства")?);
    let (from, dest) = if outgoing { (st.my_id, to) } else { (to, st.my_id) };
    let mut msg = ChatMessage::new(from, dest, text);
    msg.encrypted = true;
    msg.mark_delivered();
    if outgoing { st.chat.store_local_outgoing(&to, &msg) } else { st.chat.store_local_incoming(&to, &msg) }.map_err(|e| e.to_string())
}

/// Сигнал звонка со страницы телефону (живой, не хранится): только при подписанных ключах телефона и если он на связи.
pub fn pc_call_signal(peer: &str, text: &str) -> Result<(), String> {
    let st = STATE.get().cloned().ok_or("узел ещё не готов")?;
    if crate::mobile_self::is_blocked_phone(peer) {
        return Err("устройство заблокировано".into());
    }
    if !text.starts_with(crate::mobile_self::CALL_MARKER) || text.len() > MAX_LIVE {
        return Err("это не сигнал звонка".into());
    }
    device_x25519(peer)?;
    if live_count(peer) == 0 {
        return Err("телефон не в сети".into());
    }
    node_live_to_device(&st, peer, text);
    Ok(())
}

/// Учётные данные сервера звонков (TURN) для страницы узла: та же схема, что у телефонов; адрес для браузера — этот же компьютер.
pub fn pc_turn() -> Option<serde_json::Value> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let c = turn_config()?;
    const TTL: u64 = 6 * 3600;
    let username = format!("{}:pc", now_ms() / 1000 + TTL);
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(c.secret.as_bytes()).ok()?;
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    Some(json!({"host": "127.0.0.1", "port": c.port, "username": username, "credential": credential}))
}

/// Владелец открыл переписку с телефоном на странице: отправить «прочитано» по входящим, по которым ещё не отправляли.
pub fn pc_mark_read(peer: &str) {
    let Some(st) = STATE.get().cloned() else { return };
    let ids = crate::mobile_self::take_unread(peer);
    if !ids.is_empty() {
        node_mail_to_device(&st, peer, &crate::mobile_self::receipt(true, &ids));
    }
}

/// Смайл со страницы телефону владельца: конверт реакции уходит почтой, у телефона тот же номер сообщения.
pub fn pc_react_device(peer: &str, msg_hex: &str, emoji: &str) -> Result<(), String> {
    let st = STATE.get().cloned().ok_or("узел ещё не готов")?;
    if !is_device_peer(peer) {
        return Err("нет такого устройства".into());
    }
    let key = crate::chat_reactions::key_for_peer(msg_hex);
    node_mail_to_device(&st, peer, &crate::chat_reactions::envelope(&key, emoji));
    Ok(())
}

/// Правка своего сообщения со страницы: в истории узла и у телефона (конверт правки с тем же номером).
pub fn pc_edit_for_device(peer: &str, msg_id: &HashId, text: String) -> Result<(), String> {
    let st = STATE.get().cloned().ok_or("узел ещё не готов")?;
    // служебный ключ подписи при запертой среде жив, а история личной переписки — нет: без этой проверки сообщение уходило бы
    // телефону, и только потом запись в историю падала с ошибкой
    if crate::closed_api::is_locked() {
        return Err("закрытая среда заперта: разблокируйте её, чтобы писать".into());
    }
    if crate::mobile_self::is_blocked_phone(peer) {
        return Err("устройство заблокировано".into());
    }
    if text.is_empty() || text.len() > MAX_TEXT {
        return Err("пустое или слишком длинное сообщение".into());
    }
    let to = HashId(hex32(peer).ok_or("неверный номер устройства")?);
    let own = st.chat.load_history(&to, usize::MAX).map_err(|e| e.to_string())?.into_iter().any(|m| m.msg_id == *msg_id && m.from == st.my_id && m.attachment.is_none());
    if !own {
        return Err("изменить можно только своё текстовое сообщение".into());
    }
    st.chat.edit_message(&to, msg_id, text.clone()).map_err(|e| e.to_string())?;
    node_mail_to_device(&st, peer, &crate::mobile_self::edit(&hex::encode(msg_id.0), &text));
    Ok(())
}

/// Телефон забрал файл, который ему отправил узел: сообщение на странице — «доставлено».
pub fn file_taken(tid: &str) {
    let Some(st) = STATE.get().cloned() else { return };
    if let Some((device, msg)) = crate::mobile_self::take_file(tid) {
        if let Some(d) = hex32(&device) {
            let _ = st.chat.store_local_status(&HashId(d), &msg, crate::communication::MessageStatus::Delivered);
        }
    }
}

/// Ответ со страницы узла телефону владельца: шифруется ключом телефона из его подписанной связки и ложится в его очередь.
pub async fn pc_send_to_device(peer: &str, text: String) -> Result<ChatMessage, String> {
    pc_send_to_device_reply(peer, text, None).await
}

/// То же с ответом-цитатой: телефон получает в конверте номер сообщения на узле и выдержку.
pub async fn pc_send_to_device_reply(peer: &str, text: String, reply: Option<crate::communication::ReplyRef>) -> Result<ChatMessage, String> {
    let st = STATE.get().cloned().ok_or("узел ещё не готов")?;
    if crate::mobile_self::is_blocked_phone(peer) {
        return Err("устройство заблокировано".into());
    }
    if text.is_empty() || text.len() > MAX_TEXT {
        return Err("пустое или слишком длинное сообщение".into());
    }
    // ключ принимается узлом только с проверенной подписью (accept_pubkeys), без него не отправляем
    let x = device_x25519(peer)?;
    let to = HashId(hex32(peer).ok_or("неверный номер устройства")?);
    let mut msg = ChatMessage::new(st.my_id, to, text);
    msg.encrypted = true;
    msg.reply_to = reply.and_then(|r| r.sanitized());
    // конверт с номером (cmid = msg_id): телефон пришлёт «доставлено» и «прочитано»; до того — одна серая галка
    let envelope = crate::mobile_self::wrap_message_reply(&hex::encode(msg.msg_id.0), &msg.text, msg.reply_to.as_ref().map(|r| (r.msg_id.as_str(), r.text.as_str())));
    let blob = crate::mobile_self::seal(&envelope, &x, &hex::encode(st.my_id.0), peer, 1)?;
    if !deliver_mail(&hex::encode(st.my_id.0), peer, &blob) {
        return Err("не удалось положить сообщение в очередь телефона".into());
    }
    st.chat.store_local_outgoing(&to, &msg).map_err(|e| e.to_string())?;
    Ok(msg)
}

/// Номер устройства (как собеседника) по хэшу его токена.
pub(crate) fn device_peer_of(token_hash: &str) -> Option<String> {
    with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, token_hash))
            .map(|x| x.peer_id())
    })
}

/// Кто может быть живым владельцем группы: сопряжённые телефоны и сам компьютер.
pub(crate) fn live_owner_peers() -> Vec<String> {
    let mut v: Vec<String> = with_devices(|d| d.iter().map(|x| x.peer_id()).collect());
    if let Some(n) = node_peer_hex() {
        v.push(n);
    }
    v
}

/// Это сопряжённое устройство этого узла?
pub(crate) fn is_device_peer(peer: &str) -> bool {
    device_by_peer(peer).is_some()
}

async fn auth(mut req: Request, next: Next) -> Response {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let Some(token) = bearer else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let h = token_hash(&token);
    let Some(role) = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &h)).map(|x| x.role.clone())) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if role == "user" && owner_only_path(req.method(), req.uri().path()) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "это устройство подключено как пользователь узла: личная переписка владельца ему недоступна"}))).into_response();
    }
    req.extensions_mut().insert(DeviceKey(h));
    next.run(req).await
}

/// Знает ли узел такого получателя: он есть среди контактов владельца или виден в сети.
async fn is_known_destination(st: &MobileState, h: &HashId) -> bool {
    if st.p2p.list_peers().await.into_iter().any(|p| p.id == *h) {
        return true;
    }
    let short = hex::encode(&h.0[..8]);
    std::fs::read_to_string("contacts.json")
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["contacts"].as_array().cloned())
        .map(|a| a.iter().any(|c| c["short_id"].as_str() == Some(short.as_str())))
        .unwrap_or(false)
}

async fn resolve_hex(st: &MobileState, id: &str) -> Option<HashId> {
    if let Ok(h) = HashId::from_hex(id) {
        return Some(h);
    }
    st.p2p.find_peer_by_short_id(id).await
}

async fn info(
    State(st): State<Arc<MobileState>>,
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
) -> Json<serde_json::Value> {
    let id = hex::encode(st.my_id.0);
    let me = with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, &dev))
            .map(|x| x.peer_id())
    });
    Json(
        json!({"node_id": id, "name": format!("YANDI {}", &id[..8]), "version": crate::VERSION, "device_id": me, "online": true}),
    )
}

#[derive(Deserialize)]
struct ComplaintReq {
    /// `user` | `group`
    target_kind: String,
    target_id: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    text: String,
    /// выбранные заявителем сообщения (узел E2E-сообщений не читает, доказательства прикладывает сам человек)
    #[serde(default)]
    evidence: Vec<String>,
}

/// Подать жалобу на пользователя или группу. Заявитель нарушителю не виден.
async fn complaint_submit(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<ComplaintReq>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    // имя цели узел берёт сам: заявитель не может подписать жалобу чужим именем
    let name = if req.target_kind == "user" {
        device_by_peer(&req.target_id).map(|d| d.name).unwrap_or_default()
    } else {
        String::new()
    };
    match crate::complaints::submit(&me, &req.target_kind, &req.target_id, &name, &req.reason, &req.text, &req.evidence) {
        Ok(id) => Json(json!({"ok": true, "id": id})).into_response(),
        Err((c, m)) => (c, Json(json!({"error": m}))).into_response(),
    }
}

/// Свои жалобы: статус и ответы узла.
async fn complaints_mine(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    Json(json!({"complaints": crate::complaints::mine(&me), "measures": crate::complaints::measures_for(&me)})).into_response()
}

/// Ответ или предупреждение от имени узла: ложится в чат устройства с пометкой «Администрация узла».
pub async fn pc_admin_message(peer: &str, text: &str) -> Result<(), String> {
    let marked = format!("🛡 Администрация узла\n{text}");
    pc_send_to_device(peer, marked).await.map(|_| ())
}

/// Ограничен ли человек сейчас (мера владельца): до какого времени, 0 — нет.
pub(crate) fn restricted_until(device_token_hash: &str) -> u64 {
    device_peer_of(device_token_hash).map(|p| crate::complaints::restricted_until(&p)).unwrap_or(0)
}

#[derive(Deserialize, Default)]
struct DeviceProfilePut {
    #[serde(default)]
    nick: Option<String>,
    #[serde(default)]
    avatar_b64: Option<String>,
    /// не показывать ник и картинку другим пользователям узла
    #[serde(default)]
    hide: Option<bool>,
}

/// Устройство само выходит из узла (старая запись при пересопряжении): токен перестаёт действовать, очередь стирается.
async fn device_forget_self(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    if remove_device(&me) {
        Json(json!({"ok": true})).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Свой профиль устройства (ник, картинка, скрытие): им пользуются в списке «Пользователи».
async fn device_profile_get(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    Json(crate::device_profile::own(&me)).into_response()
}

async fn device_profile_put(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<DeviceProfilePut>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    crate::mobile_profile::reply_json(crate::device_profile::put(&me, req.nick.as_deref(), req.avatar_b64.as_deref(), req.hide))
}

/// Пользователи узла: сопряжённые устройства со своими никами и картинками и статусом «в сети». Себя не включает.
async fn users_list(State(st): State<Arc<MobileState>>, axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Json<serde_json::Value> {
    let me = device_peer_of(&dev);
    let mut out = Vec::new();
    for d in with_devices(|d| d.clone()) {
        let pid = d.peer_id();
        if Some(&pid) == me.as_ref() {
            continue;
        }
        let (nick, hash) = crate::device_profile::public(&pid);
        out.push(json!({"peer_id": pid, "name": d.name, "nick": nick, "avatar_hash": hash, "online": live_count(&pid) > 0, "last_activity_ms": d.last_activity_ms()}));
    }
    out.push(json!({"peer_id": hex::encode(st.my_id.0), "name": "Компьютер", "nick": "", "avatar_hash": "", "online": true, "is_node": true}));
    Json(json!({"users": out}))
}

async fn user_avatar(axum::Extension(DeviceKey(_dev)): axum::Extension<DeviceKey>, Path(peer): Path<String>) -> Response {
    match crate::device_profile::avatar(&peer) {
        Some(b) => ([(header::CONTENT_TYPE, "image/png")], b).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Короткий номер своего узла (как в списке контактов): 16 hex.
fn my_short(st: &MobileState) -> String {
    hex::encode(&st.my_id.0[..8])
}

/// Единый профиль (ник и аватар): один на ПК и все его телефоны.
async fn profile_get(State(st): State<Arc<MobileState>>) -> axum::response::Response {
    crate::mobile_profile::reply_json(Ok(crate::mobile_profile::snapshot(&my_short(&st))))
}

async fn profile_put(State(st): State<Arc<MobileState>>, Json(req): Json<crate::mobile_profile::PutReq>) -> axum::response::Response {
    crate::mobile_profile::reply_json(crate::mobile_profile::apply(&my_short(&st), &req))
}

async fn profile_avatar(State(st): State<Arc<MobileState>>) -> axum::response::Response {
    crate::mobile_profile::reply_avatar(crate::mobile_profile::avatar_bytes(&my_short(&st)))
}

#[derive(Deserialize)]
struct ContactNameReq {
    peer_id: String,
    name: String,
}

/// Телефон сохранил или переименовал контакт: имя уходит в общий список узла, чтобы его видели ПК и другие телефоны.
async fn contact_put(Json(req): Json<ContactNameReq>) -> axum::response::Response {
    let Some(short) = req.peer_id.get(..16) else { return StatusCode::BAD_REQUEST.into_response() };
    if req.peer_id.len() != 64 && req.peer_id.len() != 16 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match crate::web::server::contacts_upsert(short, &req.name).await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    }
}

async fn contacts(
    State(st): State<Arc<MobileState>>,
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
) -> Json<serde_json::Value> {
    let known: Vec<HashId> = st
        .p2p
        .list_peers()
        .await
        .into_iter()
        .map(|p| p.id)
        .collect();
    let raw: serde_json::Value = std::fs::read_to_string("contacts.json")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({"contacts": []}));
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
            "nick": crate::profile_sync::peer_nick(&short),
        }));
    }
    let me = with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, &dev))
            .map(|x| x.peer_id())
    });
    for d in with_devices(|d| d.clone()) {
        let pid = d.peer_id();
        if Some(&pid) == me.as_ref() {
            continue;
        }
        // last_activity_ms — для приложения: устройство, не бывшее на связи и не в контактах дольше суток, уходит с главного экрана
        let (nick, hash) = crate::device_profile::public(&pid);
        let shown = if nick.is_empty() { d.name.clone() } else { nick.clone() };
        out.push(json!({"peer_id": pid, "display_name": format!("📱 {}", shown), "online": live_count(&pid) > 0, "is_manual": false, "last_activity_ms": d.last_activity_ms(), "nick": nick, "avatar_hash": hash}));
    }
    out.push(json!({"peer_id": hex::encode(st.my_id.0), "display_name": "💻 Компьютер", "online": true, "is_manual": false}));
    Json(json!({"contacts": out}))
}

fn msg_json(m: &ChatMessage) -> serde_json::Value {
    let mut j = json!({"id": hex::encode(m.msg_id.0), "from_peer_id": hex::encode(m.from.0), "text": m.text, "ts_ms": m.timestamp});
    if let Some(r) = &m.reply_to {
        j["reply_to"] = json!({"id": r.msg_id, "text": r.text});
    }
    j
}

async fn history(
    State(st): State<Arc<MobileState>>,
    Path(peer): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    if device_by_peer(&peer).is_some() {
        return Json(json!({"messages": []})); // переписка между телефонами хранится на самих телефонах
    }
    let Some(h) = resolve_hex(&st, &peer).await else {
        return Json(json!({"messages": []}));
    };
    let mut msgs = st.chat.load_history(&h, limit).unwrap_or_default();
    msgs.sort_by_key(|m| m.timestamp);
    let skip = msgs.len().saturating_sub(limit);
    Json(json!({"messages": msgs.iter().skip(skip).map(msg_json).collect::<Vec<_>>()}))
}

#[derive(Deserialize)]
struct SendReq {
    text: String,
    #[serde(default)]
    reply_to: Option<ReplyIn>,
}

/// Ответ-цитата в запросе: `id` — номер сообщения (hex, 64), `text` — выдержка.
#[derive(Deserialize)]
struct ReplyIn {
    id: String,
    text: String,
}

async fn send(
    State(st): State<Arc<MobileState>>,
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(peer): Path<String>,
    Json(req): Json<SendReq>,
) -> Response {
    if req.text.is_empty() || req.text.len() > MAX_TEXT {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if device_by_peer(&peer).is_some() {
        let Some(me) = with_devices(|d| {
            d.iter()
                .find(|x| same(&x.token_hash, &dev))
                .map(|x| x.peer_id())
        }) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        return if deliver_mail(&me, &peer, req.text.as_bytes()) {
            Json(json!({"ok": true})).into_response()
        } else {
            StatusCode::BAD_REQUEST.into_response()
        };
    }
    let Some(h) = resolve_hex(&st, &peer).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // получателя, которого узел не знает (нет ни среди контактов, ни среди видимых узлов), не принимаем: иначе письмо
    // повисло бы в очереди неподтверждённым до суточного таймаута
    if h != st.my_id && !is_known_destination(&st, &h).await {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "получатель неизвестен узлу"}))).into_response();
    }
    let reply = req.reply_to.map(|r| crate::communication::ReplyRef { msg_id: r.id, text: r.text });
    match st.chat.send_message_full(h, req.text, None, reply).await {
        Ok(m) => Json(json!({"id": hex::encode(m.msg_id.0), "ts_ms": m.timestamp})).into_response(),
        Err(e) => {
            let msg = e.to_string();
            // отказ по правилам сети (ссылка, тип файла) — ошибка запроса, а не сбой шлюза
            let code = if msg.contains("в YANDI не передаются") || msg.contains("не передаётся") { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::BAD_GATEWAY };
            (code, Json(json!({"error": msg}))).into_response()
        }
    }
}

/// Сообщения других людей, пришедшие после подтверждённого устройством. Номер сообщения — его время (мс).
async fn inbox(
    State(st): State<Arc<MobileState>>,
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 1000);
    let acked = with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, &dev))
            .map(|x| x.acked_ts)
            .unwrap_or(0)
    });
    let after: u64 = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut out: Vec<(u64, serde_json::Value)> = Vec::new();
    for peer in st.chat.list_chats().unwrap_or_default() {
        if device_by_peer(&hex::encode(peer.0)).is_some() {
            continue; // переписка компьютера с телефоном владельца: ответы уходят телефону зашифрованной почтой, а не отсюда
        }
        for m in st.chat.load_history(&peer, 200).unwrap_or_default() {
            if m.from != st.my_id && m.timestamp > acked && m.timestamp > after {
                let mut j = json!({"id": m.timestamp, "from_peer_id": hex::encode(m.from.0), "payload_b64": base64_of(m.text.as_bytes()), "ts_ms": m.timestamp, "msg_id": hex::encode(m.msg_id.0)});
                if let Some(r) = &m.reply_to {
                    j["reply_to"] = json!({"id": r.msg_id, "text": r.text});
                }
                out.push((m.timestamp, j));
            }
        }
    }
    if let Some(me) = with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, &dev))
            .map(|x| x.peer_id())
    }) {
        // почта устройства — без курсора подтверждения: удаляется только то, что устройство подтвердило по номеру (неподтверждённое старое не
        // теряется). `after` — листание: приложение проходит мимо писем, которые не может принять (старый формат), к новым
        for m in with_mail(|l| {
            l.iter()
                .filter(|m| m.to == me && m.ts > after)
                .cloned()
                .collect::<Vec<_>>()
        }) {
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

fn key_bundle_message(ed25519_pub: &[u8], x25519_pub: &[u8]) -> Vec<u8> {
    [
        b"YANDI-MOBILE-KEYS-V1\0".as_slice(),
        ed25519_pub,
        x25519_pub,
    ]
    .concat()
}

#[derive(Deserialize)]
struct AckReq {
    ids: Vec<u64>,
}

async fn inbox_ack(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Json(req): Json<AckReq>,
) -> StatusCode {
    let Some(max) = req.ids.iter().copied().max() else {
        return StatusCode::OK;
    };
    with_devices(|d| {
        if let Some(x) = d.iter_mut().find(|x| same(&x.token_hash, &dev)) {
            x.acked_ts = x.acked_ts.max(max);
        }
    });
    if let Some(me) = with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, &dev))
            .map(|x| x.peer_id())
    }) {
        // только подтверждённые письма: старое неподтверждённое (не расшифровалось, нет ключа, старый формат) остаётся в очереди
        let ids: HashSet<u64> = req.ids.iter().copied().collect();
        with_mail(|l| l.retain(|m| !(m.to == me && ids.contains(&m.ts))));
    }
    StatusCode::OK
}

/// Выход в интернет через этот компьютер всегда доступен устройству с токеном; куда именно он идёт, решает владелец (внешний прокси в настройках).
async fn proxy_info() -> Json<serde_json::Value> {
    Json(
        json!({"running": true, "host": "этот компьютер", "port": crate::mobile_tls::configured_port(), "upstream": crate::upstream_proxy::is_active()}),
    )
}

/// Публичный ключ телефона владельца (чтобы телефон шифровал сообщение для другого телефона сам); у обычных собеседников ключа нет — 404.
async fn pubkey(
    State(st): State<Arc<MobileState>>,
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(peer): Path<String>,
) -> Response {
    if peer == hex::encode(st.my_id.0) {
        // собеседником «сам компьютер» бывает только владелец: пользователю узла переписка с ПК закрыта
        let user = with_devices(|d| d.iter().any(|x| same(&x.token_hash, &dev) && x.role == "user"));
        if user {
            return StatusCode::NOT_FOUND.into_response();
        }
        // сам компьютер как собеседник телефона (переписка с веб-чатом узла)
        return Json(crate::mobile_self::bundle_json()).into_response();
    }
    match device_by_peer(&peer).and_then(|d| match (d.x25519_pub, d.ed25519_pub, d.key_sig) {
        (Some(x), Some(e), Some(s)) => {
            Some(json!({"x25519_pub": x, "ed25519_pub": e, "signature": s}))
        }
        _ => None,
    }) {
        Some(bundle) => Json(bundle).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct KeysReq {
    ed25519_pub: String,
    x25519_pub: String,
    signature: String,
}

async fn accept_pubkeys(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Json(req): Json<KeysReq>,
) -> StatusCode {
    let decode = |k: &str| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(k).ok()
    };
    let (Some(x25519), Some(ed25519), Some(signature)) = (
        decode(&req.x25519_pub),
        decode(&req.ed25519_pub),
        decode(&req.signature),
    ) else {
        return StatusCode::BAD_REQUEST;
    };
    if x25519.len() != 32 || ed25519.len() != 32 || signature.len() != 64 {
        return StatusCode::BAD_REQUEST;
    }
    let Ok(verifying_key) = VerifyingKey::from_bytes(ed25519.as_slice().try_into().unwrap()) else {
        return StatusCode::BAD_REQUEST;
    };
    let signature = Signature::from_bytes(signature.as_slice().try_into().unwrap());
    if verifying_key
        .verify(&key_bundle_message(&ed25519, &x25519), &signature)
        .is_err()
    {
        return StatusCode::BAD_REQUEST;
    }
    let mut guard = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let loaded = match std::fs::read(devices_path()) {
            Ok(bytes) => match serde_json::from_slice::<Vec<Device>>(&bytes) {
                Ok(v) => v,
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
        };
        *guard = Some(loaded);
    }
    store_device_bundle(guard.as_mut().unwrap(), &dev, req, &ed25519, &devices_path())
}

fn store_device_bundle(devices: &mut Vec<Device>, token_hash: &str, req: KeysReq, ed: &[u8], path: &std::path::Path) -> StatusCode {
    use base64::Engine;
    let mut updated = devices.clone();
    if let Some(x) = updated.iter_mut().find(|x| same(&x.token_hash, token_hash)) {
        if let Some(old) = x.ed25519_pub.as_deref() {
            let Ok(old) = base64::engine::general_purpose::STANDARD.decode(old) else {
                return StatusCode::INTERNAL_SERVER_ERROR;
            };
            if old != ed { return StatusCode::CONFLICT; }
        }
        x.x25519_pub = Some(req.x25519_pub);
        x.ed25519_pub = Some(req.ed25519_pub);
        x.key_sig = Some(req.signature);
    } else {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(bytes) = serde_json::to_vec(&updated) else { return StatusCode::INTERNAL_SERVER_ERROR };
    if crate::util::private_file::write_private(path, &bytes).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    *devices = updated;
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
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "сервер звонков не настроен на узле"})),
        )
            .into_response();
    };
    const TTL: u64 = 6 * 3600;
    let username = format!("{}:{}", now_ms() / 1000 + TTL, &dev[..dev.len().min(16)]);
    let Ok(mut mac) = Hmac::<sha1::Sha1>::new_from_slice(c.secret.as_bytes()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    Json(json!({"host": c.host, "port": c.port, "username": username, "credential": credential, "ttl": TTL})).into_response()
}

async fn ws(
    State(st): State<Arc<MobileState>>,
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    up: WebSocketUpgrade,
) -> Response {
    let Some(me) = with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, &dev))
            .map(|x| x.peer_id())
    }) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // сокет после upgrade уже не считается лимитом входа по TLS, поэтому свои пределы: всего и на устройство
    let Some(slot) = WsSlot::take(&me) else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    up.max_message_size(2 * MAX_TEXT)
        .max_frame_size(2 * MAX_TEXT)
        .on_upgrade(move |sock| async move {
            let _slot = slot;
            ws_session(st, me, sock).await
        })
}

const WS_MAX_TOTAL: usize = 128;
const WS_MAX_PER_DEVICE: usize = 4;
static WS_OPEN: Mutex<Option<std::collections::HashMap<String, usize>>> = Mutex::new(None);

/// Место открытого WebSocket; освобождается, когда сокет закрыт.
struct WsSlot(String);

impl WsSlot {
    fn take(peer: &str) -> Option<Self> {
        let mut g = WS_OPEN.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(Default::default);
        let total: usize = m.values().sum();
        let mine = m.entry(peer.to_string()).or_insert(0);
        if total >= WS_MAX_TOTAL || *mine >= WS_MAX_PER_DEVICE {
            if *mine == 0 {
                m.remove(peer);
            }
            return None;
        }
        *mine += 1;
        Some(Self(peer.to_string()))
    }
}

impl Drop for WsSlot {
    fn drop(&mut self) {
        let mut g = WS_OPEN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = g.as_mut() {
            if let Some(n) = m.get_mut(&self.0) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    m.remove(&self.0);
                }
            }
        }
    }
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
    let mut groups_rx = crate::mobile_groups::subscribe();
    let mut seen: HashSet<HashId> = HashSet::new();
    loop {
        // забытое устройство (токен отозван) не пользуется уже открытым сокетом: ни получать, ни отправлять
        if device_by_peer(&me).is_none() {
            break;
        }
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(Event::Revoked(peer)) => {
                    if peer == me { break; }
                }
                Ok(Event::Profile) => {
                    if sock.send(Message::Binary(vec![FT_PROFILE])).await.is_err() { break; }
                }
                Ok(Event::Node(m)) => {
                    // личная переписка ПК — только владельцу; пользователю узла она по WebSocket не уходит
                    if is_user_role(&me) { continue; }
                    if seen.len() > 4096 {
                        seen.clear(); // «уже показанные» не должны расти без предела на долгом соединении
                    }
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
            h = groups_rx.recv() => match h {
                Ok(h) => {
                    if let Some(f) = h.frame_for(&me) {
                        if sock.send(Message::Binary(f)).await.is_err() { break; }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            inc = sock.recv() => {
                let Some(Ok(msg)) = inc else { break };
                if device_by_peer(&me).is_none() {
                    break; // устройство забыли, пока кадр шёл: ничего из него не исполняем
                }
                let Message::Binary(d) = msg else { if matches!(msg, Message::Close(_)) { break } else { continue } };
                match d.first().copied() {
                    Some(FT_PING) => { if sock.send(Message::Binary(vec![FT_PONG])).await.is_err() { break; } }
                    Some(FT_SEND_LIVE) if d.len() >= 37 => {
                        let mut id = [0u8; 32];
                        id.copy_from_slice(&d[1..33]);
                        let len = u32::from_le_bytes([d[33], d[34], d[35], d[36]]) as usize;
                        if len == 0 || len > MAX_LIVE || d.len() < 37 + len { continue; }
                        let to = hex::encode(id);
                        if HashId(id) == st.my_id {
                            if is_user_role(&me) || crate::mobile_self::is_blocked_phone(&me) {
                                continue; // заблокированное устройство: и звонки, и сигналы компьютеру не доходят
                            }
                            // самому компьютеру: пинг поддержки квитанций — ответить; звонков на компьютер пока нет — «не на связи»
                            let blob = &d[37..37 + len];
                            let text = device_ed25519(&me).ok().and_then(|ed| crate::mobile_self::open(blob, &me, &to, 2, &ed));
                            match text {
                                Some(t) if t.starts_with(crate::mobile_self::CALL_MARKER) && crate::mobile_self::call_page_alive() => {
                                    // сигнал звонка: страница чата открыта — отдаём ей (она и отвечает)
                                    crate::mobile_self::push_call(&me, &t);
                                }
                                Some(t) if matches!(crate::mobile_self::classify(t.clone()), crate::mobile_self::Envelope::CapPing) => {
                                    node_live_to_device(&st, &me, &crate::mobile_self::cap_ping());
                                }
                                _ => {
                                    // страница не открыта (или не сигнал) — «не на связи», чтобы телефон не висел в вызове
                                    let mut f = vec![FT_PEER_OFFLINE];
                                    f.extend_from_slice(&id);
                                    if sock.send(Message::Binary(f)).await.is_err() { break; }
                                }
                            }
                            continue;
                        }
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
                        // посторонний пользователь узла не пишет от имени ПК и не шлёт P2P-сообщения владельца
                        if is_user_role(&me) && (HashId(id) == st.my_id || device_by_peer(&hex::encode(id)).is_none()) { continue; }
                        if HashId(id) == st.my_id {
                            // закрытая среда заперта (после перезапуска): сообщение не принимаем и не теряем молча, телефон видит «не доставлено»
                            #[cfg(unix)]
                            if crate::closed_api::is_locked() {
                                // среда заперта: сообщение (уже E2E для ПК) ждёт входа владельца, а не теряется; не поместилось — «не доставлено»
                                if !queue_for_pc(&me, &d[37..37 + len]) {
                                    let mut f = vec![FT_PEER_OFFLINE];
                                    f.extend_from_slice(&id);
                                    if sock.send(Message::Binary(f)).await.is_err() { break; }
                                }
                                continue;
                            }
                            // самому компьютеру: узел — конечная точка, расшифровывает и кладёт в историю веб-чата
                            phone_to_node(&st, &me, &d[37..37 + len]);
                            continue;
                        }
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
        .route("/mobile/profile", get(profile_get).put(profile_put))
        .route("/mobile/me/profile", get(device_profile_get).put(device_profile_put))
        .route("/mobile/me", axum::routing::delete(device_forget_self))
        .route("/mobile/users", get(users_list))
        .route("/mobile/complaints", get(complaints_mine).post(complaint_submit))
        .route("/mobile/users/:peer/avatar", get(user_avatar))
        .route("/mobile/profile/avatar", get(profile_avatar))
        .route("/mobile/contacts", get(contacts).post(contact_put))
        .route("/mobile/chat/:peer", get(history).post(send))
        .route("/mobile/inbox", get(inbox))
        .route("/mobile/inbox/ack", post(inbox_ack))
        .route("/mobile/proxy/info", get(proxy_info))
        .route("/mobile/pubkey/:peer", get(pubkey))
        .route("/mobile/pubkeys", post(accept_pubkeys))
        .route("/mobile/turn", get(turn_credentials))
        .route(
            "/mobile/files",
            get(crate::mobile_files::list).post(crate::mobile_files::start),
        )
        .route(
            "/mobile/files/:id",
            axum::routing::delete(crate::mobile_files::remove),
        )
        .route("/mobile/files/:id/status", get(crate::mobile_files::status))
        .route(
            "/mobile/files/:id/chunk/:idx",
            get(crate::mobile_files::get_chunk).put(crate::mobile_files::put_chunk),
        )
        .route("/mobile/files/:id/done", post(crate::mobile_files::done))
        .route("/mobile/groups", get(crate::mobile_groups::m_list).post(crate::mobile_groups::m_create))
        .route("/mobile/groups/open", get(crate::mobile_groups::m_open))
        .route("/mobile/groups/:gid/logo", get(crate::mobile_groups::m_logo))
        .route("/mobile/groups/:gid/settings", post(crate::mobile_groups::m_settings))
        .route("/mobile/groups/:gid/autoclean", post(crate::mobile_groups::m_autoclean))
        .route("/mobile/groups/:gid/owner", post(crate::mobile_groups::m_transfer))
        .route("/mobile/groups/:gid/deleted", get(crate::mobile_groups::m_deleted))
        .route("/mobile/groups/:gid/request", post(crate::mobile_groups::m_request))
        .route("/mobile/groups/:gid/requests", get(crate::mobile_groups::m_requests))
        .route("/mobile/groups/:gid/requests/:rid", axum::routing::delete(crate::mobile_groups::m_request_done))
        .route("/mobile/groups/:gid/files", post(crate::mobile_group_files::start))
        .route("/mobile/groups/:gid/files/:id", axum::routing::delete(crate::mobile_group_files::remove).get(crate::mobile_group_files::status))
        .route(
            "/mobile/groups/:gid/files/:id/chunk/:idx",
            get(crate::mobile_group_files::get_chunk).put(crate::mobile_group_files::put_chunk),
        )
        .route("/mobile/groups/:gid/files/:id/done", post(crate::mobile_group_files::done))
        .route(
            "/mobile/groups/:gid/join",
            post(crate::mobile_groups::m_join),
        )
        .route(
            "/mobile/groups/:gid/leave",
            post(crate::mobile_groups::m_leave),
        )
        .route(
            "/mobile/groups/:gid/members",
            get(crate::mobile_groups::m_members),
        )
        .route("/mobile/groups/:gid/profile", post(crate::mobile_groups::m_card_put))
        .route("/mobile/groups/:gid/proof", post(crate::mobile_groups::m_proof_put))
        .route("/mobile/groups/:gid/members/:alias/avatar", get(crate::mobile_groups::m_card_avatar))
        .route(
            "/mobile/groups/:gid/pubkey",
            post(crate::mobile_groups::m_pub),
        )
        .route(
            "/mobile/groups/:gid/keys",
            get(crate::mobile_groups::m_keys_get).post(crate::mobile_groups::m_keys_put),
        )
        .route(
            "/mobile/groups/:gid/log",
            get(crate::mobile_groups::m_log_get).post(crate::mobile_groups::m_log_post),
        )
        .route("/mobile/groups/:gid/mod", post(crate::mobile_groups::m_mod))
        .route(
            "/mobile/groups/:gid/modlog",
            get(crate::mobile_groups::m_modlog),
        )
        .route(
            "/mobile/groups/:gid/invite",
            post(crate::mobile_groups::m_invite),
        )
        .route("/mobile/ws", get(ws))
        .layer(middleware::from_fn(auth));
    Router::new()
        // сопряжение до входа: тело читается не дольше 10 с, чтобы медленным POST не держали соединения
        .route(
            "/mobile/pair",
            post(pair).layer(tower_http::timeout::TimeoutLayer::new(std::time::Duration::from_secs(10))),
        )
        .merge(guarded)
        .with_state(st)
}

/// Поток, у которого уже прочитанное начало возвращается читателю первым.
pub struct Prepended<S> {
    head: std::io::Cursor<Vec<u8>>,
    inner: S,
}

impl<S> Prepended<S> {
    pub fn new(head: Vec<u8>, inner: S) -> Self {
        Self {
            head: std::io::Cursor::new(head),
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prepended<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
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
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        b: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, b)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
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
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        if k.trim().eq_ignore_ascii_case("proxy-authorization") {
            let v = v.trim();
            if let Some(t) = v.strip_prefix("Bearer ") {
                token = t.trim().to_string();
            } else if let Some(b) = v.strip_prefix("Basic ") {
                use base64::Engine;
                if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b.trim()) {
                    token = String::from_utf8_lossy(&raw)
                        .split_once(':')
                        .map(|(_, p)| p.to_string())
                        .unwrap_or_default();
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
        let n = tokio::time::timeout(Duration::from_secs(10), tls.read(&mut tmp))
            .await
            .map_err(|_| std::io::Error::other("timeout"))??;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let rest = buf[end..].to_vec();
    let reply =
        |code: &str| format!("HTTP/1.1 {code}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let Some((target, token)) = connect_target_and_token(&head) else {
        return tls.write_all(reply("400 Bad Request").as_bytes()).await;
    };
    let h = token_hash(&token);
    // клиент выхода — это само устройство: за ним закрепляется один внешний прокси
    let client = if token.is_empty() {
        None
    } else {
        device_peer_of(&h)
    };
    let Some(client) = client else {
        return tls
            .write_all(reply("407 Proxy Authentication Required").as_bytes())
            .await;
    };
    if TUNNELS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= MAX_TUNNELS {
        TUNNELS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        return tls
            .write_all(reply("503 Service Unavailable").as_bytes())
            .await;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            TUNNELS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _g = Guard;
    let mut out =
        match crate::exit_policy::connect_public_for(&client, &target, Duration::from_secs(15))
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let code = if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "403 Forbidden"
                } else {
                    "502 Bad Gateway"
                };
                return tls.write_all(reply(code).as_bytes()).await;
            }
        };
    let _ = out.set_nodelay(true);
    tls.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;
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
    let line = first
        .split(|b| *b == b'\r' || *b == b'\n')
        .next()
        .unwrap_or(&[]);
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
        // без таймера Hyper не ограничивает чтение заголовков: молчащий клиент держал бы слот сколько угодно
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(10))
        .serve_connection(io, svc)
        .with_upgrades()
        .await
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod mail_quota_tests {
    use super::*;

    fn mail(from: &str, to: &str, ts: u64) -> Mail {
        Mail { ts, from: from.into(), to: to.into(), payload_b64: "AA==".into() }
    }

    #[test]
    fn the_mail_queue_has_a_byte_ceiling() {
        let big = "A".repeat(60_000);
        let mut l: Vec<Mail> = Vec::new();
        let mut accepted = 0;
        for i in 0..2000 {
            let m = Mail { ts: 1_000 + i as u64, from: format!("s{}", i % 50), to: format!("r{}", i % 70), payload_b64: big.clone() };
            if mail_accept(&mut l, m, 2_000) {
                accepted += 1;
            }
        }
        let bytes: usize = l.iter().map(|x| x.payload_b64.len()).sum();
        assert!(bytes <= MAX_MAIL_BYTES, "очередь не выше потолка: {bytes}");
        assert!(accepted < 2000, "часть писем отвергнута по размеру");
    }

    #[test]
    fn one_sender_cannot_push_out_someone_elses_mail() {
        let mut l = vec![mail("B", "C", 1_000)];
        let now = 2_000;
        let mut accepted = 0;
        for i in 0..(MAX_MAIL + 10) {
            if mail_accept(&mut l, mail("A", "D", 1_500 + i as u64), now) {
                accepted += 1;
            }
        }
        assert_eq!(accepted, MAX_MAIL_PER_PAIR, "одна пара упирается в свою квоту");
        assert!(l.iter().any(|x| x.from == "B" && x.to == "C"), "письмо B → C на месте");
        // разные адресаты — упирается в квоту отправителя, а чужое всё ещё на месте
        for i in 0..(MAX_MAIL + 10) {
            mail_accept(&mut l, mail("A", &format!("X{i}"), 1_600 + i as u64), now);
        }
        assert_eq!(l.iter().filter(|x| x.from == "A").count(), MAX_MAIL_PER_SENDER);
        assert!(l.iter().any(|x| x.from == "B" && x.to == "C"));
        // другой отправитель по-прежнему может писать
        assert!(mail_accept(&mut l, mail("E", "C", 1_700), now));
    }

    #[test]
    fn expired_mail_makes_room() {
        let mut l: Vec<Mail> = (0..MAX_MAIL as u64).map(|i| mail(&format!("S{}", i % 20), "C", i)).collect();
        assert!(!mail_accept(&mut l, mail("N", "C", 10), 10), "очередь полна");
        assert!(mail_accept(&mut l, mail("N", "C", MAIL_TTL_MS + 100), MAIL_TTL_MS + 100), "просроченное ушло");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_longest_silent_device_is_evicted_not_the_oldest_paired() {
        let mk = |name: &str, created: u64, seen: u64| Device { token_hash: name.into(), name: name.into(), created_ms: created, acked_ts: 0,
            x25519_pub: None, ed25519_pub: None, key_sig: None, last_seen_ms: seen, role: owner_role() };
        let d = vec![mk("old-but-active", 1_000, 9_000), mk("newer-but-silent", 2_000, 0), mk("recent", 8_000, 8_500)];
        let i = d.iter().enumerate().min_by_key(|(_, x)| x.last_activity_ms()).map(|(i, _)| i).unwrap();
        assert_eq!(d[i].name, "newer-but-silent");
        assert_eq!(mk("never-seen", 5_000, 0).last_activity_ms(), 5_000, "без визитов считается дата сопряжения");
        // старая запись devices.json без поля last_seen_ms читается (serde default)
        let old: Device = serde_json::from_str(r#"{"token_hash":"t","name":"n","created_ms":7,"acked_ts":0}"#).unwrap();
        assert_eq!(old.last_seen_ms, 0);
    }

    #[test]
    fn mobile_device_signing_pin_persists_and_cannot_be_replaced() {
        use super::*;
        use base64::Engine;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let request = |ed: u8, x: u8| KeysReq {
            x25519_pub: b64(&[x;32]), ed25519_pub: b64(&[ed;32]), signature:b64(&[0;64]),
        };
        let mut devices = vec![Device { token_hash:"test-token".into(), name:"test".into(), created_ms:0, acked_ts:0,
            x25519_pub:None, ed25519_pub:None, key_sig:None, last_seen_ms:0, role: owner_role() }];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        assert_eq!(store_device_bundle(&mut devices,"test-token",request(1,2),&[1;32],&path),StatusCode::OK);
        let mut restart: Vec<Device> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(store_device_bundle(&mut restart,"test-token",request(3,4),&[3;32],&path),StatusCode::CONFLICT);
        assert_eq!(store_device_bundle(&mut restart,"test-token",request(1,5),&[1;32],&path),StatusCode::OK);
        // Failed write must not mutate memory or existing disk state.
        assert_eq!(store_device_bundle(&mut restart,"test-token",request(1,6),&[1;32],dir.path()),StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(restart[0].x25519_pub,Some(b64(&[5;32])));
        assert_eq!(serde_json::from_slice::<Vec<Device>>(&std::fs::read(&path).unwrap()).unwrap()[0].x25519_pub,restart[0].x25519_pub);
    }
    use super::*;

    #[test]
    fn mobile_requests_are_recognised() {
        assert!(is_mobile_request(
            b"POST /mobile/pair HTTP/1.1\r\nHost: x\r\n\r\n"
        ));
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
        let (_, k) = connect_target_and_token(
            "CONNECT a.com:443 HTTP/1.1\r\nproxy-authorization: Basic eTphYmM=\r\n\r\n",
        )
        .unwrap();
        assert_eq!(k, "abc");
        assert!(connect_target_and_token("GET / HTTP/1.1\r\n\r\n").is_none());
    }

    #[tokio::test]
    async fn pairing_body_that_never_ends_is_cut_off() {
        use tower::ServiceExt;
        let app: Router = Router::new().route(
            "/mobile/pair",
            post(pair).layer(tower_http::timeout::TimeoutLayer::new(std::time::Duration::from_millis(300))),
        );
        let slow = futures_util::stream::pending::<Result<Vec<u8>, std::io::Error>>();
        let req = axum::http::Request::post("/mobile/pair")
            .header("content-type", "application/json")
            .body(axum::body::Body::from_stream(slow))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn restricted_device_cannot_send_files_by_any_path() {
        use super::*;
        use axum::extract::Path;
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::complaints::tests::fresh();
        let th = "r".repeat(64);
        with_devices(|d| {
            d.retain(|x| x.token_hash != th);
            d.push(Device { token_hash: th.clone(), name: "r".into(), created_ms: 0, acked_ts: 0, x25519_pub: None, ed25519_pub: None, key_sig: None, last_seen_ms: 0, role: owner_role() });
        });
        let peer = device_peer_of(&th).unwrap();
        let ext = || axum::Extension(DeviceKey(th.clone()));
        let id = crate::complaints::submit(&"a".repeat(64), "user", &peer, "r", "spam", "спам", &[]).unwrap();
        crate::complaints::add_measure(&id, "restrict", "сутки", now_ms() + 3_600_000).unwrap();
        let gid = "a".repeat(32);
        let status = |r: Response| r.status();
        let r = crate::mobile_group_files::start(ext(), Path(gid.clone()), Json(serde_json::from_str(r#"{"file_size":10,"total_chunks":1}"#).unwrap())).await;
        assert_eq!(status(r), StatusCode::FORBIDDEN, "групповая заливка");
        let r = crate::mobile_group_files::put_chunk(ext(), Path((gid.clone(), "b".repeat(32), 0)), axum::body::Bytes::from_static(b"x")).await;
        assert_eq!(status(r), StatusCode::FORBIDDEN, "кусок группового файла");
        let r = crate::mobile_group_files::done(ext(), Path((gid, "b".repeat(32)))).await;
        assert_eq!(status(r), StatusCode::FORBIDDEN, "завершение группового файла");
        let r = crate::mobile_files::put_chunk(ext(), Path(("b".repeat(32), 0)), axum::body::Bytes::from_static(b"x")).await;
        assert_eq!(status(r), StatusCode::FORBIDDEN, "кусок личного файла");
        let r = crate::mobile_files::done(ext(), Path("b".repeat(32))).await;
        assert_eq!(status(r), StatusCode::FORBIDDEN, "завершение личного файла");
        with_devices(|d| d.retain(|x| x.token_hash != th));
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

#[cfg(test)]
mod pc_inbox_tests {
    use super::*;

    #[test]
    fn locked_node_queues_phone_messages_per_device_with_a_limit() {
        // каталог данных общий для тестов: очищаем очередь явно
        with_pc_inbox(|l| l.clear());
        for _ in 0..PC_INBOX_PER_DEVICE {
            assert!(queue_for_pc("dev-a", b"sealed-blob"));
        }
        assert!(!queue_for_pc("dev-a", b"sealed-blob"), "сверх лимита устройства — отказ");
        assert!(queue_for_pc("dev-b", b"sealed-blob"), "другое устройство не страдает");
        assert_eq!(with_pc_inbox(|l| l.len()), PC_INBOX_PER_DEVICE + 1);
        // на диске лежит запечатанный блоб, не текст
        let raw = std::fs::read_to_string(pc_inbox_path()).unwrap();
        assert!(!raw.contains("sealed-blob"), "блоб лежит в base64");
        with_pc_inbox(|l| l.clear());
        // не легло на диск — «не доставлено», а не тихая потеря: очередь остаётся пустой
        let p = pc_inbox_path();
        let _ = std::fs::remove_file(&p);
        std::fs::create_dir_all(&p).unwrap();
        assert!(!queue_for_pc("dev-c", b"sealed-blob"), "сбой записи очереди — отказ");
        assert_eq!(with_pc_inbox(|l| l.len()), 0);
        std::fs::remove_dir_all(&p).unwrap();
        with_pc_inbox(|l| l.clear());
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;

    #[test]
    fn user_role_is_kept_away_from_the_owners_correspondence() {
        use axum::http::Method;
        for (m, p) in [(Method::GET, "/mobile/chat/abc"), (Method::POST, "/mobile/chat/abc"), (Method::GET, "/mobile/inbox"), (Method::POST, "/mobile/inbox/ack"),
                       (Method::GET, "/mobile/contacts"), (Method::POST, "/mobile/contacts"), (Method::PUT, "/mobile/profile"), (Method::GET, "/mobile/files")] {
            assert!(owner_only_path(&m, p), "{m} {p}");
        }
        for (m, p) in [(Method::GET, "/mobile/groups"), (Method::POST, "/mobile/complaints"), (Method::GET, "/mobile/me/profile"),
                       (Method::GET, "/mobile/profile"), (Method::GET, "/mobile/ws"), (Method::GET, "/mobile/groups/x/files/y"),
                       (Method::POST, "/mobile/pubkeys"), (Method::GET, "/mobile/pubkey/abc")] {
            assert!(!owner_only_path(&m, p), "{m} {p}");
        }
        // роль в старом devices.json отсутствует — это владелец
        let old: Device = serde_json::from_str(r#"{"token_hash":"t","name":"n","created_ms":7,"acked_ts":0}"#).unwrap();
        assert_eq!(old.role, "owner");
    }

    #[test]
    fn mail_that_did_not_reach_the_disk_is_not_accepted() {
        let p = mail_path();
        let _ = std::fs::remove_file(&p);
        std::fs::create_dir_all(&p).unwrap(); // запись в файл невозможна
        let m = Mail { ts: 1, from: "a".into(), to: "b".into(), payload_b64: "eA==".into() };
        let (_, saved) = with_mail_saved(|l| mail_accept(l, m.clone(), now_ms()));
        assert!(!saved, "сбой записи виден вызывающему");
        std::fs::remove_dir_all(&p).unwrap();
        with_mail(|l| l.clear());
    }

    #[test]
    fn pairing_code_carries_its_role() {
        let _ = issue_pairing_code_for("user");
        assert_eq!(PAIRING.lock().unwrap().as_ref().unwrap().role, "user");
        let _ = issue_pairing_code_for("что-то");
        assert_eq!(PAIRING.lock().unwrap().as_ref().unwrap().role, "user", "неизвестное значение — меньше прав, владелец только явно");
        let _ = issue_pairing_code();
        assert_eq!(PAIRING.lock().unwrap().as_ref().unwrap().role, "owner");
    }

    #[test]
    fn websocket_slots_are_limited_per_device_and_released() {
        let me = "ws-limit-test-device";
        let held: Vec<_> = (0..WS_MAX_PER_DEVICE).map(|_| WsSlot::take(me).expect("в пределах лимита")).collect();
        assert!(WsSlot::take(me).is_none(), "сверх лимита устройства — отказ");
        assert!(WsSlot::take("ws-limit-other").is_some(), "другое устройство не страдает");
        drop(held);
        assert!(WsSlot::take(me).is_some(), "после закрытия место свободно");
    }
}
