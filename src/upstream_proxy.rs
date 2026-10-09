//! Внешний прокси для выхода в интернет (этап J дорожной карты).
//!
//! Узел в стране, откуда нельзя достучаться до внешнего интернета напрямую, выходит наружу через прокси, который владелец прописал в настройках
//! (например, купленный). Прокси касается ТОЛЬКО выхода в интернет: переписка, звонки, файлы и всё остальное между узлами идёт по нашему
//! транспорту и прокси не использует.
//!
//! Правила, которые здесь держатся:
//! * все соединения выхода идут через `exit_policy::connect_public`, а она, пока прокси включён, зовёт только `connect` отсюда;
//! * имя цели передаётся прокси как есть (разрешает имя прокси, а не этот компьютер: запрос имени не утекает в обход);
//! * если прокси включён и не отвечает, соединение **не уходит напрямую**: вызывающий получает ошибку;
//! * пароль лежит в закрытом файле (права 0600) и никогда не отдаётся интерфейсом назад.
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Как говорить с прокси.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Socks5,
    /// метод CONNECT обычного HTTP(S)-прокси
    HttpConnect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    None,
    Password { user: String, password: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    pub auth: Auth,
}

impl Settings {
    /// Проверка того, что ввёл владелец. Ошибка — готовая фраза для интерфейса.
    pub fn check(&self) -> Result<(), String> {
        let host_ok = !self.host.is_empty()
            && self.host.len() <= 253
            && (self.host.parse::<std::net::IpAddr>().is_ok()
                || self.host.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')));
        if !host_ok {
            return Err("Адрес прокси: имя узла или IP без пробелов и служебных знаков.".into());
        }
        if self.port == 0 {
            return Err("Порт прокси: число от 1 до 65535.".into());
        }
        if let Auth::Password { user, password } = &self.auth {
            if user.is_empty() || user.len() > 255 || password.len() > 255 || user.chars().any(|c| c.is_control() || c == ':') || password.chars().any(char::is_control) {
                return Err("Логин не пустой, до 255 знаков и без двоеточия; пароль до 255 знаков; служебных знаков в них быть не должно.".into());
            }
        }
        Ok(())
    }
}

fn bad(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, msg.into())
}

/// Соединиться с `host:port` через прокси. Всё в пределах `limit`.
pub async fn connect(s: &Settings, host: &str, port: u16, limit: Duration) -> std::io::Result<TcpStream> {
    match tokio::time::timeout(limit, connect_inner(s, host, port)).await {
        Ok(r) => r,
        Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "прокси не ответил вовремя")),
    }
}

async fn connect_inner(s: &Settings, host: &str, port: u16) -> std::io::Result<TcpStream> {
    // имя цели идёт прокси; в запрос не должно попасть ничего служебного
    if host.is_empty() || host.len() > 253 || host.bytes().any(|b| b <= 0x20 || b == 0x7f) || port == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "неверная цель"));
    }
    let mut stream = TcpStream::connect((s.host.as_str(), s.port)).await.map_err(|e| bad(format!("прокси недоступен: {e}")))?;
    let _ = stream.set_nodelay(true);
    match s.kind {
        Kind::Socks5 => socks5(&mut stream, &s.auth, host, port).await?,
        Kind::HttpConnect => http_connect(&mut stream, &s.auth, host, port).await?,
    }
    Ok(stream)
}

async fn socks5(st: &mut TcpStream, auth: &Auth, host: &str, port: u16) -> std::io::Result<()> {
    // приветствие: умеем «без входа» и «логин и пароль» (если они заданы)
    match auth {
        Auth::None => st.write_all(&[5, 1, 0]).await?,
        Auth::Password { .. } => st.write_all(&[5, 2, 0, 2]).await?,
    }
    let mut r = [0u8; 2];
    st.read_exact(&mut r).await?;
    if r[0] != 5 {
        return Err(bad("это не SOCKS5-прокси"));
    }
    match (r[1], auth) {
        (0, _) => {}
        (2, Auth::Password { user, password }) => {
            let mut m = vec![1u8, user.len() as u8];
            m.extend_from_slice(user.as_bytes());
            m.push(password.len() as u8);
            m.extend_from_slice(password.as_bytes());
            st.write_all(&m).await?;
            let mut a = [0u8; 2];
            st.read_exact(&mut a).await?;
            if a[1] != 0 {
                return Err(bad("прокси не принял логин и пароль"));
            }
        }
        (0xff, _) => return Err(bad("прокси не принимает наш способ входа")),
        _ => return Err(bad("прокси выбрал неожиданный способ входа")),
    }
    // просьба соединиться: адрес IP как есть, имя — именем (разрешит прокси)
    let mut req = vec![5u8, 1, 0];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => {
            req.push(1);
            req.extend_from_slice(&a.octets());
        }
        Ok(std::net::IpAddr::V6(a)) => {
            req.push(4);
            req.extend_from_slice(&a.octets());
        }
        Err(_) => {
            req.push(3);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    st.write_all(&req).await?;
    let mut h = [0u8; 4];
    st.read_exact(&mut h).await?;
    if h[0] != 5 {
        return Err(bad("неверный ответ прокси"));
    }
    if h[1] != 0 {
        return Err(bad(match h[1] {
            1 => "прокси: общий отказ",
            2 => "прокси: правила прокси не пускают к этой цели",
            3 => "прокси: сеть недоступна",
            4 => "прокси: цель недоступна",
            5 => "прокси: цель отказала в соединении",
            6 => "прокси: время вышло",
            7 => "прокси: команда не поддерживается",
            8 => "прокси: такой тип адреса не поддерживается",
            _ => "прокси: неизвестный отказ",
        }));
    }
    // остаток ответа (адрес, на котором прокси соединился) прочитать и выбросить
    let rest = match h[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut l = [0u8; 1];
            st.read_exact(&mut l).await?;
            l[0] as usize + 2
        }
        _ => return Err(bad("неверный ответ прокси")),
    };
    let mut skip = vec![0u8; rest];
    st.read_exact(&mut skip).await?;
    Ok(())
}

async fn http_connect(st: &mut TcpStream, auth: &Auth, host: &str, port: u16) -> std::io::Result<()> {
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n");
    if let Auth::Password { user, password } = auth {
        use base64::Engine;
        req.push_str(&format!("Proxy-Authorization: Basic {}\r\n", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))));
    }
    req.push_str("\r\n");
    st.write_all(req.as_bytes()).await?;
    // ответ: читаем до пустой строки, но не больше 8 КБ (за концом заголовков могут быть уже данные цели — их не трогаем)
    let mut buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if buf.len() > 8192 {
            return Err(bad("слишком длинный ответ прокси"));
        }
        st.read_exact(&mut byte).await?;
        buf.push(byte[0]);
    }
    let first = String::from_utf8_lossy(&buf).lines().next().unwrap_or("").to_string();
    let code = first.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok());
    match code {
        Some(200..=299) => Ok(()),
        Some(407) => Err(bad("прокси не принял логин и пароль")),
        Some(c) => Err(bad(format!("прокси ответил отказом ({c})"))),
        None => Err(bad("это не HTTP-прокси")),
    }
}

// ---------------------------------------------------------------- пул прокси: файл, состояние, выбор
//
// Прокси может быть несколько. Каждому клиенту (телефону, соседнему узлу) закрепляется один прокси: так все его соединения выходят с одного
// внешнего адреса, и сайты не видят «скачущий» адрес в одной сессии. Новому клиенту достаётся наименее загруженный прокси, на котором ещё
// есть место (по умолчанию до 5 клиентов; число задаётся в настройках, можно своё для каждого прокси). Если мест нет, клиент всё равно
// получает самый свободный прокси (мягкий лимит) или отказ (жёсткий лимит). Прокси, который не отвечает, временно исключается (10 секунд,
// дальше вдвое дольше, до 5 минут), его клиенты переезжают на живые, а он возвращается сам после первой удачной попытки. Если мёртвы
// все, пробуется тот, у кого пауза кончается раньше; прямого выхода в обход прокси не бывает.

use std::collections::HashMap;
use std::time::Instant;

/// Клиент без своего имени (старые точки входа): соединения раскладываются по прокси по очереди, без закрепления.
pub const SHARED_CLIENT: &str = "*";
const IDLE_ASSIGN: Duration = Duration::from_secs(30 * 60);
const MAX_PROXIES: usize = 100;
const COOLDOWN_BASE: Duration = Duration::from_secs(10);
const COOLDOWN_MAX: Duration = Duration::from_secs(300);
const DEFAULT_CAP: u32 = 5;

/// Один прокси в списке.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub enabled: bool,
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    pub auth: Auth,
    /// свой предел клиентов; пусто — общий
    #[serde(default)]
    pub max_clients: Option<u32>,
}

impl Entry {
    fn settings(&self) -> Settings {
        Settings { enabled: self.enabled, kind: self.kind, host: self.host.clone(), port: self.port, auth: self.auth.clone() }
    }
    fn new(s: Settings, max_clients: Option<u32>) -> Entry {
        Entry { id: new_id(), enabled: s.enabled, kind: s.kind, host: s.host, port: s.port, auth: s.auth, max_clients }
    }
}

fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut b);
    format!("p{}", hex::encode(b))
}

fn default_true() -> bool {
    true
}
fn default_cap() -> u32 {
    DEFAULT_CAP
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PoolFile {
    /// общий выключатель: выключен — выход идёт напрямую (как без прокси)
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_cap")]
    max_clients: u32,
    /// жёсткий лимит: когда мест нет, новому клиенту отказ (иначе он садится на самый свободный)
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    proxies: Vec<Entry>,
}

impl Default for PoolFile {
    fn default() -> Self {
        PoolFile { enabled: true, max_clients: DEFAULT_CAP, strict: false, proxies: Vec::new() }
    }
}

#[derive(Default, Clone)]
struct Health {
    fails: u32,
    cooldown_until: Option<Instant>,
    last_ok: Option<Instant>,
    last_error: Option<String>,
    opened: u64,
}

struct Assign {
    proxy: String,
    seen: Instant,
}

struct Pool {
    file: PoolFile,
    health: HashMap<String, Health>,
    assign: HashMap<String, Assign>,
}

fn path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("upstream_proxy.json")
}

/// Прочитать файл: новый формат (со списком) или прежний (один прокси).
fn parse_file(bytes: &[u8]) -> Option<PoolFile> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if v.get("proxies").is_some() {
        let f: PoolFile = serde_json::from_value(v).ok()?;
        let proxies = f.proxies.into_iter().filter(|e| e.settings().check().is_ok()).collect();
        return Some(PoolFile { proxies, ..f });
    }
    let s: Settings = serde_json::from_value(v).ok()?;
    s.check().ok()?;
    Some(PoolFile { proxies: vec![Entry::new(s, None)], ..PoolFile::default() })
}

fn pool_cell() -> &'static Mutex<Pool> {
    static P: OnceLock<Mutex<Pool>> = OnceLock::new();
    P.get_or_init(|| {
        let file = std::fs::read(path()).ok().and_then(|b| parse_file(&b)).unwrap_or_default();
        Mutex::new(Pool { file, health: HashMap::new(), assign: HashMap::new() })
    })
}

fn pool() -> std::sync::MutexGuard<'static, Pool> {
    pool_cell().lock().unwrap_or_else(|e| e.into_inner())
}

fn persist(p: &Pool) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&p.file).map_err(|e| e.to_string())?;
    crate::util::private_file::write_private(&path(), &bytes).map_err(|e| format!("не удалось записать настройки: {e}"))
}

impl Pool {
    fn cap_of(&self, e: &Entry) -> usize {
        e.max_clients.unwrap_or(self.file.max_clients).max(1) as usize
    }
    fn clients_of(&self, id: &str) -> usize {
        self.assign.values().filter(|a| a.proxy == id).count()
    }
    fn available(&self, e: &Entry, now: Instant) -> bool {
        e.enabled && self.health.get(&e.id).and_then(|h| h.cooldown_until).map_or(true, |t| now >= t)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Pick {
    Proxy(String),
    /// жёсткий лимит и мест нет
    Full,
    None,
}

/// Выбрать прокси для клиента. `tried` — уже испробованные для этого соединения.
fn pick(p: &mut Pool, client: &str, now: Instant, tried: &[String]) -> Pick {
    p.assign.retain(|_, a| now.duration_since(a.seen) < IDLE_ASSIGN);
    if !p.file.enabled {
        return Pick::None;
    }
    let entries: Vec<Entry> = p.file.proxies.iter().filter(|e| e.enabled && !tried.contains(&e.id)).cloned().collect();
    if entries.is_empty() {
        return Pick::None;
    }
    let shared = client == SHARED_CLIENT;
    // закреплённый прокси, пока он жив
    if !shared {
        if let Some(a) = p.assign.get(client) {
            if entries.iter().any(|e| e.id == a.proxy && p.available(e, now)) {
                let id = a.proxy.clone();
                if let Some(a) = p.assign.get_mut(client) {
                    a.seen = now;
                }
                return Pick::Proxy(id);
            }
        }
    }
    let live: Vec<&Entry> = entries.iter().filter(|e| p.available(e, now)).collect();
    if live.is_empty() {
        // все в паузе: пробуем того, у кого она кончается раньше (так узнаём, что прокси ожил); клиента не закрепляем
        let e = entries.iter().min_by_key(|e| p.health.get(&e.id).and_then(|h| h.cooldown_until).unwrap_or(now)).expect("entries not empty");
        return Pick::Proxy(e.id.clone());
    }
    let opened = |e: &Entry| p.health.get(&e.id).map_or(0, |h| h.opened);
    let chosen = if shared {
        // без закрепления: по очереди (у кого меньше всего открыто соединений)
        live.iter().min_by_key(|e| opened(e)).map(|e| e.id.clone())
    } else {
        let free = live.iter().filter(|e| p.clients_of(&e.id) < p.cap_of(e)).min_by_key(|e| (p.clients_of(&e.id), opened(e))).map(|e| e.id.clone());
        match free {
            Some(id) => Some(id),
            None if p.file.strict => return Pick::Full,
            // мягкий лимит: самый свободный по доле занятых мест
            None => live
                .iter()
                .min_by(|a, b| {
                    let fa = p.clients_of(&a.id) as f64 / p.cap_of(a) as f64;
                    let fb = p.clients_of(&b.id) as f64 / p.cap_of(b) as f64;
                    fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|e| e.id.clone()),
        }
    };
    let Some(id) = chosen else { return Pick::None };
    if !shared {
        p.assign.insert(client.to_string(), Assign { proxy: id.clone(), seen: now });
    }
    Pick::Proxy(id)
}

fn note_ok(cell: &Mutex<Pool>, id: &str, now: Instant) {
    let mut g = cell.lock().unwrap_or_else(|e| e.into_inner());
    let h = g.health.entry(id.to_string()).or_default();
    h.fails = 0;
    h.cooldown_until = None;
    h.last_ok = Some(now);
    h.last_error = None;
    h.opened += 1;
}

fn note_fail(cell: &Mutex<Pool>, id: &str, err: &str, now: Instant) {
    let mut g = cell.lock().unwrap_or_else(|e| e.into_inner());
    let h = g.health.entry(id.to_string()).or_default();
    h.fails = h.fails.saturating_add(1);
    let wait = COOLDOWN_BASE.saturating_mul(1u32 << (h.fails - 1).min(5)).min(COOLDOWN_MAX);
    h.cooldown_until = Some(now + wait);
    h.last_error = Some(err.to_string());
}

/// Ошибка самого прокси (не отвечает, не принял логин, ответил не по протоколу) или цели (прокси работает, цель недоступна или закрыта).
/// Штрафуем только прокси.
fn proxy_fault(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    if matches!(e.kind(), InvalidInput | PermissionDenied) {
        return false;
    }
    let m = e.to_string();
    // ответы прокси о цели: «прокси: …» (коды SOCKS5) и «прокси ответил отказом (…)» (HTTP)
    !(m.starts_with("прокси: ") || m.starts_with("прокси ответил отказом"))
}

/// Включён ли выход через прокси: общий выключатель и хотя бы один включённый прокси.
pub fn is_active() -> bool {
    let g = pool();
    g.file.enabled && g.file.proxies.iter().any(|e| e.enabled)
}

/// Соединиться с целью от имени клиента через подходящий прокси. Если выбранный прокси не отвечает, пробуются другие (до трёх).
pub async fn connect_for(client: &str, host: &str, port: u16, limit: Duration) -> std::io::Result<TcpStream> {
    connect_in(pool_cell(), client, host, port, limit).await
}

async fn connect_in(cell: &Mutex<Pool>, client: &str, host: &str, port: u16, limit: Duration) -> std::io::Result<TcpStream> {
    let started = Instant::now();
    let mut tried: Vec<String> = Vec::new();
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..3 {
        let left = limit.saturating_sub(started.elapsed());
        if attempt > 0 && left < Duration::from_secs(2) {
            break;
        }
        let (id, settings) = {
            let mut g = cell.lock().unwrap_or_else(|e| e.into_inner());
            match pick(&mut g, client, Instant::now(), &tried) {
                Pick::Proxy(id) => {
                    let Some(e) = g.file.proxies.iter().find(|e| e.id == id) else { break };
                    let s = e.settings();
                    (id, s)
                }
                Pick::Full => return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "на всех прокси занято (жёсткий лимит клиентов)")),
                Pick::None => break,
            }
        };
        // первая попытка — со всем временем; запасные — с тем, что осталось
        let r = connect(&settings, host, port, if attempt == 0 { limit } else { left }).await;
        match &r {
            Ok(_) => note_ok(cell, &id, Instant::now()),
            Err(e) if proxy_fault(e) => {
                note_fail(cell, &id, &e.to_string(), Instant::now());
                last = Some(std::io::Error::new(e.kind(), e.to_string()));
                tried.push(id);
                continue;
            }
            // прокси ответил; отказала цель
            Err(_) => note_ok(cell, &id, Instant::now()),
        }
        return r;
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("нет включённых прокси")))
}

// ---- обратная совместимость: один «основной» прокси (первый в списке) для прежнего вида настроек

/// Основной прокси (первый в списке) в прежнем виде; `enabled` — включён ли он и общий выключатель.
pub fn current() -> Option<Settings> {
    let g = pool();
    let master = g.file.enabled;
    g.file.proxies.first().map(|e| Settings { enabled: e.enabled && master, ..e.settings() })
}

/// Сохранить основной прокси: заменяет первый в списке (или создаёт его), остальные не трогает.
pub fn save(s: Settings) -> Result<(), String> {
    s.check()?;
    let mut g = pool();
    match g.file.proxies.first_mut() {
        Some(e) => {
            e.enabled = s.enabled;
            e.kind = s.kind;
            e.host = s.host;
            e.port = s.port;
            e.auth = s.auth;
        }
        None => g.file.proxies.push(Entry::new(s.clone(), None)),
    }
    if s.enabled {
        g.file.enabled = true;
    }
    persist(&g)
}

/// Для проверок: подменить пул одним прокси в памяти, не трогая файл настроек.
#[cfg(test)]
pub fn set_for_test(s: Option<Settings>) {
    let mut g = pool();
    g.file = PoolFile { proxies: s.map(|s| vec![Entry::new(s, None)]).unwrap_or_default(), ..PoolFile::default() };
    g.health.clear();
    g.assign.clear();
}

// ---- состояние для страницы

fn secs_left(t: Option<Instant>, now: Instant) -> Option<u64> {
    t.and_then(|t| t.checked_duration_since(now)).map(|d| d.as_secs() + 1)
}

fn entry_view(g: &Pool, e: &Entry, now: Instant) -> serde_json::Value {
    let h = g.health.get(&e.id).cloned().unwrap_or_default();
    let (auth, user, has_password) = match &e.auth {
        Auth::None => ("none", String::new(), false),
        Auth::Password { user, password } => ("password", user.clone(), !password.is_empty()),
    };
    let cooling = secs_left(h.cooldown_until, now);
    let state = if !e.enabled {
        "disabled"
    } else if cooling.is_some() {
        "cooldown"
    } else if h.last_ok.is_some() {
        "ok"
    } else {
        "unknown"
    };
    json!({
        "id": e.id, "enabled": e.enabled, "kind": e.kind, "host": e.host, "port": e.port, "auth": auth, "user": user, "has_password": has_password,
        "max_clients": e.max_clients, "cap": g.cap_of(e), "clients": g.clients_of(&e.id), "state": state,
        "last_ok_secs_ago": h.last_ok.map(|t| now.duration_since(t).as_secs()), "last_error": h.last_error, "cooldown_secs": cooling,
        "fails": h.fails, "opened": h.opened,
    })
}

fn pool_view() -> serde_json::Value {
    let now = Instant::now();
    let g = pool();
    let proxies: Vec<_> = g.file.proxies.iter().map(|e| entry_view(&g, e, now)).collect();
    json!({"enabled": g.file.enabled, "max_clients": g.file.max_clients, "strict": g.file.strict, "proxies": proxies, "limit": MAX_PROXIES})
}

/// Что видит прежний вид настроек: основной прокси; всё, кроме пароля.
fn view() -> serde_json::Value {
    let now = Instant::now();
    let g = pool();
    match g.file.proxies.first() {
        Some(e) => {
            let h = g.health.get(&e.id).cloned().unwrap_or_default();
            let (auth, user, has_password) = match &e.auth {
                Auth::None => ("none", String::new(), false),
                Auth::Password { user, password } => ("password", user.clone(), !password.is_empty()),
            };
            json!({"configured": true, "enabled": e.enabled && g.file.enabled, "kind": e.kind, "host": e.host, "port": e.port, "auth": auth, "user": user, "has_password": has_password,
                   "last_ok_secs_ago": h.last_ok.map(|t| now.duration_since(t).as_secs()), "last_error": h.last_error})
        }
        None => json!({"configured": false, "enabled": false, "kind": "socks5", "host": "", "port": 1080, "auth": "none", "user": "", "has_password": false,
                       "last_ok_secs_ago": null, "last_error": null}),
    }
}

// ---- разбор списка прокси, вставленного текстом

/// Строки в одном из видов: `схема://логин:пароль@хост:порт`, `логин:пароль@хост:порт`, `хост:порт:логин:пароль`, `хост:порт`.
/// Возвращает найденное и сообщения о строках, которые разобрать не удалось.
pub fn parse_lines(text: &str, default_kind: Kind) -> (Vec<Settings>, Vec<String>) {
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match parse_line(line, default_kind) {
            Ok(s) => ok.push(s),
            Err(e) => errors.push(format!("строка {}: {e}", n + 1)),
        }
    }
    (ok, errors)
}

fn parse_line(line: &str, default_kind: Kind) -> Result<Settings, String> {
    let mut kind = default_kind;
    let mut rest = line;
    if let Some((scheme, r)) = line.split_once("://") {
        kind = match scheme.to_ascii_lowercase().as_str() {
            "socks5" | "socks5h" | "socks" => Kind::Socks5,
            "http" | "https" => Kind::HttpConnect,
            other => return Err(format!("неизвестная схема «{other}»")),
        };
        rest = r.trim_end_matches('/');
    }
    let host_port = |hp: &str| -> Result<(String, u16), String> {
        let (h, p) = hp.rsplit_once(':').ok_or("нет порта")?;
        let port: u16 = p.parse().map_err(|_| "порт — число от 1 до 65535".to_string())?;
        Ok((h.trim_start_matches('[').trim_end_matches(']').to_string(), port))
    };
    let (host, port, auth) = if let Some((cred, hp)) = rest.rsplit_once('@') {
        let (user, password) = cred.split_once(':').ok_or("логин и пароль через двоеточие")?;
        let (h, p) = host_port(hp)?;
        (h, p, Auth::Password { user: user.to_string(), password: password.to_string() })
    } else {
        let parts: Vec<&str> = rest.splitn(4, ':').collect();
        match parts.len() {
            2 => {
                let (h, p) = host_port(rest)?;
                (h, p, Auth::None)
            }
            4 => {
                let port: u16 = parts[1].parse().map_err(|_| "порт — число от 1 до 65535".to_string())?;
                (parts[0].to_string(), port, Auth::Password { user: parts[2].to_string(), password: parts[3].to_string() })
            }
            _ => return Err("ожидается хост:порт, хост:порт:логин:пароль или логин:пароль@хост:порт".into()),
        }
    };
    let s = Settings { enabled: true, kind, host: host.trim().to_string(), port, auth };
    s.check()?;
    Ok(s)
}

// ---------------------------------------------------------------- страница настроек (под проверкой входа владельца)

use axum::{
    extract::{Json, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Form {
    enabled: bool,
    kind: Kind,
    host: String,
    port: u16,
    /// "none" или "password"
    auth: String,
    #[serde(default)]
    user: String,
    /// пусто или не прислан — оставить прежний (для того же логина)
    #[serde(default)]
    password: String,
}

fn from_form(f: Form, old: Option<Settings>) -> Result<Settings, String> {
    let auth = match f.auth.as_str() {
        "none" => Auth::None,
        "password" => {
            let password = if !f.password.is_empty() {
                f.password
            } else {
                match old.as_ref().map(|o| &o.auth) {
                    Some(Auth::Password { user, password }) if *user == f.user => password.clone(),
                    _ => String::new(),
                }
            };
            Auth::Password { user: f.user, password }
        }
        _ => return Err("Тип авторизации: «нет» или «логин и пароль».".into()),
    };
    let s = Settings { enabled: f.enabled, kind: f.kind, host: f.host.trim().to_string(), port: f.port, auth };
    s.check()?;
    Ok(s)
}

fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": msg.into()}))).into_response()
}

async fn get_proxy() -> Json<serde_json::Value> {
    Json(view())
}

async fn post_proxy(Json(f): Json<Form>) -> Response {
    let s = match from_form(f, current()) {
        Ok(s) => s,
        Err(e) => return bad_request(e),
    };
    match save(s) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": e}))).into_response(),
    }
}

/// Проверка: соединиться через заданные настройки (даже если прокси выключен) с общеизвестным публичным адресом и ничего не передавать.
async fn test_settings(s: Settings) -> Json<serde_json::Value> {
    let t = Instant::now();
    match connect(&s, "1.1.1.1", 443, Duration::from_secs(10)).await {
        Ok(_) => Json(json!({"ok": true, "ms": t.elapsed().as_millis() as u64})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

async fn test_proxy() -> Json<serde_json::Value> {
    match current() {
        Some(s) => test_settings(s).await,
        None => Json(json!({"ok": false, "error": "Прокси не настроен."})),
    }
}

// ---- список прокси

async fn get_pool() -> Json<serde_json::Value> {
    Json(pool_view())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddReq {
    /// списком: по прокси в строке
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    kind: Option<Kind>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    max_clients: Option<u32>,
}

async fn add_proxies(Json(r): Json<AddReq>) -> Response {
    let kind = r.kind.unwrap_or(Kind::Socks5);
    let (found, mut errors) = if let Some(t) = r.text.as_deref().filter(|t| !t.trim().is_empty()) {
        parse_lines(t, kind)
    } else if let (Some(host), Some(port)) = (r.host.clone(), r.port) {
        let auth = match r.auth.as_deref().unwrap_or("none") {
            "password" => Auth::Password { user: r.user.clone().unwrap_or_default(), password: r.password.clone().unwrap_or_default() },
            _ => Auth::None,
        };
        let s = Settings { enabled: true, kind, host: host.trim().to_string(), port, auth };
        match s.check() {
            Ok(()) => (vec![s], Vec::new()),
            Err(e) => (Vec::new(), vec![e]),
        }
    } else {
        return bad_request("Укажите адрес и порт или вставьте список.");
    };
    if let Some(m) = r.max_clients {
        if m == 0 || m > 1000 {
            return bad_request("Число клиентов на прокси: от 1 до 1000.");
        }
    }
    let mut g = pool();
    let mut added = 0usize;
    for s in found {
        if g.file.proxies.len() >= MAX_PROXIES {
            errors.push(format!("больше {MAX_PROXIES} прокси не помещается"));
            break;
        }
        if g.file.proxies.iter().any(|e| e.host == s.host && e.port == s.port && e.auth == s.auth) {
            errors.push(format!("{}:{} уже в списке", s.host, s.port));
            continue;
        }
        g.file.proxies.push(Entry::new(s, r.max_clients));
        added += 1;
    }
    if added > 0 {
        g.file.enabled = true;
        if let Err(e) = persist(&g) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": e}))).into_response();
        }
    }
    Json(json!({"ok": added > 0 || errors.is_empty(), "added": added, "errors": errors})).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchReq {
    #[serde(default)]
    enabled: Option<bool>,
    /// 0 — вернуть общий предел
    #[serde(default)]
    max_clients: Option<u32>,
}

async fn patch_proxy(Path(id): Path<String>, Json(r): Json<PatchReq>) -> Response {
    if let Some(m) = r.max_clients {
        if m > 1000 {
            return bad_request("Число клиентов на прокси: от 1 до 1000 (0 — общий предел).");
        }
    }
    let mut g = pool();
    let Some(e) = g.file.proxies.iter_mut().find(|e| e.id == id) else { return (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "нет такого прокси"}))).into_response() };
    if let Some(on) = r.enabled {
        e.enabled = on;
    }
    if let Some(m) = r.max_clients {
        e.max_clients = (m > 0).then_some(m);
    }
    match persist(&g) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": e}))).into_response(),
    }
}

async fn delete_proxy(Path(id): Path<String>) -> Response {
    let mut g = pool();
    let before = g.file.proxies.len();
    g.file.proxies.retain(|e| e.id != id);
    if g.file.proxies.len() == before {
        return (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "нет такого прокси"}))).into_response();
    }
    g.health.remove(&id);
    g.assign.retain(|_, a| a.proxy != id);
    match persist(&g) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": e}))).into_response(),
    }
}

async fn test_one(Path(id): Path<String>) -> Response {
    let s = pool().file.proxies.iter().find(|e| e.id == id).map(Entry::settings);
    match s {
        Some(s) => test_settings(s).await.into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "нет такого прокси"}))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolSettingsReq {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    max_clients: Option<u32>,
    #[serde(default)]
    strict: Option<bool>,
}

async fn pool_settings(Json(r): Json<PoolSettingsReq>) -> Response {
    if let Some(m) = r.max_clients {
        if m == 0 || m > 1000 {
            return bad_request("Число клиентов на прокси: от 1 до 1000.");
        }
    }
    let mut g = pool();
    if let Some(v) = r.enabled {
        g.file.enabled = v;
    }
    if let Some(v) = r.max_clients {
        g.file.max_clients = v;
    }
    if let Some(v) = r.strict {
        g.file.strict = v;
    }
    match persist(&g) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": e}))).into_response(),
    }
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        // прежний вид: один основной прокси
        .route("/api/upstream-proxy", get(get_proxy).post(post_proxy))
        .route("/api/upstream-proxy/test", post(test_proxy))
        // список прокси
        .route("/api/upstream-proxies", get(get_pool).post(add_proxies))
        .route("/api/upstream-proxies/settings", post(pool_settings))
        .route("/api/upstream-proxies/:id", post(patch_proxy).delete(delete_proxy))
        .route("/api/upstream-proxies/:id/test", post(test_one))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn settings(kind: Kind, port: u16, auth: Auth) -> Settings {
        Settings { enabled: true, kind, host: "127.0.0.1".into(), port, auth }
    }

    /// Поддельный SOCKS5-прокси: записывает, что у него просили, и отвечает как велено.
    async fn fake_socks(want_auth: Option<(&'static str, &'static str)>, reply: u8) -> (u16, tokio::sync::oneshot::Receiver<(String, u16)>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut g = [0u8; 2];
            s.read_exact(&mut g).await.unwrap();
            let mut methods = vec![0u8; g[1] as usize];
            s.read_exact(&mut methods).await.unwrap();
            if let Some((u, p)) = want_auth {
                assert!(methods.contains(&2));
                s.write_all(&[5, 2]).await.unwrap();
                let mut h = [0u8; 2];
                s.read_exact(&mut h).await.unwrap();
                let mut user = vec![0u8; h[1] as usize];
                s.read_exact(&mut user).await.unwrap();
                let mut pl = [0u8; 1];
                s.read_exact(&mut pl).await.unwrap();
                let mut pass = vec![0u8; pl[0] as usize];
                s.read_exact(&mut pass).await.unwrap();
                let ok = user == u.as_bytes() && pass == p.as_bytes();
                s.write_all(&[1, if ok { 0 } else { 1 }]).await.unwrap();
                if !ok {
                    return;
                }
            } else {
                s.write_all(&[5, 0]).await.unwrap();
            }
            let mut h = [0u8; 4];
            s.read_exact(&mut h).await.unwrap();
            let host = match h[3] {
                3 => {
                    let mut l = [0u8; 1];
                    s.read_exact(&mut l).await.unwrap();
                    let mut d = vec![0u8; l[0] as usize];
                    s.read_exact(&mut d).await.unwrap();
                    String::from_utf8(d).unwrap()
                }
                1 => {
                    let mut a = [0u8; 4];
                    s.read_exact(&mut a).await.unwrap();
                    std::net::Ipv4Addr::from(a).to_string()
                }
                _ => panic!("unexpected atyp"),
            };
            let mut p = [0u8; 2];
            s.read_exact(&mut p).await.unwrap();
            let _ = tx.send((host, u16::from_be_bytes(p)));
            s.write_all(&[5, reply, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
            if reply == 0 {
                s.write_all(b"hello from target").await.unwrap();
            }
        });
        (port, rx)
    }

    #[tokio::test]
    async fn socks5_passes_the_name_to_the_proxy_and_the_data_flows() {
        let (port, seen) = fake_socks(None, 0).await;
        let mut st = connect(&settings(Kind::Socks5, port, Auth::None), "example.org", 443, Duration::from_secs(5)).await.unwrap();
        assert_eq!(seen.await.unwrap(), ("example.org".to_string(), 443), "the NAME reached the proxy: this computer did not resolve it");
        let mut b = vec![0u8; 17];
        st.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"hello from target");
    }

    #[tokio::test]
    async fn socks5_with_login_and_password_and_with_a_wrong_one() {
        let (port, seen) = fake_socks(Some(("anna", "s3cret")), 0).await;
        let a = Auth::Password { user: "anna".into(), password: "s3cret".into() };
        connect(&settings(Kind::Socks5, port, a), "93.184.216.34", 80, Duration::from_secs(5)).await.unwrap();
        assert_eq!(seen.await.unwrap(), ("93.184.216.34".to_string(), 80), "an IP is sent as an address, not as a name");
        let (port, _seen) = fake_socks(Some(("anna", "s3cret")), 0).await;
        let wrong = Auth::Password { user: "anna".into(), password: "nope".into() };
        let e = connect(&settings(Kind::Socks5, port, wrong), "example.org", 80, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("логин и пароль"), "{e}");
    }

    #[tokio::test]
    async fn socks5_refusals_are_told_apart_and_nothing_goes_around_the_proxy() {
        let (port, _s) = fake_socks(None, 2).await;
        let e = connect(&settings(Kind::Socks5, port, Auth::None), "example.org", 80, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("правила прокси"), "{e}");
        // nobody is listening at the proxy's address: an error, not a direct connection to the target
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = dead.local_addr().unwrap().port();
        drop(dead);
        let e = connect(&settings(Kind::Socks5, p, Auth::None), "example.org", 80, Duration::from_secs(3)).await.unwrap_err();
        assert!(e.to_string().contains("прокси недоступен"), "{e}");
    }

    async fn fake_http(status: &'static str) -> (u16, tokio::sync::oneshot::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                buf.push(b[0]);
            }
            let _ = tx.send(String::from_utf8(buf).unwrap());
            s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
            if status.starts_with("200") {
                s.write_all(b"tunnel open").await.unwrap();
            }
        });
        (port, rx)
    }

    #[tokio::test]
    async fn http_connect_sends_the_target_and_the_credentials_and_reads_only_the_headers() {
        let (port, req) = fake_http("200 Connection established").await;
        let a = Auth::Password { user: "anna".into(), password: "pw".into() };
        let mut st = connect(&settings(Kind::HttpConnect, port, a), "example.org", 443, Duration::from_secs(5)).await.unwrap();
        let text = req.await.unwrap();
        assert!(text.starts_with("CONNECT example.org:443 HTTP/1.1\r\n"), "{text}");
        assert!(text.contains("Proxy-Authorization: Basic YW5uYTpwdw==\r\n"), "{text}");
        let mut b = vec![0u8; 11];
        st.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"tunnel open", "the bytes after the headers belong to the target and were not eaten");
    }

    #[tokio::test]
    async fn http_connect_refusals() {
        let (port, _r) = fake_http("407 Proxy Authentication Required").await;
        let e = connect(&settings(Kind::HttpConnect, port, Auth::None), "example.org", 443, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("логин и пароль"), "{e}");
        let (port, _r) = fake_http("403 Forbidden").await;
        let e = connect(&settings(Kind::HttpConnect, port, Auth::None), "example.org", 443, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("403"), "{e}");
    }

    #[tokio::test]
    async fn a_target_name_cannot_smuggle_headers_and_a_silent_proxy_times_out() {
        let e = connect(&settings(Kind::HttpConnect, 9, Auth::None), "evil.org\r\nX: y", 80, Duration::from_secs(2)).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        // a proxy that accepts and says nothing
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::task::spawn(async move {
            let (_s, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let e = connect(&settings(Kind::Socks5, port, Auth::None), "example.org", 80, Duration::from_millis(400)).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn what_the_owner_types_is_checked() {
        let ok = settings(Kind::Socks5, 1080, Auth::None);
        assert!(ok.check().is_ok());
        for bad in [
            Settings { host: "".into(), ..ok.clone() },
            Settings { host: "bad host".into(), ..ok.clone() },
            Settings { host: "a\r\nb".into(), ..ok.clone() },
            Settings { port: 0, ..ok.clone() },
            Settings { auth: Auth::Password { user: "a:b".into(), password: "x".into() }, ..ok.clone() },
            Settings { auth: Auth::Password { user: "".into(), password: "x".into() }, ..ok.clone() },
            Settings { auth: Auth::Password { user: "u".into(), password: "p".repeat(300) }, ..ok.clone() },
            Settings { auth: Auth::Password { user: "u".into(), password: "a\nb".into() }, ..ok.clone() },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        assert!(Settings { host: "::1".into(), ..ok.clone() }.check().is_ok());
        assert!(Settings { host: "proxy.example.com".into(), ..ok }.check().is_ok());
    }

    #[test]
    fn the_form_keeps_the_old_password_only_for_the_same_login_and_never_shows_it() {
        let old = Settings { enabled: true, kind: Kind::Socks5, host: "p.example.com".into(), port: 1080, auth: Auth::Password { user: "anna".into(), password: "secret".into() } };
        let f = |user: &str, password: &str| Form { enabled: true, kind: Kind::Socks5, host: " p.example.com ".into(), port: 1080, auth: "password".into(), user: user.into(), password: password.into() };
        let kept = from_form(f("anna", ""), Some(old.clone())).unwrap();
        assert_eq!(kept.auth, Auth::Password { user: "anna".into(), password: "secret".into() }, "an empty field keeps the saved password");
        assert_eq!(kept.host, "p.example.com", "spaces around the address are removed");
        // another login does not inherit the old password
        let other = from_form(f("bob", ""), Some(old.clone())).unwrap();
        assert_eq!(other.auth, Auth::Password { user: "bob".into(), password: String::new() });
        assert_eq!(from_form(f("anna", "new"), Some(old.clone())).unwrap().auth, Auth::Password { user: "anna".into(), password: "new".into() });
        assert!(from_form(Form { auth: "magic".into(), ..f("a", "b") }, None).is_err());
        assert!(from_form(Form { port: 0, ..f("a", "b") }, None).is_err());
    }

    // ------------------------------------------------------------ пул прокси

    fn entry(id: &str, port: u16) -> Entry {
        Entry { id: id.into(), enabled: true, kind: Kind::Socks5, host: "127.0.0.1".into(), port, auth: Auth::None, max_clients: None }
    }

    fn pool_of(ids: &[&str], cap: u32, strict: bool) -> Pool {
        Pool {
            file: PoolFile { enabled: true, max_clients: cap, strict, proxies: ids.iter().enumerate().map(|(i, id)| entry(id, 1000 + i as u16)).collect() },
            health: HashMap::new(),
            assign: HashMap::new(),
        }
    }

    fn id_of(p: Pick) -> String {
        match p {
            Pick::Proxy(id) => id,
            other => panic!("expected a proxy, got {other:?}"),
        }
    }

    #[test]
    fn a_client_keeps_its_proxy_and_new_clients_go_to_the_least_loaded() {
        let now = Instant::now();
        let mut p = pool_of(&["a", "b", "c"], 5, false);
        let first = id_of(pick(&mut p, "phone1", now, &[]));
        for _ in 0..20 {
            assert_eq!(id_of(pick(&mut p, "phone1", now, &[])), first, "the same client always exits from the same proxy");
        }
        let second = id_of(pick(&mut p, "phone2", now, &[]));
        let third = id_of(pick(&mut p, "phone3", now, &[]));
        let mut all = vec![first.clone(), second, third];
        all.sort();
        assert_eq!(all, ["a", "b", "c"], "three clients spread over three proxies, not piled on one");
        assert_eq!(p.clients_of(&first), 1);
    }

    #[test]
    fn up_to_five_clients_per_proxy_then_the_soft_limit_spills_over_and_the_strict_one_refuses() {
        let now = Instant::now();
        let mut soft = pool_of(&["a", "b"], 5, false);
        for i in 0..10 {
            id_of(pick(&mut soft, &format!("c{i}"), now, &[]));
        }
        assert_eq!((soft.clients_of("a"), soft.clients_of("b")), (5, 5), "ten clients fill two proxies by five");
        let eleventh = id_of(pick(&mut soft, "c10", now, &[]));
        assert!(eleventh == "a" || eleventh == "b", "the soft limit still serves an eleventh client");
        assert_eq!(soft.clients_of("a") + soft.clients_of("b"), 11);

        let mut strict = pool_of(&["a", "b"], 5, true);
        for i in 0..10 {
            id_of(pick(&mut strict, &format!("c{i}"), now, &[]));
        }
        assert_eq!(pick(&mut strict, "c10", now, &[]), Pick::Full, "the strict limit refuses a new client when every proxy is full");
        assert!(matches!(pick(&mut strict, "c3", now, &[]), Pick::Proxy(_)), "a client that already has a place keeps it");
    }

    #[test]
    fn a_proxy_can_have_its_own_limit() {
        let now = Instant::now();
        let mut p = pool_of(&["a", "b"], 5, true);
        p.file.proxies[0].max_clients = Some(1);
        let mut got = Vec::new();
        for i in 0..6 {
            got.push(id_of(pick(&mut p, &format!("c{i}"), now, &[])));
        }
        assert_eq!(got.iter().filter(|x| *x == "a").count(), 1, "the proxy limited to one client takes one");
        assert_eq!(got.iter().filter(|x| *x == "b").count(), 5);
    }

    #[test]
    fn a_dead_proxy_is_skipped_its_clients_move_and_it_comes_back_after_a_pause() {
        let now = Instant::now();
        let cell = Mutex::new(pool_of(&["a", "b"], 5, false));
        let ca = id_of(pick(&mut cell.lock().unwrap(), "phone1", now, &[]));
        note_fail(&cell, &ca, "прокси недоступен: refused", now);
        let moved = id_of(pick(&mut cell.lock().unwrap(), "phone1", now + Duration::from_secs(1), &[]));
        assert_ne!(moved, ca, "the client moved to the live proxy");
        assert_eq!(id_of(pick(&mut cell.lock().unwrap(), "phone1", now + Duration::from_secs(2), &[])), moved, "and stays there");
        // after the pause the first one is available again for NEW clients; the old one is not dragged back
        let later = now + Duration::from_secs(60);
        assert_eq!(id_of(pick(&mut cell.lock().unwrap(), "phone1", later, &[])), moved);
        let g = cell.lock().unwrap();
        assert!(g.available(&g.file.proxies.iter().find(|e| e.id == ca).unwrap().clone(), later));
    }

    #[test]
    fn the_pause_grows_with_the_failures_and_a_success_clears_it() {
        let now = Instant::now();
        let cell = Mutex::new(pool_of(&["a"], 5, false));
        for _ in 0..3 {
            note_fail(&cell, "a", "x", now);
        }
        let until = cell.lock().unwrap().health["a"].cooldown_until.unwrap();
        assert_eq!(until - now, Duration::from_secs(40), "10 s, 20 s, 40 s");
        for _ in 0..10 {
            note_fail(&cell, "a", "x", now);
        }
        assert_eq!(cell.lock().unwrap().health["a"].cooldown_until.unwrap() - now, COOLDOWN_MAX, "never longer than five minutes");
        note_ok(&cell, "a", now);
        let g = cell.lock().unwrap();
        assert!(g.health["a"].cooldown_until.is_none() && g.health["a"].fails == 0);
    }

    #[test]
    fn when_every_proxy_is_in_a_pause_the_one_that_wakes_first_is_tried_never_a_direct_exit() {
        let now = Instant::now();
        let cell = Mutex::new(pool_of(&["a", "b"], 5, false));
        note_fail(&cell, "a", "x", now);
        note_fail(&cell, "a", "x", now); // 20 s
        note_fail(&cell, "b", "x", now); // 10 s
        assert_eq!(id_of(pick(&mut cell.lock().unwrap(), "phone", now, &[])), "b");
        let mut g = cell.lock().unwrap();
        assert_eq!(g.assign.len(), 0, "a probe does not pin the client");
        g.file.enabled = false;
        assert_eq!(pick(&mut g, "phone", now, &[]), Pick::None, "the master switch off: no proxy is picked (the exit is then direct by the caller's rule)");
    }

    #[test]
    fn idle_clients_free_their_places() {
        let now = Instant::now();
        let mut p = pool_of(&["a"], 1, true);
        id_of(pick(&mut p, "old", now, &[]));
        assert_eq!(pick(&mut p, "new", now, &[]), Pick::Full);
        assert!(matches!(pick(&mut p, "new", now + IDLE_ASSIGN + Duration::from_secs(1), &[]), Pick::Proxy(_)), "after half an hour of silence the place is free");
    }

    #[test]
    fn clients_without_a_name_are_spread_without_being_pinned() {
        let now = Instant::now();
        let cell = Mutex::new(pool_of(&["a", "b"], 5, false));
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6 {
            let id = id_of(pick(&mut cell.lock().unwrap(), SHARED_CLIENT, now, &[]));
            note_ok(&cell, &id, now);
            seen.insert(id);
        }
        assert_eq!(seen.len(), 2, "shared traffic uses both proxies in turn");
        assert!(cell.lock().unwrap().assign.is_empty());
    }

    #[test]
    fn only_the_proxys_own_faults_are_held_against_it() {
        use std::io::{Error, ErrorKind::*};
        assert!(proxy_fault(&Error::new(Other, "прокси недоступен: Connection refused")));
        assert!(proxy_fault(&Error::new(Other, "прокси не принял логин и пароль")));
        assert!(proxy_fault(&Error::new(TimedOut, "прокси не ответил вовремя")));
        assert!(proxy_fault(&Error::new(UnexpectedEof, "early eof")));
        assert!(!proxy_fault(&Error::new(Other, "прокси: цель недоступна")));
        assert!(!proxy_fault(&Error::new(Other, "прокси: цель отказала в соединении")));
        assert!(!proxy_fault(&Error::new(Other, "прокси ответил отказом (403)")));
        assert!(!proxy_fault(&Error::new(PermissionDenied, "адрес этого компьютера")));
        assert!(!proxy_fault(&Error::new(InvalidInput, "неверная цель")));
    }

    #[tokio::test]
    async fn a_connection_fails_over_to_a_live_proxy_and_the_dead_one_is_marked() {
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);
        let (live_port, rx) = fake_socks(None, 0).await;
        let mut p = pool_of(&["dead", "live"], 5, false);
        p.file.proxies[0].port = dead_port;
        p.file.proxies[1].port = live_port;
        // the client is pinned to the dead one first
        p.assign.insert("phone".into(), Assign { proxy: "dead".into(), seen: Instant::now() });
        let cell = Mutex::new(p);
        let mut s = connect_in(&cell, "phone", "youtube.com", 443, Duration::from_secs(5)).await.expect("the live proxy serves the connection");
        let mut b = [0u8; 17];
        s.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"hello from target");
        assert_eq!(rx.await.unwrap(), ("youtube.com".to_string(), 443), "the name went to the proxy");
        let g = cell.lock().unwrap();
        assert!(g.health["dead"].cooldown_until.is_some() && g.health["dead"].last_error.as_deref().unwrap_or("").contains("прокси недоступен"));
        assert_eq!(g.assign["phone"].proxy, "live", "the client now sits on the live proxy");
        assert!(g.health["live"].last_ok.is_some());
    }

    #[tokio::test]
    async fn a_refusal_by_the_target_does_not_punish_the_proxy() {
        let (port, _rx) = fake_socks(None, 4).await; // «цель недоступна»
        let mut p = pool_of(&["a", "b"], 5, false);
        p.file.proxies[0].port = port;
        p.file.proxies[1].port = 1; // never used: the first answered
        let cell = Mutex::new(p);
        let e = connect_in(&cell, "phone", "example.org", 443, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().starts_with("прокси: "), "{e}");
        let g = cell.lock().unwrap();
        assert!(g.health["a"].cooldown_until.is_none(), "the proxy answered, so it is not in a pause");
        assert!(g.health.get("b").is_none(), "no failover for a target-side refusal");
    }

    #[tokio::test]
    async fn with_every_proxy_dead_nothing_goes_around_them() {
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = dead.local_addr().unwrap().port();
        drop(dead);
        let mut p = pool_of(&["a", "b"], 5, false);
        p.file.proxies[0].port = port;
        p.file.proxies[1].port = port;
        let cell = Mutex::new(p);
        let e = connect_in(&cell, "phone", "8.8.8.8", 53, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("прокси"), "{e}");
        let g = cell.lock().unwrap();
        assert!(g.health["a"].cooldown_until.is_some() && g.health["b"].cooldown_until.is_some());
    }

    #[test]
    fn a_pasted_list_in_the_usual_forms_is_parsed_and_bad_lines_are_reported() {
        let text = "# мои прокси\n45.147.182.91:8000:user1:pa:ss\nsocks5://anna:secret@10.1.2.3:1080\nhttp://bob:pw@proxy.example.com:3128\nlogin:pass@203.0.113.7:9050\n198.51.100.4:1080\n\nbroken line\nhost:99999\nftp://x@y.z:1\n";
        let (ok, errors) = parse_lines(text, Kind::Socks5);
        assert_eq!(ok.len(), 5, "{ok:?}");
        assert_eq!((ok[0].host.as_str(), ok[0].port), ("45.147.182.91", 8000));
        assert_eq!(ok[0].auth, Auth::Password { user: "user1".into(), password: "pa:ss".into() }, "a colon inside the password is kept");
        assert_eq!(ok[1].auth, Auth::Password { user: "anna".into(), password: "secret".into() });
        assert_eq!(ok[1].kind, Kind::Socks5);
        assert_eq!(ok[2].kind, Kind::HttpConnect);
        assert_eq!(ok[3].auth, Auth::Password { user: "login".into(), password: "pass".into() });
        assert_eq!(ok[4].auth, Auth::None);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].starts_with("строка 8") && errors[1].starts_with("строка 9") && errors[2].starts_with("строка 10"), "{errors:?}");
    }

    #[test]
    fn the_old_single_proxy_file_still_loads_as_a_list_of_one() {
        let old = br#"{"enabled": true, "kind": "socks5", "host": "45.147.182.91", "port": 8000, "auth": {"type": "password", "user": "u", "password": "p"}}"#;
        let f = parse_file(old).expect("old format");
        assert_eq!(f.proxies.len(), 1);
        assert!(f.enabled && f.proxies[0].enabled && f.max_clients == DEFAULT_CAP);
        assert_eq!(f.proxies[0].host, "45.147.182.91");
        let new = serde_json::to_vec(&f).unwrap();
        let back = parse_file(&new).expect("new format");
        assert_eq!(back.proxies, f.proxies);
        // a switched-off old proxy stays switched off
        let off = br#"{"enabled": false, "kind": "socks5", "host": "h.example", "port": 1, "auth": {"type": "none"}}"#;
        assert!(!parse_file(off).unwrap().proxies[0].enabled);
        assert!(parse_file(b"not json").is_none());
        assert!(parse_file(br#"{"proxies": [{"id":"x","enabled":true,"kind":"socks5","host":"bad host","port":1,"auth":{"type":"none"}}]}"#).unwrap().proxies.is_empty(), "an entry that fails the check is dropped");
    }
}
