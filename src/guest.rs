//! Гостевой уровень узла (docs/GUEST.md): устройство, сопряжённое без QR.
//!
//! * Опция узла «разрешить сопряжение без QR» — файл `guest_pairing.json`; по умолчанию выключена (узел принимает чужих только по воле оператора).
//! * Общий бюджет гостей: сколько гостей помещается, как часто выдаются новые, через сколько бездействия гость исчезает.
//! * Суточные квоты файлов гостя — только в оперативной памяти, на токен устройства; перезапуск и новый ключ их обнуляют (общий бюджет это компенсирует).
//! * Адреса (IP) здесь не принимаются и не хранятся вовсе.
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Файл гостя: самый большой, байт.
pub const MAX_FILE: u64 = 30 << 20;
/// Файлов за скользящие сутки.
pub const MAX_FILES_PER_DAY: usize = 20;
/// Байт файлов за скользящие сутки.
pub const MAX_BYTES_PER_DAY: u64 = 300 << 20;
pub const DAY: Duration = Duration::from_secs(24 * 3600);
/// Сколько гостей одновременно (общий бюджет узла).
pub const MAX_GUESTS: usize = 64;
/// Новых гостей в минуту на весь узел.
pub const MAX_ISSUE_PER_MIN: usize = 12;
/// Гость, молчащий дольше, удаляется.
pub const IDLE: Duration = DAY;

fn path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("guest_pairing.json")
}

/// Включено ли сопряжение без QR (нет файла или не читается — выключено).
pub fn enabled() -> bool {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| v["enabled"] == true)
        .unwrap_or(false)
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    let s = serde_json::json!({ "enabled": on }).to_string();
    crate::util::private_file::write_private(&path(), s.as_bytes()).map_err(|e| format!("не удалось сохранить настройку: {e}"))
}

/// Квота одного гостя: скользящее окно «когда и сколько байт».
#[derive(Default)]
pub struct Quota {
    sent: VecDeque<(Instant, u64)>,
}

impl Quota {
    /// Зарезервировать файл заявленного размера. Отказ — готовая фраза. Отменённые передачи ресурс не возвращают.
    pub fn reserve(&mut self, now: Instant, size: u64) -> Result<(), &'static str> {
        while matches!(self.sent.front(), Some((t, _)) if now.duration_since(*t) >= DAY) {
            self.sent.pop_front();
        }
        if size == 0 || size > MAX_FILE {
            return Err("гостю можно передавать файлы не больше 30 МБ");
        }
        if self.sent.len() >= MAX_FILES_PER_DAY {
            return Err("гостю можно передавать не больше 20 файлов в сутки");
        }
        let total: u64 = self.sent.iter().map(|(_, s)| *s).sum();
        if total + size > MAX_BYTES_PER_DAY {
            return Err("гостю можно передавать не больше 300 МБ файлов в сутки");
        }
        self.sent.push_back((now, size));
        Ok(())
    }
}

fn quotas() -> &'static Mutex<HashMap<String, Quota>> {
    static Q: OnceLock<Mutex<HashMap<String, Quota>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Квота гостя по хэшу токена устройства.
pub fn reserve_file(token_hash: &str, size: u64) -> Result<(), &'static str> {
    let mut g = quotas().lock().unwrap_or_else(|e| e.into_inner());
    g.entry(token_hash.to_string()).or_default().reserve(Instant::now(), size)
}

/// Забыть квоту гостя (гость удалён).
pub fn forget(token_hash: &str) {
    quotas().lock().unwrap_or_else(|e| e.into_inner()).remove(token_hash);
}

fn issued() -> &'static Mutex<VecDeque<Instant>> {
    static I: OnceLock<Mutex<VecDeque<Instant>>> = OnceLock::new();
    I.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Можно ли выдать ещё одного гостя прямо сейчас по частоте; если да — занимает место в окне.
pub fn take_issue_slot(now: Instant) -> bool {
    let mut q = issued().lock().unwrap_or_else(|e| e.into_inner());
    while matches!(q.front(), Some(t) if now.duration_since(*t) >= Duration::from_secs(60)) {
        q.pop_front();
    }
    if q.len() >= MAX_ISSUE_PER_MIN {
        return false;
    }
    q.push_back(now);
    true
}

/// Для проверок: сбросить окно частоты выдачи.
use axum::{extract::Json, http::StatusCode, response::{IntoResponse, Response}, routing::get, Router};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    enabled: bool,
}

async fn get_settings() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "enabled": enabled(),
        "guests": crate::mobile_api::guest_count(),
        "max_guests": MAX_GUESTS,
        "limits": {"max_file_bytes": MAX_FILE, "max_files_per_day": MAX_FILES_PER_DAY, "max_bytes_per_day": MAX_BYTES_PER_DAY, "idle_hours": 24}
    }))
}

async fn post_settings(Json(b): Json<Body>) -> Response {
    match set_enabled(b.enabled) {
        Ok(()) => {
            // выключили — действующие гости остаются до конца срока, новых нет; удалять их или нет решает владелец отдельно
            Json(serde_json::json!({"ok": true, "enabled": b.enabled})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok": false, "error": e}))).into_response(),
    }
}

/// Настройка для оболочки ПК и веб-страницы (под проверкой входа владельца).
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/guest-pairing", get(get_settings).post(post_settings))
}

#[cfg(test)]
pub(crate) fn take_issue_slot_reset_for_test() {
    issued().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_file_over_thirty_megabytes_is_refused_and_exactly_thirty_passes() {
        let mut q = Quota::default();
        let now = Instant::now();
        assert!(q.reserve(now, MAX_FILE + 1).is_err(), "превышение на байт");
        assert!(q.reserve(now, MAX_FILE).is_ok());
        assert!(q.reserve(now, 0).is_err(), "пустой файл не передача");
    }

    #[test]
    fn twenty_small_files_a_day_then_no_more_until_the_window_slides() {
        let mut q = Quota::default();
        let t0 = Instant::now();
        for _ in 0..MAX_FILES_PER_DAY {
            assert!(q.reserve(t0, 1024).is_ok());
        }
        assert!(q.reserve(t0, 1024).unwrap_err().contains("20 файлов"));
        // через сутки окно сдвинулось
        assert!(q.reserve(t0 + DAY, 1024).is_ok());
    }

    #[test]
    fn the_daily_volume_beats_the_file_count_so_splitting_does_not_help() {
        let mut q = Quota::default();
        let t0 = Instant::now();
        // 29 МБ × 10 = 290 МБ проходят, одиннадцатый (319 МБ) уже нет, хотя файлов меньше двадцати
        for _ in 0..10 {
            assert!(q.reserve(t0, 29 << 20).is_ok());
        }
        assert!(q.reserve(t0, 29 << 20).unwrap_err().contains("300 МБ"));
        // а мелкий файл ещё влезает в остаток
        assert!(q.reserve(t0, 5 << 20).is_ok());
        assert!(q.reserve(t0, 6 << 20).is_err());
    }

    #[test]
    fn a_cancelled_transfer_still_spends_the_budget() {
        let mut q = Quota::default();
        let t0 = Instant::now();
        for _ in 0..MAX_FILES_PER_DAY {
            q.reserve(t0, 1 << 20).unwrap();
        }
        // резерв не возвращается ни при каких условиях: метода «вернуть» нет, окно двигается только временем
        assert!(q.reserve(t0 + DAY - Duration::from_secs(1), 1).is_err());
    }

    #[test]
    fn quotas_are_per_guest() {
        forget("a");
        forget("b");
        for _ in 0..MAX_FILES_PER_DAY {
            reserve_file("a", 1).unwrap();
        }
        assert!(reserve_file("a", 1).is_err());
        assert!(reserve_file("b", 1).is_ok(), "другой гость не страдает");
        forget("a");
        assert!(reserve_file("a", 1).is_ok(), "после удаления гостя счёт начат заново");
        forget("a");
        forget("b");
    }

    #[test]
    fn new_guests_are_issued_at_a_limited_rate() {
        let t0 = Instant::now() + Duration::from_secs(10_000);
        issued().lock().unwrap().clear();
        for _ in 0..MAX_ISSUE_PER_MIN {
            assert!(take_issue_slot(t0));
        }
        assert!(!take_issue_slot(t0), "лимит в минуту");
        assert!(take_issue_slot(t0 + Duration::from_secs(61)), "через минуту снова можно");
        issued().lock().unwrap().clear();
    }
}
