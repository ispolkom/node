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
/// Самый большой кусок на проводе: до 1 МБ данных плюс служебное шифрования. Файлы от 5 МБ идут крупными кусками и параллельно (меньше
/// запросов — скорость упирается в канал, а не в задержку); мелкие по-прежнему по 256 КБ.
pub const MAX_CHUNK_BYTES: usize = (1 << 20) + 1024;
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
    /// абсолютный срок (мс) для файлов гостя: назначен при создании, не продлевается сменой роли и перезапуском (0 — нет)
    #[serde(default)]
    exp_ms: u64,
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

/// Начало передачи (проверка места и числа незавершённых + создание) идёт под одним замком: параллельные начала не превышают лимиты.
static BEGIN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Проверка занятого места и запись куска — под одним замком: в передаче не бывает больше байт, чем она объявила.
static PUT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn expired(m: &Meta) -> bool {
    // файлы гостя (отправителя или получателя) живут сутки от создания, а не неделю; срок записан при создании и от роли не зависит
    if m.exp_ms > 0 && now_ms() >= m.exp_ms {
        return true;
    }
    let guest = crate::mobile_api::is_guest_peer(&m.from) || crate::mobile_api::is_guest_peer(&m.to);
    now_ms().saturating_sub(m.created_ms) > if m.done && !guest && m.exp_ms == 0 { TTL_DONE_MS } else { TTL_PARTIAL_MS }
}

/// Срок файла: сутки, если на любом конце гость (иначе 0 — обычные сроки).
fn guest_exp(from: &str, to: &str) -> u64 {
    if crate::mobile_api::is_guest_peer(from) || crate::mobile_api::is_guest_peer(to) {
        now_ms() + 24 * 3600 * 1000
    } else {
        0
    }
}

/// Гостя удалили: его файлы (отправленные и ожидающие его) удаляются сразу, а не по сроку «обычного» файла.
pub async fn drop_peer_files(peer: String) {
    for m in all_meta().await {
        if m.from == peer || m.to == peer {
            let _ = tokio::fs::remove_dir_all(dir_of(&m.id)).await;
        }
    }
}

/// Передача, если она есть и не просрочена (просроченная сразу убирается: срок соблюдается при каждом обращении, а не только при уборке).
async fn live_meta(id: &str) -> Option<Meta> {
    let m = load_meta(id).await?;
    if expired(&m) {
        let _ = tokio::fs::remove_dir_all(dir_of(id)).await;
        return None;
    }
    Some(m)
}

/// Место, которое передача может занять на диске: объявленный размер плюс служебное шифрования каждого куска.
fn reserved(m: &Meta) -> u64 {
    m.file_size + m.total_chunks as u64 * CHUNK_OVERHEAD
}

/// Сколько байт уже лежит в залитых кусках передачи, не считая куска `except` (его сейчас перезаливают).
async fn stored_bytes(id: &str, except: u32) -> u64 {
    let mut sum = 0;
    for i in present(id).await {
        if i != except {
            sum += tokio::fs::metadata(dir_of(id).join(chunk_name(i))).await.map(|m| m.len()).unwrap_or(0);
        }
    }
    sum
}

/// Записать кусок, если с ним передача не превысит объявленного размера. Проверка, запись временного файла и переименование — под одним
/// замком: на диск не попадает ни байта сверх объявленного, даже временно и при параллельных запросах.
async fn store_chunk(m: &Meta, idx: u32, body: &[u8]) -> Result<(), (StatusCode, &'static str)> {
    let too_big = Err((StatusCode::PAYLOAD_TOO_LARGE, "куски больше объявленного размера файла"));
    if body.len() as u64 > reserved(m) {
        return too_big;
    }
    let d = dir_of(&m.id);
    let tmp = d.join(format!("{}.{:016x}.tmp", chunk_name(idx), rand::random::<u64>()));
    let _g = PUT.lock().await;
    // состояние перечитывается под замком: пока запрос ждал, DONE мог завершить передачу и открыть её получателю
    match load_meta(&m.id).await {
        Some(cur) if !cur.done => {}
        Some(_) => return Err((StatusCode::CONFLICT, "передача уже завершена")),
        None => return Err((StatusCode::NOT_FOUND, "передача не найдена")),
    }
    if stored_bytes(&m.id, idx).await + body.len() as u64 > reserved(m) {
        return too_big;
    }
    if tokio::fs::write(&tmp, body).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить кусок"));
    }
    if tokio::fs::rename(&tmp, d.join(chunk_name(idx))).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить кусок"));
    }
    Ok(())
}

/// Уборка по таймеру (из main): срок хранения соблюдается, даже если новых передач и запросов списка нет.
pub async fn cleanup_expired() {
    cleanup().await;
}

/// Убирает просроченное и осиротевшее. Вызывается при начале новой передачи, при чтении списка и по таймеру.
async fn cleanup() {
    let now = now_ms();
    let Ok(mut rd) = tokio::fs::read_dir(root()).await else { return };
    while let Ok(Some(e)) = rd.next_entry().await {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        let drop_it = match load_meta(&name).await {
            Some(m) => expired(&m),
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
    if crate::mobile_api::restricted_until(&dev) > 0 {
        return err(StatusCode::FORBIDDEN, "владелец узла ограничил отправку файлов для вас на время");
    }
    // получатель — другой телефон владельца или сам компьютер (переписка телефона с веб-чатом узла, см. mobile_self)
    if req.to_peer_id == me || !(is_device_peer(&req.to_peer_id) || crate::mobile_api::is_node_peer(&req.to_peer_id)) {
        return err(StatusCode::BAD_REQUEST, "файлы можно отправлять только на другое сопряжённое устройство");
    }
    // политика содержимого: только разрешённые типы файлов (имя проверяется целиком, содержимое — приложением до шифрования)
    // анонимное имя «file» шлёт само приложение (настоящее имя — внутри зашифрованного предложения)
    if req.file_name != "file" {
        if let Err(why) = crate::content_policy::attachment_allowed(&req.file_name) {
            return err(StatusCode::UNSUPPORTED_MEDIA_TYPE, &why);
        }
    }
    if req.total_chunks == 0 || req.total_chunks > MAX_CHUNKS || req.file_size == 0 || req.file_size > MAX_FILE {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "файл слишком большой или пустой");
    }
    // гость (сопряжён без QR): файлы только другим гостям и пользователям узла, не больше 30 МБ, 20 файлов и 300 МБ в сутки
    if crate::mobile_api::is_guest_device(&dev) {
        if !crate::mobile_api::is_restricted_peer(&req.to_peer_id) {
            return err(StatusCode::FORBIDDEN, "гость может передавать файлы только другим гостям и пользователям этого узла");
        }
        if let Err(why) = crate::guest::reserve_file(&dev, req.file_size) {
            return err(StatusCode::TOO_MANY_REQUESTS, why);
        }
    }
    // кусков не меньше, чем нужно для размера, и не вдвое больше
    let per = req.file_size.div_ceil(req.total_chunks as u64);
    if per > MAX_CHUNK_BYTES as u64 - CHUNK_OVERHEAD {
        return err(StatusCode::BAD_REQUEST, "куски слишком крупные");
    }
    let _g = BEGIN.lock().await;
    cleanup().await;
    let metas = all_meta().await;
    let used: u64 = metas.iter().map(reserved).sum();
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
    let exp_ms = guest_exp(&me, &req.to_peer_id);
    let meta = Meta { id: id.clone(), from: me, to: req.to_peer_id, file_name: label, file_size: req.file_size, total_chunks: req.total_chunks, created_ms: now_ms(), done: false, exp_ms };
    if tokio::fs::create_dir_all(dir_of(&id)).await.is_err() || save_meta(&meta).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "не удалось создать передачу");
    }
    Json(json!({"transfer_id": id, "chunk_max": MAX_CHUNK_BYTES})).into_response()
}

/// Залить один кусок (повторная заливка того же куска безопасна: так работает докачка).
pub async fn put_chunk(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((id, idx)): Path<(String, u32)>, body: Bytes) -> Response {
    let Some(me) = device_peer_of(&dev) else { return StatusCode::UNAUTHORIZED.into_response() };
    if crate::mobile_api::restricted_until(&dev) > 0 {
        return err(StatusCode::FORBIDDEN, "владелец узла ограничил отправку файлов для вас на время");
    }
    let Some(m) = live_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
    if m.from != me {
        return StatusCode::FORBIDDEN.into_response();
    }
    if m.done {
        return err(StatusCode::CONFLICT, "передача уже завершена");
    }
    if idx >= m.total_chunks || body.is_empty() || body.len() > MAX_CHUNK_BYTES {
        return err(StatusCode::BAD_REQUEST, "неверный номер или размер куска");
    }
    match store_chunk(&m, idx, &body).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err((code, msg)) => err(code, msg),
    }
}

async fn present(id: &str) -> Vec<u32> {
    let mut have = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir_of(id)).await else { return have };
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(n) = e.file_name().to_str() {
            if let Some(rest) = n.strip_prefix('c') {
                if let Ok(i) = rest.parse::<u32>() {
                    have.push(i); // временные «c000001.….tmp» сюда не попадают: не разбираются как число
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
    if crate::mobile_api::restricted_until(&dev) > 0 {
        return err(StatusCode::FORBIDDEN, "владелец узла ограничил отправку файлов для вас на время");
    }
    let Some(mut m) = live_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
    if m.from != me {
        return StatusCode::FORBIDDEN.into_response();
    }
    // завершение и запись куска исключают друг друга: набор кусков в момент DONE — окончательный
    let _g = PUT.lock().await;
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
    let Some(m) = live_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
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
    let Some(m) = live_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
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
    let Some(m) = live_meta(&id).await else { return StatusCode::NOT_FOUND.into_response() };
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
    let m = live_meta(id).await?;
    (m.done && m.from == from && m.to == to).then_some((m.total_chunks, m.file_size))
}

pub(crate) async fn read_chunk(id: &str, idx: u32) -> Option<Vec<u8>> {
    let m = live_meta(id).await?;
    if !m.done || idx >= m.total_chunks {
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
    let _g = BEGIN.lock().await;
    cleanup().await;
    let metas = all_meta().await;
    let used: u64 = metas.iter().map(reserved).sum();
    if used + file_size + total_chunks as u64 * CHUNK_OVERHEAD > MAX_STORE {
        return Err("на узле нет места для файлов".into());
    }
    if metas.iter().filter(|m| m.from == from && !m.done).count() >= MAX_PENDING_PER_SENDER {
        return Err("слишком много незавершённых передач".into());
    }
    let id = {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut b);
        hex::encode(b)
    };
    let meta = Meta { id: id.clone(), from: from.to_string(), to: to.to_string(), file_name: "file".into(), file_size, total_chunks, created_ms: now_ms(), done: false, exp_ms: guest_exp(from, to) };
    tokio::fs::create_dir_all(dir_of(&id)).await.map_err(|e| e.to_string())?;
    save_meta(&meta).await.map_err(|e| e.to_string())?;
    Ok(id)
}

pub(crate) async fn node_put(id: &str, idx: u32, bytes: &[u8]) -> Result<(), String> {
    if !valid_id(id) || bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
        return Err("неверный кусок".into());
    }
    let m = live_meta(id).await.ok_or("нет такой передачи")?;
    if m.done || idx >= m.total_chunks {
        return Err("неверный кусок".into());
    }
    store_chunk(&m, idx, bytes).await.map_err(|(_, msg)| msg.to_string())
}

pub(crate) async fn node_finish(id: &str) -> Result<(), String> {
    let mut m = live_meta(id).await.ok_or("нет такой передачи")?;
    let have = present(id).await;
    if (0..m.total_chunks).any(|i| have.binary_search(&i).is_err()) {
        return Err("не хватает кусков".into());
    }
    m.done = true;
    save_meta(&m).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(done: bool, age_ms: u64) -> Meta {
        Meta { id: "ab".repeat(16), from: "a".into(), to: "b".into(), file_name: String::new(), file_size: 2500, total_chunks: 3, created_ms: now_ms() - age_ms, done, exp_ms: 0 }
    }

    #[test]
    fn transfers_expire_by_their_own_terms() {
        let hour = 3600 * 1000;
        assert!(!expired(&meta(true, 6 * 24 * hour)));
        assert!(expired(&meta(true, 8 * 24 * hour)), "готовая — неделя");
        assert!(!expired(&meta(false, 23 * hour)));
        assert!(expired(&meta(false, 25 * hour)), "недокачанная — сутки");
    }

    #[test]
    fn a_transfer_reserves_its_size_plus_encryption_of_each_chunk() {
        assert_eq!(reserved(&meta(false, 0)), 2500 + 3 * 28);
    }
}

#[cfg(test)]
mod guest_ttl_tests {
    use super::*;

    #[test]
    fn a_guests_ready_file_expires_in_a_day_but_an_ordinary_one_lives_longer() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::complaints::tests::fresh();
        let guest = crate::mobile_api::test_add_device_with_role("guest", "ttg");
        let user = crate::mobile_api::test_add_device_with_role("user", "ttu");
        let other = crate::mobile_api::test_add_device_with_role("user", "tto");
        let (g, u, o) = (crate::mobile_api::device_peer_of(&guest).unwrap(), crate::mobile_api::device_peer_of(&user).unwrap(), crate::mobile_api::device_peer_of(&other).unwrap());
        let meta = |from: &str, to: &str, age_h: u64| Meta { id: "a".repeat(32), from: from.into(), to: to.into(), file_name: "f".into(), file_size: 1, total_chunks: 1, created_ms: now_ms() - age_h * 3600 * 1000, done: true, exp_ms: 0 };
        assert!(expired(&meta(&g, &u, 25)), "файл от гостя старше суток просрочен");
        assert!(expired(&meta(&u, &g, 25)), "и файл гостю тоже");
        assert!(!expired(&meta(&g, &u, 23)));
        assert!(!expired(&meta(&u, &o, 48)), "обычные устройства: срок прежний, неделя");
        assert!(expired(&meta(&u, &o, 24 * 8)));
    }

    #[test]
    fn a_guest_files_term_is_written_at_creation_and_does_not_depend_on_who_is_a_guest_later() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::complaints::tests::fresh();
        let g = crate::mobile_api::test_add_device_with_role("guest", "etg");
        let u = crate::mobile_api::test_add_device_with_role("user", "etu");
        let (gp, up) = (crate::mobile_api::device_peer_of(&g).unwrap(), crate::mobile_api::device_peer_of(&u).unwrap());
        assert!(guest_exp(&gp, &up) > now_ms(), "файл гостя получает срок");
        assert_eq!(guest_exp(&up, &"z".repeat(64)), 0, "обычный файл — без гостевого срока");
        // срок прошёл, а гостя в памяти уже нет (перезапуск, принят в пользователи): файл всё равно просрочен
        let m = Meta { id: "a".repeat(32), from: up.clone(), to: "z".repeat(64), file_name: "f".into(), file_size: 1, total_chunks: 1, created_ms: now_ms() - 25 * 3600 * 1000, done: true, exp_ms: now_ms() - 1000 };
        assert!(expired(&m));
        // срок ещё не прошёл — файл жив, и неделя «обычного» срока его не продлевает сверх суток от создания
        let live = Meta { exp_ms: now_ms() + 3600 * 1000, created_ms: now_ms() - 3600 * 1000, ..m.clone() };
        assert!(!expired(&live));
        let stale = Meta { exp_ms: now_ms() + 3600 * 1000, created_ms: now_ms() - 30 * 3600 * 1000, ..m };
        assert!(expired(&stale));
    }
}

