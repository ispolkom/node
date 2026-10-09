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
    connect_public_for(crate::upstream_proxy::SHARED_CLIENT, target, limit).await
}

/// То же от имени клиента (устройства, соседнего узла): пока включены внешние прокси, за клиентом закрепляется один прокси, и все его
/// соединения выходят с одного внешнего адреса.
pub async fn connect_public_for(client: &str, target: &str, limit: Duration) -> std::io::Result<tokio::net::TcpStream> {
    // Пока включён внешний прокси, весь выход идёт ТОЛЬКО через него; прямого соединения не будет, даже если прокси не отвечает.
    if crate::upstream_proxy::is_active() {
        return connect_through_proxy(client, target, limit).await;
    }
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

/// Имена, на которые отвечает чья-то внутренняя сеть, а не интернет: через прокси к ним не ходим (прокси разрешает имена у себя, и проверить
/// адрес здесь нельзя, поэтому отсекаем по виду имени).
fn local_name(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    !h.contains('.') || h == "localhost" || [".localhost", ".local", ".internal", ".lan", ".home", ".corp", ".intranet", ".home.arpa"].iter().any(|s| h.ends_with(s))
}

/// Выход через внешний прокси. Прокси сам находит имя цели; адреса вида «этот компьютер» и «домашняя сеть» отсекаются здесь, как и без прокси.
async fn connect_through_proxy(client: &str, target: &str, limit: Duration) -> std::io::Result<tokio::net::TcpStream> {
    let (host, port) = split_host_port(target).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("неверная цель {target}")))?;
    let refuse = |what: &str| std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{target}: {what} — выход туда не пускает"));
    match host.parse::<IpAddr>() {
        Ok(ip) if !public_destination(&ip) => return Err(refuse("адрес этого компьютера или домашней сети")),
        Ok(_) => {}
        Err(_) if local_name(&host) => return Err(refuse("имя внутренней сети")),
        Err(_) => {}
    }
    crate::upstream_proxy::connect_for(client, &host, port, limit).await
}

/// Пароль локального прокси этого узла: создаётся один раз (20 случайных знаков), лежит в папке данных узла с правами 0600.
pub fn local_proxy_password() -> std::io::Result<String> {
    let path = crate::util::data_dir::data_dir().join("local_proxy_password");
    let bytes = crate::util::private_file::read_or_create_private(
        &path,
        &|| {
            use rand::Rng;
            const A: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
            let mut r = rand::thread_rng();
            (0..20).map(|_| A[r.gen_range(0..A.len())]).collect()
        },
        &|b| std::str::from_utf8(b).map_or(false, |s| s.trim().len() >= 16),
    )?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
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
    fn ipv6_transition_and_reserved_ranges_are_never_public() {
        for bad in ["::127.0.0.1", "::10.0.0.1", "::1", "::", "fc00::1", "fd12::1", "fe80::1", "fec0::1", "ff02::1", "2002:7f00:1::1", "2002:c0a8:1::1",
                    "2001:0:4136:e378:8000:63bf:3fff:fdd2", "2001:db8::1", "64:ff9b::7f00:1", "::ffff:127.0.0.1", "::ffff:192.168.1.1", "3fff::1", "100::1"] {
            let ip: std::net::IpAddr = bad.parse().unwrap();
            assert!(!public_only(&ip), "{bad}");
        }
        for good in ["2606:4700:4700::1111", "2a00:1450:4001:81b::200e", "2001:4860:4860::8888", "::ffff:8.8.8.8"] {
            let ip: std::net::IpAddr = good.parse().unwrap();
            assert!(public_only(&ip), "{good}");
        }
    }

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
    fn names_of_an_internal_network_are_not_sent_to_a_proxy() {
        for n in ["localhost", "printer", "nas.local", "router.lan", "db.internal", "x.home.arpa", "A.LOCALHOST", "intranet"] {
            assert!(local_name(n), "{n}");
        }
        for n in ["example.org", "sub.example.co.uk", "xn--80ak6aa92e.com"] {
            assert!(!local_name(n), "{n}");
        }
    }

    #[tokio::test]
    async fn with_a_proxy_on_nothing_goes_around_it_and_the_home_network_stays_closed() {
        // a proxy address where nobody listens: every public target fails, none is reached directly
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = dead.local_addr().unwrap().port();
        drop(dead);
        crate::upstream_proxy::set_for_test(Some(crate::upstream_proxy::Settings { enabled: true, kind: crate::upstream_proxy::Kind::Socks5, host: "127.0.0.1".into(), port, auth: crate::upstream_proxy::Auth::None }));
        let e = connect_public("8.8.8.8:53", Duration::from_secs(2)).await.unwrap_err();
        assert!(e.to_string().contains("прокси"), "failed because of the proxy, not connected directly: {e}");
        for t in ["127.0.0.1:22", "192.168.1.1:80", "[::1]:80", "localhost:80", "nas.local:80", "printer:80"] {
            assert_eq!(connect_public(t, Duration::from_secs(2)).await.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied, "{t}");
        }
        crate::upstream_proxy::set_for_test(None);
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
            // Only global unicast 2000::/3 can be public; inside it the transition and documentation ranges are not
            // (6to4 2002::/16 and Teredo 2001:0::/32 carry an IPv4 address inside, 2001:db8::/32 and 3fff::/20 are documentation,
            // 2001:10::/28 and 2001:20::/28 are ORCHID, 64:ff9b::/32 is NAT64 and lies outside 2000::/3 anyway).
            // Everything else — ::/8 (incl. IPv4-compatible ::a.b.c.d), fc00::/7, fe80::/10, fec0::/10, ff00::/8 — is refused.
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] == 0 || s[1] == 0x0db8 || (s[1] & 0xfff0) == 0x0010 || (s[1] & 0xfff0) == 0x0020))
                && (s[0] & 0xfff0) != 0x3ff0
                && !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
        }
    }
}
