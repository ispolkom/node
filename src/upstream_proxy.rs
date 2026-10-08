//! Внешний прокси для выхода в интернет (этап J дорожной карты).
//!
//! Узел в стране, откуда нельзя достучаться до внешнего интернета напрямую, выходит наружу через прокси, который владелец прописал в настройках
//! (например, купленный). Прокси касается ТОЛЬКО выхода в интернет: переписка, звонки, файлы и всё остальное между узлами идёт по нашему
//! транспорту и прокси не использует.
//!
//! Правила, которые здесь держатся:
//! * все соединения выхода идут через `exit_policy::connect_public`, а она, пока прокси включён, зовёт только `connect` отсюда;
//! * имя цели передаётся прокси как есть (разрешает имя прокси, а не этот компьютер: запрос имени не утекает в обход);
//! * если прокси включён и не отвечает, соединение **не уходит напрямую**: вызывающий получает ошибку;
//! * пароль лежит в закрытом файле (права 0600) и никогда не отдаётся интерфейсом назад.
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Как говорить с прокси.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Socks5,
    /// метод CONNECT обычного HTTP(S)-прокси
    HttpConnect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    None,
    Password { user: String, password: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    pub auth: Auth,
}

impl Settings {
    /// Проверка того, что ввёл владелец. Ошибка — готовая фраза для интерфейса.
    pub fn check(&self) -> Result<(), String> {
        let host_ok = !self.host.is_empty()
            && self.host.len() <= 253
            && (self.host.parse::<std::net::IpAddr>().is_ok()
                || self.host.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')));
        if !host_ok {
            return Err("Адрес прокси: имя узла или IP без пробелов и служебных знаков.".into());
        }
        if self.port == 0 {
            return Err("Порт прокси: число от 1 до 65535.".into());
        }
        if let Auth::Password { user, password } = &self.auth {
            if user.is_empty() || user.len() > 255 || password.len() > 255 || user.chars().any(|c| c.is_control() || c == ':') || password.chars().any(char::is_control) {
                return Err("Логин не пустой, до 255 знаков и без двоеточия; пароль до 255 знаков; служебных знаков в них быть не должно.".into());
            }
        }
        Ok(())
    }
}

fn bad(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, msg.into())
}

/// Соединиться с `host:port` через прокси. Всё в пределах `limit`.
pub async fn connect(s: &Settings, host: &str, port: u16, limit: Duration) -> std::io::Result<TcpStream> {
    match tokio::time::timeout(limit, connect_inner(s, host, port)).await {
        Ok(r) => r,
        Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "прокси не ответил вовремя")),
    }
}

async fn connect_inner(s: &Settings, host: &str, port: u16) -> std::io::Result<TcpStream> {
    // имя цели идёт прокси; в запрос не должно попасть ничего служебного
    if host.is_empty() || host.len() > 253 || host.bytes().any(|b| b <= 0x20 || b == 0x7f) || port == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "неверная цель"));
    }
    let mut stream = TcpStream::connect((s.host.as_str(), s.port)).await.map_err(|e| bad(format!("прокси недоступен: {e}")))?;
    let _ = stream.set_nodelay(true);
    match s.kind {
        Kind::Socks5 => socks5(&mut stream, &s.auth, host, port).await?,
        Kind::HttpConnect => http_connect(&mut stream, &s.auth, host, port).await?,
    }
    Ok(stream)
}

async fn socks5(st: &mut TcpStream, auth: &Auth, host: &str, port: u16) -> std::io::Result<()> {
    // приветствие: умеем «без входа» и «логин и пароль» (если они заданы)
    match auth {
        Auth::None => st.write_all(&[5, 1, 0]).await?,
        Auth::Password { .. } => st.write_all(&[5, 2, 0, 2]).await?,
    }
    let mut r = [0u8; 2];
    st.read_exact(&mut r).await?;
    if r[0] != 5 {
        return Err(bad("это не SOCKS5-прокси"));
    }
    match (r[1], auth) {
        (0, _) => {}
        (2, Auth::Password { user, password }) => {
            let mut m = vec![1u8, user.len() as u8];
            m.extend_from_slice(user.as_bytes());
            m.push(password.len() as u8);
            m.extend_from_slice(password.as_bytes());
            st.write_all(&m).await?;
            let mut a = [0u8; 2];
            st.read_exact(&mut a).await?;
            if a[1] != 0 {
                return Err(bad("прокси не принял логин и пароль"));
            }
        }
        (0xff, _) => return Err(bad("прокси не принимает наш способ входа")),
        _ => return Err(bad("прокси выбрал неожиданный способ входа")),
    }
    // просьба соединиться: адрес IP как есть, имя — именем (разрешит прокси)
    let mut req = vec![5u8, 1, 0];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => {
            req.push(1);
            req.extend_from_slice(&a.octets());
        }
        Ok(std::net::IpAddr::V6(a)) => {
            req.push(4);
            req.extend_from_slice(&a.octets());
        }
        Err(_) => {
            req.push(3);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    st.write_all(&req).await?;
    let mut h = [0u8; 4];
    st.read_exact(&mut h).await?;
    if h[0] != 5 {
        return Err(bad("неверный ответ прокси"));
    }
    if h[1] != 0 {
        return Err(bad(match h[1] {
            1 => "прокси: общий отказ",
            2 => "прокси: правила прокси не пускают к этой цели",
            3 => "прокси: сеть недоступна",
            4 => "прокси: цель недоступна",
            5 => "прокси: цель отказала в соединении",
            6 => "прокси: время вышло",
            7 => "прокси: команда не поддерживается",
            8 => "прокси: такой тип адреса не поддерживается",
            _ => "прокси: неизвестный отказ",
        }));
    }
    // остаток ответа (адрес, на котором прокси соединился) прочитать и выбросить
    let rest = match h[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut l = [0u8; 1];
            st.read_exact(&mut l).await?;
            l[0] as usize + 2
        }
        _ => return Err(bad("неверный ответ прокси")),
    };
    let mut skip = vec![0u8; rest];
    st.read_exact(&mut skip).await?;
    Ok(())
}

async fn http_connect(st: &mut TcpStream, auth: &Auth, host: &str, port: u16) -> std::io::Result<()> {
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n");
    if let Auth::Password { user, password } = auth {
        use base64::Engine;
        req.push_str(&format!("Proxy-Authorization: Basic {}\r\n", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))));
    }
    req.push_str("\r\n");
    st.write_all(req.as_bytes()).await?;
    // ответ: читаем до пустой строки, но не больше 8 КБ (за концом заголовков могут быть уже данные цели — их не трогаем)
    let mut buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if buf.len() > 8192 {
            return Err(bad("слишком длинный ответ прокси"));
        }
        st.read_exact(&mut byte).await?;
        buf.push(byte[0]);
    }
    let first = String::from_utf8_lossy(&buf).lines().next().unwrap_or("").to_string();
    let code = first.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok());
    match code {
        Some(200..=299) => Ok(()),
        Some(407) => Err(bad("прокси не принял логин и пароль")),
        Some(c) => Err(bad(format!("прокси ответил отказом ({c})"))),
        None => Err(bad("это не HTTP-прокси")),
    }
}

// ---------------------------------------------------------------- настройки узла: файл и состояние

fn path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("upstream_proxy.json")
}

fn cell() -> &'static Mutex<Option<Settings>> {
    static C: OnceLock<Mutex<Option<Settings>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()).filter(|s| s.check().is_ok())))
}

/// Включённый прокси, если он есть.
pub fn active() -> Option<Settings> {
    cell().lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|s| s.enabled)
}

/// Что сохранено (включённым или нет).
pub fn current() -> Option<Settings> {
    cell().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Сохранить настройки: проверить, записать в закрытый файл, начать использовать.
pub fn save(s: Settings) -> Result<(), String> {
    s.check()?;
    let bytes = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
    crate::util::private_file::write_private(&path(), &bytes).map_err(|e| format!("не удалось записать настройки: {e}"))?;
    *cell().lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
    Ok(())
}

/// Сохранить только включённость (кнопка «выключить» без ввода всего заново).
pub fn set_enabled(on: bool) -> Result<(), String> {
    let mut s = current().ok_or_else(|| "прокси не настроен".to_string())?;
    s.enabled = on;
    save(s)
}

#[derive(Default, Clone, Serialize)]
pub struct Status {
    pub last_ok_secs_ago: Option<u64>,
    pub last_error: Option<String>,
}

fn status_cell() -> &'static Mutex<(Option<std::time::Instant>, Option<String>)> {
    static S: OnceLock<Mutex<(Option<std::time::Instant>, Option<String>)>> = OnceLock::new();
    S.get_or_init(|| Mutex::new((None, None)))
}

/// Записать итог попытки (чтобы страница показывала, работает ли прокси).
pub fn note(result: &std::io::Result<TcpStream>) {
    let mut g = status_cell().lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(_) => *g = (Some(std::time::Instant::now()), None),
        Err(e) => g.1 = Some(e.to_string()),
    }
}

pub fn status() -> Status {
    let g = status_cell().lock().unwrap_or_else(|e| e.into_inner());
    Status { last_ok_secs_ago: g.0.map(|t| t.elapsed().as_secs()), last_error: g.1.clone() }
}

/// Для проверок: подменить включённый прокси в памяти, не трогая файл настроек.
#[cfg(test)]
pub fn set_for_test(s: Option<Settings>) {
    *cell().lock().unwrap_or_else(|e| e.into_inner()) = s;
}

// ---------------------------------------------------------------- страница настроек (под проверкой входа владельца)

use axum::{extract::Json, http::StatusCode, response::{IntoResponse, Response}, routing::{get, post}, Router};
use serde_json::json;

/// Что видит страница: всё, кроме пароля.
fn view() -> serde_json::Value {
    let st = status();
    match current() {
        Some(s) => {
            let (auth, user, has_password) = match &s.auth {
                Auth::None => ("none", String::new(), false),
                Auth::Password { user, password } => ("password", user.clone(), !password.is_empty()),
            };
            json!({"configured": true, "enabled": s.enabled, "kind": s.kind, "host": s.host, "port": s.port, "auth": auth, "user": user, "has_password": has_password,
                   "last_ok_secs_ago": st.last_ok_secs_ago, "last_error": st.last_error})
        }
        None => json!({"configured": false, "enabled": false, "kind": "socks5", "host": "", "port": 1080, "auth": "none", "user": "", "has_password": false,
                       "last_ok_secs_ago": null, "last_error": null}),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Form {
    enabled: bool,
    kind: Kind,
    host: String,
    port: u16,
    /// "none" или "password"
    auth: String,
    #[serde(default)]
    user: String,
    /// пусто или не прислан — оставить прежний (для того же логина)
    #[serde(default)]
    password: String,
}

fn from_form(f: Form, old: Option<Settings>) -> Result<Settings, String> {
    let auth = match f.auth.as_str() {
        "none" => Auth::None,
        "password" => {
            let password = if !f.password.is_empty() {
                f.password
            } else {
                match old.as_ref().map(|o| &o.auth) {
                    Some(Auth::Password { user, password }) if *user == f.user => password.clone(),
                    _ => String::new(),
                }
            };
            Auth::Password { user: f.user, password }
        }
        _ => return Err("Тип авторизации: «нет» или «логин и пароль».".into()),
    };
    let s = Settings { enabled: f.enabled, kind: f.kind, host: f.host.trim().to_string(), port: f.port, auth };
    s.check()?;
    Ok(s)
}

async fn get_proxy() -> Json<serde_json::Value> {
    Json(view())
}

async fn post_proxy(Json(f): Json<Form>) -> Response {
    let s = match from_form(f, current()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e}))).into_response(),
    };
    match save(s) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": e}))).into_response(),
    }
}

/// Проверка: соединиться через сохранённые настройки (даже если прокси выключен) с общеизвестным публичным адресом и ничего не передавать.
async fn test_proxy() -> Json<serde_json::Value> {
    let Some(s) = current() else { return Json(json!({"ok": false, "error": "Прокси не настроен."})) };
    let t = std::time::Instant::now();
    match connect(&s, "1.1.1.1", 443, Duration::from_secs(10)).await {
        Ok(_) => Json(json!({"ok": true, "ms": t.elapsed().as_millis() as u64})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/upstream-proxy", get(get_proxy).post(post_proxy)).route("/api/upstream-proxy/test", post(test_proxy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn settings(kind: Kind, port: u16, auth: Auth) -> Settings {
        Settings { enabled: true, kind, host: "127.0.0.1".into(), port, auth }
    }

    /// Поддельный SOCKS5-прокси: записывает, что у него просили, и отвечает как велено.
    async fn fake_socks(want_auth: Option<(&'static str, &'static str)>, reply: u8) -> (u16, tokio::sync::oneshot::Receiver<(String, u16)>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut g = [0u8; 2];
            s.read_exact(&mut g).await.unwrap();
            let mut methods = vec![0u8; g[1] as usize];
            s.read_exact(&mut methods).await.unwrap();
            if let Some((u, p)) = want_auth {
                assert!(methods.contains(&2));
                s.write_all(&[5, 2]).await.unwrap();
                let mut h = [0u8; 2];
                s.read_exact(&mut h).await.unwrap();
                let mut user = vec![0u8; h[1] as usize];
                s.read_exact(&mut user).await.unwrap();
                let mut pl = [0u8; 1];
                s.read_exact(&mut pl).await.unwrap();
                let mut pass = vec![0u8; pl[0] as usize];
                s.read_exact(&mut pass).await.unwrap();
                let ok = user == u.as_bytes() && pass == p.as_bytes();
                s.write_all(&[1, if ok { 0 } else { 1 }]).await.unwrap();
                if !ok {
                    return;
                }
            } else {
                s.write_all(&[5, 0]).await.unwrap();
            }
            let mut h = [0u8; 4];
            s.read_exact(&mut h).await.unwrap();
            let host = match h[3] {
                3 => {
                    let mut l = [0u8; 1];
                    s.read_exact(&mut l).await.unwrap();
                    let mut d = vec![0u8; l[0] as usize];
                    s.read_exact(&mut d).await.unwrap();
                    String::from_utf8(d).unwrap()
                }
                1 => {
                    let mut a = [0u8; 4];
                    s.read_exact(&mut a).await.unwrap();
                    std::net::Ipv4Addr::from(a).to_string()
                }
                _ => panic!("unexpected atyp"),
            };
            let mut p = [0u8; 2];
            s.read_exact(&mut p).await.unwrap();
            let _ = tx.send((host, u16::from_be_bytes(p)));
            s.write_all(&[5, reply, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
            if reply == 0 {
                s.write_all(b"hello from target").await.unwrap();
            }
        });
        (port, rx)
    }

    #[tokio::test]
    async fn socks5_passes_the_name_to_the_proxy_and_the_data_flows() {
        let (port, seen) = fake_socks(None, 0).await;
        let mut st = connect(&settings(Kind::Socks5, port, Auth::None), "example.org", 443, Duration::from_secs(5)).await.unwrap();
        assert_eq!(seen.await.unwrap(), ("example.org".to_string(), 443), "the NAME reached the proxy: this computer did not resolve it");
        let mut b = vec![0u8; 17];
        st.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"hello from target");
    }

    #[tokio::test]
    async fn socks5_with_login_and_password_and_with_a_wrong_one() {
        let (port, seen) = fake_socks(Some(("anna", "s3cret")), 0).await;
        let a = Auth::Password { user: "anna".into(), password: "s3cret".into() };
        connect(&settings(Kind::Socks5, port, a), "93.184.216.34", 80, Duration::from_secs(5)).await.unwrap();
        assert_eq!(seen.await.unwrap(), ("93.184.216.34".to_string(), 80), "an IP is sent as an address, not as a name");
        let (port, _seen) = fake_socks(Some(("anna", "s3cret")), 0).await;
        let wrong = Auth::Password { user: "anna".into(), password: "nope".into() };
        let e = connect(&settings(Kind::Socks5, port, wrong), "example.org", 80, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("логин и пароль"), "{e}");
    }

    #[tokio::test]
    async fn socks5_refusals_are_told_apart_and_nothing_goes_around_the_proxy() {
        let (port, _s) = fake_socks(None, 2).await;
        let e = connect(&settings(Kind::Socks5, port, Auth::None), "example.org", 80, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("правила прокси"), "{e}");
        // nobody is listening at the proxy's address: an error, not a direct connection to the target
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = dead.local_addr().unwrap().port();
        drop(dead);
        let e = connect(&settings(Kind::Socks5, p, Auth::None), "example.org", 80, Duration::from_secs(3)).await.unwrap_err();
        assert!(e.to_string().contains("прокси недоступен"), "{e}");
    }

    async fn fake_http(status: &'static str) -> (u16, tokio::sync::oneshot::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                buf.push(b[0]);
            }
            let _ = tx.send(String::from_utf8(buf).unwrap());
            s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
            if status.starts_with("200") {
                s.write_all(b"tunnel open").await.unwrap();
            }
        });
        (port, rx)
    }

    #[tokio::test]
    async fn http_connect_sends_the_target_and_the_credentials_and_reads_only_the_headers() {
        let (port, req) = fake_http("200 Connection established").await;
        let a = Auth::Password { user: "anna".into(), password: "pw".into() };
        let mut st = connect(&settings(Kind::HttpConnect, port, a), "example.org", 443, Duration::from_secs(5)).await.unwrap();
        let text = req.await.unwrap();
        assert!(text.starts_with("CONNECT example.org:443 HTTP/1.1\r\n"), "{text}");
        assert!(text.contains("Proxy-Authorization: Basic YW5uYTpwdw==\r\n"), "{text}");
        let mut b = vec![0u8; 11];
        st.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"tunnel open", "the bytes after the headers belong to the target and were not eaten");
    }

    #[tokio::test]
    async fn http_connect_refusals() {
        let (port, _r) = fake_http("407 Proxy Authentication Required").await;
        let e = connect(&settings(Kind::HttpConnect, port, Auth::None), "example.org", 443, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("логин и пароль"), "{e}");
        let (port, _r) = fake_http("403 Forbidden").await;
        let e = connect(&settings(Kind::HttpConnect, port, Auth::None), "example.org", 443, Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("403"), "{e}");
    }

    #[tokio::test]
    async fn a_target_name_cannot_smuggle_headers_and_a_silent_proxy_times_out() {
        let e = connect(&settings(Kind::HttpConnect, 9, Auth::None), "evil.org\r\nX: y", 80, Duration::from_secs(2)).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        // a proxy that accepts and says nothing
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::task::spawn(async move {
            let (_s, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let e = connect(&settings(Kind::Socks5, port, Auth::None), "example.org", 80, Duration::from_millis(400)).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn what_the_owner_types_is_checked() {
        let ok = settings(Kind::Socks5, 1080, Auth::None);
        assert!(ok.check().is_ok());
        for bad in [
            Settings { host: "".into(), ..ok.clone() },
            Settings { host: "bad host".into(), ..ok.clone() },
            Settings { host: "a\r\nb".into(), ..ok.clone() },
            Settings { port: 0, ..ok.clone() },
            Settings { auth: Auth::Password { user: "a:b".into(), password: "x".into() }, ..ok.clone() },
            Settings { auth: Auth::Password { user: "".into(), password: "x".into() }, ..ok.clone() },
            Settings { auth: Auth::Password { user: "u".into(), password: "p".repeat(300) }, ..ok.clone() },
            Settings { auth: Auth::Password { user: "u".into(), password: "a\nb".into() }, ..ok.clone() },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        assert!(Settings { host: "::1".into(), ..ok.clone() }.check().is_ok());
        assert!(Settings { host: "proxy.example.com".into(), ..ok }.check().is_ok());
    }

    #[test]
    fn the_form_keeps_the_old_password_only_for_the_same_login_and_never_shows_it() {
        let old = Settings { enabled: true, kind: Kind::Socks5, host: "p.example.com".into(), port: 1080, auth: Auth::Password { user: "anna".into(), password: "secret".into() } };
        let f = |user: &str, password: &str| Form { enabled: true, kind: Kind::Socks5, host: " p.example.com ".into(), port: 1080, auth: "password".into(), user: user.into(), password: password.into() };
        let kept = from_form(f("anna", ""), Some(old.clone())).unwrap();
        assert_eq!(kept.auth, Auth::Password { user: "anna".into(), password: "secret".into() }, "an empty field keeps the saved password");
        assert_eq!(kept.host, "p.example.com", "spaces around the address are removed");
        // another login does not inherit the old password
        let other = from_form(f("bob", ""), Some(old.clone())).unwrap();
        assert_eq!(other.auth, Auth::Password { user: "bob".into(), password: String::new() });
        assert_eq!(from_form(f("anna", "new"), Some(old.clone())).unwrap().auth, Auth::Password { user: "anna".into(), password: "new".into() });
        assert!(from_form(Form { auth: "magic".into(), ..f("a", "b") }, None).is_err());
        assert!(from_form(Form { port: 0, ..f("a", "b") }, None).is_err());
    }
}
