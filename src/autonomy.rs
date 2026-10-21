//! Автономный узел (docs/ROADMAP_R9.md §4): полностью работает для своих клиентов, но с общей сетью ничего не имеет —
//! ни bootstrap, ни DHT, ни соседей, ни хопов, ни заявок, ни mDNS, ни проверки обновлений. Сопрягается только с
//! автономными узлами, внесёнными владельцем вручную (список доверенных, `~/.yandi/trusted_peers.json`); остальных молча
//! не принимает. Режим читается один раз при старте: смена — через `set`, затем перезапуск узла.
//!
//! Метка мира подмешивается в подписанные байты обоих рукопожатий: узел общей сети и автономный не примут друг друга
//! даже по ошибке. Для общей сети метка пустая — байты рукопожатия прежние.
use std::sync::OnceLock;

use crate::util::HashId;

static MODE: OnceLock<bool> = OnceLock::new();

/// Автономен ли узел (значение на момент старта).
pub fn enabled() -> bool {
    *MODE.get_or_init(|| crate::core::config::try_get_config().map(|c| c.network.autonomous).unwrap_or(false))
}

/// Метка мира для подписанных байт рукопожатия.
pub fn world_tag() -> &'static [u8] {
    if enabled() { b"yandi-mesh-v1\0" } else { b"" }
}

/// Принять ли узел `peer` на входе. В общей сети — да (дальше обычный допуск); в автономном — только внесённых владельцем.
pub fn admits(peer: &HashId) -> bool {
    // отозванная сборка общей сети не принимает никого из узлов (аудит R9 №8); автономный узел — только свой меш
    if crate::bootstrap::is_revoked() && !enabled() {
        return false;
    }
    !enabled() || crate::exit_policy::is_trusted(peer)
}

/// Файлы сетевого мира: при смене режима стираются, чтобы прежний мир не оставлял следов (или-или).
fn world_files() -> Vec<std::path::PathBuf> {
    let d = crate::util::data_dir::data_dir();
    vec![
        d.join("bootstrap_cache.json"),
        d.join("bootstrap_applications.json"),
        d.join("ledger.json"),
        crate::web::peers::default_peer_directory_path(),
        std::path::PathBuf::from("nodes/bootstrap.json"),
    ]
}

/// Сменить режим: сначала стереть сетевые данные прежнего мира, потом записать режим (сбой стирания — режим не меняется,
/// повтор дочищает). Вступает в силу только после перезапуска: работающие задачи старого мира иначе записали бы данные
/// обратно, поэтому ручка сама перезапускает узел (аудит R9 №12).
pub fn set(on: bool) -> Result<bool, String> {
    let mut cfg = crate::core::config::try_get_config().ok_or("конфигурация узла не загружена")?;
    // режим тот же — ничего не стираем (иначе повторное «включить» стёрло бы меш)
    if cfg.network.autonomous == on {
        return Ok(false);
    }
    crate::exit_policy::set_trusted(std::iter::empty());
    for f in world_files() {
        match std::fs::remove_file(&f) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("не удалось стереть {}: {e}; режим не изменён", f.display())),
        }
    }
    cfg.network.autonomous = on;
    crate::core::config::update_config(cfg).map_err(|e| format!("не удалось сохранить режим: {e}"))?;
    Ok(true)
}

static CAPABLE: OnceLock<bool> = OnceLock::new();

/// Может ли узел выбирать режим: белый адрес и не слабое железо (решает main при старте). Остальные всегда в общей сети.
pub fn set_capable(v: bool) {
    let _ = CAPABLE.set(v);
}

fn capable() -> bool {
    CAPABLE.get().copied().unwrap_or(false)
}

use axum::{extract::Json, http::StatusCode, response::{IntoResponse, Response}, routing::get, Router};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    on: bool,
    /// пароль администратора ещё раз: смена режима стирает сетевые данные (как удаление группы)
    password: String,
}

async fn get_mode() -> Json<serde_json::Value> {
    let saved = crate::core::config::try_get_config().map(|c| c.network.autonomous).unwrap_or(false);
    Json(serde_json::json!({ "autonomous": enabled(), "saved": saved, "restart_needed": saved != enabled(), "can_choose": capable() || enabled() }))
}

async fn post_mode(Json(b): Json<Body>) -> Response {
    let password = b.password.clone();
    match tokio::task::spawn_blocking(move || crate::web::auth::verify_admin_counted(&password)).await {
        Ok(Ok(true)) => {}
        _ => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Неверный пароль администратора"}))).into_response(),
    }
    // выключить автономность можно всегда; включить — только узлу, который мог бы помогать сети
    if b.on && !capable() {
        return (StatusCode::CONFLICT, Json(serde_json::json!({"error": "Автономный режим выбирает только узел с белым адресом и достаточной мощностью; этот узел и так не нагружает сеть."}))).into_response();
    }
    match set(b.on) {
        Ok(false) => Json(serde_json::json!({"ok": true, "changed": false})).into_response(),
        Ok(changed) => {
            // перезапуск через systemd (Restart=on-failure): выход с ошибкой через 2 с, чтобы ответ успел уйти
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                eprintln!("[autonomy] режим узла изменён — перезапуск");
                std::process::exit(75);
            });
            Json(serde_json::json!({"ok": true, "changed": changed, "restarting": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/node/autonomy", get(get_mode).post(post_mode))
}
