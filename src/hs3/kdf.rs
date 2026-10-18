//! Расписание ключей рукопожатия версии 3 (docs/HANDSHAKE_HARDENING.md, раздел 3а): транскрипт `T`, вывод ключей и MAC подтверждений.
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

/// `T = SHA-256("yandi-hs-v3" || u16 len(R) || R || u16 len(A) || A)`.
pub fn transcript(r: &[u8], a: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"yandi-hs-v3");
    h.update((r.len() as u16).to_be_bytes());
    h.update(r);
    h.update((a.len() as u16).to_be_bytes());
    h.update(a);
    h.finalize().into()
}

/// SHA-256 точных байт запроса (`H(R)` в ответе).
pub fn hash_request(r: &[u8]) -> [u8; 32] {
    Sha256::digest(r).into()
}

fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC принимает ключ любой длины");
    m.update(msg);
    m.finalize().into_bytes().into()
}

fn expand32(prk: &[u8; 32], info: &[u8]) -> [u8; 32] {
    // один блок HKDF-Expand (32 байта): T(1) = HMAC(PRK, info || 0x01)
    hmac(prk, &[info, &[1u8]].concat())
}

/// Ключи поколения. Все значения стираются при освобождении.
pub struct Keys {
    pub i2r: Zeroizing<[u8; 32]>,
    pub r2i: Zeroizing<[u8; 32]>,
    pub kc_i: Zeroizing<[u8; 32]>,
    pub kc_r: Zeroizing<[u8; 32]>,
    pub exporter: Zeroizing<[u8; 32]>,
}

/// Вывод ключей из общего значения Диффи—Хеллмана. Слабая точка (все нули) отвергается ДО вывода: `None`.
pub fn derive(t: &[u8; 32], dh: &[u8; 32], context: u8) -> Option<Keys> {
    if dh.iter().all(|b| *b == 0) {
        return None;
    }
    let prk = Zeroizing::new(hmac(t, dh)); // HKDF-Extract(salt = T, ikm = DH)
    let label = |l: &[u8]| Zeroizing::new(expand32(&prk, &[b"yandi-hs-v3/", l, &[0u8], &[context]].concat()));
    Some(Keys { i2r: label(b"i2r"), r2i: label(b"r2i"), kc_i: label(b"kc_i"), kc_r: label(b"kc_r"), exporter: label(b"exporter") })
}

/// MAC подтверждения: `HMAC(kc, "fin-i" | "fin-r" || T)`.
pub fn fin_mac(kc: &[u8; 32], initiator: bool, t: &[u8; 32]) -> [u8; 32] {
    hmac(kc, &[if initiator { b"fin-i".as_slice() } else { b"fin-r".as_slice() }, t].concat())
}

/// Сравнение MAC за постоянное время.
pub fn mac_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// Ключ файла из `exporter`: `PRK_f = HKDF-Extract("yandi-file-v1", exporter)`, `HKDF-Expand(PRK_f, "file/" || u8 len || file_id, 32)`.
pub fn file_key(exporter: &[u8; 32], file_id: &[u8]) -> Option<Zeroizing<[u8; 32]>> {
    if file_id.len() > 64 {
        return None;
    }
    let prk_f = Zeroizing::new(hmac(b"yandi-file-v1", exporter));
    Some(Zeroizing::new(expand32(&prk_f, &[b"file/".as_slice(), &[file_id.len() as u8], file_id].concat())))
}
