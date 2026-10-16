//! Резервная копия узла: ручки владельца (под проверкой входа в веб-интерфейс) и плановая задача.
//!
//! * `GET  /api/backup/status`  — что и когда копировалось, где папка автокопий, что в копию не входит;
//! * `POST /api/backup/export`  — «Сохранить резервную копию»: файл `.yandibak` отдаётся владельцу (и кладётся в папку, если она задана);
//! * `POST /api/backup/config`  — папка для автокопий (флешка, синхронизируемая с облаком папка); пусто — автокопии выключены.
//!
//! Плановая задача раз в сутки сверяет содержимое связей с прошлой копией в папке и делает новую только при изменении; критичные
//! изменения (устройства, баны, владение группой) просят копию сразу — см. [`notify_critical`].

use crate::node_backup::{self, Layout, Mode, Skip};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, OnceLock};

type RootGetter = Arc<dyn Fn() -> Option<[u8; 32]> + Send + Sync>;

struct Runtime {
    layout: Layout,
    root: RootGetter,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Раз в сутки (с запасом на задержки).
pub const DAILY_MS: u64 = 24 * 3600 * 1000;

/// Подключить копию к работающему узлу: где лежат каталоги и как достать корень ключей (он есть только у запущенного узла).
pub fn install(layout: Layout, root: RootGetter) {
    let _ = RUNTIME.set(Runtime { layout, root });
}

/// Изменилось то, без чего восстановление опасно (устройство добавлено или отозвано, участник забанен, сменился владелец группы):
/// плановая задача сделает копию в ближайшую минуту, не дожидаясь суток.
pub fn notify_critical() {
    crate::backup_signal::notify_critical();
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn rt() -> Result<&'static Runtime, Response> {
    RUNTIME.get().ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, Json(json!({"error": "резервные копии на этой платформе не включены"}))).into_response())
}

fn status_json(l: &Layout) -> serde_json::Value {
    let s = node_backup::load_state(l);
    json!({
        "counter": s.counter,
        "last_ok_ms": s.last_ok_ms,
        "last_saved_ms": s.last_saved_ms,
        "last_check_ms": s.last_check_ms,
        "last_error": s.last_error,
        "last_file": s.last_file,
        "dir": s.dir,
        "auto": s.dir.as_ref().is_some_and(|d| !d.trim().is_empty()),
        "keep": node_backup::KEEP_AUTO,
        "excluded": node_backup::EXCLUDED,
        "ext": node_backup::FILE_EXT,
    })
}

async fn status() -> Response {
    let r = match rt() { Ok(r) => r, Err(e) => return e };
    Json(status_json(&r.layout)).into_response()
}

async fn export() -> Response {
    let r = match rt() { Ok(r) => r, Err(e) => return e };
    let Some(root) = (r.root)() else {
        return (StatusCode::CONFLICT, Json(json!({"error": "ключи узла сейчас недоступны"}))).into_response();
    };
    let layout = r.layout.clone();
    let out = tokio::task::spawn_blocking(move || {
        let node_id = node_backup::node_id_from_files(&layout, &root)?;
        node_backup::run_backup(&layout, &root, &node_id, Mode::Now, now_ms())
    })
    .await;
    match out {
        Ok(Ok(o)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", o.name)),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            o.bytes,
        )
            .into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "сбой задачи копирования"}))).into_response(),
    }
}

#[derive(Deserialize)]
struct ConfigBody {
    #[serde(default)]
    dir: String,
}

/// Папка автокопий: пустая — выключено; иначе абсолютный путь вне каталогов самого узла (копию рядом с ключами хранить бессмысленно).
fn check_dir(l: &Layout, dir: &str) -> Result<Option<String>, String> {
    let d = dir.trim();
    if d.is_empty() {
        return Ok(None);
    }
    let p = std::path::Path::new(d);
    if !p.is_absolute() || d.contains('\0') || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("Папка должна быть задана полным путём без «..».".into());
    }
    for own in [&l.keys, &l.data, &l.home] {
        if p.starts_with(own) {
            return Err("Копия на том же диске в каталоге самого узла не защищает от потери диска: выберите флешку или другую папку.".into());
        }
    }
    std::fs::create_dir_all(p).map_err(|e| format!("Папку нельзя создать или открыть: {e}"))?;
    let probe = p.join(".yandi-write-test");
    std::fs::write(&probe, b"x").map_err(|e| format!("В папку нельзя писать: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(Some(d.to_string()))
}

async fn config(Json(b): Json<ConfigBody>) -> Response {
    let r = match rt() { Ok(r) => r, Err(e) => return e };
    let l = r.layout.clone();
    let dir = b.dir;
    let res = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let chosen = check_dir(&l, &dir)?;
        // под общим замком состояния: параллельное копирование не вернёт старую папку
        node_backup::update_state(&l, |s| {
            s.dir = chosen;
            // новая папка — первая плановая копия в неё делается сразу, даже если содержимое не менялось
            s.last_saved_ms = 0;
            s.last_check_ms = 0;
        })
        .map(|_| ())
    })
    .await;
    match res {
        Ok(Ok(())) => Json(status_json(&r.layout)).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "сбой"}))).into_response(),
    }
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/backup/status", get(status)).route("/api/backup/export", post(export)).route("/api/backup/config", post(config))
}

/// Пора ли делать плановую проверку.
pub fn due(state: &node_backup::BackupState, now: u64, critical: bool) -> bool {
    critical || state.last_check_ms == 0 || now.saturating_sub(state.last_check_ms) >= DAILY_MS
}

/// Плановая задача: раз в минуту смотрит, не пора ли (сутки с прошлой проверки или критичное изменение), и тогда сверяет содержимое.
pub async fn scheduler() {
    // после сбоя копия повторяется (не через сутки): критичное изменение остаётся «в долгу» до успеха, повторы — не чаще раза в 5 минут
    let (mut owed_critical, mut retry_at) = (false, 0u64);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let Some(r) = RUNTIME.get() else { continue };
        owed_critical |= crate::backup_signal::take();
        let st = node_backup::load_state(&r.layout);
        let now = now_ms();
        if st.dir.as_ref().is_none_or(|d| d.trim().is_empty()) || now < retry_at || !due(&st, now, owed_critical) {
            continue;
        }
        let Some(root) = (r.root)() else { continue };
        let layout = r.layout.clone();
        let res = tokio::task::spawn_blocking(move || {
            let node_id = node_backup::node_id_from_files(&layout, &root)?;
            node_backup::run_backup(&layout, &root, &node_id, Mode::Auto, now_ms())
        })
        .await;
        match res {
            Ok(Ok(o)) => {
                owed_critical = false;
                retry_at = 0;
                match o.skipped {
                    None => println!("💾 Резервная копия №{} записана: {}", o.counter, o.saved.map(|p| p.display().to_string()).unwrap_or_default()),
                    Some(Skip::Unchanged) | Some(Skip::NoFolder) => {}
                }
            }
            Ok(Err(e)) => {
                eprintln!("💾 Резервная копия не сделана, повтор через 5 минут: {e}");
                retry_at = now_ms() + 5 * 60 * 1000;
            }
            Err(e) => {
                eprintln!("💾 Задача копирования упала: {e}");
                retry_at = now_ms() + 5 * 60 * 1000;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scheduled_check_is_due_after_a_day_or_on_a_critical_change() {
        let mut s = node_backup::BackupState::default();
        assert!(due(&s, 1_000, false), "ещё ни разу не проверяли");
        s.last_check_ms = 1_000_000;
        assert!(!due(&s, 1_000_000 + 3_600_000, false), "через час — рано");
        assert!(due(&s, 1_000_000 + DAILY_MS, false), "через сутки пора");
        assert!(due(&s, 1_000_000 + 10, true), "критичное изменение — сразу");
    }

    #[test]
    fn the_folder_must_be_a_full_path_outside_the_node_directories_and_writable() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout { keys: d.path().join("k"), data: d.path().join("d"), home: d.path().join("h"), port: 9000 };
        assert_eq!(check_dir(&l, "  ").unwrap(), None, "пусто — автокопии выключены");
        assert!(check_dir(&l, "relative/dir").is_err());
        assert!(check_dir(&l, "/tmp/../etc").is_err());
        assert!(check_dir(&l, &l.data.join("backups").display().to_string()).unwrap_err().contains("не защищает"));
        let ok = d.path().join("flash/yandi");
        assert_eq!(check_dir(&l, &ok.display().to_string()).unwrap(), Some(ok.display().to_string()));
        assert!(ok.is_dir() && !ok.join(".yandi-write-test").exists());
    }
}
