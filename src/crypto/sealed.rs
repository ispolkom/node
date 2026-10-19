//! Запечатанный конверт «отправитель → получатель» (E2E v1 между узлами, `docs/SIMPLE_DESIGN.md`).
//!
//! Шифрует на долговременный ключ получателя (X25519, выведенный из его ключа личности Ed25519) с одноразовым эфемерным ключом
//! отправителя и подписывает ключом личности отправителя. Узлы между ними видят только непрозрачные байты.
//!
//! Формат (все числа big-endian):
//! `"YSE1"(4) | eph_pub(32) | nonce(12) | created_ms u64 | len u32 | ciphertext(len) | sig(64)`.
//! Ключ: `HKDF-SHA256(salt = eph_pub || recipient_x, ikm = X25519(eph, recipient_x), info = "yandi-sealed-v1" || from || to)`.
//! AAD шифра: `"YSE1" || eph_pub || nonce || created_ms || from || to`. Подпись Ed25519 над `"yandi-sealed-sig-v1" || (то же AAD) || ciphertext`.
//!
//! Чего НЕТ: прямой секретности для получателя (долговременный ключ), скрытия метаданных и защиты от повторов (повтор снимает
//! уровень приложения по идентификатору сообщения; здесь только окно по `created_ms`). Ключ личности используется и для подписи, и для
//! Диффи-Хеллмана — как в XEdDSA/libsodium; смена ключа = новая личность.
use anyhow::{bail, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

const MAGIC: &[u8; 4] = b"YSE1";
const HEADER: usize = 4 + 32 + 12 + 8 + 4;
const SIG_LABEL: &[u8] = b"yandi-sealed-sig-v1";
const KDF_LABEL: &[u8] = b"yandi-sealed-v1";
/// Допустимое отставание конверта от часов получателя и опережение.
pub const MAX_AGE_MS: u64 = 30 * 24 * 3600 * 1000;
pub const MAX_AHEAD_MS: u64 = 5 * 60 * 1000;
/// Предел открытого текста (пакет чата один, длина u16 в транспорте).
pub const MAX_PLAINTEXT: usize = 60_000;

fn aad(eph: &[u8; 32], nonce: &[u8; 12], created_ms: u64, from: &[u8; 32], to: &[u8; 32]) -> Vec<u8> {
    let mut a = Vec::with_capacity(4 + 32 + 12 + 8 + 64);
    a.extend_from_slice(MAGIC);
    a.extend_from_slice(eph);
    a.extend_from_slice(nonce);
    a.extend_from_slice(&created_ms.to_be_bytes());
    a.extend_from_slice(from);
    a.extend_from_slice(to);
    a
}

fn derive_key(shared: &[u8; 32], eph: &[u8; 32], recipient_x: &[u8; 32], from: &[u8; 32], to: &[u8; 32]) -> Option<[u8; 32]> {
    // нулевой общий секрет (малый порядок) — отказ
    if shared.iter().all(|b| *b == 0) {
        return None;
    }
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(eph);
    salt[32..].copy_from_slice(recipient_x);
    let mut info = Vec::with_capacity(KDF_LABEL.len() + 64);
    info.extend_from_slice(KDF_LABEL);
    info.extend_from_slice(from);
    info.extend_from_slice(to);
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&salt), shared).expand(&info, &mut key).ok()?;
    Some(key)
}

fn sig_input(aad: &[u8], ct: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(SIG_LABEL.len() + aad.len() + ct.len());
    m.extend_from_slice(SIG_LABEL);
    m.extend_from_slice(aad);
    m.extend_from_slice(ct);
    m
}

/// Запечатать `plaintext` для `recipient`. `from`/`to` — идентификаторы узлов (входят в AAD и ключ).
pub fn seal(sender: &SigningKey, recipient: &VerifyingKey, from: &[u8; 32], to: &[u8; 32], created_ms: u64, plaintext: &[u8]) -> Result<Vec<u8>> {
    if plaintext.len() > MAX_PLAINTEXT {
        bail!("сообщение слишком велико для конверта");
    }
    let recipient_x = recipient.to_montgomery().to_bytes();
    let eph_sk = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let eph_pub = PublicKey::from(&eph_sk).to_bytes();
    let shared = eph_sk.diffie_hellman(&PublicKey::from(recipient_x)).to_bytes();
    let Some(key) = derive_key(&shared, &eph_pub, &recipient_x, from, to) else { bail!("получатель дал недопустимый ключ") };
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let a = aad(&eph_pub, &nonce, created_ms, from, to);
    let ct = ChaCha20Poly1305::new((&key).into())
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &a })
        .map_err(|_| anyhow::anyhow!("не удалось зашифровать"))?;
    let sig = sender.sign(&sig_input(&a, &ct)).to_bytes();
    let mut out = Vec::with_capacity(HEADER + ct.len() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&eph_pub);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&created_ms.to_be_bytes());
    out.extend_from_slice(&(ct.len() as u32).to_be_bytes());
    out.extend_from_slice(&ct);
    out.extend_from_slice(&sig);
    Ok(out)
}

/// Открыть конверт. Сначала проверяется подпись отправителя, потом расшифровывается. Любая неудача — одна и та же ошибка без подробностей.
pub fn open(recipient: &SigningKey, sender: &VerifyingKey, from: &[u8; 32], to: &[u8; 32], now_ms: u64, data: &[u8]) -> Result<(u64, Vec<u8>)> {
    let bad = || anyhow::anyhow!("конверт отвергнут");
    if data.len() < HEADER + 16 + 64 || &data[..4] != MAGIC {
        return Err(bad());
    }
    let eph: [u8; 32] = data[4..36].try_into().unwrap();
    let nonce: [u8; 12] = data[36..48].try_into().unwrap();
    let created_ms = u64::from_be_bytes(data[48..56].try_into().unwrap());
    let len = u32::from_be_bytes(data[56..60].try_into().unwrap()) as usize;
    if len < 16 || len > MAX_PLAINTEXT + 16 || data.len() != HEADER + len + 64 {
        return Err(bad());
    }
    let ct = &data[HEADER..HEADER + len];
    let sig = Signature::from_slice(&data[HEADER + len..]).map_err(|_| bad())?;
    let a = aad(&eph, &nonce, created_ms, from, to);
    sender.verify(&sig_input(&a, ct), &sig).map_err(|_| bad())?;
    if created_ms > now_ms.saturating_add(MAX_AHEAD_MS) || now_ms.saturating_sub(created_ms) > MAX_AGE_MS {
        return Err(bad());
    }
    let my_x_secret = StaticSecret::from(recipient.to_scalar_bytes());
    let my_x_pub = PublicKey::from(&my_x_secret).to_bytes();
    let shared = my_x_secret.diffie_hellman(&PublicKey::from(eph)).to_bytes();
    let key = derive_key(&shared, &eph, &my_x_pub, from, to).ok_or_else(bad)?;
    let pt = ChaCha20Poly1305::new((&key).into()).decrypt(Nonce::from_slice(&nonce), Payload { msg: ct, aad: &a }).map_err(|_| bad())?;
    Ok((created_ms, pt))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn sk(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    fn id(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn a_sealed_message_opens_for_the_recipient_only() {
        let (a, b, c) = (sk(1), sk(2), sk(3));
        let env = seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, "привет".as_bytes()).unwrap();
        let (t, pt) = open(&b, &a.verifying_key(), &id(1), &id(2), NOW + 5, &env).unwrap();
        assert_eq!((t, pt.as_slice()), (NOW, "привет".as_bytes()));
        assert!(open(&c, &a.verifying_key(), &id(1), &id(2), NOW, &env).is_err(), "третий открыл конверт");
        assert!(!env.windows(12).any(|w| w == "привет".as_bytes()), "открытый текст в конверте");
    }

    #[test]
    fn the_envelope_is_bound_to_sender_recipient_and_time() {
        let (a, b, c) = (sk(1), sk(2), sk(3));
        let env = seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, b"x").unwrap();
        // другой отправитель по подписи
        assert!(open(&b, &c.verifying_key(), &id(1), &id(2), NOW, &env).is_err());
        // другие идентификаторы в AAD
        assert!(open(&b, &a.verifying_key(), &id(9), &id(2), NOW, &env).is_err());
        assert!(open(&b, &a.verifying_key(), &id(1), &id(9), NOW, &env).is_err());
        // окно времени
        assert!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW + MAX_AGE_MS + 1, &env).is_err());
        assert!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW - MAX_AHEAD_MS - 1, &env).is_err());
        assert!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW + MAX_AGE_MS, &env).is_ok());
    }

    #[test]
    fn any_changed_byte_or_length_is_refused() {
        let (a, b) = (sk(1), sk(2));
        let env = seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, b"hello world").unwrap();
        for i in 0..env.len() {
            let mut bad = env.clone();
            bad[i] ^= 1;
            assert!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW, &bad).is_err(), "изменённый байт {i} принят");
        }
        for n in 0..env.len() {
            assert!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW, &env[..n]).is_err(), "обрезка до {n} принята");
        }
        let mut longer = env.clone();
        longer.push(0);
        assert!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW, &longer).is_err());
    }

    #[test]
    fn each_seal_is_unique_and_sizes_are_limited() {
        let (a, b) = (sk(1), sk(2));
        let e1 = seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, b"same").unwrap();
        let e2 = seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, b"same").unwrap();
        assert_ne!(e1, e2);
        assert!(seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, &vec![0u8; MAX_PLAINTEXT + 1]).is_err());
        let big = seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, &vec![7u8; MAX_PLAINTEXT]).unwrap();
        assert_eq!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW, &big).unwrap().1.len(), MAX_PLAINTEXT);
        let empty = seal(&a, &b.verifying_key(), &id(1), &id(2), NOW, b"").unwrap();
        assert!(open(&b, &a.verifying_key(), &id(1), &id(2), NOW, &empty).unwrap().1.is_empty());
    }
}
