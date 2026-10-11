//! Вход в закрытый vault: долговечная попытка до KDF, общая очередь и blocking worker.
//! Никаких автогенерируемых счётчиков для существующего envelope.

use super::EnvelopeStore;
use crate::node_vault::{NodeVault, VaultEnvelope};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

const COUNTER: &str = "attempts.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempts {
    format: u32,
    id: [u8; 16],
    failures: u32,
    next_ms: u64,
}

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}
fn delay_ms(failures: u32) -> u64 {
    if failures <= 3 {
        0
    } else {
        (2000u64 << (failures - 4).min(8)).min(300_000)
    }
}
fn queue() -> &'static Arc<Semaphore> {
    static QUEUE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    QUEUE.get_or_init(|| Arc::new(Semaphore::new(1)))
}

impl EnvelopeStore {
    /// Только первичная настройка ДО публикации нового envelope. Существующий
    /// vault или счётчик не сбрасываются, в том числе после неудачного входа.
    pub fn prepare_attempts(&self, envelope: &VaultEnvelope) -> Result<()> {
        let _lock = self.lock()?;
        if self.load_locked()?.is_some() || self.read(COUNTER)?.is_some() {
            bail!("нельзя сбрасывать счётчики существующей закрытой среды");
        }
        envelope.validate()?;
        self.write(
            COUNTER,
            &serde_json::to_vec(&Attempts {
                format: 1,
                id: envelope.id,
                failures: 0,
                next_ms: 0,
            })?,
        )
    }

    /// Откат первичной настройки, оборвавшейся до публикации envelope: счётчик, заведённый `prepare_attempts`, убирается,
    /// чтобы настройку можно было повторить. Существующий vault не затрагивается.
    pub fn abort_setup(&self) -> Result<()> {
        let _lock = self.lock()?;
        if self.load_locked()?.is_some() {
            bail!("vault уже создан");
        }
        let name = std::ffi::CString::new(COUNTER)?;
        unsafe {
            libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), 0);
        }
        Ok(())
    }

    fn unlock_counted(&self, secret: &str, recovery: bool) -> Result<NodeVault> {
        // flock держится и во время KDF: разные процессы не обходят очередь одного vault.
        let _lock = self.lock()?;
        let snapshot = self
            .load_locked()?
            .ok_or_else(|| anyhow!("закрытая среда не создана"))?;
        let bytes = self
            .read(COUNTER)?
            .ok_or_else(|| anyhow!("нет счётчика попыток; вход заблокирован до KDF"))?;
        let mut attempts: Attempts = serde_json::from_slice(&bytes)?;
        if attempts.format != 1
            || attempts.id != snapshot.envelope.id
            || (attempts.failures <= 3 && attempts.next_ms != 0)
            || (attempts.failures > 3 && attempts.next_ms == 0)
        {
            bail!("повреждён счётчик попыток; вход заблокирован до KDF");
        }
        let now = now_ms()?;
        if now < attempts.next_ms {
            bail!("пауза перед следующей попыткой входа");
        }
        // Резервируем попытку до Argon2: crash/cancel не возвращают бесплатную попытку.
        attempts.failures = attempts
            .failures
            .checked_add(1)
            .ok_or_else(|| anyhow!("переполнение счётчика"))?;
        attempts.next_ms = if attempts.failures <= 3 {
            0
        } else {
            now.saturating_add(delay_ms(attempts.failures))
        };
        self.write(COUNTER, &serde_json::to_vec(&attempts)?)?;
        let mut vault = NodeVault::default();
        let result = if recovery {
            vault.recover(&snapshot.envelope, secret)
        } else {
            vault.unlock(&snapshot.envelope, secret)
        };
        if result.is_ok() {
            attempts.failures = 0;
            attempts.next_ms = 0;
        } else if attempts.failures > 3 {
            attempts.next_ms = now_ms()?.saturating_add(delay_ms(attempts.failures));
        }
        // При ошибке записи успешный ключ наружу не отдаётся, Drop стирает DK.
        self.write(COUNTER, &serde_json::to_vec(&attempts)?)?;
        result?;
        Ok(vault)
    }

    /// Permit переносится ВНУТРЬ worker: отмена async caller не запускает второй
    /// Argon2 параллельно ещё работающему первому. Секретная строка очищается при Drop.
    pub async fn unlock_async(
        self: Arc<Self>,
        secret: String,
        recovery: bool,
    ) -> Result<NodeVault> {
        let secret = Zeroizing::new(secret);
        let permit = queue().clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            self.unlock_counted(&secret, recovery)
        })
        .await?
    }

    /// Первичная настройка: envelope и открытый сеанс без повторного KDF.
    pub async fn create_unlocked_async(
        password: String,
        phrase: Option<String>,
    ) -> Result<(VaultEnvelope, NodeVault)> {
        let password = Zeroizing::new(password);
        let phrase = phrase.map(Zeroizing::new);
        let permit = queue().clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            VaultEnvelope::create_unlocked(&password, phrase.as_ref().map(|s| s.as_str()))
        })
        .await?
    }

    /// Смена пароля: новый замок для того же DK. Тяжёлый KDF — в общей очереди и в blocking worker.
    pub async fn rewrap_async(
        envelope: VaultEnvelope,
        vault: NodeVault,
        password: String,
    ) -> Result<(VaultEnvelope, NodeVault)> {
        let password = Zeroizing::new(password);
        let permit = queue().clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let next = vault.change_password(&envelope, &password)?;
            Ok((next, vault))
        })
        .await?
    }

    /// Растяжение замков первичной настройки тоже не выполняется на async runtime.
    pub async fn create_envelope_async(
        password: String,
        phrase: Option<String>,
    ) -> Result<VaultEnvelope> {
        let password = Zeroizing::new(password);
        let phrase = phrase.map(Zeroizing::new);
        let permit = queue().clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            VaultEnvelope::create(&password, phrase.as_ref().map(|s| s.as_str()))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, temp};
    use super::*;
    use crate::node_vault::KDF_CALLS;
    use std::sync::atomic::Ordering;

    fn installed() -> (tempfile::TempDir, Arc<EnvelopeStore>) {
        let d = temp();
        let store = Arc::new(EnvelopeStore::open(d.path()).unwrap());
        let (env, vault) = fixture();
        store.prepare_attempts(&env).unwrap();
        store.save(None, &env, &vault).unwrap();
        (d, store)
    }

    #[tokio::test]
    async fn bug_r15_missing_counter_blocks_before_any_kdf() {
        let (d, store) = installed();
        std::fs::remove_file(d.path().join(COUNTER)).unwrap();
        let calls = KDF_CALLS.load(Ordering::SeqCst);
        assert!(store
            .clone()
            .unlock_async("wrong password".into(), false)
            .await
            .is_err());
        assert!(store
            .clone()
            .unlock_async("first second third fourth".into(), true)
            .await
            .is_err());
        assert_eq!(KDF_CALLS.load(Ordering::SeqCst), calls);
        assert!(store.prepare_attempts(&fixture().0).is_err());
    }

    #[tokio::test]
    async fn bug_r15_pause_and_corrupt_counter_block_before_any_kdf() {
        let (d, store) = installed();
        let attempts = Attempts {
            format: 1,
            id: [3; 16],
            failures: 4,
            next_ms: now_ms().unwrap() + 300_000,
        };
        store
            .write(COUNTER, &serde_json::to_vec(&attempts).unwrap())
            .unwrap();
        let calls = KDF_CALLS.load(Ordering::SeqCst);
        assert!(store
            .clone()
            .unlock_async("wrong password".into(), false)
            .await
            .is_err());
        std::fs::write(d.path().join(COUNTER), br#"{"format":1,"failures":-1}"#).unwrap();
        assert!(store
            .unlock_async("wrong password".into(), false)
            .await
            .is_err());
        assert_eq!(KDF_CALLS.load(Ordering::SeqCst), calls);
    }

    #[tokio::test]
    async fn bug_r15_failure_pause_survives_reopen_and_blocks_before_kdf() {
        let (d, store) = installed();
        let previous = Attempts {
            format: 1,
            id: [3; 16],
            failures: 3,
            next_ms: 0,
        };
        store
            .write(COUNTER, &serde_json::to_vec(&previous).unwrap())
            .unwrap();
        let before = KDF_CALLS.load(Ordering::SeqCst);
        assert!(store
            .unlock_async("wrong password".into(), false)
            .await
            .is_err());
        let reopened = Arc::new(EnvelopeStore::open(d.path()).unwrap());
        let saved: Attempts =
            serde_json::from_slice(&reopened.read(COUNTER).unwrap().unwrap()).unwrap();
        assert_eq!(saved.failures, 4);
        assert!(saved.next_ms > now_ms().unwrap());
        assert!(reopened
            .unlock_async("wrong password".into(), false)
            .await
            .is_err());
        assert_eq!(KDF_CALLS.load(Ordering::SeqCst), before + 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bug_r15_kdf_does_not_block_runtime_and_attempt_is_durable() {
        let (_d, store) = installed();
        let work = store.clone().unlock_async("wrong password".into(), false);
        let tick = async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert_eq!(queue().available_permits(), 0);
        };
        let (result, ()) = tokio::join!(work, tick);
        assert!(result.is_err());
        let bytes = store.read(COUNTER).unwrap().unwrap();
        let counter: Attempts = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(counter.failures, 1);
        assert_eq!(counter.next_ms, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bug_r15_cancellation_does_not_release_running_worker_permit() {
        let (_one, first) = installed();
        let (_two, second) = installed();
        let before = KDF_CALLS.load(Ordering::SeqCst);
        let running = tokio::spawn(first.unlock_async("wrong password".into(), false));
        while KDF_CALLS.load(Ordering::SeqCst) == before {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        running.abort();
        // Другое хранилище и короткий секрет: ни flock, ни KDF_MUTEX сами по себе
        // не могут задержать эту попытку. Её задерживает именно живой permit worker.
        let next = second.unlock_async("short".into(), false);
        tokio::pin!(next);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut next)
                .await
                .is_err()
        );
        assert!(next.await.is_err());
        assert_eq!(KDF_CALLS.load(Ordering::SeqCst), before + 1);
    }

    #[test]
    fn bug_r15_even_internal_kdfs_are_globally_serialized() {
        use crate::node_vault::{wrapping_key, KDF_PEAK};
        KDF_PEAK.store(0, Ordering::SeqCst);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let threads = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    wrapping_key(b"password", &[1; 32]).unwrap();
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(KDF_PEAK.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn bug_r15_success_resets_counter_and_store_reopen_preserves_it() {
        let d = temp();
        let store = Arc::new(EnvelopeStore::open(d.path()).unwrap());
        let env = EnvelopeStore::create_envelope_async("correct password".into(), None)
            .await
            .unwrap();
        let mut vault = NodeVault::default();
        vault.unlock(&env, "correct password").unwrap();
        store.prepare_attempts(&env).unwrap();
        store.save(None, &env, &vault).unwrap();
        vault.lock();
        let reopened = Arc::new(EnvelopeStore::open(d.path()).unwrap());
        let opened = reopened
            .clone()
            .unlock_async("correct password".into(), false)
            .await
            .unwrap();
        assert!(opened.unlocked());
        let counter: Attempts =
            serde_json::from_slice(&reopened.read(COUNTER).unwrap().unwrap()).unwrap();
        assert_eq!(counter.failures, 0);
        assert_eq!(counter.next_ms, 0);
    }
}
