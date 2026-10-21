//! Гостевой уровень узла (docs/GUEST.md): устройство, сопряжённое без QR.
//!
//! * Опция узла «разрешить сопряжение без QR» — файл `guest_pairing.json`; по умолчанию ВКЛЮЧЕНА, пока владелец не выключит (владелец решает сам; файла нет — гости принимаются).
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
/// Аккаунт гостя живёт столько от создания, затем удаляется, даже если активен (docs/ROADMAP_R9.md §2).
pub const LIFETIME: Duration = DAY;
/// Сообщений гостя в сутки: считает приложение (текст зашифрован, узел не отличает сообщение от квитанции).
pub const MAX_MESSAGES_PER_DAY: usize = 10;
/// Потолок, который держит узел: кадров переписки гостя за скользящие сутки (сообщения + квитанции + правки).
/// Модифицированное приложение обойдёт счёт сообщений, но не этот потолок.
pub const MAX_MSG_FRAMES_PER_DAY: usize = 60;
/// Сумма гостевых файлов на весь узел за скользящие сутки: сверх — «узел занят», чтобы гости не забили диск.
pub const MAX_GUEST_BYTES_TOTAL: u64 = 2 << 30;
/// Жалоб от разных устройств за сутки, после которых гость удаляется.
pub const COMPLAINTS_TO_REMOVE: usize = 3;

fn path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("guest_pairing.json")
}

/// Включено ли сопряжение без QR. По умолчанию ВКЛЮЧЕНО: пока владелец сам не выключил, узел принимает гостей. Файл есть и в нём
/// не `true` (или не читается) — выключено: ошибка чтения не должна молча открывать узел, раз владелец уже что-то решил.
pub fn enabled() -> bool {
    match std::fs::read_to_string(path()) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
        Ok(s) => serde_json::from_str::<serde_json::Value>(&s).ok().map(|v| v["enabled"] == true).unwrap_or(false),
    }
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

/// Квота гостя по хэшу токена устройства; сверху — общий потолок гостевых файлов узла.
pub fn reserve_file(token_hash: &str, size: u64) -> Result<(), &'static str> {
    let now = Instant::now();
    let mut g = quotas().lock().unwrap_or_else(|e| e.into_inner());
    let total: u64 = g.values().flat_map(|q| q.sent.iter()).filter(|(t, _)| now.duration_since(*t) < DAY).map(|(_, s)| *s).sum();
    if total.saturating_add(size) > MAX_GUEST_BYTES_TOTAL {
        return Err("узел занят: гостевые файлы за сутки исчерпаны, попробуйте позже");
    }
    g.entry(token_hash.to_string()).or_default().reserve(now, size)
}

fn msg_frames() -> &'static Mutex<HashMap<String, VecDeque<Instant>>> {
    static M: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Занять гостю один кадр переписки (по номеру устройства). false — суточный потолок исчерпан.
pub fn take_msg_frame(peer: &str) -> bool {
    take_msg_frame_at(peer, Instant::now())
}

fn take_msg_frame_at(peer: &str, now: Instant) -> bool {
    let mut g = msg_frames().lock().unwrap_or_else(|e| e.into_inner());
    let q = g.entry(peer.to_string()).or_default();
    while matches!(q.front(), Some(t) if now.duration_since(*t) >= DAY) {
        q.pop_front();
    }
    if q.len() >= MAX_MSG_FRAMES_PER_DAY {
        return false;
    }
    q.push_back(now);
    true
}

fn complaints() -> &'static Mutex<HashMap<String, Vec<(Instant, String)>>> {
    static C: OnceLock<Mutex<HashMap<String, Vec<(Instant, String)>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Учесть жалобу `reporter` на гостя `target` (номера устройств). true — набралось `COMPLAINTS_TO_REMOVE` жалоб от разных
/// устройств за сутки: гостя пора удалить. Повторная жалоба того же устройства не считается.
pub fn note_complaint(target: &str, reporter: &str) -> bool {
    note_complaint_at(target, reporter, Instant::now())
}

fn note_complaint_at(target: &str, reporter: &str, now: Instant) -> bool {
    if target == reporter {
        return false;
    }
    let mut g = complaints().lock().unwrap_or_else(|e| e.into_inner());
    let l = g.entry(target.to_string()).or_default();
    l.retain(|(t, _)| now.duration_since(*t) < DAY);
    if !l.iter().any(|(_, r)| r == reporter) {
        l.push((now, reporter.to_string()));
    }
    l.len() >= COMPLAINTS_TO_REMOVE
}

/// Забыть счётчики гостя по номеру устройства (кадры переписки, жалобы на него).
pub fn forget_peer(peer: &str) {
    msg_frames().lock().unwrap_or_else(|e| e.into_inner()).remove(peer);
    complaints().lock().unwrap_or_else(|e| e.into_inner()).remove(peer);
}

/// Забыть квоту гостя (гость удалён).
pub fn forget(token_hash: &str) {
    quotas().lock().unwrap_or_else(|e| e.into_inner()).remove(token_hash);
    calls().lock().unwrap_or_else(|e| e.into_inner()).remove(token_hash);
}

/// Звонки гостя: только аудио; доступ к серверу звонков выдаётся на `CALL_SECS`, звонков за скользящие сутки не больше `CALLS_PER_DAY`
/// (= 60 минут в сутки), а одновременно на весь узел не больше `MAX_LIVE_CALLS` гостевых звонков.
pub const CALL_SECS: u64 = 15 * 60;
pub const CALLS_PER_DAY: usize = 4;
pub const MAX_LIVE_CALLS: usize = 4;

#[derive(Debug, PartialEq, Eq)]
pub enum CallRefusal {
    /// гость исчерпал 60 минут в сутки
    DayLimit,
    /// на узле уже идёт максимум гостевых звонков
    Busy,
}

fn calls() -> &'static Mutex<HashMap<String, VecDeque<Instant>>> {
    static C: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Занять гостю один звонок (на `CALL_SECS`). Вызывается при выдаче доступа к серверу звонков.
pub fn reserve_call(token_hash: &str, now: Instant) -> Result<u64, CallRefusal> {
    let mut g = calls().lock().unwrap_or_else(|e| e.into_inner());
    for q in g.values_mut() {
        while matches!(q.front(), Some(t) if now.duration_since(*t) >= DAY) {
            q.pop_front();
        }
    }
    let live = g.values().flat_map(|q| q.iter()).filter(|t| now.duration_since(**t) < Duration::from_secs(CALL_SECS)).count();
    let mine = g.entry(token_hash.to_string()).or_default();
    if mine.len() >= CALLS_PER_DAY {
        return Err(CallRefusal::DayLimit);
    }
    if live >= MAX_LIVE_CALLS {
        return Err(CallRefusal::Busy);
    }
    mine.push_back(now);
    Ok(CALL_SECS)
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
    Router::new()
        .route("/api/guest-pairing", get(get_settings).post(post_settings))
        .route("/api/pair-requests", get(list_requests))
        .route("/api/pair-requests/:id", axum::routing::post(decide_request))
}

async fn list_requests() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "requests": crate::mobile_api::pair_requests_pending() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    accept: bool,
}

async fn decide_request(axum::extract::Path(id): axum::extract::Path<String>, Json(d): Json<Decision>) -> Response {
    match crate::mobile_api::decide_pair_request(&id, d.accept) {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": e}))).into_response(),
    }
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
    fn a_guest_gets_four_fifteen_minute_calls_a_day_and_the_window_slides() {
        for t in ["c1", "c2"] { forget(t); }
        let t0 = Instant::now();
        for _ in 0..CALLS_PER_DAY {
            assert_eq!(reserve_call("c1", t0), Ok(15 * 60));
        }
        assert_eq!(reserve_call("c1", t0), Err(CallRefusal::DayLimit), "пятый звонок за сутки — отказ (60 минут)");
        assert_eq!(CALL_SECS * CALLS_PER_DAY as u64, 60 * 60);
        assert_eq!(reserve_call("c1", t0 + DAY), Ok(15 * 60), "через сутки окно сдвинулось");
        forget("c1");
    }

    #[test]
    fn the_node_caps_simultaneous_guest_calls_and_frees_the_slot_when_a_call_ends() {
        let ids: Vec<String> = (0..=MAX_LIVE_CALLS).map(|i| format!("cc{i}")).collect();
        for t in &ids { forget(t); }
        let t0 = Instant::now();
        for t in &ids[..MAX_LIVE_CALLS] {
            assert!(reserve_call(t, t0).is_ok());
        }
        assert_eq!(reserve_call(&ids[MAX_LIVE_CALLS], t0), Err(CallRefusal::Busy), "звонков одновременно не больше четырёх");
        // через 15 минут прежние звонки кончились (доступ к серверу звонков истёк): место свободно
        assert!(reserve_call(&ids[MAX_LIVE_CALLS], t0 + Duration::from_secs(CALL_SECS)).is_ok());
        for t in &ids { forget(t); }
    }

    #[test]
    fn guests_are_accepted_by_default_and_only_an_explicit_off_closes_the_node() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = std::fs::remove_file(path());
        assert!(enabled(), "файла нет — владелец ещё ничего не решал, гости принимаются");
        set_enabled(false).unwrap();
        assert!(!enabled());
        set_enabled(true).unwrap();
        assert!(enabled());
        std::fs::write(path(), b"\xff not json").unwrap();
        assert!(!enabled(), "испорченный файл не открывает узел молча");
        let _ = std::fs::remove_file(path());
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
