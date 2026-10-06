//! Рукопожатие с одноразовыми ключами и хранилище сеансов — общая часть обеих сетей (основной сети узлов и канала переписки).
//!
//! На каждое приветствие обе стороны делают СВОЙ одноразовый X25519-ключ; ключ сеанса = ECDH(одноразовый ⨯ одноразовый). Секреты не хранятся дольше рукопожатия,
//! поэтому кража долгоживущего ключа не раскрывает прошлый трафик (прямая секретность). Пакеты шифрует `crypto::store`.
use crate::crypto::store::SessionStore;
use crate::crypto::x25519::{derive_master, SecretKey};
use crate::util::HashId;
use hkdf::Hkdf;
use sha2::Sha256;
use std::collections::HashMap;

/// Два рукопожатия с одним узлом в пределах этого окна с разных сторон — встречное знакомство.
const CROSSING_WINDOW_MS: u128 = 10_000;
/// Сколько неотвеченных приглашений (секретов одноразовых ключей) хранить.
const MAX_PENDING_EPHEMERALS: usize = 64;

pub struct HandshakeManager {
    store: SessionStore,
    our_id: HashId,
    /// Секреты одноразовых ключей для исходящих приглашений: номер приглашения → секрет (затирается при удалении).
    pending_ephemerals: HashMap<u64, SecretKey>,
}

impl Default for HandshakeManager {
    fn default() -> Self {
        let mut our_id = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut our_id);
        Self::new(HashId(our_id))
    }
}

impl HandshakeManager {
    pub fn new(our_id: HashId) -> Self {
        Self { store: SessionStore::new(our_id), our_id, pending_ephemerals: HashMap::new() }
    }

    // ── Рукопожатие с одноразовыми ключами (прямая секретность) ─────────────────

    /// Инициатор: свежий одноразовый ключ для приглашения с этим номером. Возвращает публичный ключ для пакета-приглашения.
    pub fn generate_hello_ephemeral(&mut self, nonce: u64) -> [u8; 32] {
        let key = SecretKey::generate();
        let public = key.public;
        // стук к узлу, который не отвечает, повторяется каждые 30 с — неотвеченные секреты не копятся бесконечно
        if self.pending_ephemerals.len() >= MAX_PENDING_EPHEMERALS {
            if let Some(oldest) = self.pending_ephemerals.keys().min().copied() {
                self.pending_ephemerals.remove(&oldest);
            }
        }
        self.pending_ephemerals.insert(nonce, key);
        public
    }

    /// Инициатор: пришёл ответ. Секрет приглашения используется один раз и стирается.
    pub fn complete_hello_initiator(&mut self, request_nonce: u64, peer_id: HashId, their_pub: &[u8; 32]) -> Result<u64, String> {
        let key = self.pending_ephemerals.remove(&request_nonce).ok_or_else(|| format!("[PFS] No pending ephemeral for nonce {}", request_nonce))?;
        let shared = key.diffie_hellman(their_pub)?;
        let master = derive_master(&shared);
        drop(key);
        let us = self.our_id;
        self.store_session(peer_id, master, us, false)
    }

    /// Ответчик: свой одноразовый ключ, общий секрет сразу, сеанс сохранён. Возвращает публичный ключ для ответа.
    pub fn complete_hello_responder(&mut self, peer_id: HashId, their_pub: &[u8; 32]) -> Result<[u8; 32], String> {
        let key = SecretKey::generate();
        let our_pub = key.public;
        let shared = key.diffie_hellman(their_pub)?;
        let master = derive_master(&shared);
        drop(key); // секрет не хранится
        self.store_session(peer_id, master, peer_id, true)?;
        Ok(our_pub)
    }

    /// Сохранить ключ нового рукопожатия.
    ///
    /// Встречное знакомство (оба узла постучались друг к другу почти одновременно): получаются ДВА рукопожатия и два разных ключа; обе стороны по одному правилу
    /// оставляют ключ рукопожатия, начатого узлом с меньшим номером. В остальных случаях новое рукопожатие заменяет ключ: повтор старого приветствия отсекается
    /// раньше (одноразовые номера приветствий), а запрет на несколько минут не давал бы перезапущенному узлу снова договориться о ключе.
    fn store_session(&mut self, peer_id: HashId, master: [u8; 32], initiator: HashId, responder: bool) -> Result<u64, String> {
        if let Some(existing) = self.store.get(&peer_id) {
            let crossing = existing.age_ms() < CROSSING_WINDOW_MS && existing.initiator.map(|i| i != initiator).unwrap_or(false);
            if crossing {
                let winner = if self.our_id.0 < peer_id.0 { self.our_id } else { peer_id };
                if initiator != winner {
                    // проигравший ключ не становится рабочим, но на время годится для расшифровки: другая сторона могла уже отправить с ним пакеты
                    self.store.add_fallback(peer_id, master);
                    return Err("crossing handshake: the other key is kept".to_string());
                }
            }
        }
        if responder {
            // мы ответчик: новый ключ становится рабочим для отправки только после первого пакета, зашифрованного им
            Ok(self.store.install_responder(peer_id, master, initiator))
        } else {
            Ok(self.store.install_initiator(peer_id, master))
        }
    }

    /// Восстановить сеанс из сохранённого общего ключа (возобновление по QR-спариванию). Временная мера: ключ на диске не даёт прямой секретности;
    /// возобновление через новый обмен ключами — в плане (docs/CRYPTO_MAP.md, H3).
    pub fn restore_session(&mut self, peer_id: HashId, master: [u8; 32]) -> u64 {
        self.store.install(peer_id, master, None)
    }

    /// Общий ключ сеанса с узлом (для записи возобновления).
    pub fn session_key_bytes(&self, peer_id: &HashId) -> Option<[u8; 32]> {
        self.store.master_bytes(peer_id)
    }

    /// Ключ только для одной передачи файла: выводится из ключа сеанса и номера файла, чтобы передачи не делили ключ.
    pub fn derive_file_key(&self, peer_id: &HashId, file_id: &str) -> Option<[u8; 32]> {
        let master = self.store.master_bytes(peer_id)?;
        let hk = Hkdf::<Sha256>::new(Some(file_id.as_bytes()), &master);
        let mut file_key = [0u8; 32];
        hk.expand(b"yandi-file-transfer-v1", &mut file_key).expect("HKDF expand");
        Some(file_key)
    }

    pub fn has_session(&self, peer_id: &HashId) -> bool {
        self.store.has(peer_id)
    }

    pub fn has_session_by_id(&self, peer_id: &HashId) -> bool {
        self.store.has(peer_id)
    }

    /// Нужно ли договориться о новом ключе с этим узлом.
    pub fn needs_rekey(&self, peer_id: &HashId) -> bool {
        self.store.needs_rekey(peer_id)
    }

    pub fn encrypt(&self, peer_id: &HashId, data: &[u8]) -> Result<Vec<u8>, String> {
        self.store.encrypt(peer_id, data)
    }

    pub fn decrypt(&mut self, peer_id: &HashId, data: &[u8]) -> Result<Vec<u8>, String> {
        self.store.decrypt_from(peer_id, data)
    }

    /// Номер отправителя из заголовка пакета, не расшифровывая.
    pub fn extract_peer_id(data: &[u8]) -> Result<HashId, String> {
        SessionStore::extract_sender(data)
    }

    pub fn decrypt_by_peer_id(&mut self, data: &[u8]) -> Result<(HashId, Vec<u8>), String> {
        self.store.decrypt_by_sender(data)
    }

    pub fn cleanup_stale_sessions(&mut self) {
        let removed = self.store.cleanup_idle();
        if removed > 0 {
            println!("[encryption] 🧹 removed {} idle sessions, {} active", removed, self.store.len());
        }
    }

    pub fn remove_session(&mut self, peer_id: &HashId) {
        self.store.remove(peer_id);
    }

    pub fn session_count(&self) -> usize {
        self.store.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NodeIdentity;

    /// Встречное знакомство в любом порядке прихода пакетов: у обеих сторон — ОДИН ключ (рукопожатие меньшего номера);
    /// потом новое рукопожатие (перезапуск узла) снова договаривается о ключе, без запрета.
    #[test]
    fn crossing_handshakes_end_with_one_shared_key_in_every_arrival_order() {
        for order in 0..4 {
            let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
            let (a, b) = (ia.node_id(), ib.node_id());
            let mut ma = HandshakeManager::new(a);
            let mut mb = HandshakeManager::new(b);
            let req_a = ma.generate_hello_ephemeral(1);
            let req_b = mb.generate_hello_ephemeral(2);
            let (ack_b, ack_a);
            if order & 1 == 0 {
                ack_b = mb.complete_hello_responder(a, &req_a).ok();
                ack_a = ma.complete_hello_responder(b, &req_b).ok();
            } else {
                ack_a = ma.complete_hello_responder(b, &req_b).ok();
                ack_b = mb.complete_hello_responder(a, &req_a).ok();
            }
            let finish_a = |ma: &mut HandshakeManager| if let Some(p) = ack_b { let _ = ma.complete_hello_initiator(1, b, &p); };
            let finish_b = |mb: &mut HandshakeManager| if let Some(p) = ack_a { let _ = mb.complete_hello_initiator(2, a, &p); };
            if order & 2 == 0 { finish_a(&mut ma); finish_b(&mut mb); } else { finish_b(&mut mb); finish_a(&mut ma); }
            let ct = ma.encrypt(&b, b"one key").unwrap();
            assert_eq!(mb.decrypt_by_peer_id(&ct).unwrap().1, b"one key", "order {order}");
            let back = mb.encrypt(&a, b"both ways").unwrap();
            assert_eq!(ma.decrypt_by_peer_id(&back).unwrap().1, b"both ways", "order {order}");
            let winner = if a.0 < b.0 { a } else { b };
            assert_eq!(ma.store.get(&b).unwrap().initiator, Some(winner));
            assert_eq!(mb.store.get(&a).unwrap().initiator, Some(winner));
        }
        // перезапуск: B потерял ключ и знакомится заново сразу же — A принимает новое рукопожатие
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut ma = HandshakeManager::new(a);
        let mut mb = HandshakeManager::new(b);
        let p = mb.generate_hello_ephemeral(5);
        let q = ma.complete_hello_responder(b, &p).unwrap();
        mb.complete_hello_initiator(5, a, &q).unwrap();
        let mut mb2 = HandshakeManager::new(b);
        let p2 = mb2.generate_hello_ephemeral(6);
        let q2 = ma.complete_hello_responder(b, &p2).expect("a restarted peer can agree on a new key at once");
        mb2.complete_hello_initiator(6, a, &q2).unwrap();
        let ct = mb2.encrypt(&a, b"after restart").unwrap();
        assert_eq!(ma.decrypt_by_peer_id(&ct).unwrap().1, b"after restart");
    }

    /// Смена ключа в работающем сеансе: ответчик не должен писать новым ключом, пока инициатор его не получил
    /// (на живой сети пакет обогнал подтверждение, и сообщение пропало). Одна сторона меняет ключ — и обе сразу, в любом порядке событий.
    #[test]
    fn rekey_loses_nothing_in_any_order_of_events() {
        // события: 0 = B обработал запрос A, 1 = A обработал запрос B, 2 = A получил ответ B, 3 = B получил ответ A
        fn perms(v: &mut Vec<usize>, used: u8, out: &mut Vec<Vec<usize>>) {
            if v.len() == 4 { out.push(v.clone()); return; }
            for e in 0..4 { if used & (1 << e) == 0 { v.push(e); perms(v, used | (1 << e), out); v.pop(); } }
        }
        let mut all = vec![];
        perms(&mut vec![], 0, &mut all);
        let mut checked = 0;
        for (both, a_lower) in [(false, true), (false, false), (true, true), (true, false)] {
            for order in &all {
                let pos = |e: usize| order.iter().position(|x| *x == e).unwrap();
                if pos(2) < pos(0) || pos(3) < pos(1) { continue; } // ответ не приходит раньше запроса
                if !both && (order.contains(&1) && false) { continue; }
                let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
                let (mut a, mut b) = (ia.node_id(), ib.node_id());
                if (a.0 < b.0) != a_lower { std::mem::swap(&mut a, &mut b); }
                let (mut ma, mut mb) = (HandshakeManager::new(a), HandshakeManager::new(b));
                // обычное знакомство, потом ключ «стареет»; несколько смен подряд — чтобы накопились прежние ключи (на живой сети проигравший
                // ключ однажды вытолкнул только что отложенный прежний, и пакеты в пути пропали)
                for round in 0..3u64 {
                    let p = mb.generate_hello_ephemeral(100 + round);
                    let q = ma.complete_hello_responder(b, &p).unwrap();
                    mb.complete_hello_initiator(100 + round, a, &q).unwrap();
                    // подтверждение пакетом, как в работе
                    let c = mb.encrypt(&a, b"confirm").unwrap();
                    ma.decrypt_by_peer_id(&c).unwrap();
                    let c = ma.encrypt(&b, b"confirm").unwrap();
                    mb.decrypt_by_peer_id(&c).unwrap();
                    ma.store.backdate(&b, 60_000);
                    mb.store.backdate(&a, 60_000);
                }
                let req_a = ma.generate_hello_ephemeral(1);
                let req_b = mb.generate_hello_ephemeral(2);
                let (mut ack_b, mut ack_a) = (None, None);
                let mut n = 0;
                let mut talk = |ma: &mut HandshakeManager, mb: &mut HandshakeManager, when: &str| {
                    n += 1;
                    let m1 = format!("a→b {n}");
                    let c = ma.encrypt(&b, m1.as_bytes()).unwrap();
                    assert_eq!(mb.decrypt_by_peer_id(&c).unwrap_or_else(|e| panic!("{when}: a→b не прочитано: {e} (порядок {order:?}, обе={both}, a меньше={a_lower})")).1, m1.as_bytes());
                    let m2 = format!("b→a {n}");
                    let c = mb.encrypt(&a, m2.as_bytes()).unwrap();
                    assert_eq!(ma.decrypt_by_peer_id(&c).unwrap_or_else(|e| panic!("{when}: b→a не прочитано: {e} (порядок {order:?}, обе={both}, a меньше={a_lower})")).1, m2.as_bytes());
                };
                talk(&mut ma, &mut mb, "до смены ключа");
                for e in order {
                    match e {
                        0 => ack_b = mb.complete_hello_responder(a, &req_a).ok(),
                        1 if both => ack_a = ma.complete_hello_responder(b, &req_b).ok(),
                        1 => {}
                        2 => { if let Some(x) = ack_b { let _ = ma.complete_hello_initiator(1, b, &x); } }
                        _ => { if let Some(x) = ack_a { let _ = mb.complete_hello_initiator(2, a, &x); } }
                    }
                    talk(&mut ma, &mut mb, &format!("после события {e}"));
                }
                // и после всего обмен идёт в обе стороны несколько раз
                for _ in 0..3 { talk(&mut ma, &mut mb, "в конце"); }
                // прошло больше 30 с: запасные ключи истекли — связь держится на рабочих ключах, они обязаны совпасть
                ma.store.expire_old_keys(&b);
                mb.store.expire_old_keys(&a);
                for _ in 0..3 { talk(&mut ma, &mut mb, "после истечения запасных ключей"); }
                checked += 1;
            }
        }
        assert!(checked >= 12, "проверено порядков: {checked}");
    }

    #[test]
    fn unanswered_invitations_do_not_pile_up() {
        let mut m = HandshakeManager::new(HashId([1u8; 32]));
        for n in 0..1000u64 {
            m.generate_hello_ephemeral(n);
        }
        assert_eq!(m.pending_ephemerals.len(), MAX_PENDING_EPHEMERALS);
        assert!(m.pending_ephemerals.contains_key(&999), "the newest are kept");
    }

    /// Как в живом канале: рукопожатие (инициатор/ответчик), шифрование, расшифровка по отправителю из пакета — ровно исходные данные любого размера до предела.
    #[test]
    fn the_live_path_pfs_handshake_then_decrypt_by_sender_returns_exactly_the_data() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut ma = HandshakeManager::new(a);
        let mut mb = HandshakeManager::new(b);
        let pub_a = ma.generate_hello_ephemeral(7);
        let pub_b = mb.complete_hello_responder(a, &pub_a).unwrap();
        ma.complete_hello_initiator(7, b, &pub_b).unwrap();
        for len in [0usize, 1, 63, 64, 65, 1000, 65_000, u16::MAX as usize, 200_000] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let ct = ma.encrypt(&b, &data).unwrap();
            let (from, got) = mb.decrypt_by_peer_id(&ct).unwrap();
            assert_eq!((from, got.len()), (a, len));
            assert_eq!(got, data, "len {len}");
            let back = mb.encrypt(&a, &data).unwrap();
            assert_eq!(ma.decrypt_by_peer_id(&back).unwrap().1, data);
        }
        assert!(ma.encrypt(&b, &vec![0u8; crate::crypto::store::MAX_PLAINTEXT + 1]).unwrap_err().contains("too large"));
        let mut ct = ma.encrypt(&b, b"hello").unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 1;
        assert!(mb.decrypt_by_peer_id(&ct).is_err());
    }

    /// Прямая секретность: секрет одноразового ключа не остаётся в менеджере после рукопожатия.
    #[test]
    fn handshake_secrets_are_not_kept() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut ma = HandshakeManager::new(a);
        let mut mb = HandshakeManager::new(b);
        let pub_a = ma.generate_hello_ephemeral(1);
        assert_eq!(ma.pending_ephemerals.len(), 1);
        let pub_b = mb.complete_hello_responder(a, &pub_a).unwrap();
        ma.complete_hello_initiator(1, b, &pub_b).unwrap();
        assert!(ma.pending_ephemerals.is_empty(), "секрет приглашения использован один раз и удалён");
        // повтор того же ответа не работает: секрета больше нет
        assert!(ma.complete_hello_initiator(1, b, &pub_b).is_err());
    }

    /// Два рукопожатия между теми же узлами дают разные ключи (одноразовость): запись одного разговора не раскрывается другим.
    #[test]
    fn two_handshakes_give_different_keys() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut keys = vec![];
        for n in 0..2 {
            let mut ma = HandshakeManager::new(a);
            let mut mb = HandshakeManager::new(b);
            let pa = ma.generate_hello_ephemeral(n);
            let pb = mb.complete_hello_responder(a, &pa).unwrap();
            ma.complete_hello_initiator(n, b, &pb).unwrap();
            keys.push(ma.store.master_bytes(&b).unwrap());
        }
        assert_ne!(keys[0], keys[1]);
    }

    #[test]
    fn a_weak_ephemeral_key_is_refused_in_both_roles() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut ma = HandshakeManager::new(a);
        let mut mb = HandshakeManager::new(b);
        assert!(mb.complete_hello_responder(a, &[0u8; 32]).is_err());
        assert!(!mb.has_session(&a));
        ma.generate_hello_ephemeral(1);
        assert!(ma.complete_hello_initiator(1, b, &[0u8; 32]).is_err());
        assert!(!ma.has_session(&b));
    }

    #[test]
    fn file_keys_are_per_file_and_per_session() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut ma = HandshakeManager::new(a);
        let mut mb = HandshakeManager::new(b);
        let pa = ma.generate_hello_ephemeral(1);
        let pb = mb.complete_hello_responder(a, &pa).unwrap();
        ma.complete_hello_initiator(1, b, &pb).unwrap();
        let (k1, k2) = (ma.derive_file_key(&b, "file-1").unwrap(), ma.derive_file_key(&b, "file-2").unwrap());
        assert_ne!(k1, k2);
        assert_eq!(k1, mb.derive_file_key(&a, "file-1").unwrap(), "обе стороны получают один и тот же ключ файла");
        assert!(ma.derive_file_key(&HashId([9; 32]), "file-1").is_none());
    }

    /// Сеанс из сохранённого ключа (возобновление) шифрует и расшифровывает, как обычный.
    #[test]
    fn a_restored_session_works() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut ma = HandshakeManager::new(a);
        let mut mb = HandshakeManager::new(b);
        let pa = ma.generate_hello_ephemeral(1);
        let pb = mb.complete_hello_responder(a, &pa).unwrap();
        ma.complete_hello_initiator(1, b, &pb).unwrap();
        let (ka, kb) = (ma.session_key_bytes(&b).unwrap(), mb.session_key_bytes(&a).unwrap());
        assert_eq!(ka, kb, "ключ сеанса симметричен");
        let mut ma2 = HandshakeManager::new(a);
        let mut mb2 = HandshakeManager::new(b);
        ma2.restore_session(b, ka);
        mb2.restore_session(a, kb);
        let ct = ma2.encrypt(&b, b"resumed").unwrap();
        assert_eq!(mb2.decrypt(&a, &ct).unwrap(), b"resumed");
    }

    #[test]
    fn the_same_packet_twice_is_refused_and_a_new_one_is_accepted() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a, b) = (ia.node_id(), ib.node_id());
        let mut ma = HandshakeManager::new(a);
        let mut mb = HandshakeManager::new(b);
        let pa = ma.generate_hello_ephemeral(1);
        let pb = mb.complete_hello_responder(a, &pa).unwrap();
        ma.complete_hello_initiator(1, b, &pb).unwrap();
        let captured = ma.encrypt(&b, b"chat: hey, got a minute?").unwrap();
        assert!(mb.decrypt_by_peer_id(&captured).is_ok(), "the real, first delivery must be accepted");
        let replay = mb.decrypt_by_peer_id(&captured);
        assert!(replay.unwrap_err().contains("REPLAY"));
        let second_real = ma.encrypt(&b, b"chat: still there?").unwrap();
        assert_ne!(captured, second_real);
        assert!(mb.decrypt_by_peer_id(&second_real).is_ok());
    }
}
