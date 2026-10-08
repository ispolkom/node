//! Доверенные узлы: визитка этого узла и список узлов, которым он доверяет (проверка ответов, обмен мнениями, трассами и знаниями —
//! договор, приложение H).
//!
//! Визитка — одна строка `YANDI-NODE-1:<base64url JSON>`: номер узла, его открытый ключ подписи и где он слушает. В ней нет ничего
//! секретного: владелец передаёт её другому человеку любым способом, тот вставляет её у себя — и узлы начинают доверять друг другу
//! (доверие взаимное только когда оба добавили визитки друг друга). Добавить, убрать и посмотреть список может только вошедший владелец.
//!
//! Добавление действует сразу, без перезапуска: пир попадает в разрешённые AI-RPC, его ключ закрепляется за его номером на уровне
//! связи (чужой ключ с тем же номером — отказ), узел сам стучится по адресам из визитки и потом раз в 30 секунд, пока пир не в сети.
//! Тот же номер с другим ключом не заменяет запись молча: сначала убрать, потом добавить (подмена ключа — повод насторожиться).
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};


use std::collections::HashSet;
use std::path::Path;
use crate::util::HashId;

/// Доверенный узел: что о нём записал владелец (из визитки).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrustedPeer {
    pub node_id_hex: String,
    pub signing_pubkey_hex: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Где узел слушает (`адрес:порт` обнаружения) — из его визитки; к ним этот узел стучится сам.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    /// Где у узла канал переписки, файлов и звонков — тоже из визитки.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub p2p_addresses: Vec<String>,
}

/// Список доверенных узлов (открытые значения: номер узла и публичный ключ, не секреты).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PeerDirectory {
    pub peers: Vec<TrustedPeer>,
}

impl PeerDirectory {
    pub fn load_or_default(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Decode every entry to `(HashId, [u8; 32] signing pubkey, name)`,
    /// skipping (and logging) any malformed row rather than failing the
    /// whole directory over one bad entry.
    pub fn decoded(&self) -> Vec<(HashId, [u8; 32], Option<String>)> {
        let mut out = Vec::with_capacity(self.peers.len());
        for p in &self.peers {
            let node_id = match hex::decode(p.node_id_hex.trim()) {
                Ok(b) if b.len() == 32 => HashId(b.try_into().unwrap()),
                _ => {
                    eprintln!("[peers] skipping entry with malformed node_id_hex: {}", p.node_id_hex);
                    continue;
                }
            };
            let pubkey = match hex::decode(p.signing_pubkey_hex.trim()) {
                Ok(b) if b.len() == 32 => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&b);
                    arr
                }
                _ => {
                    eprintln!("[peers] skipping entry with malformed signing_pubkey_hex: {}", p.signing_pubkey_hex);
                    continue;
                }
            };
            out.push((node_id, pubkey, p.name.clone()));
        }
        out
    }

    /// Сохранить целиком (через временный файл рядом — чтобы при сбое не остался половинчатый список).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }
}

/// The signing key of a trusted contact, if the owner has one with this node id.
pub fn key_hex_of(node_hex: &str) -> Option<String> {
    let dir = PeerDirectory::load_or_default(&default_peer_directory_path());
    dir.peers.iter().find(|p| p.node_id_hex.eq_ignore_ascii_case(node_hex)).map(|p| p.signing_pubkey_hex.to_ascii_lowercase())
}

pub fn default_peer_directory_path() -> PathBuf {
    let home = std::env::var_os("YANDI_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."));
    home.join(".yandi").join("trusted_peers.json")
}

/// Транспорт узла: ставится один раз при запуске, нужен, чтобы закреплять ключи и стучаться к узлам.
fn transport_cell() -> &'static OnceLock<std::sync::Arc<crate::netlayer::transport::P2PTransport>> {
    static C: OnceLock<std::sync::Arc<crate::netlayer::transport::P2PTransport>> = OnceLock::new();
    &C
}

pub fn install_transport(t: std::sync::Arc<crate::netlayer::transport::P2PTransport>) {
    let _ = transport_cell().set(t);
}

/// При запуске: применить сохранённый список (закрепить ключи, сообщить политике выхода).
pub async fn apply_saved() {
    let dir = PeerDirectory::load_or_default(&default_peer_directory_path());
    apply_live(&dir, None).await;
}


const PREFIX: &str = "YANDI-NODE-1:";
const MAX_ADDRESSES: usize = 4;
const MAX_NAME_CHARS: usize = 64;
const MAX_CARD_CHARS: usize = 2048;
const RECONNECT_EVERY: Duration = Duration::from_secs(30);

/// Визитка узла: открытые сведения, по которым другой узел может ему доверять и до него достучаться.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeCard {
    /// номер узла (64 шестнадцатеричных знака)
    pub id: String,
    /// открытый ключ подписи Ed25519 (64 шестнадцатеричных знака)
    pub key: String,
    /// где узел слушает: `адрес:порт` обнаружения (может быть пусто)
    #[serde(default)]
    pub addr: Vec<String>,
    /// где у узла канал переписки, файлов и звонков (адрес знакомства этого канала)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub p2p: Vec<String>,
    /// имя, которое владелец дал своему узлу (подсказка; у себя можно назвать иначе)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

/// `адрес:порт`, где адрес — IP (IPv6 в квадратных скобках) или имя хоста.
fn valid_address(a: &str) -> bool {
    if a.len() > 262 {
        return false;
    }
    if a.parse::<std::net::SocketAddr>().is_ok() {
        return !a.ends_with(":0");
    }
    let Some((host, port)) = a.rsplit_once(':') else { return false };
    let port_ok = port.parse::<u16>().map(|p| p > 0).unwrap_or(false);
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'));
    port_ok && host_ok
}

fn clean_name(n: Option<&str>) -> Result<Option<String>, String> {
    match n.map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) if s.chars().count() > MAX_NAME_CHARS => Err(format!("Имя — не длиннее {MAX_NAME_CHARS} знаков.")),
        Some(s) if s.chars().any(char::is_control) => Err("В имени есть служебные знаки.".into()),
        Some(s) => Ok(Some(s.to_string())),
    }
}

impl NodeCard {
    fn check(mut self) -> Result<NodeCard, String> {
        if !is_hex64(&self.id) || !is_hex64(&self.key) {
            return Err("В визитке неверный номер узла или ключ.".into());
        }
        self.id = self.id.to_ascii_lowercase();
        self.key = self.key.to_ascii_lowercase();
        let (id, key) = (hex::decode(&self.id).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()), hex::decode(&self.key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()));
        if let (Some(id), Some(key)) = (id, key) {
            if !crate::util::types::id_acceptable(&id, &key) {
                return Err("Номер узла в визитке не сходится с его ключом.".into());
            }
        }
        if self.addr.len() > MAX_ADDRESSES || !self.addr.iter().all(|a| valid_address(a)) || self.p2p.len() > MAX_ADDRESSES || !self.p2p.iter().all(|a| valid_address(a)) {
            return Err("В визитке неверные адреса.".into());
        }
        self.name = clean_name(self.name.as_deref())?;
        Ok(self)
    }
}

pub fn encode_card(card: &NodeCard) -> String {
    let body = serde_json::to_vec(card).unwrap_or_default();
    format!("{PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body))
}

/// Разобрать вставленную визитку (пробелы и переносы по краям и внутри — от копирования — не мешают).
pub fn decode_card(text: &str) -> Result<NodeCard, String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() > MAX_CARD_CHARS {
        return Err("Визитка слишком длинная.".into());
    }
    let Some(b64) = compact.strip_prefix(PREFIX) else { return Err("Это не визитка узла YANDI (она начинается с «YANDI-NODE-1:»).".into()) };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64).map_err(|_| "Визитка повреждена (не читается).".to_string())?;
    let card: NodeCard = serde_json::from_slice(&bytes).map_err(|_| "Визитка повреждена (не читается).".to_string())?;
    card.check()
}

/// Визитка этого узла; при смене внешнего адреса заменяется (старая освобождается, когда ею перестают пользоваться).
fn my_card_cell() -> &'static std::sync::RwLock<Option<std::sync::Arc<NodeCard>>> {
    static C: std::sync::RwLock<Option<std::sync::Arc<NodeCard>>> = std::sync::RwLock::new(None);
    &C
}

fn card_file() -> PathBuf {
    default_peer_directory_path().with_file_name("node_card.txt")
}

/// Запомнить визитку этого узла (при запуске) и положить её копию в `~/.yandi/node_card.txt` — там только открытые сведения.
pub fn install_my_card(node_id: &[u8; 32], signing_pubkey: &[u8; 32], addresses: Vec<String>, p2p_addresses: Vec<String>) {
    let keep = |v: Vec<String>| v.into_iter().filter(|a| valid_address(a)).take(MAX_ADDRESSES).collect();
    let card = NodeCard { id: hex::encode(node_id), key: hex::encode(signing_pubkey), addr: keep(addresses), p2p: keep(p2p_addresses), name: None };
    let path = card_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, encode_card(&card) + "\n") {
        eprintln!("[peers] не удалось записать визитку {}: {e}", path.display());
    }
    *my_card_cell().write().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(card));
}

pub fn my_card() -> Option<std::sync::Arc<NodeCard>> {
    my_card_cell().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Канал переписки, файлов и звонков (ставится при запуске узла): к доверенным узлам этот узел стучится и по нему.
fn p2p_cell() -> &'static OnceLock<std::sync::Arc<crate::p2p::P2PTransport>> {
    static P: OnceLock<std::sync::Arc<crate::p2p::P2PTransport>> = OnceLock::new();
    &P
}

pub fn install_p2p(t: std::sync::Arc<crate::p2p::P2PTransport>) {
    let _ = p2p_cell().set(t);
}

/// Файл списка меняется только под этим замком (две вкладки владельца не перетрут друг друга).
fn edit_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Применить список к работающему узлу: закреплённые ключи на уровне связи и список для политики выхода.
async fn apply_live(dir: &PeerDirectory, _removed: Option<[u8; 32]>) {
    crate::exit_policy::set_trusted(dir.decoded().into_iter().map(|(id, _, _)| id.0));
    let Some(transport) = transport_cell().get() else { return };
    let pins: std::collections::HashMap<_, _> = dir.decoded().into_iter().map(|(id, pubkey, _)| (id, pubkey)).collect();
    transport.set_pinned_identities(pins).await;
}

/// Постучаться к доверенным пирам, которых сейчас нет в сети (по адресам из их визиток) — и по основной связи узлов, и по каналу
/// переписки, файлов и звонков.
pub async fn knock_offline() {
    let Some(transport) = transport_cell().get().cloned() else { return };
    let dir = PeerDirectory::load_or_default(&default_peer_directory_path());
    let online: std::collections::HashSet<_> = transport.get_peers().await.into_iter().map(|p| p.id.to_hex()).collect();
    let p2p = p2p_cell().get();
    let p2p_known: std::collections::HashSet<String> = match p2p {
        Some(t) => t.list_peers().await.into_iter().map(|p| p.id.to_hex()).collect(),
        None => Default::default(),
    };
    for p in &dir.peers {
        let id = p.node_id_hex.to_ascii_lowercase();
        if !online.contains(&id) {
            for a in &p.addresses {
                if let Err(e) = transport.send_hello_request(a).await {
                    eprintln!("[peers] не достучался до {a}: {e}");
                }
            }
        }
        if let Some(t) = p2p {
            if !p2p_known.contains(&id) {
                for a in &p.p2p_addresses {
                    if let Err(e) = t.send_hello_request(a).await {
                        eprintln!("[peers] канал переписки: не достучался до {a}: {e}");
                    }
                }
            }
        }
    }
}

/// Стучаться к отсутствующим доверенным пирам сейчас и потом раз в 30 секунд (запускается один раз при старте узла).
pub fn spawn_reconnect_loop() {
    tokio::spawn(async {
        loop {
            knock_offline().await;
            tokio::time::sleep(RECONNECT_EVERY).await;
        }
    });
}

#[derive(Debug, PartialEq)]
pub enum AddOutcome {
    Added,
    Updated,
}

/// Добавить (или обновить имя/адреса) доверенный узел по визитке. Сохраняет список и сразу применяет его.
pub async fn add_trusted(card: &NodeCard, name: Option<&str>) -> Result<AddOutcome, String> {
    let card = card.clone().check()?;
    let name = clean_name(name)?.or_else(|| card.name.clone());
    if my_card().map(|m| m.id == card.id).unwrap_or(false) {
        return Err("Это визитка этого же узла.".into());
    }
    let _g = edit_lock().lock().await;
    let path = default_peer_directory_path();
    let mut dir = PeerDirectory::load_or_default(&path);
    let outcome = match dir.peers.iter_mut().find(|p| p.node_id_hex.eq_ignore_ascii_case(&card.id)) {
        Some(p) if !p.signing_pubkey_hex.eq_ignore_ascii_case(&card.key) => {
            return Err("У этого узла в списке другой ключ. Если ключ действительно сменился — сначала уберите узел, потом добавьте заново.".into());
        }
        Some(p) => {
            p.name = name.or(p.name.take());
            p.addresses = card.addr.clone();
            p.p2p_addresses = card.p2p.clone();
            AddOutcome::Updated
        }
        None => {
            dir.peers.push(TrustedPeer { node_id_hex: card.id.clone(), signing_pubkey_hex: card.key.clone(), name, addresses: card.addr.clone(), p2p_addresses: card.p2p.clone() });
            AddOutcome::Added
        }
    };
    dir.save(&path).map_err(|e| format!("Не удалось сохранить список: {e}"))?;
    apply_live(&dir, None).await;
    tokio::spawn(knock_offline());
    Ok(outcome)
}

/// Убрать узел из доверенных. `false` — такого не было.
pub async fn remove_trusted(id: &str) -> Result<bool, String> {
    if !is_hex64(id) {
        return Err("Неверный номер узла.".into());
    }
    let _g = edit_lock().lock().await;
    let path = default_peer_directory_path();
    let mut dir = PeerDirectory::load_or_default(&path);
    let before = dir.peers.len();
    dir.peers.retain(|p| !p.node_id_hex.eq_ignore_ascii_case(id));
    if dir.peers.len() == before {
        return Ok(false);
    }
    dir.save(&path).map_err(|e| format!("Не удалось сохранить список: {e}"))?;
    let mut raw = [0u8; 32];
    raw.copy_from_slice(&hex::decode(id).unwrap_or_default());
    apply_live(&dir, Some(raw)).await;
    Ok(true)
}

async fn list() -> Json<Value> {
    let dir = PeerDirectory::load_or_default(&default_peer_directory_path());
    let online: HashSet<String> = match transport_cell().get() {
        Some(t) => t.get_peers().await.into_iter().map(|p| p.id.to_hex()).collect(),
        None => Default::default(),
    };
    let peers: Vec<Value> = dir.peers.iter().map(|p| json!({
        "id": p.node_id_hex.to_ascii_lowercase(),
        "name": p.name,
        "addresses": p.addresses,
        "online": online.contains(&p.node_id_hex.to_ascii_lowercase()),
    })).collect();
    Json(json!({"card": my_card().map(|c| encode_card(&c)), "peers": peers}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddBody {
    card: String,
    #[serde(default)]
    name: Option<String>,
}

fn bad(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": msg}))).into_response()
}

async fn add(Json(b): Json<AddBody>) -> Response {
    let card = match decode_card(&b.card) {
        Ok(c) => c,
        Err(e) => return bad(e),
    };
    match add_trusted(&card, b.name.as_deref()).await {
        Ok(o) => Json(json!({"ok": true, "id": card.id, "updated": o == AddOutcome::Updated})).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveBody {
    id: String,
}

async fn remove(Json(b): Json<RemoveBody>) -> Response {
    match remove_trusted(&b.id).await {
        Ok(true) => Json(json!({"ok": true})).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "Такого узла в списке нет."}))).into_response(),
        Err(e) => bad(e),
    }
}

/// Пути страницы настроек (ставятся под проверку входа владельца).
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/peers/trusted", get(list).post(add))
        .route("/api/peers/trusted/remove", post(remove))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the node id that belongs to the test key "cd"*32
    fn good_id() -> String {
        hex::encode(crate::util::types::derive_node_id(&[0xcd; 32]))
    }

    fn card() -> NodeCard {
        NodeCard { id: good_id(), key: "CD".repeat(32), addr: vec!["203.0.113.5:9000".into(), "node.example.org:9000".into(), "[2001:db8::1]:9000".into()], p2p: vec!["203.0.113.5:9001".into()], name: Some("Друг".into()) }
    }

    #[test]
    fn a_card_round_trips_and_is_normalised() {
        let text = encode_card(&card());
        assert!(text.starts_with("YANDI-NODE-1:"));
        let back = decode_card(&format!("  {}\n{} ", &text[..20], &text[20..])).unwrap();
        assert_eq!(back.key, "cd".repeat(32), "keys are kept in lower case");
        assert_eq!((back.id, back.addr, back.p2p, back.name), (card().id, card().addr, card().p2p, card().name));
    }

    #[test]
    fn broken_or_foreign_cards_are_refused_with_a_plain_reason() {
        let enc = |v: Value| format!("{PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string()));
        assert!(decode_card("hello").unwrap_err().contains("не визитка"));
        assert!(decode_card("YANDI-NODE-1:!!!").unwrap_err().contains("повреждена"));
        assert!(decode_card(&enc(json!({"id": "ab".repeat(31), "key": "cd".repeat(32)}))).unwrap_err().contains("номер"));
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "zz".repeat(32)}))).is_err());
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "secret": 1}))).is_err(), "unknown fields are refused");
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "addr": ["no-port"]}))).unwrap_err().contains("адрес"));
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "addr": ["h:0"]}))).is_err());
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "p2p": ["no-port"]}))).is_err(), "chat channel addresses are checked too");
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "p2p": ["a:1", "b:2", "c:3", "d:4", "e:5"]}))).is_err());
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "addr": ["a:1", "b:2", "c:3", "d:4", "e:5"]}))).is_err(), "at most 4 addresses");
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "name": "x".repeat(65)}))).is_err());
        assert!(decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "name": "a\u{7}b"}))).is_err());
        let ok = decode_card(&enc(json!({"id": good_id(), "key": "cd".repeat(32), "name": "я".repeat(64)}))).unwrap();
        assert_eq!(ok.addr, Vec::<String>::new());
        assert!(decode_card(&format!("{PREFIX}{}", "A".repeat(MAX_CARD_CHARS))).unwrap_err().contains("длинная"));
    }

    #[test]
    fn addresses_are_ip_or_host_with_a_port() {
        for good in ["127.0.0.1:29000", "[::1]:9000", "a-b.example:1", "localhost:65535"] {
            assert!(valid_address(good), "{good}");
        }
        for badv in ["", ":9000", "host:", "host:65536", "-a.example:9", "a..b:9", "a b:9", "host:9:9", "127.0.0.1:0", "h_x:9"] {
            assert!(!valid_address(badv), "{badv}");
        }
    }
}
