//! Узлы за NAT как клиенты через ретрансляторы (решение владельца 2026-10-06: узлами становятся только узлы с белым IP, остальные —
//! клиенты через ретрансляторы и всегда сообщают о смене адреса).
//!
//! * **Ретранслятор** — обычный узел с проверенным белым адресом (`relay` в карточке; можно выключить). Отдельной службы ему не
//!   нужно: клиент сам подключается к нему (исходящее соединение проходит NAT), и ретранслятор может продлить цепочку (`hops`) к
//!   клиенту по этому соединению. Он видит только зашифрованные слои.
//! * **Клиент** — узел, до которого нельзя достучаться снаружи: либо задано владельцем (`YANDI_CLIENT_ONLY=1`), либо узел сам
//!   заметил, что его карточку никто не смог проверить (за 10 минут при ≥2 соседях ни одной входящей пробы). Он перестаёт
//!   публиковать карточку узла, подключается к двум ретрансляторам и объявляет **запись о доступности**: «узел N сейчас доступен
//!   через ретрансляторы R1, R2», подписанную своим ключом. Записи раздаются соседям так же, как карточки.
//! * Кто знает номер и ключ узла (свои устройства, доверенные), находит его ретрансляторы по записи и строит к нему цепочку
//!   `ретранслятор → узел`. Записи проверяемы любым (ключ внутри), поэтому подделка не принимается; номер узла и его ретрансляторы
//!   в записи видны всем — это публичное объявление «я на связи» (номера случайные, имён нет).
//! * Смена адреса клиента ничего не ломает: он сам подключается к ретрансляторам заново и перевыпускает запись.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const PKT_VIA: u8 = 0xDD;
pub const VIA_TTL_SECS: u64 = 3600;
pub const REPUBLISH_SECS: u64 = 600;
const MAX_RECORDS: usize = 20_000;
const PER_NODE: usize = 4;
const GOSSIP_BATCH: usize = 24;
const MAX_PACKET_BYTES: usize = 60 * 1024;
const CLOCK_SKEW_SECS: u64 = 300;
/// Сколько секунд узел ждёт, прежде чем решить, что до него не достучаться (нет ни одной входящей пробы).
pub const DEMOTE_AFTER_SECS: u64 = 600;
pub const WANT_RELAYS: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViaRecord {
    pub v: u8,
    pub node_id: String,
    pub key: String,
    /// ретрансляторы (номера), до 4
    pub relays: Vec<String>,
    pub issued: u64,
    pub expires: u64,
    pub sig: String,
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| c.is_ascii_hexdigit())
}

impl ViaRecord {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut c = self.clone();
        c.sig = String::new();
        let mut b = b"yandi-via-v1\0".to_vec();
        b.extend_from_slice(&serde_json::to_vec(&c).unwrap_or_default());
        b
    }

    pub fn sign(mut self, key: &SigningKey) -> ViaRecord {
        self.key = hex::encode(key.verifying_key().to_bytes());
        self.sig = String::new();
        self.sig = hex::encode(key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    pub fn check(&self, now: u64) -> Result<(), &'static str> {
        if self.v != 1 {
            return Err("version");
        }
        if !is_hex(&self.node_id, 64) || !is_hex(&self.key, 64) || !is_hex(&self.sig, 128) {
            return Err("ids");
        }
        if self.relays.is_empty() || self.relays.len() > 4 || self.relays.iter().any(|r| !is_hex(r, 64) || *r == self.node_id) {
            return Err("relays");
        }
        if self.expires <= now || self.issued > now + CLOCK_SKEW_SECS || self.expires < self.issued || self.expires - self.issued > VIA_TTL_SECS {
            return Err("time");
        }
        let key = hex::decode(&self.key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or("key")?;
        if !hex::decode(&self.node_id).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).map_or(false, |id| crate::util::types::id_acceptable(&id, key.as_bytes())) {
            return Err("id not bound to key");
        }
        let sig = hex::decode(&self.sig).ok().and_then(|b| <[u8; 64]>::try_from(b).ok()).map(|b| Signature::from_bytes(&b)).ok_or("sig")?;
        key.verify(&self.signing_bytes(), &sig).map_err(|_| "signature")
    }
}

#[derive(Default)]
pub struct ViaStore {
    by_node: HashMap<String, Vec<ViaRecord>>,
    total: usize,
}

#[derive(Debug, PartialEq)]
pub enum Taken {
    New,
    Newer,
    Stale,
    Refused(&'static str),
}

impl ViaStore {
    pub fn accept(&mut self, r: ViaRecord, now: u64) -> Taken {
        if let Err(e) = r.check(now) {
            return Taken::Refused(e);
        }
        // an update of a record we already hold never needs new room; a new record is refused BEFORE a bucket is created
        let known = self.by_node.get(&r.node_id).map_or(false, |l| l.iter().any(|o| o.key == r.key));
        if !known && self.total >= MAX_RECORDS {
            return Taken::Refused("full");
        }
        let list = self.by_node.entry(r.node_id.clone()).or_default();
        if let Some(old) = list.iter_mut().find(|o| o.key == r.key) {
            if old.issued >= r.issued {
                return Taken::Stale;
            }
            *old = r;
            return Taken::Newer;
        }
        list.push(r);
        self.total += 1;
        // чужие записи под тем же номером (другой ключ) копить без предела нельзя: остаются самые свежие
        if list.len() > PER_NODE {
            list.sort_by_key(|o| std::cmp::Reverse(o.issued));
            list.truncate(PER_NODE);
            self.total = self.by_node.values().map(|v| v.len()).sum();
        }
        Taken::New
    }

    /// Самая свежая действующая запись узла с этим ключом (ключ известен тому, кто ищет: из сопряжения или визитки).
    pub fn lookup(&self, node_id: &str, key: &str, now: u64) -> Option<ViaRecord> {
        self.by_node.get(node_id)?.iter().filter(|r| r.key == key && r.expires > now).max_by_key(|r| r.issued).cloned()
    }

    pub fn forget_expired(&mut self, now: u64) {
        for v in self.by_node.values_mut() {
            v.retain(|r| r.expires > now);
        }
        self.by_node.retain(|_, v| !v.is_empty());
        self.total = self.by_node.values().map(|v| v.len()).sum();
    }

    pub fn sample(&self, n: usize, now: u64) -> Vec<ViaRecord> {
        use rand::seq::SliceRandom;
        let mut all: Vec<ViaRecord> = self.by_node.values().flatten().filter(|r| r.expires > now).cloned().collect();
        all.shuffle(&mut rand::thread_rng());
        all.truncate(n);
        all
    }
}

/// Пакет обмена записями: `[PKT_VIA][JSON-массив]`.
pub fn encode_packet(list: &[ViaRecord]) -> Vec<u8> {
    let mut l = list.to_vec();
    loop {
        let mut p = vec![PKT_VIA];
        p.extend_from_slice(&serde_json::to_vec(&l).unwrap_or_default());
        if p.len() <= MAX_PACKET_BYTES || l.is_empty() {
            return p;
        }
        l.pop();
    }
}

pub fn receive_packet(store: &mut ViaStore, plaintext: &[u8], now: u64) -> usize {
    if plaintext.first() != Some(&PKT_VIA) || plaintext.len() > MAX_PACKET_BYTES {
        return 0;
    }
    let body = &plaintext[1..];
    let end = body.iter().rposition(|&b| b != 0).map(|i| i + 1).unwrap_or(0);
    let Ok(list) = serde_json::from_slice::<Vec<ViaRecord>>(&body[..end]) else { return 0 };
    list.into_iter().take(GOSSIP_BATCH).filter(|r| matches!(store.accept(r.clone(), now), Taken::New | Taken::Newer)).count()
}

// ---------------------------------------------------------------- настройка ретранслятора

fn relay_file() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("relay.json")
}

/// Готов ли этот узел быть ретранслятором (по умолчанию — да; выключается файлом `relay.json` `{"enabled": false}` или
/// `YANDI_RELAY=off`).
pub fn relay_enabled() -> bool {
    if std::env::var("YANDI_RELAY").map(|v| v == "off").unwrap_or(false) {
        return false;
    }
    std::fs::read_to_string(relay_file()).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v["enabled"].as_bool()).unwrap_or(true)
}

/// `relay_enabled()` reads a file; the packet path asks often, so the answer is kept for a few seconds.
pub fn relay_enabled_cached() -> bool {
    static C: OnceLock<Mutex<(std::time::Instant, bool)>> = OnceLock::new();
    let m = C.get_or_init(|| Mutex::new((std::time::Instant::now() - std::time::Duration::from_secs(60), true)));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if g.0.elapsed() > std::time::Duration::from_secs(5) {
        *g = (std::time::Instant::now(), relay_enabled());
    }
    g.1
}

// ---------------------------------------------------------------- живое состояние

fn store_cell() -> &'static Mutex<ViaStore> {
    static S: OnceLock<Mutex<ViaStore>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn client_flag() -> &'static AtomicBool {
    static C: AtomicBool = AtomicBool::new(false);
    &C
}

/// Этот узел сейчас клиент (карточку узла не публикует, доступен через ретрансляторы).
pub fn is_client() -> bool {
    client_flag().load(Ordering::Relaxed)
}

fn now() -> u64 {
    crate::network_offers::now_secs()
}

/// Найти ретрансляторы узла по номеру и ключу (из сопряжения).
pub fn lookup_relays(node_id: &str, key: &str) -> Vec<String> {
    store_cell().lock().unwrap_or_else(|e| e.into_inner()).lookup(node_id, key, now()).map(|r| r.relays).unwrap_or_default()
}

/// Пакет записей от соседа.
pub fn on_packet(sender: &[u8; 32], plaintext: &[u8]) {
    static SEEN: OnceLock<Mutex<HashMap<[u8; 32], (u64, u32)>>> = OnceLock::new();
    let t = now();
    {
        let mut m = SEEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if m.len() > 10_000 {
            m.retain(|_, (s, _)| t.saturating_sub(*s) < 60);
        }
        let e = m.entry(*sender).or_insert((t, 0));
        if t.saturating_sub(e.0) >= 60 {
            *e = (t, 0);
        }
        e.1 += 1;
        if e.1 > 6 {
            return;
        }
    }
    let n = receive_packet(&mut store_cell().lock().unwrap_or_else(|e| e.into_inner()), plaintext, t);
    if n > 0 {
        println!("[relay] принято записей о доступности: {n}");
    }
}

/// Состояние для страницы и проверок: клиент ли этот узел, сколько записей о доступности известно.
pub fn router<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
    async fn status() -> axum::Json<serde_json::Value> {
        let n = store_cell().lock().unwrap_or_else(|e| e.into_inner()).total;
        axum::Json(serde_json::json!({"client": is_client(), "relay_enabled": relay_enabled(), "records": n}))
    }
    axum::Router::new().route("/api/network/relay", axum::routing::get(status))
}

/// Запуск менеджера: решает, узел это или клиент; клиент держит связь с двумя ретрансляторами и объявляет запись.
pub fn start(transport: std::sync::Arc<crate::netlayer::transport::P2PTransport>, node_id: [u8; 32], key: SigningKey, forced_client: bool) {
    crate::supervisor::supervise("relay_manager", crate::supervisor::Policy::restart(), move || {
        let (transport, key) = (transport.clone(), key.clone());
        async move {
        let started = now();
        let my_hex = hex::encode(node_id);
        let mut last_pub = 0u64;
        let mut last_relays: Vec<String> = vec![];
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            let t = now();
            let peers = transport.get_peers().await;
            let connected: std::collections::HashSet<String> = peers.iter().map(|p| p.id.to_hex()).collect();
            let demote = forced_client || (t.saturating_sub(started) >= DEMOTE_AFTER_SECS && connected.len() >= 2 && crate::reachability::inbound_probes() == 0);
            if demote != is_client() {
                client_flag().store(demote, Ordering::Relaxed);
                crate::network_offers::set_published(!demote);
                println!("[relay] узел теперь {}", if demote { "КЛИЕНТ: до него не достучаться снаружи, он доступен через ретрансляторы" } else { "узел сети" });
            }
            if demote {
                // ретрансляторы: уже связанные остаются, недостающие подбираются из проверенных карточек
                let (cards, _) = crate::network_offers::directory_snapshot(None);
                let mut relays: Vec<String> = last_relays.iter().filter(|r| connected.contains(*r)).cloned().collect();
                for c in cards.iter().filter(|c| c.relay && c.node_id != my_hex) {
                    if relays.len() >= WANT_RELAYS {
                        break;
                    }
                    if relays.contains(&c.node_id) {
                        continue;
                    }
                    if connected.contains(&c.node_id) {
                        relays.push(c.node_id.clone());
                    } else if let Some(a) = c.addr.first() {
                        let _ = transport.send_hello_request(a).await;
                    }
                }
                if relays != last_relays || t.saturating_sub(last_pub) >= REPUBLISH_SECS {
                    if !relays.is_empty() {
                        let rec = ViaRecord { v: 1, node_id: my_hex.clone(), key: String::new(), relays: relays.clone(), issued: t, expires: t + VIA_TTL_SECS, sig: String::new() }.sign(&key);
                        store_cell().lock().unwrap_or_else(|e| e.into_inner()).accept(rec, t);
                        println!("[relay] объявляю доступность через {} ретранслятор(а)", relays.len());
                        last_pub = t;
                    }
                    last_relays = relays;
                }
            }
            // раздача записей соседям
            let batch = store_cell().lock().unwrap_or_else(|e| e.into_inner()).sample(GOSSIP_BATCH, t);
            if !batch.is_empty() {
                let pkt = encode_packet(&batch);
                for p in peers {
                    let _ = transport.send_encrypted(p.id, &pkt).await;
                }
            }
            store_cell().lock().unwrap_or_else(|e| e.into_inner()).forget_expired(t);
            tokio::time::sleep(std::time::Duration::from_secs(if t.saturating_sub(started) < 600 { 30 } else { 60 })).await;
        }
    }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    /// the node id of "node n": derived from its key, as every real node id is
    fn id(n: u8) -> String {
        hex::encode(crate::util::types::derive_node_id(&key(n).verifying_key().to_bytes()))
    }
    fn rec(n: u8, relays: &[u8], issued: u64) -> ViaRecord {
        ViaRecord { v: 1, node_id: id(n), key: String::new(), relays: relays.iter().map(|r| id(*r)).collect(), issued, expires: issued + VIA_TTL_SECS, sig: String::new() }.sign(&key(n))
    }

    #[test]
    fn a_bound_node_id_cannot_be_claimed_with_another_key_in_a_via_record() {
        let victim = key(51);
        let vid = hex::encode(crate::util::types::derive_node_id(&victim.verifying_key().to_bytes()));
        let mk = |k: &SigningKey| ViaRecord { v: 1, node_id: vid.clone(), key: String::new(), relays: vec![id(2)], issued: NOW, expires: NOW + VIA_TTL_SECS, sig: String::new() }.sign(k);
        assert_eq!(mk(&victim).check(NOW), Ok(()));
        assert_eq!(mk(&key(52)).check(NOW), Err("id not bound to key"));
    }

    #[test]
    fn a_full_store_refuses_new_nodes_without_creating_empty_buckets() {
        let mut st = ViaStore::default();
        st.total = MAX_RECORDS;
        for n in 1..20u8 {
            assert_eq!(st.accept(rec(n, &[100], NOW), NOW), Taken::Refused("full"));
        }
        assert!(st.by_node.is_empty());
    }

    #[test]
    fn a_record_is_signed_by_the_node_and_any_change_or_forgery_is_refused() {
        let r = rec(1, &[2, 3], NOW);
        assert_eq!(r.check(NOW), Ok(()));
        for tamper in [|r: &mut ViaRecord| r.relays = vec![hex::encode([9u8; 32])], |r: &mut ViaRecord| r.expires -= 1] {
            let mut t = r.clone();
            tamper(&mut t);
            assert_eq!(t.check(NOW), Err("signature"));
        }
        let mut moved = r.clone();
        moved.node_id = hex::encode([7u8; 32]);
        assert_eq!(moved.check(NOW), Err("id not bound to key"), "a record cannot carry an id that is not derived from its key");
        assert_eq!(rec(1, &[], NOW).check(NOW), Err("relays"));
        assert_eq!(rec(1, &[1], NOW).check(NOW), Err("relays"), "a node is not its own relay");
        assert_eq!(rec(1, &[2], NOW).check(NOW + VIA_TTL_SECS), Err("time"), "expired");
        assert_eq!(rec(1, &[2], NOW + 3600).check(NOW), Err("time"), "from the future");
        let mut long = rec(1, &[2], NOW);
        long.expires = NOW + 10 * VIA_TTL_SECS;
        assert_eq!(long.sign(&key(1)).check(NOW), Err("time"));
    }

    #[test]
    fn the_store_keeps_the_newest_record_per_key_and_the_asker_picks_by_the_key_he_knows() {
        let mut s = ViaStore::default();
        let a = rec(1, &[2], NOW);
        assert_eq!(s.accept(a.clone(), NOW), Taken::New);
        assert_eq!(s.accept(a.clone(), NOW), Taken::Stale);
        assert_eq!(s.accept(rec(1, &[3], NOW + 60), NOW + 60), Taken::Newer);
        let real_key = hex::encode(key(1).verifying_key().to_bytes());
        assert_eq!(s.lookup(&id(1), &real_key, NOW + 60).unwrap().relays, vec![id(3)]);
        // самозванец публикует запись под номером узла 1 со своим ключом: она хранится, но найти по настоящему ключу её нельзя
        let fake = ViaRecord { v: 1, node_id: id(1), key: String::new(), relays: vec![id(8)], issued: NOW + 120, expires: NOW + 120 + VIA_TTL_SECS, sig: String::new() }.sign(&key(66));
        assert_eq!(s.accept(fake, NOW + 120), Taken::Refused("id not bound to key"), "an id cannot be taken with another key");
        assert_eq!(s.lookup(&id(1), &real_key, NOW + 120).unwrap().relays, vec![id(3)], "the forger cannot redirect the owner's devices");
        assert!(s.lookup(&id(1), &real_key, NOW + 60 + VIA_TTL_SECS).is_none(), "expired");
        // чужих записей под одним номером — не больше четырёх
        for k in 100..110u8 {
            let f = ViaRecord { v: 1, node_id: id(1), key: String::new(), relays: vec![id(8)], issued: NOW + 200 + k as u64, expires: NOW + 200 + k as u64 + VIA_TTL_SECS, sig: String::new() }.sign(&key(k));
            s.accept(f, NOW + 300);
        }
        assert!(s.by_node[&id(1)].len() <= PER_NODE);
    }

    #[test]
    fn a_gossip_packet_survives_link_padding_and_hostile_bytes() {
        let mut s = ViaStore::default();
        let mut p = encode_packet(&[rec(1, &[2], NOW), rec(2, &[3], NOW)]);
        p.extend_from_slice(&[0u8; 11]);
        assert_eq!(receive_packet(&mut s, &p, NOW), 2);
        assert_eq!(receive_packet(&mut s, &p, NOW), 0, "a repeat adds nothing");
        assert_eq!(receive_packet(&mut s, b"\xDDnot json", NOW), 0);
        assert_eq!(receive_packet(&mut s, &[0xD8, b'[', b']'], NOW), 0);
        assert_eq!(s.sample(10, NOW).len(), 2);
        s.forget_expired(NOW + VIA_TTL_SECS + 1);
        assert_eq!(s.sample(10, NOW + VIA_TTL_SECS + 1).len(), 0);
    }
}
