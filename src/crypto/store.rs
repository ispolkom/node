//! Хранилище сеансов: ключи сеансов с узлами и пакетный формат поверх `SessionCrypto`.
//!
//! Пакет: `[номер отправителя:32][одноразовый номер:12][шифртекст с тегом]`. Внутри шифртекста: `[длина:4 LE][данные][заполнитель]` (заполнитель скрывает
//! точный размер коротких сообщений). Номер отправителя идёт открыто, но входит в проверку подлинности (AAD).
use crate::crypto::session::{CryptoError, SessionCrypto};
use crate::util::HashId;
use rand::Rng;
use std::collections::HashMap;
use std::time::Instant;

/// Больше открытых данных в один пакет не шифруется (отказ, а не молча испорченное сообщение).
pub const MAX_PLAINTEXT: usize = 16 * 1024 * 1024;
/// Сеанс без движения дольше этого времени удаляется.
pub const IDLE_LIMIT_MS: u128 = 300_000;

/// Сколько секунд прежний ключ ещё принимает пакеты после смены (пакеты, отправленные до смены ключа, доходят уже после неё).
pub const FALLBACK_SECS: u64 = 30;
/// Сколько прежних ключей держим одновременно.
const MAX_FALLBACKS: usize = 4;

pub struct Fallback {
    pub crypto: SessionCrypto,
    pub until: Instant,
    /// кто начал рукопожатие этого ключа
    pub initiator: Option<HashId>,
    /// ключ победителя встречной смены, который мы ещё не успели сделать рабочим: первый же пакет, расшифрованный им, делает его рабочим
    pub unconfirmed: bool,
}

/// Ключ, который принят от инициатора рукопожатия, но ещё не подтверждён: расшифровывать им можно, отправлять — нет.
/// Инициатор получит наш ответ позже, чем мы могли бы начать писать; пока не пришёл пакет, зашифрованный новым ключом,
/// продолжаем отправлять прежним (иначе пакет обгонит ответ, и инициатор не сможет его прочитать).
pub struct Pending {
    pub crypto: SessionCrypto,
    pub initiator: HashId,
    pub until: Instant,
}

pub struct Entry {
    pub crypto: SessionCrypto,
    /// Новый ключ ответчика до первого пакета, зашифрованного им.
    pub pending: Option<Pending>,
    /// Прежние ключи только для расшифровки, пока не истекло время: смена ключа без потери пакетов в пути.
    pub fallbacks: Vec<Fallback>,
    pub version: u64,
    pub created_at: Instant,
    pub last_used: Instant,
    /// кто начал рукопожатие, из которого этот ключ (нужно, чтобы при встречном знакомстве обе стороны выбрали ОДИН ключ)
    pub initiator: Option<HashId>,
}

impl Entry {
    pub fn age_ms(&self) -> u128 {
        self.created_at.elapsed().as_millis()
    }
    pub fn idle_ms(&self) -> u128 {
        self.last_used.elapsed().as_millis()
    }
}

/// Most peers with a live encrypted session at once.
pub const MAX_SESSIONS: usize = 4096;

pub struct SessionStore {
    our_id: HashId,
    sessions: HashMap<HashId, Entry>,
    counter: u64,
}

impl SessionStore {
    pub fn new(our_id: HashId) -> Self {
        Self { our_id, sessions: HashMap::new(), counter: 0 }
    }

    pub fn our_id(&self) -> HashId {
        self.our_id
    }

    /// Поставить (или заменить) сеанс с узлом. Возвращает номер версии сеанса.
    pub fn install(&mut self, peer: HashId, master: [u8; 32], initiator: Option<HashId>) -> u64 {
        self.counter += 1;
        let now = Instant::now();
        let mut entry = Entry { crypto: SessionCrypto::from_master(master, &self.our_id.0, &peer.0), pending: None, fallbacks: Vec::new(), version: self.counter, created_at: now, last_used: now, initiator };
        // прежний ключ остаётся на время только для расшифровки
        let winner = if self.our_id.0 < peer.0 { self.our_id } else { peer };
        if let Some(old) = self.sessions.remove(&peer) {
            let mut keep: Vec<Fallback> = old.fallbacks.into_iter().filter(|f| f.until > now).collect();
            keep.insert(0, Fallback { crypto: old.crypto, until: now + std::time::Duration::from_secs(FALLBACK_SECS), initiator: old.initiator, unconfirmed: false });
            // неподтверждённый ключ другой стороны тоже остаётся для расшифровки
            if let Some(p) = old.pending.filter(|p| p.until > now) {
                keep.insert(0, Fallback { crypto: p.crypto, until: p.until, initiator: Some(p.initiator), unconfirmed: p.initiator == winner });
            }
            keep.truncate(MAX_FALLBACKS);
            entry.fallbacks = keep;
        }
        eprintln!("[keys] {}: ключ v{} рабочий (начал {}), прежних для чтения: {}", short(&peer), entry.version, initiator.map(|i| short(&i)).unwrap_or_else(|| "-".into()), entry.fallbacks.len());
        // The number of sessions is bounded (identities are free, every new one would otherwise keep keys and a replay window
        // for ever): when the table is full the session unused for the longest time makes room.
        if self.sessions.len() >= MAX_SESSIONS && !self.sessions.contains_key(&peer) {
            if let Some(oldest) = self.sessions.iter().min_by_key(|(_, e)| e.last_used).map(|(id, _)| *id) {
                self.sessions.remove(&oldest);
            }
        }
        self.sessions.insert(peer, entry);
        self.counter
    }

    /// Ключ рукопожатия, начатого нами (пришёл ответ). Если одновременно другая сторона начала своё и её ключ ждёт подтверждения (встречная смена),
    /// рабочим становится ключ того, у кого МЕНЬШЕ номер: победитель пишет своим ключом, проигравший ждёт первого пакета победителя и переходит на него.
    /// Иначе каждый писал бы своим ключом, а чужой читал бы лишь как запасной, и через 30 секунд связь замолкала бы.
    pub fn install_initiator(&mut self, peer: HashId, master: [u8; 32]) -> u64 {
        let our = self.our_id;
        let now = Instant::now();
        if our.0 > peer.0 {
            if let Some(e) = self.sessions.get_mut(&peer) {
                if e.pending.as_ref().map(|p| p.until > now && p.initiator == peer).unwrap_or(false) {
                    e.fallbacks.retain(|f| f.until > now);
                    e.fallbacks.insert(0, Fallback { crypto: SessionCrypto::from_master(master, &our.0, &peer.0), until: now + std::time::Duration::from_secs(FALLBACK_SECS), initiator: Some(our), unconfirmed: false });
                    e.fallbacks.truncate(MAX_FALLBACKS);
                    eprintln!("[keys] {}: встречная смена, победил ключ собеседника — переходим на него по его первому пакету", short(&peer));
                    return e.version;
                }
            }
        }
        self.install(peer, master, Some(our))
    }

    /// Ключ рукопожатия, начатого другой стороной (мы ответчик). Если сеанса ещё нет — он сразу рабочий; если есть — «ожидающий»:
    /// им читаем, а пишем прежним, пока не придёт пакет, зашифрованный новым (тогда он становится рабочим).
    pub fn install_responder(&mut self, peer: HashId, master: [u8; 32], initiator: HashId) -> u64 {
        let our = self.our_id;
        let now = Instant::now();
        if let Some(e) = self.sessions.get_mut(&peer) {
            let until = now + std::time::Duration::from_secs(FALLBACK_SECS);
            e.fallbacks.retain(|f| f.until > now);
            if let Some(old) = e.pending.take().filter(|p| p.until > now) {
                e.fallbacks.insert(0, Fallback { crypto: old.crypto, until: old.until, initiator: Some(old.initiator), unconfirmed: false });
                e.fallbacks.truncate(MAX_FALLBACKS);
            }
            e.pending = Some(Pending { crypto: SessionCrypto::from_master(master, &our.0, &peer.0), initiator, until });
            eprintln!("[keys] {}: новый ключ (начал {}) ждёт подтверждения; рабочий пока v{}", short(&peer), short(&initiator), e.version);
            return e.version;
        }
        self.install(peer, master, Some(initiator))
    }

    /// Принять ключ, который НЕ становится рабочим (проигравшее рукопожатие при встречном знакомстве), но на время годится для расшифровки:
    /// другая сторона могла успеть отправить пакеты именно с ним. Если сеанса с узлом нет, ключ не нужен.
    pub fn add_fallback(&mut self, peer: HashId, master: [u8; 32]) {
        eprintln!("[keys] {}: проигравший ключ оставлен только для чтения", short(&peer));
        let our = self.our_id;
        if let Some(e) = self.sessions.get_mut(&peer) {
            let now = Instant::now();
            e.fallbacks.retain(|f| f.until > now);
            // новые — в начало списка, как везде; лишними становятся самые старые (раньше проигравший ключ выталкивал только что отложенный прежний)
            e.fallbacks.insert(0, Fallback { crypto: SessionCrypto::from_master(master, &our.0, &peer.0), until: now + std::time::Duration::from_secs(FALLBACK_SECS), initiator: None, unconfirmed: false });
            e.fallbacks.truncate(MAX_FALLBACKS);
        }
    }

    /// Только для проверок: прошло больше 30 с — запасные и ожидающие ключи истекли.
    #[cfg(test)]
    pub fn expire_old_keys(&mut self, peer: &HashId) {
        if let Some(e) = self.sessions.get_mut(peer) {
            e.fallbacks.clear();
            e.pending = None;
        }
    }

    /// Только для проверок: сделать ключ «старым», не дожидаясь времени.
    #[cfg(test)]
    pub fn backdate(&mut self, peer: &HashId, ms: u64) {
        if let Some(e) = self.sessions.get_mut(peer) {
            e.created_at = Instant::now() - std::time::Duration::from_millis(ms);
        }
    }

    pub fn has(&self, peer: &HashId) -> bool {
        self.sessions.contains_key(peer)
    }

    pub fn get(&self, peer: &HashId) -> Option<&Entry> {
        self.sessions.get(peer)
    }

    pub fn touch(&mut self, peer: &HashId) {
        if let Some(e) = self.sessions.get_mut(peer) {
            e.last_used = Instant::now();
        }
    }

    /// Общий ключ сеанса (для сохранения возобновления).
    pub fn master_bytes(&self, peer: &HashId) -> Option<[u8; 32]> {
        self.sessions.get(peer).map(|e| e.crypto.master_bytes())
    }

    pub fn remove(&mut self, peer: &HashId) {
        self.sessions.remove(peer);
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Удалить сеансы без движения; возвращает, сколько удалено.
    pub fn cleanup_idle(&mut self) -> usize {
        let before = self.sessions.len();
        self.sessions.retain(|_, e| e.idle_ms() <= IDLE_LIMIT_MS);
        before - self.sessions.len()
    }

    /// Пора ли договориться о новом ключе с этим узлом.
    pub fn needs_rekey(&self, peer: &HashId) -> bool {
        self.sessions.get(peer).map(|e| e.crypto.needs_rekey()).unwrap_or(false)
    }

    /// Номер отправителя из заголовка пакета, не расшифровывая.
    pub fn extract_sender(data: &[u8]) -> Result<HashId, String> {
        if data.len() < 32 {
            return Err("Packet too short to extract sender_id".to_string());
        }
        let mut b = [0u8; 32];
        b.copy_from_slice(&data[..32]);
        Ok(HashId(b))
    }

    pub fn encrypt(&self, peer: &HashId, data: &[u8]) -> Result<Vec<u8>, String> {
        let e = self.sessions.get(peer).ok_or_else(|| format!("No session for peer: {}", hex::encode(&peer.0[..8])))?;
        if data.len() > MAX_PLAINTEXT {
            return Err(format!("Payload too large to encrypt: {} bytes (max {})", data.len(), MAX_PLAINTEXT));
        }
        let pad = if data.len() < 64 { rand::thread_rng().gen_range(8..=32) } else { rand::thread_rng().gen_range(0..=8) };
        let mut inner = Vec::with_capacity(4 + data.len() + pad);
        inner.extend_from_slice(&(data.len() as u32).to_le_bytes());
        inner.extend_from_slice(data);
        inner.extend(std::iter::repeat(0u8).take(pad));
        let (nonce, ct) = e.crypto.seal(&self.our_id.0, &inner).map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(32 + 12 + ct.len());
        out.extend_from_slice(&self.our_id.0);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Расшифровать пакет от известного отправителя (номер в заголовке обязан совпасть).
    pub fn decrypt_from(&mut self, peer: &HashId, data: &[u8]) -> Result<Vec<u8>, String> {
        if data.len() < 32 + 12 {
            return Err("Encrypted data too short (missing sender_id or nonce)".to_string());
        }
        let sender = Self::extract_sender(data)?;
        if sender != *peer {
            return Err(format!("Sender ID mismatch: expected {}, got {}", hex::encode(&peer.0[..8]), hex::encode(&sender.0[..8])));
        }
        self.open(&sender, data).map(|(_, p)| p)
    }

    /// Расшифровать пакет, найдя отправителя по заголовку.
    pub fn decrypt_by_sender(&mut self, data: &[u8]) -> Result<(HashId, Vec<u8>), String> {
        if data.len() < 32 + 12 {
            return Err("Encrypted data too short (missing nonce)".to_string());
        }
        let sender = Self::extract_sender(data)?;
        self.open(&sender, data)
    }

    fn open(&mut self, sender: &HashId, data: &[u8]) -> Result<(HashId, Vec<u8>), String> {
        let e = self.sessions.get_mut(sender).ok_or_else(|| format!("No session for sender_id: {}", hex::encode(&sender.0[..8])))?;
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&data[32..44]);
        let now = Instant::now();
        e.fallbacks.retain(|f| f.until > now);
        if e.pending.as_ref().map(|p| p.until <= now).unwrap_or(false) {
            e.pending = None;
        }
        let name = hex::encode(&sender.0[..8]);
        // «повтор» от одного ключа не значит, что пакет не подойдёт другому: у нового ключа свой счётчик, он начинается с малых номеров
        let mut replay = false;
        let first = e.crypto.open(&sender.0, &nonce, &data[44..]);
        let inner = match first {
            Ok(v) => v,
            Err(CryptoError::Auth) | Err(CryptoError::Replay) => {
                replay |= matches!(first, Err(CryptoError::Replay));
                let mut found = None;
                // пакет, зашифрованный ожидающим ключом, подтверждает его: он становится рабочим, прежний — только для чтения
                if let Some(mut p) = e.pending.take() {
                    match p.crypto.open(&sender.0, &nonce, &data[44..]) {
                        Ok(v) => {
                            let old = std::mem::replace(&mut e.crypto, p.crypto);
                            e.fallbacks.insert(0, Fallback { crypto: old, until: now + std::time::Duration::from_secs(FALLBACK_SECS), initiator: e.initiator, unconfirmed: false });
                            e.fallbacks.truncate(MAX_FALLBACKS);
                            self.counter += 1;
                            e.version = self.counter;
                            e.created_at = now;
                            e.initiator = Some(p.initiator);
                            eprintln!("[keys] {}: ключ (начал {}) подтверждён пакетом, теперь рабочий v{}", short(sender), short(&p.initiator), e.version);
                            found = Some(v);
                        }
                        Err(err) => {
                            replay |= matches!(err, CryptoError::Replay);
                            e.pending = Some(p);
                        }
                    }
                }
                // и прежние ключи (пакет мог быть отправлен до смены ключа)
                if found.is_none() {
                    let mut hit = None;
                    for (i, f) in e.fallbacks.iter_mut().enumerate() {
                        match f.crypto.open(&sender.0, &nonce, &data[44..]) {
                            Ok(v) => { found = Some(v); hit = Some(i); break; }
                            Err(CryptoError::Replay) => replay = true,
                            Err(_) => {}
                        }
                    }
                    // ключ победителя встречной смены, пришедший пакетом, — становится рабочим
                    if let Some(i) = hit {
                        if e.fallbacks[i].unconfirmed {
                            let f = e.fallbacks.remove(i);
                            let old = std::mem::replace(&mut e.crypto, f.crypto);
                            e.fallbacks.insert(0, Fallback { crypto: old, until: now + std::time::Duration::from_secs(FALLBACK_SECS), initiator: e.initiator, unconfirmed: false });
                            e.fallbacks.truncate(MAX_FALLBACKS);
                            self.counter += 1;
                            e.version = self.counter;
                            e.created_at = now;
                            e.initiator = f.initiator;
                            eprintln!("[keys] {}: ключ победителя встречной смены подтверждён пакетом, теперь рабочий v{}", short(sender), e.version);
                        }
                    }
                }
                match found {
                    Some(v) => v,
                    None if replay => return Err(format!("REPLAY: packet counter already seen for sender {name}")),
                    None => {
                        eprintln!("[keys] {name}: пакет не подошёл ни к одному ключу: рабочий v{} (возраст {} мс), ожидающий: {}, прежних: {}", e.version, e.age_ms(), e.pending.is_some(), e.fallbacks.len());
                        return Err(format!("Decryption failed for sender {name}: {}", CryptoError::Auth));
                    }
                }
            }
            Err(other) => return Err(format!("Decryption failed for sender {name}: {other}")),
        };
        e.last_used = Instant::now();
        let payload = strip_padding(&inner)?;
        Ok((*sender, payload))
    }
}

fn short(id: &HashId) -> String {
    hex::encode(&id.0[..4])
}

/// Внутри шифртекста: [длина:4 LE][данные][заполнитель] → данные.
fn strip_padding(inner: &[u8]) -> Result<Vec<u8>, String> {
    if inner.len() < 4 {
        return Err("Decrypted payload too short (missing length prefix)".to_string());
    }
    let len = u32::from_le_bytes([inner[0], inner[1], inner[2], inner[3]]) as usize;
    // compared without adding: a length near the top of the range must not overflow (32-bit targets) or slice backwards
    if len > inner.len() - 4 {
        return Err(format!("Decrypted length prefix {} exceeds payload {}", len, inner.len() - 4));
    }
    Ok(inner[4..4 + len].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_prefix_near_the_top_of_the_range_is_refused_not_sliced() {
        assert!(strip_padding(&[0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(strip_padding(&[0xff, 0xff, 0xff, 0xff, 1, 2, 3]).is_err());
        assert_eq!(strip_padding(&[2, 0, 0, 0, 9, 8, 7, 7]).unwrap(), vec![9, 8]);
    }

    #[test]
    fn the_session_table_does_not_grow_past_its_limit() {
        let me = HashId([1; 32]);
        let mut s = SessionStore::new(me);
        for n in 0..(MAX_SESSIONS as u32 + 200) {
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&n.to_be_bytes());
            id[31] = 9;
            s.install(HashId(id), [3; 32], None);
        }
        assert!(s.sessions.len() <= MAX_SESSIONS, "{}", s.sessions.len());
    }

    fn pair() -> (SessionStore, SessionStore, HashId, HashId) {
        let (a, b) = (HashId([1; 32]), HashId([2; 32]));
        let mut x = SessionStore::new(a);
        let mut y = SessionStore::new(b);
        x.install(b, [5; 32], None);
        y.install(a, [5; 32], None);
        (x, y, a, b)
    }

    #[test]
    fn data_of_every_size_comes_back_exactly() {
        let (x, mut y, a, b) = pair();
        for len in [0usize, 1, 63, 64, 65, 1000, 65_000, 70_000, 1 << 20] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let ct = x.encrypt(&b, &data).unwrap();
            let (from, got) = y.decrypt_by_sender(&ct).unwrap();
            assert_eq!(from, a);
            assert_eq!(got, data, "len {len}");
        }
        assert!(x.encrypt(&b, &vec![0u8; MAX_PLAINTEXT + 1]).unwrap_err().contains("too large"));
    }

    #[test]
    fn a_replayed_packet_and_a_changed_packet_are_refused() {
        let (x, mut y, a, b) = pair();
        let ct = x.encrypt(&b, b"hey").unwrap();
        assert!(y.decrypt_from(&a, &ct).is_ok());
        assert!(y.decrypt_from(&a, &ct).unwrap_err().contains("REPLAY"));
        let mut bad = x.encrypt(&b, b"hey").unwrap();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(y.decrypt_from(&a, &bad).is_err());
    }

    #[test]
    fn a_changed_sender_header_is_refused() {
        let (x, mut y, a, b) = pair();
        let mut ct = x.encrypt(&b, b"hey").unwrap();
        ct[0] ^= 1;
        assert!(y.decrypt_by_sender(&ct).is_err(), "другой отправитель — нет сеанса");
        assert!(y.decrypt_from(&a, &ct).unwrap_err().contains("Sender ID mismatch"));
    }

    #[test]
    fn two_sessions_with_different_nodes_do_not_mix() {
        let (a, b, c) = (HashId([1; 32]), HashId([2; 32]), HashId([3; 32]));
        let mut x = SessionStore::new(a);
        x.install(b, [5; 32], None);
        x.install(c, [6; 32], None);
        let mut yb = SessionStore::new(b);
        yb.install(a, [5; 32], None);
        let ct_for_c = x.encrypt(&c, b"for c").unwrap();
        assert!(yb.decrypt_by_sender(&ct_for_c).is_err(), "пакет, зашифрованный для C, B не открыть");
    }

    #[test]
    fn idle_sessions_are_removed_and_the_master_key_can_be_restored() {
        let (mut x, _y, _a, b) = pair();
        assert_eq!(x.master_bytes(&b), Some([5; 32]));
        assert_eq!(x.cleanup_idle(), 0);
        x.sessions.get_mut(&b).unwrap().last_used = Instant::now() - std::time::Duration::from_millis(IDLE_LIMIT_MS as u64 + 1000);
        assert_eq!(x.cleanup_idle(), 1);
        assert!(!x.has(&b));
    }

    #[test]
    fn sessions_survive_a_restart_through_the_master_key() {
        // возобновление: сохранённый общий ключ даёт рабочий сеанс, а счётчики нового сеанса начинаются заново с новой «солью»
        let (x, mut y, _a, b) = pair();
        let master = x.master_bytes(&b).unwrap();
        let mut x2 = SessionStore::new(HashId([1; 32]));
        x2.install(b, master, None);
        let ct = x2.encrypt(&b, b"after restart").unwrap();
        assert_eq!(y.decrypt_by_sender(&ct).unwrap().1, b"after restart");
    }

    #[test]
    fn a_packet_sent_before_a_key_change_still_arrives_after_it() {
        let (x, mut y, a, b) = pair();
        let old_packet = x.encrypt(&b, b"sent with the old key").unwrap();
        y.install(a, [6; 32], None); // получатель уже перешёл на новый ключ
        assert_eq!(y.decrypt_by_sender(&old_packet).unwrap().1, b"sent with the old key", "пакет в пути не потерян");
        // повтор того же пакета отвергается и через прежний ключ
        assert!(y.decrypt_by_sender(&old_packet).is_err());
        // новый ключ работает
        let mut x2 = SessionStore::new(a);
        x2.install(b, [6; 32], None);
        let new_packet = x2.encrypt(&b, b"new key").unwrap();
        assert_eq!(y.decrypt_by_sender(&new_packet).unwrap().1, b"new key");
    }

    #[test]
    fn the_old_key_stops_working_after_its_time() {
        let (x, mut y, a, b) = pair();
        let old_packet = x.encrypt(&b, b"late").unwrap();
        y.install(a, [6; 32], None);
        for f in y.sessions.get_mut(&a).unwrap().fallbacks.iter_mut() {
            f.until = Instant::now() - std::time::Duration::from_secs(1);
        }
        assert!(y.decrypt_by_sender(&old_packet).is_err(), "по истечении времени прежний ключ не принимается");
        assert!(y.sessions.get(&a).unwrap().fallbacks.is_empty(), "просроченный ключ удалён");
    }

    #[test]
    fn the_losing_key_of_a_crossing_handshake_still_opens_early_packets() {
        let (a, b) = (HashId([1; 32]), HashId([2; 32]));
        let mut x = SessionStore::new(a); // отправитель: пользовался проигравшим ключом
        x.install(b, [7; 32], None);
        let mut y = SessionStore::new(b); // получатель: рабочий ключ другой, проигравший принят запасным
        y.install(a, [8; 32], None);
        y.add_fallback(a, [7; 32]);
        let p = x.encrypt(&b, b"early probe").unwrap();
        assert_eq!(y.decrypt_by_sender(&p).unwrap().1, b"early probe");
    }

    #[test]
    fn old_keys_are_kept_in_a_small_number_only() {
        let (a, b) = (HashId([1; 32]), HashId([2; 32]));
        let mut y = SessionStore::new(b);
        for k in 0..10u8 {
            y.install(a, [k; 32], None);
        }
        assert_eq!(y.sessions.get(&a).unwrap().fallbacks.len(), MAX_FALLBACKS);
        let mut z = SessionStore::new(b);
        z.add_fallback(a, [1; 32]); // сеанса нет: запасной ключ не нужен
        assert!(!z.has(&a));
    }

    /// Найдено на живой сети: проигравший ключ встречной смены выталкивал из запасных только что отложенный прежний ключ,
    /// и пакеты, отправленные прежним ключом, переставали читаться.
    #[test]
    fn a_losing_key_never_evicts_the_key_that_was_just_replaced() {
        let (a, b) = (HashId([1; 32]), HashId([2; 32]));
        let mut x = SessionStore::new(a);
        let mut y = SessionStore::new(b);
        for k in 0..MAX_FALLBACKS as u8 + 3 {
            x.install(b, [k + 10; 32], None);
            y.install(a, [k + 10; 32], None);
        }
        // у отправителя ещё прежний ключ (последний из общих)
        let in_flight = y.encrypt(&a, b"sent with the previous key").unwrap();
        // у получателя сменили ключ и тут же пришёл проигравший
        x.install(b, [99; 32], Some(a));
        x.add_fallback(b, [77; 32]);
        assert_eq!(x.decrypt_from(&b, &in_flight).unwrap(), b"sent with the previous key");
    }
}
