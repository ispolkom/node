//! Подключение закрытой среды к узлу: потребители ([`ChatConsumer`], [`GroupsConsumer`], [`SelfConsumer`]) и ручки владельца
//! `/api/vault/*` (за входом в веб-интерфейс).

use crate::closed_env::{ClosedEnv, Consumer, DOMAIN_CHAT, DOMAIN_GROUPS, DOMAIN_PROXY, DOMAIN_SELF};
use crate::communication::ChatManager;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, OnceLock};

static GLOBAL: OnceLock<Arc<ClosedEnv>> = OnceLock::new();

/// Задаётся один раз при запуске.
pub fn install_global(env: Arc<ClosedEnv>) {
    let _ = GLOBAL.set(env);
}

pub fn global() -> Option<&'static Arc<ClosedEnv>> {
    GLOBAL.get()
}

/// Закрыта ли сейчас закрытая среда (настроена и заперта). Для ручек, которым нужно честно ответить «заперто».
pub fn is_locked() -> bool {
    global().is_some_and(|e| e.configured() && !e.unlocked())
}

pub struct ChatConsumer(pub Arc<ChatManager>);

impl Consumer for ChatConsumer {
    fn domain(&self) -> &'static str {
        DOMAIN_CHAT
    }
    fn migrate(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move {
            let cm = self.0.clone();
            tokio::task::spawn_blocking(move || cm.rekey_to(old, new).map_err(|e| e.to_string()))
                .await
                .map_err(|e| e.to_string())?
        })
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::chat_reactions::unlock(key); // реакции личных чатов — на том же ключе, что и сама переписка
            crate::local_overrides::unlock(key); // локальные имена и аватары контактов — там же
            self.0.unlock(key).await
        })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::chat_reactions::lock();
            crate::local_overrides::lock();
            self.0.lock().await
        })
    }
    fn erase(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::chat_reactions::erase();
            crate::local_overrides::erase();
            let cm = self.0.clone();
            let _ = tokio::task::spawn_blocking(move || cm.erase_encrypted()).await;
        })
    }
}

pub struct GroupsConsumer;

impl Consumer for GroupsConsumer {
    fn domain(&self) -> &'static str {
        DOMAIN_GROUPS
    }
    fn migrate(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || crate::group_client::rekey(old, new))
                .await
                .map_err(|e| e.to_string())?
        })
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = tokio::task::spawn_blocking(move || crate::group_client::unlock(key)).await;
        })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::group_client::lock() })
    }
    fn erase(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = tokio::task::spawn_blocking(crate::group_client::erase_encrypted).await;
        })
    }
    fn leftovers(&self) -> Vec<String> {
        let dir = crate::util::data_dir::data_dir().join("groups_pc");
        std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path().display().to_string()).collect()).unwrap_or_default()
    }
    fn is_service(&self) -> bool {
        true
    }
    fn adopt(&self, owner_key: [u8; 32], service_key: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move {
            // клиент групп ПК на время переноса заперт: файлы переписываются только под замком
            crate::group_client::lock();
            tokio::task::spawn_blocking(move || crate::group_client::rekey(Some(owner_key), service_key))
                .await
                .map_err(|e| e.to_string())?
        })
    }
}

pub struct SelfConsumer;

impl Consumer for SelfConsumer {
    fn domain(&self) -> &'static str {
        DOMAIN_SELF
    }
    fn migrate(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move {
            let dir = crate::util::data_dir::data_dir();
            tokio::task::spawn_blocking(move || {
                crate::mobile_self::rekey(&dir, old.as_ref(), &new).map(|_| 0usize)
            })
            .await
            .map_err(|e| e.to_string())?
        })
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::mobile_self::set_master(key);
            crate::mobile_self::unread_unlock(key);
            // открытая часть связки ключей ПК записывается рядом: пока среда заперта, телефоны всё равно могут писать ПК
            let _ = crate::mobile_self::bundle_json();
        })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::mobile_self::lock_keys();
            crate::mobile_self::unread_lock();
        })
    }
    fn erase(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::mobile_self::unread_erase();
            let _ = std::fs::remove_file(crate::util::data_dir::data_dir().join("mobile_self.enc"));
        })
    }
    fn leftovers(&self) -> Vec<String> {
        let p = crate::util::data_dir::data_dir().join("mobile_self.enc");
        if p.exists() { vec![p.display().to_string()] } else { Vec::new() }
    }
    fn is_service(&self) -> bool {
        true
    }
    fn adopt(&self, owner_key: [u8; 32], service_key: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move {
            let dir = crate::util::data_dir::data_dir();
            crate::mobile_self::lock_keys();
            tokio::task::spawn_blocking(move || crate::mobile_self::rekey(&dir, Some(&owner_key), &service_key).map(|_| 0usize))
                .await
                .map_err(|e| e.to_string())?
        })
    }
}

/// Жалобы пользователей и журнал мер: закрытый уровень.
pub struct ComplaintsConsumer;

impl Consumer for ComplaintsConsumer {
    fn domain(&self) -> &'static str {
        crate::closed_env::DOMAIN_COMPLAINTS
    }
    fn migrate(&self, _old: Option<[u8; 32]>, _new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move { Ok(0) })
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::complaints::unlock(key) })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::complaints::lock() })
    }
    fn erase(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::complaints::erase() })
    }
    fn leftovers(&self) -> Vec<String> {
        let p = crate::util::data_dir::data_dir().join("complaints.enc");
        if p.exists() { vec![p.display().to_string()] } else { Vec::new() }
    }
    fn is_service(&self) -> bool {
        true
    }
    fn adopt(&self, owner_key: [u8; 32], service_key: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move { crate::complaints::adopt(owner_key, service_key).map(|_| 0usize) })
    }
}

/// Файлы переписки (принятые, отправляемые, части передач): на диске только запечатанными блоками, личный уровень.
pub struct FilesConsumer;

impl Consumer for FilesConsumer {
    fn domain(&self) -> &'static str {
        crate::closed_env::DOMAIN_FILES
    }
    fn migrate(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move {
            // до пароля файлы запечатаны рабочим ключом машины (он нужен, чтобы узел принимал файлы и без владельца): при
            // установке пароля они перезапечатываются ключом владельца
            let failed = match old {
                Some(m) => {
                    let old_key = crate::closed_env::service_key_for(&m, crate::closed_env::DOMAIN_FILES);
                    tokio::task::spawn_blocking(move || {
                        let root = crate::util::data_dir::data_dir().join("files");
                        crate::file_vault::rekey_dir(&root, &old_key, &new)
                    })
                    .await
                    .map_err(|e| e.to_string())?
                }
                None => 0,
            };
            crate::file_vault::unlock(new);
            Ok(failed)
        })
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::file_vault::unlock(key) })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::file_vault::lock() })
    }
    fn erase(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::file_vault::lock();
            let _ = std::fs::remove_dir_all(crate::util::data_dir::data_dir().join("files"));
        })
    }
    fn leftovers(&self) -> Vec<String> {
        let p = crate::util::data_dir::data_dir().join("files");
        if p.exists() { vec![p.display().to_string()] } else { Vec::new() }
    }
}

/// Ключ подписи списка входных узлов: файла нет, ключ — подключ закрытой среды, в памяти только пока среда открыта.
pub struct BootstrapSignerConsumer;

impl Consumer for BootstrapSignerConsumer {
    fn domain(&self) -> &'static str {
        crate::closed_env::DOMAIN_BOOTSTRAP_SIGNER
    }
    fn migrate(&self, _old: Option<[u8; 32]>, _new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move { Ok(0) }) // на диске ничего нет
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::bootstrap::signer::unlock(key) })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::bootstrap::signer::lock() })
    }
}

/// Настройки внешних прокси (в них логины и пароли): закрытый уровень.
pub struct ProxyConsumer;

impl Consumer for ProxyConsumer {
    fn domain(&self) -> &'static str {
        DOMAIN_PROXY
    }
    fn migrate(&self, _old: Option<[u8; 32]>, new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || crate::upstream_proxy::seal_existing(&new).map(|_| 0usize))
                .await
                .map_err(|e| e.to_string())?
        })
    }
    fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::upstream_proxy::unlock(key) })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::upstream_proxy::lock() })
    }
    fn erase(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = std::fs::remove_file(crate::util::data_dir::data_dir().join("upstream_proxy.enc"));
        })
    }
    fn leftovers(&self) -> Vec<String> {
        let p = crate::util::data_dir::data_dir().join("upstream_proxy.enc");
        if p.exists() { vec![p.display().to_string()] } else { Vec::new() }
    }
}

#[derive(Deserialize)]
struct SetupBody {
    password: String,
    #[serde(default)]
    phrase: Option<String>,
}

#[derive(Deserialize)]
struct UnlockBody {
    password: String,
}

#[derive(Deserialize)]
struct ChangeBody {
    old_password: String,
    new_password: String,
}

#[derive(Deserialize)]
struct RecoverBody {
    phrase: String,
    new_password: String,
}

#[derive(Deserialize)]
struct AutoLockBody {
    secs: u64,
}

fn env() -> Result<&'static Arc<ClosedEnv>, Response> {
    global().ok_or_else(|| {
        (StatusCode::NOT_IMPLEMENTED, Json(json!({"error": "закрытая среда не включена на этой платформе"}))).into_response()
    })
}

/// Вход по сид-фразе (`/api/auth/recover`): если закрытая среда создана, фраза СНАЧАЛА проходит её счёт — двадцать неверных стирают
/// среду, опечатка не считается (P11-12: иначе это был бы второй проверочный путь для того же секрета без лимита). Новый пароль
/// ставится сразу и на закрытую среду. `None` — среда не создана или недоступна: прежний порядок. Ошибка — готовый код и текст.
pub(crate) async fn recover_vault_if_configured(phrase: String, new_password: String) -> Option<Result<(), (StatusCode, String)>> {
    let e = global()?;
    if !e.configured() {
        return None;
    }
    Some(match e.recover(phrase, new_password).await {
        Ok(()) => Ok(()),
        Err(err) => {
            let text = err.to_string();
            let code = if text.contains("пауза") || text.contains("заблокирован до KDF") {
                StatusCode::TOO_MANY_REQUESTS
            } else if text.contains("неверный секрет") {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((code, text))
        }
    })
}

/// Код ответа по тексту ошибки слоя входа: паузы — 429, неверный секрет — 401, прочее — 400.
fn fail(e: anyhow::Error) -> Response {
    let text = e.to_string();
    let code = if text.contains("пауза") || text.contains("заблокирован до KDF") {
        StatusCode::TOO_MANY_REQUESTS
    } else if text.contains("неверный секрет") {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::BAD_REQUEST
    };
    (code, Json(json!({"error": text}))).into_response()
}

fn ok(env: &ClosedEnv) -> Response {
    Json(json!({"ok": true, "status": env.status()})).into_response()
}

async fn status() -> Response {
    match env() {
        Ok(e) => Json(e.status()).into_response(),
        Err(r) => r,
    }
}

async fn setup(Json(b): Json<SetupBody>) -> Response {
    let e = match env() { Ok(e) => e, Err(r) => return r };
    // сид закрытой среды обязан быть сидом узла: два независимых секрета ломают восстановление (проверка до создания; среды ещё нет, считать нечего)
    #[cfg(unix)]
    if let Some(p) = b.phrase.clone().filter(|p| !p.trim().is_empty()) {
        let check = tokio::task::spawn_blocking(move || crate::web::auth::phrase_matches_identity(&p)).await.unwrap_or_else(|_| Err("Внутренняя ошибка".into()));
        if let Err(m) = check {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": m}))).into_response();
        }
    }
    match e.setup(b.password, b.phrase.filter(|p| !p.trim().is_empty())).await {
        Ok(()) => ok(e),
        Err(err) => fail(err),
    }
}

async fn unlock(Json(b): Json<UnlockBody>) -> Response {
    let e = match env() { Ok(e) => e, Err(r) => return r };
    match e.unlock(b.password).await {
        Ok(()) => ok(e),
        Err(err) => fail(err),
    }
}

async fn lock() -> Response {
    let e = match env() { Ok(e) => e, Err(r) => return r };
    e.lock().await;
    ok(e)
}

async fn change_password(Json(b): Json<ChangeBody>) -> Response {
    let e = match env() { Ok(e) => e, Err(r) => return r };
    match e.change_password(b.old_password, b.new_password).await {
        Ok(()) => ok(e),
        Err(err) => fail(err),
    }
}

async fn recover(Json(b): Json<RecoverBody>) -> Response {
    let e = match env() { Ok(e) => e, Err(r) => return r };
    match e.recover(b.phrase, b.new_password).await {
        Ok(()) => ok(e),
        Err(err) => fail(err),
    }
}

#[derive(Deserialize)]
struct PanicBody {
    confirm: String,
}

/// Слово, которое нужно ввести для экстренного стирания (защита от случайного нажатия и от чужого запроса без ведома владельца).
pub const PANIC_WORD: &str = "СТЕРЕТЬ";

/// Слово на другом языке интерфейса: у человека может не быть кириллицы на клавиатуре. Принимаются оба слова в любом регистре.
pub const PANIC_WORD_EN: &str = "DELETE";

fn panic_word_ok(typed: &str) -> bool {
    let t = typed.trim().to_uppercase();
    t == PANIC_WORD || t == PANIC_WORD_EN
}

async fn panic_erase(Json(b): Json<PanicBody>) -> Response {
    let e = match env() { Ok(e) => e, Err(r) => return r };
    if !panic_word_ok(&b.confirm) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("Для стирания введите слово {PANIC_WORD} (по-английски {PANIC_WORD_EN})")}))).into_response();
    }
    match e.panic_erase().await {
        Ok(()) => ok(e),
        Err(err) => fail(err),
    }
}

async fn auto_lock(Json(b): Json<AutoLockBody>) -> Response {
    let e = match env() { Ok(e) => e, Err(r) => return r };
    match e.set_auto_lock(b.secs) {
        Ok(()) => ok(e),
        Err(err) => fail(err),
    }
}

/// Ручки владельца; подключаются под проверкой входа в веб-интерфейс.
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/vault/status", get(status))
        .route("/api/vault/setup", post(setup))
        .route("/api/vault/unlock", post(unlock))
        .route("/api/vault/lock", post(lock))
        .route("/api/vault/change_password", post(change_password))
        .route("/api/vault/recover", post(recover))
        .route("/api/vault/auto_lock", post(auto_lock))
        .route("/api/vault/panic_erase", post(panic_erase))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Probe(Mutex<Option<[u8; 32]>>);

    impl Consumer for Probe {
        fn domain(&self) -> &'static str {
            DOMAIN_CHAT
        }
        fn migrate(&self, _old: Option<[u8; 32]>, _new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
            Box::pin(async { Ok(0) })
        }
        fn unlock(&self, key: [u8; 32]) -> BoxFuture<'_, ()> {
            Box::pin(async move { *self.0.lock().unwrap() = Some(key) })
        }
        fn lock(&self) -> BoxFuture<'_, ()> {
            Box::pin(async move { *self.0.lock().unwrap() = None })
        }
    }

    async fn code(r: Response) -> u16 {
        r.status().as_u16()
    }

    /// Ручки владельца на настоящей среде во временной папке (потребитель — проба, боевых данных нет).
    #[tokio::test]
    async fn owner_endpoints_flow_and_error_codes() {
        let base = tempfile::tempdir().unwrap();
        let env = ClosedEnv::open(&base.path().join("closed"), Box::new(|| None)).unwrap();
        let probe = Arc::new(Probe(Mutex::new(None)));
        env.register(probe.clone());
        install_global(env.clone());

        assert!(!is_locked(), "не создана — не «заперта»");
        assert_eq!(code(setup(Json(SetupBody { password: "short".into(), phrase: None })).await).await, 400);
        assert_eq!(code(setup(Json(SetupBody { password: "owner password".into(), phrase: Some("   ".into()) })).await).await, 400, "без слов среду не создать");
        assert_eq!(code(setup(Json(SetupBody { password: "owner password".into(), phrase: Some("legal winner thank year wave sausage worth useful legal winner thank yellow".into()) })).await).await, 200);
        assert!(probe.0.lock().unwrap().is_some());
        assert_eq!(code(setup(Json(SetupBody { password: "owner password".into(), phrase: None })).await).await, 400, "повторно нельзя");

        assert_eq!(code(lock().await).await, 200);
        assert!(is_locked() && probe.0.lock().unwrap().is_none());
        assert_eq!(code(unlock(Json(UnlockBody { password: "wrong password".into() })).await).await, 401);
        assert!(is_locked());
        assert_eq!(code(unlock(Json(UnlockBody { password: "owner password".into() })).await).await, 200);
        assert!(!is_locked() && probe.0.lock().unwrap().is_some());
        assert_eq!(code(auto_lock(Json(AutoLockBody { secs: 600 })).await).await, 200);
        assert_eq!(env.status().auto_lock_secs, 600);
    }

    #[test]
    fn the_panic_word_is_exact_apart_from_edge_spaces() {
        assert!(panic_word_ok("СТЕРЕТЬ") && panic_word_ok("  СТЕРЕТЬ ") && panic_word_ok("стереть"), "регистр не важен");
        assert!(panic_word_ok("DELETE") && panic_word_ok(" delete ") && panic_word_ok("Delete"), "английское слово для клавиатур без кириллицы");
        for no in ["", "нет", "СТЕРЕТЬ!", "ERASE", "DELET", "стереть всё"] {
            assert!(!panic_word_ok(no), "{no}");
        }
    }

    struct Spy {
        erased: Arc<Mutex<bool>>,
        locked: Arc<Mutex<bool>>,
        /// что «осталось» после стирания (для проверки, что неудача не выдаётся за успех)
        left: Vec<String>,
    }
    impl Consumer for Spy {
        fn domain(&self) -> &'static str {
            "panic_spy"
        }
        fn migrate(&self, _old: Option<[u8; 32]>, _new: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> {
            Box::pin(async { Ok(0) })
        }
        fn unlock(&self, _key: [u8; 32]) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn lock(&self) -> BoxFuture<'_, ()> {
            let f = self.locked.clone();
            Box::pin(async move { *f.lock().unwrap() = true })
        }
        fn erase(&self) -> BoxFuture<'_, ()> {
            let f = self.erased.clone();
            Box::pin(async move { *f.lock().unwrap() = true })
        }
        fn leftovers(&self) -> Vec<String> {
            self.left.clone()
        }
        fn is_service(&self) -> bool {
            true // служебная часть: обычное запирание её не трогает, а стирание обязано
        }
    }

    const PHRASE: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

    /// Стирание работает на самой среде (общая глобальная ячейка здесь не нужна и в тестах заполняется только один раз).
    #[tokio::test]
    async fn the_panic_erase_revokes_every_consumer_including_services_erases_and_it_can_be_created_again() {
        let base = tempfile::tempdir().unwrap();
        // рабочий ключ устройства есть: служебные части живут отдельно от закрытой среды (как на настоящем узле)
        let env = ClosedEnv::open(&base.path().join("closed"), Box::new(|| Some([7u8; 32]))).unwrap();
        let (erased, locked) = (Arc::new(Mutex::new(false)), Arc::new(Mutex::new(false)));
        env.register(Arc::new(Spy { erased: erased.clone(), locked: locked.clone(), left: vec![] }));
        env.setup("owner password".into(), Some(PHRASE.into())).await.unwrap();
        assert!(env.configured() && env.unlocked());
        *locked.lock().unwrap() = false;
        env.panic_erase().await.unwrap();
        assert!(*locked.lock().unwrap(), "служебная часть тоже отозвана: ключи в памяти не переживают стирание");
        assert!(!env.configured() && !env.unlocked() && *erased.lock().unwrap(), "среда стёрта, потребители удалили свои данные");
        assert!(env.unlock("owner password".into()).await.is_err(), "прежний пароль больше ничего не открывает");
        assert!(env.recover(PHRASE.into(), "another password".into()).await.is_err(), "и сид-фраза тоже: ключа данных больше нет");
        // после стирания среду можно создать заново
        env.setup("new owner password".into(), Some(PHRASE.into())).await.unwrap();
        assert!(env.configured());
    }

    #[tokio::test]
    async fn leftovers_after_the_erase_are_reported_not_hidden() {
        let base = tempfile::tempdir().unwrap();
        let env = ClosedEnv::open(&base.path().join("closed"), Box::new(|| None)).unwrap();
        env.register(Arc::new(Spy { erased: Default::default(), locked: Default::default(), left: vec!["/data/groups_pc/state.enc".into()] }));
        env.setup("owner password".into(), Some(PHRASE.into())).await.unwrap();
        let err = env.panic_erase().await.unwrap_err().to_string();
        assert!(err.contains("не завершено") && err.contains("state.enc"), "{err}");
        assert!(!env.unlocked(), "ключи отозваны, даже когда остатки на диске есть");
    }

    /// P14-8: часть, где осталось зашифрованное содержимое, не получает ключ заново — иначе она прочла бы старые данные.
    #[tokio::test]
    async fn a_service_with_leftovers_is_not_started_again_but_a_clean_one_is() {
        let base = tempfile::tempdir().unwrap();
        let env = ClosedEnv::open(&base.path().join("closed"), Box::new(|| Some([7u8; 32]))).unwrap();
        let (dirty_locked, clean_locked) = (Arc::new(Mutex::new(false)), Arc::new(Mutex::new(false)));
        struct Part(Arc<Mutex<bool>>, &'static str, Vec<String>);
        impl Consumer for Part {
            fn domain(&self) -> &'static str { self.1 }
            fn leftovers(&self) -> Vec<String> { self.2.clone() }
            fn migrate(&self, _o: Option<[u8; 32]>, _n: [u8; 32]) -> BoxFuture<'_, Result<usize, String>> { Box::pin(async { Ok(0) }) }
            fn unlock(&self, _k: [u8; 32]) -> BoxFuture<'_, ()> { let f = self.0.clone(); Box::pin(async move { *f.lock().unwrap() = false }) }
            fn lock(&self) -> BoxFuture<'_, ()> { let f = self.0.clone(); Box::pin(async move { *f.lock().unwrap() = true }) }
            fn is_service(&self) -> bool { true }
        }
        env.register(Arc::new(Part(dirty_locked.clone(), "panic_dirty", vec!["/data/complaints.enc".into()])));
        env.register(Arc::new(Part(clean_locked.clone(), "panic_clean", vec![])));
        env.setup("owner password".into(), Some(PHRASE.into())).await.unwrap();
        assert!(env.panic_erase().await.is_err());
        assert!(*dirty_locked.lock().unwrap(), "часть с остатками осталась запертой");
        assert!(!*clean_locked.lock().unwrap(), "чистая служебная часть поднята заново");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_disk_failure_is_an_error_and_the_keys_are_revoked_anyway() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("closed");
        let env = ClosedEnv::open(&dir, Box::new(|| None)).unwrap();
        env.setup("owner password".into(), Some(PHRASE.into())).await.unwrap();
        // каталог нельзя изменять: удалить файлы не получится
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let res = env.panic_erase().await;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let err = res.unwrap_err().to_string();
        assert!(err.contains("не завершено"), "{err}");
        assert!(env.configured(), "ключ данных остался на диске — среда не считается стёртой");
        assert!(!env.unlocked(), "но в памяти ключи отозваны");
        // после починки диска повторное стирание доводит дело до конца
        env.panic_erase().await.unwrap();
        assert!(!env.configured());
    }
}

#[cfg(test)]
mod files_consumer_tests {
    use super::*;

    #[tokio::test]
    async fn erase_removes_conversation_files_with_the_environment() {
        let down = crate::util::data_dir::data_dir().join("files").join("downloads");
        std::fs::create_dir_all(&down).unwrap();
        let p = down.join("consumer-test__a.txt");
        let c = FilesConsumer;
        assert_eq!(c.migrate(None, [0x5a; 32]).await.unwrap(), 0);
        crate::file_vault::write_all(&p, "файл".as_bytes()).unwrap();
        let raw = std::fs::read(&p).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("файл"));
        c.erase().await;
        assert!(!p.exists(), "стирание среды удаляет и файлы переписки");
        crate::file_vault::unlock([0x5a; 32]);
    }
}
