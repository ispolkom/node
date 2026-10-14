//! Жалобы пользователей и меры владельца узла.
//!
//! Хранится запечатанным ключом закрытой среды (область `complaints`): пока среда заперта, жалобы не читаются и не принимаются.
//! Любой сопряжённый пользователь может пожаловаться на другого пользователя или на группу; владелец узла отвечает заявителю и
//! нарушителю от имени узла и применяет меры: предупреждение, ограничение на срок, удаление с узла. Заявитель нарушителю не виден.

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

pub const AAD: &str = "complaints";
/// Не больше стольких жалоб в сутки от одного человека.
pub const MAX_PER_DAY: usize = 5;
const MAX_TEXT: usize = 2000;
const MAX_EVIDENCE: usize = 10;
const MAX_EVIDENCE_LEN: usize = 1000;
const MAX_COMPLAINTS: usize = 2000;
const MAX_THREAD: usize = 200;
const DAY_MS: u64 = 24 * 3600 * 1000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Msg {
    pub ts: u64,
    /// `reporter` | `target`: с кем ведётся переписка; `from`: `user` | `admin`
    pub side: String,
    pub from: String,
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Measure {
    pub ts: u64,
    /// `warn` | `restrict` | `remove` | `delete_group` | `note`
    pub kind: String,
    pub note: String,
    #[serde(default)]
    pub until_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Complaint {
    pub id: String,
    pub ts: u64,
    pub reporter: String,
    /// `user` | `group`
    pub target_kind: String,
    pub target_id: String,
    pub target_name: String,
    pub reason: String,
    pub text: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// `new` | `review` | `closed`
    pub status: String,
    #[serde(default)]
    pub thread: Vec<Msg>,
    #[serde(default)]
    pub measures: Vec<Measure>,
}

#[derive(Default)]
struct State {
    key: Option<[u8; 32]>,
    items: Vec<Complaint>,
}

static STATE: Mutex<State> = Mutex::new(State { key: None, items: Vec::new() });

fn path() -> PathBuf {
    crate::util::data_dir::data_dir().join("complaints.enc")
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn lock_state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn persist(s: &State) -> Result<(), (StatusCode, String)> {
    let Some(key) = s.key.as_ref() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "закрытая среда заперта".into()));
    };
    let bytes = serde_json::to_vec(&s.items).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::util::sealed::write(&path(), key, AAD, &bytes).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("не удалось сохранить жалобы: {e}")))
}

/// Выдан ключ области: жалобы читаются из запечатанного файла.
pub fn unlock(key: [u8; 32]) {
    let items = std::fs::read(path())
        .ok()
        .and_then(|b| crate::util::sealed::open(&key, AAD, &b))
        .and_then(|p| serde_json::from_slice::<Vec<Complaint>>(&p).ok())
        .unwrap_or_default();
    let mut s = lock_state();
    s.key = Some(key);
    s.items = items;
}

/// Заперто: из памяти убирается всё.
pub fn lock() {
    let mut s = lock_state();
    s.key = None;
    s.items = Vec::new();
}

/// Среда стёрта: файл удаляется (ключа к нему нет ни у кого).
pub fn erase() {
    lock();
    let _ = std::fs::remove_file(path());
}

fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

const REASONS: &[&str] = &["spam", "abuse", "illegal", "danger", "other"];

/// Принять жалобу. Ограничения: не больше [`MAX_PER_DAY`] в сутки на человека, одна открытая на одну цель.
pub fn submit(
    reporter: &str,
    target_kind: &str,
    target_id: &str,
    target_name: &str,
    reason: &str,
    text: &str,
    evidence: &[String],
) -> Result<String, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    if !matches!(target_kind, "user" | "group") || target_id.is_empty() || target_id.len() > 128 {
        return Err(bad("не указано, на кого жалоба"));
    }
    if target_kind == "user" && target_id == reporter {
        return Err(bad("на себя пожаловаться нельзя"));
    }
    let reason = if REASONS.contains(&reason) { reason } else { "other" };
    let text = clean(text, MAX_TEXT);
    if text.is_empty() {
        return Err(bad("опишите, что случилось"));
    }
    let evidence: Vec<String> = evidence.iter().take(MAX_EVIDENCE).map(|e| clean(e, MAX_EVIDENCE_LEN)).filter(|e| !e.is_empty()).collect();
    let mut s = lock_state();
    if s.key.is_none() {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "узел сейчас не принимает жалобы, попробуйте позже".into()));
    }
    let now = now_ms();
    let today = s.items.iter().filter(|c| c.reporter == reporter && now.saturating_sub(c.ts) < DAY_MS).count();
    if today >= MAX_PER_DAY {
        return Err((StatusCode::TOO_MANY_REQUESTS, "за сутки можно подать не больше пяти жалоб".into()));
    }
    if s.items.iter().any(|c| c.reporter == reporter && c.target_id == target_id && c.status != "closed") {
        return Err((StatusCode::CONFLICT, "жалоба на это уже рассматривается".into()));
    }
    if s.items.len() >= MAX_COMPLAINTS {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "очередь жалоб переполнена".into()));
    }
    let id = new_id();
    s.items.push(Complaint {
        id: id.clone(),
        ts: now,
        reporter: reporter.to_string(),
        target_kind: target_kind.to_string(),
        target_id: target_id.to_string(),
        target_name: clean(target_name, 80),
        reason: reason.to_string(),
        text,
        evidence,
        status: "new".into(),
        thread: Vec::new(),
        measures: Vec::new(),
    });
    persist(&s)?;
    Ok(id)
}

fn summary(c: &Complaint) -> Value {
    json!({
        "id": c.id, "ts": c.ts, "reporter": c.reporter, "target_kind": c.target_kind, "target_id": c.target_id,
        "target_name": c.target_name, "reason": c.reason, "status": c.status, "messages": c.thread.len(),
        "last_ts": c.thread.last().map(|m| m.ts).unwrap_or(c.ts),
    })
}

/// Все жалобы (для владельца): новые сверху.
pub fn list() -> Vec<Value> {
    let s = lock_state();
    let mut v: Vec<&Complaint> = s.items.iter().collect();
    v.sort_by(|a, b| b.ts.cmp(&a.ts));
    v.into_iter().map(summary).collect()
}

pub fn get(id: &str) -> Option<Value> {
    let s = lock_state();
    s.items.iter().find(|c| c.id == id).map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
}

pub fn find(id: &str) -> Option<Complaint> {
    lock_state().items.iter().find(|c| c.id == id).cloned()
}

/// Свои жалобы заявителя: состояние и ответы узла (без чужих данных).
pub fn mine(reporter: &str) -> Vec<Value> {
    let s = lock_state();
    s.items
        .iter()
        .filter(|c| c.reporter == reporter)
        .map(|c| {
            json!({
                "id": c.id, "ts": c.ts, "target_kind": c.target_kind, "target_name": c.target_name, "reason": c.reason, "status": c.status,
                "replies": c.thread.iter().filter(|m| m.side == "reporter").map(|m| json!({"ts": m.ts, "from": m.from, "text": m.text})).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// Запись в переписку жалобы. `side`: `reporter` | `target`; `from`: `user` | `admin`.
pub fn add_message(id: &str, side: &str, from: &str, text: &str) -> Result<(), (StatusCode, String)> {
    let text = clean(text, MAX_TEXT);
    if text.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "пустое сообщение".into()));
    }
    let mut s = lock_state();
    let c = s.items.iter_mut().find(|c| c.id == id).ok_or((StatusCode::NOT_FOUND, "жалобы нет".to_string()))?;
    if c.thread.len() >= MAX_THREAD {
        return Err((StatusCode::TOO_MANY_REQUESTS, "переписка по жалобе слишком длинная".into()));
    }
    c.thread.push(Msg { ts: now_ms(), side: side.to_string(), from: from.to_string(), text });
    if c.status == "new" && from == "admin" {
        c.status = "review".into();
    }
    persist(&s)
}

pub fn set_status(id: &str, status: &str) -> Result<(), (StatusCode, String)> {
    if !matches!(status, "new" | "review" | "closed") {
        return Err((StatusCode::BAD_REQUEST, "неверный статус".into()));
    }
    let mut s = lock_state();
    let c = s.items.iter_mut().find(|c| c.id == id).ok_or((StatusCode::NOT_FOUND, "жалобы нет".to_string()))?;
    c.status = status.to_string();
    persist(&s)
}

/// Записать меру в журнал жалобы (сама мера применяется вызывающим).
pub fn add_measure(id: &str, kind: &str, note: &str, until_ms: u64) -> Result<(), (StatusCode, String)> {
    let mut s = lock_state();
    let c = s.items.iter_mut().find(|c| c.id == id).ok_or((StatusCode::NOT_FOUND, "жалобы нет".to_string()))?;
    c.measures.push(Measure { ts: now_ms(), kind: kind.to_string(), note: clean(note, 500), until_ms });
    persist(&s)
}

/// Действует ли ограничение на человека (по всем жалобам): до какого времени, 0 — нет.
pub fn restricted_until(device: &str) -> u64 {
    let s = lock_state();
    let now = now_ms();
    s.items
        .iter()
        .filter(|c| c.target_kind == "user" && c.target_id == device)
        .flat_map(|c| c.measures.iter())
        .filter(|m| m.kind == "restrict" && m.until_ms > now)
        .map(|m| m.until_ms)
        .max()
        .unwrap_or(0)
}

/// Журнал мер, применённых к человеку (для него самого: прозрачность).
pub fn measures_for(device: &str) -> Vec<Value> {
    let s = lock_state();
    s.items
        .iter()
        .filter(|c| c.target_kind == "user" && c.target_id == device)
        .flat_map(|c| c.measures.iter())
        .filter(|m| m.kind != "note")
        .map(|m| json!({"ts": m.ts, "kind": m.kind, "note": m.note, "until_ms": m.until_ms}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fresh() {
        lock();
        let d = tempfile::tempdir().unwrap();
        // каталог данных тестов общий: файл удаляем, чтобы не читать чужое
        let _ = std::fs::remove_file(path());
        std::mem::forget(d);
        unlock([7; 32]);
    }

    #[test]
    fn complaint_lifecycle_limits_and_measures() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fresh();
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        assert!(submit(&a, "user", &a, "я", "spam", "x", &[]).is_err(), "на себя нельзя");
        assert!(submit(&a, "user", &b, "Боб", "spam", "   ", &[]).is_err(), "пустое описание");
        let id = submit(&a, "user", &b, "Боб", "spam", "спамит", &["сообщение 1".into()]).unwrap();
        assert!(submit(&a, "user", &b, "Боб", "spam", "ещё", &[]).is_err(), "одна открытая на цель");
        // лимит в сутки: пять жалоб на разные цели
        for i in 0..4 {
            submit(&a, "group", &format!("g{i}"), "Группа", "abuse", "плохо", &[]).unwrap();
        }
        assert_eq!(submit(&a, "group", "g9", "Группа", "abuse", "плохо", &[]).unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
        add_message(&id, "reporter", "admin", "приняли в работу").unwrap();
        assert_eq!(find(&id).unwrap().status, "review");
        // заявитель видит ответы только в своей стороне
        add_message(&id, "target", "admin", "предупреждение").unwrap();
        let mine_a = mine(&a);
        let replies = mine_a.iter().find(|c| c["id"] == id).unwrap()["replies"].as_array().unwrap().len();
        assert_eq!(replies, 1, "заявитель не видит переписку с нарушителем");
        // мера «ограничение» действует до срока
        assert_eq!(restricted_until(&b), 0);
        add_measure(&id, "restrict", "сутки", now_ms() + 1000 * 3600).unwrap();
        assert!(restricted_until(&b) > now_ms());
        assert_eq!(measures_for(&b).len(), 1);
        set_status(&id, "closed").unwrap();
        // после закрытия по той же цели можно подать новую
        submit(&a, "user", &b, "Боб", "abuse", "опять", &[]).unwrap_err(); // но суточный лимит исчерпан
    }

    #[test]
    fn locked_node_does_not_accept_or_show_complaints() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fresh();
        let a = "a".repeat(64);
        submit(&a, "group", "gx", "Г", "abuse", "плохо", &[]).unwrap();
        lock();
        assert!(list().is_empty());
        assert_eq!(submit(&a, "group", "gy", "Г", "abuse", "плохо", &[]).unwrap_err().0, StatusCode::SERVICE_UNAVAILABLE);
        unlock([7; 32]);
        assert_eq!(list().len(), 1, "после входа жалоба на месте");
    }
}
