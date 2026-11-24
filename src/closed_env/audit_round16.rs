//! Независимые атакующие проверки. Запуск: cargo test --offline bug_r16 -- --ignored --test-threads=1.
//! Проверяется безопасное поведение; игнорирование в штатном наборе НЕ означает исправление находки.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

struct Probe {
    key: Mutex<Option<[u8; 32]>>,
    fail_migration: AtomicBool,
    stop_lock: AtomicBool,
    entered: Notify,
    resume: Notify,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            key: Mutex::new(None),
            fail_migration: AtomicBool::new(false),
            stop_lock: AtomicBool::new(false),
            entered: Notify::new(),
            resume: Notify::new(),
        })
    }
}

impl Consumer for Probe {
    fn domain(&self) -> &'static str { DOMAIN_CHAT }
    fn migrate(&self, _: Option<[u8; 32]>, _: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async { Ok(usize::from(self.fail_migration.load(Ordering::SeqCst))) })
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move { *self.key.lock().unwrap() = Some(key); })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.stop_lock.load(Ordering::SeqCst) {
                self.entered.notify_one();
                self.resume.notified().await;
            }
            *self.key.lock().unwrap() = None;
        })
    }
}

#[tokio::test]
#[ignore = "R16-1: lock во время KDF должен отменять позднюю выдачу ключей"]
async fn bug_r16_lock_during_kdf_must_win() {
    let base = tempfile::Builder::new().prefix("r16_late_kdf_").tempdir().unwrap();
    let dir = base.path().join("closed");
    let env = ClosedEnv::open(&dir, Box::new(|| None)).unwrap();
    let probe = Probe::new();
    env.register(probe.clone());
    env.setup("owner password".into(), None).await.unwrap();
    env.lock().await;
    let task_env = env.clone();
    let task = tokio::spawn(async move { task_env.unlock("owner password".into()).await });
    // Счётчик зарезервирован ДО KDF и сбрасывается после него: ловим именно работающий KDF,
    // а не задержку после install. На однопоточном runtime install не может обогнать lock.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let bytes = std::fs::read(dir.join("attempts.json")).unwrap();
            let state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if state["failures"].as_u64() == Some(1) { break; }
            assert!(!task.is_finished(), "не удалось застать KDF: это ошибка стенда, не доказательство");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }).await.unwrap();
    env.lock().await;
    assert!(!env.unlocked() && probe.key.lock().unwrap().is_none());
    task.await.unwrap().unwrap();
    assert!(!env.unlocked(), "завершившийся lock не должен отменяться более старым unlock");
    assert!(probe.key.lock().unwrap().is_none(), "старый KDF не должен вернуть ключ потребителю");
}

#[tokio::test]
#[ignore = "R16-2: отмена lock не должна оставлять ключи при status.unlocked=false"]
async fn bug_r16_cancelled_lock_must_revoke_consumers() {
    let base = tempfile::Builder::new().prefix("r16_cancel_lock_").tempdir().unwrap();
    let env = ClosedEnv::open(&base.path().join("closed"), Box::new(|| None)).unwrap();
    let probe = Probe::new();
    env.register(probe.clone());
    env.setup("owner password".into(), None).await.unwrap();
    probe.stop_lock.store(true, Ordering::SeqCst);
    let task_env = env.clone();
    let task = tokio::spawn(async move { task_env.lock().await; });
    tokio::time::timeout(Duration::from_secs(5), probe.entered.notified()).await.unwrap();
    assert!(!env.unlocked(), "DK уже отозван");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(probe.key.lock().unwrap().is_none(), "потребитель остался с ключом после отменённого lock");
}

#[tokio::test]
#[ignore = "R16-3: незавершённая миграция не должна открывать потребителей для записи"]
async fn bug_r16_failed_migration_must_not_publish_open_session() {
    let base = tempfile::Builder::new().prefix("r16_migration_fail_").tempdir().unwrap();
    let env = ClosedEnv::open(&base.path().join("closed"), Box::new(|| None)).unwrap();
    let probe = Probe::new();
    probe.fail_migration.store(true, Ordering::SeqCst);
    env.register(probe.clone());
    let result = env.setup("owner password".into(), None).await;
    assert!(env.status().migration_pending, "неудача миграции действительно воспроизведена");
    assert!(!env.unlocked(), "неперенесённые файлы нельзя отдавать новому ключу для перезаписи");
    assert!(probe.key.lock().unwrap().is_none());
    assert!(result.is_err(), "setup не должен сообщать об успехе неполного переноса");
}

#[tokio::test]
#[ignore = "R16-4: crash после prepare_attempts до save не должен навсегда ломать первичную настройку"]
async fn bug_r16_setup_retries_after_counter_only_crash() {
    let base = tempfile::Builder::new().prefix("r16_setup_crash_").tempdir().unwrap();
    let dir = base.path().join("closed");
    let (envelope, _) = crate::node_vault::VaultEnvelope::create_unlocked("owner password", None).unwrap();
    {
        let store = EnvelopeStore::open(&dir).unwrap();
        store.prepare_attempts(&envelope).unwrap();
        // Точный образ диска после обрыва: опубликованного envelope ещё нет, счётчик уже есть.
    }
    let env = ClosedEnv::open(&dir, Box::new(|| None)).unwrap();
    assert!(!env.configured());
    env.setup("owner password".into(), None).await.expect("повторная настройка после обрыва должна восстановиться");
    assert!(env.configured() && env.unlocked());
}
