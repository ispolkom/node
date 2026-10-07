//! Карточки узлов и списки по странам — основа сети, которая живёт сама (решение владельца 2026-09-29: «при старте нода определяет
//! железо, юрисдикцию, состояние канала — это и должно быть основанием списков; пользователь ничего не выбирает вручную»).
//!
//! * **Карточка** — подписанные ключом узла открытые сведения: страна (и откуда она известна), есть ли публичный адрес, мощность
//!   (ядра, память), задержка канала, адреса, чем узел помогает сейчас (выход в интернет — если владелец разрешил; ретрансляция)
//!   и может ли быть выходом (публичный адрес и достаточная мощность). Город, провайдер и прочее — не публикуются.
//!   Подпись покрывает ВСЁ содержимое; карточка живёт 2 часа и переподписывается каждые 30 минут.
//! * **Каталог** — проверенные карточки, по одной (самой свежей) на узел; ключ узла закрепляется за его номером (чужой ключ с тем же
//!   номером — отказ); просроченные выпадают; размер ограничен. Списки по странам строятся из каталога.
//! * **Обмен с соседями** (как обмен списками пиров в торрентах): узел отдаёт свою карточку и часть известных ему чужих каждому узлу,
//!   с которым связан; так списки расходятся по сети без центра. Подделки, просроченные и чужие ключи отбрасываются на входе.
//! * Телефоны — только клиенты: карточек не публикуют (роль Mobile).
//! * Страна, найденная по адресу или заявленная, — ещё не проверенная: `country_source` говорит, откуда она; проверка извне — следующий шаг.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Тип пакета основной связи: обмен карточками.
pub const PKT_OFFERS: u8 = 0xD8;
pub const OFFER_TTL_SECS: u64 = 2 * 3600;
pub const RESIGN_EVERY_SECS: u64 = 30 * 60;
/// Сколько карточек в одном пакете обмена (своя + чужие).
pub const GOSSIP_BATCH: usize = 24;
pub const MAX_DIRECTORY: usize = 5000;
const MAX_PACKET_BYTES: usize = 60 * 1024;
/// Допуск часов: карточка «из будущего» больше чем на 5 минут — отказ.
const CLOCK_SKEW_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeOffer {
    pub v: u8,
    /// номер узла (64 hex)
    pub node_id: String,
    /// открытый ключ подписи (64 hex)
    pub key: String,
    /// страна (ISO-3166 alpha-2, заглавные) или пусто, если неизвестна
    pub country: Option<String>,
    /// откуда страна: `ip_lookup` (по внешнему адресу) / `claimed` (владелец указал) / `unknown`
    pub country_source: String,
    pub public_ip: bool,
    /// `low` / `medium` / `high`
    pub power: String,
    pub cpu_cores: u32,
    pub ram_gb: u32,
    pub latency_ms: Option<u32>,
    /// где слушает основная связь (`адрес:порт`), до 4
    pub addr: Vec<String>,
    /// выпускает в интернет сейчас (владелец разрешил)
    pub exit: bool,
    /// может быть выходом по железу и каналу (публичный адрес, мощность не «низкая»)
    pub can_exit: bool,
    /// готов быть ретранслятором: держит соединения клиентов за NAT и передаёт к ним цепочки
    #[serde(default)]
    pub relay: bool,
    pub issued: u64,
    pub expires: u64,
    /// подпись Ed25519 (128 hex) всего остального
    pub sig: String,
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| c.is_ascii_hexdigit())
}

fn hex_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for i in 0..N {
        out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl NodeOffer {
    /// Что подписывается: вся карточка без подписи (порядок полей постоянный).
    fn signing_bytes(&self) -> Vec<u8> {
        let mut c = self.clone();
        c.sig = String::new();
        let mut b = b"yandi-node-offer-v1\0".to_vec();
        b.extend_from_slice(&serde_json::to_vec(&c).unwrap_or_default());
        b
    }

    pub fn sign(mut self, key: &SigningKey) -> NodeOffer {
        self.key = to_hex(&key.verifying_key().to_bytes());
        self.sig = String::new();
        self.sig = to_hex(&key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    /// Проверка всего, что можно проверить без сети: форма, сроки, подпись.
    pub fn check(&self, now: u64) -> Result<(), &'static str> {
        if self.v != 1 {
            return Err("version");
        }
        if !is_hex(&self.node_id, 64) || !is_hex(&self.key, 64) || !is_hex(&self.sig, 128) {
            return Err("ids");
        }
        if let Some(c) = &self.country {
            if c.len() != 2 || !c.bytes().all(|b| b.is_ascii_uppercase()) {
                return Err("country");
            }
        }
        if !matches!(self.country_source.as_str(), "ip_lookup" | "claimed" | "unknown") || !matches!(self.power.as_str(), "low" | "medium" | "high") {
            return Err("fields");
        }
        if self.addr.len() > 4 || self.addr.iter().any(|a| a.len() > 262 || a.parse::<std::net::SocketAddr>().is_err()) {
            return Err("addr");
        }
        if self.expires <= now || self.issued > now + CLOCK_SKEW_SECS || self.expires < self.issued || self.expires - self.issued > OFFER_TTL_SECS {
            return Err("time");
        }
        let key = hex_bytes::<32>(&self.key).and_then(|k| VerifyingKey::from_bytes(&k).ok()).ok_or("key")?;
        if !hex_bytes::<32>(&self.node_id).map_or(false, |id| crate::util::types::id_acceptable(&id, key.as_bytes())) {
            return Err("id not bound to key");
        }
        let sig = hex_bytes::<64>(&self.sig).map(|s| Signature::from_bytes(&s)).ok_or("sig")?;
        key.verify(&self.signing_bytes(), &sig).map_err(|_| "signature")
    }
}

/// Проверенные карточки: по одной (самой свежей) на узел, ключ закреплён за номером.
#[derive(Default)]
pub struct Directory {
    by_node: HashMap<String, NodeOffer>,
    pinned: HashMap<String, String>,
    /// проверенные обратным подключением: узел → (адреса, до какого времени проверка действует)
    verified: HashMap<String, (Vec<String>, u64)>,
    /// неудачные проверки: узел → когда можно пробовать снова
    failed: HashMap<String, u64>,
}

#[derive(Debug, PartialEq)]
pub enum Accept {
    New,
    Newer,
    Stale,
    Refused(&'static str),
}

impl Directory {
    pub fn accept(&mut self, o: NodeOffer, now: u64) -> Accept {
        if let Err(e) = o.check(now) {
            return Accept::Refused(e);
        }
        if let Some(k) = self.pinned.get(&o.node_id) {
            if k != &o.key {
                return Accept::Refused("key changed");
            }
        }
        match self.by_node.get(&o.node_id) {
            Some(old) if old.issued >= o.issued => return Accept::Stale,
            Some(_) => {
                self.by_node.insert(o.node_id.clone(), o);
                return Accept::Newer;
            }
            None => {}
        }
        if self.by_node.len() >= MAX_DIRECTORY {
            self.forget_expired(now);
            if self.by_node.len() >= MAX_DIRECTORY {
                return Accept::Refused("full");
            }
        }
        if self.pinned.len() >= MAX_DIRECTORY * 4 {
            // pins of nodes no longer in the directory are dropped when the table is far over its size
            let alive: std::collections::HashSet<&String> = self.by_node.keys().collect();
            self.pinned.retain(|k, _| alive.contains(k));
        }
        self.pinned.insert(o.node_id.clone(), o.key.clone());
        self.by_node.insert(o.node_id.clone(), o);
        Accept::New
    }

    pub fn forget_expired(&mut self, now: u64) {
        self.by_node.retain(|_, o| o.expires > now);
        let alive: std::collections::HashSet<&String> = self.by_node.keys().collect();
        self.verified.retain(|k, _| alive.contains(k));
        self.failed.retain(|k, t| alive.contains(k) && *t > now);
    }

    /// Адреса узла проверены обратным подключением и проверка ещё действует.
    pub fn is_verified(&self, node: &str, now: u64) -> bool {
        match (self.verified.get(node), self.by_node.get(node)) {
            (Some((addrs, until)), Some(o)) => *until > now && *addrs == o.addr,
            _ => false,
        }
    }

    /// Надо ли проверять карточку сейчас (не проверена, проверка истекла или адрес сменился, и не ждёт после неудачи).
    pub fn needs_probe(&self, node: &str, now: u64) -> bool {
        self.by_node.contains_key(node) && !self.is_verified(node, now) && self.failed.get(node).map(|t| *t <= now).unwrap_or(true)
    }

    pub fn mark_verified(&mut self, node: &str, addrs: Vec<String>, until: u64) {
        self.failed.remove(node);
        self.verified.insert(node.to_string(), (addrs, until));
    }

    pub fn mark_failed(&mut self, node: &str, retry_at: u64) {
        self.verified.remove(node);
        self.failed.insert(node.to_string(), retry_at);
    }

    /// Только проверенные: их выбирают в выходы и цепочки и раздают соседям.
    pub fn fresh_verified(&self, now: u64) -> Vec<NodeOffer> {
        self.fresh(now).into_iter().filter(|o| self.is_verified(&o.node_id, now)).collect()
    }

    pub fn fresh(&self, now: u64) -> Vec<NodeOffer> {
        let mut v: Vec<NodeOffer> = self.by_node.values().filter(|o| o.expires > now).cloned().collect();
        v.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        v
    }

    /// Список страны (`None` — узлы без известной страны).
    pub fn by_country(&self, country: Option<&str>, now: u64) -> Vec<NodeOffer> {
        self.fresh(now).into_iter().filter(|o| o.country.as_deref() == country).collect()
    }

    /// Сколько узлов в каждой стране.
    pub fn countries(&self, now: u64) -> std::collections::BTreeMap<String, usize> {
        let mut m = std::collections::BTreeMap::new();
        for o in self.fresh(now) {
            *m.entry(o.country.clone().unwrap_or_else(|| "??".into())).or_insert(0) += 1;
        }
        m
    }

    /// Что отдать соседу: своя карточка первой, потом случайная часть чужих (не его собственная).
    pub fn gossip_batch(&self, own: Option<&NodeOffer>, to: &str, now: u64) -> Vec<NodeOffer> {
        use rand::seq::SliceRandom;
        let mut out: Vec<NodeOffer> = own.cloned().into_iter().collect();
        let mut others: Vec<NodeOffer> = self.fresh_verified(now).into_iter().filter(|o| o.node_id != to && own.map(|m| m.node_id != o.node_id).unwrap_or(true)).collect();
        others.shuffle(&mut rand::thread_rng());
        out.extend(others.into_iter().take(GOSSIP_BATCH.saturating_sub(out.len())));
        out
    }
}

/// Пакет обмена: `[PKT_OFFERS][JSON-массив карточек]`, не больше `MAX_PACKET_BYTES`.
pub fn encode_packet(offers: &[NodeOffer]) -> Vec<u8> {
    let mut list = offers.to_vec();
    loop {
        let mut p = vec![PKT_OFFERS];
        p.extend_from_slice(&serde_json::to_vec(&list).unwrap_or_default());
        if p.len() <= MAX_PACKET_BYTES || list.is_empty() {
            return p;
        }
        list.pop();
    }
}

/// Разобрать пакет соседа и принять годные карточки; сколько принято новыми или обновлёнными.
pub fn receive_packet(dir: &mut Directory, plaintext: &[u8], now: u64) -> usize {
    receive_packet_collect(dir, plaintext, now).len()
}

/// То же, но возвращает принятые карточки (их надо проверить обратным подключением).
pub fn receive_packet_collect(dir: &mut Directory, plaintext: &[u8], now: u64) -> Vec<NodeOffer> {
    if plaintext.first() != Some(&PKT_OFFERS) || plaintext.len() > MAX_PACKET_BYTES {
        return vec![];
    }
    // основная связь дополняет данные нулями и не снимает их — JSON нулём закончиться не может
    let body = &plaintext[1..];
    let end = body.iter().rposition(|&b| b != 0).map(|i| i + 1).unwrap_or(0);
    let Ok(list) = serde_json::from_slice::<Vec<NodeOffer>>(&body[..end]) else { return vec![] };
    list.into_iter().take(GOSSIP_BATCH).filter(|o| matches!(dir.accept(o.clone(), now), Accept::New | Accept::Newer)).collect()
}

// ---------------------------------------------------------------- состояние этого узла

static PUBLISHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Публиковать ли карточку узла (клиент за NAT её не публикует и ничего не раздаёт).
pub fn set_published(on: bool) {
    PUBLISHED.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn dir_cell() -> &'static Mutex<Directory> {
    static D: OnceLock<Mutex<Directory>> = OnceLock::new();
    D.get_or_init(Default::default)
}

fn own_cell() -> &'static RwLock<Option<NodeOffer>> {
    static O: RwLock<Option<NodeOffer>> = RwLock::new(None);
    &O
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Самооценка узла при запуске (ничего не выбирается вручную).
#[derive(Debug, Clone)]
pub struct SelfAssessment {
    pub country: Option<String>,
    pub country_source: &'static str,
    pub public_ip: bool,
    pub power: &'static str,
    pub cpu_cores: u32,
    pub ram_gb: u32,
    pub latency_ms: Option<u32>,
    pub addr: Vec<String>,
}

/// Страна из ответа сервиса адресов или заявления владельца: только правильный двухбуквенный код.
pub fn normalise_country(s: &str) -> Option<String> {
    let c = s.trim().to_ascii_uppercase();
    (c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).then_some(c)
}

/// Собрать и подписать свою карточку по самооценке и текущим решениям владельца.
pub fn build_own(node_id: &[u8; 32], key: &SigningKey, a: &SelfAssessment, now: u64) -> NodeOffer {
    let can_exit = a.public_ip && a.power != "low";
    let exit = can_exit && crate::exit_policy::mode() != crate::exit_policy::ExitMode::Off;
    NodeOffer {
        v: 1,
        node_id: to_hex(node_id),
        key: String::new(),
        country: a.country.clone(),
        country_source: a.country_source.to_string(),
        public_ip: a.public_ip,
        power: a.power.to_string(),
        cpu_cores: a.cpu_cores,
        ram_gb: a.ram_gb,
        latency_ms: a.latency_ms,
        addr: a.addr.iter().filter(|x| x.parse::<std::net::SocketAddr>().is_ok()).take(4).cloned().collect(),
        exit,
        can_exit,
        relay: can_exit && crate::relay_net::relay_enabled(),
        issued: now,
        expires: now + OFFER_TTL_SECS,
        sig: String::new(),
    }
    .sign(key)
}

pub fn own() -> Option<NodeOffer> {
    own_cell().read().ok()?.clone()
}

fn set_own(o: NodeOffer) {
    if let Ok(mut g) = own_cell().write() {
        *g = Some(o.clone());
    }
    if let Ok(mut d) = dir_cell().lock() {
        let (id, addrs, until) = (o.node_id.clone(), o.addr.clone(), o.expires);
        d.accept(o, now_secs());
        // свою карточку проверять обратным подключением незачем
        d.mark_verified(&id, addrs, until);
    }
}

/// Не больше стольких пакетов обмена от одного соседа в минуту (остальные — без чтения).
pub const MAX_PACKETS_PER_MINUTE: u32 = 6;

fn rate_ok(sender: &[u8; 32], now: u64) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<[u8; 32], (u64, u32)>>> = OnceLock::new();
    let mut m = SEEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 10_000 {
        m.retain(|_, (t, _)| now.saturating_sub(*t) < 60);
    }
    let e = m.entry(*sender).or_insert((now, 0));
    if now.saturating_sub(e.0) >= 60 {
        *e = (now, 0);
    }
    e.1 += 1;
    e.1 <= MAX_PACKETS_PER_MINUTE
}

/// Принять пакет обмена от соседа (из общей раздачи пакетов основной связи).
pub fn on_packet(sender: &[u8; 32], plaintext: &[u8]) -> usize {
    let now = now_secs();
    if !rate_ok(sender, now) {
        return 0;
    }
    let accepted = dir_cell().lock().map(|mut d| receive_packet_collect(&mut d, plaintext, now)).unwrap_or_default();
    let n = accepted.len();
    if n > 0 {
        println!("[offers] принято карточек: {n}");
        for o in accepted {
            queue_probe(o);
        }
    }
    n
}

/// Проверить принятую карточку обратным подключением (если надо): не больше `MAX_PARALLEL` одновременно.
fn queue_probe(o: NodeOffer) {
    static SEM: OnceLock<std::sync::Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let now = now_secs();
    if o.addr.is_empty() || !dir_cell().lock().map(|d| d.needs_probe(&o.node_id, now)).unwrap_or(false) {
        return;
    }
    // чтобы одну карточку не проверяли дважды одновременно
    static BUSY: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    if !BUSY.get_or_init(Default::default).lock().map(|mut b| b.insert(o.node_id.clone())).unwrap_or(false) {
        return;
    }
    let sem = SEM.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(crate::reachability::MAX_PARALLEL))).clone();
    tokio::spawn(async move {
        let _permit = sem.acquire_owned().await;
        let mut ok = false;
        for a in o.addr.iter().take(2) {
            if crate::reachability::probe(&o, a).await.is_ok() {
                ok = true;
                break;
            }
        }
        let now = now_secs();
        if let Ok(mut d) = dir_cell().lock() {
            if ok {
                if let Some(c) = crate::netlayer::tcp_carrier::global() {
                    for a in &o.addr {
                        if let Ok(sa) = a.parse::<std::net::SocketAddr>() {
                            c.hint(sa);
                        }
                    }
                }
                d.mark_verified(&o.node_id, o.addr.clone(), now + crate::reachability::VERIFIED_SECS);
                println!("[offers] узел {} достижим по заявленному адресу — карточка принята в списки", &o.node_id[..8]);
            } else {
                d.mark_failed(&o.node_id, now + crate::reachability::RETRY_SECS);
                println!("[offers] узел {} не отозвался по заявленному адресу — карточка не используется", &o.node_id[..8]);
            }
        }
        if let Some(b) = BUSY.get() {
            if let Ok(mut b) = b.lock() {
                b.remove(&o.node_id);
            }
        }
    });
}

/// Карточка узла из каталога (свежая), если есть.
pub fn offer_of(node_hex: &str) -> Option<NodeOffer> {
    let now = now_secs();
    dir_cell().lock().unwrap_or_else(|e| e.into_inner()).by_node.get(node_hex).filter(|o| o.expires > now).cloned()
}

/// Проверенные карточки (по ним выбирают выходы и цепочки) и счёт по странам (тоже только проверенных).
pub fn directory_snapshot(country: Option<&str>) -> (Vec<NodeOffer>, std::collections::BTreeMap<String, usize>) {
    let (all, counts) = directory_snapshot_all(country);
    let now = now_secs();
    let d = dir_cell().lock().unwrap_or_else(|e| e.into_inner());
    let _ = counts;
    let list: Vec<NodeOffer> = all.into_iter().filter(|(_, v)| *v).map(|(o, _)| o).collect();
    let mut m = std::collections::BTreeMap::new();
    for o in d.fresh_verified(now) {
        *m.entry(o.country.clone().unwrap_or_else(|| "??".into())).or_insert(0) += 1;
    }
    (list, m)
}

/// Все карточки с отметкой «проверена обратным подключением» — для страницы сети.
pub fn directory_snapshot_all(country: Option<&str>) -> (Vec<(NodeOffer, bool)>, std::collections::BTreeMap<String, usize>) {
    let now = now_secs();
    let d = dir_cell().lock().unwrap_or_else(|e| e.into_inner());
    let list = match country {
        Some("??") => d.by_country(None, now),
        Some(c) => d.by_country(Some(c), now),
        None => d.fresh(now),
    };
    (list.into_iter().map(|o| { let v = d.is_verified(&o.node_id, now); (o, v) }).collect(), d.countries(now))
}

/// Запуск: своя карточка сразу и переподпись каждые 30 минут; обмен с соседями — через 15 с, потом раз в минуту первые 10 минут,
/// дальше раз в 5 минут. Узел с публичным адресом слушает пробы (`reachability`). Если внешний адрес не задан владельцем, раз в
/// 5 минут он проверяется: при смене карточка перевыпускается и раздаётся соседям сразу (`on_address_change` — запомнить новый
/// адрес в других местах, например в визитке).
pub fn start(
    transport: std::sync::Arc<crate::netlayer::transport::P2PTransport>,
    node_id: [u8; 32],
    key: SigningKey,
    a: SelfAssessment,
    watch_ip: bool,
    on_address_change: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
) {
    set_own(build_own(&node_id, &key, &a, now_secs()));
    if let Some(o) = own() {
        println!("[offers] карточка узла: страна {} ({}), публичный адрес {}, мощность {}, выход {}",
            o.country.as_deref().unwrap_or("неизвестна"), o.country_source, if o.public_ip { "есть" } else { "нет" }, o.power, if o.exit { "да" } else { "нет" });
    }
    // слушатель проб: на TCP-порту основной связи (порты TCP и UDP независимы)
    // порт TCP — номер порта обнаружения (по нему же слушает запасной путь по TCP, даже если внешний адрес ещё не известен)
    let port = a.addr.first().and_then(|x| x.parse::<std::net::SocketAddr>().ok()).map(|s| s.port()).or_else(|| Some(crate::core::get_config().ports.discovery));
    if let Some(port) = port {
        let key = key.clone();
        crate::supervisor::supervise("reachability_listener", crate::supervisor::Policy::restart(), move || crate::reachability::serve(port, node_id, key.clone(), None));
    }
    let a = std::sync::Arc::new(Mutex::new(a));
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    if watch_ip {
        let (a, wake) = (a.clone(), wake.clone());
        crate::supervisor::supervise("ip_watch", crate::supervisor::Policy::restart(), move || {
        let (a, wake, on_address_change) = (a.clone(), wake.clone(), on_address_change.clone());
        async move {
            let svc = crate::netlayer::external_ip::ExternalIpService::new();
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                let Ok(ip) = svc.get_external_ip().await else { continue };
                let (port, old) = {
                    let g = a.lock().unwrap_or_else(|e| e.into_inner());
                    (g.addr.first().and_then(|x| x.parse::<std::net::SocketAddr>().ok()).map(|s| s.port()), g.addr.first().cloned())
                };
                let Some(port) = port else { continue };
                let new = if ip.contains(':') { format!("[{ip}]:{port}") } else { format!("{ip}:{port}") };
                if old.as_deref() != Some(new.as_str()) {
                    println!("[offers] внешний адрес изменился: {} → {new}", old.unwrap_or_default());
                    a.lock().unwrap_or_else(|e| e.into_inner()).addr = vec![new];
                    if let Some(f) = &on_address_change {
                        f(ip);
                    }
                    wake.notify_one();
                }
            }
        }
    });
    }
    crate::supervisor::supervise("offers_gossip", crate::supervisor::Policy::restart(), move || {
        let (transport, key, a, wake) = (transport.clone(), key.clone(), a.clone(), wake.clone());
        async move {
        let started = now_secs();
        let mut last_sign = started;
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        loop {
            let now = now_secs();
            // переподпись раз в 30 минут — и сразу, если изменилось, чем узел помогает (владелец включил выход, выбрал модель…)
            // или сменился адрес
            let fresh = build_own(&node_id, &key, &a.lock().unwrap_or_else(|e| e.into_inner()).clone(), now);
            let changed = own().map(|o| o.exit != fresh.exit || o.addr != fresh.addr).unwrap_or(true);
            if changed || now - last_sign >= RESIGN_EVERY_SECS {
                set_own(fresh);
                last_sign = now;
            }
            let peers = if PUBLISHED.load(std::sync::atomic::Ordering::Relaxed) { transport.get_peers().await } else { vec![] };
            for p in peers {
                let batch = {
                    let d = dir_cell().lock().unwrap_or_else(|e| e.into_inner());
                    d.gossip_batch(own().as_ref(), &p.id.to_hex(), now)
                };
                if batch.is_empty() {
                    continue;
                }
                let _ = transport.send_encrypted(p.id, &encode_packet(&batch)).await;
            }
            if let Ok(mut d) = dir_cell().lock() {
                d.forget_expired(now);
            }
            let wait = if now - started < 600 { 60 } else { 300 };
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(wait)) => {}
                _ = wake.notified() => {}
            }
        }
    }
    });
}

/// Страница сети (под проверкой входа): своя карточка, сколько узлов в каждой стране и список (`?country=XX`, `??` — без страны).
pub fn router<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
    use axum::extract::Query;
    use axum::routing::get;
    async fn list(Query(q): Query<HashMap<String, String>>) -> axum::Json<serde_json::Value> {
        let country = q.get("country").map(|c| c.to_ascii_uppercase());
        let (offers, countries) = directory_snapshot_all(country.as_deref());
        let offers: Vec<serde_json::Value> = offers
            .into_iter()
            .map(|(o, verified)| {
                let mut v = serde_json::to_value(o).unwrap_or_default();
                v["verified"] = serde_json::json!(verified);
                v
            })
            .collect();
        axum::Json(serde_json::json!({"me": own(), "countries": countries, "offers": offers}))
    }
    axum::Router::new().route("/api/network/offers", get(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_neighbour_flooding_packets_is_read_only_a_few_times_a_minute() {
        let s = [42u8; 32];
        let now = 1_900_000_000;
        let read: u32 = (0..20).map(|_| rate_ok(&s, now) as u32).sum();
        assert_eq!(read, MAX_PACKETS_PER_MINUTE);
        assert!(rate_ok(&s, now + 61), "a new minute, a new allowance");
    }

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    #[test]
    fn a_bound_node_id_cannot_be_claimed_with_another_key() {
        let now = 1_800_000_000;
        let a = SelfAssessment { country: None, country_source: "unknown", public_ip: true, power: "low", cpu_cores: 1, ram_gb: 1, latency_ms: None, addr: vec![] };
        let victim = key(41);
        let victim_id = crate::util::types::derive_node_id(&victim.verifying_key().to_bytes());
        // the owner's own offer is accepted
        assert_eq!(build_offer_for_test(&victim_id, &victim, &a, now).check(now), Ok(()));
        // an impostor signs the victim's id with its own key
        assert_eq!(build_offer_for_test(&victim_id, &key(42), &a, now).check(now), Err("id not bound to key"));
    }

    fn offer(n: u8, country: Option<&str>, now: u64) -> NodeOffer {
        let a = SelfAssessment { country: country.map(String::from), country_source: "ip_lookup", public_ip: true, power: "high", cpu_cores: 8, ram_gb: 32, latency_ms: Some(17), addr: vec![format!("203.0.113.{n}:9000")] };
        build_offer_for_test(&crate::util::types::derive_node_id(&key(n).verifying_key().to_bytes()), &key(n), &a, now)
    }
    // без правил выхода (они глобальные) — те же поля руками
    fn build_offer_for_test(node_id: &[u8; 32], k: &SigningKey, a: &SelfAssessment, now: u64) -> NodeOffer {
        NodeOffer { v: 1, node_id: to_hex(node_id), key: String::new(), country: a.country.clone(), country_source: a.country_source.into(), public_ip: a.public_ip, power: a.power.into(), cpu_cores: a.cpu_cores, ram_gb: a.ram_gb, latency_ms: a.latency_ms, addr: a.addr.clone(), exit: true, can_exit: true, relay: false, issued: now, expires: now + OFFER_TTL_SECS, sig: String::new() }.sign(k)
    }
    const NOW: u64 = 1_800_000_000;

    #[test]
    fn a_signed_card_checks_and_any_change_breaks_it() {
        let o = offer(1, Some("NL"), NOW);
        assert_eq!(o.check(NOW), Ok(()));
        for tamper in [
            |o: &mut NodeOffer| o.country = Some("DE".into()),
            |o: &mut NodeOffer| o.exit = !o.exit,
            |o: &mut NodeOffer| o.power = "low".into(),
            |o: &mut NodeOffer| o.addr = vec!["198.51.100.9:9000".into()],
            |o: &mut NodeOffer| o.expires -= 60,
        ] {
            let mut t = o.clone();
            tamper(&mut t);
            assert_eq!(t.check(NOW), Err("signature"));
        }
        let mut other_key = o.clone();
        other_key.key = to_hex(&key(9).verifying_key().to_bytes());
        assert_eq!(other_key.check(NOW), Err("id not bound to key"), "someone else's key cannot vouch for this card");
        let mut other_id = o.clone();
        other_id.node_id = "ab".repeat(32);
        assert_eq!(other_id.check(NOW), Err("id not bound to key"), "a card cannot carry an id that is not derived from its key");
    }

    #[test]
    fn broken_shapes_and_bad_times_are_refused() {
        assert_eq!(offer(1, Some("nl"), NOW).check(NOW), Err("country"));
        assert_eq!(offer(1, Some("NLD"), NOW).check(NOW), Err("country"));
        assert_eq!(offer(1, None, NOW).check(NOW), Ok(()), "an unknown country is allowed");
        assert_eq!(offer(1, Some("NL"), NOW).check(NOW + OFFER_TTL_SECS), Err("time"), "expired");
        assert_eq!(offer(1, Some("NL"), NOW + 3600).check(NOW), Err("time"), "from the future");
        let mut long = offer(1, Some("NL"), NOW);
        long.expires = NOW + 10 * OFFER_TTL_SECS;
        let long = long.sign(&key(1));
        assert_eq!(long.check(NOW), Err("time"), "a card may not live longer than 2 hours");
        let mut bad_addr = offer(1, Some("NL"), NOW);
        bad_addr.addr = vec!["not an address".into()];
        assert_eq!(bad_addr.sign(&key(1)).check(NOW), Err("addr"));
        assert!(serde_json::from_str::<NodeOffer>(&serde_json::to_string(&offer(1, None, NOW)).unwrap().replace("\"v\":1", "\"v\":1,\"city\":\"Moscow\"")).is_err(), "no extra fields (no city)");
    }

    #[test]
    fn the_directory_keeps_the_newest_card_per_node_pins_keys_and_builds_country_lists() {
        let mut d = Directory::default();
        assert_eq!(d.accept(offer(1, Some("NL"), NOW), NOW), Accept::New);
        assert_eq!(d.accept(offer(1, Some("NL"), NOW), NOW), Accept::Stale);
        assert_eq!(d.accept(offer(1, Some("DE"), NOW + 60), NOW + 60), Accept::Newer, "moved: the newest card wins");
        // тот же номер узла, чужой ключ — отказ (номер выводится из ключа, поэтому чужой ключ не может его занять)
        let mut impostor = offer(1, Some("NL"), NOW + 120);
        impostor = NodeOffer { key: String::new(), ..impostor }.sign(&key(7));
        assert_eq!(d.accept(impostor, NOW + 120), Accept::Refused("id not bound to key"));
        d.accept(offer(2, Some("NL"), NOW), NOW);
        d.accept(offer(3, None, NOW), NOW);
        let nl: Vec<String> = d.by_country(Some("NL"), NOW + 60).iter().map(|o| o.node_id.clone()).collect();
        assert_eq!(nl, vec![offer(2, Some("NL"), NOW).node_id]);
        assert_eq!(d.by_country(Some("DE"), NOW + 60).len(), 1);
        assert_eq!(d.countries(NOW + 60).into_iter().collect::<Vec<_>>(), vec![("??".to_string(), 1), ("DE".to_string(), 1), ("NL".to_string(), 1)]);
        // просроченные выпадают
        assert_eq!(d.fresh(NOW + OFFER_TTL_SECS + 61).len(), 0);
    }

    #[test]
    fn gossip_carries_own_card_first_never_echoes_the_receiver_and_survives_hostile_packets() {
        let mut d = Directory::default();
        for n in 2..40u8 {
            let o = offer(n, Some("NL"), NOW);
            let (id, addrs, until) = (o.node_id.clone(), o.addr.clone(), o.expires);
            d.accept(o, NOW);
            d.mark_verified(&id, addrs, until);
        }
        let own = offer(1, Some("RU"), NOW);
        let to = to_hex(&[5u8; 32]);
        let batch = d.gossip_batch(Some(&own), &to, NOW);
        assert_eq!(batch.len(), GOSSIP_BATCH);
        assert_eq!(batch[0], own);
        assert!(batch.iter().all(|o| o.node_id != to));
        // приём: подделки и мусор не проходят, годные — проходят
        let mut other = Directory::default();
        let mut forged = offer(50, Some("NL"), NOW);
        forged.exit = false;
        let pkt = encode_packet(&[batch.clone(), vec![forged]].concat());
        assert!(pkt.len() <= 60 * 1024);
        let mut padded = pkt.clone();
        padded.extend_from_slice(&[0u8; 8]);
        assert_eq!(receive_packet(&mut other, &padded, NOW), GOSSIP_BATCH, "the forged card is dropped; only the first 24 are read; transport padding is ignored");
        assert_eq!(receive_packet(&mut other, b"\xD8not json", NOW), 0);
        assert_eq!(receive_packet(&mut other, &[0xD7, b'[', b']'], NOW), 0);
        assert_eq!(receive_packet(&mut other, &pkt, NOW), 0, "a repeat adds nothing");
    }

    #[test]
    fn only_cards_proved_by_a_connect_back_are_listed_gossiped_and_used_until_the_address_changes() {
        let mut d = Directory::default();
        let o = offer(1, Some("NL"), NOW);
        let id = o.node_id.clone();
        d.accept(o.clone(), NOW);
        assert!(d.needs_probe(&id, NOW), "a new card must be checked");
        assert!(d.fresh_verified(NOW).is_empty(), "not yet proved: not used");
        let to = to_hex(&[9u8; 32]);
        assert!(d.gossip_batch(None, &to, NOW).is_empty(), "not yet proved: not passed on to neighbours");
        d.mark_failed(&id, NOW + 600);
        assert!(!d.needs_probe(&id, NOW + 599), "after a failure wait");
        assert!(d.needs_probe(&id, NOW + 600));
        d.mark_verified(&id, o.addr.clone(), NOW + 3600);
        assert_eq!(d.fresh_verified(NOW).len(), 1);
        assert!(!d.needs_probe(&id, NOW));
        assert!(!d.is_verified(&id, NOW + 3600), "the proof expires");
        // тот же узел сообщил новый адрес — прежняя проверка не годится
        let mut moved = offer(1, Some("NL"), NOW + 60);
        moved.addr = vec!["203.0.113.99:9000".into()];
        let moved = moved.sign(&key(1));
        assert_eq!(d.accept(moved, NOW + 60), Accept::Newer);
        assert!(!d.is_verified(&id, NOW + 60), "a new address has to be proved again");
        assert!(d.needs_probe(&id, NOW + 60));
    }

    #[test]
    fn countries_are_two_capital_letters() {
        assert_eq!(normalise_country(" ru "), Some("RU".into()));
        assert_eq!(normalise_country("Unknown"), None);
        assert_eq!(normalise_country(""), None);
        assert_eq!(normalise_country("R1"), None);
    }
}
