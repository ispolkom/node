//! Резервная копия связей узла: файл `.yandibak` (этап 1 восстановления, решения владельца 2026-10-16).
//!
//! В копии только СВЯЗИ: ключи и сертификаты узла, реестр сопряжённых устройств, контакты и закреплённые ключи, группы (описание,
//! участники, роли, эпоха ключей, номера журнала), аватарки, настройки маршрутов. НЕ в копии: переписка, файлы любых видов, итоги
//! звонков, статусы доставки, счётчики непрочитанного, журналы групп, сама фраза восстановления, машинная печать `device.seal`.
//!
//! Файл — открытый простой JSON: `{"header": {...}, "ct": "<base64>"}`. Безопасность — от шифрования, не от «секретности формата»:
//! тело зашифровано AES-256-GCM ключом `HKDF(корень ключей, соль файла)`. Корень открывается кодом восстановления (его обёртка —
//! `auth.json` — лежит в заголовке, она и так защищена Argon2id), поэтому для чтения копии нужен только сам файл и код, который у
//! владельца уже есть. Заголовок входит в AAD: подмена полей ломает расшифровку.
//!
//! Автокопия (раз в сутки, если изменилось содержимое; сразу при критичных изменениях) считает изменением только «структуру»:
//! подвижные поля (последняя активность, номера журнала, признак «в сети») в сравнение не входят.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const MAGIC: &str = "YANDIBAK";
pub const VERSION: u32 = 1;
pub const FILE_EXT: &str = "yandibak";
const INFO: &[u8] = b"yandi/backup/v1";
/// Запас номеров журнала при восстановлении: копия могла отстать от жизни, а у телефонов в кеше записи до реального номера.
pub const GROUP_SEQ_GAP: u64 = 100_000;
const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_TOTAL: usize = 64 * 1024 * 1024;
/// Сколько последних автокопий хранится в папке.
pub const KEEP_AUTO: usize = 7;

/// Что НЕ входит в копию (показывается владельцу в заметке).
pub const EXCLUDED: [&str; 6] = ["переписка", "файлы всех видов (в том числе голосовые)", "итоги и история звонков", "статусы доставки и прочтения", "журналы групп", "recovery-фраза и машинная печать"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Area {
    /// `~/.yandi_keys`
    Keys,
    /// каталог данных узла
    Data,
    /// `~/.yandi`
    Home,
}

/// Где лежат каталоги узла.
#[derive(Clone, Debug)]
pub struct Layout {
    pub keys: PathBuf,
    pub data: PathBuf,
    pub home: PathBuf,
    pub port: u16,
}

impl Layout {
    /// Настоящий узел этого пользователя.
    pub fn live(port: u16) -> Layout {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Layout { keys: home.join(".yandi_keys"), data: crate::util::data_dir::data_dir(), home: home.join(".yandi"), port }
    }

    fn base(&self, a: Area) -> &Path {
        match a {
            Area::Keys => &self.keys,
            Area::Data => &self.data,
            Area::Home => &self.home,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub area: Area,
    /// путь внутри каталога, через `/`
    pub path: String,
    pub data: Vec<u8>,
}

fn whitelist(l: &Layout) -> Vec<(Area, String)> {
    let mut v: Vec<(Area, String)> = Vec::new();
    for p in ["auth.json"] {
        v.push((Area::Keys, p.into()));
    }
    v.push((Area::Keys, format!("node_identity_{}.json", l.port)));
    for p in [
        "contacts.json",
        "mobile_devices.json",
        "mobile_self.enc",
        "mobile_self.json",
        "mobile_self_public.json",
        "mobile_tls.json",
        "web_phone_status.json",
        "web_hidden_phones.json",
        "device_profiles/index.json",
        "groups/index.json",
        "groups_pc/state.enc",
        "closed/envelope.json",
        "closed/anchor.json",
        "closed/settings.json",
        "exit.json",
        "relay.json",
        "route_rules.json",
        "gateways.json",
        "upstream_proxy.enc",
        "upstream_proxy.json",
        "turn.json",
        "local_overrides.enc",
        "local_overrides.json",
    ] {
        v.push((Area::Data, p.into()));
    }
    for p in ["profile.json", "trusted_peers.json", "paired_anchors.json", "paired_clients.json", "tls/cert.pem", "tls/key.pem"] {
        v.push((Area::Home, p.into()));
    }
    v
}

/// Путь `base/rel` без символических ссылок внутри `base` (сам каталог узла может быть ссылкой, например перенесённый на другой диск):
/// иначе ссылка в родительском каталоге вывела бы чтение или запись за пределы каталогов узла.
fn checked_path(base: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut cur = base.to_path_buf();
    for part in rel.split('/') {
        cur.push(part);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Err(format!("{rel}: символическая ссылка в пути, копия её не примет")),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{rel}: {e}")),
        }
    }
    Ok(cur)
}

/// Собрать файлы для копии. Отсутствующие пропускаются (узел мог не пользоваться возможностью); слишком большие и не-обычные файлы —
/// ошибка, а не тихий пропуск: копия не должна молча получиться неполной.
pub fn collect(l: &Layout) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut take = |area: Area, rel: String| -> Result<(), String> {
        let path = checked_path(l.base(area), &rel)?;
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("{rel}: {e}")),
        };
        if !meta.is_file() {
            return Err(format!("{rel}: не обычный файл"));
        }
        if meta.len() > MAX_FILE {
            return Err(format!("{rel}: слишком большой для копии ({} байт)", meta.len()));
        }
        let data = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
        total += data.len();
        if total > MAX_TOTAL {
            return Err("копия получилась слишком большой".into());
        }
        out.push(Entry { area, path: rel, data });
        Ok(())
    };
    for (a, p) in whitelist(l) {
        take(a, p)?;
    }
    // аватарки: маленькие картинки в ~/.yandi/avatars
    let av = l.home.join("avatars");
    if let Ok(rd) = std::fs::read_dir(&av) {
        let mut names: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                let n = n.to_ascii_lowercase();
                [".png", ".jpg", ".jpeg", ".gif"].iter().any(|x| n.ends_with(x)) && !n.contains('/') && !n.starts_with('.')
            })
            .collect();
        names.sort();
        for n in names {
            let big = std::fs::metadata(av.join(&n)).map(|m| m.len() > 512 * 1024).unwrap_or(true);
            if !big {
                take(Area::Home, format!("avatars/{n}"))?;
            }
        }
    }
    // аватарки сопряжённых устройств: ~/.local/share/yandi/device_profiles/*.png (индекс уже в белом списке)
    let dp = l.data.join("device_profiles");
    if let Ok(rd) = std::fs::read_dir(&dp) {
        let mut names: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                let n = n.to_ascii_lowercase();
                [".png", ".jpg", ".jpeg", ".gif"].iter().any(|x| n.ends_with(x)) && !n.starts_with('.')
            })
            .collect();
        names.sort();
        for n in names {
            let big = std::fs::metadata(dp.join(&n)).map(|m| m.len() > 512 * 1024).unwrap_or(true);
            if !big {
                take(Area::Data, format!("device_profiles/{n}"))?;
            }
        }
    }
    Ok(out)
}

// ── Проверка «изменилось ли» ─────────────────────────────────────────────────

fn strip(v: &mut Value, keys: &[&str]) {
    match v {
        Value::Object(m) => {
            for k in keys {
                m.remove(*k);
            }
            for (_, x) in m.iter_mut() {
                strip(x, keys);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| strip(x, keys)),
        _ => {}
    }
}

/// Файл без подвижных полей (то, что меняется само и не считается изменением связей).
fn normalized(e: &Entry) -> Vec<u8> {
    let volatile: &[&str] = match e.path.as_str() {
        "mobile_devices.json" => &["acked_ts", "last_seen_ms"],
        "device_profiles/index.json" => &["updated_ms"],
        "groups/index.json" => &["first_seq", "next_seq", "last_post_ms", "last_seen_ms"],
        "contacts.json" => &["online", "last_seen", "last_seen_ms"],
        _ => return e.data.clone(),
    };
    match serde_json::from_slice::<Value>(&e.data) {
        Ok(mut v) => {
            strip(&mut v, volatile);
            serde_json::to_vec(&v).unwrap_or_else(|_| e.data.clone())
        }
        Err(_) => e.data.clone(),
    }
}

/// Файлы, которые перезаписываются самим узлом при обычной работе (счётчики, курсоры) и в запечатанном виде не разбираются:
/// в копию входят, но изменением не считаются.
fn counts_as_change(e: &Entry) -> bool {
    !matches!(e.path.as_str(), "groups_pc/state.enc" | "mobile_self.enc")
}

/// Отпечаток логического содержимого: одинаковый, пока не изменились связи (а не шифртекст, который каждый раз другой).
pub fn digest(entries: &[Entry]) -> String {
    let mut parts: Vec<(String, [u8; 32])> = entries
        .iter()
        .filter(|e| counts_as_change(e))
        .map(|e| (format!("{:?}/{}", e.area, e.path), Sha256::digest(normalized(e)).into()))
        .collect();
    parts.sort();
    let mut h = Sha256::new();
    h.update(b"yandi-backup-digest-v1");
    for (name, d) in &parts {
        h.update((name.len() as u32).to_be_bytes());
        h.update(name.as_bytes());
        h.update(d);
    }
    hex::encode(h.finalize())
}

// ── Файл ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub magic: String,
    pub v: u32,
    /// случайный номер этой копии
    pub id: String,
    pub created_ms: u64,
    /// монотонный номер копии этого узла (по нему видно, какая свежее)
    pub counter: u64,
    /// короткий номер узла, чьи связи в копии (для сверки с карточкой при восстановлении)
    pub node_id: String,
    /// отпечаток корня ключей: позволяет сказать «этот код не от этой копии», не раскрывая корень
    pub root_id: String,
    /// `auth.json` (корень ключей под кодом восстановления), base64: по нему код открывает корень на новом железе
    pub auth_b64: String,
    pub salt_b64: String,
    pub nonce_b64: String,
}

#[derive(Serialize, Deserialize)]
struct FileWire {
    header: Header,
    ct: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Manifest {
    pub entries: Vec<EntryWire>,
    pub excluded: Vec<String>,
    pub node_version: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EntryWire {
    pub area: Area,
    pub path: String,
    pub sha256: String,
    pub data_b64: String,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn derive_key(root: &[u8; 32], salt: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 32];
    Hkdf::<Sha256>::new(Some(salt), root).expand(INFO, &mut k).expect("32 байта — допустимая длина");
    k
}

pub struct SealParams<'a> {
    pub root: &'a [u8; 32],
    pub auth_json: &'a [u8],
    pub node_id: &'a str,
    pub counter: u64,
    pub now_ms: u64,
}

/// Собрать и зашифровать копию. Возвращает готовые байты файла.
pub fn seal(entries: &[Entry], p: &SealParams) -> Result<Vec<u8>, String> {
    let manifest = Manifest {
        entries: entries
            .iter()
            .map(|e| EntryWire { area: e.area, path: e.path.clone(), sha256: hex::encode(Sha256::digest(&e.data)), data_b64: b64().encode(&e.data) })
            .collect(),
        excluded: EXCLUDED.iter().map(|s| s.to_string()).collect(),
        node_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let plain = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    let mut id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    rand::rngs::OsRng.fill_bytes(&mut id);
    let header = Header {
        magic: MAGIC.into(),
        v: VERSION,
        id: hex::encode(id),
        created_ms: p.now_ms,
        counter: p.counter,
        node_id: p.node_id.into(),
        root_id: key_root::wrap::root_id(p.root),
        auth_b64: b64().encode(p.auth_json),
        salt_b64: b64().encode(salt),
        nonce_b64: b64().encode(nonce),
    };
    let aad = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
    let key = derive_key(p.root, &salt);
    let ct = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| e.to_string())?
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &plain, aad: &aad })
        .map_err(|_| "шифрование".to_string())?;
    serde_json::to_vec_pretty(&FileWire { header, ct: b64().encode(ct) }).map_err(|e| e.to_string())
}

fn parse_wire(bytes: &[u8]) -> Result<FileWire, String> {
    if bytes.len() > (MAX_TOTAL as usize) * 2 {
        return Err("файл слишком большой".into());
    }
    let w: FileWire = serde_json::from_slice(bytes).map_err(|_| "это не файл резервной копии YANDI".to_string())?;
    if w.header.magic != MAGIC {
        return Err("это не файл резервной копии YANDI".into());
    }
    if w.header.v != VERSION {
        return Err(format!("версия копии {} не поддерживается этой программой (нужна {VERSION})", w.header.v));
    }
    Ok(w)
}

/// Только заголовок (без расшифровки): номер узла, время, номер копии.
pub fn read_header(bytes: &[u8]) -> Result<Header, String> {
    parse_wire(bytes).map(|w| w.header)
}

#[derive(Debug)]
pub struct Opened {
    pub header: Header,
    pub manifest: Manifest,
}

impl Opened {
    pub fn entries(&self) -> Result<Vec<Entry>, String> {
        self.manifest
            .entries
            .iter()
            .map(|e| {
                let data = b64().decode(&e.data_b64).map_err(|_| "повреждённая запись в копии".to_string())?;
                if hex::encode(Sha256::digest(&data)) != e.sha256 {
                    return Err(format!("{}: контрольная сумма не сходится", e.path));
                }
                if !safe_rel(&e.path) {
                    return Err(format!("недопустимый путь в копии: {}", e.path));
                }
                Ok(Entry { area: e.area, path: e.path.clone(), data })
            })
            .collect()
    }
}

/// Путь внутри каталога: без `..`, абсолютных и пустых частей (копия из чужих рук не должна писать за пределы каталогов узла).
fn safe_rel(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('/') && !p.contains('\\') && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains('\0'))
}

/// Расшифровать копию корнем ключей.
pub fn open_with_root(bytes: &[u8], root: &[u8; 32]) -> Result<Opened, String> {
    let w = parse_wire(bytes)?;
    if w.header.root_id != key_root::wrap::root_id(root) {
        return Err("этот код не относится к этой копии".into());
    }
    let salt = b64().decode(&w.header.salt_b64).map_err(|_| "повреждённый заголовок".to_string())?;
    let nonce = b64().decode(&w.header.nonce_b64).map_err(|_| "повреждённый заголовок".to_string())?;
    if nonce.len() != 12 {
        return Err("повреждённый заголовок".into());
    }
    let ct = b64().decode(&w.ct).map_err(|_| "повреждённый файл".to_string())?;
    let aad = serde_json::to_vec(&w.header).map_err(|e| e.to_string())?;
    let key = derive_key(root, &salt);
    let plain = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| e.to_string())?
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ct, aad: &aad })
        .map_err(|_| "файл повреждён или изменён".to_string())?;
    let manifest: Manifest = serde_json::from_slice(&plain).map_err(|_| "повреждённое содержимое копии".to_string())?;
    Ok(Opened { header: w.header, manifest })
}

/// Открыть копию кодом восстановления (или мастер-паролем, на который тоже сделана обёртка): корень достаётся из `auth.json` заголовка.
pub fn open_with_code(bytes: &[u8], code: &str) -> Result<Opened, String> {
    open_with_code_policy(bytes, code, &key_root::KdfPolicy::production())
}

/// То же с явным нижним порогом стоимости KDF (в тестах — дешёвым; в программе всегда рабочий).
pub fn open_with_code_policy(bytes: &[u8], code: &str, policy: &key_root::KdfPolicy) -> Result<Opened, String> {
    let w = parse_wire(bytes)?;
    let auth = b64().decode(&w.header.auth_b64).map_err(|_| "повреждённый заголовок".to_string())?;
    let doc = key_root::RootDocument::parse(&auth).map_err(|e| e.to_string())?;
    let secret = key_root::recovery_secret(code);
    let root = doc
        .unlock_with_password(&secret, policy)
        .map_err(|_| "код восстановления не подходит к этой копии".to_string())?;
    open_with_root(bytes, &root)
}

// ── Восстановление ──────────────────────────────────────────────────────────

/// После восстановления журнал групп пуст, но номера записей продолжаются с запасом: у телефонов в кеше записи до реального номера.
fn rebase_groups(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut v: Value = serde_json::from_slice(data).map_err(|_| "groups/index.json не разобран".to_string())?;
    if let Some(list) = v.as_array_mut() {
        for g in list {
            let next = g.get("next_seq").and_then(Value::as_u64).unwrap_or(1).max(g.get("first_seq").and_then(Value::as_u64).unwrap_or(1));
            let base = next.saturating_add(GROUP_SEQ_GAP);
            if let Some(o) = g.as_object_mut() {
                o.insert("first_seq".into(), base.into());
                o.insert("next_seq".into(), base.into());
            }
        }
    }
    serde_json::to_vec(&v).map_err(|e| e.to_string())
}

#[derive(Debug)]
pub struct RestoreReport {
    pub written: Vec<String>,
    pub kept_old: Vec<PathBuf>,
    pub groups: usize,
    pub node_id: String,
}

pub struct RestoreOptions {
    /// перезаписывать, если на этой машине уже есть ключи узла (прежние файлы сохраняются рядом)
    pub force: bool,
}

/// Создать каталог (и недостающие родители) с правами 0700.
fn private_dirs(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Записать файлы копии в каталоги узла. Узел при этом не должен работать. Журналы групп и переписка не восстанавливаются.
pub fn restore(opened: &Opened, l: &Layout, opt: &RestoreOptions) -> Result<RestoreReport, String> {
    let entries = opened.entries()?;
    // пути проверяются ДО любой записи: ни ссылок внутри каталогов узла, ни (без явного разрешения) уже существующих файлов
    let mut targets = Vec::new();
    for e in &entries {
        targets.push(checked_path(l.base(e.area), &e.path)?);
    }
    if !opt.force {
        let existing: Vec<String> = entries.iter().zip(&targets).filter(|(_, p)| p.exists()).map(|(e, _)| format!("{:?}/{}", e.area, e.path)).collect();
        if !existing.is_empty() {
            return Err(format!(
                "на этой машине уже есть файлы узла ({}); восстановление не перезапишет их без явного разрешения (--force)",
                existing.join(", ")
            ));
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut kept = Vec::new();
    let mut written = Vec::new();
    let mut groups = 0usize;
    for (e, path) in entries.iter().zip(targets) {
        if path.exists() {
            let old = path.with_file_name(format!("{}.before-restore-{stamp}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
            std::fs::rename(&path, &old).map_err(|er| format!("{}: {er}", e.path))?;
            kept.push(old);
        }
        let data = if e.path == "groups/index.json" {
            let d = rebase_groups(&e.data)?;
            groups = serde_json::from_slice::<Value>(&d).ok().and_then(|v| v.as_array().map(|a| a.len())).unwrap_or(0);
            d
        } else {
            e.data.clone()
        };
        if let Some(dir) = path.parent() {
            // каталоги — приватные (0700): узел не стартует с каталогом данных, доступным другим пользователям
            private_dirs(dir).map_err(|er| format!("{}: {er}", e.path))?;
        }
        crate::util::private_file::write_private(&path, &data).map_err(|er| format!("{}: {er}", e.path))?;
        written.push(format!("{:?}/{}", e.area, e.path));
    }
    if groups > 0 {
        // узел при старте известит участников: «группа восстановлена, переписка и файлы на узле не сохранены»
        crate::util::private_file::write_private(&l.data.join("restored_notice.json"), format!("{{\"groups\":{groups}}}").as_bytes())
            .map_err(|er| format!("restored_notice.json: {er}"))?;
    }
    Ok(RestoreReport { written, kept_old: kept, groups, node_id: opened.header.node_id.clone() })
}

// ── Состояние автокопии ─────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct BackupState {
    /// номер последней сделанной копии
    pub counter: u64,
    /// отпечаток содержимого последней копии, сохранённой В ПАПКУ (для «изменилось ли»); выгрузка по кнопке без папки его не трогает
    pub last_digest: String,
    /// время последней успешной копии любого вида (мс)
    pub last_ok_ms: u64,
    /// время последней копии, записанной в папку (мс); 0 — в папку ещё ничего не записано
    pub last_saved_ms: u64,
    /// время последней проверки (мс): копия могла не понадобиться
    pub last_check_ms: u64,
    pub last_error: Option<String>,
    /// папка для автокопий (флешка, синхронизируемая с облаком папка); пусто — автокопии выключены
    pub dir: Option<String>,
    pub last_file: Option<String>,
}

fn state_path(l: &Layout) -> PathBuf {
    l.data.join("backup_state.json")
}

pub fn load_state(l: &Layout) -> BackupState {
    std::fs::read_to_string(state_path(l)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_state(l: &Layout, s: &BackupState) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    crate::util::private_file::write_private(&state_path(l), &bytes).map_err(|e| e.to_string())
}

/// Имя файла автокопии: узнаваемое, сортируется по номеру.
pub fn file_name(node_id: &str, counter: u64, now_ms: u64) -> String {
    let secs = now_ms / 1000;
    let (y, mo, d, h, mi) = civil(secs);
    format!("yandi-{node_id}-{counter:06}-{y:04}{mo:02}{d:02}-{h:02}{mi:02}.{FILE_EXT}")
}

/// Дата и время по UTC из секунд эпохи (без внешних зависимостей).
pub fn civil_utc(secs: u64) -> (u64, u64, u64, u64, u64) {
    civil(secs)
}

fn civil(secs: u64) -> (u64, u64, u64, u64, u64) {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m as u64, d as u64, rem / 3600, (rem % 3600) / 60)
}

/// Записать копию в папку: временный файл, проверка прочитанным обратно, переименование; старые автокопии этого узла ужимаются до
/// [`KEEP_AUTO`].
pub fn write_to_dir(dir: &Path, name: &str, bytes: &[u8], root: &[u8; 32], node_id: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("папка копий: {e}"))?;
    let target = dir.join(name);
    // временное имя случайное, файл создаётся только заново (O_EXCL: заранее подложенная ссылка или файл — ошибка, а не перезапись чужого)
    let mut rnd = [0u8; 6];
    rand::rngs::OsRng.fill_bytes(&mut rnd);
    let tmp = dir.join(format!(".{name}.{}.part", hex::encode(rnd)));
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| format!("запись копии: {e}"))?;
        f.write_all(bytes).and_then(|_| f.sync_all()).map_err(|e| format!("запись копии: {e}"))?;
    }
    let back = std::fs::read(&tmp).map_err(|e| format!("проверка копии: {e}"))?;
    if back != bytes || open_with_root(&back, root).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err("проверка записанной копии не прошла".into());
    }
    if std::fs::symlink_metadata(&target).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        let _ = std::fs::remove_file(&tmp);
        return Err("на месте файла копии лежит символическая ссылка".into());
    }
    std::fs::rename(&tmp, &target).map_err(|e| format!("запись копии: {e}"))?;
    prune_dir(dir, node_id);
    Ok(target)
}

fn prune_dir(dir: &Path, node_id: &str) {
    let prefix = format!("yandi-{node_id}-");
    let suffix = format!(".{FILE_EXT}");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut ours: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(&suffix))
        .collect();
    ours.sort();
    while ours.len() > KEEP_AUTO {
        let old = ours.remove(0);
        let _ = std::fs::remove_file(dir.join(old));
    }
}

/// Решение «делать ли автокопию сейчас».
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// содержимое не менялось
    Unchanged,
    /// менялось (или принудительно)
    Make,
}

pub fn decide(state: &BackupState, current_digest: &str, force: bool) -> Decision {
    if force || state.last_digest != current_digest || state.last_saved_ms == 0 {
        Decision::Make
    } else {
        Decision::Unchanged
    }
}

// ── Запуск копии ────────────────────────────────────────────────────────────

/// Как запущена копия.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// плановая/по событию: только если папка задана и содержимое изменилось
    Auto,
    /// по кнопке: всегда делается, файл отдаётся владельцу (и кладётся в папку, если она задана)
    Now,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Skip {
    NoFolder,
    Unchanged,
}

#[derive(Debug)]
pub struct Outcome {
    pub skipped: Option<Skip>,
    pub name: String,
    pub bytes: Vec<u8>,
    pub saved: Option<PathBuf>,
    pub counter: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Сделать копию: собрать, при плановом запуске сравнить с прошлой, зашифровать, записать в папку (с проверкой), обновить состояние.
pub fn run_backup(l: &Layout, root: &[u8; 32], node_id: &str, mode: Mode, now: u64) -> Result<Outcome, String> {
    let mut state = load_state(l);
    state.last_check_ms = now;
    let result = (|| -> Result<Outcome, String> {
        let dir = state.dir.clone().filter(|d| !d.trim().is_empty()).map(PathBuf::from);
        if mode == Mode::Auto && dir.is_none() {
            return Ok(Outcome { skipped: Some(Skip::NoFolder), name: String::new(), bytes: Vec::new(), saved: None, counter: state.counter });
        }
        let entries = collect(l)?;
        let dg = digest(&entries);
        if mode == Mode::Auto && decide(&state, &dg, false) == Decision::Unchanged {
            return Ok(Outcome { skipped: Some(Skip::Unchanged), name: String::new(), bytes: Vec::new(), saved: None, counter: state.counter });
        }
        let auth = std::fs::read(l.keys.join("auth.json")).map_err(|e| format!("auth.json: {e}"))?;
        let counter = state.counter + 1;
        let bytes = seal(&entries, &SealParams { root, auth_json: &auth, node_id, counter, now_ms: now })?;
        let name = file_name(node_id, counter, now);
        let saved = match &dir {
            Some(d) => Some(write_to_dir(d, &name, &bytes, root, node_id)?),
            None => None,
        };
        state.counter = counter;
        state.last_ok_ms = now;
        state.last_error = None;
        if let Some(p) = &saved {
            // только копия, лежащая в папке, сдвигает «последнее сохранённое»: выгрузка по кнопке на флешку-в-руке — другое дело
            state.last_digest = dg;
            state.last_saved_ms = now;
            state.last_file = Some(p.display().to_string());
        }
        Ok(Outcome { skipped: None, name, bytes, saved, counter })
    })();
    if let Err(e) = &result {
        state.last_error = Some(e.clone());
    }
    let _ = save_state(l, &state);
    result
}

/// Короткий номер узла (первые 8 байт адреса) по файлу личности, открытому корнем.
pub fn node_id_from_files(l: &Layout, root: &[u8; 32]) -> Result<String, String> {
    let bytes = std::fs::read(l.keys.join(format!("node_identity_{}.json", l.port))).map_err(|e| format!("файл личности узла: {e}"))?;
    let m = key_root::identity_store::open_identity(&bytes, &key_root::OpenContext { root: Some(root) }).map_err(|e| e.to_string())?;
    Ok(hex::encode(&m.address[..8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Layout) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout { keys: d.path().join("keys"), data: d.path().join("data"), home: d.path().join("home"), port: 9000 };
        for p in [&l.keys, &l.data, &l.home] {
            std::fs::create_dir_all(p).unwrap();
        }
        (d, l)
    }

    fn put(l: &Layout, area: Area, rel: &str, data: &[u8]) {
        let p = l.base(area).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }

    /// Настоящая обёртка корня (как у узла) под кодом восстановления: чтобы открытие кодом было настоящим.
    fn real_root_and_auth(code: &key_root::RecoveryCode) -> ([u8; 32], Vec<u8>) {
        use key_root::{FixedMachine, KdfParams, KdfPolicy, RootDocument};
        let machine = FixedMachine("test-machine".into());
        let device_key = [9u8; 32];
        let root = [7u8; 32];
        let fast = KdfParams { memory_kib: 64, iterations: 1, parallelism: 1 };
        let doc = RootDocument::create(&root, "login-hash", &device_key, "test-device", &machine, code.secret(), fast, &KdfPolicy::for_tests())
            .expect("корневой документ");
        (root, doc.to_json())
    }

    fn sample(l: &Layout) {
        put(l, Area::Keys, "auth.json", b"{\"auth\":1}");
        put(l, Area::Keys, "node_identity_9000.json", b"{\"identity\":1}");
        put(l, Area::Keys, "device.seal", b"MACHINE-BOUND-SECRET");
        put(l, Area::Data, "contacts.json", r#"{"contacts":[{"name":"Алиса","short_id":"aaaa","online":true}]}"#.as_bytes());
        put(l, Area::Data, "mobile_devices.json", r#"[{"token_hash":"t","name":"Телефон","acked_ts":5,"last_seen_ms":10,"role":"owner"}]"#.as_bytes());
        put(
            l,
            Area::Data,
            "groups/index.json",
            r#"[{"id":"g1","name":"Семья","members":[{"alias":"A","last_post_ms":5}],"epoch":1,"first_seq":1,"next_seq":43}]"#.as_bytes(),
        );
        put(l, Area::Data, "groups/g1.log", b"{\"seq\":1,\"secret\":\"message body\"}\n");
        put(l, Area::Home, "avatars/aa.png", b"\x89PNG\r\n");
        put(l, Area::Home, "chats/chat_x.enc", b"CHAT HISTORY");
        put(l, Area::Data, "files/uploads/doc.bin", b"A FILE");
    }

    #[test]
    fn the_collection_takes_only_relationships_not_chats_files_journals_or_the_machine_seal() {
        let (_d, l) = layout();
        sample(&l);
        let got = collect(&l).unwrap();
        let names: Vec<String> = got.iter().map(|e| format!("{:?}/{}", e.area, e.path)).collect();
        assert!(names.contains(&"Keys/auth.json".to_string()) && names.contains(&"Data/contacts.json".to_string()));
        assert!(names.contains(&"Home/avatars/aa.png".to_string()), "аватарки входят");
        for banned in ["device.seal", "chats", "files", ".log"] {
            assert!(!names.iter().any(|n| n.contains(banned)), "{banned} не должно быть в копии: {names:?}");
        }
    }

    #[test]
    fn a_backup_opens_with_the_recovery_code_alone_and_gives_every_file_back_byte_for_byte() {
        let (_d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let entries = collect(&l).unwrap();
        let bytes = seal(&entries, &SealParams { root: &root, auth_json: &auth, node_id: "abcd1234abcd1234", counter: 3, now_ms: 1_700_000_000_000 }).unwrap();
        // шифртекст не содержит открытых данных
        assert!(!String::from_utf8_lossy(&bytes).contains("Алиса"));
        let header = read_header(&bytes).unwrap();
        assert_eq!((header.counter, header.node_id.as_str()), (3, "abcd1234abcd1234"));
        // открыть можно только кодом (корень достаётся из auth.json в заголовке)
        let opened = open_with_code_policy(&bytes, &code.display(), &key_root::KdfPolicy::for_tests()).unwrap();
        let back = opened.entries().unwrap();
        assert_eq!(back.len(), entries.len());
        for (a, b) in entries.iter().zip(&back) {
            assert_eq!((a.area, &a.path, &a.data), (b.area, &b.path, &b.data));
        }
        // чужой код — отказ с понятной причиной
        let other = key_root::RecoveryCode::generate().display();
        assert!(open_with_code_policy(&bytes, &other, &key_root::KdfPolicy::for_tests()).unwrap_err().contains("код восстановления не подходит"));
    }

    #[test]
    fn tampering_with_any_part_of_the_file_is_detected() {
        let (_d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let bytes = seal(&collect(&l).unwrap(), &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        // подмена номера копии (чтобы выдать старую за новую) ломает расшифровку: заголовок в AAD
        let mut v: Value = serde_json::from_slice(&bytes).unwrap();
        v["header"]["counter"] = 99.into();
        assert!(open_with_root(&serde_json::to_vec(&v).unwrap(), &root).is_err());
        // порча шифртекста
        let mut v: Value = serde_json::from_slice(&bytes).unwrap();
        let ct = v["ct"].as_str().unwrap().to_string();
        let mut raw = b64().decode(ct).unwrap();
        raw[10] ^= 1;
        v["ct"] = b64().encode(raw).into();
        assert!(open_with_root(&serde_json::to_vec(&v).unwrap(), &root).is_err());
        // не наш файл
        assert!(read_header(b"hello").is_err());
    }

    #[test]
    fn restore_writes_files_rebases_group_numbers_and_never_restores_journals() {
        let (_d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let bytes = seal(&collect(&l).unwrap(), &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        let opened = open_with_root(&bytes, &root).unwrap();

        let d2 = tempfile::tempdir().unwrap();
        // каталоги машины ещё не существуют: восстановление создаёт их само и обязано сделать приватными
        let fresh = Layout { keys: d2.path().join("keys"), data: d2.path().join("share/yandi"), home: d2.path().join("dot-yandi"), port: 9000 };
        let report = restore(&opened, &fresh, &RestoreOptions { force: false }).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            for dir in [&fresh.keys, &fresh.data, &fresh.home, &fresh.data.join("groups")] {
                assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700, "{} должен быть 0700: узел не стартует с открытым каталогом данных", dir.display());
            }
        }
        assert!(report.written.iter().any(|w| w == "Keys/auth.json"));
        assert_eq!(report.groups, 1);
        assert!(fresh.data.join("restored_notice.json").exists(), "метка для извещения участников о восстановленных группах");
        assert_eq!(std::fs::read(fresh.keys.join("auth.json")).unwrap(), b"{\"auth\":1}");
        assert!(!fresh.keys.join("device.seal").exists(), "машинная печать не переносится");
        assert!(!fresh.data.join("groups/g1.log").exists(), "журнал группы не восстанавливается");
        // номера журнала продолжаются с запасом: у телефонов в кеше записи до реального номера
        let idx: Value = serde_json::from_slice(&std::fs::read(fresh.data.join("groups/index.json")).unwrap()).unwrap();
        assert_eq!(idx[0]["next_seq"], 43 + GROUP_SEQ_GAP);
        assert_eq!(idx[0]["first_seq"], 43 + GROUP_SEQ_GAP);
        // ключи уже есть: без явного разрешения не перезаписываем
        assert!(restore(&opened, &fresh, &RestoreOptions { force: false }).unwrap_err().contains("уже есть файлы узла"));
        // с разрешением — прежние файлы остаются рядом
        let again = restore(&opened, &fresh, &RestoreOptions { force: true }).unwrap();
        assert!(!again.kept_old.is_empty() && again.kept_old.iter().all(|p| p.exists()));
    }

    #[test]
    fn a_path_that_climbs_out_of_the_node_directories_is_refused() {
        for bad in ["../x", "/etc/passwd", "a/../../b", "a//b", "", ".", "a\\b"] {
            assert!(!safe_rel(bad), "{bad:?}");
        }
        for ok in ["auth.json", "groups/index.json", "avatars/aa.png"] {
            assert!(safe_rel(ok), "{ok:?}");
        }
    }

    #[test]
    fn only_a_real_change_of_relationships_makes_a_new_backup() {
        let (_d, l) = layout();
        sample(&l);
        let base = digest(&collect(&l).unwrap());
        // подвижные поля: «в сети», последняя активность, номера журнала — изменением не считаются
        put(&l, Area::Data, "contacts.json", r#"{"contacts":[{"name":"Алиса","short_id":"aaaa","online":false}]}"#.as_bytes());
        put(&l, Area::Data, "mobile_devices.json", r#"[{"token_hash":"t","name":"Телефон","acked_ts":500,"last_seen_ms":1000,"role":"owner"}]"#.as_bytes());
        put(
            &l,
            Area::Data,
            "groups/index.json",
            r#"[{"id":"g1","name":"Семья","members":[{"alias":"A","last_post_ms":99999}],"epoch":1,"first_seq":1,"next_seq":900}]"#.as_bytes(),
        );
        put(&l, Area::Home, "chats/chat_x.enc", b"MORE CHAT HISTORY");
        put(&l, Area::Data, "groups_pc/state.enc", b"cursor moved");
        assert_eq!(digest(&collect(&l).unwrap()), base, "сообщения, активность и курсоры копию не вызывают");
        // настоящие изменения: новый контакт, смена роли участника, новое устройство
        put(&l, Area::Data, "contacts.json", r#"{"contacts":[{"name":"Алиса","short_id":"aaaa"},{"name":"Боб","short_id":"bbbb"}]}"#.as_bytes());
        let with_contact = digest(&collect(&l).unwrap());
        assert_ne!(with_contact, base);
        put(
            &l,
            Area::Data,
            "groups/index.json",
            r#"[{"id":"g1","name":"Семья","members":[{"alias":"A","role":"admin","last_post_ms":1}],"epoch":1,"first_seq":1,"next_seq":900}]"#.as_bytes(),
        );
        assert_ne!(digest(&collect(&l).unwrap()), with_contact, "смена роли — изменение");
        // решение
        let st = BackupState { last_digest: base.clone(), last_ok_ms: 5, last_saved_ms: 5, ..Default::default() };
        assert_eq!(decide(&st, &base, false), Decision::Unchanged);
        assert_eq!(decide(&st, &base, true), Decision::Make);
        assert_eq!(decide(&st, "other", false), Decision::Make);
        assert_eq!(decide(&BackupState::default(), &base, false), Decision::Make, "первой копии нет — делаем");
    }

    #[test]
    fn auto_copies_are_written_checked_and_trimmed_to_the_last_few() {
        let (d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let out = d.path().join("flash");
        for n in 1..=10u64 {
            let bytes = seal(&collect(&l).unwrap(), &SealParams { root: &root, auth_json: &auth, node_id: "nodeid00", counter: n, now_ms: 1_700_000_000_000 + n * 86_400_000 }).unwrap();
            let name = file_name("nodeid00", n, 1_700_000_000_000 + n * 86_400_000);
            write_to_dir(&out, &name, &bytes, &root, "nodeid00").unwrap();
        }
        // посторонний файл в папке не трогается
        std::fs::write(out.join("photo.jpg"), b"x").unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        names.sort();
        let ours: Vec<&String> = names.iter().filter(|n| n.ends_with(".yandibak")).collect();
        assert_eq!(ours.len(), KEEP_AUTO);
        assert!(ours.first().unwrap().contains("-000004-"), "остались последние: {ours:?}");
        assert!(names.contains(&"photo.jpg".to_string()));
        assert!(!names.iter().any(|n| n.ends_with(".part")), "временных файлов нет");
    }

    #[test]
    fn run_backup_skips_without_a_folder_or_changes_and_counts_copies() {
        let (d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, _auth) = real_root_and_auth(&code);
        std::fs::write(l.keys.join("auth.json"), &_auth).unwrap();
        let t = 1_700_000_000_000u64;
        // папка не задана: плановая копия не делается, по кнопке — делается и отдаётся
        let o = run_backup(&l, &root, "nodeid00", Mode::Auto, t).unwrap();
        assert_eq!(o.skipped, Some(Skip::NoFolder));
        let manual = run_backup(&l, &root, "nodeid00", Mode::Now, t + 1).unwrap();
        assert!(manual.skipped.is_none() && manual.counter == 1 && !manual.bytes.is_empty() && manual.saved.is_none());
        // папку задали: первая плановая — делается
        let mut st = load_state(&l);
        st.dir = Some(d.path().join("flash").display().to_string());
        save_state(&l, &st).unwrap();
        let first = run_backup(&l, &root, "nodeid00", Mode::Auto, t + 2).unwrap();
        assert!(first.skipped.is_none() && first.counter == 2 && first.saved.as_ref().unwrap().exists());
        // ничего не менялось (подвижные поля не в счёт) — вторая не делается
        put(&l, Area::Data, "contacts.json", r#"{"contacts":[{"name":"Алиса","short_id":"aaaa","online":false}]}"#.as_bytes());
        let again = run_backup(&l, &root, "nodeid00", Mode::Auto, t + 3).unwrap();
        assert_eq!(again.skipped, Some(Skip::Unchanged));
        assert_eq!(load_state(&l).counter, 2, "счётчик не растёт без изменений");
        // новый контакт — делается
        put(&l, Area::Data, "contacts.json", r#"{"contacts":[{"name":"Боб","short_id":"bbbb"}]}"#.as_bytes());
        let third = run_backup(&l, &root, "nodeid00", Mode::Auto, t + 4).unwrap();
        assert!(third.skipped.is_none() && third.counter == 3);
        let st = load_state(&l);
        assert_eq!((st.counter, st.last_ok_ms, st.last_saved_ms, st.last_error), (3, t + 4, t + 4, None));
        // сбой (папку нельзя создать) запоминается и не портит счётчик
        let mut bad = load_state(&l);
        bad.dir = Some("/proc/definitely/not/writable".into());
        bad.last_digest = String::new();
        save_state(&l, &bad).unwrap();
        assert!(run_backup(&l, &root, "nodeid00", Mode::Auto, t + 5).is_err());
        let st = load_state(&l);
        assert_eq!(st.counter, 3);
        assert!(st.last_error.is_some());
    }

    #[test]
    fn file_names_are_readable_and_sort_by_number() {
        assert_eq!(file_name("ab12", 7, 1_700_000_000_000), "yandi-ab12-000007-20231114-2213.yandibak");
    }

    // ── Найдено ревизией Codex (ANALYSIS8, пробами): все семь сценариев воспроизведены и закрыты ──

    fn sealed_sample(l: &Layout) -> (Opened, [u8; 32]) {
        sample(l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let bytes = seal(&collect(l).unwrap(), &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        (open_with_root(&bytes, &root).unwrap(), root)
    }

    #[test]
    fn restore_never_overwrites_an_existing_identity_or_any_node_file_without_force() {
        let (_d, l) = layout();
        let (opened, _) = sealed_sample(&l);
        let d2 = tempfile::tempdir().unwrap();
        let other = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        std::fs::create_dir_all(&other.keys).unwrap();
        // на машине уже есть личность узла, auth.json нет — раньше это не останавливало запись
        std::fs::write(other.keys.join("node_identity_9000.json"), b"MY OWN IDENTITY").unwrap();
        let err = restore(&opened, &other, &RestoreOptions { force: false }).unwrap_err();
        assert!(err.contains("node_identity_9000.json"), "{err}");
        assert_eq!(std::fs::read(other.keys.join("node_identity_9000.json")).unwrap(), b"MY OWN IDENTITY");
        assert!(!other.keys.join("auth.json").exists(), "до отказа ничего не записано");
    }

    #[test]
    fn a_symlink_in_the_path_neither_leaks_files_out_of_the_copy_nor_lets_a_restore_write_outside() {
        use std::os::unix::fs::symlink;
        // сбор: каталог groups — ссылка наружу; чужой index.json в копию попасть не должен
        let (d, l) = layout();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("index.json"), b"[\"SECRET\"]").unwrap();
        symlink(&outside, l.data.join("groups")).unwrap();
        assert!(collect(&l).unwrap_err().contains("символическая ссылка"));
        // восстановление: ссылка в каталоге назначения — отказ до записи
        let (_d1, src) = layout();
        let (opened, _) = sealed_sample(&src);
        let (d2, dst) = layout();
        let escape = d2.path().join("escape");
        std::fs::create_dir_all(&escape).unwrap();
        symlink(&escape, dst.data.join("groups")).unwrap();
        let err = restore(&opened, &dst, &RestoreOptions { force: true }).unwrap_err();
        assert!(err.contains("символическая ссылка"), "{err}");
        assert!(std::fs::read_dir(&escape).unwrap().next().is_none(), "за пределы каталогов узла ничего не записано");
        assert!(!dst.keys.join("auth.json").exists(), "проверка путей — до любой записи");
    }

    #[test]
    fn a_preplanted_temp_link_in_the_backup_folder_cannot_clobber_another_file() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("flash");
        std::fs::create_dir_all(&dir).unwrap();
        let victim = d.path().join("victim");
        std::fs::write(&victim, b"IMPORTANT").unwrap();
        let name = file_name("node", 1, 1);
        // старое имя временного файла (`.<имя>.part`) заранее подложено ссылкой на чужой файл
        std::os::unix::fs::symlink(&victim, dir.join(format!(".{name}.part"))).unwrap();
        let root = [7u8; 32];
        let b = seal(&[], &SealParams { root: &root, auth_json: b"{}", node_id: "node", counter: 1, now_ms: 1 }).unwrap();
        // имя временного файла случайное, создаётся только заново: подложенная ссылка не задета
        write_to_dir(&dir, &name, &b, &root, "node").unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"IMPORTANT");
        // и сам файл копии на месте ссылки не принимается
        let name2 = file_name("node", 2, 2);
        std::os::unix::fs::symlink(&victim, dir.join(&name2)).unwrap();
        assert!(write_to_dir(&dir, &name2, &b, &root, "node").is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"IMPORTANT");
    }

    #[test]
    fn device_avatars_are_collected_and_a_settings_change_counts_as_a_change() {
        let (_d, l) = layout();
        put(&l, Area::Data, "device_profiles/index.json", b"{}");
        put(&l, Area::Data, "device_profiles/abc.png", b"image");
        put(&l, Area::Data, "device_profiles/notes.txt", b"not an avatar");
        let got = collect(&l).unwrap();
        assert!(got.iter().any(|e| e.path == "device_profiles/abc.png"), "аватарки сопряжённых устройств входят");
        assert!(!got.iter().any(|e| e.path.ends_with("notes.txt")));
        let a = digest(&[Entry { area: Area::Data, path: "closed/settings.json".into(), data: br#"{"auto_lock_secs":900}"#.to_vec() }]);
        let b = digest(&[Entry { area: Area::Data, path: "closed/settings.json".into(), data: br#"{"auto_lock_secs":0}"#.to_vec() }]);
        assert_ne!(a, b, "смена настроек закрытой среды — изменение");
    }
}
