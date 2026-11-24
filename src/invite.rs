//! Ссылка-приглашение в контакты, часть узла (docs/GUEST.md, ROADMAP11 G3 фаза 1: оба телефона на одном узле).
//!
//! Узел приглашающего А гарантирует одноразовость: приглашение регистрируется на узле (только в памяти), погашается атомарно
//! `unused → consumed`, а введение Б (его ключи и подпись) ждёт А в очереди, пока А не заберёт. Секретный токен узлу при регистрации
//! не нужен (только его хеш), контактный секрет пары выводят сами телефоны (HKDF от X25519 и токена), узел его не знает.
//!
//! * Подписываются **присланные байты** (JSON, который собрал телефон), поэтому канонизация не нужна: подпись — Ed25519 с меткой
//!   домена `yandi-invite-v1\0` / `yandi-claim-v1\0`; узел разбирает те же байты после проверки подписи.
//! * Нет файла и нет диска: перезапуск узла стирает реестр, и приглашение перестаёт действовать (восстанавливать отсутствующее
//!   `unused` по предъявленной ссылке нельзя).
//! * Ни адресов (IP), ни токенов в журналах и ответах нет.
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_ACTIVE_PER_DEVICE: usize = 10;
/// Всего записей одного устройства (с уже погашенными: они держат защиту от повторного использования до срока).
pub const MAX_TOTAL_PER_DEVICE: usize = 100;
pub const MAX_CLAIMS_PER_ISSUER: usize = 20;
pub const MAX_TTL_MS: u64 = 24 * 3600 * 1000;
/// Допустимый разбег часов телефона и узла.
pub const SKEW_MS: u64 = 5 * 60 * 1000;
/// Сколько после погашения введение ждёт приглашающего (не короче этого, даже если срок самой ссылки уже вышел).
pub const DELIVERY_MS: u64 = 24 * 3600 * 1000;
const INVITE_DOMAIN: &[u8] = b"yandi-invite-v1\0";
const CLAIM_DOMAIN: &[u8] = b"yandi-claim-v1\0";
const MAX_FIELD: usize = 4096;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

#[derive(Deserialize)]
struct InviteDoc {
    v: u32,
    id: String,
    issuer_ed: String,
    #[allow(dead_code)]
    issuer_x: String,
    redeem_node: String,
    created_at: u64,
    expires_at: u64,
    token_hash: String,
}

#[derive(Deserialize)]
struct ClaimDoc {
    v: u32,
    invite_id: String,
    invite_digest: String,
    b_ed: String,
    #[allow(dead_code)]
    b_x: String,
    #[allow(dead_code)]
    request_id: String,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Refusal {
    Malformed,
    BadSignature,
    Expired,
    UnknownInvite,
    BadToken,
    InviteUsed,
    TooMany,
    NoRoom,
    WrongNode,
    NotYours,
    Forbidden,
}

impl Refusal {
    pub fn text(self) -> &'static str {
        match self {
            Refusal::Malformed => "приглашение повреждено",
            Refusal::BadSignature => "подпись приглашения не сошлась",
            Refusal::Expired => "приглашение истекло",
            Refusal::UnknownInvite => "приглашение не найдено на узле (оно отозвано, истекло или узел перезапускали)",
            Refusal::BadToken => "приглашение недействительно",
            Refusal::InviteUsed => "приглашение уже использовано",
            Refusal::TooMany => "слишком много действующих приглашений",
            Refusal::NoRoom => "у приглашающего нет места для новых контактов; повторите позже",
            Refusal::WrongNode => "приглашение выдано для другого узла",
            Refusal::NotYours => "ключи не принадлежат этому устройству",
            Refusal::Forbidden => "нельзя принять собственное приглашение",
        }
    }
}

struct Entry {
    issuer_peer: String,
    expires_at: u64,
    token_hash: [u8; 32],
    digest: [u8; 32],
    consumed: Option<([u8; 32], u64)>,
}

#[derive(Clone, Serialize)]
pub struct Claim {
    pub invite_id: String,
    pub claim: String,
    pub claim_sig: String,
    /// номер устройства-«Б» на этом узле
    pub from_peer: String,
    pub consumed_at: u64,
}

#[derive(Default)]
pub struct Registry {
    invites: HashMap<String, Entry>,
    claims: HashMap<String, VecDeque<Claim>>,
}

fn dec(s: &str, n: usize) -> Result<Vec<u8>, Refusal> {
    if s.len() > MAX_FIELD {
        return Err(Refusal::Malformed);
    }
    let v = b64().decode(s).map_err(|_| Refusal::Malformed)?;
    if v.len() != n {
        return Err(Refusal::Malformed);
    }
    Ok(v)
}

fn verify(domain: &[u8], key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), Refusal> {
    let key: [u8; 32] = key.try_into().map_err(|_| Refusal::Malformed)?;
    let vk = VerifyingKey::from_bytes(&key).map_err(|_| Refusal::Malformed)?;
    let sig: [u8; 64] = sig.try_into().map_err(|_| Refusal::Malformed)?;
    let mut data = domain.to_vec();
    data.extend_from_slice(msg);
    vk.verify(&data, &Signature::from_bytes(&sig)).map_err(|_| Refusal::BadSignature)
}

fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

impl Registry {
    /// Периодическая очистка просроченного (вызывает таймер узла).
    pub fn sweep(&mut self, now: u64) {
        self.purge(now);
    }

    fn purge(&mut self, now: u64) {
        // приглашение живёт до срока ссылки, а ПОГАШЕННОЕ — ещё и пока введение ждёт приглашающего (сутки от погашения):
        // успешное погашение перед самым сроком не должно сразу терять введение
        self.invites.retain(|_, e| e.expires_at > now || e.consumed.map_or(false, |(_, at)| now < at + DELIVERY_MS));
        // O(приглашения + введения): множество живых номеров строится один раз, а не перебирается для каждого введения
        let live: std::collections::HashSet<&String> = self.invites.keys().collect();
        for q in self.claims.values_mut() {
            q.retain(|c| live.contains(&c.invite_id) && now < c.consumed_at + DELIVERY_MS);
        }
        self.claims.retain(|_, q| !q.is_empty());
    }

    /// А регистрирует приглашение. `issuer_ed` — ключ подписи этого устройства (из реестра устройств узла), `node_id` — номер этого узла.
    pub fn register(&mut self, now: u64, issuer_peer: &str, issuer_ed: &[u8], node_id: &[u8; 32], invite: &[u8], sig: &[u8]) -> Result<String, Refusal> {
        self.purge(now);
        let doc: InviteDoc = serde_json::from_slice(invite).map_err(|_| Refusal::Malformed)?;
        if doc.v != 1 || invite.len() > MAX_FIELD {
            return Err(Refusal::Malformed);
        }
        let id = dec(&doc.id, 16)?;
        let doc_ed = dec(&doc.issuer_ed, 32)?;
        let th: [u8; 32] = dec(&doc.token_hash, 32)?.try_into().unwrap();
        if !eq(&doc_ed, issuer_ed) {
            return Err(Refusal::NotYours);
        }
        verify(INVITE_DOMAIN, issuer_ed, invite, sig)?;
        if !eq(&dec(&doc.redeem_node, 32)?, node_id) {
            return Err(Refusal::WrongNode);
        }
        if doc.created_at > now + SKEW_MS || doc.expires_at <= now || doc.expires_at > now + MAX_TTL_MS + SKEW_MS || doc.expires_at <= doc.created_at {
            return Err(Refusal::Expired);
        }
        let key = hex::encode(&id);
        if self.invites.contains_key(&key) {
            return Err(Refusal::Malformed); // повтор номера: не затираем действующее
        }
        // действующих (ещё не использованных) не больше десяти; погашенные считаются отдельно, чтобы ссылки можно было выдавать дальше
        let own = || self.invites.values().filter(|e| e.issuer_peer == issuer_peer);
        if own().filter(|e| e.consumed.is_none()).count() >= MAX_ACTIVE_PER_DEVICE || own().count() >= MAX_TOTAL_PER_DEVICE {
            return Err(Refusal::TooMany);
        }
        self.invites.insert(key.clone(), Entry { issuer_peer: issuer_peer.to_string(), expires_at: doc.expires_at, token_hash: th, digest: sha(invite), consumed: None });
        Ok(key)
    }

    /// Б погашает приглашение. `redeemer_ed` — ключ подписи устройства Б на этом узле. Возвращает (claim_digest, consumed_at).
    #[allow(clippy::too_many_arguments)]
    pub fn redeem(&mut self, now: u64, redeemer_peer: &str, redeemer_ed: &[u8], invite: &[u8], invite_sig: &[u8], token: &[u8], claim: &[u8], claim_sig: &[u8]) -> Result<([u8; 32], u64), Refusal> {
        // глобальная уборка здесь не нужна: срок проверяется у самой записи, а чистит таймер (sweep) — недопустимый запрос дёшев
        let doc: InviteDoc = serde_json::from_slice(invite).map_err(|_| Refusal::Malformed)?;
        let cdoc: ClaimDoc = serde_json::from_slice(claim).map_err(|_| Refusal::Malformed)?;
        if doc.v != 1 || cdoc.v != 1 || invite.len() > MAX_FIELD || claim.len() > MAX_FIELD {
            return Err(Refusal::Malformed);
        }
        let id = dec(&doc.id, 16)?;
        let issuer_ed = dec(&doc.issuer_ed, 32)?;
        verify(INVITE_DOMAIN, &issuer_ed, invite, invite_sig)?;
        let key = hex::encode(&id);
        let claim_digest = sha(claim);
        // сначала узнаём, есть ли приглашение: отсутствующее по ссылке не восстанавливается
        let Some(entry) = self.invites.get(&key) else { return Err(Refusal::UnknownInvite) };
        if entry.expires_at <= now {
            return Err(Refusal::Expired);
        }
        if !eq(&entry.digest, &sha(invite)) {
            return Err(Refusal::BadSignature);
        }
        // токен: хеш должен сойтись (постоянное время)
        if !eq(&sha(token), &entry.token_hash) {
            return Err(Refusal::BadToken);
        }
        // введение подписано ключом Б, и это ключ именно этого устройства на узле
        let b_ed = dec(&cdoc.b_ed, 32)?;
        if !eq(&b_ed, redeemer_ed) {
            return Err(Refusal::NotYours);
        }
        verify(CLAIM_DOMAIN, &b_ed, claim, claim_sig)?;
        if cdoc.invite_id != doc.id || !eq(&dec(&cdoc.invite_digest, 32)?, &entry.digest) {
            return Err(Refusal::Malformed);
        }
        if entry.issuer_peer == redeemer_peer {
            return Err(Refusal::Forbidden);
        }
        if let Some((d, at)) = entry.consumed {
            // повтор того же введения (ответ потерялся) — та же квитанция; другое — «использовано»
            return if d == claim_digest { Ok((d, at)) } else { Err(Refusal::InviteUsed) };
        }
        // место для введения резервируется в той же операции: нет места — погашения нет
        let issuer = entry.issuer_peer.clone();
        let q = self.claims.entry(issuer).or_default();
        if q.len() >= MAX_CLAIMS_PER_ISSUER {
            return Err(Refusal::NoRoom);
        }
        q.push_back(Claim {
            invite_id: key.clone(),
            claim: b64().encode(claim),
            claim_sig: b64().encode(claim_sig),
            from_peer: redeemer_peer.to_string(),
            consumed_at: now,
        });
        if let Some(e) = self.invites.get_mut(&key) {
            e.consumed = Some((claim_digest, now));
        }
        Ok((claim_digest, now))
    }

    /// Введения, ждущие А.
    pub fn claims_for(&mut self, now: u64, issuer_peer: &str) -> Vec<Claim> {
        self.purge(now);
        self.claims.get(issuer_peer).map(|q| q.iter().cloned().collect()).unwrap_or_default()
    }

    /// А забрал введения: убираем (приглашение остаётся погашенным до срока, повторно его не использовать).
    pub fn ack(&mut self, issuer_peer: &str, invite_ids: &[String]) {
        if let Some(q) = self.claims.get_mut(issuer_peer) {
            q.retain(|c| !invite_ids.contains(&c.invite_id));
        }
        self.claims.retain(|_, q| !q.is_empty());
    }

    /// А отозвал приглашение (или устройство удалено).
    pub fn revoke(&mut self, issuer_peer: &str, invite_id_hex: Option<&str>) {
        self.invites.retain(|k, e| !(e.issuer_peer == issuer_peer && invite_id_hex.map_or(true, |i| i == k)));
        if let Some(q) = self.claims.get_mut(issuer_peer) {
            let live: std::collections::HashSet<&String> = self.invites.keys().collect();
            q.retain(|c| live.contains(&c.invite_id));
        }
    }

    pub fn active_of(&self, issuer_peer: &str) -> usize {
        self.invites.values().filter(|e| e.issuer_peer == issuer_peer && e.consumed.is_none()).count()
    }
}

pub fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::default()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::RngCore;

    pub(crate) struct Party {
        pub sk: SigningKey,
        pub peer: String,
    }

    pub(crate) fn party(tag: &str) -> Party {
        let mut s = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut s);
        Party { sk: SigningKey::from_bytes(&s), peer: tag.to_string() }
    }

    fn sign(sk: &SigningKey, domain: &[u8], msg: &[u8]) -> Vec<u8> {
        let mut d = domain.to_vec();
        d.extend_from_slice(msg);
        sk.sign(&d).to_bytes().to_vec()
    }

    pub(crate) const NODE: [u8; 32] = [9; 32];

    pub(crate) struct Made {
        pub invite: Vec<u8>,
        pub sig: Vec<u8>,
        pub token: [u8; 32],
    }

    pub(crate) fn make_invite(a: &Party, now: u64, ttl: u64) -> Made {
        let mut id = [0u8; 16];
        let mut token = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut id);
        rand::thread_rng().fill_bytes(&mut token);
        let doc = serde_json::json!({
            "v": 1, "id": b64().encode(id), "issuer_ed": b64().encode(a.sk.verifying_key().to_bytes()), "issuer_x": b64().encode([1u8; 32]),
            "redeem_node": b64().encode(NODE), "created_at": now, "expires_at": now + ttl, "token_hash": b64().encode(sha(&token))
        });
        let invite = serde_json::to_vec(&doc).unwrap();
        let sig = sign(&a.sk, INVITE_DOMAIN, &invite);
        Made { invite, sig, token }
    }

    pub(crate) fn make_claim(b: &Party, m: &Made, request: u8) -> (Vec<u8>, Vec<u8>) {
        let id = serde_json::from_slice::<serde_json::Value>(&m.invite).unwrap()["id"].as_str().unwrap().to_string();
        let doc = serde_json::json!({
            "v": 1, "invite_id": id, "invite_digest": b64().encode(sha(&m.invite)),
            "b_ed": b64().encode(b.sk.verifying_key().to_bytes()), "b_x": b64().encode([2u8; 32]), "request_id": b64().encode([request; 16])
        });
        let claim = serde_json::to_vec(&doc).unwrap();
        let sig = sign(&b.sk, CLAIM_DOMAIN, &claim);
        (claim, sig)
    }

    const NOW: u64 = 1_800_000_000_000;

    fn reg(r: &mut Registry, a: &Party, m: &Made) -> Result<String, Refusal> {
        r.register(NOW, &a.peer, &a.sk.verifying_key().to_bytes(), &NODE, &m.invite, &m.sig)
    }

    fn red(r: &mut Registry, b: &Party, m: &Made, c: &(Vec<u8>, Vec<u8>), at: u64) -> Result<([u8; 32], u64), Refusal> {
        r.redeem(at, &b.peer, &b.sk.verifying_key().to_bytes(), &m.invite, &m.sig, &m.token, &c.0, &c.1)
    }

    #[test]
    fn a_registered_invite_is_redeemed_once_and_the_introduction_waits_for_the_issuer() {
        let (a, b) = (party("a"), party("b"));
        let mut r = Registry::default();
        let m = make_invite(&a, NOW, 3_600_000);
        reg(&mut r, &a, &m).unwrap();
        let c = make_claim(&b, &m, 1);
        let (d, at) = red(&mut r, &b, &m, &c, NOW + 10).unwrap();
        assert_eq!(at, NOW + 10);
        let q = r.claims_for(NOW + 20, "a");
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].from_peer, "b");
        assert_eq!(b64().decode(&q[0].claim).unwrap(), c.0);
        assert_eq!(d, sha(&c.0));
        assert!(r.claims_for(NOW + 20, "b").is_empty(), "чужие введения не отдаются");
        r.ack("a", &[q[0].invite_id.clone()]);
        assert!(r.claims_for(NOW + 30, "a").is_empty());
    }

    #[test]
    fn a_lost_reply_is_repeated_with_the_same_receipt_but_a_different_claim_is_refused() {
        let (a, b, c2) = (party("a"), party("b"), party("c"));
        let mut r = Registry::default();
        let m = make_invite(&a, NOW, 3_600_000);
        reg(&mut r, &a, &m).unwrap();
        let claim = make_claim(&b, &m, 1);
        let first = red(&mut r, &b, &m, &claim, NOW + 1).unwrap();
        assert_eq!(red(&mut r, &b, &m, &claim, NOW + 99).unwrap(), first, "тот же Claim — та же квитанция");
        assert_eq!(r.claims_for(NOW + 100, "a").len(), 1, "введение не задвоилось");
        let other = make_claim(&c2, &m, 2);
        assert_eq!(red(&mut r, &c2, &m, &other, NOW + 2), Err(Refusal::InviteUsed));
        // и тот же Б другим запросом — тоже «использовано»
        let again = make_claim(&b, &m, 3);
        assert_eq!(red(&mut r, &b, &m, &again, NOW + 3), Err(Refusal::InviteUsed));
    }

    #[test]
    fn two_people_pressing_at_once_leave_exactly_one_winner() {
        let a = party("a");
        let m = std::sync::Arc::new(make_invite(&a, NOW, 3_600_000));
        let r = std::sync::Arc::new(Mutex::new(Registry::default()));
        reg(&mut r.lock().unwrap(), &a, &m).unwrap();
        let mut hs = Vec::new();
        for i in 0..16u8 {
            let (r, m) = (r.clone(), m.clone());
            hs.push(std::thread::spawn(move || {
                let b = party(&format!("b{i}"));
                let c = make_claim(&b, &m, i);
                let out = r.lock().unwrap().redeem(NOW + 5, &b.peer, &b.sk.verifying_key().to_bytes(), &m.invite, &m.sig, &m.token, &c.0, &c.1);
                out.is_ok()
            }));
        }
        let wins = hs.into_iter().filter_map(|h| h.join().ok()).filter(|w| *w).count();
        assert_eq!(wins, 1);
        assert_eq!(r.lock().unwrap().claims_for(NOW + 6, "a").len(), 1);
    }

    #[test]
    fn a_forged_invite_a_wrong_token_an_expired_or_unknown_invite_are_refused() {
        let (a, b, evil) = (party("a"), party("b"), party("e"));
        let mut r = Registry::default();
        let m = make_invite(&a, NOW, 3_600_000);
        // регистрация чужим ключом / с чужой подписью
        assert_eq!(r.register(NOW, "e", &evil.sk.verifying_key().to_bytes(), &NODE, &m.invite, &m.sig), Err(Refusal::NotYours));
        let mut bad_sig = m.sig.clone();
        bad_sig[0] ^= 1;
        assert_eq!(r.register(NOW, "a", &a.sk.verifying_key().to_bytes(), &NODE, &m.invite, &bad_sig), Err(Refusal::BadSignature));
        // чужой узел
        assert_eq!(r.register(NOW, "a", &a.sk.verifying_key().to_bytes(), &[7; 32], &m.invite, &m.sig), Err(Refusal::WrongNode));
        reg(&mut r, &a, &m).unwrap();
        let c = make_claim(&b, &m, 1);
        // неверный токен
        let mut wrong = m.token;
        wrong[0] ^= 1;
        assert_eq!(r.redeem(NOW + 1, "b", &b.sk.verifying_key().to_bytes(), &m.invite, &m.sig, &wrong, &c.0, &c.1), Err(Refusal::BadToken));
        // подмена ключа Б: введение подписано одним ключом, устройство — другим
        assert_eq!(r.redeem(NOW + 1, "b", &evil.sk.verifying_key().to_bytes(), &m.invite, &m.sig, &m.token, &c.0, &c.1), Err(Refusal::NotYours));
        // сломанная подпись введения
        let mut cs = c.1.clone();
        cs[3] ^= 1;
        assert_eq!(r.redeem(NOW + 1, "b", &b.sk.verifying_key().to_bytes(), &m.invite, &m.sig, &m.token, &c.0, &cs), Err(Refusal::BadSignature));
        // срок
        assert_eq!(red(&mut r, &b, &m, &c, NOW + 3_600_000), Err(Refusal::Expired), "срок проверяется у самой записи, не дожидаясь уборки");
        r.sweep(NOW + 3_600_000);
        assert_eq!(red(&mut r, &b, &m, &c, NOW + 3_600_001), Err(Refusal::UnknownInvite), "после уборки — как неизвестное");
        // приглашение, которого узел не регистрировал, по ссылке не появляется
        let m2 = make_invite(&a, NOW, 3_600_000);
        let c2 = make_claim(&b, &m2, 1);
        assert_eq!(red(&mut r, &b, &m2, &c2, NOW + 1), Err(Refusal::UnknownInvite));
        // своё приглашение принять нельзя
        let m3 = make_invite(&a, NOW, 3_600_000);
        reg(&mut r, &a, &m3).unwrap();
        let c3 = make_claim(&a, &m3, 1);
        assert_eq!(red(&mut r, &a, &m3, &c3, NOW + 1), Err(Refusal::Forbidden));
    }

    #[test]
    fn a_restart_forgets_every_invite_and_nothing_is_restored_from_the_link() {
        let (a, b) = (party("a"), party("b"));
        let mut r = Registry::default();
        let m = make_invite(&a, NOW, 3_600_000);
        reg(&mut r, &a, &m).unwrap();
        r = Registry::default(); // перезапуск: память пуста
        let c = make_claim(&b, &m, 1);
        assert_eq!(red(&mut r, &b, &m, &c, NOW + 1), Err(Refusal::UnknownInvite));
    }

    #[test]
    fn limits_ten_active_invites_a_day_at_most_and_no_room_means_no_redemption() {
        let a = party("a");
        let mut r = Registry::default();
        for _ in 0..MAX_ACTIVE_PER_DEVICE {
            let m = make_invite(&a, NOW, 3_600_000);
            reg(&mut r, &a, &m).unwrap();
        }
        let extra = make_invite(&a, NOW, 3_600_000);
        assert_eq!(reg(&mut r, &a, &extra), Err(Refusal::TooMany));
        // срок больше суток не принимается
        let mut r2 = Registry::default();
        let long = make_invite(&a, NOW, MAX_TTL_MS + 3_600_000);
        assert_eq!(reg(&mut r2, &a, &long), Err(Refusal::Expired));
        // место под введения у А кончилось — погашения нет, приглашение остаётся нетронутым
        let mut r3 = Registry::default();
        let invites: Vec<Made> = (0..MAX_CLAIMS_PER_ISSUER + 1).map(|_| make_invite(&a, NOW, 3_600_000)).collect();
        // десять активных за раз — это предел регистрации, поэтому регистрируем и гасим партиями
        let mut done = 0usize;
        for chunk in invites.chunks(MAX_ACTIVE_PER_DEVICE) {
            for m in chunk {
                reg(&mut r3, &a, m).unwrap();
            }
            for m in chunk {
                let b = party(&format!("b{done}"));
                let c = make_claim(&b, m, 1);
                let out = red(&mut r3, &b, m, &c, NOW + 1);
                if done < MAX_CLAIMS_PER_ISSUER {
                    out.unwrap();
                } else {
                    assert_eq!(out, Err(Refusal::NoRoom));
                    // приглашение не потрачено: после того как А забрал введения, оно гасится
                    let q = r3.claims_for(NOW + 2, "a");
                    r3.ack("a", &q.iter().map(|c| c.invite_id.clone()).collect::<Vec<_>>());
                    let c = make_claim(&b, m, 1);
                    red(&mut r3, &b, m, &c, NOW + 3).unwrap();
                }
                done += 1;
            }
        }
    }

    #[test]
    fn revoking_and_expiry_clean_everything_up() {
        let (a, b) = (party("a"), party("b"));
        let mut r = Registry::default();
        let m = make_invite(&a, NOW, 3_600_000);
        reg(&mut r, &a, &m).unwrap();
        let c = make_claim(&b, &m, 1);
        assert_eq!(r.active_of("a"), 1);
        red(&mut r, &b, &m, &c, NOW + 1).unwrap();
        assert_eq!(r.active_of("a"), 0, "погашенное не считается действующим");
        r.revoke("a", None);
        assert!(r.claims_for(NOW + 1, "a").is_empty());
        assert!(r.claims_for(NOW + 2, "a").is_empty());
        // срок ссылки вышел, а введение уже принято: оно ждёт приглашающего ещё сутки от погашения, потом пропадает
        let m2 = make_invite(&a, NOW, 60_000);
        reg(&mut r, &a, &m2).unwrap();
        let c2 = make_claim(&b, &m2, 2);
        red(&mut r, &b, &m2, &c2, NOW + 59_000).unwrap();
        assert_eq!(r.claims_for(NOW + 60_001, "a").len(), 1, "погашено за секунду до срока — введение не потеряно");
        assert_eq!(r.claims_for(NOW + 59_000 + DELIVERY_MS - 1, "a").len(), 1);
        assert!(r.claims_for(NOW + 59_000 + DELIVERY_MS, "a").is_empty(), "через сутки от погашения введение стёрто");
        // а непогашенное приглашение исчезает ровно в срок
        let m3 = make_invite(&a, NOW, 60_000);
        reg(&mut r, &a, &m3).unwrap();
        r.sweep(NOW + 60_001);
        assert_eq!(r.active_of("a"), 0);
    }
}
