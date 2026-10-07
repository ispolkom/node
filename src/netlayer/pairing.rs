// src/netlayer/pairing.rs
//! Pairing + session resume для YANDI (Iter 4).
//!
//! Цель — мобилка перманентно «приземлена» на свой anchor, переживает смену IP/сети
//! без полного re-handshake'а.
//!
//! **Что здесь:**
//! - `PairingPayload` — то, что anchor показывает в QR (anchor_id, pubkey, fingerprint, url).
//! - Парсинг QR-payload'а в обе стороны (compact JSON).
//! - `SessionToken { session_id, resume_secret, expires_at }`.
//! - HMAC-resume (`hmac(resume_secret, new_addr || session_id)`) — blake3 keyed.
//! - `PairedClientStore` — anchor хранит `{client_pubkey → SessionToken}` в `~/.yandi/paired_clients.json`.
//! - `PairedAnchorStore` — mobile хранит `Vec<PairingPayload>` (preference order) в
//!   `~/.yandi/paired_anchors.json`.
//! - Wire 0xC0 RESUME / 0xC1 RESUME_ACK encode/decode.
//!
//! **Чего НЕТ (намеренно):**
//! - Реальной QR-генерации в web UI (PNG-image render) — это интеграция;
//!   payload-формат готов, embed в HTML делается отдельно.
//! - Auto-reconnect loop в transport — ядро (parsing+verify) готово; цикл подключается
//!   при integration-фазе. План 4.6 → known issue до полной интеграции.
//! - Ротации resume_secret per-resume (план 4.3 «обновляется») — Iter 4.x: при первом
//!   успешном resume anchor возвращает новый `resume_secret`. На текущем шаге выдача
//!   токена есть, явная ротация — TODO: добавить `rotate_secret()` поверх store при integration.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::util::HashId;

/// Wire byte для RESUME-пакета (mobile → anchor).
pub const PKT_RESUME: u8 = 0xC0;
/// Wire byte для RESUME_ACK (anchor → mobile).
pub const PKT_RESUME_ACK: u8 = 0xC1;
/// 🆕 Hardening Step 3: anchor выдаёт session_token первой синхронизацией после pair'инга.
/// `0xC2 SESSION_ISSUE`: encrypted payload через WS-канал mobile↔anchor.
pub const PKT_SESSION_ISSUE: u8 = 0xC2;

/// 7 дней TTL по умолчанию.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 7 * 24 * 3600;

// ----------------- Payload (QR) -----------------

/// То, что anchor показывает в QR-коде. Mobile сканит → парсит → сохраняет в свой store.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingPayload {
    pub anchor_id: HashId,
    /// Hex-encoded X25519 public key anchor'а (для будущего ECDH; в Iter 4 не используется напрямую,
    /// но валидируется при resume).
    pub anchor_x25519_hex: String,
    /// SHA-256 fingerprint TLS-сертификата anchor'а (тот же, что `TlsIdentity::fingerprint_hex`).
    pub fingerprint_hex: String,
    /// `wss://host:port/` — куда мобилка коннектится.
    pub anchor_url: String,
}

impl PairingPayload {
    /// Сериализовать в compact JSON (то что попадёт в QR). Принципиально читабельно
    /// чтобы дебажить из консоли.
    pub fn to_qr_string(&self) -> String {
        serde_json::to_string(self).expect("serialize PairingPayload")
    }

    pub fn from_qr_string(s: &str) -> Result<Self> {
        serde_json::from_str::<Self>(s).context("parse PairingPayload from QR")
    }
}

// ----------------- Session Token -----------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionToken {
    pub session_id: u128,
    /// 32 байта секрета, HMAC-key для resume. Anchor И mobile хранят одинаковый.
    pub resume_secret_hex: String,
    pub expires_at: u64,
    /// 🔐 Hardening Step 2: 32-байтный AES-256 session-key, который существовал на момент
    /// pair'инга. Anchor И mobile хранят одинаковый, чтобы при resume можно было сразу
    /// восстановить encrypted-channel (без полного ECDH-handshake'а). Optional для
    /// backward-compat'a со старыми (Iter 4) токенами — там этого поля не было.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key_hex: Option<String>,
    /// Node id this token was first resumed under (remembered at the first resume): later resumes must claim the same id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id_hex: Option<String>,
}

impl SessionToken {
    pub fn new(ttl_secs: u64) -> Self {
        use rand::RngCore;
        let mut rng = rand::thread_rng();
        let session_id = {
            let mut b = [0u8; 16];
            rng.fill_bytes(&mut b);
            u128::from_be_bytes(b)
        };
        let mut secret = [0u8; 32];
        rng.fill_bytes(&mut secret);
        Self {
            session_id,
            resume_secret_hex: hex::encode(secret),
            expires_at: now_secs().saturating_add(ttl_secs),
            session_key_hex: None,
            node_id_hex: None,
        }
    }

    /// Step 2: создать токен сразу с известным session-key (используется при первой
    /// успешной key-exchange'е, чтобы потом resume не требовал ECDH).
    pub fn new_with_session_key(ttl_secs: u64, session_key: &[u8; 32]) -> Self {
        let mut tok = Self::new(ttl_secs);
        tok.session_key_hex = Some(hex::encode(session_key));
        tok
    }

    pub fn is_expired(&self) -> bool {
        now_secs() > self.expires_at
    }

    pub fn refresh(&mut self, ttl_secs: u64) {
        self.expires_at = now_secs().saturating_add(ttl_secs);
    }

    pub fn resume_secret(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.resume_secret_hex)
            .context("hex decode resume_secret")?;
        if bytes.len() != 32 {
            anyhow::bail!("resume_secret length != 32");
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    /// Step 2: декодировать сохранённый session-key (32 байта). `None` если поле
    /// не выставлено (старый токен без session-key resume) или формат битый.
    pub fn session_key(&self) -> Option<[u8; 32]> {
        let hex_str = self.session_key_hex.as_ref()?;
        let bytes = hex::decode(hex_str).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Some(out)
    }

    /// Step 2: задать session-key пост-фактум (например, после успешного ECDH).
    pub fn set_session_key(&mut self, session_key: &[u8; 32]) {
        self.session_key_hex = Some(hex::encode(session_key));
    }
}

/// HMAC-resume: blake3 keyed hash от (session_id_be_16 || new_addr_utf8).
/// Возвращает 32 байта, hex-кодируем для wire/store.
pub fn compute_resume_mac(secret: &[u8; 32], session_id: u128, new_addr: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(secret);
    hasher.update(&session_id.to_be_bytes());
    hasher.update(new_addr.as_bytes());
    *hasher.finalize().as_bytes()
}

pub fn verify_resume_mac(
    secret: &[u8; 32],
    session_id: u128,
    new_addr: &str,
    provided: &[u8; 32],
) -> bool {
    let expected = compute_resume_mac(secret, session_id, new_addr);
    constant_time_eq(&expected, provided)
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ----------------- Wire format -----------------

/// Hardening Step 2: `0xC0 RESUME` (v2 — с node_id для plaintext pre-Hello-flow'а).
///
/// `[C0][node_id:32][session_id:16][addr_len:1][addr…][mac:32]`
///
/// Mobile шлёт это плейнтекстом ДО Hello: anchor по session_id находит paired-token,
/// проверяет HMAC, восстанавливает session-key через `EncryptionManager::restore_session`
/// и отвечает 0xC1 ACK encrypted (используя восстановленный ключ).
pub fn encode_resume(node_id: HashId, session_id: u128, new_addr: &str, mac: &[u8; 32]) -> Vec<u8> {
    let addr = new_addr.as_bytes();
    let alen = addr.len().min(255);
    let mut buf = Vec::with_capacity(1 + 32 + 16 + 1 + alen + 32);
    buf.push(PKT_RESUME);
    buf.extend_from_slice(&node_id.0);
    buf.extend_from_slice(&session_id.to_be_bytes());
    buf.push(alen as u8);
    buf.extend_from_slice(&addr[..alen]);
    buf.extend_from_slice(mac);
    buf
}

pub fn decode_resume(data: &[u8]) -> Result<(HashId, u128, String, [u8; 32])> {
    if data.len() < 1 + 32 + 16 + 1 + 32 {
        anyhow::bail!("RESUME too short");
    }
    if data[0] != PKT_RESUME {
        anyhow::bail!("RESUME bad magic: 0x{:02x}", data[0]);
    }
    let mut nid = [0u8; 32];
    nid.copy_from_slice(&data[1..33]);
    let node_id = HashId(nid);
    let mut sid = [0u8; 16];
    sid.copy_from_slice(&data[33..49]);
    let session_id = u128::from_be_bytes(sid);
    let alen = data[49] as usize;
    if data.len() < 50 + alen + 32 {
        anyhow::bail!("RESUME truncated addr");
    }
    let addr = String::from_utf8(data[50..50 + alen].to_vec())
        .context("RESUME addr not utf8")?;
    let mut mac = [0u8; 32];
    mac.copy_from_slice(&data[50 + alen..50 + alen + 32]);
    Ok((node_id, session_id, addr, mac))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeStatus {
    Ok = 0,
    BadMac = 1,
    Expired = 2,
    Unknown = 3,
}

impl ResumeStatus {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Ok,
            1 => Self::BadMac,
            2 => Self::Expired,
            _ => Self::Unknown,
        }
    }
}

/// `0xC1 RESUME_ACK`: `[C1][status:1][optional new_secret_flag:1][optional new_secret:32]`.
/// Если status != Ok — без секрета.
pub fn encode_resume_ack(status: ResumeStatus, new_secret: Option<&[u8; 32]>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 1 + 1 + 32);
    buf.push(PKT_RESUME_ACK);
    buf.push(status as u8);
    if let Some(s) = new_secret {
        buf.push(1);
        buf.extend_from_slice(s);
    } else {
        buf.push(0);
    }
    buf
}

/// Hardening Step 3: 0xC2 SESSION_ISSUE — anchor выдаёт SessionToken после успешного
/// pair'инга (QR-сканинг + первое Hello). Payload encrypted (через `send_encrypted`)
/// чтобы session-key/resume-secret не утекли. Wire-layout (внутри plaintext'а после
/// decrypt'a):
///
/// `[C2][session_id:16][expires_at:8][resume_secret:32][session_key:32]`
///
/// `expires_at` — UNIX-секунды.
pub fn encode_session_issue(tok: &SessionToken, session_key: &[u8; 32]) -> Result<Vec<u8>> {
    let resume_secret = tok.resume_secret()?;
    let mut buf = Vec::with_capacity(1 + 16 + 8 + 32 + 32);
    buf.push(PKT_SESSION_ISSUE);
    buf.extend_from_slice(&tok.session_id.to_be_bytes());
    buf.extend_from_slice(&tok.expires_at.to_be_bytes());
    buf.extend_from_slice(&resume_secret);
    buf.extend_from_slice(session_key);
    Ok(buf)
}

pub fn decode_session_issue(data: &[u8]) -> Result<(SessionToken, [u8; 32])> {
    if data.len() < 1 + 16 + 8 + 32 + 32 {
        anyhow::bail!("SESSION_ISSUE too short");
    }
    if data[0] != PKT_SESSION_ISSUE {
        anyhow::bail!("SESSION_ISSUE bad magic: 0x{:02x}", data[0]);
    }
    let mut sid = [0u8; 16];
    sid.copy_from_slice(&data[1..17]);
    let session_id = u128::from_be_bytes(sid);
    let mut exp = [0u8; 8];
    exp.copy_from_slice(&data[17..25]);
    let expires_at = u64::from_be_bytes(exp);
    let mut resume_secret = [0u8; 32];
    resume_secret.copy_from_slice(&data[25..57]);
    let mut session_key = [0u8; 32];
    session_key.copy_from_slice(&data[57..89]);
    let tok = SessionToken {
        session_id,
        resume_secret_hex: hex::encode(resume_secret),
        expires_at,
        session_key_hex: Some(hex::encode(session_key)),
        node_id_hex: None,
    };
    Ok((tok, session_key))
}

pub fn decode_resume_ack(data: &[u8]) -> Result<(ResumeStatus, Option<[u8; 32]>)> {
    if data.len() < 1 + 1 + 1 {
        anyhow::bail!("RESUME_ACK too short");
    }
    if data[0] != PKT_RESUME_ACK {
        anyhow::bail!("RESUME_ACK bad magic: 0x{:02x}", data[0]);
    }
    let status = ResumeStatus::from_byte(data[1]);
    let secret = if data[2] == 1 {
        if data.len() < 3 + 32 {
            anyhow::bail!("RESUME_ACK truncated secret");
        }
        let mut s = [0u8; 32];
        s.copy_from_slice(&data[3..35]);
        Some(s)
    } else {
        None
    };
    Ok((status, secret))
}

// ----------------- Stores -----------------

/// На anchor-стороне: кого мы спарили, какой токен у каждого.
/// Ключ — hex public_key мобилки (Ed25519 32B).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PairedClientStore {
    pub clients: HashMap<String, SessionToken>,
}

impl PairedClientStore {
    pub fn load_or_default(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }
        let s = serde_json::to_string_pretty(self).context("serialize PairedClientStore")?;
        crate::util::private_file::write_private(path, s.as_bytes()).with_context(|| format!("write {:?}", path))?;
        Ok(())
    }

    pub fn issue(&mut self, client_pubkey_hex: String, ttl_secs: u64) -> SessionToken {
        let tok = SessionToken::new(ttl_secs);
        self.clients.insert(client_pubkey_hex, tok.clone());
        tok
    }

    pub fn get(&self, client_pubkey_hex: &str) -> Option<&SessionToken> {
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

/// На mobile-стороне: список наших anchor'ов в порядке предпочтения. Также храним
/// session_token для каждого (resume_secret + session_id).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PairedAnchorStore {
    pub anchors: Vec<PairedAnchorEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedAnchorEntry {
    pub payload: PairingPayload,
    pub session: Option<SessionToken>,
    /// Преферанс (меньше — выше в очереди). Не обязательно уникальный.
    pub preference: u32,
}

impl PairedAnchorStore {
    pub fn load_or_default(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
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
            self.anchors.push(PairedAnchorEntry { payload, session: None, preference });
        }
        self.anchors.sort_by_key(|e| e.preference);
    }

    pub fn set_session(&mut self, anchor_id: &HashId, session: SessionToken) -> bool {
        if let Some(e) = self.anchors.iter_mut().find(|e| e.payload.anchor_id == *anchor_id) {
            e.session = Some(session);
            true
        } else {
            false
        }
    }

    pub fn primary(&self) -> Option<&PairedAnchorEntry> {
        self.anchors.first()
    }
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

    #[test]
    fn pairing_payload_qr_roundtrip() {
        let p = PairingPayload {
            anchor_id: HashId([0xAB; 32]),
            anchor_x25519_hex: hex::encode([0x33u8; 32]),
            fingerprint_hex: "abcdef".repeat(10) + "abcd",
            anchor_url: "wss://anchor.example:443/".into(),
        };
        let s = p.to_qr_string();
        let back = PairingPayload::from_qr_string(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn session_token_new_unique_and_not_expired() {
        let a = SessionToken::new(100);
        let b = SessionToken::new(100);
        assert_ne!(a.session_id, b.session_id);
        assert_ne!(a.resume_secret_hex, b.resume_secret_hex);
        assert!(!a.is_expired());
        assert_eq!(a.resume_secret().unwrap().len(), 32);
    }

    #[test]
    fn session_token_expires() {
        let mut t = SessionToken::new(60);
        t.expires_at = 1; // в прошлом
        assert!(t.is_expired());
        t.refresh(60);
        assert!(!t.is_expired());
    }

    #[test]
    fn resume_mac_verify_roundtrip() {
        let secret = [0xAFu8; 32];
        let sid: u128 = 0x1234_5678_9ABC_DEF0_1122_3344_5566_7788;
        let addr = "1.2.3.4:9000";
        let mac = compute_resume_mac(&secret, sid, addr);
        assert!(verify_resume_mac(&secret, sid, addr, &mac));
        // wrong addr — must fail
        assert!(!verify_resume_mac(&secret, sid, "5.6.7.8:9000", &mac));
        // wrong secret — must fail
        assert!(!verify_resume_mac(&[0u8; 32], sid, addr, &mac));
    }

    #[test]
    fn resume_packet_roundtrip() {
        let mac = [0x77u8; 32];
        let nid = HashId([0xCDu8; 32]);
        let bytes = encode_resume(nid, 99u128, "1.2.3.4:5000", &mac);
        let (nid2, sid, addr, mac2) = decode_resume(&bytes).unwrap();
        assert_eq!(nid2, nid);
        assert_eq!(sid, 99u128);
        assert_eq!(addr, "1.2.3.4:5000");
        assert_eq!(mac2, mac);
    }

    #[test]
    fn session_issue_roundtrip() {
        let sk = [0x5Au8; 32];
        let tok = SessionToken::new_with_session_key(60, &sk);
        let bytes = encode_session_issue(&tok, &sk).unwrap();
        let (tok2, sk2) = decode_session_issue(&bytes).unwrap();
        assert_eq!(tok.session_id, tok2.session_id);
        assert_eq!(tok.resume_secret_hex, tok2.resume_secret_hex);
        assert_eq!(tok.expires_at, tok2.expires_at);
        assert_eq!(sk2, sk);
        assert_eq!(tok2.session_key(), Some(sk));
    }

    #[test]
    fn session_token_new_with_session_key_stores_and_recovers() {
        let key = [0xA7u8; 32];
        let tok = SessionToken::new_with_session_key(60, &key);
        assert_eq!(tok.session_key(), Some(key));
        // Старый токен без session_key
        let mut legacy = SessionToken::new(60);
        assert!(legacy.session_key().is_none());
        legacy.set_session_key(&key);
        assert_eq!(legacy.session_key(), Some(key));
    }

    #[test]
    fn resume_ack_roundtrip_no_secret() {
        let bytes = encode_resume_ack(ResumeStatus::Ok, None);
        let (status, secret) = decode_resume_ack(&bytes).unwrap();
        assert_eq!(status, ResumeStatus::Ok);
        assert!(secret.is_none());
    }

    #[test]
    fn resume_ack_roundtrip_with_new_secret() {
        let s = [0x42u8; 32];
        let bytes = encode_resume_ack(ResumeStatus::Ok, Some(&s));
        let (status, secret) = decode_resume_ack(&bytes).unwrap();
        assert_eq!(status, ResumeStatus::Ok);
        assert_eq!(secret.unwrap(), s);
    }

    #[test]
    fn paired_client_store_issue_lookup_revoke_persist() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        let mut store = PairedClientStore::default();
        let pubkey_hex = "deadbeef".to_string();
        let tok = store.issue(pubkey_hex.clone(), 60);
        assert!(store.get(&pubkey_hex).is_some());
        store.save(&path).unwrap();

        // reload
        let store2 = PairedClientStore::load_or_default(&path);
        let tok2 = store2.get(&pubkey_hex).unwrap();
        assert_eq!(tok.session_id, tok2.session_id);
        assert_eq!(tok.resume_secret_hex, tok2.resume_secret_hex);

        // revoke
        let mut store3 = store2;
        assert!(store3.revoke(&pubkey_hex));
        assert!(store3.get(&pubkey_hex).is_none());
    }

    #[test]
    fn paired_client_store_gc_expired() {
        let mut store = PairedClientStore::default();
        let _ = store.issue("a".into(), 60);
        store.clients.get_mut("a").unwrap().expires_at = 1;
        let _ = store.issue("b".into(), 60);
        let cleared = store.gc_expired();
        assert_eq!(cleared, 1);
        assert!(store.get("a").is_none());
        assert!(store.get("b").is_some());
    }

    #[test]
    fn paired_anchor_store_preference_order() {
        let mut s = PairedAnchorStore::default();
        let p1 = PairingPayload {
            anchor_id: HashId([1u8; 32]),
            anchor_x25519_hex: "00".into(),
            fingerprint_hex: "ff".into(),
            anchor_url: "wss://primary/".into(),
        };
        let p2 = PairingPayload {
            anchor_id: HashId([2u8; 32]),
            anchor_x25519_hex: "00".into(),
            fingerprint_hex: "ff".into(),
            anchor_url: "wss://secondary/".into(),
        };
        s.add_or_update(p2.clone(), 5);
        s.add_or_update(p1.clone(), 1);
        assert_eq!(s.primary().unwrap().payload, p1);
    }

    #[test]
    fn paired_anchor_store_set_session_persist() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_anchors.json");
        let mut s = PairedAnchorStore::default();
        let payload = PairingPayload {
            anchor_id: HashId([9u8; 32]),
            anchor_x25519_hex: "ab".into(),
            fingerprint_hex: "cd".into(),
            anchor_url: "wss://a/".into(),
        };
        s.add_or_update(payload.clone(), 0);
        let tok = SessionToken::new(60);
        assert!(s.set_session(&payload.anchor_id, tok.clone()));
        s.save(&path).unwrap();

        let s2 = PairedAnchorStore::load_or_default(&path);
        let entry = s2.anchors.iter().find(|e| e.payload.anchor_id == payload.anchor_id).unwrap();
        assert_eq!(entry.session.as_ref().unwrap().session_id, tok.session_id);
    }
}
