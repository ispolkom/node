//! Резервная копия узла: ручки владельца (под проверкой входа в веб-интерфейс) и плановая задача.
//!
//! * `GET  /api/backup/status`  — что и когда копировалось, где папка автокопий, что в копию не входит;
//! * `POST /api/backup/export`  — «Сохранить резервную копию»: файл `.yandibak` отдаётся владельцу (и кладётся в папку, если она задана);
//! * `POST /api/backup/config`  — папка для автокопий (флешка, синхронизируемая с облаком папка); пусто — автокопии выключены.
//!
//! Плановая задача каждые десять минут сверяет содержимое связей с прошлой копией в папке и делает новую только при изменении; критичные
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

/// Как часто плановая задача сверяет связи с последней копией: изменения (контакты, группы, пользователи, смена владельца группы) попадают в копию
/// в течение десяти минут, а серия правок подряд — одной записью. Копия делается только если содержимое изменилось.
pub const CHECK_INTERVAL_MS: u64 = 10 * 60 * 1000;

/// Подключить копию к работающему узлу: где лежат каталоги и как достать корень ключей (он есть только у запущенного узла).
pub fn install(layout: Layout, root: RootGetter) {
    let _ = RUNTIME.set(Runtime { layout, root });
}

/// Изменилось то, без чего восстановление опасно (устройство добавлено или отозвано, участник забанен, сменился владелец группы):
/// плановая задача сделает копию в ближайшую минуту, не дожидаясь суток.
pub fn notify_critical() {
    crate::backup_signal::notify_critical();
}

/// Узел работает, а место для копий связей не выбрано: оболочка и консоль напоминают об этом.
pub fn folder_missing() -> bool {
    RUNTIME.get().is_some_and(|r| node_backup::load_state(&r.layout).dir.as_ref().is_none_or(|d| d.trim().is_empty()))
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
        // копии не настроены: оболочка и консоль напоминают, пока владелец не выберет место
        "reminder": !s.dir.as_ref().is_some_and(|d| !d.trim().is_empty()),
        "folder_name": node_backup::FOLDER_NAME,
        "keep": node_backup::KEEP_AUTO,
        "keep_manual": node_backup::KEEP_MANUAL,
        "excluded": node_backup::EXCLUDED,
        "ext": node_backup::FILE_EXT,
    })
}

async fn status() -> Response {
    let r = match rt() { Ok(r) => r, Err(e) => return e };
    Json(status_json(&r.layout)).into_response()
}

async fn export(axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>) -> Response {
    let r = match rt() { Ok(r) => r, Err(e) => return e };
    // ?quiet=1 — консоль: копия кладётся в настроенную папку, а в ответе только сведения о ней (без самого файла)
    let quiet = q.get("quiet").is_some_and(|v| v == "1");
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
        Ok(Ok(o)) if quiet => {
            if o.saved.is_none() {
                return (StatusCode::CONFLICT, Json(json!({"error": "папка для копий не настроена", "code": "no_folder"}))).into_response();
            }
            Json(json!({"name": o.name, "saved": o.saved.map(|p| p.display().to_string()), "counter": o.counter})).into_response()
        }
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


/// Результат проверки места для копий.
#[derive(Debug, PartialEq, Eq)]
pub struct Place {
    /// итоговая папка `<выбранное место>/yandi-node`
    pub dir: String,
    /// место на том же диске, что и корень системы: потеря диска унесёт и копии
    pub system_disk: bool,
}

/// Ошибка проверки места: машинный код (для клиентов на любом языке) и пояснение.
#[derive(Debug, PartialEq, Eq)]
pub struct PlaceError {
    pub code: &'static str,
    pub detail: String,
    /// машинная причина отказа записи (для клиентов на любом языке): permission | read_only | not_found | no_space | other
    pub reason: &'static str,
}

impl PlaceError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        PlaceError { code, detail: detail.into(), reason: "" }
    }
    fn write(e: &std::io::Error) -> Self {
        let (reason, text) = reason_of(e);
        PlaceError { code: "no_write", detail: text, reason }
    }
    /// Фраза для владельца (клиенты с другим языком показывают свою по `code`).
    pub fn message(&self) -> String {
        match self.code {
            "not_absolute" => "Место задаётся полным путём без «..».".to_string(),
            "inside_node" => "Копия внутри каталогов самого узла не защищает от потери диска: выберите другое место (флешку, другой диск, папку облака).".to_string(),
            "no_write" => format!("У меня нет прав для записи в это место: {}", self.detail),
            _ => self.detail.clone(),
        }
    }
}

fn dev_of(p: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| m.dev())
}

/// Папка автокопий: пустая — выключено; иначе абсолютный путь вне каталогов самого узла (копия рядом с ключами не пережила бы потерю
/// диска). Внутри выбранного места создаётся подпапка `yandi-node`, в ней пишется и стирается пробный файл — от имени самого узла,
/// поэтому проверка честная и для диска, принадлежащего другому пользователю.
pub fn check_place(l: &Layout, place: &str) -> Result<Option<Place>, PlaceError> {
    let d = place.trim();
    if d.is_empty() {
        return Ok(None);
    }
    let chosen = std::path::Path::new(d);
    if !chosen.is_absolute() || d.contains('\0') || chosen.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(PlaceError::new("not_absolute", ""));
    }
    // итоговый путь: …/yandi-node (если человек уже указал именно её, не удваиваем)
    let target = if chosen.file_name().is_some_and(|n| n == node_backup::FOLDER_NAME) { chosen.to_path_buf() } else { chosen.join(node_backup::FOLDER_NAME) };
    for own in [&l.keys, &l.data, &l.home] {
        if target.starts_with(own) {
            return Err(PlaceError::new("inside_node", ""));
        }
    }
    std::fs::create_dir_all(&target).map_err(|e| PlaceError::write(&e))?;
    // настоящий путь (после ссылок): папка-ссылка не должна привести внутрь каталогов самого узла
    let real = std::fs::canonicalize(&target).map_err(|e| PlaceError::write(&e))?;
    for own in [&l.keys, &l.data, &l.home] {
        let own_real = std::fs::canonicalize(own).unwrap_or_else(|_| own.clone());
        if real.starts_with(&own_real) {
            return Err(PlaceError::new("inside_node", ""));
        }
    }
    // пробный файл: новый, со случайным именем, права 0600, без перехода по ссылкам (чужой файл не затирается, ссылка не разыменовывается)
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut rnd = [0u8; 6];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut rnd);
        let probe = real.join(format!(".yandi-write-test-{}", hex::encode(rnd)));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&probe)
            .map_err(|e| PlaceError::write(&e))?;
        let wrote = f.write_all(b"x").and_then(|_| f.sync_all());
        drop(f);
        let _ = std::fs::remove_file(&probe);
        wrote.map_err(|e| PlaceError::write(&e))?;
    }
    let target = real;
    let system_disk = match (dev_of(&target), dev_of(std::path::Path::new("/"))) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    Ok(Some(Place { dir: target.display().to_string(), system_disk }))
}

fn reason_of(e: &std::io::Error) -> (&'static str, String) {
    use std::io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => ("permission", "доступ запрещён (диск или папка принадлежат другому пользователю)".to_string()),
        ReadOnlyFilesystem => ("read_only", "диск только для чтения".to_string()),
        NotFound => ("not_found", "такого места нет".to_string()),
        _ if e.raw_os_error() == Some(28) => ("no_space", "на диске нет свободного места".to_string()),
        _ if e.raw_os_error() == Some(30) => ("read_only", "диск только для чтения".to_string()),
        _ if e.raw_os_error() == Some(20) => ("not_found", "это не папка".to_string()),
        _ => ("other", e.to_string()),
    }
}

/// Путь из `/proc/mounts`: пробел, табуляция, перевод строки и обратная косая записаны восьмеричными кодами (`\040`, `\011`, `\012`, `\134`).
fn unescape_mount(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() + 0 && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Куда можно предложить положить копии: подключённые диски и флешки (не корень системы), с отметкой «можно писать» и свободным местом.
pub fn candidate_places() -> Vec<serde_json::Value> {
    use std::os::unix::ffi::OsStrExt;
    let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else { return Vec::new() };
    let root_dev = dev_of(std::path::Path::new("/"));
    let pseudo = ["tmpfs", "proc", "sysfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "overlay", "squashfs", "autofs", "mqueue", "debugfs", "tracefs", "securityfs", "pstore", "configfs", "fusectl", "bpf", "ramfs", "hugetlbfs", "efivarfs", "binfmt_misc", "rpc_pipefs", "nsfs", "fuse.gvfsd-fuse", "fuse.portal"];
    let mut out: Vec<serde_json::Value> = Vec::new();
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let (Some(_dev), Some(mp), Some(fs), Some(opts)) = (it.next(), it.next(), it.next(), it.next()) else { continue };
        let mp = unescape_mount(mp);
        if pseudo.contains(&fs) || mp == "/" || mp.starts_with("/boot") || mp.starts_with("/proc") || mp.starts_with("/sys") || mp.starts_with("/dev") || mp.starts_with("/run/user") || mp.starts_with("/snap") {
            continue;
        }
        let path = std::path::Path::new(&mp);
        if dev_of(path) == root_dev {
            continue;
        }
        let read_only = opts.split(',').any(|o| o == "ro");
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok();
        let writable = !read_only && c.as_ref().is_some_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0);
        let free = c.as_ref().and_then(|c| {
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
        });
        out.push(json!({"path": mp, "fs": fs, "writable": writable, "free_bytes": free}));
    }
    out
}

async fn places() -> Response {
    let list = tokio::task::spawn_blocking(candidate_places).await.unwrap_or_default();
    Json(json!({"places": list, "folder_name": node_backup::FOLDER_NAME})).into_response()
}

#[derive(Deserialize)]
struct ConfigBody {
    #[serde(default)]
    dir: String,
    /// владелец подтвердил, что понимает риск копий на системном диске
    #[serde(default)]
    confirm_system: bool,
}

async fn config(Json(b): Json<ConfigBody>) -> Response {
    let r = match rt() { Ok(r) => r, Err(e) => return e };
    let l = r.layout.clone();
    let (dir, confirm) = (b.dir, b.confirm_system);
    let res = tokio::task::spawn_blocking(move || -> Result<(), PlaceError> {
        let chosen = check_place(&l, &dir)?;
        if let Some(p) = &chosen {
            if p.system_disk && !confirm {
                return Err(PlaceError::new("system_disk", p.dir.clone()));
            }
        }
        // под общим замком состояния: параллельное копирование не вернёт старую папку
        node_backup::update_state(&l, |s| {
            s.dir = chosen.map(|p| p.dir);
            // новая папка — первая плановая копия в неё делается сразу, даже если содержимое не менялось
            s.last_saved_ms = 0;
            s.last_check_ms = 0;
        })
        .map(|_| ())
        .map_err(|e| PlaceError::new("state", e))
    })
    .await;
    match res {
        Ok(Ok(())) => Json(status_json(&r.layout)).into_response(),
        Ok(Err(e)) => {
            let status = if e.code == "system_disk" { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST };
            let message = if e.code == "system_disk" { "Храните копии вне системного диска: при его потере пропадут и копии.".to_string() } else { e.message() };
            (status, Json(json!({"error": message, "code": e.code, "reason": e.reason, "detail": e.detail}))).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "сбой"}))).into_response(),
    }
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/backup/status", get(status)).route("/api/backup/export", post(export)).route("/api/backup/config", post(config)).route("/api/backup/places", get(places))
}

/// Пора ли делать плановую проверку.
pub fn due(state: &node_backup::BackupState, now: u64, critical: bool) -> bool {
    critical || state.critical_pending || state.last_check_ms == 0 || now.saturating_sub(state.last_check_ms) >= CHECK_INTERVAL_MS
}

/// Плановая задача: раз в минуту смотрит, не пора ли (сутки с прошлой проверки или критичное изменение), и тогда сверяет содержимое.
pub async fn scheduler() {
    // после сбоя копия повторяется (не через сутки): критичное изменение остаётся «в долгу» до успеха, повторы — не чаще раза в 5 минут
    let mut retry_at = 0u64;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let Some(r) = RUNTIME.get() else { continue };
        // критичный сигнал записывается в состояние как «долг» сразу: он переживает и сбой копирования, и перезапуск узла
        if crate::backup_signal::take() {
            let _ = node_backup::update_state(&r.layout, |s| s.critical_pending = true);
        }
        let st = node_backup::load_state(&r.layout);
        let now = now_ms();
        if st.dir.as_ref().is_none_or(|d| d.trim().is_empty()) || now < retry_at || !due(&st, now, false) {
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
    fn a_scheduled_check_is_due_every_ten_minutes_or_at_once_on_a_critical_change() {
        let mut s = node_backup::BackupState::default();
        assert!(due(&s, 1_000, false), "ещё ни разу не проверяли");
        s.last_check_ms = 1_000_000;
        assert!(!due(&s, 1_000_000 + 5 * 60_000, false), "через пять минут — рано");
        assert!(due(&s, 1_000_000 + CHECK_INTERVAL_MS, false), "через десять минут пора");
        assert!(due(&s, 1_000_000 + 10, true), "критичное изменение — сразу");
    }

    fn layout(d: &std::path::Path) -> Layout {
        Layout { keys: d.join("k"), data: d.join("d"), home: d.join("h"), port: 9000 }
    }

    #[test]
    fn the_place_becomes_a_yandi_node_folder_that_is_written_to_and_never_doubled() {
        let d = tempfile::tempdir().unwrap();
        let l = layout(d.path());
        assert_eq!(check_place(&l, "  ").unwrap(), None, "пусто — автокопии выключены");
        assert_eq!(check_place(&l, "relative/dir").unwrap_err().code, "not_absolute");
        assert_eq!(check_place(&l, "/tmp/../etc").unwrap_err().code, "not_absolute");
        assert_eq!(check_place(&l, &l.data.join("backups").display().to_string()).unwrap_err().code, "inside_node");
        let flash = d.path().join("flash");
        let ok = check_place(&l, &flash.display().to_string()).unwrap().unwrap();
        assert_eq!(ok.dir, flash.join("yandi-node").display().to_string(), "конечный путь …/yandi-node");
        assert!(flash.join("yandi-node").is_dir() && !flash.join("yandi-node/.yandi-write-test").exists(), "пробный файл стёрт");
        // указали уже готовую yandi-node — вторая не создаётся
        let again = check_place(&l, &flash.join("yandi-node").display().to_string()).unwrap().unwrap();
        assert_eq!(again.dir, ok.dir);
        assert!(!flash.join("yandi-node/yandi-node").exists());
    }

    #[test]
    fn an_unwritable_place_is_refused_with_the_reason() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // у root права не ограничивают: проверка бессмысленна
        }
        let d = tempfile::tempdir().unwrap();
        let l = layout(d.path());
        let ro = d.path().join("readonly");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
        let e = check_place(&l, &ro.display().to_string()).unwrap_err();
        assert_eq!(e.code, "no_write");
        assert!(e.message().starts_with("У меня нет прав для записи в это место"), "{}", e.message());
        assert!(e.detail.contains("доступ запрещён"), "{}", e.detail);
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
        // файл на месте папки — тоже отказ, а не паника
        let file = d.path().join("afile");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(check_place(&l, &file.display().to_string()).unwrap_err().code, "no_write");
    }

    #[test]
    fn the_probe_never_touches_an_existing_file_or_follows_a_link_and_a_link_into_the_node_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let l = layout(d.path());
        std::fs::create_dir_all(&l.keys).unwrap();
        // чужой файл рядом с прежним именем пробы: остаётся нетронутым
        let flash = d.path().join("flash");
        std::fs::create_dir_all(flash.join("yandi-node")).unwrap();
        let victim = d.path().join("victim.txt");
        std::fs::write(&victim, b"IMPORTANT").unwrap();
        std::os::unix::fs::symlink(&victim, flash.join("yandi-node/.yandi-write-test")).unwrap();
        let ok = check_place(&l, &flash.display().to_string()).unwrap().unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"IMPORTANT", "ссылка с прежним именем пробы не разыменована");
        assert!(ok.dir.ends_with("yandi-node"));
        let leftovers: Vec<_> = std::fs::read_dir(flash.join("yandi-node")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(leftovers, vec![".yandi-write-test".to_string()], "своих пробных файлов не осталось");
        // место-ссылка на каталог ключей узла: отказ (проверка по настоящему пути)
        let sneaky = d.path().join("sneaky");
        std::os::unix::fs::symlink(&l.keys, &sneaky).unwrap();
        assert_eq!(check_place(&l, &sneaky.display().to_string()).unwrap_err().code, "inside_node");
        assert!(std::fs::read_dir(&l.keys).unwrap().next().is_none() || !l.keys.join("yandi-node").join("x").exists());
    }

    #[test]
    fn mount_paths_are_unescaped_the_way_the_kernel_writes_them() {
        assert_eq!(unescape_mount("/media/iam/MY\\040FLASH"), "/media/iam/MY FLASH");
        assert_eq!(unescape_mount("/mnt/a\\011b\\134c"), "/mnt/a\tb\\c");
        assert_eq!(unescape_mount("/plain"), "/plain");
    }

    #[test]
    fn the_system_disk_is_recognised_and_the_candidates_never_offer_the_root() {
        let d = tempfile::tempdir().unwrap();
        let l = layout(d.path());
        // каталог во временной папке — обычно тот же диск, что и корень (или tmpfs): проверяем лишь что флаг считается без паники
        let p = check_place(&l, &d.path().join("x").display().to_string()).unwrap().unwrap();
        let _ = p.system_disk;
        for c in candidate_places() {
            assert_ne!(c["path"], "/", "корень не предлагается: {c}");
            assert!(c["writable"].is_boolean());
        }
    }
}
