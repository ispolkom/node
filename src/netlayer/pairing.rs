// src/netlayer/pairing.rs
//! Сопряжение клиента с якорем: что показывается в QR и какие записи о праве хранятся на диске.
//!
//! **Что здесь:**
//! - `PairingPayload` — публичное содержимое QR (anchor_id, открытый X25519, отпечаток TLS, адрес). Секретов в нём нет.
//! - `ClientGrant` — запись якоря о праве клиента: кто (ключ подписи клиента) и до какого срока. Это **не секрет**: по ней нельзя
//!   ни расшифровать трафик, ни войти без закрытого ключа клиента. Рабочих ключей и секретов возобновления на диске нет.
//! - `PairedClientStore` (`paired_clients.json`) и `PairedAnchorStore` (`paired_anchors.json`) — файлы формата версии 2 со строгим
//!   читателем и миграцией со старого формата (`read_*`).
//!
//! **Чего больше нет (docs/RESUME_REDESIGN.md, этап 2а):** возобновления сеанса по сохранённым секретам (пакеты 0xC0/0xC1/0xC2),
//! `session_key_hex`, `resume_secret_hex`. Соединение после обрыва восстанавливается обычным подписанным Hello с новыми эфемерными
//! ключами. Старый файл с секретами не принимается как действующий: при чтении он переписывается без секретных полей, а права
//! клиентов из него **не переносятся** (новое сопряжение выдаёт новое право).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::util::HashId;

/// Версия формата файлов `paired_clients.json` и `paired_anchors.json`.
pub const STORE_VERSION: u32 = 2;

/// Поля старого формата, которые были секретными; в формате v2 их быть не должно ни в каком виде.
pub const LEGACY_SECRET_FIELDS: [&str; 2] = ["resume_secret_hex", "session_key_hex"];

/// 7 дней TTL по умолчанию.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 7 * 24 * 3600;

/// Один писатель или мигратор за раз: чтение–преобразование–запись файла не пересекаются между потоками этого процесса.
static STORE_LOCK: Mutex<()> = Mutex::new(());

// ----------------- Payload (QR) -----------------

/// То, что anchor показывает в QR-коде. Mobile сканит → парсит → сохраняет в свой store.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingPayload {
    pub anchor_id: HashId,
    /// Hex-encoded X25519 public key anchor'а (открытый, не секрет).
    pub anchor_x25519_hex: String,
    /// SHA-256 fingerprint TLS-сертификата anchor'а (тот же, что `TlsIdentity::fingerprint_hex`).
    pub fingerprint_hex: String,
    /// `wss://host:port/` — куда мобилка коннектится.
    pub anchor_url: String,
}

impl PairingPayload {
    /// Сериализовать в compact JSON (то что попадёт в QR).
    pub fn to_qr_string(&self) -> String {
        serde_json::to_string(self).expect("serialize PairingPayload")
    }

    pub fn from_qr_string(s: &str) -> Result<Self> {
        serde_json::from_str::<Self>(s).context("parse PairingPayload from QR")
    }
}

// ----------------- Grant -----------------

/// Запись якоря о праве клиента. Секретного материала не содержит.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClientGrant {
    /// Случайный номер права (32 hex-знака); служит только для различения записей и сообщений, не секрет.
    pub session_id_hex: String,
    pub expires_at: u64,
}

impl ClientGrant {
    pub fn new(ttl_secs: u64) -> Self {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut b);
        Self { session_id_hex: hex::encode(b), expires_at: now_secs().saturating_add(ttl_secs) }
    }

    pub fn is_expired(&self) -> bool {
        now_secs() > self.expires_at
    }

    pub fn refresh(&mut self, ttl_secs: u64) {
        self.expires_at = now_secs().saturating_add(ttl_secs);
    }
}

// ----------------- Чтение файлов со строгим результатом -----------------

/// Чем закончилось чтение файла хранилища. Ошибки не превращаются в «пустое состояние»: повреждённый или нечитаемый файл не
/// перезаписывается молча.
#[derive(Debug)]
pub enum StoreRead<T> {
    /// Файла нет — чистое начало.
    Missing,
    /// Формат v2, прочитан строго.
    Current(T),
    /// Старый формат (v1, возможно с секретами): содержимое очищено от секретов, права клиентов не перенесены.
    Migrated(T),
    /// Версия выше известной: файл не трогаем.
    Unsupported(u32),
    /// Не разобран как ни один из известных форматов.
    Corrupt(String),
    /// Ошибка ввода-вывода или прав доступа.
    Io(String),
}

fn read_text(path: &Path) -> std::result::Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{e}")),
        Ok(m) if m.file_type().is_symlink() || !m.is_file() => return Err("не обычный файл".into()),
        Ok(m) if m.len() > 4 * 1024 * 1024 => return Err("файл слишком велик".into()),
        Ok(_) => {}
    }
    fs::read_to_string(path).map(Some).map_err(|e| e.to_string())
}

/// Есть ли в тексте имена секретных полей старого формата (поиск по тексту, а не по разобранному дереву: старые номера сеансов — 128-битные
/// числа, которые общее дерево JSON не вмещает).
fn has_legacy_secret(text: &str) -> bool {
    LEGACY_SECRET_FIELDS.iter().any(|f| text.contains(f))
}

/// Только версия формата; остальное не разбирается.
fn probe_version(text: &str) -> std::result::Result<Option<u32>, String> {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        version: Option<u32>,
    }
    serde_json::from_str::<Probe>(text).map(|p| p.version).map_err(|e| format!("не JSON-объект: {e}"))
}

#[derive(Deserialize)]
struct OldAnchorEntry {
    payload: PairingPayload,
    #[serde(default)]
    preference: u32,
}

#[derive(Deserialize)]
struct OldAnchors {
    anchors: Vec<OldAnchorEntry>,
}

#[derive(Deserialize)]
struct OldClients {
    #[serde(default)]
    #[allow(dead_code)]
    clients: HashMap<String, serde::de::IgnoredAny>,
}

// ----------------- Stores -----------------

/// На anchor-стороне: кому выдано право. Ключ — hex public_key клиента (Ed25519 32B).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedClientStore {
    pub version: u32,
    pub clients: HashMap<String, ClientGrant>,
    /// Файл не удалось прочитать как известный формат: хранилище пустое в памяти и **записывать его нельзя**, чтобы не затереть файл.
    #[serde(skip)]
    pub blocked: Option<String>,
}

impl Default for PairedClientStore {
    fn default() -> Self {
        Self { version: STORE_VERSION, clients: HashMap::new(), blocked: None }
    }
}

impl PairedClientStore {
    /// Строгое чтение. Старый формат (v1): права клиентов НЕ переносятся (новое сопряжение выдаст новое право), файл переписывается без секретов.
    pub fn read(path: &Path) -> StoreRead<Self> {
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let text = match read_text(path) {
            Ok(None) => return StoreRead::Missing,
            Ok(Some(t)) => t,
            Err(e) => return StoreRead::Io(e),
        };
        let ver = match probe_version(&text) {
            Ok(x) => x,
            Err(e) => return StoreRead::Corrupt(e),
        };
        match ver {
            Some(STORE_VERSION) => {
                if has_legacy_secret(&text) {
                    return StoreRead::Corrupt("в файле формата v2 найдены поля старого формата с секретами".into());
                }
                match serde_json::from_str::<Self>(&text) {
                    Ok(s) => StoreRead::Current(s),
                    Err(e) => StoreRead::Corrupt(format!("схема v2: {e}")),
                }
            }
            Some(v) => StoreRead::Unsupported(v),
            None => match serde_json::from_str::<OldClients>(&text) {
                // старый формат: права клиентов не переносятся
                Ok(_) => StoreRead::Migrated(Self::default()),
                Err(e) => StoreRead::Corrupt(format!("схема v1: {e}")),
            },
        }
    }

    /// Загрузка для работы узла: `Migrated` сразу переписывает файл без секретов; повреждённое/нечитаемое/будущее — пустое хранилище с
    /// `blocked` (файл не трогается), причина — в журнал без значений.
    pub fn load_or_default(path: &Path) -> Self {
        match Self::read(path) {
            StoreRead::Missing => Self::default(),
            StoreRead::Current(s) => s,
            StoreRead::Migrated(s) => {
                match s.save(path) {
                    Ok(()) => eprintln!("[pairing] {}: старый формат с секретами заменён пустым списком формата v2 (права клиентов не перенесены, нужно новое сопряжение)", path.display()),
                    Err(e) => eprintln!("[pairing] {}: старый формат не удалось заменить ({e}); файл не используется", path.display()),
                }
                s
            }
            StoreRead::Unsupported(v) => Self::blocked_with(format!("версия формата {v} выше известной")),
            StoreRead::Corrupt(e) => Self::blocked_with(format!("файл повреждён: {e}")),
            StoreRead::Io(e) => Self::blocked_with(format!("файл не читается: {e}")),
        }
    }

    fn blocked_with(reason: String) -> Self {
        Self { blocked: Some(reason), ..Self::default() }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(why) = &self.blocked {
            anyhow::bail!("хранилище не записывается: {why}");
        }
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }
        let s = serde_json::to_string_pretty(self).context("serialize PairedClientStore")?;
        crate::util::private_file::write_private(path, s.as_bytes()).with_context(|| format!("write {:?}", path))?;
        Ok(())
    }

    pub fn issue(&mut self, client_pubkey_hex: String, ttl_secs: u64) -> ClientGrant {
        let g = ClientGrant::new(ttl_secs);
        self.clients.insert(client_pubkey_hex, g.clone());
        g
    }

    pub fn get(&self, client_pubkey_hex: &str) -> Option<&ClientGrant> {
        self.clients.get(client_pubkey_hex)
    }

    pub fn refresh(&mut self, client_pubkey_hex: &str, ttl_secs: u64) -> bool {
        if let Some(t) = self.clients.get_mut(client_pubkey_hex) {
            t.refresh(ttl_secs);
            true
        } else {
            false
        }
    }

    pub fn revoke(&mut self, client_pubkey_hex: &str) -> bool {
        self.clients.remove(client_pubkey_hex).is_some()
    }

    pub fn gc_expired(&mut self) -> usize {
        let before = self.clients.len();
        self.clients.retain(|_, t| !t.is_expired());
        before - self.clients.len()
    }
}

/// На клиентской стороне: наши якоря в порядке предпочтения. Секретов нет: только то, что было в QR, и порядок.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedAnchorStore {
    pub version: u32,
    pub anchors: Vec<PairedAnchorEntry>,
    #[serde(skip)]
    pub blocked: Option<String>,
}

impl Default for PairedAnchorStore {
    fn default() -> Self {
        Self { version: STORE_VERSION, anchors: Vec::new(), blocked: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairedAnchorEntry {
    pub payload: PairingPayload,
    /// Преферанс (меньше — выше в очереди). Не обязательно уникальный.
    pub preference: u32,
}

impl PairedAnchorStore {
    /// Строгое чтение. Старый формат: сохраняются `payload` и `preference` (публичные данные QR и порядок), поле `session` отбрасывается.
    pub fn read(path: &Path) -> StoreRead<Self> {
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let text = match read_text(path) {
            Ok(None) => return StoreRead::Missing,
            Ok(Some(t)) => t,
            Err(e) => return StoreRead::Io(e),
        };
        let ver = match probe_version(&text) {
            Ok(x) => x,
            Err(e) => return StoreRead::Corrupt(e),
        };
        match ver {
            Some(STORE_VERSION) => {
                if has_legacy_secret(&text) {
                    return StoreRead::Corrupt("в файле формата v2 найдены поля старого формата с секретами".into());
                }
                match serde_json::from_str::<Self>(&text) {
                    Ok(s) => StoreRead::Current(s),
                    Err(e) => StoreRead::Corrupt(format!("схема v2: {e}")),
                }
            }
            Some(v) => StoreRead::Unsupported(v),
            None => match serde_json::from_str::<OldAnchors>(&text) {
                Ok(o) => StoreRead::Migrated(Self {
                    version: STORE_VERSION,
                    anchors: o.anchors.into_iter().map(|e| PairedAnchorEntry { payload: e.payload, preference: e.preference }).collect(),
                    blocked: None,
                }),
                Err(e) => StoreRead::Corrupt(format!("схема v1: {e}")),
            },
        }
    }

    pub fn load_or_default(path: &Path) -> Self {
        match Self::read(path) {
            StoreRead::Missing => Self::default(),
            StoreRead::Current(s) => s,
            StoreRead::Migrated(s) => {
                match s.save(path) {
                    Ok(()) => eprintln!("[pairing] {}: старый формат заменён форматом v2 без секретов (адреса и порядок якорей сохранены; нужно новое сопряжение для права на якоре)", path.display()),
                    Err(e) => eprintln!("[pairing] {}: старый формат не удалось заменить ({e}); файл не используется", path.display()),
                }
                s
            }
            StoreRead::Unsupported(v) => Self { blocked: Some(format!("версия формата {v} выше известной")), ..Self::default() },
            StoreRead::Corrupt(e) => Self { blocked: Some(format!("файл повреждён: {e}")), ..Self::default() },
            StoreRead::Io(e) => Self { blocked: Some(format!("файл не читается: {e}")), ..Self::default() },
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(why) = &self.blocked {
            anyhow::bail!("хранилище не записывается: {why}");
        }
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }
        let s = serde_json::to_string_pretty(self).context("serialize PairedAnchorStore")?;
        crate::util::private_file::write_private(path, s.as_bytes()).with_context(|| format!("write {:?}", path))?;
        Ok(())
    }

    pub fn add_or_update(&mut self, payload: PairingPayload, preference: u32) {
        if let Some(existing) = self.anchors.iter_mut().find(|e| e.payload.anchor_id == payload.anchor_id) {
            existing.payload = payload;
            existing.preference = preference;
        } else {
            self.anchors.push(PairedAnchorEntry { payload, preference });
        }
        self.anchors.sort_by_key(|e| e.preference);
    }

    pub fn primary(&self) -> Option<&PairedAnchorEntry> {
        self.anchors.first()
    }
}

/// Очистка текста файла хранилища от секретов старого формата для резервной копии и восстановления: возвращает текст формата v2 без
/// секретов или `None`, если файл нельзя разобрать (такой файл в копию не кладётся, чтобы в неё не попали остатки секретов).
pub fn sanitized_store_text(kind: StoreKind, bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let ver = probe_version(text).ok()?;
    match (kind, ver) {
        (StoreKind::Clients, Some(STORE_VERSION)) if !has_legacy_secret(text) => serde_json::from_str::<PairedClientStore>(text).ok().map(|_| text.to_string()),
        (StoreKind::Clients, None) => {
            serde_json::from_str::<OldClients>(text).ok()?;
            serde_json::to_string_pretty(&PairedClientStore::default()).ok()
        }
        (StoreKind::Anchors, Some(STORE_VERSION)) if !has_legacy_secret(text) => serde_json::from_str::<PairedAnchorStore>(text).ok().map(|_| text.to_string()),
        (StoreKind::Anchors, None) => {
            let o: OldAnchors = serde_json::from_str(text).ok()?;
            let s = PairedAnchorStore {
                version: STORE_VERSION,
                anchors: o.anchors.into_iter().map(|e| PairedAnchorEntry { payload: e.payload, preference: e.preference }).collect(),
                blocked: None,
            };
            serde_json::to_string_pretty(&s).ok()
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    Clients,
    Anchors,
}

pub fn default_paired_clients_path() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".yandi").join("paired_clients.json")
}

pub fn default_paired_anchors_path() -> PathBuf {
    // Hardening Step 6: CLI `--anchor-store <path>` имеет приоритет.
    if let Some(p) = crate::netlayer::packet::anchor_store_override() {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".yandi").join("paired_anchors.json")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ----------------- Tests -----------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Известные «секреты» старого формата: тесты проверяют, что их нет ни в новых файлах, ни в копиях.
    const OLD_RESUME_SECRET: &str = "11aa22bb33cc44dd55ee66ff77008899aabbccddeeff00112233445566778899";
    const OLD_SESSION_KEY: &str = "99887766554433221100ffeeddccbbaa99887766554433221100ffeeddccbbaa";
    const BIG_SESSION_ID: &str = "340282366920938463463374607431768211455"; // u128::MAX: старые номера не помещались в общее дерево JSON

    fn old_clients_json() -> String {
        format!(
            r#"{{"clients":{{"{pk}":{{"session_id":{BIG_SESSION_ID},"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":4102444800,"session_key_hex":"{OLD_SESSION_KEY}","node_id_hex":"{pk}"}}}}}}"#,
            pk = "ab".repeat(32)
        )
    }

    fn payload() -> PairingPayload {
        PairingPayload {
            anchor_id: HashId([0xAB; 32]),
            anchor_x25519_hex: hex::encode([0x33u8; 32]),
            fingerprint_hex: "abcdef".repeat(10) + "abcd",
            anchor_url: "wss://anchor.example:443/".into(),
        }
    }

    fn old_anchors_json() -> String {
        let p = serde_json::to_string(&payload()).unwrap();
        format!(
            r#"{{"anchors":[{{"payload":{p},"session":{{"session_id":{BIG_SESSION_ID},"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":4102444800,"session_key_hex":"{OLD_SESSION_KEY}"}},"preference":3}}]}}"#
        )
    }

    fn no_old_secrets(text: &str) {
        for needle in [OLD_RESUME_SECRET, OLD_SESSION_KEY, "resume_secret_hex", "session_key_hex", BIG_SESSION_ID] {
            assert!(!text.contains(needle), "в файле остался {needle}");
        }
    }

    #[test]
    fn pairing_payload_qr_roundtrip() {
        let p = payload();
        let back = PairingPayload::from_qr_string(&p.to_qr_string()).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn a_grant_is_unique_not_a_secret_and_expires() {
        let a = ClientGrant::new(100);
        let b = ClientGrant::new(100);
        assert_ne!(a.session_id_hex, b.session_id_hex);
        assert!(!a.is_expired());
        let mut t = ClientGrant::new(60);
        t.expires_at = 1;
        assert!(t.is_expired());
        t.refresh(60);
        assert!(!t.is_expired());
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("session_id_hex") && json.contains("expires_at") && !json.contains("secret") && !json.contains("key"), "{json}");
    }

    #[test]
    fn the_client_store_roundtrips_in_v2_without_any_secret_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        let mut st = PairedClientStore::default();
        let g = st.issue("cd".repeat(32), 3600);
        st.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"version\": 2") && !has_legacy_secret(&text), "{text}");
        match PairedClientStore::read(&path) {
            StoreRead::Current(back) => assert_eq!(back.get(&"cd".repeat(32)), Some(&g)),
            other => panic!("{other:?}"),
        }
        assert!(st.revoke(&"cd".repeat(32)) && !st.revoke(&"cd".repeat(32)));
        let mut e = PairedClientStore::default();
        e.clients.insert("x".into(), ClientGrant { session_id_hex: "0".repeat(32), expires_at: 1 });
        assert_eq!(e.gc_expired(), 1);
    }

    #[test]
    fn an_old_clients_file_is_rewritten_without_secrets_and_its_rights_are_not_carried_over() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        fs::write(&path, old_clients_json()).unwrap();
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Migrated(_)));
        let st = PairedClientStore::load_or_default(&path);
        assert!(st.clients.is_empty(), "право из старого файла новым не становится");
        assert!(st.blocked.is_none());
        let text = fs::read_to_string(&path).unwrap();
        no_old_secrets(&text);
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Current(_)), "после переписывания файл читается как v2");
    }

    #[test]
    fn an_old_anchors_file_keeps_the_public_data_and_order_but_loses_the_session() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_anchors.json");
        fs::write(&path, old_anchors_json()).unwrap();
        let st = PairedAnchorStore::load_or_default(&path);
        assert_eq!(st.anchors.len(), 1);
        assert_eq!(st.anchors[0].payload, payload());
        assert_eq!(st.anchors[0].preference, 3);
        no_old_secrets(&fs::read_to_string(&path).unwrap());
    }

    #[test]
    fn a_v2_file_with_old_secret_fields_is_refused_and_left_untouched() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        let text = format!(r#"{{"version":2,"clients":{{"{pk}":{{"session_id_hex":"{z}","expires_at":9,"resume_secret_hex":"{OLD_RESUME_SECRET}"}}}}}}"#, pk = "ab".repeat(32), z = "0".repeat(32));
        fs::write(&path, &text).unwrap();
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Corrupt(_)), "старое поле нельзя прочитать как новое");
        let st = PairedClientStore::load_or_default(&path);
        assert!(st.blocked.is_some() && st.clients.is_empty());
        assert!(st.save(&path).is_err(), "заблокированное хранилище не записывается");
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "файл не изменён");
    }

    #[test]
    fn corrupt_future_and_unreadable_files_are_not_replaced_by_an_empty_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_anchors.json");
        fs::write(&path, b"{not json").unwrap();
        assert!(matches!(PairedAnchorStore::read(&path), StoreRead::Corrupt(_)));
        let st = PairedAnchorStore::load_or_default(&path);
        assert!(st.blocked.is_some() && st.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{not json");
        fs::write(&path, br#"{"version":99,"anchors":[]}"#).unwrap();
        assert!(matches!(PairedAnchorStore::read(&path), StoreRead::Unsupported(99)));
        assert!(PairedAnchorStore::load_or_default(&path).blocked.is_some());
        // каталог на месте файла: ошибка ввода-вывода, а не «пусто»
        let d = dir.path().join("dir.json");
        fs::create_dir(&d).unwrap();
        assert!(matches!(PairedAnchorStore::read(&d), StoreRead::Io(_)));
        assert!(matches!(PairedAnchorStore::read(&dir.path().join("none.json")), StoreRead::Missing));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_not_followed() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.json");
        fs::write(&real, old_clients_json()).unwrap();
        let link = dir.path().join("paired_clients.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(PairedClientStore::read(&link), StoreRead::Io(_)));
        assert_eq!(fs::read_to_string(&real).unwrap(), old_clients_json(), "цель ссылки не тронута");
    }

    #[test]
    fn backups_and_restores_sanitize_both_files_and_drop_what_cannot_be_read() {
        let c = sanitized_store_text(StoreKind::Clients, old_clients_json().as_bytes()).unwrap();
        no_old_secrets(&c);
        let a = sanitized_store_text(StoreKind::Anchors, old_anchors_json().as_bytes()).unwrap();
        no_old_secrets(&a);
        assert!(a.contains("\"preference\": 3"));
        // уже чистый v2 проходит как есть
        let v2 = serde_json::to_string_pretty(&PairedClientStore::default()).unwrap();
        assert_eq!(sanitized_store_text(StoreKind::Clients, v2.as_bytes()).unwrap(), v2);
        // мусор, будущая версия, v2 с секретным полем — в копию не попадают
        assert!(sanitized_store_text(StoreKind::Clients, b"\xff\xfe").is_none());
        assert!(sanitized_store_text(StoreKind::Anchors, b"{oops").is_none());
        assert!(sanitized_store_text(StoreKind::Clients, br#"{"version":9,"clients":{}}"#).is_none());
        assert!(sanitized_store_text(StoreKind::Clients, format!(r#"{{"version":2,"clients":{{}},"resume_secret_hex":"{OLD_RESUME_SECRET}"}}"#).as_bytes()).is_none());
    }

    /// Страж по исходникам: механизма возобновления и хранения рабочего ключа в коде больше нет.
    #[test]
    fn the_resume_mechanism_is_gone_from_the_sources() {
        let sources = [
            ("transport.rs", include_str!("transport.rs")),
            ("server.rs", include_str!("../web/server.rs")),
            ("node_backup.rs", include_str!("../node_backup.rs")),
            ("pairing.rs (до тестов)", include_str!("pairing.rs").split("#[cfg(test)]\nmod tests {").next().unwrap()),
        ];
        for (name, text) in sources {
            for banned in ["PKT_RESUME", "PKT_SESSION_ISSUE", "compute_resume_mac", "derive_resume_key", "new_with_session_key", "resume_secret()", "session_key()", "SessionToken", "handle_resume_packet", "handle_session_issue", "encode_resume", "decode_resume"] {
                assert!(!text.contains(banned), "{name}: осталось {banned}");
            }
        }
    }
}
