//! Парольный ключ закрытого уровня узла (роадмап 3.1).
//!
//! Не использует auth.json, device.key, node_kek или идентификатор машины.
//! Это криптографическая основа, ещё не подключённая к рабочим хранилищам:
//! её наличие само по себе НЕ закрывает историю/ключи старого узла.
//! Сохранение envelope и миграцию данных выполняет вызывающий слой.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, bail, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const FORMAT: u32 = 1;
const MAX_SECRET: usize = 1024;
const MAX_DATA: usize = 16 * 1024 * 1024;
const KEY_AAD: &[u8] = b"YANDI-NODE-CLOSED-KEY-V1\0";
const DATA_AAD: &[u8] = b"YANDI-NODE-CLOSED-DATA-V1\0";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyWrap {
    salt: [u8; 32],
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
}

/// На диске только случайный DK под паролем и необязательными словами.
/// Параметры KDF фиксированы форматом, файл не может понизить цену перебора.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultEnvelope {
    version: u32,
    id: [u8; 16],
    password: KeyWrap,
    recovery: Option<KeyWrap>,
    tag: [u8; 32],
}

/// Каждый новый экземпляр заперт. Секреты не реализуют Debug/Clone/Serialize.
#[derive(Default)]
pub struct NodeVault {
    session: Option<Session>,
}

struct Session {
    id: [u8; 16],
    key: Zeroizing<[u8; 32]>,
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}

fn password_ok(password: &str) -> Result<()> {
    if password.chars().count() < 6 || password.len() > MAX_SECRET {
        bail!("пароль должен содержать не менее 6 знаков и не более 1024 байтов");
    }
    Ok(())
}

fn phrase_secret(phrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    if phrase.len() > MAX_SECRET {
        bail!("слишком длинная фраза");
    }
    let words: Vec<_> = phrase.split_whitespace().collect();
    if words.len() != 4 || words.iter().any(|w| w.chars().count() < 5) {
        bail!("нужны четыре слова не короче пяти знаков");
    }
    // Длины сохраняют границы слов; пробелы между словами нормализуются.
    let mut encoded = Zeroizing::new(Vec::new());
    for word in words {
        encoded.extend_from_slice(&(word.len() as u32).to_be_bytes());
        encoded.extend_from_slice(word.as_bytes());
    }
    Ok(encoded)
}

fn wrapping_key(secret: &[u8], salt: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(65536, 3, 1, Some(32)).map_err(|_| anyhow!("параметры KDF"))?;
    // Обычный hash_password_into освобождает рабочие блоки без их стирания.
    // Владеем всей памятью KDF: Zeroizing очищает её и при ошибке/панике.
    let mut memory = Zeroizing::new(vec![argon2::Block::default(); params.block_count()]);
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(secret, salt, key.as_mut(), memory.as_mut_slice())
        .map_err(|_| anyhow!("ошибка KDF"))?;
    Ok(key)
}

fn key_aad(id: &[u8; 16], purpose: &[u8]) -> Vec<u8> {
    [KEY_AAD, id.as_slice(), purpose].concat()
}

fn wrap(key: &[u8; 32], secret: &[u8], id: &[u8; 16], purpose: &[u8]) -> Result<KeyWrap> {
    let salt = random();
    let nonce = random();
    let derived = wrapping_key(secret, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(derived.as_ref()).map_err(|_| anyhow!("ключ AES"))?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: key,
                aad: &key_aad(id, purpose),
            },
        )
        .map_err(|_| anyhow!("шифрование ключа"))?;
    Ok(KeyWrap {
        salt,
        nonce,
        ciphertext,
    })
}

fn unwrap(
    wrapped: &KeyWrap,
    secret: &[u8],
    id: &[u8; 16],
    purpose: &[u8],
) -> Result<Zeroizing<[u8; 32]>> {
    if wrapped.ciphertext.len() != 48 {
        bail!("повреждённый замок ключа");
    }
    let derived = wrapping_key(secret, &wrapped.salt)?;
    let cipher = Aes256Gcm::new_from_slice(derived.as_ref()).map_err(|_| anyhow!("ключ AES"))?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&wrapped.nonce),
                Payload {
                    msg: &wrapped.ciphertext,
                    aad: &key_aad(id, purpose),
                },
            )
            .map_err(|_| anyhow!("неверный секрет или повреждённый замок"))?,
    );
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(&plain);
    Ok(key)
}

impl VaultEnvelope {
    /// Только создание нового envelope, не замена существующего на диске.
    pub fn create(password: &str, recovery: Option<&str>) -> Result<Self> {
        password_ok(password)?;
        let phrase = recovery.map(phrase_secret).transpose()?;
        let id = random();
        let key = Zeroizing::new(random());
        let mut envelope = Self {
            version: FORMAT,
            id,
            password: wrap(&key, password.as_bytes(), &id, b"password")?,
            recovery: phrase
                .as_ref()
                .map(|s| wrap(&key, s, &id, b"recovery"))
                .transpose()?,
            tag: [0; 32],
        };
        envelope.tag = envelope.mac(&key).finalize().into_bytes().into();
        Ok(envelope)
    }

    fn mac(&self, key: &[u8; 32]) -> Hmac<sha2::Sha256> {
        let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(key).expect("ключ HMAC");
        mac.update(b"YANDI-NODE-CLOSED-ENVELOPE-V1\0");
        mac.update(&self.version.to_be_bytes());
        mac.update(&self.id);
        mac.update(&self.password.salt);
        mac.update(&self.password.nonce);
        mac.update(&self.password.ciphertext);
        mac.update(&[u8::from(self.recovery.is_some())]);
        if let Some(wrapped) = &self.recovery {
            mac.update(&wrapped.salt);
            mac.update(&wrapped.nonce);
            mac.update(&wrapped.ciphertext);
        }
        mac
    }

    fn verify(&self, key: &[u8; 32]) -> Result<()> {
        self.mac(key)
            .verify_slice(&self.tag)
            .map_err(|_| anyhow!("повреждённый envelope"))
    }

    fn validate(&self) -> Result<()> {
        if self.version != FORMAT
            || self.password.ciphertext.len() != 48
            || self
                .recovery
                .as_ref()
                .is_some_and(|r| r.ciphertext.len() != 48)
        {
            bail!("неподдерживаемый или повреждённый формат ключа");
        }
        Ok(())
    }
}

impl NodeVault {
    pub fn unlocked(&self) -> bool {
        self.session.is_some()
    }

    /// Drop Zeroizing стирает DK. &mut self исключает позднюю запись unlock в сеанс после lock.
    pub fn lock(&mut self) {
        self.session = None;
    }

    pub fn unlock(&mut self, envelope: &VaultEnvelope, password: &str) -> Result<()> {
        self.lock();
        envelope.validate()?;
        password_ok(password)?;
        let key = unwrap(
            &envelope.password,
            password.as_bytes(),
            &envelope.id,
            b"password",
        )?;
        envelope.verify(&key)?;
        self.session = Some(Session {
            id: envelope.id,
            key,
        });
        Ok(())
    }

    pub fn recover(&mut self, envelope: &VaultEnvelope, phrase: &str) -> Result<()> {
        self.lock();
        envelope.validate()?;
        let wrapped = envelope
            .recovery
            .as_ref()
            .ok_or_else(|| anyhow!("слова не заданы"))?;
        let secret = phrase_secret(phrase)?;
        let key = unwrap(wrapped, &secret, &envelope.id, b"recovery")?;
        envelope.verify(&key)?;
        self.session = Some(Session {
            id: envelope.id,
            key,
        });
        Ok(())
    }

    /// Сначала вызывающий слой надёжно сохраняет возвращённый envelope, затем считает пароль сменённым.
    /// DK и recovery не меняются: контейнеры не надо перешифровывать.
    pub fn change_password(
        &self,
        envelope: &VaultEnvelope,
        password: &str,
    ) -> Result<VaultEnvelope> {
        envelope.validate()?;
        password_ok(password)?;
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow!("закрытая среда заперта"))?;
        if session.id != envelope.id {
            bail!("ключ другого хранилища");
        }
        envelope.verify(&session.key)?;
        let mut next = envelope.clone();
        next.password = wrap(&session.key, password.as_bytes(), &session.id, b"password")?;
        next.tag = next.mac(&session.key).finalize().into_bytes().into();
        Ok(next)
    }

    fn data_cipher(&self, domain: &str) -> Result<(Aes256Gcm, Vec<u8>)> {
        if domain.is_empty() || domain.len() > 128 {
            bail!("неверная область данных");
        }
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow!("закрытая среда заперта"))?;
        let aad = [DATA_AAD, session.id.as_slice(), domain.as_bytes()].concat();
        let mut key = Zeroizing::new([0; 32]);
        hkdf::Hkdf::<sha2::Sha256>::new(Some(DATA_AAD), session.key.as_ref())
            .expand(&aad, key.as_mut())
            .map_err(|_| anyhow!("HKDF"))?;
        Ok((
            Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| anyhow!("ключ AES"))?,
            aad,
        ))
    }

    /// domain — стабильное логическое имя (например history/peer), не абсолютный путь.
    pub fn seal(&self, domain: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > MAX_DATA {
            bail!("слишком большой блок");
        }
        let (cipher, aad) = self.data_cipher(domain)?;
        let nonce: [u8; 12] = random();
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow!("шифрование данных"))?;
        Ok([nonce.as_slice(), &ciphertext].concat())
    }

    /// Вызывающий слой отвечает за стирание своих копий расшифрованных данных при lock.
    pub fn open(&self, domain: &str, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if !(28..=MAX_DATA + 28).contains(&blob.len()) {
            bail!("неверная длина блока");
        }
        let (cipher, aad) = self.data_cipher(domain)?;
        Ok(Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&blob[..12]),
                    Payload {
                        msg: &blob[12..],
                        aad: &aad,
                    },
                )
                .map_err(|_| anyhow!("неверный ключ, область или повреждённый блок"))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_locked_and_copied_envelope_requires_password() {
        let envelope = VaultEnvelope::create("пароль владельца", None).unwrap();
        let encoded = serde_json::to_vec(&envelope).unwrap();
        let mut original = NodeVault::default();
        original.unlock(&envelope, "пароль владельца").unwrap();
        let data = original.seal("history/one", b"private history").unwrap();
        original.lock();
        assert!(original.open("history/one", &data).is_err());
        assert!(original.seal("history/one", b"x").is_err());
        let copied: VaultEnvelope = serde_json::from_slice(&encoded).unwrap();
        let mut restarted = NodeVault::default();
        assert!(!restarted.unlocked());
        assert!(restarted.unlock(&copied, "wrong password").is_err());
        assert!(!restarted.unlocked());
        restarted.unlock(&copied, "пароль владельца").unwrap();
        assert_eq!(
            &**restarted.open("history/one", &data).unwrap(),
            b"private history"
        );
        assert!(restarted.open("history/other", &data).is_err());
        for index in 0..data.len() {
            let mut damaged = data.clone();
            damaged[index] ^= 1;
            assert!(restarted.open("history/one", &damaged).is_err());
        }
    }

    #[test]
    fn password_change_preserves_data_and_optional_recovery() {
        let envelope = VaultEnvelope::create(
            "old password",
            Some("словоодин словодва словотри словочетыре"),
        )
        .unwrap();
        let mut vault = NodeVault::default();
        assert!(vault.change_password(&envelope, "new password").is_err());
        vault
            .recover(&envelope, "словоодин\nсловодва  словотри словочетыре")
            .unwrap();
        let data = vault.seal("groups/state", b"group keys").unwrap();
        let next = vault.change_password(&envelope, "new password").unwrap();
        // Подмена/удаление recovery не остаётся незамеченной при входе паролем.
        let mut stripped = next.clone();
        stripped.recovery = None;
        assert!(vault.unlock(&stripped, "new password").is_err());
        let mut corrupt = next.clone();
        corrupt.recovery.as_mut().unwrap().ciphertext[0] ^= 1;
        assert!(vault.unlock(&corrupt, "new password").is_err());
        assert!(vault.unlock(&next, "old password").is_err());
        assert!(!vault.unlocked());
        vault.unlock(&next, "new password").unwrap();
        assert_eq!(&**vault.open("groups/state", &data).unwrap(), b"group keys");
        vault
            .recover(&next, "словоодин словодва словотри словочетыре")
            .unwrap();
        assert_eq!(&**vault.open("groups/state", &data).unwrap(), b"group keys");
        // Ошибка смены не портит прежний envelope, он сохраняется до успешной публикации нового.
        vault.unlock(&envelope, "old password").unwrap();
        assert_eq!(&**vault.open("groups/state", &data).unwrap(), b"group keys");
    }

    #[test]
    fn corrupt_envelopes_rejected_without_mutation_and_live_key_revoked() {
        let envelope = VaultEnvelope::create("correct password", None).unwrap();
        let original = serde_json::to_vec(&envelope).unwrap();
        let mut vault = NodeVault::default();
        vault.unlock(&envelope, "correct password").unwrap();
        let mut wrong = envelope.clone();
        wrong.version = 0;
        assert!(vault.unlock(&wrong, "correct password").is_err());
        assert!(!vault.unlocked());
        wrong = envelope.clone();
        wrong.password.ciphertext.pop();
        assert!(vault.unlock(&wrong, "correct password").is_err());
        wrong = envelope.clone();
        wrong.id[0] ^= 1;
        assert!(vault.unlock(&wrong, "correct password").is_err());
        wrong = envelope.clone();
        wrong.password.ciphertext[0] ^= 1;
        assert!(vault.unlock(&wrong, "correct password").is_err());
        assert_eq!(serde_json::to_vec(&envelope).unwrap(), original);
        assert!(vault
            .recover(&envelope, "first second third fourth")
            .is_err());
        assert!(VaultEnvelope::create("short", None).is_err());
        assert!(VaultEnvelope::create("password", Some("one two three four")).is_err());
        assert!(phrase_secret("abcde fghij klmno pqrst extra").is_err());
        assert!(serde_json::from_str::<VaultEnvelope>("{}").is_err());
    }
}
