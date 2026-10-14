//! Закрытая среда узла (роадмап 3.1): сеанс владельца поверх `node_vault`.
//!
//! Рабочий уровень (идентичность, TLS, хэши токенов, очереди почты) живёт на ключе, привязанном к машине, и стартует сам.
//! Закрытый уровень (история чата ПК, группы ПК, ключи «ПК как собеседник») шифруется ключами, выведенными из ключа данных
//! под паролем владельца: после перезапуска узел заперт и прочесть это нельзя, в том числе на другом ПК.
//!
//! Потребители ([`Consumer`]) получают свой отдельный ключ при разблокировке и обязаны стереть его и расшифрованное состояние
//! при блокировке. Любое открытие/блокировка нумеруется поколением: поздняя разблокировка, пережившая блокировку, ничего не выдаёт.
//!
//! Честная граница: активный root на работающем узле достанет ключи из памяти; копии, зашифрованные прежним рабочим ключом,
//! задним числом не защищены (их остатки на диске могут пережить перезапись, особенно на SSD).

use crate::node_vault::store::{EnvelopeStore, Snapshot};
use crate::node_vault::NodeVault;
use anyhow::{anyhow, bail, Result};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub const DOMAIN_CHAT: &str = "chat";
pub const DOMAIN_GROUPS: &str = "groups_pc";
pub const DOMAIN_SELF: &str = "mobile_self";
pub const DOMAIN_PROXY: &str = "upstream_proxy";

/// Автоблокировка по умолчанию при создании среды.
pub const DEFAULT_AUTO_LOCK_SECS: u64 = 15 * 60;
const MARKER: &str = "migration.pending";
const SETTINGS: &str = "settings.json";

/// Часть узла, чьи данные закрыты. Все методы идемпотентны.
pub trait Consumer: Send + Sync {
    fn domain(&self) -> &'static str;
    /// Перенос данных с прежнего рабочего ключа (`old`, если он ещё доступен) на новый. Число файлов, которые не удалось перенести.
    fn migrate(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>>;
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()>;
    fn lock(&self) -> BoxFuture<'_, ()>;
}

#[derive(Clone, Serialize)]
pub struct Status {
    pub configured: bool,
    pub unlocked: bool,
    /// часть данных ещё не перенесена с прежнего ключа (перенос повторится при входе)
    pub migration_pending: bool,
    pub auto_lock_secs: u64,
    pub generation: u64,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    #[serde(default)]
    auto_lock_secs: u64,
}

struct Inner {
    vault: Option<NodeVault>,
    generation: u64,
    last_activity: Instant,
    auto_lock: Duration,
}

pub struct ClosedEnv {
    dir: PathBuf,
    store: Arc<EnvelopeStore>,
    inner: Mutex<Inner>,
    /// настройка, смена пароля, вход: по одной операции
    op: tokio::sync::Mutex<()>,
    /// применение ключей потребителям и блокировка не пересекаются
    apply: tokio::sync::Mutex<()>,
    consumers: Mutex<Vec<Arc<dyn Consumer>>>,
    legacy_master: Box<dyn Fn() -> Option<[u8; 32]> + Send + Sync>,
    configured: std::sync::atomic::AtomicBool,
    load_error: Mutex<Option<String>>,
}

impl ClosedEnv {
    /// `dir` — каталог закрытой среды (создаётся с правами 0700). `legacy_master` отдаёт прежний рабочий ключ для переноса данных.
    pub fn open(dir: &Path, legacy_master: Box<dyn Fn() -> Option<[u8; 32]> + Send + Sync>) -> Result<Arc<Self>> {
        let store = Arc::new(EnvelopeStore::open(dir)?);
        let (configured, load_error) = match store.load() {
            Ok(snapshot) => (snapshot.is_some(), None),
            // повреждённая среда не считается «не настроенной»: иначе её можно было бы тихо создать заново поверх
            Err(e) => (true, Some(e.to_string())),
        };
        let settings: Settings = std::fs::read(dir.join(SETTINGS))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Arc::new(Self {
            dir: dir.to_path_buf(),
            store,
            inner: Mutex::new(Inner {
                vault: None,
                generation: 0,
                last_activity: Instant::now(),
                auto_lock: Duration::from_secs(settings.auto_lock_secs),
            }),
            op: tokio::sync::Mutex::new(()),
            apply: tokio::sync::Mutex::new(()),
            consumers: Mutex::new(Vec::new()),
            legacy_master,
            configured: std::sync::atomic::AtomicBool::new(configured),
            load_error: Mutex::new(load_error),
        }))
    }

    pub fn register(&self, consumer: Arc<dyn Consumer>) {
        self.consumers.lock().unwrap_or_else(|e| e.into_inner()).push(consumer);
    }

    pub fn configured(&self) -> bool {
        self.configured.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn unlocked(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).vault.is_some()
    }

    pub fn status(&self) -> Status {
        let i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Status {
            configured: self.configured(),
            unlocked: i.vault.is_some(),
            migration_pending: self.dir.join(MARKER).exists(),
            auto_lock_secs: i.auto_lock.as_secs(),
            generation: i.generation,
            error: self.load_error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }

    /// Любое действие владельца отодвигает автоблокировку.
    pub fn touch(&self) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).last_activity = Instant::now();
    }

    pub fn set_auto_lock(&self, secs: u64) -> Result<()> {
        let secs = secs.min(30 * 24 * 3600);
        let body = serde_json::to_vec(&Settings { auto_lock_secs: secs })?;
        crate::util::private_file::write_private(&self.dir.join(SETTINGS), &body)?;
        let mut i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        i.auto_lock = Duration::from_secs(secs);
        i.last_activity = Instant::now();
        Ok(())
    }

    /// Фоновая проверка простоя: заперто само, если владелец давно не действовал (0 — не запирать).
    pub fn spawn_auto_lock(self: &Arc<Self>) {
        let env = self.clone();
        crate::supervisor::supervise("closed-env-auto-lock", crate::supervisor::Policy::restart(), move || {
            let env = env.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    let idle = {
                        let i = env.inner.lock().unwrap_or_else(|e| e.into_inner());
                        i.vault.is_some() && !i.auto_lock.is_zero() && i.last_activity.elapsed() >= i.auto_lock
                    };
                    if idle {
                        env.lock().await;
                    }
                }
            }
        });
    }

    fn current(&self, generation: u64) -> bool {
        let i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        i.generation == generation && i.vault.is_some()
    }

    /// Первичная настройка: пароль и четыре слова (обязательны: пять неверных паролей оставляют только сид).
    /// Создаёт среду, переносит данные, открывает сеанс.
    pub async fn setup(&self, password: String, phrase: Option<String>) -> Result<()> {
        let _op = self.op.lock().await;
        if self.configured() {
            bail!("закрытая среда уже создана");
        }
        if phrase.as_deref().map_or(true, |p| p.trim().is_empty()) {
            bail!("нужны четыре секретных слова");
        }
        let (envelope, vault) = EnvelopeStore::create_unlocked_async(password, phrase).await?;
        self.store.prepare_attempts(&envelope)?;
        // метка ДО публикации: оборванный перенос будет доделан при следующем входе
        let marker = self.dir.join(MARKER);
        crate::util::private_file::write_private(&marker, b"1")?;
        if let Err(e) = self.store.save(None, &envelope, &vault) {
            let _ = std::fs::remove_file(&marker);
            let _ = self.store.abort_setup();
            return Err(e);
        }
        self.configured.store(true, std::sync::atomic::Ordering::SeqCst);
        // по умолчанию среда запирается сама через 15 минут бездействия; владелец меняет это в «Настройках»
        let _ = self.set_auto_lock(DEFAULT_AUTO_LOCK_SECS);
        self.install(vault).await
    }

    pub async fn unlock(&self, password: String) -> Result<()> {
        let _op = self.op.lock().await;
        if !self.configured() {
            bail!("закрытая среда не создана");
        }
        if self.unlocked() {
            self.touch();
            return Ok(());
        }
        let vault = match self.store.clone().unlock_async(password, false).await {
            Ok(v) => v,
            Err(e) => return Err(self.after_failed_entry(e).await),
        };
        self.install(vault).await
    }

    /// После неудачного входа: если среда стёрта (пять неверных сид-фраз), она считается не созданной,
    /// ключи у потребителей отзываются, и мастер настройки откроется заново.
    async fn after_failed_entry(&self, e: anyhow::Error) -> anyhow::Error {
        let erased = matches!(self.store.load(), Ok(None));
        if erased && self.configured() {
            self.configured.store(false, std::sync::atomic::Ordering::SeqCst);
            let _ = std::fs::remove_file(self.dir.join(MARKER));
            self.lock().await;
            eprintln!("[closed] ⚠️  закрытая среда стёрта: пять неверных сид-фраз");
        }
        e
    }

    /// Забыли пароль: открыть словами и сразу задать новый.
    pub async fn recover(&self, phrase: String, new_password: String) -> Result<()> {
        let _op = self.op.lock().await;
        if !self.configured() {
            bail!("закрытая среда не создана");
        }
        let vault = match self.store.clone().unlock_async(phrase, true).await {
            Ok(v) => v,
            Err(e) => return Err(self.after_failed_entry(e).await),
        };
        let vault = self.save_new_password(vault, new_password).await?;
        self.install(vault).await
    }

    /// Смена пароля: старый проверяется заново (с теми же счётчиками и паузами). Данные не перешифровываются: DK тот же.
    pub async fn change_password(&self, old_password: String, new_password: String) -> Result<()> {
        let _op = self.op.lock().await;
        if !self.configured() {
            bail!("закрытая среда не создана");
        }
        let checked = self.store.clone().unlock_async(old_password, false).await?;
        let checked = self.save_new_password(checked, new_password).await?;
        drop(checked); // DK стирается; текущий сеанс не затрагивается
        Ok(())
    }

    async fn save_new_password(&self, vault: NodeVault, new_password: String) -> Result<NodeVault> {
        let snapshot: Snapshot = self.store.load()?.ok_or_else(|| anyhow!("закрытая среда не создана"))?;
        let (next, vault) = EnvelopeStore::rewrap_async(snapshot.envelope.clone(), vault, new_password).await?;
        // сначала надёжная запись, только потом пароль считается сменённым
        self.store.save(Some(&snapshot), &next, &vault)?;
        Ok(vault)
    }

    /// Отозвать ключи сразу (синхронно для вызывающего), затем стереть состояние у потребителей.
    pub async fn lock(&self) {
        {
            let mut i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            i.vault = None; // Drop стирает DK
            i.generation += 1;
        }
        let _apply = self.apply.lock().await;
        let consumers: Vec<_> = self.consumers.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for c in consumers {
            c.lock().await;
        }
    }

    /// Принять открытый сеанс и выдать ключи потребителям.
    async fn install(&self, vault: NodeVault) -> Result<()> {
        let consumers: Vec<_> = self.consumers.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut keys: Vec<(Arc<dyn Consumer>, Zeroizing<[u8; 32]>)> = Vec::new();
        for c in consumers {
            let key = vault.subkey(c.domain())?;
            keys.push((c, key));
        }
        let generation = {
            let mut i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            i.generation += 1;
            i.vault = Some(vault);
            i.last_activity = Instant::now();
            i.generation
        };
        let _apply = self.apply.lock().await;
        let marker = self.dir.join(MARKER);
        if marker.exists() {
            let old = (self.legacy_master)();
            let mut failures = 0usize;
            for (c, key) in &keys {
                if !self.current(generation) {
                    return Ok(()); // заперто, пока шёл перенос: ключи не выдаём
                }
                c.lock().await;
                match c.migrate(old, **key).await {
                    Ok(n) => failures += n,
                    Err(e) => {
                        failures += 1;
                        eprintln!("[closed] перенос данных «{}» не завершён: {e}", c.domain());
                    }
                }
            }
            if failures == 0 {
                let _ = std::fs::remove_file(&marker);
            } else {
                eprintln!("[closed] ⚠️  не всё перенесено ({failures}); метка оставлена, перенос повторится при следующем входе");
            }
        }
        for (c, key) in &keys {
            if !self.current(generation) {
                return Ok(());
            }
            c.unlock(**key).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    struct Mock {
        domain: &'static str,
        key: Mutex<Option<[u8; 32]>>,
        migrated_with: Mutex<Vec<(Option<[u8; 32]>, [u8; 32])>>,
        fail_migration: AtomicBool,
        unlock_delay_ms: AtomicU64,
    }

    impl Mock {
        fn new(domain: &'static str) -> Arc<Self> {
            Arc::new(Self {
                domain,
                key: Mutex::new(None),
                migrated_with: Mutex::new(Vec::new()),
                fail_migration: AtomicBool::new(false),
                unlock_delay_ms: AtomicU64::new(0),
            })
        }
        fn key(&self) -> Option<[u8; 32]> {
            *self.key.lock().unwrap()
        }
    }

    impl Consumer for Mock {
        fn domain(&self) -> &'static str {
            self.domain
        }
        fn migrate(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
            Box::pin(async move {
                self.migrated_with.lock().unwrap().push((old, new));
                Ok(usize::from(self.fail_migration.load(Ordering::SeqCst)))
            })
        }
        fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
            Box::pin(async move {
                let delay = self.unlock_delay_ms.load(Ordering::SeqCst);
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                *self.key.lock().unwrap() = Some(key);
            })
        }
        fn lock(&self) -> BoxFuture<'_, ()> {
            Box::pin(async move {
                *self.key.lock().unwrap() = None;
            })
        }
    }

    fn env(base: &Path, legacy: Option<[u8; 32]>) -> (Arc<ClosedEnv>, Arc<Mock>, Arc<Mock>) {
        let env = ClosedEnv::open(&base.join("closed"), Box::new(move || legacy)).unwrap();
        let (a, b) = (Mock::new(DOMAIN_CHAT), Mock::new(DOMAIN_GROUPS));
        env.register(a.clone());
        env.register(b.clone());
        (env, a, b)
    }

    #[tokio::test]
    async fn setup_lock_restart_unlock_and_password_change() {
        let base = tempfile::tempdir().unwrap();
        let (e, chat, groups) = env(base.path(), Some([9; 32]));
        assert!(!e.configured() && !e.unlocked());
        assert!(e.unlock("whatever1".into()).await.is_err(), "не создано");

        e.setup("owner password".into(), Some("alpha1 bravo2 charlie3 delta4".into())).await.unwrap();
        assert!(e.configured() && e.unlocked());
        let (kc, kg) = (chat.key().unwrap(), groups.key().unwrap());
        assert_ne!(kc, kg, "области разделены");
        assert_eq!(chat.migrated_with.lock().unwrap()[0], (Some([9; 32]), kc), "перенос со старого рабочего ключа");
        assert!(!e.status().migration_pending);
        assert!(e.setup("another password".into(), None).await.is_err(), "повторная настройка запрещена");

        e.lock().await;
        assert!(!e.unlocked() && chat.key().is_none() && groups.key().is_none());

        // перезапуск: среда есть, но заперта, потребители ключей не получают
        drop(e);
        let (e, chat, groups) = env(base.path(), Some([9; 32]));
        assert!(e.configured() && !e.unlocked());
        assert!(chat.key().is_none() && groups.key().is_none());
        assert!(e.unlock("wrong password".into()).await.is_err());
        assert!(!e.unlocked() && chat.key().is_none());
        e.unlock("owner password".into()).await.unwrap();
        assert_eq!((chat.key().unwrap(), groups.key().unwrap()), (kc, kg), "ключи областей те же после перезапуска");
        assert!(chat.migrated_with.lock().unwrap().is_empty(), "перенос уже сделан");

        // смена пароля: данные те же, старый пароль не работает
        assert!(e.change_password("not the password".into(), "new password!".into()).await.is_err());
        e.change_password("owner password".into(), "new password!".into()).await.unwrap();
        assert!(e.unlocked(), "смена пароля не запирает сеанс");
        e.lock().await;
        assert!(e.unlock("owner password".into()).await.is_err());
        e.unlock("new password!".into()).await.unwrap();
        assert_eq!(chat.key().unwrap(), kc);

        // забыли пароль: слова + новый пароль
        e.lock().await;
        e.recover("alpha1 bravo2 charlie3 delta4".into(), "recovered pass".into()).await.unwrap();
        assert_eq!(groups.key().unwrap(), kg);
        e.lock().await;
        e.unlock("recovered pass".into()).await.unwrap();
        assert_eq!(chat.key().unwrap(), kc);
    }

    #[tokio::test]
    async fn five_wrong_phrases_erase_the_environment_and_it_can_be_created_again() {
        let base = tempfile::tempdir().unwrap();
        let (e, chat, _groups) = env(base.path(), None);
        let phrase = "alpha1 bravo2 charlie3 delta4";
        e.setup("owner password".into(), Some(phrase.into())).await.unwrap();
        e.lock().await;
        for _ in 0..5 {
            // пауза растёт после четвёртой попытки; для теста подменяем её
            let c = base.path().join("closed").join("attempts.json");
            if let Ok(b) = std::fs::read(&c) {
                if let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&b) {
                    if v["next_ms"].as_u64().unwrap_or(0) > 0 {
                        v["next_ms"] = serde_json::json!(1);
                        std::fs::write(&c, serde_json::to_vec(&v).unwrap()).unwrap();
                    }
                }
            }
            assert!(e.recover("wrong1 wrong2 wrong3 wrong4".into(), "new password".into()).await.is_err());
        }
        assert!(!e.configured(), "после стирания среда не создана");
        assert!(chat.key().is_none());
        e.setup("owner password".into(), Some(phrase.into())).await.unwrap();
        assert!(e.unlocked());
    }

    #[tokio::test]
    async fn unfinished_migration_is_retried_and_lock_wins_over_late_unlock() {
        let base = tempfile::tempdir().unwrap();
        let (e, chat, _groups) = env(base.path(), Some([1; 32]));
        chat.fail_migration.store(true, Ordering::SeqCst);
        e.setup("owner password".into(), Some("alpha1 bravo2 charlie3 delta4".into())).await.unwrap();
        assert!(e.status().migration_pending, "метка осталась");
        e.lock().await;
        chat.fail_migration.store(false, Ordering::SeqCst);
        e.unlock("owner password".into()).await.unwrap();
        assert!(!e.status().migration_pending, "повторный перенос прошёл");
        assert_eq!(chat.migrated_with.lock().unwrap().len(), 2);

        // блокировка во время выдачи ключей: итог — заперто, ключей у потребителей нет
        e.lock().await;
        chat.unlock_delay_ms.store(1500, Ordering::SeqCst);
        let before = e.status().generation;
        let late = e.clone();
        let unlocking = tokio::spawn(async move { late.unlock("owner password".into()).await });
        // ждём момента, когда сеанс уже принят и ключи выдаются (с задержкой), и только тогда блокируем
        while e.status().generation == before {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        e.lock().await;
        let _ = unlocking.await.unwrap();
        assert!(!e.unlocked());
        assert!(chat.key().is_none(), "поздняя разблокировка не оставила ключ");
    }

    #[test]
    fn damaged_environment_is_not_treated_as_new() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("closed");
        {
            let _ = ClosedEnv::open(&dir, Box::new(|| None)).unwrap();
        }
        std::fs::write(dir.join("anchor.json"), b"{broken").unwrap();
        std::fs::write(dir.join("envelope.json"), b"{}").unwrap();
        let e = ClosedEnv::open(&dir, Box::new(|| None)).unwrap();
        assert!(e.configured(), "повреждённое — не «не настроено»");
        assert!(e.status().error.is_some());
    }
}
