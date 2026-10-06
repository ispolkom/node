//! Шифрование пакетов сеанса (AES-256-GCM) со счётчиками вместо случайных номеров.
//!
//! Что здесь устроено и почему:
//! * **Номер пакета — счётчик.** Одноразовый номер (nonce) = `[версия:1][соль:5][счётчик:6]`. Повторить его при одном ключе нельзя, а получатель
//!   может отличить свежий пакет от воспроизведённого по счётчику (скользящее окно в 2048 пакетов, как в IPsec/WireGuard). Прежнее окно из 1024
//!   случайных номеров давало воспроизвести любой более старый пакет и забывалось при перезапуске.
//! * **Ключ у каждого направления свой.** Из общего ключа сеанса выводятся два ключа: «меньший номер → больший» и обратно. Иначе счётчики двух сторон
//!   сталкивались бы под одним ключом, а это катастрофа для GCM.
//! * **Заголовок под защитой.** Номер отправителя (идёт открыто) входит в проверку подлинности (AAD): подмена заголовка ломает расшифровку.
//! * **Потолок.** Больше `MAX_MESSAGES_PER_KEY` пакетов под одним ключом не шифруется (ошибка «нужен новый ключ»); `needs_rekey()` заранее говорит, что пора.
//! * **Ключи затираются** при удалении сеанса.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Версия формата пакета (первый байт одноразового номера). Принимается только она.
pub const FRAME_VERSION: u8 = 2;
/// Жёсткий потолок пакетов под одним ключом.
pub const MAX_MESSAGES_PER_KEY: u64 = 1 << 32;
/// После стольких пакетов или времени пора договариваться о новом ключе.
pub const REKEY_SOFT_MESSAGES: u64 = 1 << 28;
/// Возраст ключа по умолчанию, после которого пора менять ключ.
pub const REKEY_SOFT_AGE: Duration = Duration::from_secs(3600);

/// Возраст ключа для смены: `YANDI_REKEY_AFTER_SECS` (от 10 секунд до суток) или час по умолчанию. Читается один раз.
pub fn rekey_age() -> Duration {
    static V: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *V.get_or_init(|| parse_rekey_age(std::env::var("YANDI_REKEY_AFTER_SECS").ok().as_deref()))
}

pub fn parse_rekey_age(v: Option<&str>) -> Duration {
    match v.and_then(|t| t.trim().parse::<u64>().ok()) {
        Some(n) => Duration::from_secs(n.clamp(10, 86_400)),
        None => REKEY_SOFT_AGE,
    }
}
/// Размер окна защиты от повторов (в пакетах).
pub const WINDOW_BITS: u64 = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// ключ исчерпан: нужен новый
    Exhausted,
    /// чужая версия формата
    BadVersion,
    /// пакет уже был принят или слишком стар
    Replay,
    /// проверка подлинности не прошла (подмена, чужой ключ, испорченные данные)
    Auth,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::Exhausted => write!(f, "session key exhausted: a new handshake is needed"),
            CryptoError::BadVersion => write!(f, "unsupported packet format version"),
            CryptoError::Replay => write!(f, "REPLAY: packet counter already seen or too old"),
            CryptoError::Auth => write!(f, "authentication failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Скользящее окно принятых счётчиков.
#[derive(Clone, Debug)]
pub struct ReplayWindow {
    top: u64,
    bits: [u64; (WINDOW_BITS / 64) as usize],
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self { top: 0, bits: [0; (WINDOW_BITS / 64) as usize] }
    }
}

impl ReplayWindow {
    fn idx(c: u64) -> (usize, u64) {
        let p = c % WINDOW_BITS;
        ((p / 64) as usize, 1u64 << (p % 64))
    }

    /// Можно ли принять этот счётчик (без изменения окна).
    pub fn would_accept(&self, c: u64) -> bool {
        if c == 0 {
            return false; // счётчики начинаются с 1
        }
        if c > self.top {
            return true;
        }
        if self.top - c >= WINDOW_BITS {
            return false;
        }
        let (w, m) = Self::idx(c);
        self.bits[w] & m == 0
    }

    /// Отметить счётчик принятым. Вызывать только после успешной проверки подлинности.
    pub fn mark(&mut self, c: u64) {
        if c > self.top {
            let shift = c - self.top;
            if shift >= WINDOW_BITS {
                self.bits = [0; (WINDOW_BITS / 64) as usize];
            } else {
                for n in (self.top + 1)..=c {
                    let (w, m) = Self::idx(n);
                    self.bits[w] &= !m;
                }
            }
            self.top = c;
        }
        let (w, m) = Self::idx(c);
        self.bits[w] |= m;
    }
}

/// Общий ключ сеанса (32 байта) → ключи двух направлений.
fn directional_keys(master: &[u8; 32], our_id: &[u8; 32], peer_id: &[u8; 32]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let hk = Hkdf::<Sha256>::new(None, master);
    let mut low_to_high = Zeroizing::new([0u8; 32]);
    let mut high_to_low = Zeroizing::new([0u8; 32]);
    hk.expand(b"yandi-v2 key low->high", &mut *low_to_high).expect("hkdf 32 bytes");
    hk.expand(b"yandi-v2 key high->low", &mut *high_to_low).expect("hkdf 32 bytes");
    if our_id <= peer_id {
        (low_to_high, high_to_low)
    } else {
        (high_to_low, low_to_high)
    }
}

/// Защита пакетов одного сеанса между двумя узлами.
pub struct SessionCrypto {
    master: Zeroizing<[u8; 32]>,
    tx: Aes256Gcm,
    rx: Aes256Gcm,
    salt: [u8; 5],
    counter: AtomicU64,
    window: ReplayWindow,
    created: Instant,
}

impl std::fmt::Debug for SessionCrypto {
    // ключей в отладочном выводе не бывает
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCrypto").field("sent", &self.counter.load(Ordering::Relaxed)).finish_non_exhaustive()
    }
}

impl Clone for SessionCrypto {
    fn clone(&self) -> Self {
        // копия получает те же ключи, но СВОЙ счётчик не может продолжать чужой: берём текущее значение и окно как есть (копии не должны использоваться параллельно)
        let (tx, rx) = (self.tx.clone(), self.rx.clone());
        Self { master: self.master.clone(), tx, rx, salt: self.salt, counter: AtomicU64::new(self.counter.load(Ordering::Relaxed)), window: self.window.clone(), created: self.created }
    }
}

impl SessionCrypto {
    /// Из общего ключа сеанса и номеров обоих узлов.
    pub fn from_master(master: [u8; 32], our_id: &[u8; 32], peer_id: &[u8; 32]) -> Self {
        let (txk, rxk) = directional_keys(&master, our_id, peer_id);
        // 40-битная случайная «соль» сеанса: при возобновлении из сохранённого ключа счётчик начинается заново, и только соль различает запуски
        let mut salt = [0u8; 5];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut salt);
        Self {
            master: Zeroizing::new(master),
            tx: Aes256Gcm::new_from_slice(&*txk).expect("32-byte key"),
            rx: Aes256Gcm::new_from_slice(&*rxk).expect("32-byte key"),
            salt,
            counter: AtomicU64::new(0),
            window: ReplayWindow::default(),
            created: Instant::now(),
        }
    }

    /// Общий ключ сеанса (для сохранения возобновления и для вывода ключей передачи файлов).
    pub fn master_bytes(&self) -> [u8; 32] {
        *self.master
    }

    /// Пора договариваться о новом ключе (не авария: пока можно шифровать).
    pub fn needs_rekey(&self) -> bool {
        self.counter.load(Ordering::Relaxed) >= REKEY_SOFT_MESSAGES || self.created.elapsed() >= rekey_age()
    }

    pub fn age(&self) -> Duration {
        self.created.elapsed()
    }

    /// Зашифровать. Возвращает (одноразовый номер, шифртекст с тегом).
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Result<([u8; 12], Vec<u8>), CryptoError> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        if n >= MAX_MESSAGES_PER_KEY {
            return Err(CryptoError::Exhausted);
        }
        let mut nonce = [0u8; 12];
        nonce[0] = FRAME_VERSION;
        nonce[1..6].copy_from_slice(&self.salt);
        nonce[6..].copy_from_slice(&n.to_be_bytes()[2..]);
        let ct = self.tx.encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &with_version(aad) }).map_err(|_| CryptoError::Auth)?;
        Ok((nonce, ct))
    }

    /// Расшифровать. Окно повторов меняется только после успешной проверки подлинности: подделка не может его испортить.
    pub fn open(&mut self, aad: &[u8], nonce: &[u8; 12], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if nonce[0] != FRAME_VERSION {
            return Err(CryptoError::BadVersion);
        }
        let mut c = [0u8; 8];
        c[2..].copy_from_slice(&nonce[6..]);
        let counter = u64::from_be_bytes(c);
        if !self.window.would_accept(counter) {
            return Err(CryptoError::Replay);
        }
        let pt = self.rx.decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: &with_version(aad) }).map_err(|_| CryptoError::Auth)?;
        self.window.mark(counter);
        Ok(pt)
    }
}

fn with_version(aad: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(aad.len() + 1);
    v.push(FRAME_VERSION);
    v.extend_from_slice(aad);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (SessionCrypto, SessionCrypto, [u8; 32], [u8; 32]) {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let master = [7u8; 32];
        (SessionCrypto::from_master(master, &a, &b), SessionCrypto::from_master(master, &b, &a), a, b)
    }

    #[test]
    fn a_message_goes_both_ways() {
        let (mut x, mut y, a, b) = pair();
        let (n, ct) = x.seal(&a, b"hello b").unwrap();
        assert_eq!(y.open(&a, &n, &ct).unwrap(), b"hello b");
        let (n, ct) = y.seal(&b, b"hello a").unwrap();
        assert_eq!(x.open(&b, &n, &ct).unwrap(), b"hello a");
    }

    #[test]
    fn the_same_packet_is_refused_the_second_time() {
        let (x, mut y, a, _) = pair();
        let (n, ct) = x.seal(&a, b"once").unwrap();
        assert!(y.open(&a, &n, &ct).is_ok());
        assert_eq!(y.open(&a, &n, &ct).unwrap_err(), CryptoError::Replay);
    }

    #[test]
    fn an_old_packet_is_refused_after_the_window_moves_but_an_unseen_one_inside_it_is_accepted() {
        let (x, mut y, a, _) = pair();
        let first = x.seal(&a, b"first").unwrap();
        let skipped = x.seal(&a, b"skipped").unwrap(); // потерялся в пути, придёт позже
        for _ in 0..100 {
            let (n, ct) = x.seal(&a, b"x").unwrap();
            y.open(&a, &n, &ct).unwrap();
        }
        assert_eq!(y.open(&a, &skipped.0, &skipped.1).unwrap(), b"skipped", "пакет, пришедший с опозданием, но внутри окна и новый");
        assert!(y.open(&a, &first.0, &first.1).is_ok(), "первый тоже ещё внутри окна и не принимался");
        assert_eq!(y.open(&a, &first.0, &first.1).unwrap_err(), CryptoError::Replay);
        for _ in 0..(WINDOW_BITS as usize + 5) {
            let (n, ct) = x.seal(&a, b"y").unwrap();
            y.open(&a, &n, &ct).unwrap();
        }
        let old = x.seal(&a, b"late").unwrap();
        for _ in 0..(WINDOW_BITS as usize + 5) {
            let (n, ct) = x.seal(&a, b"z").unwrap();
            y.open(&a, &n, &ct).unwrap();
        }
        assert_eq!(y.open(&a, &old.0, &old.1).unwrap_err(), CryptoError::Replay, "слишком старый пакет окно уже не принимает");
    }

    #[test]
    fn a_reflected_packet_does_not_open_at_the_sender() {
        // ключ у направлений разный: пакет, отправленный A, нельзя «подсунуть» обратно A как будто он от B
        let (mut x, _y, a, _) = pair();
        let (n, ct) = x.seal(&a, b"mine").unwrap();
        assert_eq!(x.open(&a, &n, &ct).unwrap_err(), CryptoError::Auth);
    }

    #[test]
    fn a_changed_byte_a_changed_header_or_a_wrong_key_is_refused_and_does_not_hurt_the_window() {
        let (x, mut y, a, b) = pair();
        let (n, ct) = x.seal(&a, b"data").unwrap();
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert_eq!(y.open(&a, &n, &bad).unwrap_err(), CryptoError::Auth);
        assert_eq!(y.open(&b, &n, &ct).unwrap_err(), CryptoError::Auth, "другой заголовок (номер отправителя) — другая проверка");
        let mut n2 = n;
        n2[11] ^= 1;
        assert!(y.open(&a, &n2, &ct).is_err(), "изменённый счётчик");
        // настоящий пакет после всех неудачных попыток всё ещё принимается
        assert_eq!(y.open(&a, &n, &ct).unwrap(), b"data");
        let mut other = SessionCrypto::from_master([9u8; 32], &[2u8; 32], &[1u8; 32]);
        assert!(other.open(&a, &n, &ct).is_err(), "чужой ключ");
    }

    #[test]
    fn a_foreign_format_version_is_refused() {
        let (x, mut y, a, _) = pair();
        let (mut n, ct) = x.seal(&a, b"v").unwrap();
        n[0] = 1;
        assert_eq!(y.open(&a, &n, &ct).unwrap_err(), CryptoError::BadVersion);
    }

    #[test]
    fn counters_never_repeat_and_start_at_one() {
        let (x, _y, a, _) = pair();
        let mut seen = std::collections::HashSet::new();
        for i in 1..=500u64 {
            let (n, _) = x.seal(&a, b"p").unwrap();
            assert!(seen.insert(n), "одноразовый номер повторился");
            let mut c = [0u8; 8];
            c[2..].copy_from_slice(&n[6..]);
            assert_eq!(u64::from_be_bytes(c), i);
        }
    }

    #[test]
    fn the_ceiling_stops_encryption() {
        let (x, _y, a, _) = pair();
        x.counter.store(MAX_MESSAGES_PER_KEY - 2, Ordering::SeqCst);
        assert!(x.seal(&a, b"last ok").is_ok());
        assert_eq!(x.seal(&a, b"too many").unwrap_err(), CryptoError::Exhausted);
        assert_eq!(x.seal(&a, b"still no").unwrap_err(), CryptoError::Exhausted);
    }

    #[test]
    fn the_rekey_age_is_configurable_within_limits() {
        assert_eq!(parse_rekey_age(None), REKEY_SOFT_AGE);
        assert_eq!(parse_rekey_age(Some("30")), Duration::from_secs(30));
        assert_eq!(parse_rekey_age(Some("1")), Duration::from_secs(10), "не чаще раза в 10 секунд");
        assert_eq!(parse_rekey_age(Some("999999999")), Duration::from_secs(86_400));
        assert_eq!(parse_rekey_age(Some("мусор")), REKEY_SOFT_AGE);
    }

    #[test]
    fn rekey_is_announced_before_the_ceiling() {
        let (x, _y, a, _) = pair();
        assert!(!x.needs_rekey());
        x.counter.store(REKEY_SOFT_MESSAGES, Ordering::SeqCst);
        assert!(x.needs_rekey());
        assert!(x.seal(&a, b"still works").is_ok());
    }

    #[test]
    fn each_session_has_its_own_salt_and_the_two_directions_use_different_keys() {
        let (x, y, a, b) = pair();
        let (n1, c1) = x.seal(&a, b"same text").unwrap();
        let (n2, c2) = y.seal(&b, b"same text").unwrap();
        assert_ne!(c1, c2, "один и тот же текст в разных направлениях шифруется по-разному");
        let _ = (n1, n2);
    }

    #[test]
    fn the_replay_window_matches_a_simple_model_on_random_orders() {
        // случайный порядок прихода: окно принимает ровно то, что принимает простая модель
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut w = ReplayWindow::default();
        let mut model: std::collections::BTreeSet<u64> = Default::default();
        let mut top = 0u64;
        for _ in 0..20_000 {
            let c = 1 + (top.saturating_sub(300) + rnd() % 600).max(1);
            let expect = c != 0 && !model.contains(&c) && (c > top || top - c < WINDOW_BITS);
            assert_eq!(w.would_accept(c), expect, "c={c} top={top}");
            if expect {
                w.mark(c);
                model.insert(c);
                top = top.max(c);
            }
        }
    }

    #[test]
    fn a_zero_counter_is_never_accepted() {
        assert!(!ReplayWindow::default().would_accept(0));
    }

    #[test]
    fn debug_output_has_no_key_material() {
        let (x, _y, _, _) = pair();
        let s = format!("{x:?}");
        assert!(!s.contains("07") && s.starts_with("SessionCrypto"), "{s}");
    }

    /// «Золотой» вектор: формат на проводе не должен меняться молча. Меняете — повышайте FRAME_VERSION и обновляйте вектор осознанно.
    #[test]
    fn golden_vector() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let mut x = SessionCrypto::from_master([7u8; 32], &a, &b);
        x.salt = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        let (nonce, ct) = x.seal(&a, b"golden").unwrap();
        assert_eq!(hex::encode(nonce), "02aabbccddee000000000001");
        assert_eq!(hex::encode(&ct), GOLDEN_CT, "формат/ключи/AAD изменились");
        let mut y = SessionCrypto::from_master([7u8; 32], &b, &a);
        assert_eq!(y.open(&a, &nonce, &ct).unwrap(), b"golden");
    }
    /// Вектор получен независимо: Node.js (OpenSSL), HKDF-SHA256 без соли, info = «yandi-v2 key low->high», AES-256-GCM, AAD = [версия=2] ‖ номер отправителя.
    const GOLDEN_CT: &str = "4bd3c4d90dd64958679aa40615886a59cc95f45f606e";
}
