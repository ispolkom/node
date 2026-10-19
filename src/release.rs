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
use zeroize::Zeroizing;

const KEY_SALT: &[u8] = b"YANDI-NODE-RELEASE-KEY-V1";
const SIG_DOMAIN: &[u8] = b"YANDI-RELEASE-MANIFEST-V1\0";
pub const MANIFEST_NAME: &str = "MANIFEST.json";
pub const SIGNATURE_NAME: &str = "MANIFEST.json.sig";
pub const MAX_MANIFEST: usize = 1 << 20;
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
    /// платформа выпуска (`deb`, `android`, `win`…); пусто — прежний манифест без платформы
    #[serde(default)]
    pub platform: String,
    /// срок действия манифеста (секунды Unix; 0 — без срока): защита от «замораживания» — свежего подписанного файла нет, а
    /// старый вечно подсовывают
    #[serde(default)]
    pub expires: u64,
    /// краткое описание изменений для уведомления об обновлении (до 2000 знаков)
    #[serde(default)]
    pub notes: String,
    /// кошелёк поддержки проекта (не узла): подписан вместе с выпуском, меняется одной строкой манифеста
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<SupportWallet>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SupportWallet {
    pub asset: String,
    pub network: String,
    pub address: String,
}

/// Адрес Tron: Base58Check, 25 байт, префикс сети 0x41, контрольные 4 байта — двойной SHA-256. Опечатка в адресе поддержки — потерянные деньги,
/// поэтому в подписанный выпуск попадает только проверенный адрес (пока поддерживается только сеть Tron).
pub fn tron_address_ok(addr: &str) -> bool {
    use sha2::{Digest, Sha256};
    const B58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    if addr.len() != 34 {
        return false;
    }
    let mut num = [0u8; 25];
    for c in addr.bytes() {
        let Some(d) = B58.iter().position(|b| *b == c) else { return false };
        let mut carry = d as u32;
        for b in num.iter_mut().rev() {
            carry += 58 * (*b as u32);
            *b = (carry & 0xff) as u8;
            carry >>= 8;
        }
        if carry != 0 {
            return false;
        }
    }
    let sum = Sha256::digest(Sha256::digest(&num[..21]));
    num[0] == 0x41 && sum[..4] == num[21..]
}

impl SupportWallet {
    /// Разбор `Монета|Сеть|Адрес` (так задаётся при выпуске).
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split('|');
        let w = Self { asset: it.next()?.trim().into(), network: it.next()?.trim().into(), address: it.next()?.trim().into() };
        (it.next().is_none() && w.valid()).then_some(w)
    }
    fn valid(&self) -> bool {
        (1..=16).contains(&self.asset.chars().count())
            && (1..=32).contains(&self.network.chars().count())
            && !self.asset.chars().chain(self.network.chars()).any(|c| c.is_control() || c == '|')
            && tron_address_ok(&self.address)
    }
}

/// Release phrase normalization v1: NFC (ICU4X 2.2.0), then Unicode whitespace collapse.
pub const PHRASE_NORMALIZATION_VERSION: u32 = 1;
pub fn normalize(p: &str) -> Zeroizing<String> {
    let mut nfc = Zeroizing::new(String::new());
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc()
        .normalize_to(p, &mut *nfc)
        .expect("String write");
    let mut out = Zeroizing::new(String::new());
    for word in nfc.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
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

fn derive_signing_seed(phrase: &str, params: Params) -> Zeroizing<[u8; 32]> {
    let mut seed = Zeroizing::new([0u8; 32]);
    let normalized = normalize(phrase);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(normalized.as_bytes(), KEY_SALT, &mut *seed)
        .expect("argon2");
    seed
}
pub fn derive_signing_key(phrase: &str, params: Params) -> SigningKey {
    SigningKey::from_bytes(&derive_signing_seed(phrase, params))
}

pub fn key_id(vk: &VerifyingKey) -> String {
    hex::encode(&Sha256::digest(vk.to_bytes())[..8])
}

/// Open each component without following links; all subsequent reads use this fd.
#[cfg(unix)]
pub struct ReleaseDir(std::fs::File);
#[cfg(unix)]
impl ReleaseDir {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        use std::os::unix::{
            fs::OpenOptionsExt,
            io::{AsRawFd, FromRawFd},
        };
        let mut dir = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(if path.is_absolute() { "/" } else { "." })?;
        for component in path.components() {
            use std::path::Component;
            let name = match component {
                Component::RootDir | Component::CurDir => continue,
                Component::Normal(n) => std::ffi::CString::new(n.as_encoded_bytes())?,
                _ => return Err(std::io::Error::other("unsafe directory component")),
            };
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            dir = unsafe { std::fs::File::from_raw_fd(fd) };
        }
        Ok(Self(dir))
    }
    pub fn file(&self, name: &str) -> std::io::Result<std::fs::File> {
        use std::os::unix::io::{AsRawFd, FromRawFd};
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name == "."
            || name == ".."
        {
            return Err(std::io::Error::other("unsafe file name"));
        }
        let name = std::ffi::CString::new(name)?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let f = unsafe { std::fs::File::from_raw_fd(fd) };
        if !f.metadata()?.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        Ok(f)
    }
    pub fn read(&self, name: &str, limit: u64) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let f = self.file(name)?;
        if f.metadata()?.len() > limit {
            return Err(std::io::Error::other("file too large"));
        }
        let mut bytes = Vec::new();
        f.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(std::io::Error::other("file too large"));
        }
        Ok(bytes)
    }
}

/// Без openat/O_NOFOLLOW (не Unix): проверяем звенья ссылок через symlink_metadata, остальное то же.
#[cfg(not(unix))]
pub struct ReleaseDir(std::path::PathBuf);
#[cfg(not(unix))]
impl ReleaseDir {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(std::io::Error::other("unsafe directory component"));
        }
        Ok(Self(path.to_path_buf()))
    }
    pub fn file(&self, name: &str) -> std::io::Result<std::fs::File> {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
            return Err(std::io::Error::other("unsafe file name"));
        }
        let path = self.0.join(name);
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        std::fs::File::open(path)
    }
    pub fn read(&self, name: &str, limit: u64) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let f = self.file(name)?;
        if f.metadata()?.len() > limit {
            return Err(std::io::Error::other("file too large"));
        }
        let mut bytes = Vec::new();
        f.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(std::io::Error::other("file too large"));
        }
        Ok(bytes)
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let dir = ReleaseDir::open(path.parent().unwrap_or(Path::new(".")))?;
    let mut f = dir.file(
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| std::io::Error::other("bad name"))?,
    )?;
    sha256_fd(&mut f)
}

pub fn sha256_fd(f: &mut std::fs::File) -> std::io::Result<(String, u64)> {
    use std::io::Read;
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

pub fn build_manifest(
    dir: &Path,
    version: &str,
    seq: u64,
    created: u64,
    key_id: &str,
    names: &[String],
) -> Result<Manifest, String> {
    if names.is_empty() || names.len() > MAX_FILES {
        return Err("нужен хотя бы один файл (не больше 256)".into());
    }
    let mut files = Vec::new();
    for name in names {
        if !plain_name(name) {
            return Err(format!("недопустимое имя файла: {name}"));
        }
        let (sha, size) = sha256_file(&dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        files.push(FileEntry {
            name: name.clone(),
            sha256: sha,
            size,
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let m = Manifest {
        format: 1,
        product: "yandi-node".into(),
        version: version.into(),
        seq,
        created,
        key_id: key_id.into(),
        files,
        platform: String::new(),
        expires: 0,
        notes: String::new(),
        support: None,
    };
    validate_manifest(&m).map_err(|e| e.to_string())?;
    Ok(m)
}

pub fn manifest_bytes(m: &Manifest) -> Vec<u8> {
    serde_json::to_vec_pretty(m).expect("serialize manifest")
}

/// Подпись над ТОЧНЫМИ байтами файла манифеста с доменным префиксом (никакой повторной канонизации).
pub fn sign(manifest: &[u8], key: &SigningKey) -> Result<String, VerifyError> {
    let m = parse_manifest(manifest)?;
    if m.key_id != key_id(&key.verifying_key()) {
        return Err(VerifyError::WrongKeyId);
    }
    let sig: Signature = key.sign(&[SIG_DOMAIN, manifest].concat());
    Ok(hex::encode(sig.to_bytes()))
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
            Self::Rollback { seq, min } => {
                write!(f, "откат: выпуск №{seq} старше уже установленного №{min}")
            }
            Self::File(e) => write!(f, "{e}"),
        }
    }
}

/// Проверить подпись и разобрать манифест. `min_seq` — номер уже установленного выпуска (0 — не проверять).
pub fn verify_manifest(
    manifest: &[u8],
    sig_hex: &str,
    vk: &VerifyingKey,
    min_seq: u64,
) -> Result<Manifest, VerifyError> {
    if manifest.len() > MAX_MANIFEST {
        return Err(VerifyError::TooLarge);
    }
    if !lower_hex(sig_hex.trim(), 128) {
        return Err(VerifyError::BadSignature);
    }
    let sig_bytes: [u8; 64] = hex::decode(sig_hex.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(VerifyError::BadSignature)?;
    vk.verify(
        &[SIG_DOMAIN, manifest].concat(),
        &Signature::from_bytes(&sig_bytes),
    )
    .map_err(|_| VerifyError::BadSignature)?;
    let m = parse_manifest(manifest)?;
    if m.key_id != key_id(vk) {
        return Err(VerifyError::WrongKeyId);
    }
    if m.seq < min_seq {
        return Err(VerifyError::Rollback {
            seq: m.seq,
            min: min_seq,
        });
    }
    Ok(m)
}

pub fn lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn validate_manifest(m: &Manifest) -> Result<(), VerifyError> {
    if m.format != 1
        || m.product != "yandi-node"
        || m.seq == 0
        || m.version.is_empty()
        || m.version.len() > 200
        || m.version.chars().any(char::is_control)
        || !lower_hex(&m.key_id, 16)
        || m.files.is_empty()
        || m.files.len() > MAX_FILES
        || m.platform.len() > 32
        || !m.platform.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || m.notes.chars().count() > 2000
        || m.support.as_ref().is_some_and(|w| !w.valid())
    {
        return Err(VerifyError::Malformed("неизвестный формат/состав".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for f in &m.files {
        if !plain_name(&f.name) || !lower_hex(&f.sha256, 64) || !seen.insert(&f.name) {
            return Err(VerifyError::Malformed(format!(
                "недопустимая запись: {}",
                f.name
            )));
        }
    }
    Ok(())
}
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, VerifyError> {
    if bytes.len() > MAX_MANIFEST {
        return Err(VerifyError::TooLarge);
    }
    let m = serde_json::from_slice(bytes).map_err(|e| VerifyError::Malformed(e.to_string()))?;
    validate_manifest(&m)?;
    Ok(m)
}

/// Файлы выпуска в каталоге совпадают с манифестом (размер и SHA-256). Лишние файлы не ошибка (их просто никто не установит).
pub fn check_files(dir: &Path, m: &Manifest) -> Result<(), VerifyError> {
    validate_manifest(m)?;
    let dir = ReleaseDir::open(dir).map_err(|e| VerifyError::File(e.to_string()))?;
    for f in &m.files {
        let mut fd = dir
            .file(&f.name)
            .map_err(|e| VerifyError::File(format!("{}: {e}", f.name)))?;
        let (sha, size) =
            sha256_fd(&mut fd).map_err(|e| VerifyError::File(format!("{}: {e}", f.name)))?;
        if size != f.size || sha != f.sha256 {
            return Err(VerifyError::File(format!(
                "{}: содержимое не совпало с манифестом",
                f.name
            )));
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

    /// Настоящий путь без ссылок в звеньях (на macOS /var → /private/var): ReleaseDir не ходит по ссылкам.
    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
    }

    fn key() -> SigningKey {
        derive_signing_key(
            "один два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать",
            cheap(),
        )
    }

    fn release(dir: &Path, seq: u64) -> (Vec<u8>, String, VerifyingKey) {
        std::fs::write(dir.join("yandi-node-linux-x86_64.tar.gz"), b"binary bytes").unwrap();
        std::fs::write(dir.join("README.txt"), b"readme").unwrap();
        let k = key();
        let vk = k.verifying_key();
        let m = build_manifest(
            dir,
            "0.2.0",
            seq,
            1_700_000_000,
            &key_id(&vk),
            &["yandi-node-linux-x86_64.tar.gz".into(), "README.txt".into()],
        )
        .unwrap();
        let bytes = manifest_bytes(&m);
        let sig = sign(&bytes, &k).unwrap();
        (bytes, sig, vk)
    }

    #[test]
    fn r7_nfc_vectors_and_legacy_nfc_key_compatibility() {
        for (input, expected) in [
            (" е\u{308}  и\u{306} ", "ё й"),
            ("Cafe\u{301}\tÅ", "Café Å"),
            ("A\u{30a}", "Å"),
        ] {
            assert_eq!(&*normalize(input), expected);
            assert_eq!(
                derive_signing_key(input, cheap()).to_bytes(),
                derive_signing_key(expected, cheap()).to_bytes()
            );
        }
        // Independent libargon2 v0x13 vectors: m=64 KiB, t=1, p=1, 32 bytes.
        for (phrase, expected) in [
            (
                "ё й",
                "b9d2f7a2953d739fba0712d0567314ead3ca7a07479414d51b35b7f1807d11c6",
            ),
            (
                "Café Å",
                "716695edf93b61302a06064fd43e7a1ec3f740c4b00eeb6969c7bf5d39601527",
            ),
            (
                "Å",
                "d8e7911bb15a707acb7709f089ebed88c0203b7ec8fed4731530b5ea2f1c44c9",
            ),
        ] {
            assert_eq!(
                hex::encode(&*derive_signing_seed(phrase, cheap())),
                expected
            );
        }
        // Exact legacy KDF for an already NFC phrase: salt and parameters unchanged.
        let phrase =
            "один два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать";
        let mut legacy = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, cheap())
            .hash_password_into(phrase.as_bytes(), KEY_SALT, &mut *legacy)
            .unwrap();
        assert_eq!(derive_signing_key(phrase, cheap()).to_bytes(), *legacy);
    }

    #[test]
    fn r6_normalized_phrase_is_zeroizing() {
        let phrase: Zeroizing<String> = normalize("  secret   words ");
        assert_eq!(&*phrase, "secret words");
        let mut seed: Zeroizing<[u8; 32]> = derive_signing_seed(&phrase, cheap());
        use zeroize::Zeroize;
        seed.zeroize();
        assert_eq!(*seed, [0; 32]);
    }

    #[test]
    fn r9_validator_rejects_duplicates_uppercase_and_invalid_before_signing() {
        let d = tmpdir();
        let (bytes, _, _) = release(d.path(), 1);
        let good = parse_manifest(&bytes).unwrap();
        assert_eq!(
            verify_manifest(
                &bytes,
                &sign(&bytes, &key()).unwrap().to_uppercase(),
                &key().verifying_key(),
                0
            ),
            Err(VerifyError::BadSignature)
        );
        let k = key();
        for mode in 0..4 {
            let mut m = good.clone();
            match mode {
                0 => m.files.push(m.files[0].clone()),
                1 => m.files[0].sha256 = "A".repeat(64),
                2 => m.seq = 0,
                _ => m.key_id = "A".repeat(16),
            }
            let bytes = manifest_bytes(&m);
            assert!(sign(&bytes, &k).is_err());
            let sig = hex::encode(k.sign(&[SIG_DOMAIN, &bytes].concat()).to_bytes());
            assert!(verify_manifest(&bytes, &sig, &k.verifying_key(), 0).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn r5_pinned_directory_and_fd_survive_path_replacement() {
        use std::os::unix::fs::symlink;
        let root = tmpdir();
        let path = root.path().join("release");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("file"), b"verified").unwrap();
        let dir = ReleaseDir::open(&path).unwrap();
        let mut fd = dir.file("file").unwrap();
        std::fs::rename(&path, root.path().join("old")).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("file"), b"evil").unwrap();
        assert_eq!(dir.read("file", 100).unwrap(), b"verified");
        assert_eq!(
            sha256_fd(&mut fd).unwrap().0,
            hex::encode(Sha256::digest(b"verified"))
        );
        symlink(&path, root.path().join("link")).unwrap();
        assert!(ReleaseDir::open(&root.path().join("link")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn r1_bounded_metadata_rejects_sparse_symlink_fifo_and_directory() {
        use std::os::unix::fs::symlink;
        let d = tmpdir();
        let dir = ReleaseDir::open(d.path()).unwrap();
        let large = std::fs::File::create(d.path().join("large")).unwrap();
        large.set_len(MAX_MANIFEST as u64 + 1).unwrap();
        assert!(dir.read("large", MAX_MANIFEST as u64).is_err());
        symlink("large", d.path().join("link")).unwrap();
        assert!(dir.read("link", MAX_MANIFEST as u64).is_err());
        std::fs::create_dir(d.path().join("directory")).unwrap();
        assert!(dir.read("directory", 256).is_err());
        let fifo =
            std::ffi::CString::new(d.path().join("fifo").as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(dir.read("fifo", 256).is_err());
        assert!(dir.read("large", 256).is_err());
    }

    #[test]
    fn signed_release_verifies_and_files_match() {
        let d = tmpdir();
        let (bytes, sig, vk) = release(d.path(), 7);
        let m = verify_manifest(&bytes, &sig, &vk, 7).unwrap();
        assert_eq!(m.seq, 7);
        check_files(d.path(), &m).unwrap();
        assert_eq!(
            derive_signing_key(
                "один два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать",
                cheap()
            )
            .to_bytes(),
            key().to_bytes()
        );
        assert_ne!(
            derive_signing_key(
                "другая фраза совсем не такая как первая ни одним словом вообще",
                cheap()
            )
            .to_bytes(),
            key().to_bytes()
        );
    }

    #[test]
    fn any_change_to_manifest_signature_key_or_files_is_refused() {
        let d = tmpdir();
        let (bytes, sig, vk) = release(d.path(), 7);
        // изменённый байт манифеста
        let mut bad = bytes.clone();
        let i = bad.iter().position(|b| *b == b'7').unwrap();
        bad[i] = b'8';
        assert_eq!(
            verify_manifest(&bad, &sig, &vk, 0),
            Err(VerifyError::BadSignature)
        );
        // изменённая подпись и мусор вместо подписи
        let mut s = sig.clone().into_bytes();
        s[0] = if s[0] == b'0' { b'1' } else { b'0' };
        assert_eq!(
            verify_manifest(&bytes, std::str::from_utf8(&s).unwrap(), &vk, 0),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(
            verify_manifest(&bytes, "zz", &vk, 0),
            Err(VerifyError::BadSignature)
        );
        // чужой ключ
        let other = derive_signing_key(
            "совсем другая длинная фраза из многих слов для чужого ключа подписи выпусков",
            cheap(),
        );
        assert_eq!(
            verify_manifest(&bytes, &sig, &other.verifying_key(), 0),
            Err(VerifyError::BadSignature)
        );
        // подмена файла после подписи
        let m = verify_manifest(&bytes, &sig, &vk, 0).unwrap();
        std::fs::write(
            d.path().join("yandi-node-linux-x86_64.tar.gz"),
            b"evil bytes!",
        )
        .unwrap();
        assert!(matches!(
            check_files(d.path(), &m),
            Err(VerifyError::File(_))
        ));
        // отсутствующий файл
        std::fs::remove_file(d.path().join("README.txt")).unwrap();
        assert!(matches!(
            check_files(d.path(), &m),
            Err(VerifyError::File(_))
        ));
    }

    #[test]
    fn rollback_oversize_and_bad_names_are_refused() {
        let d = tmpdir();
        let (bytes, sig, vk) = release(d.path(), 3);
        assert_eq!(
            verify_manifest(&bytes, &sig, &vk, 4),
            Err(VerifyError::Rollback { seq: 3, min: 4 })
        );
        assert!(verify_manifest(&bytes, &sig, &vk, 3).is_ok());
        assert_eq!(
            verify_manifest(&vec![b' '; MAX_MANIFEST + 1], &sig, &vk, 0),
            Err(VerifyError::TooLarge)
        );
        for bad in ["../evil", "a/b", "", "..", "MANIFEST.json", "x\ny"] {
            assert!(
                build_manifest(d.path(), "1", 1, 0, "k", &[bad.into()]).is_err(),
                "{bad:?}"
            );
        }
        // манифест, подписанный верно, но с «..» в имени, при проверке тоже не принимается
        let k = key();
        let evil = Manifest {
            format: 1,
            product: "yandi-node".into(),
            version: "1".into(),
            seq: 1,
            created: 0,
            key_id: key_id(&k.verifying_key()),
            files: vec![FileEntry {
                name: "../etc/passwd".into(),
                sha256: "0".repeat(64),
                size: 1,
            }],
            platform: String::new(),
            expires: 0,
            notes: String::new(),
            support: None,
        };
        let eb = manifest_bytes(&evil);
        let es = hex::encode(k.sign(&[SIG_DOMAIN, &eb].concat()).to_bytes());
        assert!(matches!(
            verify_manifest(&eb, &es, &k.verifying_key(), 0),
            Err(VerifyError::Malformed(_))
        ));
    }

    #[test]
    fn support_wallet_is_signed_optional_and_validated() {
        let w = SupportWallet::parse("USDT|Tron (TRC20)|TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB").expect("valid");
        let mut m = parse_manifest(&{
            let d = tmpdir();
            release(d.path(), 1).0
        })
        .unwrap();
        assert!(m.support.is_none() && !String::from_utf8(manifest_bytes(&m)).unwrap().contains("support"), "без кошелька поле не пишется");
        m.support = Some(w.clone());
        let back = parse_manifest(&manifest_bytes(&m)).unwrap();
        assert_eq!(back.support, Some(w));
        m.support = Some(SupportWallet { address: "x y".into(), ..m.support.clone().unwrap() });
        assert!(validate_manifest(&m).is_err());
        assert!(SupportWallet::parse("USDT|Tron (TRC20)|TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsC").is_none(), "опечатка в последнем знаке: контрольная сумма не сходится");
        assert!(SupportWallet::parse("USDT|Tron (TRC20)|00000000000000000000000000").is_none());
        assert!(SupportWallet::parse("USDTUSDTUSDTUSDTU|Tron|TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB").is_none(), "монета длиннее 16 знаков");
        assert!(SupportWallet::parse("USDT|Tron").is_none() && SupportWallet::parse("a|b|c|d").is_none());
    }

    #[test]
    fn weak_phrases_are_refused_and_spaces_do_not_change_the_key() {
        assert!(weakness("слишком короткая фраза").is_some());
        assert!(weakness(&"слово ".repeat(14).trim().to_string()).is_some());
        assert_eq!(&*normalize("  а   б \t в "), "а б в");
        let a = derive_signing_key(
            "раз  два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать",
            cheap(),
        );
        let b = derive_signing_key(
            "раз два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать",
            cheap(),
        );
        assert_eq!(a.to_bytes(), b.to_bytes());
    }
}
