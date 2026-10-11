//! Долговечное хранение envelope с журналом незавершённой замены.
//! Каталог задаётся явно: здесь нет обращений к боевым данным или auto-unlock.
//! Anchor обнаруживает откат envelope, пока сам anchor не откатан/подменён.
//! Для защиты от отката всего диска нужен внешний доверенный счётчик.

use super::{NodeVault, VaultEnvelope};
use anyhow::{anyhow, bail, Result};
use hmac::Mac;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

const ENVELOPE: &str = "envelope.json";
const PENDING: &str = "envelope.pending";
const ANCHOR: &str = "anchor.json";
const LIMIT: usize = 16384;

pub mod gate;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    revision: u64,
    digest: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchor {
    format: u32,
    current: Option<Stamp>,
    pending: Option<Stamp>,
}

#[derive(Clone)]
pub struct Snapshot {
    pub envelope: VaultEnvelope,
    stamp: Stamp,
}

impl Snapshot {
    pub fn revision(&self) -> u64 {
        self.stamp.revision
    }
}

/// Сериализация между потоками и процессами через flock отдельного файла.
/// Все операции open/rename/unlink относительно удерживаемого dirfd, без перехода по ссылкам.
pub struct EnvelopeStore {
    dir: File,
}

impl EnvelopeStore {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() || path.file_name().is_none() {
            bail!("нужен отдельный абсолютный каталог vault");
        }
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {
                File::open(
                    path.parent()
                        .ok_or_else(|| anyhow!("каталог без родителя"))?,
                )?
                .sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let meta = dir.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.permissions().mode() & 0o077 != 0 {
            bail!("каталог vault должен принадлежать владельцу и иметь права 0700");
        }
        Ok(Self { dir })
    }

    fn file(&self, name: &str, flags: i32) -> std::io::Result<File> {
        let name = CString::new(name).map_err(|_| std::io::ErrorKind::InvalidInput)?;
        let fd = unsafe {
            libc::openat(
                self.dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata()?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "небезопасный файл vault",
            ));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }

    fn lock(&self) -> Result<File> {
        let lock = self.file(".lock", libc::O_RDWR | libc::O_CREAT)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(lock) // закрытие fd освобождает flock, в том числе при ошибке/панике
    }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let file = match self.file(name, libc::O_RDONLY) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if file.metadata()?.len() > LIMIT as u64 {
            bail!("слишком большой файл vault");
        }
        let mut bytes = Vec::new();
        file.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > LIMIT {
            bail!("слишком большой файл vault");
        }
        Ok(Some(bytes))
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let from = CString::new(from)?;
        let to = CString::new(to)?;
        if unsafe {
            libc::renameat(
                self.dir.as_raw_fd(),
                from.as_ptr(),
                self.dir.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        self.dir.sync_all()?;
        Ok(())
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let temporary = format!(".write-{}.tmp", hex::encode(super::random::<16>()));
        let mut file = self.file(&temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            self.rename(&temporary, name)
        })();
        // Удаляем только собственный зашифрованный/несекретный temp, не источник.
        if result.is_err() {
            let temporary = CString::new(temporary)?;
            unsafe {
                libc::unlinkat(self.dir.as_raw_fd(), temporary.as_ptr(), 0);
            }
        }
        result
    }

    fn snapshot(bytes: &[u8]) -> Result<Snapshot> {
        let envelope: VaultEnvelope = serde_json::from_slice(bytes)?;
        envelope.validate()?;
        if envelope.revision == 0 {
            bail!("envelope без сохранённой версии");
        }
        Ok(Snapshot {
            stamp: Stamp {
                revision: envelope.revision,
                digest: Sha256::digest(bytes).into(),
            },
            envelope,
        })
    }

    fn anchor(&self, anchor: &Anchor) -> Result<()> {
        self.write(ANCHOR, &serde_json::to_vec(anchor)?)
    }

    fn load_locked(&self) -> Result<Option<Snapshot>> {
        let bytes = self.read(ENVELOPE)?;
        let pending_bytes = self.read(PENDING)?;
        let Some(anchor_bytes) = self.read(ANCHOR)? else {
            if bytes.is_some() {
                bail!("отсутствует метка версии существующего vault");
            }
            // Первая публикация оборвалась после fsync кандидата, до записи намерения.
            // Только revision=1 и отсутствие опубликованного envelope; существующий
            // envelope без anchor никогда не считается первым запуском.
            if let Some(candidate) = pending_bytes {
                let staged = Self::snapshot(&candidate)?;
                if staged.revision() != 1 {
                    bail!("отсутствует метка существующего vault");
                }
                self.anchor(&Anchor {
                    format: 1,
                    current: None,
                    pending: Some(staged.stamp),
                })?;
                return self.load_locked();
            }
            return Ok(None);
        };
        let mut anchor: Anchor = serde_json::from_slice(&anchor_bytes)?;
        if anchor.format != 1 || (anchor.current.is_none() && anchor.pending.is_none()) {
            bail!("повреждённая метка vault");
        }
        if let Some(next) = anchor.pending.clone() {
            let expected = anchor
                .current
                .as_ref()
                .map_or(Some(1), |old| old.revision.checked_add(1));
            if Some(next.revision) != expected {
                bail!("повреждённая последовательность версий");
            }
            let published = bytes.as_ref().map(|b| Self::snapshot(b)).transpose()?;
            if published.as_ref().map(|s| &s.stamp) != Some(&next) {
                if published.as_ref().map(|s| &s.stamp) != anchor.current.as_ref() {
                    bail!("откат или подмена envelope");
                }
                let staged =
                    Self::snapshot(pending_bytes.as_deref().ok_or_else(|| {
                        anyhow!("утрачен кандидат обновления; источник сохранён")
                    })?)?;
                if staged.stamp != next {
                    bail!("повреждён кандидат обновления; источник сохранён");
                }
                self.rename(PENDING, ENVELOPE)?;
            }
            anchor.current = Some(next);
            anchor.pending = None;
            self.anchor(&anchor)?;
            return self.load_locked();
        }
        let published = Self::snapshot(
            bytes
                .as_deref()
                .ok_or_else(|| anyhow!("утрачен envelope"))?,
        )?;
        if Some(&published.stamp) != anchor.current.as_ref() {
            bail!("откат или подмена envelope");
        }
        Ok(Some(published))
    }

    pub fn load(&self) -> Result<Option<Snapshot>> {
        let _lock = self.lock()?;
        self.load_locked()
    }

    /// expected=None разрешён только для действительно нового хранилища.
    /// Версия и всё содержимое сравниваются, устаревший писатель ничего не меняет.
    pub fn save(
        &self,
        expected: Option<&Snapshot>,
        envelope: &VaultEnvelope,
        vault: &NodeVault,
    ) -> Result<Snapshot> {
        self.save_with_hook(expected, envelope, vault, &|_| Ok(()))
    }

    fn save_with_hook(
        &self,
        expected: Option<&Snapshot>,
        envelope: &VaultEnvelope,
        vault: &NodeVault,
        hook: &dyn Fn(u8) -> Result<()>,
    ) -> Result<Snapshot> {
        let _lock = self.lock()?;
        let current = self.load_locked()?;
        if current.as_ref().map(|s| &s.stamp) != expected.map(|s| &s.stamp) {
            bail!("версия vault изменилась; повторите чтение");
        }
        let session = vault
            .session
            .as_ref()
            .ok_or_else(|| anyhow!("закрытая среда заперта"))?;
        envelope.validate()?;
        envelope.verify(&session.key)?;
        if envelope.id != session.id
            || current
                .as_ref()
                .is_some_and(|s| s.envelope.id != envelope.id)
        {
            bail!("ключ другого vault");
        }
        let mut envelope = envelope.clone();
        envelope.revision = current
            .as_ref()
            .map_or(Some(1), |s| s.stamp.revision.checked_add(1))
            .ok_or_else(|| anyhow!("переполнение версии"))?;
        envelope.tag = envelope.mac(&session.key).finalize().into_bytes().into();
        let bytes = serde_json::to_vec(&envelope)?;
        let next = Self::snapshot(&bytes)?;
        // Сначала полностью сохранённый кандидат, затем намерение, затем публикация.
        self.write(PENDING, &bytes)?;
        hook(1)?;
        self.anchor(&Anchor {
            format: 1,
            current: current.map(|s| s.stamp),
            pending: Some(next.stamp.clone()),
        })?;
        hook(2)?;
        self.rename(PENDING, ENVELOPE)?;
        hook(3)?;
        self.anchor(&Anchor {
            format: 1,
            current: Some(next.stamp.clone()),
            pending: None,
        })?;
        hook(4)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::Mac;
    use zeroize::Zeroizing;

    // Store-тестам не нужен KDF: MAC/ревизии проверяются настоящим DK,
    // криптографические roundtrip проверяются отдельным тестом ниже.
    pub(super) fn fixture() -> (VaultEnvelope, NodeVault) {
        let key = Zeroizing::new([7; 32]);
        let mut envelope = VaultEnvelope {
            version: 1,
            id: [3; 16],
            revision: 0,
            password: super::super::KeyWrap {
                salt: [1; 32],
                nonce: [2; 12],
                ciphertext: vec![4; 48],
            },
            recovery: None,
            tag: [0; 32],
        };
        envelope.tag = envelope.mac(&key).finalize().into_bytes().into();
        (
            envelope,
            NodeVault {
                session: Some(super::super::Session { id: [3; 16], key }),
            },
        )
    }
    pub(super) fn temp() -> tempfile::TempDir {
        let directory = tempfile::Builder::new()
            .prefix("r15_store_")
            .tempdir()
            .unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    #[test]
    fn bug_r15_envelope_is_private_and_old_revision_rejected() {
        let d = temp();
        let store = EnvelopeStore::open(d.path()).unwrap();
        let (env, vault) = fixture();
        let old = store.save(None, &env, &vault).unwrap();
        let old_bytes = std::fs::read(d.path().join(ENVELOPE)).unwrap();
        let next = store.save(Some(&old), &env, &vault).unwrap();
        assert_eq!(next.revision(), 2);
        assert!(store.save(Some(&old), &env, &vault).is_err());
        assert_eq!(
            std::fs::metadata(d.path().join(ENVELOPE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::write(d.path().join(ENVELOPE), &old_bytes).unwrap();
        assert!(store.load().is_err());
        assert_eq!(std::fs::read(d.path().join(ENVELOPE)).unwrap(), old_bytes);
    }

    #[test]
    fn bug_r15_restarts_finish_interrupted_publication_and_keep_only_copy() {
        for phase in 1..=4 {
            let d = temp();
            let store = EnvelopeStore::open(d.path()).unwrap();
            let (env, vault) = fixture();
            assert!(store
                .save_with_hook(None, &env, &vault, &|p| if p == phase {
                    bail!("обрыв")
                } else {
                    Ok(())
                })
                .is_err());
            let restored = EnvelopeStore::open(d.path())
                .unwrap()
                .load()
                .unwrap()
                .unwrap();
            assert_eq!(restored.revision(), 1);
        }
        for phase in 1..=4 {
            let d = temp();
            let store = EnvelopeStore::open(d.path()).unwrap();
            let (env, vault) = fixture();
            let old = store.save(None, &env, &vault).unwrap();
            let interrupted = store.save_with_hook(Some(&old), &env, &vault, &|p| {
                if p == phase {
                    bail!("обрыв")
                } else {
                    Ok(())
                }
            });
            assert!(interrupted.is_err());
            drop(store);
            let restored = EnvelopeStore::open(d.path())
                .unwrap()
                .load()
                .unwrap()
                .unwrap();
            assert_eq!(restored.revision(), if phase == 1 { 1 } else { 2 });
            restored
                .envelope
                .verify(&vault.session.as_ref().unwrap().key)
                .unwrap();
        }
        let d = temp();
        let store = EnvelopeStore::open(d.path()).unwrap();
        let (env, vault) = fixture();
        let old = store.save(None, &env, &vault).unwrap();
        let bytes = std::fs::read(d.path().join(ENVELOPE)).unwrap();
        assert!(store
            .save_with_hook(Some(&old), &env, &vault, &|p| if p == 2 {
                bail!("обрыв")
            } else {
                Ok(())
            })
            .is_err());
        std::fs::write(d.path().join(PENDING), b"broken").unwrap();
        assert!(store.load().is_err());
        assert_eq!(std::fs::read(d.path().join(ENVELOPE)).unwrap(), bytes);
    }

    #[test]
    fn bug_r15_missing_anchor_and_links_fail_closed() {
        let d = temp();
        let store = EnvelopeStore::open(d.path()).unwrap();
        let (env, vault) = fixture();
        store.save(None, &env, &vault).unwrap();
        std::fs::remove_file(d.path().join(ANCHOR)).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(None, &env, &vault).is_err());
        let outside = temp();
        let victim = outside.path().join("precious");
        std::fs::write(&victim, b"keep").unwrap();
        std::fs::remove_file(d.path().join(ENVELOPE)).unwrap();
        std::os::unix::fs::symlink(&victim, d.path().join(ENVELOPE)).unwrap();
        assert!(store.load().is_err());
        assert_eq!(std::fs::read(victim).unwrap(), b"keep");
    }

    #[test]
    fn bug_r15_two_writers_cannot_overwrite_a_newer_envelope() {
        let d = temp();
        let store = EnvelopeStore::open(d.path()).unwrap();
        let (env, vault) = fixture();
        let snapshot = store.save(None, &env, &vault).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut writers = Vec::new();
        for _ in 0..2 {
            let path = d.path().to_path_buf();
            let old = snapshot.clone();
            let barrier = barrier.clone();
            writers.push(std::thread::spawn(move || {
                let store = EnvelopeStore::open(&path).unwrap();
                let (env, vault) = fixture();
                barrier.wait();
                store.save(Some(&old), &env, &vault).is_ok()
            }));
        }
        let succeeded = writers
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>();
        assert_eq!(succeeded, 1);
        assert_eq!(store.load().unwrap().unwrap().revision(), 2);
    }

    #[test]
    fn bug_r15_real_envelope_survives_store_and_revision_is_authenticated() {
        let d = temp();
        let store = EnvelopeStore::open(d.path()).unwrap();
        let env = VaultEnvelope::create("пароль владельца", None).unwrap();
        let mut vault = NodeVault::default();
        vault.unlock(&env, "пароль владельца").unwrap();
        let encrypted = vault.seal("test", b"private").unwrap();
        store.save(None, &env, &vault).unwrap();
        vault.lock();
        let loaded = store.load().unwrap().unwrap();
        vault.unlock(&loaded.envelope, "пароль владельца").unwrap();
        assert_eq!(&**vault.open("test", &encrypted).unwrap(), b"private");
        let mut altered = loaded.envelope;
        altered.revision += 1;
        assert!(vault.unlock(&altered, "пароль владельца").is_err());
        assert!(!vault.unlocked());
    }
}
