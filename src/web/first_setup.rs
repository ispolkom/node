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

fn router(sh: Shared) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::to("/setup") }))
        .route("/setup", get(page))
        .route("/api/auth/setup", post(setup))
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
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("порт {port} занят или недоступен: {e}"))?;
    serve(listener, auth, make_keys).await
}

async fn serve(listener: tokio::net::TcpListener, auth: AuthState, make_keys: Arc<dyn Fn(&AuthState, &SetupBody) -> Result<(), String> + Send + Sync>) -> Result<(), String> {
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let (tx, rx) = oneshot::channel::<()>();
    let sh = Shared { auth, done: Arc::new(Mutex::new(Some(tx))), port, make_keys };
    axum::serve(listener, router(sh))
        .with_graceful_shutdown(async move {
            let _ = rx.await;
        })
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn start(make: Arc<dyn Fn(&AuthState, &SetupBody) -> Result<(), String> + Send + Sync>) -> (u16, tokio::task::JoinHandle<Result<(), String>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (port, tokio::spawn(serve(listener, AuthState::default(), make)))
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
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
