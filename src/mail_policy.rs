//! Срок хранения почты между устройствами на узле (шифртекст «от кого и кому»): настраивается владельцем узла, по умолчанию 7 суток
//! (раньше было 30). Короче срок — меньше следов у того, кто завладел узлом; длиннее — дольше ждёт телефон, который долго не выходит на связь.
//! У гостей срок всегда сутки (docs/GUEST.md) и от этой настройки не зависит.
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_DAYS: u32 = 7;
pub const MIN_DAYS: u32 = 1;
pub const MAX_DAYS: u32 = 30;

fn path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("mail_policy.json")
}

fn read_days() -> u32 {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["days"].as_u64())
        .map(|d| (d as u32).clamp(MIN_DAYS, MAX_DAYS))
        .unwrap_or(DEFAULT_DAYS)
}

static CACHE: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

/// Срок в сутках (читается с диска не чаще раза в полминуты).
pub fn days() -> u32 {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, d)) = *c {
        if at.elapsed() < Duration::from_secs(30) {
            return d;
        }
    }
    let d = read_days();
    *c = Some((Instant::now(), d));
    d
}

pub fn ttl_ms() -> u64 {
    days() as u64 * 24 * 3600 * 1000
}

pub fn set_days(days: u32) -> Result<(), String> {
    if !(MIN_DAYS..=MAX_DAYS).contains(&days) {
        return Err(format!("Срок хранения почты: от {MIN_DAYS} до {MAX_DAYS} суток"));
    }
    let s = serde_json::json!({ "days": days }).to_string();
    crate::util::private_file::write_private(&path(), s.as_bytes()).map_err(|e| format!("не удалось сохранить: {e}"))?;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), days));
    Ok(())
}

use axum::{extract::Json, http::StatusCode, response::{IntoResponse, Response}, routing::get, Router};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    days: u32,
}

async fn get_policy() -> Json<serde_json::Value> {
    Json(serde_json::json!({"days": days(), "min": MIN_DAYS, "max": MAX_DAYS, "default": DEFAULT_DAYS, "guest_hours": 24}))
}

async fn post_policy(Json(b): Json<Body>) -> Response {
    match set_days(b.days) {
        Ok(()) => Json(serde_json::json!({"ok": true, "days": b.days})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": e}))).into_response(),
    }
}

/// Настройка для оболочки ПК (под проверкой входа владельца).
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/mail-policy", get(get_policy).post(post_policy))
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    let _ = std::fs::remove_file(path());
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_a_week_and_the_owner_can_change_it_within_limits() {
        reset_for_test();
        assert_eq!(days(), DEFAULT_DAYS);
        assert_eq!(ttl_ms(), 7 * 24 * 3600 * 1000);
        set_days(3).unwrap();
        assert_eq!(days(), 3);
        assert!(set_days(0).is_err() && set_days(31).is_err(), "вне пределов не принимается");
        assert_eq!(days(), 3, "отказ ничего не меняет");
        // испорченный файл — по умолчанию, а не ноль и не «навсегда»
        std::fs::write(path(), b"junk").unwrap();
        *CACHE.lock().unwrap() = None;
        assert_eq!(days(), DEFAULT_DAYS);
        // выход за пределы в файле (правка вручную) прижимается к границам
        std::fs::write(path(), br#"{"days": 9999}"#).unwrap();
        *CACHE.lock().unwrap() = None;
        assert_eq!(days(), MAX_DAYS);
        reset_for_test();
    }
}
