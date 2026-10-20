//! Заявка узла во входной список (решение владельца 2026-10-19/20).
//!
//! * Узел с белым IP при старте отправляет свою заявку ТОЛЬКО узлам из подписанного бутстрапа; остальным такие карточки не шлются.
//! * В заявке только данные подключения (то, что нужно вписать в список): номер и ключ узла, адреса для узлов, адрес для телефона, страна,
//!   чем может быть. Никаких личных и контактных данных. Заявка подписана ключом узла: её нельзя выдать за чужую.
//! * Принимает заявки только узел, который сам есть в списке; остальные молча игнорируют (они для других целей).
//! * Принятое ничего не добавляет в список: заявка ложится в «Заявки узлов» админки, каждый адрес прозванивается (узел: подпись под свежей
//!   строкой; телефон: `/mobile/hello`), и владелец списка решает сам (`from-card` + подпись из закрытой среды).
//!
//! Канал — обычные зашифрованные пакеты между узлами (`send_encrypted`), нового шифрования нет.
use super::{valid_mobile_addr, Doc, SIGNER_KEYS};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

/// Тип пакета основной связи: заявка узла.
pub const PKT_APPLY: u8 = 0xDE;
const MAX_PACKET: usize = 4096;
/// Сколько заявок держит принимающий узел одновременно.
pub const MAX_PENDING: usize = 64;
/// Чаще раза в минуту от одного заявителя не принимается.
const MIN_GAP_SECS: u64 = 60;
const SKEW_SECS: u64 = 300;
const MAX_LIFE_SECS: u64 = 7 * 86_400;
const SIG_DOMAIN: &[u8] = b"yandi-bootstrap-apply-v1\0";
const ROLES: [&str; 4] = ["entry", "relay", "exit", "guest"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    pub v: u8,
    /// номер узла (64 hex)
    pub node_id: String,
    /// открытый ключ подписи узла (64 hex)
    pub key: String,
    /// адреса основной связи `ip:порт` (до 4)
    pub addr: Vec<String>,
    /// адрес входа для телефона `адрес:порт` (до 2)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mobile: Vec<String>,
    pub region: Option<String>,
    /// чем узел готов быть: entry, relay, exit, guest
    pub roles: Vec<String>,
    pub issued: u64,
    pub expires: u64,
    /// подпись Ed25519 (128 hex) всего остального
    pub sig: String,
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| c.is_ascii_hexdigit())
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    hex::decode(s).ok().and_then(|b| <[u8; N]>::try_from(b).ok())
}

impl Application {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut c = self.clone();
        c.sig = String::new();
        let mut b = SIG_DOMAIN.to_vec();
        b.extend_from_slice(&serde_json::to_vec(&c).unwrap_or_default());
        b
    }

    /// Подписать ключом узла (номер и ключ в заявке — этого узла).
    pub fn sign(mut self, key: &SigningKey) -> Application {
        self.key = hex::encode(key.verifying_key().to_bytes());
        self.sig = String::new();
        self.sig = hex::encode(key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    /// Форма, срок, подпись и то, что номер выведен из ключа.
    pub fn check(&self, now: u64) -> Result<(), &'static str> {
        if self.v != 1 || !is_hex(&self.node_id, 64) || !is_hex(&self.key, 64) || !is_hex(&self.sig, 128) {
            return Err("форма");
        }
        let (Some(id), Some(key)) = (unhex::<32>(&self.node_id), unhex::<32>(&self.key)) else { return Err("форма") };
        if !crate::util::types::id_acceptable(&id, &key) {
            return Err("номер не выведен из ключа");
        }
        if self.addr.is_empty() || self.addr.len() > 4 || self.addr.iter().any(|a| a.parse::<std::net::SocketAddr>().is_err()) {
            return Err("адреса узла");
        }
        if self.mobile.len() > 2 || self.mobile.iter().any(|a| !valid_mobile_addr(a)) {
            return Err("адрес для телефона");
        }
        if let Some(r) = &self.region {
            if r.len() != 2 || !r.bytes().all(|b| b.is_ascii_uppercase()) {
                return Err("страна");
            }
        }
        if self.roles.is_empty() || self.roles.len() > 4 || self.roles.iter().any(|r| !ROLES.contains(&r.as_str())) {
            return Err("роли");
        }
        if self.roles.iter().any(|r| r == "guest") && self.mobile.is_empty() {
            return Err("guest без адреса для телефона");
        }
        if self.issued > now + SKEW_SECS || self.expires <= now || self.expires > self.issued.saturating_add(MAX_LIFE_SECS) {
            return Err("срок");
        }
        let vk = VerifyingKey::from_bytes(&key).map_err(|_| "ключ")?;
        let sig = Signature::from_bytes(&unhex::<64>(&self.sig).ok_or("форма")?);
        vk.verify(&self.signing_bytes(), &sig).map_err(|_| "подпись")
    }

    /// Визитка для `yandi bootstrap from-card` (только открытые данные).
    pub fn card_text(&self) -> String {
        let card = crate::web::peers::NodeCard { id: self.node_id.clone(), key: self.key.clone(), addr: self.addr.clone(), p2p: vec![], name: None };
        crate::web::peers::encode_card(&card)
    }

    /// Готовая команда для владельца списка.
    pub fn command(&self) -> String {
        let mobile: String = self.mobile.iter().map(|m| format!(" --mobile {m}")).collect();
        let region = self.region.as_deref().map(|r| format!(" --region {r}")).unwrap_or_default();
        format!("yandi bootstrap from-card bootstrap/bootstrap.json \"{}\"{mobile}{region} --roles {}", self.card_text(), self.roles.join(","))
    }
}

/// Пакет: `[PKT_APPLY][JSON]`.
pub fn encode_packet(a: &Application) -> Vec<u8> {
    let mut p = vec![PKT_APPLY];
    p.extend_from_slice(&serde_json::to_vec(a).unwrap_or_default());
    p
}

/// Разобрать пакет (основная связь дополняет данные нулями — снимаем).
pub fn decode_packet(plaintext: &[u8]) -> Option<Application> {
    if plaintext.first() != Some(&PKT_APPLY) || plaintext.len() > MAX_PACKET {
        return None;
    }
    let body = &plaintext[1..];
    let end = body.iter().rposition(|&b| b != 0).map(|i| i + 1)?;
    serde_json::from_slice(&body[..end]).ok()
}

// ---------------------------------------------------------------- принимающая сторона

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub addr: String,
    /// `node` (основная связь) или `mobile` (вход для телефона)
    pub kind: String,
    /// `ok` / `fail` / `skipped` (не проверялся: имя вместо IP, закрытый адрес, нет IPv6 у проверяющего)
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub app: Application,
    pub received: u64,
    /// `new` / `verified` / `unreachable` / `dismissed` / `listed`
    pub status: String,
    #[serde(default)]
    pub probes: Vec<Probe>,
}

static STORE: Mutex<Option<Vec<Pending>>> = Mutex::new(None);
static OWN_ID: OnceLock<String> = OnceLock::new();

fn store_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("bootstrap_applications.json")
}

fn with_store<R>(f: impl FnOnce(&mut Vec<Pending>) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let v = g.get_or_insert_with(|| std::fs::read(store_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
    let r = f(v);
    // это открытые сведения (никаких секретов): обычный файл, запись через временный
    let p = store_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
    r
}

/// Заявки для страницы администратора (новые сверху).
pub fn list() -> Vec<Pending> {
    let mut v = with_store(|s| s.clone());
    v.sort_by(|a, b| b.received.cmp(&a.received));
    v
}

/// Отклонить заявку (она остаётся записью «отклонена», чтобы повторная не всплывала сразу).
pub fn dismiss(id: &str) -> bool {
    with_store(|s| match s.iter_mut().find(|p| p.app.node_id.starts_with(&id.to_ascii_lowercase()) && id.len() >= 8) {
        Some(p) => {
            p.status = "dismissed".into();
            true
        }
        None => false,
    })
}

/// Результат разбора пакета.
#[derive(Debug, PartialEq)]
pub enum Accept {
    /// принята (новая или обновлённая): надо прозвонить адреса
    New,
    /// игнорируется: этот узел не во входном списке, пакет не по форме, чужой отправитель, слишком часто, дубль, переполнено
    Ignored(&'static str),
    /// узел уже в списке
    Listed,
}

/// Принять заявку: чистая логика (без сети), чтобы проверять тестами.
/// `listed_ids` — номера узлов подписанного списка; `own_id` — номер этого узла.
pub fn accept(store: &mut Vec<Pending>, listed_ids: &std::collections::HashSet<String>, own_id: &str, sender: &[u8; 32], plaintext: &[u8], now: u64) -> Accept {
    // заявки принимает только узел из списка; остальные их игнорируют
    if !listed_ids.contains(&own_id.to_ascii_lowercase()) {
        return Accept::Ignored("этот узел не во входном списке");
    }
    let Some(app) = decode_packet(plaintext) else { return Accept::Ignored("не по форме") };
    if app.node_id != hex::encode(sender) {
        return Accept::Ignored("отправитель не совпадает с заявителем");
    }
    if app.check(now).is_err() {
        return Accept::Ignored("проверка заявки");
    }
    store.retain(|p| p.app.expires > now || p.status == "dismissed");
    if listed_ids.contains(&app.node_id) {
        return Accept::Listed;
    }
    if let Some(old) = store.iter_mut().find(|p| p.app.node_id == app.node_id) {
        if now.saturating_sub(old.received) < MIN_GAP_SECS {
            return Accept::Ignored("слишком часто");
        }
        if old.status == "dismissed" && old.app.addr == app.addr && old.app.mobile == app.mobile {
            return Accept::Ignored("отклонённая заявка без изменений");
        }
        old.app = app;
        old.received = now;
        old.status = "new".into();
        old.probes.clear();
        return Accept::New;
    }
    if store.len() >= MAX_PENDING {
        return Accept::Ignored("список заявок полон");
    }
    store.push(Pending { app, received: now, status: "new".into(), probes: vec![] });
    Accept::New
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn listed_ids(d: &Doc) -> std::collections::HashSet<String> {
    d.entries.iter().map(|e| e.id.to_ascii_lowercase()).collect()
}

/// Пакет от соседа (вызывается из разбора пакетов основной связи).
pub fn on_packet(sender: &[u8; 32], plaintext: &[u8]) {
    let (Some(own), Some(doc)) = (OWN_ID.get(), super::load_cached(SIGNER_KEYS, now_secs())) else { return };
    let ids = listed_ids(&doc);
    let now = now_secs();
    let (res, app) = with_store(|s| {
        let r = accept(s, &ids, own, sender, plaintext, now);
        let app = if r == Accept::New { s.iter().find(|p| p.app.node_id == hex::encode(sender)).map(|p| p.app.clone()) } else { None };
        (r, app)
    });
    if res == Accept::New {
        if let (Some(app), Ok(h)) = (app, tokio::runtime::Handle::try_current()) {
            println!("[bootstrap] заявка узла {}… принята, прозваниваю адреса", &app.node_id[..8]);
            h.spawn(async move { probe_application(app).await });
        }
    }
}

static PROBES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// Прозвонить адреса заявки встречным подключением и записать результат; заявка «проверена», если хотя бы один адрес узла ответил его ключом.
async fn probe_application(app: Application) {
    let Ok(_permit) = PROBES.acquire().await else { return };
    let mut probes = Vec::new();
    for a in &app.addr {
        let r = match crate::reachability::probe_identity(&app.key, &app.node_id, a).await {
            Ok(()) => "ok",
            Err("address not allowed") => "skipped",
            Err(_) => "fail",
        };
        probes.push(Probe { addr: a.clone(), kind: "node".into(), result: r.into() });
    }
    for m in &app.mobile {
        let r = match probe_mobile(m, &app.key, &app.node_id, false).await {
            Ok(()) => "ok",
            Err("skipped") => "skipped",
            Err(_) => "fail",
        };
        probes.push(Probe { addr: m.clone(), kind: "mobile".into(), result: r.into() });
    }
    let node_ok = probes.iter().any(|p| p.kind == "node" && p.result == "ok");
    with_store(|s| {
        if let Some(p) = s.iter_mut().find(|p| p.app.node_id == app.node_id && p.app.issued == app.issued) {
            if p.status == "new" {
                p.status = if node_ok { "verified" } else { "unreachable" }.into();
            }
            p.probes = probes;
        }
    });
}

/// Проверить вход для телефона: узел обязан представиться (`/mobile/hello`) подписью своего TLS-сертификата ключом из заявки. Только адреса-IP
/// (имена не разрешаются, чтобы заявка не стала способом стучаться куда угодно) и только публичные, кроме тренировочной сети.
/// `lenient` — разрешить любые IP (для тестов).
pub async fn probe_mobile(addr: &str, key_hex: &str, id_hex: &str, lenient: bool) -> Result<(), &'static str> {
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let sa: std::net::SocketAddr = addr.parse().map_err(|_| "skipped")?;
    if !lenient && !(std::env::var(crate::testnet::ENV).is_ok() || crate::exit_policy::public_only(&sa.ip())) {
        return Err("skipped");
    }
    let vk = unhex::<32>(key_hex).and_then(|k| VerifyingKey::from_bytes(&k).ok()).ok_or("key")?;
    let mut nonce = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);
    let work = async {
        let tcp = tokio::net::TcpStream::connect(sa).await.map_err(|_| "connect")?;
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|_| "tls")?
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAny(provider.signature_verification_algorithms.supported_schemes())))
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::IpAddress(sa.ip().into());
        let mut tls = tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg)).connect(name, tcp).await.map_err(|_| "tls")?;
        let fp = {
            let (_, conn) = tls.get_ref();
            let der = conn.peer_certificates().and_then(|c| c.first()).ok_or("cert")?;
            hex::encode(Sha256::digest(der.as_ref()))
        };
        let req = format!("GET /mobile/hello?nonce={} HTTP/1.1\r\nHost: node\r\nConnection: close\r\n\r\n", hex::encode(nonce));
        tls.write_all(req.as_bytes()).await.map_err(|_| "write")?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while buf.len() < 8192 {
            match tls.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("reply")?;
        if !buf.starts_with(b"HTTP/1.1 200") {
            return Err("status");
        }
        let v: serde_json::Value = serde_json::from_slice(&buf[split + 4..]).map_err(|_| "json")?;
        if v["node_id"].as_str().map(str::to_ascii_lowercase).as_deref() != Some(id_hex.to_ascii_lowercase().as_str())
            || v["key"].as_str().map(str::to_ascii_lowercase).as_deref() != Some(key_hex.to_ascii_lowercase().as_str())
        {
            return Err("identity");
        }
        let claimed = v["tls_fingerprint"].as_str().unwrap_or("").to_ascii_lowercase();
        if claimed != fp {
            return Err("fingerprint");
        }
        let sig = Signature::from_bytes(&unhex::<64>(v["signature"].as_str().unwrap_or("")).ok_or("signature")?);
        vk.verify(&crate::mobile_api::hello_message(&nonce, &claimed), &sig).map_err(|_| "signature")?;
        Ok(())
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), work).await.map_err(|_| "timeout")?
}

/// Проверяющий сертификатов, который принимает любой: отпечаток сверяется отдельно с подписанным узлом (см. `probe_mobile`).
#[derive(Debug)]
struct AcceptAny(Vec<rustls::SignatureScheme>);

impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.clone()
    }
}

// ---------------------------------------------------------------- отправляющая сторона

/// Отправлять ли заявки (по умолчанию да; `YANDI_BOOTSTRAP_APPLY=0` выключает).
fn enabled() -> bool {
    std::env::var("YANDI_BOOTSTRAP_APPLY").map_or(true, |v| v != "0")
}

/// Собрать заявку этого узла из его карточки сети: адреса, адрес для телефона (внешний IP и порт входа для телефона), страна.
fn build_own(key: &SigningKey, node_id: [u8; 32], now: u64) -> Option<Application> {
    let own = crate::network_offers::own()?;
    if !own.public_ip {
        return None;
    }
    let addr: Vec<String> = own.addr.iter().filter(|a| a.parse::<std::net::SocketAddr>().is_ok()).take(4).cloned().collect();
    let first = addr.first()?.parse::<std::net::SocketAddr>().ok()?;
    let mobile: Vec<String> = crate::mobile_tls::configured_port()
        .map(|p| vec![std::net::SocketAddr::new(first.ip(), p).to_string()])
        .unwrap_or_default();
    let mut roles = vec!["entry".to_string()];
    if own.relay {
        roles.push("relay".into());
    }
    if own.exit {
        roles.push("exit".into());
    }
    if !mobile.is_empty() {
        roles.push("guest".into());
    }
    let app = Application { v: 1, node_id: hex::encode(node_id), key: String::new(), addr, mobile, region: own.country.clone(), roles, issued: now, expires: now + 24 * 3600, sig: String::new() };
    Some(app.sign(key))
}

/// Запустить отправку заявок: после старта узел получает список (скачанный или сохранённый) и шлёт заявку каждому входному узлу, кроме себя.
pub fn start(transport: std::sync::Arc<crate::netlayer::transport::P2PTransport>, node_id: [u8; 32], key: SigningKey) {
    let _ = OWN_ID.set(hex::encode(node_id));
    if !enabled() {
        return;
    }
    crate::supervisor::supervise("bootstrap_apply", crate::supervisor::Policy::restart(), move || {
        let (transport, key) = (transport.clone(), key.clone());
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
            loop {
                let wait = match one_round(&transport, node_id, &key).await {
                    Round::Sent => 6 * 3600,
                    Round::Retry => 300,
                    Round::Idle => 3600,
                };
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
        }
    });
}

enum Round {
    /// всем входным узлам отправлено
    Sent,
    /// кому-то не удалось (нет соединения): повторить скоро
    Retry,
    /// слать нечего или незачем (нет белого IP, нет списка, узел уже в списке)
    Idle,
}

async fn one_round(transport: &std::sync::Arc<crate::netlayer::transport::P2PTransport>, node_id: [u8; 32], key: &SigningKey) -> Round {
    let Some(doc) = super::fetch(SIGNER_KEYS).await else { return Round::Idle };
    let me = hex::encode(node_id);
    // уже в списке — заявка не нужна
    if doc.entries.iter().any(|e| e.id.eq_ignore_ascii_case(&me)) {
        return Round::Idle;
    }
    let Some(app) = build_own(key, node_id, now_secs()) else { return Round::Idle };
    let pkt = encode_packet(&app);
    let mut all = true;
    let mut sent = 0;
    for e in doc.entries.iter().filter(|e| e.roles.iter().any(|r| r == "entry")) {
        let Some(id) = unhex::<32>(&e.id) else { continue };
        match transport.send_encrypted(crate::util::types::HashId(id), &pkt).await {
            Ok(()) => sent += 1,
            Err(_) => all = false,
        }
    }
    if sent > 0 {
        println!("[bootstrap] заявка во входной список отправлена узлам списка: {sent}");
    }
    if all { Round::Sent } else { Round::Retry }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    fn node_id(k: &SigningKey) -> [u8; 32] {
        crate::util::types::derive_node_id(&k.verifying_key().to_bytes())
    }
    fn app(k: &SigningKey, now: u64) -> Application {
        Application {
            v: 1,
            node_id: hex::encode(node_id(k)),
            key: String::new(),
            addr: vec!["203.0.113.5:9000".into()],
            mobile: vec!["203.0.113.5:8444".into()],
            region: Some("NL".into()),
            roles: vec!["entry".into(), "guest".into()],
            issued: now,
            expires: now + 3600,
            sig: String::new(),
        }
        .sign(k)
    }
    fn ids(own: &str) -> HashSet<String> {
        [own.to_string()].into_iter().collect()
    }

    #[test]
    fn an_application_is_signed_checked_and_every_tampering_is_refused() {
        let (k, now) = (key(1), 1_000_000);
        let a = app(&k, now);
        assert_eq!(a.check(now), Ok(()));
        let mut t = a.clone();
        t.addr = vec!["203.0.113.66:9000".into()];
        assert_eq!(t.check(now), Err("подпись"));
        let mut t = a.clone();
        t.mobile = vec![];
        assert!(t.check(now).is_err(), "подпись/guest без телефона");
        // номер должен быть выведен из ключа: чужой номер с чужой подписью не проходит
        let mut t = a.clone();
        t.node_id = hex::encode(node_id(&key(2)));
        assert_eq!(t.check(now), Err("номер не выведен из ключа"));
        assert!(a.check(now + 4000).is_err(), "срок вышел");
        let mut fut = a.clone();
        fut.issued = now + 10_000;
        assert!(fut.sign(&k).check(now).is_err(), "из будущего");
        let mut long = a.clone();
        long.expires = now + 30 * 86_400;
        assert!(long.sign(&k).check(now).is_err(), "слишком долгий срок");
        let mut bad = a.clone();
        bad.roles = vec!["boss".into()];
        assert_eq!(bad.sign(&k).check(now), Err("роли"));
        let mut bad = a.clone();
        bad.mobile = vec!["bad host:1".into()];
        assert_eq!(bad.sign(&k).check(now), Err("адрес для телефона"));
        let mut bad = a.clone();
        bad.addr = vec!["не адрес".into()];
        assert_eq!(bad.sign(&k).check(now), Err("адреса узла"));
        assert!(a.command().contains("--mobile 203.0.113.5:8444") && a.command().contains("--roles entry,guest") && a.command().contains("--region NL"));
        assert!(crate::web::peers::decode_card(&a.card_text()).is_ok(), "визитка читается тем же разбором, что у from-card");
    }

    #[test]
    fn the_packet_round_trips_with_padding_and_is_bounded() {
        let a = app(&key(1), 1000);
        let mut p = encode_packet(&a);
        assert_eq!(decode_packet(&p), Some(a.clone()));
        p.extend_from_slice(&[0u8; 50]); // основная связь дополняет нулями
        assert_eq!(decode_packet(&p), Some(a));
        assert_eq!(decode_packet(&[PKT_APPLY]), None);
        assert_eq!(decode_packet(&[0x00, 1, 2]), None);
        let mut big = vec![PKT_APPLY];
        big.extend(std::iter::repeat(b'a').take(MAX_PACKET));
        assert_eq!(decode_packet(&big), None);
    }

    #[test]
    fn only_a_listed_node_takes_applications_and_only_from_the_applicant_itself() {
        let (k, other, now) = (key(1), key(2), 1_000_000);
        let me = hex::encode(node_id(&key(9)));
        let pkt = encode_packet(&app(&k, now));
        let sender = node_id(&k);
        // не во входном списке — игнорируется молча
        let mut store = vec![];
        assert_eq!(accept(&mut store, &HashSet::new(), &me, &sender, &pkt, now), Accept::Ignored("этот узел не во входном списке"));
        assert!(store.is_empty());
        // отправитель не тот, чья заявка (чужую заявку нельзя переслать от имени другого)
        assert_eq!(accept(&mut store, &ids(&me), &me, &node_id(&other), &pkt, now), Accept::Ignored("отправитель не совпадает с заявителем"));
        // верная
        assert_eq!(accept(&mut store, &ids(&me), &me, &sender, &pkt, now), Accept::New);
        assert_eq!((store.len(), store[0].status.as_str()), (1, "new"));
        // слишком часто
        assert_eq!(accept(&mut store, &ids(&me), &me, &sender, &pkt, now + 10), Accept::Ignored("слишком часто"));
        // позже — обновляется, дубля нет
        assert_eq!(accept(&mut store, &ids(&me), &me, &sender, &pkt, now + 120), Accept::New);
        assert_eq!(store.len(), 1);
        // отклонённая без изменений не всплывает снова
        store[0].status = "dismissed".into();
        assert_eq!(accept(&mut store, &ids(&me), &me, &sender, &pkt, now + 300), Accept::Ignored("отклонённая заявка без изменений"));
        // узел уже в списке
        let mut listed = ids(&me);
        listed.insert(hex::encode(sender));
        assert_eq!(accept(&mut vec![], &listed, &me, &sender, &pkt, now), Accept::Listed);
        // мусор и просроченное
        assert_eq!(accept(&mut vec![], &ids(&me), &me, &sender, b"\xDEnot json", now), Accept::Ignored("не по форме"));
        assert_eq!(accept(&mut vec![], &ids(&me), &me, &sender, &pkt, now + 100_000), Accept::Ignored("проверка заявки"));
    }

    #[test]
    fn the_pending_list_is_capped_and_never_pushes_out_existing_applications() {
        let (me, now) = (hex::encode(node_id(&key(250))), 1_000_000);
        let mut store = vec![];
        for n in 1..=MAX_PENDING as u8 {
            let k = key(n);
            let r = accept(&mut store, &ids(&me), &me, &node_id(&k), &encode_packet(&app(&k, now)), now);
            assert_eq!(r, Accept::New, "{n}");
        }
        let k = key(200);
        assert_eq!(accept(&mut store, &ids(&me), &me, &node_id(&k), &encode_packet(&app(&k, now)), now), Accept::Ignored("список заявок полон"));
        assert_eq!(store.len(), MAX_PENDING);
    }

    /// Узел телефона в тесте: локальный TLS-сервер, который отвечает на /mobile/hello так же, как узел.
    async fn fake_mobile_node(sk: SigningKey, lie_about_fingerprint: bool, wrong_key: bool) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let id = crate::netlayer::tls_cert::TlsIdentity::load_or_generate_in(dir.path(), "aabbccdd").unwrap();
        let cfg = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(id.cert_chain.clone(), id.private_key.clone_key())
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(cfg));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let fp = id.fingerprint_hex.clone();
        let h = tokio::spawn(async move {
            let _keep = dir;
            while let Ok((s, _)) = l.accept().await {
                let (acceptor, sk, fp) = (acceptor.clone(), sk.clone(), fp.clone());
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(s).await else { return };
                    let mut buf = vec![0u8; 2048];
                    let n = tls.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let nonce = req.split("nonce=").nth(1).and_then(|r| r.split(' ').next()).unwrap_or("");
                    let nonce = hex::decode(nonce).unwrap_or_default();
                    let claimed = if lie_about_fingerprint { "cd".repeat(32) } else { fp.clone() };
                    let sig = sk.sign(&crate::mobile_api::hello_message(&nonce, &claimed));
                    let key = if wrong_key { "11".repeat(32) } else { hex::encode(sk.verifying_key().to_bytes()) };
                    let body = serde_json::json!({"node_id": hex::encode(crate::util::types::derive_node_id(&sk.verifying_key().to_bytes())), "key": key, "tls_fingerprint": claimed, "signature": hex::encode(sig.to_bytes())}).to_string();
                    let _ = tls.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        (addr, h)
    }

    #[tokio::test]
    async fn the_phone_address_is_probed_by_the_nodes_own_introduction_and_fakes_do_not_pass() {
        let sk = key(7);
        let (key_hex, id_hex) = (hex::encode(sk.verifying_key().to_bytes()), hex::encode(node_id(&sk)));
        let (addr, h) = fake_mobile_node(sk.clone(), false, false).await;
        assert_eq!(probe_mobile(&addr, &key_hex, &id_hex, true).await, Ok(()), "настоящий узел представился");
        // публичные проверки не стучатся в закрытые адреса (заявка не сканер домашней сети)
        assert_eq!(probe_mobile(&addr, &key_hex, &id_hex, false).await, Err("skipped"));
        assert_eq!(probe_mobile("не адрес", &key_hex, &id_hex, true).await, Err("skipped"), "имена не разрешаются");
        // ответил другой ключ / номер не тот
        assert!(probe_mobile(&addr, &"22".repeat(32), &id_hex, true).await.is_err());
        h.abort();
        let (addr, h) = fake_mobile_node(sk.clone(), true, false).await;
        assert_eq!(probe_mobile(&addr, &key_hex, &id_hex, true).await, Err("fingerprint"), "подписан не тот сертификат, по которому подключились");
        h.abort();
        let (addr, h) = fake_mobile_node(sk.clone(), false, true).await;
        assert_eq!(probe_mobile(&addr, &key_hex, &id_hex, true).await, Err("identity"));
        h.abort();
        // подставной узел без ключа не сможет подписать: подпись другого ключа
        let (addr, h) = fake_mobile_node(key(8), false, false).await;
        assert!(probe_mobile(&addr, &key_hex, &id_hex, true).await.is_err());
        h.abort();
    }
}
