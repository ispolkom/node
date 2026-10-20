//! Сеанс администратора и извещения для личного аккаунта (docs/ROLES.md).
//!
//! * Админ-панель открывается админ-паролем на [`TTL`]; пока сеанса нет, админские маршруты отвечают 403 `admin_required` — проверка на сервере,
//!   а не только в интерфейсе. Владелец на узле один, поэтому сеанс общий для процесса (локальный вход по сокету не имеет cookie-сеансов).
//! * Извещения считаются по текущему состоянию узла (ничего не хранится): тип, подпись, число — без текста жалоб, вложений, кодов и токенов.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Сколько длится сеанс администратора после ввода пароля.
pub const TTL: Duration = Duration::from_secs(15 * 60);

/// Сеанс администратора принадлежит одному клиенту: при входе выдаётся случайный токен, на узле хранится только его хэш и срок. Любой другой
/// клиент (другая cookie, другой процесс того же пользователя) токена не знает и админского доступа не получает (P14-3).
static SESSION: Mutex<Option<(Instant, [u8; 32])>> = Mutex::new(None);

/// Заголовок, которым клиент предъявляет токен сеанса.
pub const HEADER: &str = "x-yandi-admin";

fn digest(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest([b"yandi-admin-session:".as_slice(), token.as_bytes()].concat()).into()
}

/// Сколько осталось сеанса у предъявившего этот токен (None — нет токена, он чужой или срок вышел).
pub fn remaining_for(token: Option<&str>) -> Option<Duration> {
    let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    match *g {
        Some((t, _)) if t <= Instant::now() => {
            *g = None;
            None
        }
        Some((t, h)) => {
            let given = digest(token?);
            // сравнение хэшей фиксированной длины без раннего выхода
            let same = given.iter().zip(h.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
            same.then(|| t - Instant::now())
        }
        None => None,
    }
}

/// Начать сеанс: выдать новый токен (прежний перестаёт действовать).
pub fn start() -> String {
    use rand::RngCore;
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = hex::encode(raw);
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now() + TTL, digest(&token)));
    token
}

pub fn end() {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ---- админ-пароль (этап 2)
//
// Отдельный пароль администратора хранится рядом с auth.json (`admin_auth.json`, Argon2id, права 0600). Файла нет — админ-пароль совпадает с
// паролем входа (так было до этапа 2). Файл есть, но не читается или повреждён, — отказ, а не откат на пароль входа: иначе порча файла
// молча подменяла бы более строгий пароль более слабым. Рабочие данные узла этим паролем не шифруются: он только открывает админ-панель.

fn password_file() -> std::path::PathBuf {
    crate::web::auth::auth_file_path().with_file_name("admin_auth.json")
}

/// Есть ли у администратора отдельный пароль.
pub fn has_separate() -> bool {
    std::fs::symlink_metadata(password_file()).is_ok()
}

fn verify_in(file: &std::path::Path, password: &str, login: impl Fn(&str) -> Result<bool, String>) -> Result<bool, String> {
    match std::fs::symlink_metadata(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => login(password),
        Err(_) => Err("Файл админ-пароля недоступен".to_string()),
        Ok(_) => {
            let json = std::fs::read_to_string(file).map_err(|_| "Файл админ-пароля недоступен".to_string())?;
            let v: serde_json::Value = serde_json::from_str(&json).map_err(|_| "Файл админ-пароля повреждён".to_string())?;
            let hash = v.get("hash").and_then(|h| h.as_str()).ok_or("Файл админ-пароля повреждён")?;
            Ok(crate::web::auth::verify_login_password(password, hash))
        }
    }
}

/// Проверить админ-пароль (без счёта попыток; счёт — в `auth::verify_admin_counted`).
pub fn verify_admin(password: &str) -> Result<bool, String> {
    verify_in(&password_file(), password, crate::web::auth::verify_login)
}

fn set_in(file: &std::path::Path, new: &str, login: impl Fn(&str) -> Result<bool, String>) -> Result<(), String> {
    crate::web::auth::check_new_password(new)?;
    if login(new).unwrap_or(false) {
        return Err("Пароль администратора должен отличаться от пароля входа".to_string());
    }
    let hash = crate::web::auth::hash_login_password(new)?;
    let json = serde_json::json!({"version": 1, "hash": hash}).to_string();
    crate::util::private_file::write_private(file, json.as_bytes()).map_err(|_| "Не удалось записать админ-пароль".to_string())
}

/// Назначить отдельный админ-пароль (проверка длины, отличие от пароля входа, атомарная запись).
pub fn set_password(new: &str) -> Result<(), String> {
    set_in(&password_file(), new, crate::web::auth::verify_login)
}

/// Вернуть общий пароль (админ-пароль = пароль входа): после восстановления по сид-фразе.
pub fn clear_password() -> Result<(), String> {
    let f = password_file();
    match std::fs::symlink_metadata(&f) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Файл админ-пароля недоступен".to_string()),
        Ok(m) => {
            let r = if m.is_dir() { std::fs::remove_dir_all(&f) } else { std::fs::remove_file(&f) };
            r.map_err(|_| "Файл админ-пароля не удалён".to_string())
        }
    }
}

/// Маршруты, которым нужен сеанс администратора: управление узлом, а не личный аккаунт.
pub fn is_admin_path(path: &str) -> bool {
    const PREFIXES: [&str; 14] = [
        "/api/admin/password",
        "/api/admin/bootstrap",
        "/api/complaints",
        "/api/pair-requests",
        "/api/guest-pairing",
        "/api/mail-policy",
        "/api/upstream-prox",
        "/api/mobile/pairing",
        "/api/mobile/devices",
        "/api/mobile/invite-policy",
        "/api/vault/panic_erase",
        "/api/exit/settings",
        "/pair/issue",
        "/pair/qr.json",
    ];
    PREFIXES.iter().any(|p| path == *p || path.starts_with(&format!("{p}/")) || (p.ends_with('x') && path.starts_with(p)))
}

/// Админский ли запрос с учётом метода: часть маршрутов общая (чтение личное, запись управленческая). P14-1.
pub fn is_admin_route(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    if is_admin_path(path) {
        return true;
    }
    let reads = method == Method::GET || method == Method::HEAD || method == Method::OPTIONS;
    let p = path.trim_end_matches('/');
    // группы всего узла и их удаление — управление узлом, а не группы самого человека (те живут на /api/groups)
    if p == "/api/mobile/groups" || p.starts_with("/api/mobile/groups/") {
        return true;
    }
    match p {
        // настройки узла (адрес привязки, порты, журнал), ретрансляция, запуск и остановка, копии
        "/api/settings" => !reads,
        "/api/network/relay" | "/api/node/start" | "/api/node/stop" | "/api/backup/export" | "/api/backup/config" => !reads,
        // общий секрет сопряжения и QR для сопряжения выдаются только администратору
        "/api/devices/pairing" | "/pair/qr" => true,
        _ => false,
    }
}

/// Нужен ли сеанс для этого запроса и есть ли он.
pub fn allowed(method: &axum::http::Method, path: &str, token: Option<&str>) -> bool {
    !is_admin_route(method, path) || remaining_for(token).is_some()
}

// ---- извещения

/// Что администратор уже видел: по типу — безопасные отпечатки событий (хэш идентификатора, обрезанный), а не сами события. Хранится на диске,
/// чтобы перезапуск узла не превращал прочитанное в новое.
static ACKED: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);

fn ack_file() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("notice_ack.json")
}

fn acked_load() -> HashMap<String, Vec<String>> {
    let mut g = ACKED.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(std::fs::read(ack_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
    }
    g.clone().unwrap_or_default()
}

fn fingerprint(id: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest([b"yandi-notice:".as_slice(), id.as_bytes()].concat())[..8])
}

/// Одно извещение: тип, подпись, число (идентификаторы событий наружу не выходят).
pub struct Notice {
    pub kind: &'static str,
    pub label: String,
    pub count: u64,
    ids: Vec<String>,
}

fn notice(kind: &'static str, label: &str, ids: Vec<String>) -> Notice {
    Notice { kind, label: label.to_string(), count: ids.len() as u64, ids: ids.iter().map(|i| fingerprint(&format!("{kind}:{i}"))).collect() }
}

/// Что сейчас требует внимания администратора (по текущему состоянию узла).
pub fn compute() -> Vec<Notice> {
    let mut out = Vec::new();
    let complaints: Vec<String> = crate::complaints::list().iter().filter(|c| c["status"] == "new").map(|c| c["id"].to_string()).collect();
    if !complaints.is_empty() {
        out.push(notice("complaints", "Новых жалоб", complaints));
    }
    let requests: Vec<String> = crate::mobile_api::pair_requests_pending().iter().map(|r| r["id"].to_string()).collect();
    if !requests.is_empty() {
        out.push(notice("pair_requests", "Запросов сопряжения", requests));
    }
    let st = crate::updates::status_json();
    if !st["available"].is_null() {
        let v = st["available"]["version"].as_str().unwrap_or("?").to_string();
        out.push(notice("update", "Доступно обновление", vec![v]));
    }
    if let Some((added, working)) = crate::upstream_proxy::summary() {
        if added > working {
            // у состояния «не отвечает» нет собственных событий: новым считается рост числа
            out.push(notice("proxies", "Внешних прокси не отвечает", (1..=(added - working)).map(|i| i.to_string()).collect()));
        }
    }
    out
}

/// Извещения с пометкой «новое» (есть событие, которого администратор ещё не видел) и число непрочитанного.
pub fn view(token: Option<&str>) -> serde_json::Value {
    let acked = acked_load();
    let mut unread = 0u64;
    let items: Vec<serde_json::Value> = compute()
        .into_iter()
        .map(|n| {
            let seen = acked.get(n.kind).cloned().unwrap_or_default();
            let fresh = n.ids.iter().filter(|i| !seen.contains(i)).count() as u64;
            unread += fresh;
            serde_json::json!({"kind": n.kind, "label": n.label, "count": n.count, "new": fresh > 0})
        })
        .collect();
    serde_json::json!({"items": items, "unread": unread, "admin_active": remaining_for(token).is_some(), "admin_ttl_secs": TTL.as_secs()})
}

/// Отметить просмотренным именно те события, что показаны сейчас; события, которых уже нет, из памяти уходят.
pub fn ack(kinds: &[String]) {
    let current = compute();
    let mut map = acked_load();
    for n in current {
        if kinds.iter().any(|k| k == n.kind) {
            map.insert(n.kind.to_string(), n.ids.clone());
        }
    }
    // типы без событий забываются: после их исчезновения новое событие снова будет «новым»
    map.retain(|k, _| compute().iter().any(|n| n.kind == k.as_str()));
    if let Ok(json) = serde_json::to_vec(&map) {
        let _ = crate::util::private_file::write_private(&ack_file(), &json);
    }
    *ACKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(map);
}

/// Сброс для тестов.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    end();
    *ACKED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let _ = std::fs::remove_file(ack_file());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("yandi-admin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("admin_auth.json")
    }

    #[test]
    fn signing_the_bootstrap_list_and_reading_its_key_need_an_administrator_session() {
        use axum::http::Method;
        assert!(is_admin_route(&Method::GET, "/api/admin/bootstrap/pubkey"));
        assert!(is_admin_route(&Method::POST, "/api/admin/bootstrap/sign"));
        assert!(is_admin_route(&Method::GET, "/api/admin/bootstrap/applications"));
        assert!(is_admin_route(&Method::POST, "/api/admin/bootstrap/applications/dismiss"));
    }

    #[test]
    fn without_a_file_the_admin_password_is_the_login_password() {
        let f = tmp("none");
        let login = |p: &str| Ok(p == "login-pass-1");
        assert_eq!(verify_in(&f, "login-pass-1", login), Ok(true));
        assert_eq!(verify_in(&f, "other", login), Ok(false));
    }

    #[test]
    fn a_separate_password_replaces_the_login_one_for_the_panel() {
        let f = tmp("sep");
        let login = |p: &str| Ok(p == "login-pass-1");
        set_in(&f, "admin-pass-2", login).unwrap();
        assert_eq!(verify_in(&f, "admin-pass-2", login), Ok(true));
        assert_eq!(verify_in(&f, "login-pass-1", login), Ok(false), "пароль входа админ-панель больше не открывает");
    }

    #[test]
    fn the_admin_password_must_differ_from_login_and_be_long_enough() {
        let f = tmp("rules");
        let login = |p: &str| Ok(p == "login-pass-1");
        assert!(set_in(&f, "login-pass-1", login).is_err());
        assert!(set_in(&f, "abc", login).is_err());
        assert!(!f.exists(), "отказ ничего не записывает");
    }

    #[test]
    fn a_broken_file_refuses_instead_of_falling_back_to_the_login_password() {
        let f = tmp("broken");
        std::fs::write(&f, b"{not json").unwrap();
        let login = |_: &str| Ok(true);
        assert!(verify_in(&f, "login-pass-1", login).is_err());
        std::fs::write(&f, br#"{"version":1}"#).unwrap();
        assert!(verify_in(&f, "login-pass-1", login).is_err());
    }

    #[test]
    fn admin_paths_need_a_session_and_personal_ones_do_not() {
        for p in [
            "/api/complaints",
            "/api/complaints/c1/measure",
            "/api/admin/password",
            "/api/pair-requests/p1",
            "/api/guest-pairing",
            "/api/mail-policy",
            "/api/upstream-proxies",
            "/api/upstream-proxies/p1/test",
            "/api/upstream-proxy/test",
            "/api/mobile/pairing",
            "/api/mobile/devices/d1",
            "/api/vault/panic_erase",
            "/api/exit/settings",
            "/pair/issue",
        ] {
            assert!(is_admin_path(p), "{p}");
        }
        for p in ["/api/status", "/api/chats", "/api/chat/send/x", "/api/mobile/groups", "/api/vault/status", "/api/vault/unlock", "/api/notices", "/api/admin/session", "/api/contacts", "/api/complaintsx"] {
            assert!(!is_admin_path(p), "{p}");
        }
    }

    #[test]
    fn the_session_belongs_to_the_client_that_opened_it() {
        use axum::http::Method;
        reset_for_test();
        let g = Method::GET;
        assert!(remaining_for(None).is_none() && !allowed(&g, "/api/complaints", None) && allowed(&g, "/api/status", None));
        let mine = start();
        assert!(remaining_for(Some(&mine)).is_some());
        assert!(allowed(&g, "/api/complaints", Some(&mine)));
        // другой клиент без токена или с чужим не получает доступа, пока сеанс открыт
        assert!(!allowed(&g, "/api/complaints", None), "клиент без токена");
        assert!(!allowed(&g, "/api/complaints", Some("00")), "чужой токен");
        // новый вход отзывает прежний токен
        let next = start();
        assert!(remaining_for(Some(&mine)).is_none() && remaining_for(Some(&next)).is_some());
        end();
        assert!(!allowed(&g, "/api/complaints", Some(&next)), "после выхода админские маршруты закрыты");
        // истёкший сеанс исчезает сам
        let t = start();
        SESSION.lock().unwrap().as_mut().unwrap().0 = Instant::now() - Duration::from_secs(1);
        assert!(remaining_for(Some(&t)).is_none() && !allowed(&g, "/api/guest-pairing", Some(&t)));
        reset_for_test();
    }

    #[test]
    fn shared_routes_are_admin_only_for_writes_and_secret_reads() {
        use axum::http::Method;
        for (m, p) in [
            (Method::PUT, "/api/settings"),
            (Method::POST, "/api/network/relay"),
            (Method::POST, "/api/node/stop"),
            (Method::POST, "/api/node/start"),
            (Method::POST, "/api/backup/export"),
            (Method::POST, "/api/backup/config"),
            (Method::GET, "/api/devices/pairing"),
            (Method::GET, "/pair/qr"),
            (Method::GET, "/pair/qr/"),
            (Method::GET, "/api/mobile/groups"),
            (Method::POST, "/api/mobile/groups/abc/delete"),
            (Method::POST, "/api/admin/password"),
        ] {
            assert!(is_admin_route(&m, p), "{m} {p}");
        }
        for (m, p) in [
            (Method::GET, "/api/settings"),
            (Method::GET, "/api/network/relay"),
            (Method::GET, "/api/backup/status"),
            (Method::GET, "/api/groups"),
            (Method::POST, "/api/groups/create"),
            (Method::GET, "/api/status"),
        ] {
            assert!(!is_admin_route(&m, p), "{m} {p}");
        }
    }

    #[test]
    fn a_notice_shows_only_type_label_and_count_and_new_means_an_unseen_event() {
        reset_for_test();
        let v = view(None);
        assert!(v["items"].as_array().unwrap().iter().all(|i| i.as_object().unwrap().keys().all(|k| ["kind", "label", "count", "new"].contains(&k.as_str()))));
        // события одного типа различаются отпечатками: тот же счёт, но другое событие — снова «новое»
        let a = notice("complaints", "x", vec!["1".into(), "2".into()]);
        let b = notice("complaints", "x", vec!["3".into(), "4".into()]);
        assert_eq!(a.count, b.count);
        assert!(a.ids.iter().all(|i| !b.ids.contains(i)));
        assert!(a.ids.iter().all(|i| i.len() == 16), "наружу и на диск идёт не идентификатор, а отпечаток");
        reset_for_test();
    }
}
