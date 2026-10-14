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
        Box::pin(async move { self.0.unlock(key).await })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.0.lock().await })
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
        Box::pin(async move { crate::mobile_self::set_master(key) })
    }
    fn lock(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { crate::mobile_self::lock_keys() })
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
        assert_eq!(code(setup(Json(SetupBody { password: "owner password".into(), phrase: Some("alpha1 bravo2 charlie3 delta4".into()) })).await).await, 200);
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
}
