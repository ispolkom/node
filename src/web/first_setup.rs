//! First run: a tiny local web server that does one job — let the person create the login password and the master password in the
//! browser — before the node creates its identity. The keys have to exist first, because the identity is stored under them.
//!
//! It listens on 127.0.0.1 only, answers only for the `127.0.0.1` / `localhost` host names (a web page on another site cannot reach
//! it through DNS tricks), and stops as soon as the setup has succeeded so the real web server can take the port.
use crate::web::auth::{make_session_cookie, setup_auth, AuthState};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Clone)]
struct Shared {
    auth: AuthState,
    done: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    port: u16,
    /// How the keys are made: the real thing in production, a stand-in in tests.
    make_keys: Arc<dyn Fn(&AuthState, &SetupBody) -> Result<(), String> + Send + Sync>,
}

#[derive(Deserialize)]
struct SetupBody {
    login_password: String,
    #[serde(default)]
    login_password_repeat: String,
    master_password: String,
    #[serde(default)]
    master_password_repeat: String,
}

async fn host_guard(State(sh): State<Shared>, headers: HeaderMap, req: axum::extract::Request, next: Next) -> Response {
    #[cfg(target_os = "linux")]
    if super::ui_socket::is_local(&req) {
        return next.run(req).await;
    }
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let ok = host == format!("127.0.0.1:{}", sh.port) || host == format!("localhost:{}", sh.port);
    if !ok {
        return (StatusCode::MISDIRECTED_REQUEST, "wrong host").into_response();
    }
    next.run(req).await
}

async fn page() -> Html<&'static str> {
    Html(include_str!("ui/setup.html"))
}

async fn setup(State(sh): State<Shared>, Json(body): Json<SetupBody>) -> Response {
    let sh2 = sh.clone();
    let result = tokio::task::spawn_blocking(move || (sh2.make_keys)(&sh2.auth, &body)).await.unwrap_or_else(|_| Err("Внутренняя ошибка".to_string()));
    match result {
        Ok(()) => {
            let token = sh.auth.create_session(false);
            let mut headers = HeaderMap::new();
            headers.insert(header::SET_COOKIE, make_session_cookie(&token, false).parse().unwrap());
            if let Some(tx) = sh.done.lock().ok().and_then(|mut d| d.take()) {
                let _ = tx.send(());
            }
            (StatusCode::OK, headers, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

#[derive(Deserialize)]
struct SeedBody {
    #[serde(default)]
    words: usize,
}

/// A new seed phrase for the first-start wizard: 12 words by default, 24 on request. Nothing is stored: the phrase exists only in this
/// answer and in the wizard's memory until the person has written it down and chosen a password.
async fn new_seed(Json(b): Json<SeedBody>) -> Response {
    let n = if b.words == 0 { 12 } else { b.words };
    match key_root::Seed::generate(n) {
        Some(seed) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({"words": seed.words(), "count": n})),
        )
            .into_response(),
        None => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Нужно 12 или 24 слова"}))).into_response(),
    }
}

/// Восстановление на новой машине вместо первичной настройки: файл копии `.yandibak` + сид-фраза + НОВЫЙ пароль для этой установки.
/// Пароль прежней установки не переносится. Показывается номер копии, её время и узел (заголовок открытый, фраза не нужна).
#[cfg(unix)]
fn read_backup_file(path: &str) -> Result<Vec<u8>, String> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Err("Нужен полный путь к файлу копии".into());
    }
    let meta = std::fs::symlink_metadata(p).map_err(|_| "Файл копии не найден".to_string())?;
    if !meta.is_file() {
        return Err("Это не файл копии".into());
    }
    if meta.len() > 64 * 1024 * 1024 {
        return Err("Файл слишком большой для копии связей".into());
    }
    std::fs::read(p).map_err(|_| "Файл копии не читается".to_string())
}

#[derive(Deserialize)]
struct InspectBody {
    path: String,
}

#[derive(Deserialize)]
struct RestoreBody {
    path: String,
    phrase: String,
    password: String,
}

static RESTORE_FAILURES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const RESTORE_FAILURE_LIMIT: u32 = 20;

async fn restore_inspect(Json(b): Json<InspectBody>) -> Response {
    #[cfg(unix)]
    {
        let r = tokio::task::spawn_blocking(move || {
            let bytes = read_backup_file(&b.path)?;
            let h = crate::node_backup::read_header(&bytes)?;
            let (y, mo, d, hh, mi) = crate::node_backup::civil_utc(h.created_ms / 1000);
            Ok::<_, String>(serde_json::json!({
                "counter": h.counter, "node_id": h.node_id,
                "text": format!("Копия №{} от {y:04}-{mo:02}-{d:02} {hh:02}:{mi:02} UTC; узел {}", h.counter, h.node_id),
            }))
        })
        .await
        .unwrap_or_else(|_| Err("Внутренняя ошибка".into()));
        return match r {
            Ok(v) => Json(v).into_response(),
            Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
        };
    }
    #[cfg(not(unix))]
    {
        let _ = b;
        (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({"error": "Только Linux и macOS"}))).into_response()
    }
}

#[cfg(unix)]
fn do_restore(auth: &AuthState, b: &RestoreBody) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    if b.password.chars().count() < 6 {
        return Err("Пароль — не короче 6 знаков".into());
    }
    // фраза не сид: опечатка, а не попытка (не считается и не запускает дорогой KDF)
    if key_root::looks_like_seed(&b.phrase) {
        key_root::Seed::parse(&b.phrase).map_err(|e| e.to_string())?;
    } else if b.phrase.trim().is_empty() {
        return Err("Введите сид-фразу (12 или 24 слова)".into());
    }
    if RESTORE_FAILURES.load(Ordering::SeqCst) >= RESTORE_FAILURE_LIMIT {
        return Err("Исчерпано число попыток восстановления; перезапустите узел".into());
    }
    let bytes = read_backup_file(&b.path)?;
    let opened = match crate::node_backup::open_with_code(&bytes, &b.phrase) {
        Ok(o) => o,
        Err(e) => {
            RESTORE_FAILURES.fetch_add(1, Ordering::SeqCst);
            return Err(e);
        }
    };
    RESTORE_FAILURES.store(0, Ordering::SeqCst);
    let layout = crate::node_backup::Layout::live(crate::get_config().ports.discovery);
    crate::node_backup::restore(&opened, &layout, &crate::node_backup::RestoreOptions { force: false })?;
    let dir = key_root::KeyDir::open(&layout.keys).map_err(|e| format!("Каталог ключей: {e}"))?;
    let machine = key_root::SystemMachine;
    let device = key_root::HardwareDeviceKey::new(dir.file("device.seal"), dir.file("device.key"));
    let policy = key_root::KdfPolicy::production();
    let secret = key_root::recovery_secret(&b.phrase);
    key_root::Recovery { dir: &dir, port: layout.port, machine: &machine, device: &device, password: &secret, policy: &policy }
        .run()
        .map_err(|e| format!("Файлы записаны, но ключ устройства создать не удалось: {e}"))?;
    crate::node_backup::finalize_vault(&layout, &b.phrase, &b.password)?;
    // тот же вход в состояние, что и после первичной настройки: корень открыт ключом этой машины
    let (root, _) = key_root::unlock_root(&dir, &machine, &device).map_err(|e| format!("Корень не открылся: {e}"))?;
    auth.is_setup.store(true, Ordering::Relaxed);
    auth.needs_rebind.store(false, Ordering::Relaxed);
    if let Ok(mut mk) = auth.master_key.lock() {
        *mk = Some(*root);
    }
    Ok(())
}

async fn restore(State(sh): State<Shared>, Json(body): Json<RestoreBody>) -> Response {
    #[cfg(unix)]
    {
        let sh2 = sh.clone();
        let result = tokio::task::spawn_blocking(move || do_restore(&sh2.auth, &body)).await.unwrap_or_else(|_| Err("Внутренняя ошибка".to_string()));
        return match result {
            Ok(()) => {
                let token = sh.auth.create_session(false);
                let mut headers = HeaderMap::new();
                headers.insert(header::SET_COOKIE, make_session_cookie(&token, false).parse().unwrap());
                if let Some(tx) = sh.done.lock().ok().and_then(|mut d| d.take()) {
                    let _ = tx.send(());
                }
                (StatusCode::OK, headers, Json(serde_json::json!({"ok": true}))).into_response()
            }
            Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
        };
    }
    #[cfg(not(unix))]
    {
        let _ = (sh, body);
        (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({"error": "Только Linux и macOS"}))).into_response()
    }
}

fn router(sh: Shared) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::to("/setup") }))
        .route("/setup", get(page))
        .route("/api/auth/setup", post(setup))
        .route("/api/auth/seed/new", post(new_seed))
        .route("/api/auth/restore/inspect", post(restore_inspect))
        .route("/api/auth/restore", post(restore))
        .route("/api/auth/state", get(|State(sh): State<Shared>| async move {
            crate::web::auth::ui_auth_state(&sh.auth)
        }))
        .layer(middleware::from_fn_with_state(sh.clone(), host_guard))
        .with_state(sh)
}

/// Serve the setup page until the keys have been created, then stop (the response reaches the browser first).
pub async fn run(auth: AuthState, port: u16) -> Result<(), String> {
    run_with(auth, port, Arc::new(|state: &AuthState, b: &SetupBody| {
        setup_auth(state, &b.login_password, &b.login_password_repeat, &b.master_password, &b.master_password_repeat).map(|_| ())
    }))
    .await
}

async fn run_with(auth: AuthState, port: u16, make_keys: Arc<dyn Fn(&AuthState, &SetupBody) -> Result<(), String> + Send + Sync>) -> Result<(), String> {
    // На Linux настройка доступна только оболочке через локальный сокет: TCP-порт в браузер не открывается.
    #[cfg(target_os = "linux")]
    let listener = if std::env::var_os("YANDI_LOCAL_TCP").is_some() {
        // только тренировочная сеть (testnet): управление по петле; на боевом узле переменной нет
        Some(tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("порт {port} занят или недоступен: {e}"))?)
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    let listener = Some(tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("порт {port} занят или недоступен: {e}"))?);
    serve(listener, auth, make_keys, crate::util::data_dir::data_dir()).await
}

async fn serve(listener: Option<tokio::net::TcpListener>, auth: AuthState, make_keys: Arc<dyn Fn(&AuthState, &SetupBody) -> Result<(), String> + Send + Sync>, socket_dir: std::path::PathBuf) -> Result<(), String> {
    let port = match &listener {
        Some(l) => l.local_addr().map_err(|e| e.to_string())?.port(),
        None => 0,
    };
    let (tx, rx) = oneshot::channel::<()>();
    let sh = Shared { auth, done: Arc::new(Mutex::new(Some(tx))), port, make_keys };
    let app = router(sh);
    #[cfg(target_os = "linux")]
    let socket = super::ui_socket::Endpoint::bind(&socket_dir)
        .map_err(|_| "Cannot start setup UI socket: unsafe permissions or occupied socket".to_string())?;
    let app_tcp = app.clone();
    let tcp = async move {
        match listener {
            Some(l) => axum::serve(l, app_tcp)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .into_future()
                .await
                .map_err(|e| e.to_string()),
            // только сокет: ждём, пока настройка не завершится (сам сокет остановит `serve` ниже)
            None => {
                let _ = rx.await;
                Ok(())
            }
        }
    };
    #[cfg(target_os = "linux")]
    {
        tokio::select! {
            result = tcp => result,
            result = socket.serve(app) => result.map_err(|_| "Setup UI socket listener stopped".to_string()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    tcp.await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn start(make: Arc<dyn Fn(&AuthState, &SetupBody) -> Result<(), String> + Send + Sync>) -> (u16, tokio::task::JoinHandle<Result<(), String>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let socket_dir = dir.path().join("data");
        (port, tokio::spawn(async move {
            let _dir = dir;
            serve(Some(listener), AuthState::default(), make, socket_dir).await
        }))
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn setup_over_socket_finishes_and_releases_the_endpoint_for_the_daemon() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint_path = data.join("run/yandi-ui.sock");
        let handle = tokio::spawn(serve(Some(listener), AuthState::default(), Arc::new(|_, _| Ok(())), data.clone()));
        for _ in 0..100 {
            if endpoint_path.exists() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut socket = tokio::net::UnixStream::connect(&endpoint_path).await.unwrap();
        let body = r#"{"login_password":"test-login","master_password":"test-master"}"#;
        socket.write_all(format!("POST /api/auth/setup HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), socket.read_to_end(&mut raw)).await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 200"));
        assert!(tokio::time::timeout(std::time::Duration::from_secs(3), handle).await.unwrap().unwrap().is_ok());
        assert!(!endpoint_path.exists());
        let daemon_endpoint = super::super::ui_socket::Endpoint::bind(&data).unwrap();
        drop(daemon_endpoint);
    }

    #[tokio::test]
    async fn the_page_is_served_and_the_server_stops_after_a_successful_setup() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let (port, handle) = start(Arc::new(move |_, b: &SetupBody| {
            c2.fetch_add(1, Ordering::SeqCst);
            if b.master_password != b.master_password_repeat { Err("Мастер-пароли не совпадают".into()) } else { Ok(()) }
        }))
        .await;
        let base = format!("http://127.0.0.1:{port}");
        let html = client().get(format!("{base}/setup")).send().await.unwrap().text().await.unwrap();
        assert!(html.contains("Повторите пароль входа"), "the page must ask for the login password twice");
        // a failed attempt does not stop the server and says why
        let bad = client().post(format!("{base}/api/auth/setup")).json(&serde_json::json!({"login_password": "loginpass1", "login_password_repeat": "loginpass1", "master_password": "aaaa aaaa aaaa", "master_password_repeat": "bbbb"})).send().await.unwrap();
        assert_eq!(bad.status(), 400);
        assert!(bad.text().await.unwrap().contains("не совпадают"));
        assert!(!handle.is_finished());
        // a good one sets a session cookie, and the server then shuts down by itself
        let ok = client().post(format!("{base}/api/auth/setup")).json(&serde_json::json!({"login_password": "loginpass1", "login_password_repeat": "loginpass1", "master_password": "aaaa aaaa aaaa", "master_password_repeat": "aaaa aaaa aaaa"})).send().await.unwrap();
        assert_eq!(ok.status(), 200);
        assert!(ok.headers().get("set-cookie").is_some());
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), handle).await.expect("the setup server did not stop").unwrap();
        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_wizard_gets_a_new_seed_of_twelve_or_twenty_four_words_and_nothing_is_stored() {
        let (port, handle) = start(Arc::new(|_, _| Ok(()))).await;
        let url = format!("http://127.0.0.1:{port}/api/auth/seed/new");
        for (ask, count) in [(serde_json::json!({}), 12usize), (serde_json::json!({"words": 12}), 12), (serde_json::json!({"words": 24}), 24)] {
            let r = client().post(&url).json(&ask).send().await.unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
            let v: serde_json::Value = r.json().await.unwrap();
            let words: Vec<String> = v["words"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
            assert_eq!(words.len(), count);
            assert!(key_root::Seed::parse(&words.join(" ")).is_ok(), "каждая фраза проходит собственную проверку");
        }
        let a: serde_json::Value = client().post(&url).json(&serde_json::json!({})).send().await.unwrap().json().await.unwrap();
        let b: serde_json::Value = client().post(&url).json(&serde_json::json!({})).send().await.unwrap().json().await.unwrap();
        assert_ne!(a["words"], b["words"], "каждый раз новая");
        assert_eq!(client().post(&url).json(&serde_json::json!({"words": 15})).send().await.unwrap().status(), 400);
        assert!(!handle.is_finished(), "сид сам по себе настройку не завершает");
        handle.abort();
    }

    #[tokio::test]
    async fn a_request_for_another_host_name_is_refused() {
        let (port, handle) = start(Arc::new(|_, _| Ok(()))).await;
        let resp = client().get(format!("http://127.0.0.1:{port}/setup")).header("Host", "evil.example").send().await.unwrap();
        assert_eq!(resp.status(), 421);
        let resp = client().post(format!("http://127.0.0.1:{port}/api/auth/setup")).header("Host", "evil.example:80").json(&serde_json::json!({"login_password":"x","master_password":"y"})).send().await.unwrap();
        assert_eq!(resp.status(), 421);
        handle.abort();
    }

    #[tokio::test]
    async fn nothing_but_the_setup_is_served() {
        let (port, handle) = start(Arc::new(|_, _| Ok(()))).await;
        for path in ["/api/chats", "/login", "/api/auth/login", "/pair/issue", "/index.html"] {
            let r = client().get(format!("http://127.0.0.1:{port}{path}")).send().await.unwrap();
            assert_eq!(r.status(), 404, "{path} must not exist in the setup server");
        }
        handle.abort();
    }
}
