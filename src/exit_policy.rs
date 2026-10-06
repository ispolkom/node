//! Правила выхода в интернет через этот узел (узел как «выход» для других узлов — HTTP-прокси, SOCKS5, туннель).
//!
//! * **Кто может выходить через этот узел** — решает владелец: никто, только доверенные узлы (по умолчанию) или все. Чужой трафик
//!   выходит в интернет с адреса владельца выхода, поэтому по умолчанию — только те, кому он доверяет (визитки, «Доверенные узлы»).
//! * **Куда нельзя** — на этот компьютер и в домашнюю/служебную сеть владельца выхода (роутер, базы данных, службы на этом компьютере, страницы узла…): иначе
//!   любой узел со связью мог бы через выход заглянуть в его домашнюю сеть. Проверяется найденный адрес, и соединение идёт ровно с
//!   проверенным адресом — подмена имени на «домашний» адрес между проверкой и соединением не проходит.
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::util::HashId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitMode {
    /// никто не выходит в интернет через этот узел
    Off,
    /// только доверенные узлы (по умолчанию)
    Trusted,
    /// любой узел со связью
    All,
}

fn settings_path() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(p) = TEST_PATH.with(|c| c.borrow().clone()) {
        return p;
    }
    crate::util::data_dir::data_dir().join("exit.json")
}

#[cfg(test)]
thread_local! {
    static TEST_PATH: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn mode_cell() -> &'static RwLock<Option<ExitMode>> {
    static M: RwLock<Option<ExitMode>> = RwLock::new(None);
    &M
}

/// Выбор владельца (читается из файла один раз и потом держится в памяти).
pub fn mode() -> ExitMode {
    if let Some(m) = *mode_cell().read().unwrap_or_else(|e| e.into_inner()) {
        return m;
    }
    let m = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| serde_json::from_value::<ExitMode>(v["mode"].clone()).ok())
        .unwrap_or(ExitMode::Trusted);
    *mode_cell().write().unwrap_or_else(|e| e.into_inner()) = Some(m);
    m
}

pub fn set_mode(m: ExitMode) -> std::io::Result<()> {
    let p = settings_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, json!({"mode": m}).to_string())?;
    *mode_cell().write().unwrap_or_else(|e| e.into_inner()) = Some(m);
    Ok(())
}

fn trusted_cell() -> &'static RwLock<HashSet<[u8; 32]>> {
    static T: OnceLock<RwLock<HashSet<[u8; 32]>>> = OnceLock::new();
    T.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Кому этот узел доверяет (ставится при запуске и при каждом изменении списка доверенных узлов).
pub fn set_trusted(ids: impl IntoIterator<Item = [u8; 32]>) {
    *trusted_cell().write().unwrap_or_else(|e| e.into_inner()) = ids.into_iter().collect();
}

pub fn is_trusted(peer: &HashId) -> bool {
    trusted_cell().read().unwrap_or_else(|e| e.into_inner()).contains(&peer.0)
}

/// Может ли узел `peer` выходить в интернет через этот узел.
/// «Все» — это все, кто сам помогает сети или не может помогать; тот, кто мог бы, но отказывается, после льготного объёма — нет
/// (`reciprocity`).
pub fn may_exit(peer: &HashId) -> bool {
    match mode() {
        ExitMode::Off => false,
        ExitMode::All => crate::reciprocity::verdict_for(&peer.0, is_trusted(peer)) == crate::reciprocity::Verdict::Allow,
        ExitMode::Trusted => is_trusted(peer),
    }
}

/// Выход по цепочке через несколько узлов: отправителя выход не знает, виден лишь предыдущий узел цепочки.
/// «Только доверенные» значит: цепочка пришла от доверенного узла; «все» — без условий (взаимность по клиенту тут невозможна).
pub fn may_exit_anonymous(prev_hop: &HashId) -> bool {
    match mode() {
        ExitMode::Off => false,
        ExitMode::All => true,
        ExitMode::Trusted => is_trusted(prev_hop),
    }
}

/// Отказ — одной строкой в журнал.
pub fn refuse_log(peer: &HashId, what: &str) {
    let why = match crate::reciprocity::verdict_for(&peer.0, is_trusted(peer)) {
        crate::reciprocity::Verdict::Deny(w) if mode() == ExitMode::All => w,
        _ => "",
    };
    eprintln!("[exit] ⛔ узел {} не может выходить в интернет через этот узел ({what}; правило: {:?}{}{})", hex::encode(&peer.0[..8]), mode(), if why.is_empty() { "" } else { "; " }, why);
}

/// Адрес в интернете (не этот компьютер, не домашняя или служебная сеть).
pub fn public_destination(ip: &IpAddr) -> bool {
    public_only(ip)
}

/// Разобрать `хост:порт` (IPv6 — в квадратных скобках).
fn split_host_port(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    (!host.is_empty() && port > 0).then_some((host, port))
}

/// Найти адреса цели и оставить только адреса в интернете. Ни одного — отказ (`PermissionDenied`).
pub async fn resolve_public(target: &str) -> std::io::Result<Vec<SocketAddr>> {
    let (host, port) = split_host_port(target).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("неверная цель {target}")))?;
    let all: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await?.collect();
    let ok: Vec<SocketAddr> = all.iter().copied().filter(|a| public_destination(&a.ip())).collect();
    if ok.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{target}: адрес этого компьютера или домашней сети — выход туда не пускает")));
    }
    Ok(ok)
}

/// Соединиться с целью в интернете — ровно с проверенным адресом (первым, что ответит), не дольше `limit`.
pub async fn connect_public(target: &str, limit: Duration) -> std::io::Result<tokio::net::TcpStream> {
    let addrs = resolve_public(target).await?;
    let mut last = None;
    for a in addrs {
        match tokio::time::timeout(limit, tokio::net::TcpStream::connect(a)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => last = Some(e),
            Err(_) => last = Some(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no address")))
}

/// Пароль локального прокси этого узла: создаётся один раз (20 случайных знаков), лежит в папке данных узла с правами 0600.
pub fn local_proxy_password() -> std::io::Result<String> {
    let path = crate::util::data_dir::data_dir().join("local_proxy_password");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if s.len() >= 16 {
            return Ok(s);
        }
    }
    use rand::Rng;
    const A: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut r = rand::thread_rng();
    let pw: String = (0..20).map(|_| A[r.gen_range(0..A.len())] as char).collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::util::private_file::write_private(&path, pw.as_bytes())?;
    Ok(pw)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    mode: ExitMode,
}

async fn get_settings() -> Json<serde_json::Value> {
    Json(json!({"mode": mode(), "trusted": trusted_cell().read().unwrap_or_else(|e| e.into_inner()).len(), "warning": crate::reciprocity::warning_for_owner()}))
}

async fn post_settings(Json(b): Json<Body>) -> Response {
    match set_mode(b.mode) {
        Ok(()) => Json(json!({"ok": true, "mode": b.mode})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("Не удалось сохранить: {e}")}))).into_response(),
    }
}

/// Путь страницы настроек (под проверкой входа владельца).
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/exit/settings", get(get_settings).post(post_settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_split_with_ipv6_brackets() {
        assert_eq!(split_host_port("example.org:443"), Some(("example.org".into(), 443)));
        assert_eq!(split_host_port("[2001:db8::1]:80"), Some(("2001:db8::1".into(), 80)));
        assert_eq!(split_host_port("host:0"), None);
        assert_eq!(split_host_port("nohost"), None);
        assert_eq!(split_host_port(":80"), None);
    }

    #[tokio::test]
    async fn the_exit_never_connects_to_this_computer_or_the_home_network() {
        for t in ["127.0.0.1:22", "localhost:3306", "[::1]:8080", "192.168.1.1:80", "10.0.0.5:443", "172.16.3.4:80", "169.254.169.254:80", "100.64.0.1:80", "0.0.0.0:80", "[fe80::1]:80", "[fc00::5]:80", "[::ffff:127.0.0.1]:80"] {
            let e = resolve_public(t).await.unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{t}");
            assert!(connect_public(t, Duration::from_secs(1)).await.is_err(), "{t}");
        }
        assert!(resolve_public("8.8.8.8:53").await.is_ok());
        assert!(resolve_public("[2001:4860:4860::8888]:53").await.is_ok());
    }

    #[test]
    fn who_may_exit_follows_the_owners_choice() {
        let dir = tempfile::tempdir().unwrap();
        TEST_PATH.with(|c| *c.borrow_mut() = Some(dir.path().join("exit.json")));
        *mode_cell().write().unwrap() = None;
        let (friend, stranger) = (HashId([1; 32]), HashId([2; 32]));
        set_trusted([[1u8; 32]]);
        assert_eq!(mode(), ExitMode::Trusted, "default: only trusted nodes");
        assert!(may_exit(&friend) && !may_exit(&stranger));
        set_mode(ExitMode::Off).unwrap();
        assert!(!may_exit(&friend));
        set_mode(ExitMode::All).unwrap();
        assert!(may_exit(&stranger));
        *mode_cell().write().unwrap() = None;
        assert_eq!(mode(), ExitMode::All, "the choice survives a restart");
        set_trusted([]);
        set_mode(ExitMode::Trusted).unwrap();
        assert!(!may_exit(&friend), "a node removed from the trusted list can no longer exit");
    }
}

/// Публичный ли адрес (не этот компьютер, не домашняя/служебная сеть).
pub fn public_only(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.is_documentation() || v.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (18..=19).contains(&o[1]))
                || o[0] >= 240)
        }
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return public_only(&IpAddr::V4(m));
            }
            let s = v.segments();
            !(v.is_loopback() || v.is_unspecified() || v.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || (s[0] == 0x2001 && s[1] == 0x0db8) || (s[0] == 0x64 && s[1] == 0xff9b))
        }
    }
}
