//! Подписанный выпуск ноды: манифест (версия, номер против отката, хэши файлов) и подпись Ed25519.
//!
//! Ключ подписи выводится из фразы разработчика (Argon2id, отдельная соль: другой ключ, чем у APK, даже при той же фразе); сам ключ
//! нигде не хранится. Открытый ключ публичен и закрепляется у проверяющего (`--pubkey`/файл рядом с установщиком).
//! Проверяется ТОЛЬКО установщиком/обновлятором и командой `yandi-release verify`: запущенная нода сама себя проверить не может
//! (подменённый бинарник пропустит проверку).
use argon2::{Algorithm, Argon2, Params, Version};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

const KEY_SALT: &[u8] = b"YANDI-NODE-RELEASE-KEY-V1";
const SIG_DOMAIN: &[u8] = b"YANDI-RELEASE-MANIFEST-V1\0";
pub const MANIFEST_NAME: &str = "MANIFEST.json";
pub const SIGNATURE_NAME: &str = "MANIFEST.json.sig";
const MAX_MANIFEST: usize = 1 << 20;
const MAX_FILES: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub product: String,
    pub version: String,
    /// монотонный номер выпуска: проверяющий отвергает номер ниже уже установленного (защита от отката)
    pub seq: u64,
    pub created: u64,
    /// первые 16 hex SHA-256 открытого ключа: чтобы сразу видеть, чьей подписью выпуск
    pub key_id: String,
    pub files: Vec<FileEntry>,
}

pub fn normalize(p: &str) -> String {
    p.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Почему фраза слишком слабая (None — годится). Открытый ключ публичен, слабую фразу подберут офлайн.
pub fn weakness(p: &str) -> Option<&'static str> {
    let words: Vec<&str> = p.split(' ').collect();
    let distinct: std::collections::HashSet<&str> = words.iter().copied().collect();
    if words.len() < 12 {
        return Some("нужно не меньше 12 слов");
    }
    if p.chars().count() < 60 {
        return Some("нужно не меньше 60 символов");
    }
    if distinct.len() < 10 {
        return Some("слишком много повторяющихся слов");
    }
    None
}

/// Параметры для боевого вывода ключа; тесты берут дешёвые.
pub fn production_params() -> Params {
    Params::new(1 << 20, 4, 1, Some(32)).expect("argon2 params")
}

pub fn derive_signing_key(phrase: &str, params: Params) -> SigningKey {
    let mut seed = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(normalize(phrase).as_bytes(), KEY_SALT, &mut seed)
        .expect("argon2");
    SigningKey::from_bytes(&seed)
}

pub fn key_id(vk: &VerifyingKey) -> String {
    hex::encode(&Sha256::digest(vk.to_bytes())[..8])
}

pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(h.finalize()), size))
}

/// Имя файла в манифесте: только простое имя внутри каталога выпуска (никаких путей и «..»).
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name != MANIFEST_NAME
        && name != SIGNATURE_NAME
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c.is_control())
}

pub fn build_manifest(dir: &Path, version: &str, seq: u64, created: u64, key_id: &str, names: &[String]) -> Result<Manifest, String> {
    if names.is_empty() || names.len() > MAX_FILES {
        return Err("нужен хотя бы один файл (не больше 256)".into());
    }
    let mut files = Vec::new();
    for name in names {
        if !plain_name(name) {
            return Err(format!("недопустимое имя файла: {name}"));
        }
        let (sha, size) = sha256_file(&dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        files.push(FileEntry { name: name.clone(), sha256: sha, size });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    if files.windows(2).any(|w| w[0].name == w[1].name) {
        return Err("повторяющиеся имена файлов".into());
    }
    Ok(Manifest { format: 1, product: "yandi-node".into(), version: version.into(), seq, created, key_id: key_id.into(), files })
}

pub fn manifest_bytes(m: &Manifest) -> Vec<u8> {
    serde_json::to_vec_pretty(m).expect("serialize manifest")
}

/// Подпись над ТОЧНЫМИ байтами файла манифеста с доменным префиксом (никакой повторной канонизации).
pub fn sign(manifest: &[u8], key: &SigningKey) -> String {
    let sig: Signature = key.sign(&[SIG_DOMAIN, manifest].concat());
    hex::encode(sig.to_bytes())
}

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    BadKey,
    BadSignature,
    TooLarge,
    Malformed(String),
    WrongKeyId,
    Rollback { seq: u64, min: u64 },
    File(String),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadKey => write!(f, "открытый ключ непригоден"),
            Self::BadSignature => write!(f, "подпись манифеста НЕ верна"),
            Self::TooLarge => write!(f, "манифест слишком большой"),
            Self::Malformed(e) => write!(f, "манифест повреждён: {e}"),
            Self::WrongKeyId => write!(f, "key_id в манифесте не совпадает с закреплённым ключом"),
            Self::Rollback { seq, min } => write!(f, "откат: выпуск №{seq} старше уже установленного №{min}"),
            Self::File(e) => write!(f, "{e}"),
        }
    }
}

/// Проверить подпись и разобрать манифест. `min_seq` — номер уже установленного выпуска (0 — не проверять).
pub fn verify_manifest(manifest: &[u8], sig_hex: &str, vk: &VerifyingKey, min_seq: u64) -> Result<Manifest, VerifyError> {
    if manifest.len() > MAX_MANIFEST {
        return Err(VerifyError::TooLarge);
    }
    let sig_bytes: [u8; 64] = hex::decode(sig_hex.trim()).ok().and_then(|b| b.try_into().ok()).ok_or(VerifyError::BadSignature)?;
    vk.verify(&[SIG_DOMAIN, manifest].concat(), &Signature::from_bytes(&sig_bytes)).map_err(|_| VerifyError::BadSignature)?;
    let m: Manifest = serde_json::from_slice(manifest).map_err(|e| VerifyError::Malformed(e.to_string()))?;
    if m.format != 1 || m.product != "yandi-node" || m.files.is_empty() || m.files.len() > MAX_FILES {
        return Err(VerifyError::Malformed("неизвестный формат/состав".into()));
    }
    if m.key_id != key_id(vk) {
        return Err(VerifyError::WrongKeyId);
    }
    if m.seq < min_seq {
        return Err(VerifyError::Rollback { seq: m.seq, min: min_seq });
    }
    for f in &m.files {
        if !plain_name(&f.name) || f.sha256.len() != 64 || !f.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(VerifyError::Malformed(format!("недопустимая запись: {}", f.name)));
        }
    }
    Ok(m)
}

/// Файлы выпуска в каталоге совпадают с манифестом (размер и SHA-256). Лишние файлы не ошибка (их просто никто не установит).
pub fn check_files(dir: &Path, m: &Manifest) -> Result<(), VerifyError> {
    for f in &m.files {
        let p = dir.join(&f.name);
        let meta = std::fs::symlink_metadata(&p).map_err(|e| VerifyError::File(format!("{}: {e}", f.name)))?;
        if !meta.is_file() {
            return Err(VerifyError::File(format!("{}: не обычный файл", f.name)));
        }
        let (sha, size) = sha256_file(&p).map_err(|e| VerifyError::File(format!("{}: {e}", f.name)))?;
        if size != f.size || sha != f.sha256 {
            return Err(VerifyError::File(format!("{}: содержимое не совпало с манифестом", f.name)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cheap() -> Params {
        Params::new(64, 1, 1, Some(32)).unwrap()
    }

    fn key() -> SigningKey {
        derive_signing_key("один два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать", cheap())
    }

    fn release(dir: &Path, seq: u64) -> (Vec<u8>, String, VerifyingKey) {
        std::fs::write(dir.join("yandi-node-linux-x86_64.tar.gz"), b"binary bytes").unwrap();
        std::fs::write(dir.join("README.txt"), b"readme").unwrap();
        let k = key();
        let vk = k.verifying_key();
        let m = build_manifest(dir, "0.2.0", seq, 1_700_000_000, &key_id(&vk), &["yandi-node-linux-x86_64.tar.gz".into(), "README.txt".into()]).unwrap();
        let bytes = manifest_bytes(&m);
        let sig = sign(&bytes, &k);
        (bytes, sig, vk)
    }

    #[test]
    fn signed_release_verifies_and_files_match() {
        let d = tempfile::tempdir().unwrap();
        let (bytes, sig, vk) = release(d.path(), 7);
        let m = verify_manifest(&bytes, &sig, &vk, 7).unwrap();
        assert_eq!(m.seq, 7);
        check_files(d.path(), &m).unwrap();
        assert_eq!(derive_signing_key("один два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать", cheap()).to_bytes(), key().to_bytes());
        assert_ne!(derive_signing_key("другая фраза совсем не такая как первая ни одним словом вообще", cheap()).to_bytes(), key().to_bytes());
    }

    #[test]
    fn any_change_to_manifest_signature_key_or_files_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let (bytes, sig, vk) = release(d.path(), 7);
        // изменённый байт манифеста
        let mut bad = bytes.clone();
        let i = bad.iter().position(|b| *b == b'7').unwrap();
        bad[i] = b'8';
        assert_eq!(verify_manifest(&bad, &sig, &vk, 0), Err(VerifyError::BadSignature));
        // изменённая подпись и мусор вместо подписи
        let mut s = sig.clone().into_bytes();
        s[0] = if s[0] == b'0' { b'1' } else { b'0' };
        assert_eq!(verify_manifest(&bytes, std::str::from_utf8(&s).unwrap(), &vk, 0), Err(VerifyError::BadSignature));
        assert_eq!(verify_manifest(&bytes, "zz", &vk, 0), Err(VerifyError::BadSignature));
        // чужой ключ
        let other = derive_signing_key("совсем другая длинная фраза из многих слов для чужого ключа подписи выпусков", cheap());
        assert_eq!(verify_manifest(&bytes, &sig, &other.verifying_key(), 0), Err(VerifyError::BadSignature));
        // подмена файла после подписи
        let m = verify_manifest(&bytes, &sig, &vk, 0).unwrap();
        std::fs::write(d.path().join("yandi-node-linux-x86_64.tar.gz"), b"evil bytes!").unwrap();
        assert!(matches!(check_files(d.path(), &m), Err(VerifyError::File(_))));
        // отсутствующий файл
        std::fs::remove_file(d.path().join("README.txt")).unwrap();
        assert!(matches!(check_files(d.path(), &m), Err(VerifyError::File(_))));
    }

    #[test]
    fn rollback_oversize_and_bad_names_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let (bytes, sig, vk) = release(d.path(), 3);
        assert_eq!(verify_manifest(&bytes, &sig, &vk, 4), Err(VerifyError::Rollback { seq: 3, min: 4 }));
        assert!(verify_manifest(&bytes, &sig, &vk, 3).is_ok());
        assert_eq!(verify_manifest(&vec![b' '; MAX_MANIFEST + 1], &sig, &vk, 0), Err(VerifyError::TooLarge));
        for bad in ["../evil", "a/b", "", "..", "MANIFEST.json", "x\ny"] {
            assert!(build_manifest(d.path(), "1", 1, 0, "k", &[bad.into()]).is_err(), "{bad:?}");
        }
        // манифест, подписанный верно, но с «..» в имени, при проверке тоже не принимается
        let k = key();
        let evil = Manifest { format: 1, product: "yandi-node".into(), version: "1".into(), seq: 1, created: 0, key_id: key_id(&k.verifying_key()),
            files: vec![FileEntry { name: "../etc/passwd".into(), sha256: "0".repeat(64), size: 1 }] };
        let eb = manifest_bytes(&evil);
        let es = sign(&eb, &k);
        assert!(matches!(verify_manifest(&eb, &es, &k.verifying_key(), 0), Err(VerifyError::Malformed(_))));
    }

    #[test]
    fn weak_phrases_are_refused_and_spaces_do_not_change_the_key() {
        assert!(weakness("слишком короткая фраза").is_some());
        assert!(weakness(&"слово ".repeat(14).trim().to_string()).is_some());
        assert_eq!(normalize("  а   б \t в "), "а б в");
        let a = derive_signing_key("раз  два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать", cheap());
        let b = derive_signing_key("раз два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать", cheap());
        assert_eq!(a.to_bytes(), b.to_bytes());
    }
}
