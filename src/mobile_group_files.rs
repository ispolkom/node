//! Файлы групп (`/mobile/groups/{id}/files...`, договорённость — docs/CLIENT_WIRE.md «Группы»).
//!
//! Как и в личных передачах, узел хранит только шифртекст: отправитель шифрует каждый кусок сам (ключ файла уходит участникам внутри
//! зашифрованной записи журнала группы), поэтому узел не знает ни имени, ни содержимого. Файлы лежат отдельно от журнала, у каждой
//! группы своя папка и свой потолок объёма, срок хранения ограничен. Забрать файл может только участник группы (не выгнанный и не
//! запрещённый); стереть — отправитель, модератор или владелец группы. Удалили группу — удалились её файлы.
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::mobile_api::{device_peer_of, DeviceKey};
use crate::mobile_files::{MAX_CHUNKS, MAX_CHUNK_BYTES};
use crate::mobile_groups::{member_state, valid_gid, Role};

/// Самый большой файл в группе (в открытом виде), байт.
pub const MAX_GROUP_FILE: u64 = 200 << 20;
/// Потолок файлов одной группы и всех групп узла.
const MAX_PER_GROUP: u64 = 2 << 30;
const MAX_ALL_GROUPS: u64 = 8 << 30;
const MAX_PENDING_PER_SENDER: usize = 5;
/// Готовый файл хранится месяц, недокачанный — сутки.
const TTL_DONE_MS: u64 = 30 * 24 * 3600 * 1000;
const TTL_PARTIAL_MS: u64 = 24 * 3600 * 1000;
const CHUNK_OVERHEAD: u64 = 28;

#[derive(Clone, Serialize, Deserialize)]
struct Meta {
    id: String,
    /// кто залил (хэш участника в группе, а не номер устройства)
    by: String,
    file_size: u64,
    total_chunks: u32,
    created_ms: u64,
    done: bool,
}

fn root() -> PathBuf {
    crate::util::data_dir::data_dir().join("groups").join("files")
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn group_dir(gid: &str) -> PathBuf {
    root().join(gid)
}

fn dir_of(gid: &str, id: &str) -> PathBuf {
    group_dir(gid).join(id)
}

fn chunk_name(idx: u32) -> String {
    format!("c{idx:06}")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"error": msg}))).into_response()
}

/// Группа удалена или участник вышел из дел: убираем файлы группы целиком.
pub(crate) fn drop_group(gid: &str) {
    if valid_gid(gid) {
        let _ = std::fs::remove_dir_all(group_dir(gid));
    }
}

async fn load_meta(gid: &str, id: &str) -> Option<Meta> {
    if !valid_gid(gid) || !valid_id(id) {
        return None;
    }
    let s = tokio::fs::read_to_string(dir_of(gid, id).join("meta.json")).await.ok()?;
    serde_json::from_str(&s).ok()
}

async fn save_meta(gid: &str, m: &Meta) -> std::io::Result<()> {
    let d = dir_of(gid, &m.id);
    let tmp = d.join("meta.json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(m).unwrap_or_default()).await?;
    tokio::fs::rename(&tmp, d.join("meta.json")).await
}

async fn metas_of(gid: &str) -> Vec<Meta> {
    let mut out = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(group_dir(gid)).await else { return out };
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(name) = e.file_name().to_str() {
            if let Some(m) = load_meta(gid, name).await {
                out.push(m);
            }
        }
    }
    out
}

/// Самый большой кусок этой заливки на проводе. Приложения режут файл на куски одного размера C (последний короче), значит
/// C < размер / (кусков − 1): больше быть не может. Плюс служебное шифрования. Сумма сохранённого дополнительно проверяется при завершении,
/// так что фактические байты не превысят заявленный размер, по которому считаются квоты.
fn chunk_limit(m: &Meta) -> usize {
    let per = if m.total_chunks <= 1 { m.file_size } else { m.file_size.div_ceil(m.total_chunks as u64 - 1) };
    (per + CHUNK_OVERHEAD).min(MAX_CHUNK_BYTES as u64) as usize
}

/// Временное имя куска — своё у каждой заливки куска (одинаковые параллельные заливки одного куска не мешают друг другу).
fn tmp_name(idx: u32) -> String {
    use rand::RngCore;
    let mut b = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut b);
    format!("{}.{}.tmp", chunk_name(idx), hex::encode(b))
}

/// Влезет ли кусок idx размером len в заявленный размер файла с учётом уже сохранённых (заменяемый кусок того же номера не считается).
/// Проверка и запись куска — под общим замком, чтобы параллельные куски одной заливки не превысили лимит вместе.
static PUT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn fits(gid: &str, id: &str, m: &Meta, idx: u32, len: usize) -> bool {
    let same = tokio::fs::metadata(dir_of(gid, id).join(chunk_name(idx))).await.map(|x| x.len()).unwrap_or(0);
    stored_bytes(gid, id).await - same + len as u64 <= m.file_size + m.total_chunks as u64 * CHUNK_OVERHEAD
}

/// Суммарный объём сохранённых кусков (на диске).
async fn stored_bytes(gid: &str, id: &str) -> u64 {
    let mut total = 0;
    let Ok(mut rd) = tokio::fs::read_dir(dir_of(gid, id)).await else { return 0 };
    while let Ok(Some(e)) = rd.next_entry().await {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n.starts_with('c') && !n.ends_with(".tmp") {
            total += e.metadata().await.map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

fn used(metas: &[Meta]) -> u64 {
    metas.iter().map(|m| m.file_size + m.total_chunks as u64 * CHUNK_OVERHEAD).sum()
}

async fn used_everywhere() -> u64 {
    let mut total = 0;
    let Ok(mut rd) = tokio::fs::read_dir(root()).await else { return 0 };
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(g) = e.file_name().to_str() {
            total += used(&metas_of(g).await);
        }
    }
    total
}

fn expired(m: &Meta) -> bool {
    now_ms().saturating_sub(m.created_ms) > if m.done { TTL_DONE_MS } else { TTL_PARTIAL_MS }
}

/// Убирает просроченное во всех группах (вызывается по таймеру): срок хранения соблюдается, даже если в группу больше ничего не заливают.
pub async fn cleanup_all() {
    let Ok(mut rd) = tokio::fs::read_dir(root()).await else { return };
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(g) = e.file_name().to_str() {
            if valid_gid(g) {
                cleanup(g).await;
            }
        }
    }
}

/// Убирает просроченное и осиротевшее в одной группе.
async fn cleanup(gid: &str) {
    let now = now_ms();
    let Ok(mut rd) = tokio::fs::read_dir(group_dir(gid)).await else { return };
    while let Ok(Some(e)) = rd.next_entry().await {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        let drop_it = match load_meta(gid, &name).await {
            Some(m) => now.saturating_sub(m.created_ms) > if m.done { TTL_DONE_MS } else { TTL_PARTIAL_MS },
            None => e.metadata().await.ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()).unwrap_or(0) > 24 * 3600,
        };
        if drop_it {
            let _ = tokio::fs::remove_dir_all(e.path()).await;
        }
    }
}

/// (устройство → участник группы, его роль и заглушён ли); иначе ответ-отказ.
fn member(gid: &str, dev: &str) -> Result<(String, Role, bool), Response> {
    if !valid_gid(gid) {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    let me = device_peer_of(dev).ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;
    let (key, role, muted) = member_state(gid, &me).ok_or_else(|| err(StatusCode::FORBIDDEN, "вы не участник этой группы"))?;
    Ok((key, role, muted))
}

#[derive(Deserialize)]
pub struct StartReq {
    file_size: u64,
    total_chunks: u32,
}

/// Общая часть начала заливки (телефон и компьютер-участник): проверки размера, квот и числа незавершённых заливок.
async fn begin(gid: &str, key: &str, muted: bool, file_size: u64, total_chunks: u32) -> Result<String, (StatusCode, &'static str)> {
    if muted {
        return Err((StatusCode::FORBIDDEN, "вы заглушены в этой группе"));
    }
    if total_chunks == 0 || total_chunks > MAX_CHUNKS || file_size == 0 || file_size > MAX_GROUP_FILE {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "файл слишком большой или пустой (в группе до 200 МБ)"));
    }
    let per = file_size.div_ceil(total_chunks as u64);
    if per > MAX_CHUNK_BYTES as u64 - CHUNK_OVERHEAD {
        return Err((StatusCode::BAD_REQUEST, "куски слишком крупные"));
    }
    if total_chunks as u64 > file_size {
        return Err((StatusCode::BAD_REQUEST, "кусков больше, чем байт в файле"));
    }
    // проверка квот и создание заливки — под одним замком: параллельные начала не превысят лимит вместе
    static BEGIN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = BEGIN.lock().await;
    cleanup(gid).await;
    let metas = metas_of(gid).await;
    let need = file_size + total_chunks as u64 * CHUNK_OVERHEAD;
    if used(&metas) + need > MAX_PER_GROUP || used_everywhere().await + need > MAX_ALL_GROUPS {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "в группе нет места для файлов"));
    }
    if metas.iter().filter(|m| m.by == key && !m.done).count() >= MAX_PENDING_PER_SENDER {
        return Err((StatusCode::TOO_MANY_REQUESTS, "слишком много незавершённых заливок"));
    }
    let id = {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut b);
        hex::encode(b)
    };
    let meta = Meta { id: id.clone(), by: key.to_string(), file_size, total_chunks, created_ms: now_ms(), done: false };
    if tokio::fs::create_dir_all(dir_of(gid, &id)).await.is_err() || save_meta(gid, &meta).await.is_err() {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "не удалось создать заливку"));
    }
    Ok(id)
}

/// Начать заливку файла в группу.
pub async fn start(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path(gid): Path<String>, Json(req): Json<StartReq>) -> Response {
    if crate::mobile_api::restricted_until(&dev) > 0 {
        return err(StatusCode::FORBIDDEN, "владелец узла ограничил отправку файлов для вас на время");
    }
    let (key, _, muted) = match member(&gid, &dev) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match begin(&gid, &key, muted, req.file_size, req.total_chunks).await {
        Ok(id) => Json(json!({"file_id": id, "chunk_max": MAX_CHUNK_BYTES})).into_response(),
        Err((code, msg)) => err(code, msg),
    }
}

/// Залить кусок (повтор безопасен — так работает докачка). Только тот, кто начал заливку.
pub async fn put_chunk(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((gid, id, idx)): Path<(String, String, u32)>, body: Bytes) -> Response {
    if crate::mobile_api::restricted_until(&dev) > 0 {
        return err(StatusCode::FORBIDDEN, "владелец узла ограничил отправку файлов для вас на время");
    }
    let (key, _, muted) = match member(&gid, &dev) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(m) = load_meta(&gid, &id).await.filter(|m| !expired(m)) else { return StatusCode::NOT_FOUND.into_response() };
    if m.by != key || muted {
        return StatusCode::FORBIDDEN.into_response();
    }
    if m.done {
        return err(StatusCode::CONFLICT, "заливка уже завершена");
    }
    if idx >= m.total_chunks || body.is_empty() || body.len() > chunk_limit(&m) {
        return err(StatusCode::BAD_REQUEST, "неверный номер или размер куска");
    }
    let _guard = PUT.lock().await;
    if !fits(&gid, &id, &m, idx, body.len()).await {
        return err(StatusCode::BAD_REQUEST, "куски больше заявленного размера файла");
    }
    let d = dir_of(&gid, &id);
    let tmp = d.join(tmp_name(idx));
    if tokio::fs::write(&tmp, &body).await.is_err() || tokio::fs::rename(&tmp, d.join(chunk_name(idx))).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить кусок");
    }
    StatusCode::OK.into_response()
}

async fn present(gid: &str, id: &str) -> Vec<u32> {
    let mut have = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir_of(gid, id)).await else { return have };
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(i) = e.file_name().to_str().and_then(|n| n.strip_prefix('c')).and_then(|r| r.parse::<u32>().ok()) {
            have.push(i);
        }
    }
    have.sort_unstable();
    have
}

/// Все куски залиты — файл становится доступным участникам.
pub async fn done(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((gid, id)): Path<(String, String)>) -> Response {
    if crate::mobile_api::restricted_until(&dev) > 0 {
        return err(StatusCode::FORBIDDEN, "владелец узла ограничил отправку файлов для вас на время");
    }
    let (key, _, _) = match member(&gid, &dev) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(mut m) = load_meta(&gid, &id).await.filter(|m| !expired(m)) else { return StatusCode::NOT_FOUND.into_response() };
    if m.by != key {
        return StatusCode::FORBIDDEN.into_response();
    }
    let have = present(&gid, &id).await;
    let missing: Vec<u32> = (0..m.total_chunks).filter(|i| have.binary_search(i).is_err()).take(100).collect();
    if !missing.is_empty() {
        return (StatusCode::CONFLICT, Json(json!({"error": "не хватает кусков", "missing": missing}))).into_response();
    }
    if stored_bytes(&gid, &id).await > m.file_size + m.total_chunks as u64 * CHUNK_OVERHEAD {
        return err(StatusCode::BAD_REQUEST, "залито больше, чем заявлено");
    }
    m.done = true;
    if save_meta(&gid, &m).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "не удалось завершить заливку");
    }
    Json(json!({"ok": true})).into_response()
}

/// Состояние файла: для докачки и для проверки перед скачиванием.
pub async fn status(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((gid, id)): Path<(String, String)>) -> Response {
    if let Err(r) = member(&gid, &dev) {
        return r;
    }
    let Some(m) = load_meta(&gid, &id).await.filter(|m| !expired(m)) else { return StatusCode::NOT_FOUND.into_response() };
    Json(json!({"done": m.done, "total_chunks": m.total_chunks, "file_size": m.file_size, "have": present(&gid, &id).await})).into_response()
}

/// Скачать кусок (любой участник группы, когда файл готов).
pub async fn get_chunk(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((gid, id, idx)): Path<(String, String, u32)>) -> Response {
    if let Err(r) = member(&gid, &dev) {
        return r;
    }
    let Some(m) = load_meta(&gid, &id).await.filter(|m| !expired(m)) else { return StatusCode::NOT_FOUND.into_response() };
    if !m.done || idx >= m.total_chunks {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(dir_of(&gid, &id).join(chunk_name(idx))).await {
        Ok(b) => ([(header::CONTENT_TYPE, "application/octet-stream")], b).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Стереть файл: тот, кто залил, либо владелец/модератор группы.
pub async fn remove(axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>, Path((gid, id)): Path<(String, String)>) -> Response {
    let (key, role, _) = match member(&gid, &dev) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(m) = load_meta(&gid, &id).await else { return StatusCode::NOT_FOUND.into_response() };
    // чужой файл стирает тот, кто выше автора по роли (как и записи): модератор не трогает файлы админа и владельца
    if m.by != key && !crate::mobile_groups::may_override(role, crate::mobile_groups::role_of_key(&gid, &m.by)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let _ = tokio::fs::remove_dir_all(dir_of(&gid, &id)).await;
    StatusCode::OK.into_response()
}

// ── Компьютер как участник группы (`group_client`): те же проверки, в самом узле, без сети ──

fn pc_member(gid: &str, device: &str) -> Result<(String, Role, bool), String> {
    if !valid_gid(gid) {
        return Err("нет такой группы".into());
    }
    member_state(gid, device).ok_or_else(|| "вы не участник этой группы".to_string())
}

pub(crate) async fn pc_begin(gid: &str, device: &str, file_size: u64, total_chunks: u32) -> Result<String, String> {
    let (key, _, muted) = pc_member(gid, device)?;
    begin(gid, &key, muted, file_size, total_chunks).await.map_err(|(_, m)| m.to_string())
}

pub(crate) async fn pc_put(gid: &str, device: &str, id: &str, idx: u32, bytes: &[u8]) -> Result<(), String> {
    let (key, _, muted) = pc_member(gid, device)?;
    let m = load_meta(gid, id).await.filter(|m| !expired(m)).ok_or("нет такой заливки")?;
    if m.by != key || muted || m.done || idx >= m.total_chunks || bytes.is_empty() || bytes.len() > chunk_limit(&m) {
        return Err("кусок не принят".into());
    }
    let _guard = PUT.lock().await;
    if !fits(gid, id, &m, idx, bytes.len()).await {
        return Err("куски больше заявленного размера файла".into());
    }
    let d = dir_of(gid, id);
    let tmp = d.join(tmp_name(idx));
    tokio::fs::write(&tmp, bytes).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, d.join(chunk_name(idx))).await.map_err(|e| e.to_string())
}

pub(crate) async fn pc_done(gid: &str, device: &str, id: &str) -> Result<(), String> {
    let (key, _, _) = pc_member(gid, device)?;
    let mut m = load_meta(gid, id).await.filter(|m| !expired(m)).ok_or("нет такой заливки")?;
    if m.by != key {
        return Err("чужая заливка".into());
    }
    let have = present(gid, id).await;
    if (0..m.total_chunks).any(|i| have.binary_search(&i).is_err()) {
        return Err("не хватает кусков".into());
    }
    if stored_bytes(gid, id).await > m.file_size + m.total_chunks as u64 * CHUNK_OVERHEAD {
        return Err("залито больше, чем заявлено".into());
    }
    m.done = true;
    save_meta(gid, &m).await.map_err(|e| e.to_string())
}

/// Кусок готового файла для участника группы.
pub(crate) async fn pc_chunk(gid: &str, device: &str, id: &str, idx: u32) -> Result<Vec<u8>, String> {
    pc_member(gid, device)?;
    let m = load_meta(gid, id).await.filter(|m| !expired(m)).ok_or("файла больше нет на узле (стёрт или истёк срок)")?;
    if !m.done || idx >= m.total_chunks {
        return Err("файл не готов".into());
    }
    tokio::fs::read(dir_of(gid, id).join(chunk_name(idx))).await.map_err(|_| "кусок не найден".to_string())
}

pub(crate) async fn pc_remove(gid: &str, device: &str, id: &str) -> Result<(), String> {
    let (key, role, _) = pc_member(gid, device)?;
    let m = load_meta(gid, id).await.ok_or("нет такого файла")?;
    if m.by != key && !crate::mobile_groups::may_override(role, crate::mobile_groups::role_of_key(gid, &m.by)) {
        return Err("файл стирает залившего или тот, кто выше него по роли в группе".into());
    }
    let _ = tokio::fs::remove_dir_all(dir_of(gid, id)).await;
    Ok(())
}
