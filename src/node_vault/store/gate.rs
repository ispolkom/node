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
    /// неверные пароли подряд; после [`MAX_PASSWORD_FAILURES`] пароль больше не принимается, нужна сид-фраза
    failures: u32,
    next_ms: u64,
    /// неверные сид-фразы подряд; после [`MAX_PHRASE_FAILURES`] закрытая среда стирается
    #[serde(default)]
    rec_failures: u32,
}

pub const MAX_PASSWORD_FAILURES: u32 = 5;
pub const MAX_PHRASE_FAILURES: u32 = 5;

impl Attempts {
    fn total(&self) -> u32 {
        self.failures.saturating_add(self.rec_failures)
    }
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
                rec_failures: 0,
            })?,
        )
    }

    /// Криптостирание: убирает обёрнутый ключ данных и метки. Зашифрованные файлы потребителей остаются на диске, но без
    /// ключа не читаются. Вызывается под flock, после этого среда считается не созданной.
    fn wipe_locked(&self) {
        for name in [super::ENVELOPE, super::PENDING, super::ANCHOR, COUNTER] {
            if let Ok(c) = std::ffi::CString::new(name) {
                unsafe {
                    libc::unlinkat(self.dir.as_raw_fd(), c.as_ptr(), 0);
                }
            }
        }
        let _ = self.dir.sync_all();
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
            || (attempts.total() <= 3 && attempts.next_ms != 0)
            || (attempts.total() > 3 && attempts.next_ms == 0)
        {
            bail!("повреждён счётчик попыток; вход заблокирован до KDF");
        }
        let now = now_ms()?;
        if now < attempts.next_ms {
            bail!("пауза перед следующей попыткой входа");
        }
        if !recovery && attempts.failures >= MAX_PASSWORD_FAILURES {
            bail!("пять неверных паролей: вход только по сид-фразе");
        }
        // Резервируем попытку до Argon2: crash/cancel не возвращают бесплатную попытку.
        let counter = if recovery {
            &mut attempts.rec_failures
        } else {
            &mut attempts.failures
        };
        *counter = counter
            .checked_add(1)
            .ok_or_else(|| anyhow!("переполнение счётчика"))?;
        attempts.next_ms = if attempts.total() <= 3 {
            0
        } else {
            now.saturating_add(delay_ms(attempts.total()))
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
            attempts.rec_failures = 0;
            attempts.next_ms = 0;
        } else {
            if recovery && attempts.rec_failures >= MAX_PHRASE_FAILURES {
                self.wipe_locked();
                bail!("пять неверных сид-фраз: закрытая среда стёрта");
            }
            if attempts.total() > 3 {
                attempts.next_ms = now_ms()?.saturating_add(delay_ms(attempts.total()));
            }
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
        #[cfg(test)]
        let tag = crate::node_vault::current_tag();
        let permit = queue().clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            #[cfg(test)]
            crate::node_vault::set_worker_tag(tag);
            let r = self.unlock_counted(&secret, recovery);
            #[cfg(test)]
            crate::node_vault::set_worker_tag(None);
            r
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
    use crate::node_vault::{tagged_kdf_calls, KDF_TAG};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TAG: AtomicU64 = AtomicU64::new(1);
    fn kdf_calls() -> usize {
        tagged_kdf_calls(crate::node_vault::current_tag().expect("тест внутри KDF_TAG.scope"))
    }

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
        KDF_TAG.scope(NEXT_TAG.fetch_add(1, Ordering::SeqCst), async {
        let (d, store) = installed();
        std::fs::remove_file(d.path().join(COUNTER)).unwrap();
        let calls = kdf_calls();
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
        assert_eq!(kdf_calls(), calls);
        assert!(store.prepare_attempts(&fixture().0).is_err());
    }).await
    }

    #[tokio::test]
    async fn bug_r15_pause_and_corrupt_counter_block_before_any_kdf() {
        KDF_TAG.scope(NEXT_TAG.fetch_add(1, Ordering::SeqCst), async {
        let (d, store) = installed();
        let attempts = Attempts {
            format: 1,
            id: [3; 16],
            failures: 4,
            next_ms: now_ms().unwrap() + 300_000,
            rec_failures: 0,
        };
        store
            .write(COUNTER, &serde_json::to_vec(&attempts).unwrap())
            .unwrap();
        let calls = kdf_calls();
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
        assert_eq!(kdf_calls(), calls);
    }).await
    }

    #[tokio::test]
    async fn bug_r15_failure_pause_survives_reopen_and_blocks_before_kdf() {
        KDF_TAG.scope(NEXT_TAG.fetch_add(1, Ordering::SeqCst), async {
        let (d, store) = installed();
        let previous = Attempts {
            format: 1,
            id: [3; 16],
            failures: 3,
            // уже были неверные сид-фразы: пауза после этой попытки 16 с, а не 2 — очередь KDF общая, и в загруженном прогоне
            // второй вход ждёт своего permit дольше короткой паузы
            next_ms: 1,
            rec_failures: 3,
        };
        store
            .write(COUNTER, &serde_json::to_vec(&previous).unwrap())
            .unwrap();
        let before = kdf_calls();
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
        assert_eq!(kdf_calls(), before + 1);
    }).await
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
        KDF_TAG.scope(NEXT_TAG.fetch_add(1, Ordering::SeqCst), async {
        let (_one, first) = installed();
        let (_two, second) = installed();
        let before = kdf_calls();
        let tag = crate::node_vault::current_tag().unwrap();
        let running = tokio::spawn(KDF_TAG.scope(tag, first.unlock_async("wrong password".into(), false)));
        while kdf_calls() == before {
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
        assert_eq!(kdf_calls(), before + 1);
    }).await
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

    #[tokio::test]
    async fn five_wrong_passwords_close_the_password_door_and_the_phrase_still_opens() {
        let d = temp();
        let store = Arc::new(EnvelopeStore::open(d.path()).unwrap());
        let phrase = "alpha bravo charlie delta";
        let env = EnvelopeStore::create_envelope_async("correct password".into(), Some(phrase.into()))
            .await
            .unwrap();
        let mut vault = NodeVault::default();
        vault.unlock(&env, "correct password").unwrap();
        store.prepare_attempts(&env).unwrap();
        store.save(None, &env, &vault).unwrap();
        for _ in 0..MAX_PASSWORD_FAILURES {
            // пауза после четвёртой попытки не мешает счёту: снимаем её вручную
            let mut a: Attempts = serde_json::from_slice(&store.read(COUNTER).unwrap().unwrap()).unwrap();
            a.next_ms = if a.total() > 3 { 1 } else { 0 };
            store.write(COUNTER, &serde_json::to_vec(&a).unwrap()).unwrap();
            assert!(store.clone().unlock_async("wrong password".into(), false).await.is_err());
        }
        let mut a: Attempts = serde_json::from_slice(&store.read(COUNTER).unwrap().unwrap()).unwrap();
        a.next_ms = 1;
        store.write(COUNTER, &serde_json::to_vec(&a).unwrap()).unwrap();
        // правильный пароль уже не принимается: нужна сид-фраза
        let refused = match store.clone().unlock_async("correct password".into(), false).await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("пароль после пяти неверных не должен приниматься"),
        };
        assert!(refused.contains("сид-фраз"), "{refused}");
        // а сид-фраза открывает и сбрасывает оба счётчика
        let opened = store.clone().unlock_async(phrase.into(), true).await.unwrap();
        assert!(opened.unlocked());
        let a: Attempts = serde_json::from_slice(&store.read(COUNTER).unwrap().unwrap()).unwrap();
        assert_eq!((a.failures, a.rec_failures), (0, 0));
    }

    #[tokio::test]
    async fn five_wrong_phrases_erase_the_closed_environment() {
        let d = temp();
        let store = Arc::new(EnvelopeStore::open(d.path()).unwrap());
        let phrase = "alpha bravo charlie delta";
        let env = EnvelopeStore::create_envelope_async("correct password".into(), Some(phrase.into()))
            .await
            .unwrap();
        let mut vault = NodeVault::default();
        vault.unlock(&env, "correct password").unwrap();
        store.prepare_attempts(&env).unwrap();
        store.save(None, &env, &vault).unwrap();
        for i in 0..MAX_PHRASE_FAILURES {
            let mut a: Attempts = serde_json::from_slice(&store.read(COUNTER).unwrap().unwrap()).unwrap();
            a.next_ms = if a.total() > 3 { 1 } else { 0 };
            store.write(COUNTER, &serde_json::to_vec(&a).unwrap()).unwrap();
            let err = match store.clone().unlock_async("one two three four".into(), true).await {
                Err(e) => e.to_string(),
                Ok(_) => panic!("чужая фраза не должна открывать"),
            };
            assert_eq!(err.contains("стёрта"), i + 1 == MAX_PHRASE_FAILURES, "{err}");
        }
        // среда стёрта: ни envelope, ни метки, ни счётчика; правильная фраза ничего не открывает
        assert!(store.load().unwrap().is_none());
        assert!(store.read(COUNTER).unwrap().is_none());
        assert!(store.clone().unlock_async(phrase.into(), true).await.is_err());
        assert!(!std::fs::read_dir(d.path()).unwrap().any(|e| {
            let n = e.unwrap().file_name().to_string_lossy().to_string();
            n == "envelope.json" || n == "anchor.json"
        }));
    }
}
