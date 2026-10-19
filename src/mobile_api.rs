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
/// Приглашённых устройств на узле не больше: приглашения не вытесняют никого, при нехватке места код отклоняется.
const MAX_INVITED: usize = 48;
/// Живых кодов приглашений на всём узле одновременно.
const MAX_LIVE_INVITES: usize = 64;
const INVITE_TTL: Duration = Duration::from_secs(600);
/// Неверных попыток ввода кода приглашения на весь узел за окно: больше — приглашения временно не принимаются (подбор кода).
const INVITE_FAIL_LIMIT: u32 = 20;
const INVITE_FAIL_WINDOW: Duration = Duration::from_secs(300);
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
    /// абсолютный срок (мс), если письмо гостя или гостю: назначается при приёме и не продлевается ни сменой роли, ни перезапуском
    #[serde(default)]
    exp: u64,
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
    /// Кто пригласил это устройство (номер устройства-пригласившего). Только у роли `invited`; по цепочке находится вся ветка при отзыве.
    #[serde(default)]
    invited_by: Option<String>,
}

fn owner_role() -> String {
    "owner".to_string()
}

fn is_user_role(peer: &str) -> bool {
    with_devices(|d| d.iter().any(|x| x.peer_id() == peer && x.restricted_role()))
}

/// Отправитель — пользователь узла или гость (не владелец).
pub(crate) fn is_restricted_device(dev: &str) -> bool {
    with_devices(|d| d.iter().any(|x| same(&x.token_hash, dev) && x.restricted_role()))
}

/// Устройство с таким номером — гость или пользователь узла (не владелец).
pub(crate) fn is_restricted_peer(peer: &str) -> bool {
    is_user_role(peer)
}

/// Сколько гостей сейчас на узле.
pub(crate) fn guest_count() -> usize {
    with_devices(|d| d.iter().filter(|x| x.role == "guest").count())
}

/// Для проверок в других модулях: устройство с ролью, возвращает хэш токена.
#[cfg(test)]
pub(crate) fn test_add_device_with_role(role: &str, tag: &str) -> String {
    let th = format!("{:0<64}", tag);
    with_devices(|d| {
        d.retain(|x| x.token_hash != th);
        d.push(Device { token_hash: th.clone(), name: tag.into(), created_ms: now_ms(), acked_ts: 0, x25519_pub: None, ed25519_pub: None, key_sig: None, last_seen_ms: now_ms(), role: role.into(), invited_by: None });
    });
    th
}

/// Устройство с таким номером — гость.
pub(crate) fn is_guest_peer(peer: &str) -> bool {
    with_devices(|d| d.iter().any(|x| x.role == "guest" && x.peer_id() == peer))
}

/// Гость (сопряжён без QR, docs/GUEST.md): у него всё то же, что у пользователя узла, минус группы, интернет, звонки, каталоги и файлы свыше лимитов.
pub(crate) fn is_guest_device(dev: &str) -> bool {
    with_devices(|d| d.iter().any(|x| same(&x.token_hash, dev) && x.role == "guest"))
}

/// Активность гостя по номеру устройства: обновляет «последний раз на связи», не чаще раза в минуту.
fn touch_guest(peer: &str) {
    let now = now_ms();
    with_devices(|d| {
        if let Some(x) = d.iter_mut().find(|x| x.role == "guest" && x.peer_id() == peer && now.saturating_sub(x.last_seen_ms) > 60_000) {
            x.last_seen_ms = now;
        }
    });
}

/// Видит ли устройство `me` присутствие устройства `peer`: гость не видит ничьего (P10-4), а присутствие гостей не видят пользователи узла.
fn presence_allowed(me: &str, peer: &str) -> bool {
    !is_guest_peer(me) && !(is_guest_peer(peer) && is_user_role(me))
}

/// Живые сигналы (звонки) идут только негостевым устройствам и только от них (P10-5).
fn live_allowed(me: &str) -> bool {
    !is_guest_peer(me)
}

/// Что гостю закрыто сверх закрытого пользователю: группы, прокси-выход, TURN (звонки), каталог пользователей. Файлы открыты, но под лимитами.
fn guest_blocked(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    owner_only_path(method, path)
        // профиль и жалобы пишутся в постоянное хранилище: гостю закрыты, пока владелец не примет его (docs/GUEST.md, P10-2)
        || (path == "/mobile/me/profile" && method != Method::GET)
        || (path == "/mobile/complaints" && method == Method::POST)
        || path.starts_with("/mobile/groups")
        || path.starts_with("/mobile/proxy")
        || path == "/mobile/turn"
        || path == "/mobile/users"
        || path.starts_with("/mobile/users/")
}

/// Что недоступно роли `user`: личная переписка и контакты ПК, отправка от имени узла, профиль владельца.
/// Выход в интернет через узел (`/mobile/proxy/info`) пользователю доступен (docs/ROLES.md); гостю он закрыт в `guest_blocked`.
fn owner_only_path(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    path == "/mobile/contacts"
        || path.starts_with("/mobile/chat/")
        || (path == "/mobile/profile" && method != Method::GET)
}

impl Device {
    /// `user`, `invited` и `guest` не видят личной переписки владельца.
    fn restricted_role(&self) -> bool {
        self.role == "user" || self.role == "guest" || self.role == "invited"
    }

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
    spawn_guest_purge();
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

/// Убрать из очереди ПК письма старше срока хранения почты: время письма открытое, поэтому срок действует и пока закрытая среда заперта.
/// Файл переписывается, только если что-то убрано.
fn sweep_pc_inbox() {
    let ttl = crate::mail_policy::ttl_ms();
    let now = now_ms();
    let changed = with_pc_inbox(|l| {
        let n = l.len();
        l.retain(|m| now.saturating_sub(m.ts) <= ttl);
        l.len() != n
    });
    if changed {
        let _ = with_pc_inbox_saved(|_| ());
    }
}

/// Сколько сообщений ждёт владельца ПК. Только число: содержимое (шифртекст), отправители и время наружу не выдаются, поэтому число можно
/// показывать и при запертой закрытой среде.
pub(crate) fn pc_inbox_pending() -> usize {
    sweep_pc_inbox();
    with_pc_inbox(|l| l.len())
}

/// Положить в ожидание. false — очередь переполнена (телефон увидит «не доставлено»).
fn queue_for_pc(device: &str, blob: &[u8]) -> bool {
    use base64::Engine;
    sweep_pc_inbox(); // просроченные не занимают места новым
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
/// Раз в десять минут: гости, молчавшие сутки, уходят вместе со своей почтой и квотами.
fn spawn_guest_purge() {
    crate::supervisor::supervise("guest-purge", crate::supervisor::Policy::restart(), move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
            purge_idle_guests();
        }
    });
}

fn spawn_pc_inbox_drain() {
    crate::supervisor::supervise("pc-inbox-drain", crate::supervisor::Policy::restart(), move || async move {
        use base64::Engine;
        let mut swept = false;
        let (mut announced, mut announce_tries) = (false, 0u32);
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
            // после восстановления из резервной копии участникам групп уходит извещение (метку оставляет `yandi backup restore`);
            // отправка возможна только при открытой среде, поэтому — после проверки замка, с повторами, а не один раз при старте
            if !announced {
                announce_tries += 1;
                if announce_restored_groups().await || announce_tries >= 40 {
                    announced = true;
                }
            }
            // по одному: письмо уходит из очереди только после обработки, сбой сохранения оставляет его на повтор
            while let Some(m) = with_pc_inbox(|l| l.first().cloned()) {
                let done = if now_ms().saturating_sub(m.ts) > crate::mail_policy::ttl_ms() {
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
    // гости живут только в памяти (docs/GUEST.md): на диск и в резервную копию они не попадают, после перезапуска узла телефон гостя сопрягается заново
    let persisted: Vec<&Device> = list.iter().filter(|x| x.role != "guest").collect();
    let Ok(s) = serde_json::to_string_pretty(&persisted) else {
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
// ---- приглашения новых устройств (docs/ROLES.md, «Приглашённый»)
//
// Устройство, сопряжённое по QR (роль `owner` или `user`), выдаёт одноразовый код, по которому подключается новое устройство с ролью
// `invited`: права как у пользователя, но приглашать он не может, пока администратор узла не повысит его до `user`. Кто кого
// пригласил, записывается (`Device::invited_by`): при злоупотреблении администратор отзывает всю ветку.

struct InviteCode {
    code: String,
    expires: Instant,
    inviter: String,
}

static INVITE_CODES: Mutex<Vec<InviteCode>> = Mutex::new(Vec::new());
/// Когда каждый пригласивший выдавал коды (мс): для суточного предела.
static INVITE_ISSUED: Mutex<Option<std::collections::HashMap<String, Vec<u64>>>> = Mutex::new(None);
static INVITE_FAILS: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

/// Политика приглашений, задаётся администратором (`mobile_invites.json` в каталоге данных).
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct InvitePolicy {
    pub enabled: bool,
    /// кодов в сутки от одного человека
    pub per_day: u32,
    /// живых (не использованных и не истёкших) кодов у одного человека
    pub max_live: u32,
}

impl Default for InvitePolicy {
    fn default() -> Self {
        Self { enabled: true, per_day: 10, max_live: 3 }
    }
}

fn invite_policy_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("mobile_invites.json")
}

pub fn invite_policy() -> InvitePolicy {
    match std::fs::read(invite_policy_path()) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|_| InvitePolicy { enabled: false, ..InvitePolicy::default() }), // битый файл — приглашения выключены
        Err(_) => InvitePolicy::default(),
    }
}

pub fn set_invite_policy(p: InvitePolicy) -> Result<(), String> {
    if p.per_day > 100 || p.max_live > 20 {
        return Err("пределы слишком велики (не больше 100 в сутки и 20 живых кодов)".into());
    }
    let bytes = serde_json::to_vec_pretty(&p).map_err(|e| e.to_string())?;
    crate::util::private_file::write_private(&invite_policy_path(), &bytes).map_err(|e| format!("не удалось сохранить: {e}"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum InviteError {
    Disabled,
    NotAllowed,
    TooManyLive,
    DailyLimit,
    NodeFull,
}

impl InviteError {
    fn message(&self) -> &'static str {
        match self {
            InviteError::Disabled => "приглашения на этом узле выключены администратором",
            InviteError::NotAllowed => "приглашать могут только устройства, сопряжённые по QR; приглашённое устройство получает это право от администратора",
            InviteError::TooManyLive => "уже выдано максимум действующих кодов: дождитесь их использования или отзовите",
            InviteError::DailyLimit => "дневной предел приглашений исчерпан",
            InviteError::NodeFull => "на узле нет места для новых приглашённых устройств",
        }
    }
}

fn purge_invites(codes: &mut Vec<InviteCode>) {
    let now = Instant::now();
    codes.retain(|c| c.expires > now);
}

/// Может ли устройство приглашать: роль `owner` или `user` и приглашения не выключены.
fn may_invite(peer: &str) -> bool {
    with_devices(|d| d.iter().any(|x| x.peer_id() == peer && (x.role == "owner" || x.role == "user")))
}

/// Выдать одноразовый код приглашения от устройства `inviter` (номер устройства). Возвращает код и срок жизни в секундах.
pub fn issue_device_invite(inviter: &str) -> Result<(String, u64), InviteError> {
    use rand::Rng;
    let policy = invite_policy();
    if !policy.enabled {
        return Err(InviteError::Disabled);
    }
    if !may_invite(inviter) {
        return Err(InviteError::NotAllowed);
    }
    if with_devices(|d| d.iter().filter(|x| x.role == "invited").count()) >= MAX_INVITED {
        return Err(InviteError::NodeFull);
    }
    let mut codes = INVITE_CODES.lock().unwrap_or_else(|e| e.into_inner());
    purge_invites(&mut codes);
    if codes.len() >= MAX_LIVE_INVITES || codes.iter().filter(|c| c.inviter == inviter).count() as u32 >= policy.max_live {
        return Err(InviteError::TooManyLive);
    }
    let now = now_ms();
    {
        let mut g = INVITE_ISSUED.lock().unwrap_or_else(|e| e.into_inner());
        let log = g.get_or_insert_with(Default::default);
        let mine = log.entry(inviter.to_string()).or_default();
        mine.retain(|t| now.saturating_sub(*t) < 24 * 3600 * 1000);
        if mine.len() as u32 >= policy.per_day {
            return Err(InviteError::DailyLimit);
        }
        mine.push(now);
        if log.len() > 4096 {
            log.retain(|_, v| v.iter().any(|t| now.saturating_sub(*t) < 24 * 3600 * 1000));
        }
    }
    let code = format!("{:08}", rand::thread_rng().gen_range(0..100_000_000u32));
    codes.push(InviteCode { code: code.clone(), expires: Instant::now() + INVITE_TTL, inviter: inviter.to_string() });
    Ok((code, INVITE_TTL.as_secs()))
}

/// Отозвать все живые коды этого пригласившего.
pub fn revoke_device_invites(inviter: &str) -> usize {
    let mut codes = INVITE_CODES.lock().unwrap_or_else(|e| e.into_inner());
    let before = codes.len();
    codes.retain(|c| c.inviter != inviter);
    before - codes.len()
}

/// Подошёл ли код приглашения: если да, он погашается и возвращается номер пригласившего. Неверные попытки считаются на весь узел.
fn redeem_device_invite(code: &str) -> Option<String> {
    {
        let mut f = INVITE_FAILS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((start, n)) = f.as_mut() {
            if start.elapsed() > INVITE_FAIL_WINDOW {
                *f = None;
            } else if *n >= INVITE_FAIL_LIMIT {
                return None;
            }
        }
    }
    let mut codes = INVITE_CODES.lock().unwrap_or_else(|e| e.into_inner());
    purge_invites(&mut codes);
    let hit = codes.iter().position(|c| same(&c.code, code.trim()));
    match hit {
        Some(i) => {
            let c = codes.remove(i);
            // пригласивший мог потерять право (отозван, понижен) между выдачей и вводом
            if may_invite(&c.inviter) {
                Some(c.inviter)
            } else {
                None
            }
        }
        None => {
            let mut f = INVITE_FAILS.lock().unwrap_or_else(|e| e.into_inner());
            match f.as_mut() {
                Some((_, n)) => *n += 1,
                None => *f = Some((Instant::now(), 1)),
            }
            None
        }
    }
}

/// Повысить приглашённое устройство до пользователя (после этого оно может приглашать само). Только администратор узла.
pub fn promote_device(id: &str) -> Result<(), String> {
    let peer = resolve_device(id).ok_or("нет такого устройства")?;
    if !with_devices(|d| d.iter().any(|x| x.peer_id() == peer && x.role == "invited")) {
        return Err("повысить можно только приглашённое устройство".into());
    }
    let before = with_devices(|d| d.clone());
    let ((), saved) = devices_apply(|d| {
        if let Some(x) = d.iter_mut().find(|x| x.peer_id() == peer && x.role == "invited") {
            x.role = "user".to_string();
        }
    });
    if !saved {
        devices_apply(|d| *d = before);
        return Err("не удалось сохранить изменение".into());
    }
    crate::backup_signal::notify_critical();
    Ok(())
}

/// Номера устройств ветки: само устройство и все, кого оно пригласило, и те, кого пригласили они (цепочка `invited_by`, без циклов).
pub fn branch_of(peer: &str) -> Vec<String> {
    with_devices(|d| {
        let mut out = vec![peer.to_string()];
        let mut i = 0;
        while i < out.len() && out.len() <= 4096 {
            let cur = out[i].clone();
            for x in d.iter() {
                let p = x.peer_id();
                if x.invited_by.as_deref() == Some(cur.as_str()) && !out.contains(&p) {
                    out.push(p);
                }
            }
            i += 1;
        }
        out
    })
}

/// Отозвать устройство вместе со всей его веткой приглашённых. Возвращает, сколько устройств удалено.
pub fn remove_branch(id: &str) -> usize {
    let Some(peer) = resolve_device(id) else { return 0 };
    let branch = branch_of(&peer);
    revoke_device_invites(&peer);
    // сначала самые дальние (дети), потом корень: порядок обхода в ширину, значит идём с конца
    branch.iter().rev().filter(|p| remove_device(p)).count()
}

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
pub fn device_ed_keys() -> Vec<([u8; 32], String, String)> {
    use base64::Engine;
    with_devices(|d| {
        d.iter()
            .filter_map(|x| {
                let raw = base64::engine::general_purpose::STANDARD.decode(x.key_sig.as_ref().and(x.ed25519_pub.as_deref())?).ok()?;
                Some((<[u8; 32]>::try_from(raw.as_slice()).ok()?, x.name.clone(), x.peer_id()))
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
    let all = with_devices(|d| d.clone());
    let names: std::collections::HashMap<String, String> = all.iter().map(|x| (x.peer_id(), x.name.clone())).collect();
    all.into_iter()
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
                "role": x.role,
                "invited_by": x.invited_by,
                "invited_by_name": x.invited_by.as_deref().and_then(|b| names.get(b).cloned()),
            })
        })
        .collect()
}

/// Для тестов других модулей: зарегистрировать устройство с данным токеном, вернуть его полный номер.
#[cfg(test)]
pub(crate) fn test_add_device(token: &str) -> String {
    let dev = Device {
        token_hash: token_hash(token),
        name: "тест".into(),
        created_ms: 1,
        acked_ts: 0,
        x25519_pub: None,
        ed25519_pub: None,
        key_sig: None,
        last_seen_ms: 0,
        role: owner_role(),
        invited_by: None,
    };
    let peer = dev.peer_id();
    devices_apply(|d| d.push(dev));
    peer
}

/// Полный номер устройства по его началу (не короче 8 знаков). Неоднозначное начало — отказ.
/// Все проверки прав (владение группами, пароль) надо делать над результатом, а не над сокращением.
pub fn resolve_device(id: &str) -> Option<String> {
    if id.len() < 8 {
        return None;
    }
    with_devices(|d| {
        let mut it = d.iter().map(|x| x.peer_id()).filter(|p| p.starts_with(id));
        let first = it.next()?;
        if it.next().is_some() {
            return None;
        }
        Some(first)
    })
}

/// Забыть устройство (его токен перестаёт действовать, очередь сообщений для него очищается). `id` — начало номера устройства, не короче 8 знаков.
pub fn remove_device(id: &str) -> bool {
    let Some(peer) = resolve_device(id) else {
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
    // отметка «последний раз на связи» удалённого устройства не остаётся в памяти
    if let Some(m) = LAST_SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&peer);
    }
    // файлы удалённого устройства (отправленные им и ждавшие его) не остаются на диске
    if let Ok(h) = tokio::runtime::Handle::try_current() {
        h.spawn(crate::mobile_files::drop_peer_files(peer.clone()));
    }
    crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).revoke(&peer, None);
    crate::backup_signal::notify_critical(); // устройство отозвано: старая копия вернула бы его
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
    // отметку времени ведём только для существующего устройства: закрывающийся сокет уже удалённого гостя не должен заводить запись заново
    if device_by_peer(peer).is_some() || is_node_peer(peer) {
        LAST_SEEN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Default::default)
            .insert(peer.to_string(), now_ms());
    }
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
        exp: 0,
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

/// Срок письма. У писем гостя или гостю срок абсолютный (`exp`, назначен при приёме): он не зависит от того, жив ли гость в памяти,
/// и не продлевается принятием гостя в пользователи (docs/GUEST.md). Остальные — прежние 30 дней.
fn expire_mail(l: &mut Vec<Mail>, now: u64) {
    let ttl = crate::mail_policy::ttl_ms();
    l.retain(|x| if x.exp > 0 { now < x.exp } else { now.saturating_sub(x.ts) < ttl });
}

/// Письму с гостем на любом конце — срок сутки от приёма.
fn stamp_guest_mail(m: &mut Mail, now: u64) {
    if m.exp == 0 && with_devices(|d| d.iter().any(|x| x.role == "guest" && (x.peer_id() == m.from || x.peer_id() == m.to))) {
        m.exp = now + crate::guest::IDLE.as_millis() as u64;
    }
}

/// Положить письмо в общую очередь. Просроченные письма убираются; переполнение решается отказом новому письму (с квотами на отправителя
/// и на пару), а не вытеснением чужих неполученных писем.
fn mail_accept(l: &mut Vec<Mail>, mut m: Mail, now: u64) -> bool {
    expire_mail(l, now);
    stamp_guest_mail(&mut m, now);
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



// ---- приглашения в контакты (ссылка-приглашение, docs/GUEST.md, src/invite.rs)

fn device_ed_bytes(dev: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let k = with_devices(|d| d.iter().find(|x| same(&x.token_hash, dev)).and_then(|x| x.ed25519_pub.clone()))?;
    base64::engine::general_purpose::STANDARD.decode(k).ok().filter(|v| v.len() == 32)
}

fn b64u() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn invite_refusal(r: crate::invite::Refusal) -> Response {
    use crate::invite::Refusal::*;
    let code = match r {
        InviteUsed => StatusCode::CONFLICT,
        UnknownInvite => StatusCode::NOT_FOUND,
        TooMany | NoRoom => StatusCode::TOO_MANY_REQUESTS,
        BadSignature | BadToken | NotYours | Forbidden | WrongNode => StatusCode::FORBIDDEN,
        Expired => StatusCode::GONE,
        Malformed => StatusCode::BAD_REQUEST,
    };
    (code, Json(json!({"error": r.text(), "code": format!("{r:?}")}))).into_response()
}

#[derive(Deserialize)]
struct InviteRegisterReq {
    invite: String,
    signature: String,
}

/// А регистрирует приглашение на узле: после подтверждения ссылку можно отправлять.
async fn invite_register(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<InviteRegisterReq>) -> Response {
    use base64::Engine;
    let (Some(peer), Some(ed), Some(node)) = (device_peer_of(&dev), device_ed_bytes(&dev), STATE.get().map(|s| s.my_id.0)) else {
        return (StatusCode::CONFLICT, Json(json!({"error": "сначала устройство должно передать узлу свои ключи"}))).into_response();
    };
    let (Ok(inv), Ok(sig)) = (b64u().decode(&req.invite), b64u().decode(&req.signature)) else { return invite_refusal(crate::invite::Refusal::Malformed) };
    let out = crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).register(now_ms(), &peer, &ed, &node, &inv, &sig);
    match out {
        Ok(id) => Json(json!({"id": id})).into_response(),
        Err(r) => invite_refusal(r),
    }
}

#[derive(Deserialize)]
struct InviteRevokeReq {
    #[serde(default)]
    id: Option<String>,
}

async fn invite_revoke(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<InviteRevokeReq>) -> Response {
    let Some(peer) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).revoke(&peer, req.id.as_deref());
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct InviteRedeemReq {
    invite: String,
    invite_signature: String,
    token: String,
    claim: String,
    claim_signature: String,
}

/// Б гасит приглашение: узел А атомарно отмечает «использовано» и откладывает введение для А.
async fn invite_redeem(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<InviteRedeemReq>) -> Response {
    use base64::Engine;
    let (Some(peer), Some(ed)) = (device_peer_of(&dev), device_ed_bytes(&dev)) else {
        return (StatusCode::CONFLICT, Json(json!({"error": "сначала устройство должно передать узлу свои ключи"}))).into_response();
    };
    let (Ok(inv), Ok(isig), Ok(tok), Ok(claim), Ok(csig)) = (
        b64u().decode(&req.invite),
        b64u().decode(&req.invite_signature),
        b64u().decode(&req.token),
        b64u().decode(&req.claim),
        b64u().decode(&req.claim_signature),
    ) else {
        return invite_refusal(crate::invite::Refusal::Malformed);
    };
    let out = crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).redeem(now_ms(), &peer, &ed, &inv, &isig, &tok, &claim, &csig);
    match out {
        Ok((digest, at)) => Json(json!({"claim_digest": b64u().encode(digest), "consumed_at": at})).into_response(),
        Err(r) => invite_refusal(r),
    }
}

async fn invite_claims(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let Some(peer) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    let list = crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).claims_for(now_ms(), &peer);
    Json(json!({"claims": list})).into_response()
}

#[derive(Deserialize)]
struct InviteAckReq {
    ids: Vec<String>,
}

async fn invite_claims_ack(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<InviteAckReq>) -> Response {
    let Some(peer) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).ack(&peer, &req.ids);
    Json(json!({"ok": true})).into_response()
}

// ---- запросы сопряжения: гость просит владельца принять его как пользователя узла (docs/GUEST.md)

const PAIR_REQUEST_TTL_MS: u64 = 24 * 3600 * 1000;
const MAX_PAIR_REQUESTS: usize = 50;

#[derive(Clone)]
struct PairRequest {
    id: String,
    token_hash: String,
    name: String,
    created_ms: u64,
    /// pending | accepted | rejected
    state: &'static str,
}

/// Только в оперативной памяти: ни IP, ни текста, только метка устройства и решение; перезапуск узла очередь стирает.
static PAIR_REQUESTS: Mutex<Vec<PairRequest>> = Mutex::new(Vec::new());

fn with_pair_requests<R>(f: impl FnOnce(&mut Vec<PairRequest>) -> R) -> R {
    let mut g = PAIR_REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    // срок и осиротевшие: запрос живёт сутки от подачи, пока жив сам гость
    let now = now_ms();
    let alive: std::collections::HashSet<String> = with_devices(|d| d.iter().map(|x| x.token_hash.clone()).collect());
    g.retain(|r| now.saturating_sub(r.created_ms) < PAIR_REQUEST_TTL_MS && alive.contains(&r.token_hash));
    f(&mut g)
}

/// Короткая метка устройства для сверки владельцем: первые 12 знаков номера, по четыре.
fn short_mark(peer: &str) -> String {
    peer.chars().take(12).collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

/// Гость просит сопряжения. Один запрос на устройство в сутки (отклонённый тоже занимает слот).
async fn pair_request_post(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let Some((name, role)) = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| (x.name.clone(), x.role.clone()))) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if role != "guest" {
        return (StatusCode::CONFLICT, Json(json!({"error": "устройство уже сопряжено", "state": "accepted"}))).into_response();
    }
    let out = with_pair_requests(|l| {
        if let Some(r) = l.iter().find(|r| r.token_hash == dev) {
            return Ok((r.id.clone(), r.state));
        }
        if l.iter().filter(|r| r.state == "pending").count() >= MAX_PAIR_REQUESTS {
            return Err(());
        }
        use rand::RngCore;
        let mut b = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut b);
        let id = hex::encode(b);
        l.push(PairRequest { id: id.clone(), token_hash: dev.clone(), name, created_ms: now_ms(), state: "pending" });
        Ok((id, "pending"))
    });
    match out {
        Ok((id, state)) => Json(json!({"id": id, "state": state})).into_response(),
        Err(()) => (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "у владельца сейчас слишком много запросов; повторите позже"}))).into_response(),
    }
}

/// Что с моим запросом: pending / accepted / rejected / none.
async fn pair_request_get(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let role = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.role.clone()));
    let state = if matches!(role.as_deref(), Some("user") | Some("owner") | Some("invited")) {
        "accepted"
    } else {
        with_pair_requests(|l| l.iter().find(|r| r.token_hash == dev).map(|r| r.state)).unwrap_or("none")
    };
    Json(json!({"state": state})).into_response()
}

/// Для оболочки ПК: ждущие запросы (без адресов).
pub(crate) fn pair_requests_pending() -> Vec<serde_json::Value> {
    let mut v: Vec<PairRequest> = with_pair_requests(|l| l.iter().filter(|r| r.state == "pending").cloned().collect());
    v.sort_by_key(|r| r.created_ms);
    v.into_iter()
        .map(|r| {
            let mark = with_devices(|d| d.iter().find(|x| x.token_hash == r.token_hash).map(|x| short_mark(&x.peer_id()))).unwrap_or_default();
            json!({"id": r.id, "name": r.name, "mark": mark, "created_ms": r.created_ms, "expires_ms": r.created_ms + PAIR_REQUEST_TTL_MS})
        })
        .collect()
}

/// Решение владельца. Принять = роль «пользователь узла» (владельцем устройство становится только по QR). Отклонить = слот занят до конца суток.
pub(crate) fn decide_pair_request(id: &str, accept: bool) -> Result<(), &'static str> {
    // занять запрос одним действием под замком: конкурирующее решение увидит «обрабатывается» и получит отказ
    let Some(r) = with_pair_requests(|l| {
        let x = l.iter_mut().find(|r| r.id == id && r.state == "pending")?;
        x.state = "deciding";
        Some(x.clone())
    }) else {
        return Err("запрос не найден или уже обработан");
    };
    let release = |state: &'static str| {
        with_pair_requests(|l| {
            if let Some(x) = l.iter_mut().find(|x| x.id == id) {
                x.state = state;
            }
        })
    };
    if accept {
        let mut found = false;
        let ((), saved) = devices_apply(|d| {
            if let Some(x) = d.iter_mut().find(|x| x.token_hash == r.token_hash && x.role == "guest") {
                x.role = "user".to_string();
                found = true;
            }
        });
        if !found {
            release("pending");
            return Err("устройство уже не гость");
        }
        if !saved {
            devices_apply(|d| {
                if let Some(x) = d.iter_mut().find(|x| x.token_hash == r.token_hash) {
                    x.role = "guest".to_string();
                }
            });
            release("pending");
            return Err("не удалось сохранить; попробуйте ещё раз");
        }
        crate::guest::forget(&r.token_hash);
        crate::backup_signal::notify_critical(); // новое постоянное устройство в реестре
    }
    release(if accept { "accepted" } else { "rejected" });
    Ok(())
}

#[derive(Deserialize)]
struct GuestPairReq {
    #[serde(default)]
    device_name: String,
}

/// Убрать просроченные письма и записать файл очереди только если что-то убрано (нет лишней записи при каждом вызове).
fn sweep_mail() {
    let changed = with_mail(|l| {
        let n = l.len();
        expire_mail(l, now_ms());
        l.len() != n
    });
    if changed {
        let _ = with_mail_saved(|_| ());
    }
}

/// Удалить гостей, молчавших дольше суток; вернуть, сколько удалено. Группы/почта/квоты забываются так же, как при отзыве.
/// Дешёвая часть (без почты и диска): её можно вызывать с публичной ручки; полную уборку делает таймер.
pub(crate) fn purge_idle_guests_cheap() -> usize {
    let limit = crate::guest::IDLE.as_millis() as u64;
    let now = now_ms();
    let mut gone: Vec<Device> = Vec::new();
    with_devices(|d| {
        let (stale, keep): (Vec<Device>, Vec<Device>) = std::mem::take(d).into_iter().partition(|x| x.role == "guest" && now.saturating_sub(x.last_activity_ms()) > limit);
        *d = keep;
        gone = stale;
    });
    for g in &gone {
        crate::guest::forget(&g.token_hash);
        crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).revoke(&g.peer_id(), None);
        let peer = g.peer_id();
        after_forget(peer);
    }
    gone.len()
}

/// Полная уборка (таймер): гости, письма, очереди приглашений и запросов сопряжения.
pub(crate) fn purge_idle_guests() -> usize {
    let n = purge_idle_guests_cheap();
    sweep_mail();
    sweep_pc_inbox();
    crate::invite::registry().lock().unwrap_or_else(|e| e.into_inner()).sweep(now_ms());
    with_pair_requests(|_| ());
    n
}

/// Автоматическое сопряжение без QR: если оператор узла разрешил, устройство получает роль `guest` (docs/GUEST.md). Адрес устройства здесь не читается и не хранится.
async fn pair_guest(Json(req): Json<GuestPairReq>) -> Response {
    if !crate::guest::enabled() {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "этот узел не принимает сопряжение без QR"}))).into_response();
    }
    // сначала дешёвый отказ по частоте: ни уборки, ни диска, ни перебора списков для отвергнутых запросов
    if !crate::guest::take_issue_slot(Instant::now()) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "узел сейчас принимает слишком много новых устройств; повторите позже"}))).into_response();
    }
    purge_idle_guests_cheap();
    use rand::RngCore;
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let th = token_hash(&token);
    let mut name: String = req.device_name.trim().chars().take(64).collect();
    if name.is_empty() || crate::content_policy::is_reserved_name(&name) {
        name = "Гость".to_string();
    }
    name = format!("{name} ({})", &th[..4]);
    let mut full = false;
    with_devices(|d| {
        if d.iter().filter(|x| x.role == "guest").count() >= crate::guest::MAX_GUESTS {
            full = true;
            return;
        }
        d.push(Device {
            token_hash: th.clone(),
            name,
            created_ms: now_ms(),
            acked_ts: now_ms(),
            x25519_pub: None,
            ed25519_pub: None,
            key_sig: None,
            last_seen_ms: now_ms(),
            role: "guest".to_string(),
            invited_by: None,
        });
    });
    if full {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "на этом узле нет места для новых гостей"}))).into_response();
    }
    Json(json!({
        "token": token,
        "role": "guest",
        "limits": {"max_file_bytes": crate::guest::MAX_FILE, "max_files_per_day": crate::guest::MAX_FILES_PER_DAY, "max_bytes_per_day": crate::guest::MAX_BYTES_PER_DAY, "idle_hours": 24}
    }))
    .into_response()
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
    // не код администратора — тогда код приглашения от доверенного устройства
    let (role, invited_by) = match granted {
        Some(role) => (role, None),
        None => match redeem_device_invite(&req.pairing_code) {
            Some(inviter) => {
                // приглашения никого не вытесняют: нет места — отказ (код при этом уже погашен, пригласивший выдаст новый)
                if with_devices(|d| d.iter().filter(|x| x.role == "invited").count()) >= MAX_INVITED {
                    return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": InviteError::NodeFull.message()}))).into_response();
                }
                ("invited".to_string(), Some(inviter))
            }
            None => {
                return (StatusCode::FORBIDDEN, Json(json!({"error": "bad or expired pairing code"}))).into_response();
            }
        },
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
    let mut evicted_device: Option<Device> = None;
    let th = token_hash(&token);
    let ((), saved) = devices_apply(|d| {
        // гости считаются отдельно: поток гостей не вытесняет устройства владельца и пользователей
        // приглашённые считаются отдельно и никого не вытесняют
        if role != "invited" && d.iter().filter(|x| x.role != "guest" && x.role != "invited").count() >= MAX_DEVICES {
            if let Some(i) = d.iter().enumerate().filter(|(_, x)| x.role != "guest" && x.role != "invited").min_by_key(|(_, x)| x.last_activity_ms()).map(|(i, _)| i) {
                let gone = d.remove(i);
                evicted = Some(gone.peer_id());
                evicted_device = Some(gone);
            }
        }
        let th = th.clone();
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
            invited_by,
        });
    });
    crate::backup_signal::notify_critical(); // новое устройство в реестре: копия нужна сразу
    if !saved {
        // сопряжение подтверждается только после записи списка: иначе токен не пережил бы перезапуск, а вытесненное устройство
        // осталось бы в списке уже без своих групп
        // откатывается только собственное изменение: чужое успешное (отзыв, другое сопряжение) за это время не затрагивается
        devices_apply(|d| {
            d.retain(|x| x.token_hash != th);
            if let Some(e) = evicted_device.take() {
                d.push(e);
            }
        });
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "не удалось сохранить сопряжение; попробуйте ещё раз"}))).into_response();
    }
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
    // номер письма занимается только после долговечной обработки (для сообщения — после записи в историю), см. open_deferred
    let (Some(text), Some(from)) = (crate::mobile_self::open_deferred(blob, from_device, &hex::encode(st.my_id.0), 1, &ed), hex32(from_device)) else { return true };
    let device = HashId(from);
    crate::mobile_self::unhide_phone(from_device);
    let envelope = crate::mobile_self::classify(text);
    // всё, кроме сообщения, обрабатывается «в памяти»: номер занимается сразу (отказ записи журнала — повтор письма позже)
    // предложение файла — исключение: его номер занимается после записи файлового сообщения в историю (в фоновой задаче ниже)
    let is_file_offer = matches!(&envelope, crate::mobile_self::Envelope::Plain(t) if t.starts_with(crate::mobile_self::FILE_MARKER));
    // квитанция и правка меняют историю: их номер тоже занимается только после успешной записи (ошибка диска/замка — письмо на повтор)
    let writes_history = matches!(&envelope, crate::mobile_self::Envelope::Message { .. } | crate::mobile_self::Envelope::Receipt { .. } | crate::mobile_self::Envelope::Edit { .. });
    if !writes_history && !is_file_offer && !crate::mobile_self::commit_replay(blob) {
        return false;
    }
    let text = match envelope {
        crate::mobile_self::Envelope::Receipt { read, ids } => {
            let status = if read { crate::communication::MessageStatus::Read } else { crate::communication::MessageStatus::Delivered };
            let mut stored = true;
            for id in ids {
                if let Ok(mid) = HashId::from_hex(&id) {
                    stored &= st.chat.store_local_status(&device, &mid, status.clone()).is_ok();
                }
            }
            return stored && crate::mobile_self::commit_replay(blob);
        }
        crate::mobile_self::Envelope::Edit { cmid, text } => {
            if st.chat.edit_message(&device, &crate::mobile_self::incoming_id(from_device, &cmid), text).is_err() {
                return false; // история не переписана: номер письма не занят, оно останется в очереди
            }
            return crate::mobile_self::commit_replay(blob);
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
                return false; // номер письма не занят: повтор пройдёт; квитанции «доставлено» не будет, письмо остаётся в очереди
            }
            // сообщение долговечно сохранено; номер письма занимается теперь. Не записался журнал — письмо обработается ещё раз
            // (сохранение идемпотентно по номеру сообщения), но квитанцию телефону не шлём до успешной фиксации
            if !crate::mobile_self::commit_replay(blob) {
                return false;
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
        let blob = blob.to_vec();
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
            if chat.store_local_incoming(&HashId(from), &msg).is_err() {
                // запись в историю не удалась (диск, замок среды): номер письма не занят, куски на узле целы — письмо вернуть в очередь
                if !queue_for_pc(&dev, &blob) {
                    eprintln!("[mobile] файл с телефона не записан в историю и не поставлен на повтор");
                }
                return;
            }
            // сообщение сохранено: теперь письмо окончательно обработано, а серверные куски больше не нужны
            let _ = crate::mobile_self::commit_replay(&blob);
            if let Some(id) = &receipt_id {
                crate::mobile_files::drop_transfer(id.strip_prefix("file_").unwrap_or(id)).await;
            }
            {
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
        let m = Mail { ts: next_ts(), from: node, to: device.to_string(), payload_b64: base64_of(&blob), exp: 0 };
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
    #[cfg(unix)]
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
    #[cfg(unix)]
    if crate::closed_api::is_locked() {
        return Err("закрытая среда заперта: разблокируйте её, чтобы писать".into());
    }
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
    if role == "guest" {
        if guest_blocked(req.method(), req.uri().path()) {
            return (StatusCode::FORBIDDEN, Json(json!({"error": "гостю это недоступно: группы, выход в интернет, звонки и каталоги закрыты (сопряжение без QR)"}))).into_response();
        }
        // гость, молчащий сутки, удаляется: любое обращение считается «на связи» (не чаще раза в минуту)
        let now = now_ms();
        with_devices(|d| {
            if let Some(x) = d.iter_mut().find(|x| same(&x.token_hash, &h) && now.saturating_sub(x.last_seen_ms) > 60_000) {
                x.last_seen_ms = now;
            }
        });
    }
    if (role == "user" || role == "invited") && owner_only_path(req.method(), req.uri().path()) {
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
    std::fs::read_to_string(crate::util::data_dir::data_dir().join("contacts.json"))
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

/// Есть ли версия приложения новее той, что у телефона (`?v=1.0.0-b3`): узел сам проверил подписанный манифест, телефон на сайт не ходит.
async fn app_update(axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>) -> Json<serde_json::Value> {
    Json(crate::android_update::answer(q.get("v").map(String::as_str).unwrap_or("")))
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
    let role = with_devices(|d| d.iter().find(|x| same(&x.token_hash, &dev)).map(|x| x.role.clone()));
    let can_invite = me.as_deref().is_some_and(may_invite) && invite_policy().enabled;
    Json(
        json!({"node_id": id, "name": format!("YANDI {}", &id[..8]), "version": crate::VERSION, "device_id": me, "online": true, "role": role, "can_invite": can_invite}),
    )
}

/// Выдать код приглашения нового устройства (только устройство, сопряжённое по QR: роль `owner` или `user`).
async fn device_invite_issue(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let me = me_peer_of(&dev);
    match issue_device_invite(&me) {
        Ok((code, ttl)) => Json(json!({"code": code, "expires_in_secs": ttl})).into_response(),
        Err(e) => {
            let status = match e {
                InviteError::Disabled | InviteError::NotAllowed => StatusCode::FORBIDDEN,
                InviteError::TooManyLive | InviteError::DailyLimit => StatusCode::TOO_MANY_REQUESTS,
                InviteError::NodeFull => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, Json(json!({"error": e.message()}))).into_response()
        }
    }
}

/// Отозвать свои действующие коды приглашений.
async fn device_invite_revoke(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let me = me_peer_of(&dev);
    Json(json!({"revoked": revoke_device_invites(&me)})).into_response()
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

/// Текст извещения участнику о группе, возвращённой из резервной копии.
pub fn restored_group_notice(name: &str) -> String {
    format!("Группа «{name}» восстановлена из резервной копии узла: переписка и файлы на узле не сохранены. У вас остаётся то, что вы получили раньше.")
}

/// После восстановления из копии (`yandi backup restore` оставляет метку) каждому участнику каждой группы уходит извещение от имени узла.
/// Почта у устройства сохраняется, поэтому телефон прочтёт его, когда подключится. Метка снимается после постановки в очередь.
/// Возвращает true, когда делать больше нечего: метки нет или все извещения поставлены в очередь (метка снята). Если хоть одно не ушло
/// (среда заперта, сбой), метка остаётся и вызов повторяется.
pub async fn announce_restored_groups() -> bool {
    let marker = crate::util::data_dir::data_dir().join("restored_notice.json");
    if !marker.exists() {
        return true;
    }
    let groups = crate::mobile_groups::all_groups();
    let devices: Vec<String> = with_devices(|d| d.iter().map(|x| x.peer_id()).collect());
    for (gid, name, keys) in groups {
        for device in &devices {
            if keys.contains(&crate::mobile_groups::member_key_for(&gid, device)) && pc_admin_message(device, &restored_group_notice(&name)).await.is_err() {
                return false; // метка остаётся: извещение повторится, когда отправка станет возможной
            }
        }
    }
    let _ = std::fs::remove_file(&marker);
    true
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
    let raw: serde_json::Value = std::fs::read_to_string(crate::util::data_dir::data_dir().join("contacts.json"))
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
    // просроченное (письма гостей: абсолютный срок) не выдаётся, даже если чистильщик ещё не успел
    sweep_mail();
    let acked = with_devices(|d| {
        d.iter()
            .find(|x| same(&x.token_hash, &dev))
            .map(|x| x.acked_ts)
            .unwrap_or(0)
    });
    let after: u64 = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut out: Vec<(u64, serde_json::Value)> = Vec::new();
    // посторонний пользователь узла получает только свою почту устройства: переписка ПК с внешним миром принадлежит владельцу
    let own_mail_only = is_user_role(&me_peer_of(&dev));
    for peer in if own_mail_only { Vec::new() } else { st.chat.list_chats().unwrap_or_default() } {
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

/// Номер устройства по хэшу его токена (пусто, если токен неизвестен).
fn me_peer_of(token_hash: &str) -> String {
    device_peer_of(token_hash).unwrap_or_default()
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
        let user = with_devices(|d| d.iter().any(|x| same(&x.token_hash, &dev) && x.restricted_role()));
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
    // гости на диск не пишутся (docs/GUEST.md) — и здесь тоже
    let persisted: Vec<&Device> = updated.iter().filter(|x| x.role != "guest").collect();
    let Ok(bytes) = serde_json::to_vec(&persisted) else { return StatusCode::INTERNAL_SERVER_ERROR };
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
                    // гость звонков и живых сигналов не получает (docs/GUEST.md, P10-5)
                    if m.to == me && live_allowed(&me) {
                        if let (Some(from), Ok(p)) = (hex32(&m.from), base64::engine::general_purpose::STANDARD.decode(&m.payload_b64)) {
                            if sock.send(Message::Binary(frame(FT_LIVE_MSG, &from, m.ts, &p))).await.is_err() { break; }
                        }
                    }
                }
                Ok(Event::Presence(peer, up)) => {
                    // присутствие других устройств гостю не раскрывается (P10-4), а присутствие гостей не видят пользователи узла
                    if peer != me && presence_allowed(&me, &peer) {
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
                touch_guest(&me); // кадр по живому сокету — активность гостя (не чаще раза в минуту)
                let Message::Binary(d) = msg else { if matches!(msg, Message::Close(_)) { break } else { continue } };
                match d.first().copied() {
                    Some(FT_PING) => { if sock.send(Message::Binary(vec![FT_PONG])).await.is_err() { break; } }
                    Some(FT_SEND_LIVE) if d.len() >= 37 && !live_allowed(&me) => continue, // гость не звонит и не шлёт живых сигналов
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
                                other => {
                                    // страница не открыта (или не сигнал) — «не на связи», чтобы телефон не висел в вызове
                                    if let Some(t) = &other {
                                        crate::mobile_self::note_missed_call(t);
                                    }
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
                        let m = Mail { ts: next_ts(), from: me.clone(), to, payload_b64: base64::engine::general_purpose::STANDARD.encode(&d[37..37 + len]), exp: 0 };
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
        .route("/mobile/device-invite", post(device_invite_issue).delete(device_invite_revoke))
        .route("/mobile/update", get(app_update))
        .route("/mobile/profile", get(profile_get).put(profile_put))
        .route("/mobile/me/profile", get(device_profile_get).put(device_profile_put))
        .route("/mobile/me", axum::routing::delete(device_forget_self))
        .route("/mobile/users", get(users_list))
        .route("/mobile/complaints", get(complaints_mine).post(complaint_submit))
        .route("/mobile/users/:peer/avatar", get(user_avatar))
        .route("/mobile/profile/avatar", get(profile_avatar))
        .route("/mobile/contacts", get(contacts).post(contact_put))
        .route("/mobile/chat/:peer", get(history).post(send))
        .route("/mobile/pair/request", post(pair_request_post).get(pair_request_get))
        .route("/mobile/invite", post(invite_register))
        .route("/mobile/invite/revoke", post(invite_revoke))
        .route("/mobile/invite/redeem", post(invite_redeem))
        .route("/mobile/invite/claims", get(invite_claims))
        .route("/mobile/invite/claims/ack", post(invite_claims_ack))
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
        .route(
            "/mobile/pair/guest",
            post(pair_guest).layer(tower_http::timeout::TimeoutLayer::new(std::time::Duration::from_secs(10))),
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
    // гостю выход в интернет через узел закрыт (docs/GUEST.md): проверка здесь, а не только в обычных ручках
    if is_guest_device(&h) {
        return tls.write_all(reply("403 Forbidden").as_bytes()).await;
    }
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
    // отзыв устройства (удаление владельцем) обрывает и уже открытый CONNECT, а не только новые
    let mut revoked_rx = bus().subscribe();
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
    if device_peer_of(&h).is_none() {
        return tls.write_all(reply("407 Proxy Authentication Required").as_bytes()).await; // отозвали, пока соединялись
    }
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
    let revoked = wait_revoked(&mut revoked_rx, &client, || device_peer_of(&h).is_some());
    tokio::select! { _ = up => {}, _ = down => {}, _ = revoked => {} }
    Ok(())
}

/// Ждёт отзыва устройства `client`. Пропуск событий шины (переполнение) не считается отзывом: тогда сверяемся со списком устройств.
async fn wait_revoked(rx: &mut tokio::sync::broadcast::Receiver<Event>, client: &str, still_registered: impl Fn() -> bool) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match rx.recv().await {
            Ok(Event::Revoked(p)) if p == client => return,
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => {
                if !still_registered() {
                    return;
                }
            }
            Err(RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
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
        Mail { ts, from: from.into(), to: to.into(), payload_b64: "AA==".into(), exp: 0 }
    }

    #[test]
    fn the_mail_queue_has_a_byte_ceiling() {
        let big = "A".repeat(60_000);
        let mut l: Vec<Mail> = Vec::new();
        let mut accepted = 0;
        for i in 0..2000 {
            let m = Mail { ts: 1_000 + i as u64, from: format!("s{}", i % 50), to: format!("r{}", i % 70), payload_b64: big.clone(), exp: 0 };
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
        assert!(mail_accept(&mut l, mail("N", "C", crate::mail_policy::ttl_ms() + 100), crate::mail_policy::ttl_ms() + 100), "просроченное ушло");
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn an_open_connect_ends_when_its_device_is_revoked_and_only_then() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<Event>(8);
        let waiting = tokio::task::spawn(async move {
            wait_revoked(&mut rx, "dev-a", || true).await;
        });
        let _ = tx.send(Event::Revoked("dev-b".into()));
        let _ = tx.send(Event::Profile);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "чужой отзыв не обрывает соединение");
        let _ = tx.send(Event::Revoked("dev-a".into()));
        tokio::time::timeout(Duration::from_secs(2), waiting).await.expect("отзыв своего устройства обрывает").unwrap();
    }

    #[test]
    fn the_longest_silent_device_is_evicted_not_the_oldest_paired() {
        let mk = |name: &str, created: u64, seen: u64| Device { token_hash: name.into(), name: name.into(), created_ms: created, acked_ts: 0,
            x25519_pub: None, ed25519_pub: None, key_sig: None, last_seen_ms: seen, role: owner_role(), invited_by: None };
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
            x25519_pub:None, ed25519_pub:None, key_sig:None, last_seen_ms:0, role: owner_role(), invited_by: None }];
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
            d.push(Device { token_hash: th.clone(), name: "r".into(), created_ms: 0, acked_ts: 0, x25519_pub: None, ed25519_pub: None, key_sig: None, last_seen_ms: 0, role: owner_role(), invited_by: None });
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
        for (m, p) in [(Method::GET, "/mobile/chat/abc"), (Method::POST, "/mobile/chat/abc"),                        (Method::GET, "/mobile/contacts"), (Method::POST, "/mobile/contacts"), (Method::PUT, "/mobile/profile")] {
            assert!(owner_only_path(&m, p), "{m} {p}");
        }
        for (m, p) in [(Method::GET, "/mobile/groups"), (Method::POST, "/mobile/complaints"), (Method::GET, "/mobile/me/profile"),
                       (Method::GET, "/mobile/profile"), (Method::GET, "/mobile/ws"), (Method::GET, "/mobile/groups/x/files/y"),
                       (Method::POST, "/mobile/pubkeys"), (Method::GET, "/mobile/pubkey/abc"), (Method::GET, "/mobile/proxy/info"),
                       (Method::GET, "/mobile/inbox"), (Method::POST, "/mobile/inbox/ack"), (Method::GET, "/mobile/files"), (Method::POST, "/mobile/files")] {
            assert!(!owner_only_path(&m, p), "{m} {p}");
        }
        // роль в старом devices.json отсутствует — это владелец
        let old: Device = serde_json::from_str(r#"{"token_hash":"t","name":"n","created_ms":7,"acked_ts":0}"#).unwrap();
        assert_eq!(old.role, "owner");
    }

    #[test]
    fn the_pending_count_for_the_pc_is_a_bare_number_and_follows_the_queue() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = with_pc_inbox(|l| l.clear());
        assert_eq!(pc_inbox_pending(), 0);
        assert!(queue_for_pc("dev-a", b"ciphertext-1"));
        assert!(queue_for_pc("dev-b", b"ciphertext-2"));
        assert_eq!(pc_inbox_pending(), 2);
        let _ = with_pc_inbox(|l| l.clear());
        assert_eq!(pc_inbox_pending(), 0);
    }

    #[test]
    fn mail_that_did_not_reach_the_disk_is_not_accepted() {
        let p = mail_path();
        let _ = std::fs::remove_file(&p);
        std::fs::create_dir_all(&p).unwrap(); // запись в файл невозможна
        let m = Mail { ts: 1, from: "a".into(), to: "b".into(), payload_b64: "eA==".into(), exp: 0 };
        let (_, saved) = with_mail_saved(|l| mail_accept(l, m.clone(), now_ms()));
        assert!(!saved, "сбой записи виден вызывающему");
        std::fs::remove_dir_all(&p).unwrap();
        with_mail(|l| l.clear());
    }

    #[tokio::test]
    async fn a_pairing_that_cannot_be_saved_is_refused_and_leaves_no_trace() {
        let _lock = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let p = devices_path();
        let before = with_devices(|d| d.len());
        let code = issue_pairing_code();
        let _ = std::fs::remove_file(&p);
        std::fs::create_dir_all(&p).unwrap(); // записать список невозможно
        let r = pair(Json(PairReq { pairing_code: code, device_name: "тест-сохранения".into() })).await;
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(with_devices(|d| d.len()), before, "устройство не осталось в памяти");
        assert!(with_devices(|d| d.iter().all(|x| x.name != "тест-сохранения")));
        std::fs::remove_dir_all(&p).unwrap();
        // исправно записываемое сопряжение проходит
        let code = issue_pairing_code();
        let r = pair(Json(PairReq { pairing_code: code, device_name: "тест-сохранения".into() })).await;
        assert_eq!(r.status(), StatusCode::OK);
        with_devices(|d| d.retain(|x| x.name != "тест-сохранения"));
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

#[cfg(test)]
mod guest_tests {
    use super::*;
    use axum::http::Method;
    use tower::ServiceExt;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn add(role: &str, tag: &str) -> String {
        let th = format!("{:0<64}", tag);
        with_devices(|d| {
            d.retain(|x| x.token_hash != th);
            d.push(Device { token_hash: th.clone(), name: tag.into(), created_ms: now_ms(), acked_ts: 0, x25519_pub: None, ed25519_pub: None, key_sig: None, last_seen_ms: now_ms(), role: role.into(), invited_by: None });
        });
        th
    }

    fn wipe(tags: &[&str]) {
        with_devices(|d| d.retain(|x| !tags.iter().any(|t| x.name == *t) && x.role != "guest"));
    }

    #[test]
    fn a_guest_is_kept_out_of_groups_internet_calls_directories_and_the_owners_chats() {
        for (m, p) in [
            (Method::GET, "/mobile/groups"),
            (Method::POST, "/mobile/groups"),
            (Method::POST, "/mobile/groups/abc/join"),
            (Method::GET, "/mobile/groups/abc/files/1"),
            (Method::GET, "/mobile/proxy/info"),
            (Method::GET, "/mobile/turn"),
            (Method::GET, "/mobile/users"),
            (Method::GET, "/mobile/users/abc/avatar"),
            (Method::GET, "/mobile/contacts"),
            (Method::POST, "/mobile/chat/abc"),
        ] {
            assert!(guest_blocked(&m, p), "{m} {p} должен быть закрыт гостю");
        }
        for (m, p) in [
            (Method::GET, "/mobile/inbox"),
            (Method::POST, "/mobile/inbox/ack"),
            (Method::GET, "/mobile/files"),
            (Method::POST, "/mobile/files"),
            (Method::GET, "/mobile/complaints"),
            (Method::GET, "/mobile/info"),
            (Method::GET, "/mobile/pubkey/abc"),
        ] {
            assert!(!guest_blocked(&m, p), "{m} {p} гостю нужен");
        }
    }

    #[tokio::test]
    async fn the_gateway_itself_refuses_a_guest_but_not_the_owner() {
        let _g = lock();
        crate::complaints::tests::fresh();
        let owner = add("owner", "go");
        let guest = add("guest", "gg");
        let app: Router = Router::new()
            .route("/mobile/groups", get(|| async { "ok" }))
            .route("/mobile/turn", get(|| async { "ok" }))
            .route("/mobile/proxy/info", get(|| async { "ok" }))
            .route("/mobile/inbox", get(|| async { "ok" }))
            .layer(middleware::from_fn(auth));
        let call = |path: &'static str, th: String| {
            let app = app.clone();
            async move {
                // токен — сам хэш в тесте не восстановить, поэтому кладём устройство с хэшем токена «t-…»
                let token = format!("tok-{}", &th[..2]);
                let h = token_hash(&token);
                with_devices(|d| {
                    if let Some(x) = d.iter_mut().find(|x| x.token_hash == th) {
                        x.token_hash = h;
                    }
                });
                let r = app
                    .oneshot(axum::http::Request::get(path).header("authorization", format!("Bearer {token}")).body(axum::body::Body::empty()).unwrap())
                    .await
                    .unwrap();
                r.status()
            }
        };
        // у гостя
        assert_eq!(call("/mobile/groups", guest.clone()).await, StatusCode::FORBIDDEN);
        assert_eq!(call("/mobile/turn", guest.clone()).await, StatusCode::FORBIDDEN);
        assert_eq!(call("/mobile/proxy/info", guest.clone()).await, StatusCode::FORBIDDEN);
        assert_eq!(call("/mobile/inbox", guest.clone()).await, StatusCode::OK);
        // у владельца
        assert_eq!(call("/mobile/groups", owner.clone()).await, StatusCode::OK);
        assert_eq!(call("/mobile/turn", owner.clone()).await, StatusCode::OK);
        wipe(&["go", "gg"]);
    }

    #[tokio::test]
    async fn guest_pairing_is_off_by_default_and_a_guest_lives_only_in_memory() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let _ = std::fs::remove_file(crate::util::data_dir::data_dir().join("guest_pairing.json"));
        assert!(!crate::guest::enabled(), "по умолчанию узел чужих без QR не принимает");
        let r = pair_guest(Json(GuestPairReq { device_name: "Гость".into() })).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        crate::guest::set_enabled(true).unwrap();
        let r = pair_guest(Json(GuestPairReq { device_name: "Вася".into() })).await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["role"], "guest");
        assert_eq!(v["limits"]["max_file_bytes"], 30u64 << 20);
        let token = v["token"].as_str().unwrap().to_string();
        assert!(with_devices(|d| d.iter().any(|x| x.role == "guest" && x.token_hash == token_hash(&token))));
        // на диске гостя нет: ни в реестре устройств, ни, значит, в резервной копии
        let on_disk = std::fs::read_to_string(devices_path()).unwrap_or_default();
        assert!(!on_disk.contains(&token_hash(&token)), "гость не пишется на диск");
        assert!(!on_disk.contains("guest"));
        // ответ не содержит адресов
        assert!(!String::from_utf8_lossy(&body).contains("127.0.0.1"));
        crate::guest::set_enabled(false).unwrap();
        wipe(&[]);
    }

    #[tokio::test]
    async fn guests_cannot_push_out_the_owners_devices_and_the_number_of_guests_is_capped() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        crate::guest::set_enabled(true).unwrap();
        // владельческие устройства заполняют список до предела
        let owners_before = with_devices(|d| d.iter().filter(|x| x.role == "owner").count());
        let owners: Vec<String> = (0..MAX_DEVICES).map(|i| add("owner", &format!("own{i:02}"))).collect();
        let tags: Vec<String> = (0..MAX_DEVICES).map(|i| format!("own{i:02}")).collect();
        for i in 0..crate::guest::MAX_GUESTS {
            // обходим частоту выдачи: она проверяется отдельно
            crate::guest::take_issue_slot_reset_for_test();
            let r = pair_guest(Json(GuestPairReq { device_name: format!("g{i}") })).await;
            assert_eq!(r.status(), StatusCode::OK, "гость {i}");
        }
        crate::guest::take_issue_slot_reset_for_test();
        let r = pair_guest(Json(GuestPairReq { device_name: "лишний".into() })).await;
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE, "общий потолок гостей");
        let alive = with_devices(|d| d.iter().filter(|x| x.role == "owner").count());
        assert_eq!(alive, owners_before + owners.len(), "поток гостей не вытеснил ни одного устройства владельца");
        crate::guest::set_enabled(false).unwrap();
        let t: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();
        wipe(&t);
    }

    #[test]
    fn a_guest_silent_for_a_day_is_removed_with_its_quota() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let th = add("guest", "gx");
        let fresh_one = add("guest", "gy");
        crate::guest::reserve_file(&th, 1).unwrap();
        with_devices(|d| {
            for x in d.iter_mut() {
                if x.token_hash == th {
                    x.created_ms = 1;
                    x.last_seen_ms = 1;
                }
            }
        });
        assert_eq!(purge_idle_guests(), 1);
        assert!(!with_devices(|d| d.iter().any(|x| x.token_hash == th)));
        assert!(with_devices(|d| d.iter().any(|x| x.token_hash == fresh_one)), "живой гость остаётся");
        wipe(&["gx", "gy"]);
    }

    #[tokio::test]
    async fn guest_files_obey_the_limits_and_go_only_to_guests_and_users() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let a = add("guest", "fa");
        let b = add("guest", "fb");
        let owner = add("owner", "fo");
        let to = |th: &str| device_peer_of(th).unwrap();
        let start = |from: &str, to_peer: String, size: u64, chunks: u32| {
            let ext = axum::Extension(DeviceKey(from.to_string()));
            let req: crate::mobile_files::StartReq = serde_json::from_value(json!({"to_peer_id": to_peer, "file_name": "file", "file_size": size, "total_chunks": chunks})).unwrap();
            async move { crate::mobile_files::start(ext, Json(req)).await.status() }
        };
        crate::guest::forget(&a);
        // 30 МБ + 1 байт — отказ; ровно 30 МБ (в 30 кусков по 1 МБ) — можно
        assert_eq!(start(&a, to(&b), crate::guest::MAX_FILE + 1, 31).await, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(start(&a, to(&b), crate::guest::MAX_FILE, 30).await, StatusCode::OK);
        // владельцу и самому узлу гость файлы не шлёт; пользователь узла — тоже, но другому пользователю и гостю может
        assert_eq!(start(&a, to(&owner), 1000, 1).await, StatusCode::FORBIDDEN);
        let user = add("user", "fu");
        assert_eq!(start(&user, to(&owner), 1000, 1).await, StatusCode::FORBIDDEN, "пользователь узла не шлёт файлы владельцу");
        assert_eq!(start(&user, to(&b), 1000, 1).await, StatusCode::OK, "пользователь узла шлёт гостю");
        assert_eq!(start(&a, to(&user), 1000, 1).await, StatusCode::OK, "гость шлёт пользователю");
        // 20 файлов в сутки: уже один потрачен, ещё 19 проходят, двадцать первый нет (по 100 байт)
        for _ in 0..18 {
            assert_eq!(start(&a, to(&b), 100, 1).await, StatusCode::OK);
        }
        assert_eq!(start(&a, to(&b), 100, 1).await, StatusCode::TOO_MANY_REQUESTS);
        crate::guest::forget(&a);
        // владелец лимитом гостя не связан
        assert_eq!(start(&owner, to(&b), crate::guest::MAX_FILE + 1, 31).await, StatusCode::OK);
        wipe(&["fa", "fb", "fo", "fu"]);
        let _ = std::fs::remove_dir_all(crate::util::data_dir::data_dir().join("mobile_files"));
    }

    async fn body_of(r: Response) -> serde_json::Value {
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&b).unwrap()
    }

    fn requests_reset() {
        PAIR_REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    #[tokio::test]
    async fn a_guest_asks_to_be_paired_the_owner_decides_and_the_queue_keeps_no_address() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        requests_reset();
        let a = add("guest", "qa");
        let b = add("guest", "qb");
        let ext = |t: &str| axum::Extension(DeviceKey(t.to_string()));
        // подал дважды — запрос один
        let first = body_of(pair_request_post(ext(&a)).await).await;
        let again = body_of(pair_request_post(ext(&a)).await).await;
        assert_eq!(first["id"], again["id"]);
        assert_eq!(first["state"], "pending");
        body_of(pair_request_post(ext(&b)).await).await;
        // владелец видит двоих, без адресов и без лишних полей
        let list = pair_requests_pending();
        assert_eq!(list.len(), 2);
        for r in &list {
            let keys: Vec<&String> = r.as_object().unwrap().keys().collect();
            assert!(keys.iter().all(|k| ["id", "name", "mark", "created_ms", "expires_ms"].contains(&k.as_str())), "{keys:?}");
        }
        // принять первого: роль user (не owner), на диск попадает как постоянное устройство
        let id_a = first["id"].as_str().unwrap();
        decide_pair_request(id_a, true).unwrap();
        assert_eq!(with_devices(|d| d.iter().find(|x| x.token_hash == a).map(|x| x.role.clone())).unwrap(), "user");
        assert_eq!(body_of(pair_request_get(ext(&a)).await).await["state"], "accepted");
        assert!(decide_pair_request(id_a, true).is_err(), "дважды принять нельзя");
        // отклонить второго: остаётся гостем, повторный запрос слот не освобождает
        let id_b = pair_requests_pending()[0]["id"].as_str().unwrap().to_string();
        decide_pair_request(&id_b, false).unwrap();
        assert_eq!(with_devices(|d| d.iter().find(|x| x.token_hash == b).map(|x| x.role.clone())).unwrap(), "guest");
        assert_eq!(body_of(pair_request_get(ext(&b)).await).await["state"], "rejected");
        let retry = body_of(pair_request_post(ext(&b)).await).await;
        assert_eq!(retry["state"], "rejected", "отклонённый не может завалить владельца новыми запросами");
        assert!(pair_requests_pending().is_empty());
        // уже сопряжённый просить не может
        assert_eq!(pair_request_post(ext(&a)).await.status(), StatusCode::CONFLICT);
        requests_reset();
        wipe(&["qa", "qb"]);
    }

    #[tokio::test]
    async fn the_request_queue_is_capped_and_forgets_a_vanished_guest() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        requests_reset();
        let ext = |t: &str| axum::Extension(DeviceKey(t.to_string()));
        let ths: Vec<String> = (0..=MAX_PAIR_REQUESTS).map(|i| add("guest", &format!("c{i:02}"))).collect();
        for t in ths.iter().take(MAX_PAIR_REQUESTS) {
            assert_eq!(pair_request_post(ext(t)).await.status(), StatusCode::OK);
        }
        assert_eq!(pair_request_post(ext(&ths[MAX_PAIR_REQUESTS])).await.status(), StatusCode::TOO_MANY_REQUESTS, "общий предел очереди");
        // гость исчез — его запрос тоже
        with_devices(|d| d.retain(|x| x.token_hash != ths[0]));
        assert_eq!(pair_requests_pending().len(), MAX_PAIR_REQUESTS - 1);
        // истёк срок — запрос стёрт
        with_pair_requests(|l| l[0].created_ms = 1);
        assert_eq!(pair_requests_pending().len(), MAX_PAIR_REQUESTS - 2);
        requests_reset();
        wipe(&[]);
        with_devices(|d| d.retain(|x| !x.name.starts_with('c') || x.name.len() != 3));
    }

    #[test]
    fn a_guests_mail_lives_a_day_and_other_mail_keeps_its_longer_term() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let guest = add("guest", "mg");
        let user = add("user", "mu");
        let gp = device_peer_of(&guest).unwrap();
        let up = device_peer_of(&user).unwrap();
        let day = 24 * 3600 * 1000u64;
        let now = now_ms();
        let mk = |from: &str, to: &str, age: u64| Mail { ts: now - age, from: from.into(), to: to.into(), payload_b64: "x".into(), exp: 0 };
        // срок назначается при приёме: письмо гостя получает exp = приём + сутки
        let mut l: Vec<Mail> = Vec::new();
        assert!(mail_accept(&mut l, mk(&gp, &up, 0), now));
        assert!(mail_accept(&mut l, mk(&up, &gp, 0), now));
        assert!(mail_accept(&mut l, mk(&up, &"z".repeat(64), 0), now));
        assert_eq!(l[0].exp, now + day);
        assert_eq!(l[1].exp, now + day);
        assert_eq!(l[2].exp, 0, "письмо обычных устройств без гостевого срока");
        // сутки спустя гостевые ушли, обычное осталось
        expire_mail(&mut l, now + day + 1);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].to, "z".repeat(64));
        // гость исчез (перезапуск узла, принят в пользователи): срок письма от этого не продлевается
        let mut l: Vec<Mail> = Vec::new();
        mail_accept(&mut l, mk(&gp, &up, 0), now);
        wipe(&["mg"]);
        expire_mail(&mut l, now + day + 1);
        assert!(l.is_empty(), "после исчезновения гостя письмо всё равно уходит по своему сроку");
        wipe(&["mg", "mu"]);
    }

    #[test]
    fn uploading_keys_does_not_write_a_guest_to_disk() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let owner = add("owner", "kg_o");
        let guest = add("guest", "kg_g");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let sk = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let ed = sk.verifying_key().to_bytes();
        let b = base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        let req = || KeysReq { x25519_pub: b.encode([3u8; 32]), ed25519_pub: b.encode(ed), signature: b.encode([0u8; 64]) };
        let mut devices: Vec<Device> = with_devices(|d| d.clone());
        assert_eq!(store_device_bundle(&mut devices, &guest, req(), &ed, &path), StatusCode::OK);
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains(&guest), "гость не попал в файл");
        assert!(!on_disk.contains("guest"));
        assert!(on_disk.contains(&owner), "устройство владельца в файле есть");
        wipe(&["kg_o", "kg_g"]);
    }

    #[tokio::test]
    async fn a_guest_token_cannot_open_an_internet_tunnel_through_the_connect_entrance() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let guest = add("guest", "cg");
        let h = token_hash("guest-tok");
        with_devices(|d| {
            if let Some(x) = d.iter_mut().find(|x| x.token_hash == guest) {
                x.token_hash = h.clone();
            }
        });
        let ask = |token: &'static str| async move {
            let (mut client, server) = tokio::io::duplex(8192);
            let task = tokio::task::spawn(serve_connect(server, Vec::new()));
            client.write_all(format!("CONNECT example.org:443 HTTP/1.1\r\nProxy-Authorization: Bearer {token}\r\n\r\n").as_bytes()).await.unwrap();
            let mut out = Vec::new();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), client.read_to_end(&mut out)).await;
            let _ = task.await;
            String::from_utf8_lossy(&out).to_string()
        };
        let r = ask("guest-tok").await;
        assert!(r.starts_with("HTTP/1.1 403"), "гостю — отказ до обращения к сети: {r}");
        assert!(ask("no-such-token").await.starts_with("HTTP/1.1 407"), "неизвестному — по-прежнему 407");
        wipe(&["cg"]);
    }

    #[test]
    fn a_guest_cannot_write_a_profile_or_a_complaint_but_can_read() {
        use axum::http::Method;
        assert!(guest_blocked(&Method::PUT, "/mobile/me/profile"));
        assert!(!guest_blocked(&Method::GET, "/mobile/me/profile"));
        assert!(guest_blocked(&Method::POST, "/mobile/complaints"));
        assert!(!guest_blocked(&Method::GET, "/mobile/complaints"));
    }

    #[test]
    fn presence_and_live_signals_are_closed_to_guests() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let guest = add("guest", "pg");
        let user = add("user", "pu");
        let owner = add("owner", "po");
        let (g, u, o) = (device_peer_of(&guest).unwrap(), device_peer_of(&user).unwrap(), device_peer_of(&owner).unwrap());
        assert!(!presence_allowed(&g, &o), "гость не видит присутствие владельца");
        assert!(!presence_allowed(&g, &u));
        assert!(presence_allowed(&o, &g), "владелец видит гостей");
        assert!(presence_allowed(&o, &u));
        assert!(!presence_allowed(&u, &g), "пользователи узла не видят присутствие гостей");
        assert!(presence_allowed(&u, &o));
        assert!(!live_allowed(&g));
        assert!(live_allowed(&u) && live_allowed(&o));
        wipe(&["pg", "pu", "po"]);
    }

    #[test]
    fn two_owner_sessions_deciding_at_once_leave_one_decision() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        requests_reset();
        let th = add("guest", "dq");
        let (tx, rx) = (th.clone(), th.clone());
        let _ = (tx, rx);
        with_pair_requests(|l| l.push(PairRequest { id: "r1".into(), token_hash: th.clone(), name: "dq".into(), created_ms: now_ms(), state: "pending" }));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let hs: Vec<_> = [true, false]
            .into_iter()
            .map(|accept| {
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    decide_pair_request("r1", accept).is_ok()
                })
            })
            .collect();
        let oks = hs.into_iter().map(|h| h.join().unwrap()).filter(|o| *o).count();
        assert_eq!(oks, 1, "ровно одно решение выигрывает");
        let role = with_devices(|d| d.iter().find(|x| x.token_hash == th).map(|x| x.role.clone())).unwrap();
        let state = with_pair_requests(|l| l.iter().find(|r| r.id == "r1").map(|r| r.state)).unwrap();
        assert!((role == "user" && state == "accepted") || (role == "guest" && state == "rejected"), "роль и решение согласованы: {role}/{state}");
        requests_reset();
        wipe(&["dq"]);
    }

    #[test]
    fn a_ws_frame_keeps_a_guest_alive_but_a_silent_one_is_still_purged() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        let a = add("guest", "wa");
        let b = add("guest", "wb");
        let day = 24 * 3600 * 1000u64;
        with_devices(|d| {
            for x in d.iter_mut() {
                if x.token_hash == a || x.token_hash == b {
                    x.created_ms = 1;
                    x.last_seen_ms = now_ms() - day - 1000;
                }
            }
        });
        touch_guest(&device_peer_of(&a).unwrap()); // кадр на сокете гостя A
        assert_eq!(purge_idle_guests_cheap(), 1);
        assert!(with_devices(|d| d.iter().any(|x| x.token_hash == a)), "активный через сокет остаётся");
        assert!(!with_devices(|d| d.iter().any(|x| x.token_hash == b)), "молчавший удалён");
        wipe(&["wa", "wb"]);
    }

    #[tokio::test]
    async fn a_refused_guest_request_does_not_touch_the_mail_file() {
        let _g = lock();
        crate::complaints::tests::fresh();
        wipe(&[]);
        crate::guest::set_enabled(true).unwrap();
        let _ = with_mail_saved(|l| l.clear());
        let before = std::fs::metadata(mail_path()).and_then(|m| m.modified()).ok();
        std::thread::sleep(std::time::Duration::from_millis(30));
        for _ in 0..crate::guest::MAX_ISSUE_PER_MIN {
            crate::guest::take_issue_slot(Instant::now());
        }
        let r = pair_guest(Json(GuestPairReq { device_name: "x".into() })).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        let after = std::fs::metadata(mail_path()).and_then(|m| m.modified()).ok();
        assert_eq!(before, after, "отвергнутый запрос не пишет файл почты");
        crate::guest::set_enabled(false).unwrap();
        crate::guest::take_issue_slot_reset_for_test();
        wipe(&[]);
    }

    #[test]
    fn expired_pc_mail_is_removed_even_while_the_environment_is_locked_and_frees_the_room() {
        let _g = lock();
        crate::complaints::tests::fresh();
        crate::mail_policy::reset_for_test();
        with_pc_inbox(|l| l.clear());
        let day = 24 * 3600 * 1000u64;
        crate::mail_policy::set_days(1).unwrap();
        let now = now_ms();
        with_pc_inbox(|l| {
            for i in 0..PC_INBOX_PER_DEVICE {
                l.push(PcMail { ts: now - 2 * day - i as u64, from: "dev-old".into(), blob_b64: "x".into() });
            }
            l.push(PcMail { ts: now - 1000, from: "dev-new".into(), blob_b64: "y".into() });
        });
        // лимит устройства исчерпан старыми письмами; без чистки новое не приняли бы
        assert!(queue_for_pc("dev-old", b"fresh"), "просроченные не занимают место новому письму");
        let left = with_pc_inbox(|l| l.clone());
        assert_eq!(left.len(), 2, "осталось своё свежее и письмо другого устройства");
        assert!(left.iter().all(|m| now.saturating_sub(m.ts) < day));
        // таймерная уборка тоже чистит
        with_pc_inbox(|l| l.push(PcMail { ts: now - 3 * day, from: "dev-x".into(), blob_b64: "z".into() }));
        sweep_pc_inbox();
        assert_eq!(with_pc_inbox(|l| l.len()), 2);
        with_pc_inbox(|l| l.clear());
        crate::mail_policy::reset_for_test();
    }
}

#[cfg(test)]
mod invite_tests {
    use super::*;

    static LOCK: Mutex<()> = Mutex::new(());

    fn reset() {
        INVITE_CODES.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *INVITE_ISSUED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *INVITE_FAILS.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_file(invite_policy_path());
        with_devices(|d| d.retain(|x| !x.name.starts_with("inv-")));
    }

    fn add(role: &str, tag: &str) -> String {
        let th = test_add_device_with_role(role, &format!("inv-{tag}"));
        device_peer_of(&th).expect("устройство добавлено")
    }

    async fn redeem(code: &str, name: &str) -> (StatusCode, Option<String>) {
        let r = pair(Json(PairReq { pairing_code: code.into(), device_name: name.into() })).await;
        let status = r.status();
        let peer = with_devices(|d| d.iter().find(|x| x.name == name).map(|x| x.peer_id()));
        (status, peer)
    }

    #[test]
    fn only_devices_paired_by_qr_may_invite() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let owner = add("owner", "o1");
        let user = add("user", "u1");
        let invited = add("invited", "i1");
        let guest = add("guest", "g1");
        assert!(issue_device_invite(&owner).is_ok());
        assert!(issue_device_invite(&user).is_ok());
        assert_eq!(issue_device_invite(&invited), Err(InviteError::NotAllowed));
        assert_eq!(issue_device_invite(&guest), Err(InviteError::NotAllowed));
        assert_eq!(issue_device_invite("не-устройство"), Err(InviteError::NotAllowed));
        reset();
    }

    #[test]
    fn limits_and_the_administrators_switch_apply() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let user = add("user", "u2");
        set_invite_policy(InvitePolicy { enabled: true, per_day: 4, max_live: 2 }).unwrap();
        assert!(issue_device_invite(&user).is_ok());
        assert!(issue_device_invite(&user).is_ok());
        assert_eq!(issue_device_invite(&user), Err(InviteError::TooManyLive));
        assert_eq!(revoke_device_invites(&user), 2, "отзыв снимает действующие коды");
        assert!(issue_device_invite(&user).is_ok());
        assert!(issue_device_invite(&user).is_ok());
        assert_eq!(revoke_device_invites(&user), 2);
        assert_eq!(issue_device_invite(&user), Err(InviteError::DailyLimit), "суточный предел считает и отозванные коды");
        set_invite_policy(InvitePolicy { enabled: false, ..InvitePolicy::default() }).unwrap();
        assert_eq!(issue_device_invite(&user), Err(InviteError::Disabled));
        assert!(set_invite_policy(InvitePolicy { enabled: true, per_day: 1000, max_live: 3 }).is_err(), "нелепые пределы отвергаются");
        // повреждённый файл политики выключает приглашения, а не включает их без пределов
        std::fs::write(invite_policy_path(), b"{not json").unwrap();
        assert!(!invite_policy().enabled);
        reset();
    }

    #[tokio::test]
    async fn an_invited_device_cannot_invite_until_promoted_and_the_chain_is_recorded() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = add("user", "chain-a");
        let (code, ttl) = issue_device_invite(&a).unwrap();
        assert_eq!(code.len(), 8);
        assert_eq!(ttl, 600);
        let (st, b) = redeem(&code, "inv-chain-b").await;
        assert_eq!(st, StatusCode::OK);
        let b = b.expect("устройство добавлено");
        with_devices(|d| {
            let x = d.iter().find(|x| x.peer_id() == b).unwrap();
            assert_eq!(x.role, "invited");
            assert_eq!(x.invited_by.as_deref(), Some(a.as_str()), "записано, кто пригласил");
            assert!(x.restricted_role(), "приглашённый ограничен как пользователь");
        });
        // код одноразовый
        let (st2, _) = redeem(&code, "inv-chain-b2").await;
        assert_eq!(st2, StatusCode::FORBIDDEN);
        // приглашённый приглашать не может
        assert_eq!(issue_device_invite(&b), Err(InviteError::NotAllowed));
        // повысить можно только приглашённого, и только администратор (здесь — функция узла)
        assert!(promote_device(&a[..16]).is_err(), "пользователя «повышать» нечем");
        promote_device(&b[..16]).unwrap();
        assert!(issue_device_invite(&b).is_ok(), "после повышения приглашает сам");
        reset();
    }

    #[tokio::test]
    async fn a_code_dies_with_its_inviter_and_with_time() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = add("user", "dead-a");
        let (code, _) = issue_device_invite(&a).unwrap();
        // пригласившего отозвали, пока код жил
        with_devices(|d| d.retain(|x| x.peer_id() != a));
        let (st, _) = redeem(&code, "inv-dead-b").await;
        assert_eq!(st, StatusCode::FORBIDDEN, "код отозванного пригласившего не работает");
        // истёкший код
        let a2 = add("user", "dead-a2");
        let (code2, _) = issue_device_invite(&a2).unwrap();
        INVITE_CODES.lock().unwrap().iter_mut().for_each(|c| c.expires = Instant::now() - Duration::from_secs(1));
        let (st, _) = redeem(&code2, "inv-dead-c").await;
        assert_eq!(st, StatusCode::FORBIDDEN, "истёкший код не работает");
        reset();
    }

    #[tokio::test]
    async fn guessing_codes_is_stopped_for_the_whole_node() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = add("user", "guess-a");
        let (good, _) = issue_device_invite(&a).unwrap();
        let wrong = if good == "00000000" { "11111111" } else { "00000000" };
        for i in 0..INVITE_FAIL_LIMIT {
            let (st, _) = redeem(wrong, &format!("inv-guess-{i}")).await;
            assert_eq!(st, StatusCode::FORBIDDEN);
        }
        let (st, _) = redeem(&good, "inv-guess-real").await;
        assert_eq!(st, StatusCode::FORBIDDEN, "после череды неверных попыток верный код временно не принимается");
        reset();
    }

    #[tokio::test]
    async fn invitations_never_evict_anyone_and_stop_when_the_node_is_full() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = add("user", "full-a");
        let (code, _) = issue_device_invite(&a).unwrap();
        let before_owners = with_devices(|d| d.iter().filter(|x| x.role != "invited" && x.role != "guest").count());
        for i in 0..MAX_INVITED {
            add("invited", &format!("fill{i:03}"));
        }
        assert_eq!(issue_device_invite(&a), Err(InviteError::NodeFull));
        let (st, _) = redeem(&code, "inv-full-new").await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        let after_owners = with_devices(|d| d.iter().filter(|x| x.role != "invited" && x.role != "guest").count());
        assert_eq!(before_owners, after_owners, "никого не вытеснили");
        reset();
    }

    #[tokio::test]
    async fn revoking_a_branch_removes_the_inviter_and_everyone_below() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = add("user", "br-a");
        let (c1, _) = issue_device_invite(&a).unwrap();
        let (_, b) = redeem(&c1, "inv-br-b").await;
        let b = b.unwrap();
        promote_device(&b[..16]).unwrap();
        let (c2, _) = issue_device_invite(&b).unwrap();
        let (_, c) = redeem(&c2, "inv-br-c").await;
        let c = c.unwrap();
        let other = add("user", "br-other");
        let mut branch = branch_of(&a);
        branch.sort();
        let mut expect = vec![a.clone(), b.clone(), c.clone()];
        expect.sort();
        assert_eq!(branch, expect, "ветка: сам, приглашённый им и приглашённый тем");
        assert_eq!(remove_branch(&a[..16]), 3);
        with_devices(|d| {
            assert!(d.iter().all(|x| x.peer_id() != a && x.peer_id() != b && x.peer_id() != c));
            assert!(d.iter().any(|x| x.peer_id() == other), "чужие не тронуты");
        });
        assert_eq!(remove_branch("нет-такого-устройства"), 0);
        reset();
    }

    #[test]
    fn the_overview_shows_roles_and_who_invited_whom() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = add("user", "ov-a");
        let th = test_add_device_with_role("invited", "inv-ov-b");
        with_devices(|d| d.iter_mut().filter(|x| x.token_hash == th).for_each(|x| x.invited_by = Some(a.clone())));
        let ov = devices_overview();
        let row = ov.iter().find(|r| r["name"] == "inv-ov-b").expect("есть в списке");
        assert_eq!(row["role"], "invited");
        assert_eq!(row["invited_by"], a.as_str());
        assert_eq!(row["invited_by_name"], "inv-ov-a");
        reset();
    }

    #[test]
    fn the_administrator_routes_need_a_session() {
        use axum::http::Method;
        assert!(crate::web::admin::is_admin_route(&Method::POST, "/api/mobile/devices/abcdef0123456789/promote"));
        assert!(crate::web::admin::is_admin_route(&Method::POST, "/api/mobile/devices/abcdef0123456789/revoke-branch"));
        assert!(crate::web::admin::is_admin_route(&Method::GET, "/api/mobile/invite-policy"));
        assert!(crate::web::admin::is_admin_route(&Method::POST, "/api/mobile/invite-policy"));
    }
}
