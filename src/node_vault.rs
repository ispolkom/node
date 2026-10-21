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

#[cfg(unix)]
pub mod store;

const FORMAT: u32 = 1;
const MAX_SECRET: usize = 1024;
const MAX_DATA: usize = 16 * 1024 * 1024;
const KEY_AAD: &[u8] = b"YANDI-NODE-CLOSED-KEY-V1\0";
const DATA_AAD: &[u8] = b"YANDI-NODE-CLOSED-DATA-V1\0";

static KDF_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
static KDF_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Тестовая разметка: попытки входа, начатые внутри `KDF_TAG.scope(метка, …)`, считаются отдельно от общего счётчика, чтобы параллельные
/// тесты не сдвигали друг другу «ни одного вызова KDF».
#[cfg(test)]
tokio::task_local! {
    pub(crate) static KDF_TAG: u64;
}
#[cfg(test)]
thread_local! {
    static WORKER_TAG: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
static KDF_TAGGED: std::sync::Mutex<Vec<(u64, usize)>> = std::sync::Mutex::new(Vec::new());
#[cfg(test)]
pub(crate) fn set_worker_tag(tag: Option<u64>) {
    WORKER_TAG.with(|c| c.set(tag));
}
/// Метка текущего async-контекста теста (если он внутри `KDF_TAG.scope`).
#[cfg(test)]
pub(crate) fn current_tag() -> Option<u64> {
    KDF_TAG.try_with(|t| *t).ok()
}
/// Сколько вызовов KDF сделали попытки с этой меткой.
#[cfg(test)]
pub(crate) fn tagged_kdf_calls(tag: u64) -> usize {
    KDF_TAGGED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(t, _)| *t == tag).map(|(_, n)| *n).unwrap_or(0)
}
#[cfg(test)]
static KDF_ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static KDF_PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
struct KdfProbe;
#[cfg(test)]
impl KdfProbe {
    fn new() -> Self {
        use std::sync::atomic::Ordering;
        let active = KDF_ACTIVE.fetch_add(1, Ordering::SeqCst) + 1;
        KDF_PEAK.fetch_max(active, Ordering::SeqCst);
        Self
    }
}
#[cfg(test)]
impl Drop for KdfProbe {
    fn drop(&mut self) {
        KDF_ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

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
    #[serde(default)]
    revision: u64,
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

pub(crate) fn phrase_secret(phrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    if phrase.len() > MAX_SECRET {
        bail!("слишком длинная фраза");
    }
    // Сид-фраза (12 или 24 слова): ключ делается из байтов сида, а не из текста; регистр и пробелы не важны.
    let seed = key_root::Seed::parse(phrase).map_err(|e| anyhow!("{e}"))?;
    Ok(Zeroizing::new(seed.secret().as_bytes().to_vec()))
}

fn wrapping_key(secret: &[u8], salt: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    // Даже внутренние синхронные примитивы не растягивают два ключа одновременно.
    let _single = KDF_MUTEX
        .lock()
        .map_err(|_| anyhow!("очередь KDF остановлена после ошибки"))?;
    #[cfg(test)]
    KDF_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    if let Some(tag) = WORKER_TAG.with(|c| c.get()) {
        let mut g = KDF_TAGGED.lock().unwrap_or_else(|e| e.into_inner());
        match g.iter_mut().find(|(t, _)| *t == tag) {
            Some((_, n)) => *n += 1,
            None => g.push((tag, 1)),
        }
    }
    #[cfg(test)]
    let _probe = KdfProbe::new();
    // Цена подбора копии диска: в бою 256 МиБ и 3 прохода (около секунды на попытку, GPU почти не ускоряет);
    // в тестах прежние 64 МиБ, чтобы набор не растягивался.
    let memory_kib = if cfg!(test) { 65536 } else { 262_144 };
    let params = Params::new(memory_kib, 3, 1, Some(32)).map_err(|_| anyhow!("параметры KDF"))?;
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
        Ok(Self::create_unlocked(password, recovery)?.0)
    }

    /// Новый envelope и сразу открытый сеанс с тем же DK: второй KDF для первого входа не нужен.
    pub fn create_unlocked(password: &str, recovery: Option<&str>) -> Result<(Self, NodeVault)> {
        password_ok(password)?;
        let phrase = recovery.map(phrase_secret).transpose()?;
        // с сид-фразой (и контрольным словом) номер и ключ среды выводятся из неё: та же фраза на новой машине даёт ту же среду, а значит и тот же
        // ключ подписи списка узлов. Без фразы (старый путь) они случайны.
        let (id, key): ([u8; 16], Zeroizing<[u8; 32]>) = match &phrase {
            Some(secret) if key_root::looks_like_seed_secret(secret) => {
                let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(b"YANDI-NODE-CLOSED-SEED-V1\0"), secret.as_slice());
                let mut id = [0u8; 16];
                let mut key = Zeroizing::new([0u8; 32]);
                hk.expand(b"vault-id", &mut id).map_err(|_| anyhow!("HKDF"))?;
                hk.expand(b"vault-key", key.as_mut()).map_err(|_| anyhow!("HKDF"))?;
                (id, key)
            }
            _ => (random(), Zeroizing::new(random())),
        };
        let mut envelope = Self {
            version: FORMAT,
            id,
            password: wrap(&key, password.as_bytes(), &id, b"password")?,
            recovery: phrase
                .as_ref()
                .map(|s| wrap(&key, s, &id, b"recovery"))
                .transpose()?,
            tag: [0; 32],
            revision: 0,
        };
        envelope.tag = envelope.mac(&key).finalize().into_bytes().into();
        let vault = NodeVault {
            session: Some(Session { id, key }),
        };
        Ok((envelope, vault))
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
        // revision=0 сохраняет совместимость с envelope криптоосновы раунда 14.
        if self.revision != 0 {
            mac.update(b"storage-revision\0");
            mac.update(&self.revision.to_be_bytes());
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

    /// Отдельный 32-байтовый ключ для области данных (чаты, группы ПК, ключи «ПК как собеседник»).
    /// Разные области не выводятся друг из друга; копию вызывающий слой стирает при блокировке.
    pub fn subkey(&self, domain: &str) -> Result<Zeroizing<[u8; 32]>> {
        if domain.is_empty() || domain.len() > 128 {
            bail!("неверная область данных");
        }
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow!("закрытая среда заперта"))?;
        let info = [b"YANDI-NODE-CLOSED-SUBKEY-V1\0".as_slice(), session.id.as_slice(), domain.as_bytes()].concat();
        let mut key = Zeroizing::new([0; 32]);
        hkdf::Hkdf::<sha2::Sha256>::new(Some(DATA_AAD), session.key.as_ref())
            .expand(&info, key.as_mut())
            .map_err(|_| anyhow!("HKDF"))?;
        Ok(key)
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
            Some("legal winner thank year wave sausage worth useful legal winner thank yellow"),
        )
        .unwrap();
        let mut vault = NodeVault::default();
        assert!(vault.change_password(&envelope, "new password").is_err());
        vault
            .recover(&envelope, "Legal winner\nthank  year wave sausage worth useful legal winner thank yellow")
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
            .recover(&next, "legal winner thank year wave sausage worth useful legal winner thank yellow")
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

    #[test]
    fn subkeys_are_domain_separated_and_follow_lock() {
        let (envelope, mut vault) = VaultEnvelope::create_unlocked("owner password", None).unwrap();
        let chat = vault.subkey("chat").unwrap();
        assert_eq!(*chat, *vault.subkey("chat").unwrap());
        assert_ne!(*chat, *vault.subkey("groups_pc").unwrap());
        assert!(vault.subkey("").is_err());
        // сеанс из create_unlocked тот же DK, что даёт вход паролем
        let mut again = NodeVault::default();
        again.unlock(&envelope, "owner password").unwrap();
        assert_eq!(*chat, *again.subkey("chat").unwrap());
        vault.lock();
        assert!(vault.subkey("chat").is_err());
    }

    #[test]
    fn recovery_phrase_must_be_a_valid_seed() {
        // 12 слов с неверной контрольной суммой, чужое слово, неверное число слов, прежний формат из четырёх слов
        for bad in [
            "winner legal thank year wave sausage worth useful legal winner thank yellow",
            "legal winner thank year wave sausage worth useful legal winner thank yelow",
            "legal winner thank year wave sausage worth useful legal winner thank",
            "alpha1 bravo2 charlie3 delta4",
        ] {
            assert!(VaultEnvelope::create("owner password", Some(bad)).is_err(), "{bad}");
        }
        // 24 слова тоже годятся, регистр и пробелы не важны
        let long = format!("{}art", "abandon ".repeat(23));
        assert!(VaultEnvelope::create("owner password", Some(&long)).is_ok());
        assert!(VaultEnvelope::create("owner password", Some(&long.to_uppercase())).is_ok());
    }

    #[test]
    fn the_same_seed_phrase_makes_the_same_environment_keys_on_any_machine_and_the_control_word_makes_others() {
        let words = "legal winner thank year wave sausage worth useful legal winner thank yellow";
        let joined = key_root::join_phrase(words, "Воробей-2026");
        let vault_of = |phrase: Option<&str>, password: &str| VaultEnvelope::create_unlocked(password, phrase).unwrap().1;
        let a = vault_of(Some(words), "password-one-1");
        // другая машина, другой пароль — те же ключи области
        let b = vault_of(Some(words), "another-pass-2");
        assert_eq!(*a.subkey("bootstrap_signer").unwrap(), *b.subkey("bootstrap_signer").unwrap());
        assert_eq!(*a.subkey("chat").unwrap(), *b.subkey("chat").unwrap());
        // область данных по-прежнему отдельная
        assert_ne!(*a.subkey("bootstrap_signer").unwrap(), *a.subkey("chat").unwrap());
        // с контрольным словом — другая среда
        let c = vault_of(Some(&joined), "password-one-1");
        assert_ne!(*a.subkey("bootstrap_signer").unwrap(), *c.subkey("bootstrap_signer").unwrap());
        // без фразы — случайная среда, как раньше
        let d = vault_of(None, "password-one-1");
        assert_ne!(*a.subkey("chat").unwrap(), *d.subkey("chat").unwrap());
    }
}
