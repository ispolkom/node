//! Bootstrap: как новый узел находит вход в сеть.
//!
//! Узел при запуске берёт **подписанный список входных узлов** (`bootstrap/bootstrap.json` в репозитории проекта, по ссылке или из
//! копии на диске), проверяет подпись закреплённым ключом проекта и подключается к узлам из списка; дальше сеть строится сама
//! (карточки узлов, обмен соседями). Сам список — не секрет, но его нельзя подменить:
//!
//! * подпись Ed25519 охватывает весь документ (изменить адрес или ключ входного узла, не зная ключа подписи, нельзя);
//! * `sequence` растёт с каждой публикацией — старый список (откат) отвергается, если на диске уже есть новее;
//! * `expires` — после этого срока список считается устаревшим и не применяется (остаются сохранённая копия и найденные соседи);
//! * в списке у каждого входного узла записан его **ключ**: подключившись, узел проверяет, что по адресу отвечает именно он
//!   (подмена входного узла на другой адрес не проходит);
//! * ключей подписи может быть несколько (`SIGNER_KEYS`), чтобы ключ можно было заменить, не ломая старые версии программы.
//!
//! Что здесь **не** решается: тот, кто может заблокировать GitHub (или другой источник списка), закроет вход в сеть новичку —
//! поэтому есть копия на диске, второй адрес (`YANDI_BOOTSTRAP_URL`), приглашения со своими входами и соседи, найденные раньше.
//! Источник видит адрес каждого, кто его запрашивает.
pub mod tool;

use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Закреплённые ключи подписи списка (hex). Заменить на свои при форке проекта: `yandi bootstrap keygen`.
pub const SIGNER_KEYS: &[&str] = &[include_str!("../../bootstrap/signer.pub")];

/// Откуда брать список по умолчанию (переопределяется `YANDI_BOOTSTRAP_URL`, можно несколько через запятую).
pub const DEFAULT_URLS: &[&str] = &["https://raw.githubusercontent.com/ispolkom/node/main/bootstrap/bootstrap.json"];

/// Сколько секунд ждать ответа источника.
pub const FETCH_TIMEOUT_SECS: u64 = 10;
/// Не больше стольких входных узлов в списке (защита от раздутого файла).
pub const MAX_ENTRIES: usize = 64;
/// Не больше столько байт принимаем от источника.
pub const MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// номер узла (64 hex)
    pub id: String,
    /// ключ подписи узла (64 hex): им проверяется, что по адресу отвечает именно он
    pub key: String,
    /// адреса основной связи `адрес:порт` (до 4)
    pub addr: Vec<String>,
    /// страна (ISO-3166 alpha-2), если известна
    #[serde(default)]
    pub region: Option<String>,
    /// чем узел помогает: `entry`, `relay`, `exit`
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Doc {
    /// версия формата
    pub format: u32,
    /// имя сети
    pub network: String,
    /// когда выпущен (unix, секунды)
    pub issued: u64,
    /// до какого времени действует (unix, секунды)
    pub expires: u64,
    /// номер выпуска: растёт при каждой публикации
    pub sequence: u64,
    pub entries: Vec<Entry>,
    /// ключ, которым подписано (hex) — должен быть среди закреплённых
    pub signer: String,
    /// подпись Ed25519 (128 hex) всего остального
    pub signature: String,
}

#[derive(Debug, PartialEq)]
pub enum BootstrapError {
    Parse,
    Format,
    UnknownSigner,
    BadSignature,
    Expired,
    Rollback,
    Shape(&'static str),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::Parse => write!(f, "файл не читается"),
            BootstrapError::Format => write!(f, "неизвестная версия формата"),
            BootstrapError::UnknownSigner => write!(f, "подписан не закреплённым ключом"),
            BootstrapError::BadSignature => write!(f, "подпись не сходится"),
            BootstrapError::Expired => write!(f, "список устарел"),
            BootstrapError::Rollback => write!(f, "список старше уже известного"),
            BootstrapError::Shape(w) => write!(f, "неверное содержимое: {w}"),
        }
    }
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| c.is_ascii_hexdigit())
}

impl Doc {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut c = self.clone();
        c.signature = String::new();
        let mut b = b"yandi-bootstrap-v1\0".to_vec();
        b.extend_from_slice(&serde_json::to_vec(&c).unwrap_or_default());
        b
    }

    /// Подписать (для владельца списка): ключ записывается в `signer`.
    pub fn sign(mut self, key: &SigningKey) -> Doc {
        self.signer = hex::encode(key.verifying_key().to_bytes());
        self.signature = String::new();
        self.signature = hex::encode(key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    /// Форма: размеры, адреса, ключи. Не зависит от времени и подписи.
    pub fn check_shape(&self) -> Result<(), BootstrapError> {
        if self.format != 1 {
            return Err(BootstrapError::Format);
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(BootstrapError::Shape("слишком много узлов"));
        }
        if self.network.is_empty() || self.network.len() > 64 {
            return Err(BootstrapError::Shape("имя сети"));
        }
        for e in &self.entries {
            if !is_hex(&e.id, 64) || !is_hex(&e.key, 64) {
                return Err(BootstrapError::Shape("номер или ключ узла"));
            }
            if e.addr.is_empty() || e.addr.len() > 4 || e.addr.iter().any(|a| a.len() > 262 || a.parse::<std::net::SocketAddr>().is_err()) {
                return Err(BootstrapError::Shape("адрес узла"));
            }
            if let Some(r) = &e.region {
                if r.len() != 2 || !r.bytes().all(|b| b.is_ascii_uppercase()) {
                    return Err(BootstrapError::Shape("страна"));
                }
            }
            if e.roles.len() > 8 || e.roles.iter().any(|r| !matches!(r.as_str(), "entry" | "relay" | "exit")) {
                return Err(BootstrapError::Shape("роль"));
            }
        }
        Ok(())
    }

    /// Полная проверка: форма, закреплённый ключ подписи, подпись, срок, откат (`known_sequence` — уже известный выпуск).
    pub fn verify(&self, signers: &[&str], now: u64, known_sequence: Option<u64>) -> Result<(), BootstrapError> {
        self.check_shape()?;
        if !signers.iter().any(|k| k.trim().eq_ignore_ascii_case(&self.signer)) {
            return Err(BootstrapError::UnknownSigner);
        }
        let key = hex::decode(&self.signer).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or(BootstrapError::UnknownSigner)?;
        let sig = hex::decode(&self.signature).ok().and_then(|b| <[u8; 64]>::try_from(b).ok()).map(|b| Signature::from_bytes(&b)).ok_or(BootstrapError::BadSignature)?;
        key.verify(&self.signing_bytes(), &sig).map_err(|_| BootstrapError::BadSignature)?;
        if self.expires <= now {
            return Err(BootstrapError::Expired);
        }
        if known_sequence.map(|k| self.sequence < k).unwrap_or(false) {
            return Err(BootstrapError::Rollback);
        }
        Ok(())
    }
}

pub fn parse(bytes: &[u8]) -> Result<Doc, BootstrapError> {
    if bytes.len() > MAX_BYTES {
        return Err(BootstrapError::Shape("файл слишком большой"));
    }
    serde_json::from_slice(bytes).map_err(|_| BootstrapError::Parse)
}

fn cache_path() -> PathBuf {
    crate::util::data_dir::data_dir().join("bootstrap_cache.json")
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Сохранённая копия (если подпись сходится и список не устарел).
pub fn load_cached(signers: &[&str], now: u64) -> Option<Doc> {
    let bytes = std::fs::read(cache_path()).ok()?;
    let d = parse(&bytes).ok()?;
    d.verify(signers, now, None).ok()?;
    Some(d)
}

fn save_cache(d: &Doc) {
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // write a temporary file and rename it: a stop in the middle never leaves a torn cache
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(d).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn urls() -> Vec<String> {
    match std::env::var("YANDI_BOOTSTRAP_URL") {
        Ok(v) if !v.trim().is_empty() => v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => DEFAULT_URLS.iter().map(|s| s.to_string()).collect(),
    }
}

/// Скачать список из первого ответившего источника, проверить и сохранить. Возвращает действующий список: свежий или, если
/// скачать не удалось, сохранённый на диске.
pub async fn fetch(signers: &[&str]) -> Option<Doc> {
    let t = now();
    let known = load_cached(signers, 0).map(|d| d.sequence);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(FETCH_TIMEOUT_SECS)).build().ok()?;
    for url in urls() {
        let Ok(resp) = client.get(&url).send().await else { continue };
        if !resp.status().is_success() {
            continue;
        }
        if resp.content_length().map_or(false, |n| n as usize > MAX_BYTES) {
            continue;
        }
        let mut resp = resp;
        let mut bytes: Vec<u8> = Vec::new();
        let mut ok = true;
        loop {
            match resp.chunk().await {
                Ok(Some(c)) => {
                    if bytes.len() + c.len() > MAX_BYTES {
                        ok = false;
                        break;
                    }
                    bytes.extend_from_slice(&c);
                }
                Ok(None) => break,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        match parse(&bytes).and_then(|d| d.verify(signers, t, known).map(|_| d)) {
            Ok(d) => {
                println!("[bootstrap] список входных узлов получен ({} узлов, выпуск {})", d.entries.len(), d.sequence);
                save_cache(&d);
                return Some(d);
            }
            Err(e) => eprintln!("[bootstrap] {url}: {e}"),
        }
    }
    let cached = load_cached(signers, t);
    if cached.is_some() {
        println!("[bootstrap] источник недоступен — использую сохранённую копию списка");
    }
    cached
}

/// Входные узлы в виде старой настройки узла (адрес + закреплённый ключ).
pub fn to_config(d: &Doc) -> crate::netlayer::bootstrap::BootstrapConfig {
    let mut c = crate::netlayer::bootstrap::BootstrapConfig::default();
    c.comment = format!("подписанный список «{}», выпуск {}", d.network, d.sequence);
    c.version = d.sequence.to_string();
    for (i, e) in d.entries.iter().enumerate() {
        c.add_node(crate::netlayer::bootstrap::BootstrapNode {
            name: format!("{}-{}", d.network, i + 1),
            address: e.addr[0].clone(),
            jurisdiction: e.region.clone(),
            cluster: Some("signed".into()),
            enabled: true,
            role: e.roles.first().cloned(),
            ed25519_fingerprint: Some(e.key.clone()),
        });
    }
    c
}

/// Настройка для запуска: подписанный список (скачанный или сохранённый) плюс **локальный** файл владельца `nodes/bootstrap.json`
/// (его владелец правит сам, подписи не требует).
pub async fn effective_config(local: &Path) -> crate::netlayer::bootstrap::BootstrapConfig {
    let mut cfg = crate::netlayer::bootstrap::BootstrapConfig::load_from_file(local).unwrap_or_default();
    if let Some(d) = fetch(SIGNER_KEYS).await {
        let signed = to_config(&d);
        for n in signed.nodes {
            // не дублируем адрес, который владелец уже указал сам
            if !cfg.nodes.iter().any(|x| x.address == n.address) {
                cfg.add_node(n);
            }
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    fn signer(n: u8) -> String {
        hex::encode(key(n).verifying_key().to_bytes())
    }
    fn entry(n: u8) -> Entry {
        Entry { id: hex::encode([n; 32]), key: hex::encode(key(n).verifying_key().to_bytes()), addr: vec![format!("203.0.113.{n}:9000")], region: Some("NL".into()), roles: vec!["entry".into(), "relay".into()] }
    }
    fn doc(seq: u64) -> Doc {
        Doc { format: 1, network: "yandi".into(), issued: NOW, expires: NOW + 86_400 * 30, sequence: seq, entries: vec![entry(1), entry(2)], signer: String::new(), signature: String::new() }.sign(&key(99))
    }

    #[test]
    fn a_signed_list_verifies_and_every_kind_of_tampering_is_refused() {
        let d = doc(5);
        let s = signer(99);
        assert_eq!(d.verify(&[&s], NOW, None), Ok(()));
        // подмена адреса, ключа входного узла, срока, номера выпуска, роли
        for tamper in [
            |d: &mut Doc| d.entries[0].addr = vec!["198.51.100.9:9000".into()],
            |d: &mut Doc| d.entries[0].key = hex::encode([7u8; 32]),
            |d: &mut Doc| d.expires += 1,
            |d: &mut Doc| d.sequence += 1,
            |d: &mut Doc| d.entries[1].roles = vec!["exit".into()],
            |d: &mut Doc| d.entries.push(entry(3)),
        ] {
            let mut t = d.clone();
            tamper(&mut t);
            assert_eq!(t.verify(&[&s], NOW, None), Err(BootstrapError::BadSignature));
        }
        // подписан чужим ключом (подставлен весь файл целиком)
        let forged = Doc { signer: String::new(), signature: String::new(), ..d.clone() }.sign(&key(7));
        assert_eq!(forged.verify(&[&s], NOW, None), Err(BootstrapError::UnknownSigner));
        // подпись другого человека, но в поле signer — закреплённый ключ
        let mut liar = forged.clone();
        liar.signer = s.clone();
        assert_eq!(liar.verify(&[&s], NOW, None), Err(BootstrapError::BadSignature));
    }

    #[test]
    fn stale_and_rolled_back_lists_are_refused_but_a_newer_or_equal_one_is_taken() {
        let s = signer(99);
        let d = doc(5);
        assert_eq!(d.verify(&[&s], NOW + 86_400 * 30, None), Err(BootstrapError::Expired));
        assert_eq!(d.verify(&[&s], NOW, Some(6)), Err(BootstrapError::Rollback), "older than what we already know");
        assert_eq!(d.verify(&[&s], NOW, Some(5)), Ok(()));
        assert_eq!(d.verify(&[&s], NOW, Some(4)), Ok(()));
    }

    #[test]
    fn a_key_can_be_replaced_because_several_signers_are_pinned() {
        let (old, new) = (signer(98), signer(99));
        assert_eq!(doc(1).verify(&[&old, &new], NOW, None), Ok(()), "signed by the second pinned key");
        assert_eq!(doc(1).verify(&[&old], NOW, None), Err(BootstrapError::UnknownSigner));
    }

    #[test]
    fn hostile_files_do_not_get_in() {
        let s = signer(99);
        let mut d = doc(1);
        d.entries = (0..(MAX_ENTRIES as u8 + 1)).map(entry).collect();
        let d = d.sign(&key(99));
        assert_eq!(d.verify(&[&s], NOW, None), Err(BootstrapError::Shape("слишком много узлов")));
        for bad in [
            |e: &mut Entry| e.addr = vec!["не адрес".into()],
            |e: &mut Entry| e.addr = vec![],
            |e: &mut Entry| e.id = "zz".into(),
            |e: &mut Entry| e.region = Some("nl".into()),
            |e: &mut Entry| e.roles = vec!["boss".into()],
        ] {
            let mut x = doc(1);
            bad(&mut x.entries[0]);
            let x = x.sign(&key(99));
            assert!(matches!(x.verify(&[&s], NOW, None), Err(BootstrapError::Shape(_))));
        }
        assert_eq!(parse(b"{not json").unwrap_err(), BootstrapError::Parse);
        assert!(parse(&vec![b' '; MAX_BYTES + 1]).is_err());
        let mut v = doc(1);
        v.format = 2;
        assert_eq!(v.sign(&key(99)).verify(&[&s], NOW, None), Err(BootstrapError::Format));
        // лишнее поле в файле — отказ (не принимаем то, чего не знаем)
        let extra = serde_json::to_string(&doc(1)).unwrap().replace("\"format\":1", "\"format\":1,\"city\":\"Moscow\"");
        assert_eq!(parse(extra.as_bytes()).unwrap_err(), BootstrapError::Parse);
    }

    #[test]
    fn the_list_turns_into_node_settings_with_pinned_keys() {
        let c = to_config(&doc(3));
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.nodes[0].address, "203.0.113.1:9000");
        assert_eq!(c.nodes[0].ed25519_fingerprint.as_deref(), Some(hex::encode(key(1).verifying_key().to_bytes()).as_str()));
        assert_eq!(c.fingerprint_map().len(), 2, "the node pins both entries by address");
    }

    #[test]
    fn the_shipped_list_is_signed_by_the_pinned_key() {
        // файл из репозитория: проверяется «вживую», чтобы нельзя было закоммитить список с неверной подписью
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/bootstrap/bootstrap.json")).expect("bootstrap/bootstrap.json exists");
        let d = parse(&bytes).expect("parses");
        assert_eq!(d.verify(SIGNER_KEYS, d.issued, None), Ok(()), "signature of the shipped list");
        assert!(SIGNER_KEYS.iter().all(|k| k.trim().len() == 64), "pinned keys are 64 hex");
    }
}
