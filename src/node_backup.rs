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
pub const GROUP_SEQ_GAP: u64 = 1_000_000_000;
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

/// Куда на этой машине ложится файл копии: файл якорей может быть перенесён флагом `--anchor-store` — тогда это его путь.
fn target_path(l: &Layout, area: Area, rel: &str) -> Result<PathBuf, String> {
    target_path_with(l, area, rel, crate::netlayer::packet::anchor_store_override().as_deref())
}

fn target_path_with(l: &Layout, area: Area, rel: &str, anchors_override: Option<&str>) -> Result<PathBuf, String> {
    if area == Area::Home && rel == "paired_anchors.json" {
        if let Some(p) = anchors_override {
            let p = PathBuf::from(p);
            let (Some(dir), Some(name)) = (p.parent().filter(|d| !d.as_os_str().is_empty()), p.file_name().and_then(|n| n.to_str())) else {
                return Err(format!("{rel}: путь --anchor-store не годится"));
            };
            return checked_path(dir, name);
        }
    }
    checked_path(l.base(area), rel)
}

/// Файлы связи формата сопряжения: в копию и из копии они проходят очистку от секретов старого формата (docs/RESUME_REDESIGN.md).
fn pairing_store_kind(rel: &str) -> Option<crate::netlayer::pairing::StoreKind> {
    match rel {
        "paired_clients.json" => Some(crate::netlayer::pairing::StoreKind::Clients),
        "paired_anchors.json" => Some(crate::netlayer::pairing::StoreKind::Anchors),
        _ => None,
    }
}

/// Собрать файлы для копии. Отсутствующие пропускаются (узел мог не пользоваться возможностью); слишком большие и не-обычные файлы —
/// ошибка, а не тихий пропуск: копия не должна молча получиться неполной.
pub fn collect(l: &Layout) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut take = |area: Area, rel: String| -> Result<(), String> {
        let path = target_path(l, area, &rel)?;
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
        let mut data = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
        if let Some(kind) = pairing_store_kind(&rel) {
            // секретов старого формата в копии быть не должно; неразобранный файл связи в копию не кладётся (сопряжение делается заново)
            match crate::netlayer::pairing::sanitized_store_text(kind, &data) {
                Some(clean) => data = clean.into_bytes(),
                None => {
                    eprintln!("[backup] {rel}: формат не распознан, в копию не включён (после восстановления сопряжение нужно повторить)");
                    return Ok(());
                }
            }
        }
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
    /// Что не удалось очистить от секретов старого формата (с точными путями) и что пропущено; пусто — всё очищено.
    pub unclean: Vec<String>,
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
/// Это остаток, который создаёт само восстановление: `<имя>.before-restore-<штамп>-<номер>` или `.<имя>.restore-<штамп>-<номер>` с числовыми
/// компонентами. Любые другие файлы рядом (заметки, ручные копии, файлы с похожим именем) не считаются остатками и не трогаются.
fn is_restore_leftover(own_name: &str, name: &str) -> bool {
    fn nums(rest: &str) -> bool {
        let mut it = rest.split('-');
        let (Some(a), Some(b), None) = (it.next(), it.next(), it.next()) else { return false };
        !a.is_empty() && !b.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
    }
    if let Some(rest) = name.strip_prefix(&format!("{own_name}.before-restore-")) {
        return nums(rest);
    }
    if let Some(rest) = name.strip_prefix(&format!(".{own_name}.restore-")) {
        return nums(rest);
    }
    false
}

/// Привести к безопасному виду активные файлы связи и убрать/очистить остатки восстановления рядом с ними. Возвращает всё, что сделать
/// не удалось или пропущено (путь и причина). Остатки определяются только точным шаблоном имён, который создаёт восстановление; файл
/// блокировки и временные файлы записи (`.…tmp…`) не трогаются. Чтение идёт без перехода по ссылкам.
pub fn sweep_pairing_files(l: &Layout) -> Vec<String> {
    sweep_pairing_files_with(l, crate::netlayer::packet::anchor_store_override().as_deref())
}

fn sweep_pairing_files_with(l: &Layout, anchors_override: Option<&str>) -> Vec<String> {
    use crate::netlayer::pairing::{Cleaned, StoreKind};
    let mut problems = Vec::new();
    for (rel, kind) in [("paired_clients.json", StoreKind::Clients), ("paired_anchors.json", StoreKind::Anchors)] {
        let path = match target_path_with(l, Area::Home, rel, anchors_override) {
            Ok(p) => p,
            Err(e) => {
                problems.push(format!("{rel}: путь не определён ({e})"));
                continue;
            }
        };
        // 1. активный файл — под своей блокировкой: старый формат переписывается очищенным видом, неразбираемый — в отчёт
        match crate::netlayer::pairing::sanitize_file_in_place(kind, &path, true) {
            Ok(Cleaned::Absent | Cleaned::AlreadyClean | Cleaned::Rewritten) => {}
            Ok(Cleaned::RewrittenDurabilityUnknown(why)) => problems.push(format!("{}: очищен, но запись каталога не подтверждена на диске ({why}); при отключении питания мог вернуться прежний файл — перезапишите или проверьте после перезагрузки", path.display())),
            Ok(Cleaned::Unparseable) => problems.push(format!("{}: файл не разбирается по строгой схеме, возможные секреты не проверены; удалите или проверьте его вручную", path.display())),
            Err(e) => problems.push(format!("{}: не удалось очистить ({e})", path.display())),
        }
        // 2. остатки восстановления рядом: очистить или удалить; каждая ошибка попадает в отчёт
        let dir = match path.parent().filter(|d| !d.as_os_str().is_empty()) {
            Some(d) => d.to_path_buf(),
            None => PathBuf::from("."),
        };
        let Some(own_name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                problems.push(format!("{}: каталог не читается, остатки не проверены ({e})", dir.display()));
                continue;
            }
        };
        for entry in rd {
            let e = match entry {
                Ok(e) => e,
                Err(er) => {
                    problems.push(format!("{}: запись каталога не прочитана ({er})", dir.display()));
                    continue;
                }
            };
            let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
            if !is_restore_leftover(&own_name, &name) {
                continue;
            }
            let p = e.path();
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_symlink() => {
                    problems.push(format!("{}: символическая ссылка вместо остатка, не тронута", p.display()));
                    continue;
                }
                Ok(m) if !m.is_file() => {
                    problems.push(format!("{}: остаток не обычный файл, не тронут", p.display()));
                    continue;
                }
                Err(er) => {
                    problems.push(format!("{}: {er}", p.display()));
                    continue;
                }
                _ => {}
            }
            match crate::netlayer::pairing::sanitize_file_in_place(kind, &p, false) {
                // разобрать нельзя — проверить на секреты нельзя; а имя по шаблону не доказывает, что файл создан восстановлением (P18-2),
                // поэтому не удаляем: сообщаем путь, решение за владельцем
                Ok(Cleaned::RewrittenDurabilityUnknown(why)) => problems.push(format!("{}: остаток очищен, но запись каталога не подтверждена на диске ({why})", p.display())),
                Ok(Cleaned::Unparseable) => problems.push(format!("{}: похож на остаток восстановления, но не разбирается; не тронут — проверьте и удалите вручную", p.display())),
                Ok(_) => {}
                Err(er) => problems.push(format!("{}: остаток не очищен ({er})", p.display())),
            }
        }
    }
    problems
}

pub fn restore(opened: &Opened, l: &Layout, opt: &RestoreOptions) -> Result<RestoreReport, String> {
    restore_with(opened, l, opt, crate::netlayer::packet::anchor_store_override().as_deref())
}

fn restore_with(opened: &Opened, l: &Layout, opt: &RestoreOptions, anchors_override: Option<&str>) -> Result<RestoreReport, String> {
    let entries = opened.entries()?;
    // пути проверяются ДО любой записи: ни ссылок внутри каталогов узла, ни (без явного разрешения) уже существующих файлов
    let mut targets = Vec::new();
    for e in &entries {
        targets.push(target_path_with(l, e.area, &e.path, anchors_override)?);
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
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    // Каталоги назначения должны быть приватными (0700) и наши: узел с другими правами не запустится. Проверка и исправление — ДО любых
    // изменений; чужой каталог, права которого исправить нельзя, отвергается сразу, а не после «успешного» восстановления.
    #[cfg(unix)]
    for base in [&l.keys, &l.data, &l.home] {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(base) {
            if m.is_dir() && m.permissions().mode() & 0o777 != 0o700 {
                std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700)).map_err(|er| {
                    format!("каталог {} должен быть приватным (0700), но права исправить не удалось ({er}); создайте каталог от своего пользователя", base.display())
                })?;
            }
        }
    }
    // Фаза 1: всё готовится заранее, активные файлы не трогаются. Любая ошибка здесь (неразобранный индекс групп, диск) не меняет ничего.
    let mut groups = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let mut staged: Vec<(PathBuf, PathBuf, String, Option<crate::netlayer::pairing::StoreKind>)> = Vec::new(); // (рабочий путь, подготовленный файл, подпись для отчёта, вид файла связи — по ЛОГИЧЕСКОМУ имени записи, а не по имени на диске)
    let cleanup = |staged: &[(PathBuf, PathBuf, String, Option<crate::netlayer::pairing::StoreKind>)]| {
        for (_, tmp, _, _) in staged {
            let _ = std::fs::remove_file(tmp);
        }
    };
    for (i, (e, path)) in entries.iter().zip(targets).enumerate() {
        let sanitized_pairing = match pairing_store_kind(&e.path) {
            Some(kind) => match crate::netlayer::pairing::sanitized_store_text(kind, &e.data) {
                Some(clean) => Some(clean.into_bytes()),
                None => {
                    // копия с неразобранным файлом связи: он не восстанавливается (нужно новое сопряжение) — об этом говорим, а не молчим (P18-5)
                    skipped.push(format!("{:?}/{}: файл связи в копии не разбирается по строгой схеме, не восстановлен (нужно новое сопряжение)", e.area, e.path));
                    continue;
                }
            },
            None => None,
        };
        let data = if let Some(clean) = sanitized_pairing {
            clean
        } else if e.path == "groups/index.json" {
            match rebase_groups(&e.data) {
                Ok(d) => {
                    groups = serde_json::from_slice::<Value>(&d).ok().and_then(|v| v.as_array().map(|a| a.len())).unwrap_or(0);
                    d
                }
                Err(er) => {
                    cleanup(&staged);
                    return Err(er);
                }
            }
        } else {
            e.data.clone()
        };
        if let Some(dir) = path.parent() {
            // каталоги — приватные (0700): узел не стартует с каталогом данных, доступным другим пользователям
            if let Err(er) = private_dirs(dir) {
                cleanup(&staged);
                return Err(format!("{}: {er}", e.path));
            }
        }
        let tmp = path.with_file_name(format!(".{}.restore-{stamp}-{i}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
        if let Err(er) = crate::util::private_file::write_private(&tmp, &data) {
            cleanup(&staged);
            return Err(format!("{}: {er}", e.path));
        }
        staged.push((path, tmp, format!("{:?}/{}", e.area, e.path), pairing_store_kind(&e.path)));
    }
    // Фаза 2: замена. Прежние файлы переименовываются под уникальными именами, новые встают на их место. Исключение — файлы связи
    // (`paired_*.json`): их прежнее содержимое может нести секреты старого формата, поэтому сырой копии «до восстановления» не
    // создаётся; для отката в памяти хранится только очищенный вид прежнего файла.
    let mut kept: Vec<PathBuf> = Vec::new();
    let mut written: Vec<String> = Vec::new();
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new(); // (рабочий путь, куда ушёл прежний)
    let mut pairing_rollback: std::collections::HashMap<PathBuf, Option<Vec<u8>>> = std::collections::HashMap::new(); // прежний очищенный вид (None — прежнего нет или он не разбирается)
    let mut failure: Option<String> = None;
    for (i, (path, tmp, label, pairing_kind)) in staged.iter().enumerate() {
        let pairing_kind = *pairing_kind;
        // файл связи: чтение прежнего очищенного вида и замена идут под ОДНИМ замком файла — параллельная выдача права не вклинится между
        // ними (P18-1); сырой копии «до восстановления» не создаётся
        let mut old: Option<PathBuf> = None;
        let renamed = if let Some(kind) = pairing_kind {
            match crate::netlayer::pairing::locked(path, || {
                let prior = crate::netlayer::pairing::read_bytes_nofollow(path).and_then(|b| crate::netlayer::pairing::sanitized_store_text(kind, &b)).map(|t| t.into_bytes());
                let r = std::fs::rename(tmp, path);
                (prior, r)
            }) {
                Ok((prior, r)) => {
                    pairing_rollback.insert(path.clone(), prior);
                    r
                }
                Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
            }
        } else {
            if path.exists() {
                let o = path.with_file_name(format!("{}.before-restore-{stamp}-{i}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
                if let Err(er) = std::fs::rename(path, &o) {
                    failure = Some(format!("{label}: {er}"));
                    break;
                }
                old = Some(o);
            }
            std::fs::rename(tmp, path)
        };
        if let Err(er) = renamed {
            if let Some(o) = &old {
                let _ = std::fs::rename(o, path);
            }
            failure = Some(format!("{label}: {er}"));
            break;
        }
        done.push((path.clone(), old.clone()));
        if let Some(o) = old {
            kept.push(o);
        }
        written.push(label.clone());
    }
    if let Some(er) = failure {
        // откат: уже заменённые файлы возвращаются (прежний — поверх нового одним rename), не дошедшие подготовленные удаляются;
        // что вернуть не удалось — перечисляется, а не замалчивается
        let mut stuck: Vec<String> = Vec::new();
        for (path, old) in done.iter().rev() {
            match old {
                Some(o) => {
                    if let Err(e2) = std::fs::rename(o, path) {
                        stuck.push(format!("{} (прежний файл: {})", path.display(), o.display()));
                        let _ = e2;
                    }
                }
                None => match pairing_rollback.get(path) {
                    // файл связи: возвращается очищенный вид прежнего содержимого (сырых копий нет)
                    Some(Some(prior)) => {
                        let r = crate::netlayer::pairing::locked(path, || crate::util::private_file::write_private(path, prior));
                        if let Err(e2) = r.map_err(|e| e.to_string()).and_then(|x| x.map_err(|e| e.to_string())) {
                            stuck.push(format!("{} (прежний очищенный файл связи не возвращён: {e2})", path.display()));
                        }
                    }
                    _ => {
                        if let Err(e2) = std::fs::remove_file(path) {
                            stuck.push(format!("{} (новый файл не убран: {e2})", path.display()));
                        }
                    }
                },
            }
        }
        cleanup(&staged);
        return Err(if stuck.is_empty() {
            format!("восстановление не завершено и откатено ({er}); файлы узла остались прежними. Можно повторить команду")
        } else {
            format!("восстановление не завершено ({er}), откат прошёл не полностью: {}. Прежние файлы лежат рядом (*.before-restore-*); повторите команду с --force", stuck.join("; "))
        });
    }
    // Очистка не зависит от содержимого копии: активные файлы связи и любые остатки рядом с ними (`*.before-restore-*`, промежуточные
    // `.restore-*`, в том числе от прежних сбоев) проверяются всегда; всё, что не удалось очистить или удалить, попадает в отчёт.
    let mut unclean = skipped;
    unclean.extend(sweep_pairing_files_with(l, anchors_override));
    // переименования живут в каталогах: без их сброса на диск после отключения питания мог бы вернуться прежний файл (P18-4)
    let mut dirs: Vec<PathBuf> = done.iter().filter_map(|(p, _)| p.parent().map(|d| d.to_path_buf())).collect();
    dirs.sort();
    dirs.dedup();
    for d in dirs {
        if let Err(er) = crate::util::private_file::sync_dir_checked(&d.join("x")) {
            unclean.push(format!("{}: запись каталога не подтверждена на диске ({er}); после отключения питания могли вернуться прежние файлы", d.display()));
        }
    }
    if groups > 0 {
        // узел при старте известит участников: «группа восстановлена, переписка и файлы на узле не сохранены»
        // не обязательная часть: ключи и связи уже на месте, и сбой этой записи (диск) не должен превращать удачное восстановление в ошибку
        let _ = crate::util::private_file::write_private(&l.data.join("restored_notice.json"), format!("{{\"groups\":{groups}}}").as_bytes());
    }
    Ok(RestoreReport { written, kept_old: kept, unclean, groups, node_id: opened.header.node_id.clone() })
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
    /// критичное изменение (устройство, бан, владелец) ещё не отражено в сохранённой копии; снимается только успехом или подтверждением
    /// «изменений нет» и переживает перезапуск узла
    #[serde(default)]
    pub critical_pending: bool,
    /// папка для автокопий (флешка, синхронизируемая с облаком папка); пусто — автокопии выключены
    pub dir: Option<String>,
    pub last_file: Option<String>,
}

fn state_path(l: &Layout) -> PathBuf {
    l.data.join("backup_state.json")
}

static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Операции над состоянием копий (копирование, смена папки) идут по одной: иначе завершившееся копирование возвращало бы старую настройку
/// папки, а две выгрузки получали бы один номер.
pub fn lock_state() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Прочитать-изменить-записать состояние под замком; ошибка записи возвращается вызывающему.
pub fn update_state(l: &Layout, f: impl FnOnce(&mut BackupState)) -> Result<BackupState, String> {
    let _g = lock_state();
    let mut s = load_state(l);
    f(&mut s);
    save_state(l, &s)?;
    Ok(s)
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
    // имя файла живёт в каталоге: без его fsync после отключения питания запись об успехе могла бы пережить сам файл
    sync_dir(dir).map_err(|e| format!("закрепление записи в папке: {e}"))?;
    prune_dir(dir, node_id, name);
    let _ = sync_dir(dir);
    // успех — только если новая копия реально осталась в папке (ротация её не удалила)
    if !target.is_file() {
        return Err("новая копия не осталась в папке после записи".into());
    }
    Ok(target)
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Наибольший номер копии этого узла, уже лежащей в папке: после восстановления на новой машине счётчик продолжается с него, а не с нуля
/// (иначе ротация сочла бы свежую копию самой младшей).
pub fn max_counter_in(dir: &Path, node_id: &str) -> u64 {
    let prefix = format!("yandi-{node_id}-");
    let suffix = format!(".{FILE_EXT}");
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(&suffix))
        .filter_map(|n| n[prefix.len()..].split('-').next().and_then(|c| c.parse::<u64>().ok()))
        .max()
        .unwrap_or(0)
}

fn prune_dir(dir: &Path, node_id: &str, keep: &str) {
    let prefix = format!("yandi-{node_id}-");
    let suffix = format!(".{FILE_EXT}");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut ours: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(&suffix))
        .collect();
    // по номеру копии (а не по строке): старые удаляются, свежая ({keep}) не удаляется никогда
    ours.sort_by_key(|n| n[prefix.len()..].split('-').next().and_then(|c| c.parse::<u64>().ok()).unwrap_or(0));
    while ours.len() > KEEP_AUTO {
        let old = ours.remove(0);
        if old != keep {
            let _ = std::fs::remove_file(dir.join(old));
        }
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
    let _guard = lock_state();
    let mut state = load_state(l);
    let result = (|| -> Result<Outcome, String> {
        let dir = state.dir.clone().filter(|d| !d.trim().is_empty()).map(PathBuf::from);
        if mode == Mode::Auto && dir.is_none() {
            state.last_check_ms = now;
            return Ok(Outcome { skipped: Some(Skip::NoFolder), name: String::new(), bytes: Vec::new(), saved: None, counter: state.counter });
        }
        let entries = collect(l)?;
        let dg = digest(&entries);
        if mode == Mode::Auto && decide(&state, &dg, false) == Decision::Unchanged {
            state.last_check_ms = now; // проверили: ничего нового; неудачная попытка срок «следующей проверки» НЕ сдвигает
            state.critical_pending = false;
            return Ok(Outcome { skipped: Some(Skip::Unchanged), name: String::new(), bytes: Vec::new(), saved: None, counter: state.counter });
        }
        let auth = std::fs::read(l.keys.join("auth.json")).map_err(|e| format!("auth.json: {e}"))?;
        // номер — после наибольшего из известных: из состояния и из копий этого узла, уже лежащих в папке
        let counter = state.counter.max(dir.as_deref().map(|d| max_counter_in(d, node_id)).unwrap_or(0)) + 1;
        let bytes = seal(&entries, &SealParams { root, auth_json: &auth, node_id, counter, now_ms: now })?;
        let name = file_name(node_id, counter, now);
        let saved = match &dir {
            Some(d) => Some(write_to_dir(d, &name, &bytes, root, node_id)?),
            None => None,
        };
        state.counter = counter;
        state.last_ok_ms = now;
        state.last_check_ms = now;
        state.critical_pending = false;
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
    // ошибка сохранения состояния не глотается: иначе «успех» не записан, а вызывающий об этом не знает
    match (save_state(l, &state), result) {
        (_, Err(e)) => Err(e),
        (Err(e), Ok(_)) => Err(format!("копия записана, но состояние не сохранилось: {e}")),
        (Ok(()), Ok(o)) => Ok(o),
    }
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

    /// P15-9: старые секреты возобновления не попадают в копию, не восстанавливаются и не остаются в файле «до восстановления».
    #[test]
    fn old_resume_secrets_never_enter_a_backup_or_survive_a_restore() {
        const SECRET: &str = "11aa22bb33cc44dd55ee66ff77008899aabbccddeeff00112233445566778899";
        let old = format!(r#"{{"clients":{{"{pk}":{{"session_id":5,"resume_secret_hex":"{SECRET}","expires_at":4102444800,"session_key_hex":"{SECRET}"}}}}}}"#, pk = "ab".repeat(32));
        let (_d, l) = layout();
        sample(&l);
        put(&l, Area::Home, "paired_clients.json", old.as_bytes());
        put(&l, Area::Home, "paired_anchors.json", b"{not json at all, but might hold a secret");
        let got = collect(&l).unwrap();
        for e in &got {
            assert!(!String::from_utf8_lossy(&e.data).contains(SECRET), "{}: секрет в копии", e.path);
            assert!(!String::from_utf8_lossy(&e.data).contains("resume_secret_hex"), "{}", e.path);
        }
        assert!(got.iter().any(|e| e.path == "paired_clients.json"), "очищенный файл связи в копии есть");
        assert!(!got.iter().any(|e| e.path == "paired_anchors.json"), "неразобранный файл в копию не кладётся");
        // восстановление поверх машины, где лежит старый файл: он не остаётся рядом со старым секретом
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let mut forged = got.clone();
        forged.push(Entry { area: Area::Home, path: "paired_anchors.json".into(), data: b"{garbage".to_vec() });
        forged.retain(|e| e.path != "paired_clients.json");
        forged.push(Entry { area: Area::Home, path: "paired_clients.json".into(), data: old.clone().into_bytes() }); // копия старой сборки с секретом
        let bytes = seal(&forged, &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        let opened = open_with_root(&bytes, &root).unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let target = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        std::fs::create_dir_all(&target.home).unwrap();
        std::fs::write(target.home.join("paired_clients.json"), old.as_bytes()).unwrap();
        restore(&opened, &target, &RestoreOptions { force: true }).unwrap();
        let restored = std::fs::read_to_string(target.home.join("paired_clients.json")).unwrap();
        assert!(!restored.contains(SECRET) && !restored.contains("resume_secret_hex"), "восстановленный файл очищен");
        assert!(!target.home.join("paired_anchors.json").exists(), "неразобранный файл связи не восстановлен");
        for entry in std::fs::read_dir(&target.home).unwrap().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            assert!(!text.contains(SECRET), "{:?}: остаток секрета", entry.path());
        }
    }

    const SECRET2: &str = "11aa22bb33cc44dd55ee66ff77008899aabbccddeeff00112233445566778899";

    fn old_v1() -> String {
        format!(r#"{{"clients":{{"{pk}":{{"session_id":5,"resume_secret_hex":"{SECRET2}","expires_at":4102444800,"session_key_hex":"{SECRET2}"}}}}}}"#, pk = "ab".repeat(32))
    }

    /// P16-6/7: восстановление не оставляет сырой копии файла связи, чистит остатки рядом и активные файлы, не зависящие от содержимого копии.
    #[test]
    fn p16_restore_leaves_no_raw_pairing_copy_and_cleans_leftovers_even_when_the_copy_has_none() {
        let (_d, src) = layout();
        let (opened, _) = sealed_sample(&src); // в этой копии файлов связи нет
        let d2 = tempfile::tempdir().unwrap();
        let t = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        std::fs::create_dir_all(&t.home).unwrap();
        // на машине: активный старый файл, остатки прежних сбоев и посторонний файл с похожим именем
        std::fs::write(t.home.join("paired_clients.json"), old_v1()).unwrap();
        std::fs::write(t.home.join("paired_clients.json.before-restore-1-0"), old_v1()).unwrap();
        std::fs::write(t.home.join(".paired_clients.json.restore-1-0"), b"{garbage that may hold a secret").unwrap();
        std::fs::write(t.home.join("my_paired_clients.json"), b"MINE").unwrap();
        let report = restore(&opened, &t, &RestoreOptions { force: true }).unwrap();
        assert!(report.unclean.iter().any(|p| p.contains(".paired_clients.json.restore-1-0")), "неразбираемый остаток назван в отчёте: {:?}", report.unclean);
        assert_eq!(report.unclean.len(), 1, "{:?}", report.unclean);
        for entry in std::fs::read_dir(&t.home).unwrap().flatten() {
            if entry.file_name() == ".paired_clients.json.restore-1-0" {
                continue; // неразбираемый файл не тронут и не проверяется: он в отчёте
            }
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            assert!(!text.contains(SECRET2) && !text.contains("resume_secret_hex"), "{:?}: остаток секрета", entry.path());
        }
        assert!(t.home.join(".paired_clients.json.restore-1-0").exists(), "неразбираемый остаток не удаляется по одному имени (P18-2)");
        assert_eq!(std::fs::read(t.home.join("my_paired_clients.json")).unwrap(), b"MINE", "посторонний файл не тронут");
        let active = std::fs::read_to_string(t.home.join("paired_clients.json")).unwrap();
        assert!(active.contains("\"version\": 2"), "активный файл переписан в формат v2: {active}");
    }

    #[cfg(unix)]
    #[test]
    fn p16_what_could_not_be_cleaned_is_reported_with_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, l) = layout();
        std::fs::create_dir_all(&l.home).unwrap();
        std::fs::write(l.home.join("paired_anchors.json.before-restore-9-9"), b"{garbage").unwrap();
        std::fs::set_permissions(&l.home, std::fs::Permissions::from_mode(0o500)).unwrap();
        let problems = sweep_pairing_files_with(&l, None);
        std::fs::set_permissions(&l.home, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(problems.iter().any(|p| p.contains("before-restore-9-9")), "{problems:?}");
    }

    #[test]
    fn p16_the_anchor_store_override_decides_where_the_anchors_file_lives() {
        let (_d, l) = layout();
        let custom = _d.path().join("elsewhere").join("anchors-custom.json");
        std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
        let p = target_path_with(&l, Area::Home, "paired_anchors.json", Some(custom.to_str().unwrap())).unwrap();
        assert_eq!(p, custom);
        assert_eq!(target_path_with(&l, Area::Home, "paired_anchors.json", None).unwrap(), l.home.join("paired_anchors.json"));
        // клиентские права переопределением не затрагиваются
        assert_eq!(target_path_with(&l, Area::Home, "paired_clients.json", Some(custom.to_str().unwrap())).unwrap(), l.home.join("paired_clients.json"));
    }

    // ---- замечания проверки Sol (круг 37): P17-6…8 ----

    /// P18-5: файл связи в копии, который не разбирается, не восстанавливается молча — он назван в отчёте.
    #[test]
    fn p18_5_a_pairing_file_skipped_during_restore_is_reported() {
        let (_d, src) = layout();
        sample(&src);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let mut entries = collect(&src).unwrap();
        entries.retain(|e| e.path != "paired_anchors.json");
        entries.push(Entry { area: Area::Home, path: "paired_anchors.json".into(), data: b"{garbage".to_vec() });
        let bytes = seal(&entries, &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        let opened = open_with_root(&bytes, &root).unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let t = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        let report = restore(&opened, &t, &RestoreOptions { force: true }).unwrap();
        assert!(report.unclean.iter().any(|u| u.contains("paired_anchors.json") && u.contains("не восстановлен")), "{:?}", report.unclean);
        assert!(!t.home.join("paired_anchors.json").exists(), "неразобранный файл не записан");
    }

    /// P18-1: прежнее содержимое файла связи читается под тем же замком, что и замена; откат возвращает то, что было на самом деле
    /// (в том числе право, выданное параллельно ПЕРЕД восстановлением), а не устаревшее чтение до замка.
    #[cfg(unix)]
    #[test]
    fn p18_1_rollback_returns_what_a_concurrent_writer_committed_before_the_swap() {
        use crate::netlayer::pairing::{locked, PairedClientStore, StoreRead};
        let (_d, src) = layout();
        sample(&src);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        let mut entries = collect(&src).unwrap();
        entries.retain(|e| e.path != "paired_clients.json" && e.path != "paired_anchors.json");
        let empty_clients = r#"{"version":2,"clients":{}}"#;
        let empty_anchors = r#"{"version":2,"anchors":[]}"#;
        entries.push(Entry { area: Area::Home, path: "paired_clients.json".into(), data: empty_clients.as_bytes().to_vec() });
        entries.push(Entry { area: Area::Home, path: "paired_anchors.json".into(), data: empty_anchors.as_bytes().to_vec() });
        let bytes = seal(&entries, &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        let opened = open_with_root(&bytes, &root).unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let t = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        std::fs::create_dir_all(&t.home).unwrap();
        let clients = t.home.join("paired_clients.json");
        std::fs::write(&clients, empty_clients).unwrap();
        // вторая замена обязана упасть: на месте файла блокировки якорей — каталог
        std::fs::create_dir(t.home.join(".paired_anchors.json.lock")).unwrap();
        let pk = "cd".repeat(32);
        let granted = format!(r#"{{"version":2,"clients":{{"{pk}":{{"session_id_hex":"{}","expires_at":4102444800}}}}}}"#, "1".repeat(32));
        let (tx, rx) = std::sync::mpsc::channel();
        let (c2, g2) = (clients.clone(), granted.clone());
        let writer = std::thread::spawn(move || {
            locked(&c2, || {
                tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(300));
                std::fs::write(&c2, &g2).unwrap(); // параллельная выдача права, зафиксированная до того, как замок отпущен
            })
            .unwrap();
        });
        rx.recv().unwrap(); // замок взят писателем
        let err = restore(&opened, &t, &RestoreOptions { force: true }).unwrap_err();
        writer.join().unwrap();
        assert!(err.contains("откат") || err.contains("не завершено"), "{err}");
        match PairedClientStore::read(&clients) {
            StoreRead::Current(s) => assert!(s.get(&pk).is_some(), "право, выданное до замены, потеряно откатом"),
            other => panic!("файл связи не читается после отката: {other:?}"),
        }
    }

    #[test]
    fn p17_7_the_sweep_touches_only_the_exact_leftovers_the_restore_creates() {
        let (_d, l) = layout();
        std::fs::create_dir_all(&l.home).unwrap();
        // чужие файлы с похожими именами должны остаться байт в байт
        let theirs: Vec<(String, &[u8])> = vec![
            ("paired_clients.json.notes".into(), b"my note"),
            ("paired_clients.json.json".into(), b"{}"),
            (".paired_clients.json.config".into(), b"cfg"),
            ("paired_clients.json.before-restore-abc-1".into(), b"not numeric"),
            ("paired_clients.json.before-restore-1".into(), b"one number"),
            ("paired_clients.json.before-restore-1-2-3".into(), b"three numbers"),
            ("paired_clients.json.резерв".into(), "юникод".as_bytes()),
            ("paired_anchors.json.manual-backup".into(), b"manual"),
        ];
        for (n, c) in &theirs {
            std::fs::write(l.home.join(n), c).unwrap();
        }
        // настоящие остатки восстановления
        std::fs::write(l.home.join("paired_clients.json.before-restore-123-0"), old_v1()).unwrap();
        std::fs::write(l.home.join(".paired_anchors.json.restore-5-1"), b"garbage").unwrap();
        let problems = sweep_pairing_files_with(&l, None);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(".paired_anchors.json.restore-5-1"), "неразбираемый остаток назван в отчёте: {problems:?}");
        for (n, c) in &theirs {
            assert_eq!(std::fs::read(l.home.join(n)).unwrap(), *c, "{n} тронут");
        }
        assert_eq!(std::fs::read(l.home.join(".paired_anchors.json.restore-5-1")).unwrap(), b"garbage", "по одному имени не удаляется (P18-2)");
        let cleaned = std::fs::read_to_string(l.home.join("paired_clients.json.before-restore-123-0")).unwrap();
        assert!(!cleaned.contains(SECRET2) && cleaned.contains("\"version\": 2"));
        // нестандартное имя файла якорей: соседи с общим началом имени не затрагиваются
        let custom = _d.path().join("elsewhere");
        std::fs::create_dir_all(&custom).unwrap();
        for n in ["profile.json", "profile.photo", ".profile.config"] {
            std::fs::write(custom.join(n), b"MINE").unwrap();
        }
        let ov = custom.join("profile.json");
        let problems = sweep_pairing_files_with(&l, Some(ov.to_str().unwrap()));
        // профиль не файл связи: он разбирается как «не по схеме» и сообщается, но НЕ удаляется
        assert!(problems.iter().any(|p| p.contains("profile.json")), "{problems:?}");
        for n in ["profile.json", "profile.photo", ".profile.config"] {
            assert_eq!(std::fs::read(custom.join(n)).unwrap(), b"MINE", "{n}");
        }
    }

    #[test]
    fn p17_6_a_nonstandard_anchor_store_name_never_gets_a_raw_before_restore_copy() {
        let (_d, src) = layout();
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        sample(&src);
        let mut entries = collect(&src).unwrap();
        entries.retain(|e| e.path != "paired_anchors.json");
        // копия старой сборки: файл якорей в старом формате с секретом
        let payload = serde_json::to_string(&crate::netlayer::pairing::PairingPayload {
            anchor_id: crate::util::HashId([1u8; 32]),
            anchor_x25519_hex: "a".repeat(64),
            fingerprint_hex: "b".repeat(64),
            anchor_url: "wss://a.example/".into(),
        })
        .unwrap();
        let old_anchors = format!(r#"{{"anchors":[{{"payload":{payload},"session":{{"session_id":7,"resume_secret_hex":"{SECRET2}","expires_at":4102444800}},"preference":1}}]}}"#);
        entries.push(Entry { area: Area::Home, path: "paired_anchors.json".into(), data: old_anchors.clone().into_bytes() });
        let bytes = seal(&entries, &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        let opened = open_with_root(&bytes, &root).unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let t = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        let private = d2.path().join("private");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::create_dir_all(&t.home).unwrap();
        let custom = private.join("anchors-custom.json");
        std::fs::write(&custom, old_anchors.as_bytes()).unwrap(); // уже лежит старый файл по нестандартному пути
        let report = restore_with(&opened, &t, &RestoreOptions { force: true }, Some(custom.to_str().unwrap())).unwrap();
        assert!(report.unclean.is_empty(), "{:?}", report.unclean);
        assert!(report.kept_old.iter().all(|k| !k.to_string_lossy().contains("anchors-custom")), "сырой before-restore для файла связи: {:?}", report.kept_old);
        for entry in std::fs::read_dir(&private).unwrap().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            assert!(!text.contains(SECRET2), "{:?}: остаток секрета", entry.path());
        }
        assert!(std::fs::read_to_string(&custom).unwrap().contains("\"version\": 2"));
    }

    #[cfg(unix)]
    #[test]
    fn p17_8_every_failure_of_the_sweep_is_reported_and_symlinks_are_not_followed() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, l) = layout();
        std::fs::create_dir_all(&l.home).unwrap();
        // ссылка на чужой файл под видом остатка: не следуем, сообщаем
        let target = _d.path().join("victim.txt");
        std::fs::write(&target, b"IMPORTANT").unwrap();
        std::os::unix::fs::symlink(&target, l.home.join("paired_clients.json.before-restore-1-1")).unwrap();
        let problems = sweep_pairing_files_with(&l, None);
        assert!(problems.iter().any(|p| p.contains("символическая ссылка")), "{problems:?}");
        assert_eq!(std::fs::read(&target).unwrap(), b"IMPORTANT");
        // активный файл — ссылка: не читается и не переписывается, это в отчёте
        std::fs::remove_file(l.home.join("paired_clients.json.before-restore-1-1")).unwrap();
        std::os::unix::fs::symlink(&target, l.home.join("paired_clients.json")).unwrap();
        let problems = sweep_pairing_files_with(&l, None);
        assert!(problems.iter().any(|p| p.contains("paired_clients.json")), "{problems:?}");
        assert_eq!(std::fs::read(&target).unwrap(), b"IMPORTANT");
        std::fs::remove_file(l.home.join("paired_clients.json")).unwrap();
        // каталог не читается: остатки не проверены — это не молчание
        std::fs::set_permissions(&l.home, std::fs::Permissions::from_mode(0o300)).unwrap();
        let problems = sweep_pairing_files_with(&l, None);
        std::fs::set_permissions(&l.home, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(problems.iter().any(|p| p.contains("не читается") || p.contains("не очищить") || p.contains("каталог")), "{problems:?}");
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

    // ── ревью P8 ──

    #[test]
    fn after_a_restore_the_counter_continues_from_the_folder_and_the_new_copy_is_never_rotated_away() {
        let (d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        std::fs::write(l.keys.join("auth.json"), &auth).unwrap();
        let out = d.path().join("cloud");
        // в облачной папке остались копии №30–36 прежней машины
        for n in 30..=36u64 {
            let b = seal(&collect(&l).unwrap(), &SealParams { root: &root, auth_json: &auth, node_id: "nodeid00", counter: n, now_ms: n }).unwrap();
            write_to_dir(&out, &file_name("nodeid00", n, n), &b, &root, "nodeid00").unwrap();
        }
        assert_eq!(max_counter_in(&out, "nodeid00"), 36);
        assert_eq!(max_counter_in(&out, "other"), 0, "чужие копии не считаются");
        // новая машина: состояние пустое, папка выбрана снова
        let mut st = load_state(&l);
        st.dir = Some(out.display().to_string());
        save_state(&l, &st).unwrap();
        let o = run_backup(&l, &root, "nodeid00", Mode::Auto, 1_700_000_000_000).unwrap();
        assert_eq!(o.counter, 37, "счётчик продолжился с папки, а не с единицы");
        assert!(o.saved.as_ref().unwrap().is_file(), "свежая копия осталась после ротации");
        assert_eq!(load_state(&l).counter, 37);
    }

    #[test]
    fn a_failed_restore_changes_nothing_and_leaves_no_staged_files() {
        let (_d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        // копия с повреждённым индексом групп: подготовка падает ДО замены любого файла
        let mut entries = collect(&l).unwrap();
        for e in entries.iter_mut() {
            if e.path == "groups/index.json" {
                e.data = b"{ not json".to_vec();
            }
        }
        let bytes = seal(&entries, &SealParams { root: &root, auth_json: &auth, node_id: "n", counter: 1, now_ms: 1 }).unwrap();
        let opened = open_with_root(&bytes, &root).unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let dst = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        std::fs::create_dir_all(&dst.keys).unwrap();
        std::fs::create_dir_all(&dst.data).unwrap();
        std::fs::write(dst.data.join("contacts.json"), b"MY CONTACTS").unwrap();
        let err = restore(&opened, &dst, &RestoreOptions { force: true }).unwrap_err();
        assert!(err.contains("groups/index.json"), "{err}");
        assert_eq!(std::fs::read(dst.data.join("contacts.json")).unwrap(), b"MY CONTACTS", "активные файлы не тронуты");
        assert!(!dst.keys.join("auth.json").exists());
        let strays: Vec<_> = std::fs::read_dir(&dst.data).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).filter(|n| n.contains(".restore-") || n.contains(".before-restore")).collect();
        assert!(strays.is_empty(), "подготовленные файлы убраны: {strays:?}");
    }

    #[test]
    fn restoring_into_an_existing_world_readable_directory_makes_it_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, src) = layout();
        let (opened, _) = sealed_sample(&src);
        let d2 = tempfile::tempdir().unwrap();
        let dst = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        for p in [&dst.keys, &dst.data, &dst.home] {
            std::fs::create_dir_all(p).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        restore(&opened, &dst, &RestoreOptions { force: false }).unwrap();
        for p in [&dst.keys, &dst.data, &dst.home] {
            assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o700, "{} узел иначе не стартует", p.display());
        }
    }

    #[test]
    fn a_failed_attempt_does_not_postpone_the_next_check_and_state_changes_are_serialised() {
        let (d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        std::fs::write(l.keys.join("auth.json"), &auth).unwrap();
        let mut st = load_state(&l);
        st.dir = Some("/proc/definitely/not/writable".into());
        save_state(&l, &st).unwrap();
        assert!(run_backup(&l, &root, "nodeid00", Mode::Auto, 5_000).is_err());
        assert_eq!(load_state(&l).last_check_ms, 0, "неудача не превращается в «проверено»");
        // смена папки идёт под замком состояния и не теряется
        let s = update_state(&l, |s| s.dir = Some(d.path().join("ok").display().to_string())).unwrap();
        assert!(s.dir.unwrap().ends_with("ok"));
    }

    // ── контрольный проход P9 ──

    #[test]
    fn the_notice_file_is_a_nicety_a_failure_to_write_it_does_not_fail_a_finished_restore() {
        let (_d, src) = layout();
        let (opened, _) = sealed_sample(&src);
        let d2 = tempfile::tempdir().unwrap();
        let dst = Layout { keys: d2.path().join("keys"), data: d2.path().join("data"), home: d2.path().join("home"), port: 9000 };
        std::fs::create_dir_all(&dst.data).unwrap();
        // на месте файла извещения — каталог: записать его нельзя
        std::fs::create_dir_all(dst.data.join("restored_notice.json")).unwrap();
        let r = restore(&opened, &dst, &RestoreOptions { force: false }).expect("ключи и связи установлены, извещение — не обязательная часть");
        assert!(r.written.iter().any(|w| w == "Keys/auth.json"));
    }

    #[test]
    fn a_critical_debt_survives_a_failure_and_clears_only_on_success() {
        let (d, l) = layout();
        sample(&l);
        let code = key_root::RecoveryCode::generate();
        let (root, auth) = real_root_and_auth(&code);
        std::fs::write(l.keys.join("auth.json"), &auth).unwrap();
        update_state(&l, |s| {
            s.dir = Some("/proc/definitely/not/writable".into());
            s.critical_pending = true;
        })
        .unwrap();
        assert!(run_backup(&l, &root, "nodeid00", Mode::Auto, 10).is_err());
        assert!(load_state(&l).critical_pending, "сбой не снимает «долг»: он остаётся в файле состояния и переживёт перезапуск");
        update_state(&l, |s| s.dir = Some(d.path().join("flash").display().to_string())).unwrap();
        run_backup(&l, &root, "nodeid00", Mode::Auto, 20).unwrap();
        assert!(!load_state(&l).critical_pending, "успех снимает долг");
    }
}


/// После восстановления на новой машине: если копия принесла закрытую среду, её пароль прежней установки не принимается.
/// Среда открывается сид-фразой и заново оборачивается паролем, выбранным на этой машине (как в кошельках: пароль привязан к установке,
/// сид — к личности). Счётчики попыток начинаются заново. Возвращает `false`, если закрытой среды в копии не было.
#[cfg(unix)]
pub fn finalize_vault(l: &Layout, phrase: &str, new_password: &str) -> Result<bool, String> {
    use crate::node_vault::{store::EnvelopeStore, NodeVault};
    let dir = l.data.join("closed");
    if !dir.join("envelope.json").exists() {
        return Ok(false);
    }
    let store = EnvelopeStore::open(&dir).map_err(|e| format!("закрытая среда: {e}"))?;
    let snapshot = store.load().map_err(|e| format!("закрытая среда: {e}"))?.ok_or("закрытая среда в копии пуста")?;
    store.restore_attempts().map_err(|e| format!("закрытая среда: {e}"))?;
    let mut vault = NodeVault::default();
    vault
        .recover(&snapshot.envelope, phrase)
        .map_err(|_| "сид-фраза не открывает закрытую среду из копии".to_string())?;
    let next = vault.change_password(&snapshot.envelope, new_password).map_err(|e| format!("новый пароль: {e}"))?;
    store.save(Some(&snapshot), &next, &vault).map_err(|e| format!("закрытая среда: {e}"))?;
    Ok(true)
}

#[cfg(all(test, unix))]
mod vault_tests {
    use super::*;
    use crate::node_vault::{store::EnvelopeStore, NodeVault, VaultEnvelope};

    const SEED: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

    #[test]
    fn restore_on_a_new_machine_drops_the_old_password_and_sets_a_new_one() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Layout { keys: tmp.path().join("keys"), data: tmp.path().join("data"), home: tmp.path().join("home"), port: 9000 };
        let dir = l.data.join("closed");
        let (env, vault) = VaultEnvelope::create_unlocked("old password", Some(SEED)).unwrap();
        let store = EnvelopeStore::open(&dir).unwrap();
        store.prepare_attempts(&env).unwrap();
        store.save(None, &env, &vault).unwrap();
        // как в копии: envelope и метка есть, счётчика попыток нет
        std::fs::remove_file(dir.join("attempts.json")).unwrap();
        drop(store);
        assert!(finalize_vault(&l, "alpha bravo charlie delta", "new password").is_err(), "чужая фраза не открывает");
        assert!(finalize_vault(&l, &SEED.to_uppercase(), "short").is_err(), "короткий пароль не принимается");
        assert_eq!(finalize_vault(&l, &SEED.to_uppercase(), "new password"), Ok(true));
        let store = EnvelopeStore::open(&dir).unwrap();
        let snap = store.load().unwrap().unwrap();
        let mut v = NodeVault::default();
        assert!(v.unlock(&snap.envelope, "old password").is_err(), "старый пароль не действует");
        v.unlock(&snap.envelope, "new password").unwrap();
        assert!(dir.join("attempts.json").exists(), "счётчик попыток заведён заново");
        // и сид-фраза по-прежнему открывает ту же среду
        let mut r = NodeVault::default();
        r.recover(&snap.envelope, SEED).unwrap();
        // без закрытой среды в копии делать нечего
        let empty = Layout { keys: tmp.path().join("k2"), data: tmp.path().join("d2"), home: tmp.path().join("h2"), port: 9000 };
        assert_eq!(finalize_vault(&empty, SEED, "new password"), Ok(false));
    }
}
