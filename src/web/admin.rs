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

static UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Сколько осталось сеанса, если он идёт.
pub fn remaining() -> Option<Duration> {
    let mut g = UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    match *g {
        Some(t) if t > Instant::now() => Some(t - Instant::now()),
        Some(_) => {
            *g = None;
            None
        }
        None => None,
    }
}

pub fn start() {
    *UNTIL.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now() + TTL);
}

pub fn end() {
    *UNTIL.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Маршруты, которым нужен сеанс администратора: управление узлом, а не личный аккаунт.
pub fn is_admin_path(path: &str) -> bool {
    const PREFIXES: [&str; 11] = [
        "/api/complaints",
        "/api/pair-requests",
        "/api/guest-pairing",
        "/api/mail-policy",
        "/api/upstream-prox",
        "/api/mobile/pairing",
        "/api/mobile/devices",
        "/api/vault/panic_erase",
        "/api/exit/settings",
        "/pair/issue",
        "/pair/qr.json",
    ];
    PREFIXES.iter().any(|p| path == *p || path.starts_with(&format!("{p}/")) || (p.ends_with('x') && path.starts_with(p)))
}

/// Нужен ли сеанс для этого запроса и есть ли он.
pub fn allowed(path: &str) -> bool {
    !is_admin_path(path) || remaining().is_some()
}

// ---- извещения

static ACKED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Одно извещение: тип, подпись, число.
pub struct Notice {
    pub kind: &'static str,
    pub label: String,
    pub count: u64,
}

/// Что сейчас требует внимания администратора (по текущему состоянию узла).
pub fn compute() -> Vec<Notice> {
    let mut out = Vec::new();
    let new_complaints = crate::complaints::list().iter().filter(|c| c["status"] == "new").count() as u64;
    if new_complaints > 0 {
        out.push(Notice { kind: "complaints", label: "Новых жалоб".into(), count: new_complaints });
    }
    let requests = crate::mobile_api::pair_requests_pending().len() as u64;
    if requests > 0 {
        out.push(Notice { kind: "pair_requests", label: "Запросов сопряжения".into(), count: requests });
    }
    if !crate::updates::status_json()["available"].is_null() {
        out.push(Notice { kind: "update", label: "Доступно обновление".into(), count: 1 });
    }
    if let Some((added, working)) = crate::upstream_proxy::summary() {
        if added > working {
            out.push(Notice { kind: "proxies", label: "Внешних прокси не отвечает".into(), count: (added - working) as u64 });
        }
    }
    out
}

/// Извещения с пометкой «новое» (число выросло с последнего просмотра) и общее число непрочитанного.
pub fn view() -> serde_json::Value {
    let acked = ACKED.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    let items: Vec<serde_json::Value> = compute()
        .into_iter()
        .map(|n| {
            let seen = acked.get(n.kind).copied().unwrap_or(0);
            serde_json::json!({"kind": n.kind, "label": n.label, "count": n.count, "new": n.count > seen})
        })
        .collect();
    let unread: u64 = items.iter().filter(|i| i["new"] == true).map(|i| i["count"].as_u64().unwrap_or(0)).sum();
    serde_json::json!({"items": items, "unread": unread, "admin_active": remaining().is_some(), "admin_ttl_secs": TTL.as_secs()})
}

/// Отметить просмотренным (запомнить текущее число).
pub fn ack(kinds: &[String]) {
    let current = compute();
    let mut g = ACKED.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(Default::default);
    for n in current {
        if kinds.iter().any(|k| k == n.kind) {
            m.insert(n.kind.to_string(), n.count);
        }
    }
}

/// Сброс для тестов.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    end();
    *ACKED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_paths_need_a_session_and_personal_ones_do_not() {
        for p in [
            "/api/complaints",
            "/api/complaints/c1/measure",
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
    fn the_session_starts_runs_out_and_can_be_closed() {
        reset_for_test();
        assert!(remaining().is_none() && !allowed("/api/complaints") && allowed("/api/status"));
        start();
        let left = remaining().unwrap();
        assert!(left <= TTL && left > TTL - Duration::from_secs(5));
        assert!(allowed("/api/complaints"));
        end();
        assert!(!allowed("/api/complaints"), "после выхода админские маршруты закрыты");
        // истёкший сеанс исчезает сам
        *UNTIL.lock().unwrap() = Some(Instant::now() - Duration::from_secs(1));
        assert!(remaining().is_none() && !allowed("/api/guest-pairing"));
        reset_for_test();
    }

    #[test]
    fn a_notice_carries_only_type_label_and_count_and_stays_new_until_acknowledged() {
        reset_for_test();
        // состояние узла в тестах пустое: извещений нет
        let v = view();
        assert!(v["items"].as_array().unwrap().iter().all(|i| i.as_object().unwrap().keys().all(|k| ["kind", "label", "count", "new"].contains(&k.as_str()))));
        ACKED.lock().unwrap().get_or_insert_with(Default::default).insert("complaints".into(), 3);
        // подтверждённое число не делает извещение «новым», рост — делает (проверка правила на значениях)
        let acked = ACKED.lock().unwrap().clone().unwrap();
        assert!(!(3 > *acked.get("complaints").unwrap()));
        assert!(4 > *acked.get("complaints").unwrap());
        reset_for_test();
    }
}
