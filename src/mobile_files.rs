//! Файлы между телефонами владельца (`/mobile/files...`, договорённость — docs/CLIENT_WIRE.md).
//!
//! Узел хранит только шифртекст: телефон-отправитель шифрует каждый кусок сам (ключ уходит получателю в зашифрованном сообщении
//! чата, узел его не видит), поэтому узел не знает ни имени файла, ни содержимого. Он знает размер, число кусков, кто кому и когда.
//! Файл ждёт получателя на диске узла (получатель может быть не в сети), докачивается по кускам и удаляется получателем после
//! скачивания либо по сроку. Лимиты ограничивают занимаемое место: размер файла, число кусков, общий объём и число незавершённых
//! передач одного отправителя.
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::mobile_api::{device_peer_of, is_device_peer, DeviceKey};

/// Самый большой файл (в открытом виде), байт.
pub const MAX_FILE: u64 = 1 << 30;
/// Больше кусков одной передачи не бывает (при куске в 256 КБ этого хватает на 1 ГБ).
pub const MAX_CHUNKS: u32 = 4096;
/// Самый большой кусок на проводе: 256 КБ данных плюс служебное шифрования.
pub const MAX_CHUNK_BYTES: usize = 320 * 1024;
/// Общий объём чужих файлов на этом узле.
const MAX_STORE: u64 = 4 << 30;
const MAX_PENDING_PER_SENDER: usize = 20;
/// Готовый файл лежит, пока его не заберут, но не дольше недели; недокачанный — сутки.
const TTL_DONE_MS: u64 = 7 * 24 * 3600 * 1000;
const TTL_PARTIAL_MS: u64 = 24 * 3600 * 1000;
/// Накладные расходы шифрования одного куска (случайное число 12 байт и метка 16 байт).
const CHUNK_OVERHEAD: u64 = 28;

#[derive(Clone, Serialize, Deserialize)]
struct Meta {
    id: String,
    from: String,
    to: String,
    /// подпись для списка; настоящее имя файла узлу не передаётся (оно внутри зашифрованного сообщения)
    file_name: String,
    file_size: u64,
    total_chunks: u32,
    created_ms: u64,
    done: bool,
}

fn root() -> PathBuf {
    crate::util::data_dir::data_dir().join("mobile_files")
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn dir_of(id: &str) -> PathBuf {
    root().join(id)
}

fn chunk_name(idx: u32) -> String {
    format!("c{idx:06}")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

async fn load_meta(id: &str) -> Option<Meta> {
    if !valid_id(id) {
        return None;
    }
    let s = tokio::fs::read_to_string(dir_of(id).join("meta.json")).await.ok()?;
    serde_json::from_str(&s).ok()
}

async fn save_meta(m: &Meta) -> std::io::Result<()> {
    let p = dir_of(&m.id).join("meta.json");
    let tmp = dir_of(&m.id).join("meta.json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(m).unwrap_or_default()).await?;
    tokio::fs::rename(&tmp, &p).await
}

async fn all_meta() -> Vec<Meta> {
    let mut out = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(root()).await else { return out };
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(name) = e.file_name().to_str() {
            if let Some(m) = load_meta(name).await {
                out.push(m);
            }
        }
    }
    out
}

/// Убирает просроченное и осиротевшее. Вызывается при начале новой передачи и при чтении списка.
async fn cleanup() {
    let now = now_ms();
    let Ok(mut rd) = tokio::fs::read_dir(root()).await else { return };
    while let Ok(Some(e)) = rd.next_entry().await {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        let drop_it = match load_meta(&name).await {
            Some(m) => now.saturating_sub(m.created_ms) > if m.done { TTL_DONE_MS } else { TTL_PARTIAL_MS },
            None => {
                // папка без описания: оставляем на сутки (идёт создание), потом убираем
                let age = e.metadata().await.ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()).unwrap_or(0);
                age > 24 * 3600
            }
        };
        if drop_it {
            let _ = tokio::fs::remove_dir_all(e.path()).await;
        }
    }
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"error": msg}))).into_response()
}

#[derive(Deserialize)]
pub struct StartReq {
    to_peer_id: String,
    #[serde(default)]
    file_name: String,
    file_size: u64,
    total_chunks: u32,
}

/// Начать передачу: узел выдаёт номер, под которым отправитель заливает куски.
pub async fn start(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Json(req): Json<StartReq>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    // получатель — другой телефон владельца или сам компьютер (переписка телефона с веб-чатом узла, см. mobile_self)
    if req.to_peer_id == me || !(is_device_peer(&req.to_peer_id) || crate::mobile_api::is_node_peer(&req.to_peer_id)) {
        return err(StatusCode::BAD_REQUEST, "файлы можно отправлять только на другое сопряжённое устройство");
    }
    if req.total_chunks == 0 || req.total_chunks > MAX_CHUNKS || req.file_size == 0 || req.file_size > MAX_FILE {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "файл слишком большой или пустой");
    }
    // кусков не меньше, чем нужно для размера, и не вдвое больше
    let per = req.file_size.div_ceil(req.total_chunks as u64);
    if per > MAX_CHUNK_BYTES as u64 - CHUNK_OVERHEAD {
        return err(StatusCode::BAD_REQUEST, "куски слишком крупные");
    }
    cleanup().await;
    let metas = all_meta().await;
    let used: u64 = metas.iter().map(|m| m.file_size + m.total_chunks as u64 * CHUNK_OVERHEAD).sum();
    let need = req.file_size + req.total_chunks as u64 * CHUNK_OVERHEAD;
    if used + need > MAX_STORE {
        return err(StatusCode::INSUFFICIENT_STORAGE, "на узле нет места для файлов");
    }
    if metas.iter().filter(|m| m.from == me && !m.done).count() >= MAX_PENDING_PER_SENDER {
        return err(StatusCode::TOO_MANY_REQUESTS, "слишком много незавершённых передач");
    }
    let id = {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut b);
        hex::encode(b)
    };
    let label: String = req.file_name.chars().filter(|c| !c.is_control() && *c != '/' && *c != '\\').take(64).collect();
    let meta = Meta { id: id.clone(), from: me, to: req.to_peer_id, file_name: label, file_size: req.file_size, total_chunks: req.total_chunks, created_ms: now_ms(), done: false };
    if tokio::fs::create_dir_all(dir_of(&id)).await.is_err() || save_meta(&meta).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "не удалось создать передачу");
    }
    Json(json!({"transfer_id": id, "chunk_max": MAX_CHUNK_BYTES})).into_response()
}

/// Залить один кусок (повторная заливка того же куска безопасна: так работает докачка).
pub async fn put_chunk(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((id, idx)): Path<(String, u32)>, body: Bytes) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    let Some(m) = load_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
    if m.from != me {
        return StatusCode::FORBIDDEN.into_response();
    }
    if m.done {
        return err(StatusCode::CONFLICT, "передача уже завершена");
    }
    if idx >= m.total_chunks || body.is_empty() || body.len() > MAX_CHUNK_BYTES {
        return err(StatusCode::BAD_REQUEST, "неверный номер или размер куска");
    }
    let d = dir_of(&id);
    let tmp = d.join(format!("{}.tmp", chunk_name(idx)));
    if tokio::fs::write(&tmp, &body).await.is_err() || tokio::fs::rename(&tmp, d.join(chunk_name(idx))).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить кусок");
    }
    StatusCode::OK.into_response()
}

async fn present(id: &str) -> Vec<u32> {
    let mut have = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir_of(id)).await else { return have };
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(n) = e.file_name().to_str() {
            if let Some(rest) = n.strip_prefix('c') {
                if let Ok(i) = rest.parse::<u32>() {
                    have.push(i);
                }
            }
        }
    }
    have.sort_unstable();
    have
}

/// Отправитель сообщает, что все куски залиты; узел проверяет, что ни одного не потеряно.
pub async fn done(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path(id): Path<String>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    let Some(mut m) = load_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
    if m.from != me {
        return StatusCode::FORBIDDEN.into_response();
    }
    let have = present(&id).await;
    let missing: Vec<u32> = (0..m.total_chunks).filter(|i| have.binary_search(i).is_err()).take(100).collect();
    if !missing.is_empty() {
        return (StatusCode::CONFLICT, Json(json!({"error": "не хватает кусков", "missing": missing}))).into_response();
    }
    m.done = true;
    if save_meta(&m).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "не удалось завершить передачу");
    }
    Json(json!({"ok": true})).into_response()
}

/// Состояние передачи: для докачки отправителю и для проверки получателю.
pub async fn status(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path(id): Path<String>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    let Some(m) = load_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
    if m.from != me && m.to != me {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(json!({"done": m.done, "total_chunks": m.total_chunks, "file_size": m.file_size, "have": present(&id).await})).into_response()
}

/// Готовые файлы, адресованные этому устройству.
pub async fn list(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    cleanup().await;
    let mut files: Vec<Meta> = all_meta().await.into_iter().filter(|m| m.done && m.to == me).collect();
    files.sort_by_key(|m| m.created_ms);
    let out: Vec<_> = files
        .iter()
        .map(|m| json!({"id": m.id, "from_peer_id": m.from, "file_name": m.file_name, "file_size": m.file_size, "total_chunks": m.total_chunks, "created_ms": m.created_ms}))
        .collect();
    Json(json!({"files": out})).into_response()
}

/// Скачать один кусок (получатель или отправитель, когда передача завершена).
pub async fn get_chunk(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((id, idx)): Path<(String, u32)>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    let Some(m) = load_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
    if m.from != me && m.to != me {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !m.done || idx >= m.total_chunks {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(dir_of(&id).join(chunk_name(idx))).await {
        Ok(b) => ([(header::CONTENT_TYPE, "application/octet-stream")], b).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Удалить передачу: получатель после скачивания, отправитель при отмене.
pub async fn remove(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path(id): Path<String>) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    let Some(m) = load_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
    if m.from != me && m.to != me {
        return StatusCode::FORBIDDEN.into_response();
    }
    if m.to == me && m.done && crate::mobile_api::is_node_peer(&m.from) {
        // телефон забрал файл, отправленный со страницы узла: там он станет «доставлено»
        crate::mobile_api::file_taken(&id);
    }
    let _ = tokio::fs::remove_dir_all(dir_of(&id)).await;
    StatusCode::OK.into_response()
}

// ── Узел сам как сторона передачи (компьютер ↔ телефон, `mobile_self`) ──

/// Готовая передача от этого устройства этому получателю: (число кусков, размер). Иначе None.
pub(crate) async fn ready_for(id: &str, from: &str, to: &str) -> Option<(u32, u64)> {
    let m = load_meta(id).await?;
    (m.done && m.from == from && m.to == to).then_some((m.total_chunks, m.file_size))
}

pub(crate) async fn read_chunk(id: &str, idx: u32) -> Option<Vec<u8>> {
    if !valid_id(id) {
        return None;
    }
    tokio::fs::read(dir_of(id).join(chunk_name(idx))).await.ok()
}

pub(crate) async fn drop_transfer(id: &str) {
    if valid_id(id) {
        let _ = tokio::fs::remove_dir_all(dir_of(id)).await;
    }
}

/// Узел начинает передачу телефону (те же лимиты, что и у телефонов).
pub(crate) async fn node_begin(from: &str, to: &str, file_size: u64, total_chunks: u32) -> Result<String, String> {
    if total_chunks == 0 || total_chunks > MAX_CHUNKS || file_size == 0 || file_size > MAX_FILE {
        return Err("файл слишком большой или пустой".into());
    }
    cleanup().await;
    let metas = all_meta().await;
    let used: u64 = metas.iter().map(|m| m.file_size + m.total_chunks as u64 * CHUNK_OVERHEAD).sum();
    if used + file_size + total_chunks as u64 * CHUNK_OVERHEAD > MAX_STORE {
        return Err("на узле нет места для файлов".into());
    }
    let id = {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut b);
        hex::encode(b)
    };
    let meta = Meta { id: id.clone(), from: from.to_string(), to: to.to_string(), file_name: "file".into(), file_size, total_chunks, created_ms: now_ms(), done: false };
    tokio::fs::create_dir_all(dir_of(&id)).await.map_err(|e| e.to_string())?;
    save_meta(&meta).await.map_err(|e| e.to_string())?;
    Ok(id)
}

pub(crate) async fn node_put(id: &str, idx: u32, bytes: &[u8]) -> Result<(), String> {
    if !valid_id(id) || bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
        return Err("неверный кусок".into());
    }
    let d = dir_of(id);
    let tmp = d.join(format!("{}.tmp", chunk_name(idx)));
    tokio::fs::write(&tmp, bytes).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, d.join(chunk_name(idx))).await.map_err(|e| e.to_string())
}

pub(crate) async fn node_finish(id: &str) -> Result<(), String> {
    let mut m = load_meta(id).await.ok_or("нет такой передачи")?;
    m.done = true;
    save_meta(&m).await.map_err(|e| e.to_string())
}
