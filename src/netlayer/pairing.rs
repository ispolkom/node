// src/netlayer/pairing.rs
//! Сопряжение клиента с якорем: что показывается в QR и какие записи о праве хранятся на диске.
//!
//! **Что здесь:**
//! - `PairingPayload` — публичное содержимое QR (anchor_id, открытый X25519, отпечаток TLS, адрес). Секретов в нём нет.
//! - `ClientGrant` — запись якоря о праве клиента: кто (ключ подписи клиента) и до какого срока. Это **не секрет**: по ней нельзя
//!   ни расшифровать трафик, ни войти без закрытого ключа клиента. Рабочих ключей и секретов возобновления на диске нет.
//! - `PairedClientStore` (`paired_clients.json`) и `PairedAnchorStore` (`paired_anchors.json`) — файлы формата версии 2 со строгой
//!   схемой (неизвестные поля на любом уровне отвергаются), строгим читателем, миграцией со старого формата и транзакциями
//!   `update` под межпроцессной блокировкой.
//!
//! **Чего больше нет (docs/RESUME_REDESIGN.md, этап 2а):** возобновления сеанса по сохранённым секретам (пакеты 0xC0/0xC1/0xC2),
//! `session_key_hex`, `resume_secret_hex`. Соединение после обрыва восстанавливается обычным подписанным Hello с новыми эфемерными
//! ключами. Старый файл с секретами не принимается как действующий: при чтении он переписывается без секретных полей, а права
//! клиентов из него **не переносятся** (новое сопряжение выдаёт новое право). Если переписать не удалось, хранилище блокируется
//! и владельцу сообщается, что секреты ещё на диске.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::util::HashId;

/// Версия формата файлов `paired_clients.json` и `paired_anchors.json`.
pub const STORE_VERSION: u32 = 2;

/// 7 дней TTL по умолчанию.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 7 * 24 * 3600;

/// Предел размера файла хранилища при чтении и записи (одинаковый), число прав и якорей.
pub const MAX_STORE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_CLIENTS: usize = 4096;
pub const MAX_ANCHORS: usize = 64;

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ----------------- Payload (QR) -----------------

/// То, что anchor показывает в QR-коде. Mobile сканит → парсит → сохраняет в свой store.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairingPayload {
    pub anchor_id: HashId,
    /// Hex-encoded X25519 public key anchor'а (открытый, не секрет).
    pub anchor_x25519_hex: String,
    /// SHA-256 fingerprint TLS-сертификата anchor'а (тот же, что `TlsIdentity::fingerprint_hex`).
    pub fingerprint_hex: String,
    /// `wss://host:port/` — куда мобилка коннектится.
    pub anchor_url: String,
}

impl PairingPayload {
    /// Форма публичных полей: произвольный текст в них не пройдёт. Секрет «правильной формы» (например, 64 hex в допустимом поле) эта проверка распознать не может — границы гарантии в `docs/RESUME_REDESIGN.md`, раздел 5.
    pub fn validate(&self) -> Result<()> {
        if !is_hex(&self.anchor_x25519_hex, 64) {
            bail!("anchor_x25519_hex: нужно 64 hex-знака в нижнем регистре");
        }
        if !is_hex(&self.fingerprint_hex, 64) {
            bail!("fingerprint_hex: нужно 64 hex-знака в нижнем регистре");
        }
        // адрес разбирается штатным разборщиком: схема wss, узел обязателен, без имени пользователя, пароля, запроса и фрагмента
        if self.anchor_url.len() > 512 {
            bail!("anchor_url: длиннее 512 знаков");
        }
        let u = url::Url::parse(&self.anchor_url).map_err(|e| anyhow::anyhow!("anchor_url: {e}"))?;
        if u.scheme() != "wss" || u.host_str().map_or(true, str::is_empty) || !u.username().is_empty() || u.password().is_some() || u.query().is_some() || u.fragment().is_some() || u.path() != "/" {
            bail!("anchor_url: нужен wss://узел[:порт]/ без имени пользователя, пароля, пути, запроса и фрагмента");
        }
        Ok(())
    }

    /// Сериализовать в compact JSON (то что попадёт в QR).
    pub fn to_qr_string(&self) -> String {
        serde_json::to_string(self).expect("serialize PairingPayload")
    }

    pub fn from_qr_string(s: &str) -> Result<Self> {
        let p = serde_json::from_str::<Self>(s).context("parse PairingPayload from QR")?;
        p.validate()?;
        Ok(p)
    }
}

// ----------------- Grant -----------------

/// Запись якоря о праве клиента. Секретного материала не содержит.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClientGrant {
    /// Случайный номер права (32 hex-знака); служит только для различения записей и сообщений, не секрет.
    pub session_id_hex: String,
    pub expires_at: u64,
}

impl ClientGrant {
    pub fn new(ttl_secs: u64) -> Self {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut b);
        Self { session_id_hex: hex::encode(b), expires_at: now_secs().saturating_add(ttl_secs) }
    }

    pub fn is_expired(&self) -> bool {
        now_secs() > self.expires_at
    }

    pub fn refresh(&mut self, ttl_secs: u64) {
        self.expires_at = now_secs().saturating_add(ttl_secs);
    }
}

/// Карта JSON без повторяющихся ключей (обычная схлопывает дубликаты молча, оставляя последний).
fn unique_map<'de, D, V>(d: D) -> std::result::Result<HashMap<String, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Vis<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Vis<V> {
        type Value = HashMap<String, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("объект JSON с неповторяющимися ключами")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> std::result::Result<Self::Value, A::Error> {
            let mut m = HashMap::new();
            while let Some((k, v)) = a.next_entry::<String, V>()? {
                if m.insert(k, v).is_some() {
                    return Err(serde::de::Error::custom("повторяющийся ключ"));
                }
            }
            Ok(m)
        }
    }
    d.deserialize_map(Vis(std::marker::PhantomData))
}

// ----------------- Старый формат (v1): строгое описание -----------------
// v1 допускается ровно в этом виде; любое отклонение — `Corrupt`, а не «пустое состояние». Секретные поля читаются, чтобы убедиться,
// что это действительно старый файл, и нигде не сохраняются.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct OldGrant {
    session_id: u128,
    resume_secret_hex: String,
    expires_at: u64,
    #[serde(default)]
    session_key_hex: Option<String>,
    #[serde(default)]
    node_id_hex: Option<String>,
}

impl OldGrant {
    /// Допустимая историческая форма старых полей: секреты были 32 байтами в hex; иначе это не файл прежней сборки.
    fn validate(&self) -> Result<()> {
        let hex64 = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex64(&self.resume_secret_hex) || self.session_key_hex.as_deref().is_some_and(|k| !hex64(k)) || self.node_id_hex.as_deref().is_some_and(|k| !hex64(k)) {
            bail!("поля старой записи не той формы");
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct OldClients {
    #[serde(deserialize_with = "unique_map")]
    clients: HashMap<String, OldGrant>,
}

impl OldClients {
    fn validate(&self) -> Result<()> {
        for (k, g) in &self.clients {
            if !is_hex(k, 64) {
                bail!("ключ клиента не 64 hex-знака");
            }
            g.validate()?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OldAnchorEntry {
    payload: PairingPayload,
    #[serde(default)]
    #[allow(dead_code)]
    session: Option<OldGrant>,
    #[serde(default)]
    preference: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OldAnchors {
    anchors: Vec<OldAnchorEntry>,
}

/// Только версия формата; остальное не разбирается.
fn probe_version(text: &str) -> std::result::Result<Option<u32>, String> {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        version: Option<u32>,
    }
    serde_json::from_str::<Probe>(text).map(|p| p.version).map_err(|e| format!("не JSON-объект: {e}"))
}

// ----------------- Чтение, запись, блокировка -----------------

/// Чем закончилось чтение файла хранилища. Ошибки не превращаются в «пустое состояние»: повреждённый или нечитаемый файл не
/// перезаписывается молча.
#[derive(Debug)]
pub enum StoreRead<T> {
    /// Файла нет — чистое начало.
    Missing,
    /// Формат v2, прочитан строго.
    Current(T),
    /// Старый формат (v1, возможно с секретами): содержимое очищено от секретов, права клиентов не перенесены.
    Migrated(T),
    /// Версия выше известной: файл не трогаем.
    Unsupported(u32),
    /// Не разобран как ни один из известных форматов.
    Corrupt(String),
    /// Ошибка ввода-вывода или прав доступа.
    Io(String),
}

fn read_text(path: &Path) -> std::result::Result<Option<String>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // открытие без перехода по ссылке и проверка уже открытого дескриптора: между проверкой и чтением путь подменить нельзя;
        // O_NONBLOCK — чтобы FIFO на месте файла не повесил открытие до проверки типа (обычному файлу флаг не мешает)
        let f = match fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{e}")),
        };
        let m = f.metadata().map_err(|e| e.to_string())?;
        if !m.is_file() {
            return Err("не обычный файл".into());
        }
        if m.len() > MAX_STORE_BYTES {
            return Err("файл слишком велик".into());
        }
        let mut s = String::new();
        f.take(MAX_STORE_BYTES + 1).read_to_string(&mut s).map_err(|e| e.to_string())?;
        if s.len() as u64 > MAX_STORE_BYTES {
            return Err("файл слишком велик".into());
        }
        Ok(Some(s))
    }
    #[cfg(not(unix))]
    {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{e}")),
            Ok(m) if m.file_type().is_symlink() || !m.is_file() => return Err("не обычный файл".into()),
            Ok(m) if m.len() > MAX_STORE_BYTES => return Err("файл слишком велик".into()),
            Ok(_) => {}
        }
        fs::read_to_string(path).map(Some).map_err(|e| e.to_string())
    }
}

/// Одна транзакция над файлом хранилища: блокировка на стабильном соседнем файле `<имя>.lock` (flock) держится на всё чтение,
/// преобразование и запись — и между потоками, и между процессами (CLI-импорт, выдача права, миграция, восстановление).
/// Вложенный вызов из того же потока не допускается (взаимная блокировка).
fn with_lock<R>(path: &Path, f: impl FnOnce() -> R) -> Result<R> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        if let Some(parent) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "store".into());
        let lock_path = path.with_file_name(format!(".{name}.lock"));
        let lock = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&lock_path)
            .with_context(|| format!("блокировка {:?}", lock_path))?;
        // flock принадлежит открытому описанию файла: каждый вызов открывает файл заново, поэтому потоки одного процесса тоже исключают друг друга
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("flock {:?}: {}", lock_path, std::io::Error::last_os_error());
        }
        let r = f();
        drop(lock); // закрытие снимает блокировку
        Ok(r)
    }
    #[cfg(not(unix))]
    {
        static FALLBACK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = path;
        Ok(f())
    }
}

/// Выполнить `f` под блокировкой файла хранилища (для других модулей: восстановление, очистка). Вложенный вызов для того же файла запрещён.
pub(crate) fn locked<R>(path: &Path, f: impl FnOnce() -> R) -> Result<R> {
    with_lock(path, f)
}

/// Результат очистки одного файла связи на месте.
#[derive(Debug, PartialEq, Eq)]
pub enum Cleaned {
    Absent,
    AlreadyClean,
    Rewritten,
    /// Файл не разбирается по строгой схеме: содержимое не проверено, файл не тронут.
    Unparseable,
}

/// Очистить файл связи на месте: прочитать без перехода по ссылке, пересобрать проверенную модель, записать. `locked_file` — под блокировкой
/// файла (активные файлы хранилища); для остатков восстановления блокировка не нужна (их никто не обновляет) и файл `.lock` рядом не создаётся.
pub fn sanitize_file_in_place(kind: StoreKind, path: &Path, locked_file: bool) -> Result<Cleaned> {
    let body = || -> Result<Cleaned> {
        let Some(text) = read_text(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))? else { return Ok(Cleaned::Absent) };
        let Some(clean) = sanitized_store_text(kind, text.as_bytes()) else { return Ok(Cleaned::Unparseable) };
        if clean == text {
            return Ok(Cleaned::AlreadyClean);
        }
        write_text(path, &clean)?;
        Ok(Cleaned::Rewritten)
    };
    if locked_file {
        with_lock(path, body)?
    } else {
        body()
    }
}

/// Запись без блокировки (вызывается только под ней): лимит размера, приватный файл, атомарная замена.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(value).context("serialize store")?;
    if s.len() as u64 > MAX_STORE_BYTES {
        bail!("хранилище больше допустимого ({} байт)", MAX_STORE_BYTES);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
    }
    write_text(path, &s)
}

/// Запись текста файла связи: приватно и атомарно. Если переименование выполнено, а сбросить каталог на диск не удалось, запись считается
/// выполненной, но владельцу об этом прямо сообщается в журнал (долговечность не подтверждена).
fn write_text(path: &Path, text: &str) -> Result<()> {
    match crate::util::private_file::write_private_checked(path, text.as_bytes()).with_context(|| format!("write {:?}", path))? {
        crate::util::private_file::Committed::Durable => {}
        crate::util::private_file::Committed::DurabilityUnknown(why) => {
            eprintln!("[pairing] {}: запись выполнена, но долговечность не подтверждена (каталог не сброшен на диск: {why})", path.display());
        }
    }
    Ok(())
}

// ----------------- Stores -----------------

/// На anchor-стороне: кому выдано право. Ключ — hex public_key клиента (Ed25519 32B).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedClientStore {
    pub version: u32,
    #[serde(deserialize_with = "unique_map")]
    pub clients: HashMap<String, ClientGrant>,
    /// Файл не удалось прочитать как известный формат (или очистить): хранилище пустое в памяти и **записывать его нельзя**.
    #[serde(skip)]
    pub blocked: Option<String>,
}

impl Default for PairedClientStore {
    fn default() -> Self {
        Self { version: STORE_VERSION, clients: HashMap::new(), blocked: None }
    }
}

impl PairedClientStore {
    fn check(&self) -> Result<()> {
        if self.version != STORE_VERSION {
            bail!("версия {} не поддерживается", self.version);
        }
        if self.clients.len() > MAX_CLIENTS {
            bail!("слишком много прав ({} > {MAX_CLIENTS})", self.clients.len());
        }
        for (k, g) in &self.clients {
            if !is_hex(k, 64) {
                bail!("ключ клиента: нужно 64 hex-знака в нижнем регистре");
            }
            if !is_hex(&g.session_id_hex, 32) {
                bail!("session_id_hex: нужно 32 hex-знака");
            }
        }
        Ok(())
    }

    /// Разбор текста без ввода-вывода: одна проверка для чтения, очистки и копий (P18-3).
    fn parse(text: &str) -> StoreRead<Self> {
        match probe_version(text) {
            Err(e) => StoreRead::Corrupt(e),
            Ok(Some(STORE_VERSION)) => match serde_json::from_str::<Self>(text) {
                Ok(s) => match s.check() {
                    Ok(()) => StoreRead::Current(s),
                    Err(e) => StoreRead::Corrupt(format!("схема v2: {e}")),
                },
                Err(e) => StoreRead::Corrupt(format!("схема v2: {e}")),
            },
            Ok(Some(v)) => StoreRead::Unsupported(v),
            // старый формат: права клиентов не переносятся
            Ok(None) => match serde_json::from_str::<OldClients>(text) {
                Ok(o) => match o.validate() {
                    Ok(()) => StoreRead::Migrated(Self::default()),
                    Err(e) => StoreRead::Corrupt(format!("схема v1: {e}")),
                },
                Err(e) => StoreRead::Corrupt(format!("схема v1: {e}")),
            },
        }
    }

    fn read_unlocked(path: &Path) -> StoreRead<Self> {
        match read_text(path) {
            Ok(None) => StoreRead::Missing,
            Ok(Some(t)) => Self::parse(&t),
            Err(e) => StoreRead::Io(e),
        }
    }

    /// Строгое чтение (под блокировкой). Миграцию не выполняет.
    pub fn read(path: &Path) -> StoreRead<Self> {
        match with_lock(path, || Self::read_unlocked(path)) {
            Ok(r) => r,
            Err(e) => StoreRead::Io(e.to_string()),
        }
    }

    fn blocked_with(reason: String) -> Self {
        Self { blocked: Some(reason), ..Self::default() }
    }

    /// Состояние для транзакции или работы узла. Старый формат переписывается без секретов; не удалось — хранилище **блокируется** с
    /// понятной причиной (секреты ещё на диске), а не выдаётся рабочим. Вызывается под блокировкой.
    fn load_unlocked(path: &Path) -> Self {
        match Self::read_unlocked(path) {
            StoreRead::Missing => Self::default(),
            StoreRead::Current(s) => s,
            StoreRead::Migrated(s) => match write_json(path, &s) {
                Ok(()) => {
                    eprintln!("[pairing] {}: старый формат с секретами заменён пустым списком формата v2 (права клиентов не перенесены, нужно новое сопряжение)", path.display());
                    s
                }
                Err(e) => {
                    eprintln!("[pairing] {}: СЕКРЕТЫ СТАРОГО ФОРМАТА ОСТАЛИСЬ НА ДИСКЕ: не удалось переписать файл ({e}); хранилище заблокировано", path.display());
                    Self::blocked_with(format!("секреты старого формата остались на диске: файл не переписан ({e})"))
                }
            },
            StoreRead::Unsupported(v) => Self::blocked_with(format!("версия формата {v} выше известной")),
            StoreRead::Corrupt(e) => Self::blocked_with(format!("файл повреждён: {e}")),
            StoreRead::Io(e) => Self::blocked_with(format!("файл не читается: {e}")),
        }
    }

    /// Загрузка для работы узла (с миграцией под блокировкой).
    pub fn load_or_default(path: &Path) -> Self {
        with_lock(path, || Self::load_unlocked(path)).unwrap_or_else(|e| Self::blocked_with(format!("блокировка недоступна: {e}")))
    }

    /// Транзакция: блокировка → строгое чтение → изменение копии → проверка → запись. Память и диск меняются только вместе: при ошибке
    /// записи возвращается ошибка, а прежнее состояние файла сохраняется. Заблокированное хранилище не изменяется.
    pub fn update<R>(path: &Path, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        with_lock(path, || {
            let mut st = Self::load_unlocked(path);
            if let Some(why) = &st.blocked {
                bail!("хранилище не изменяется: {why}");
            }
            let r = f(&mut st)?;
            st.check()?;
            write_json(path, &st)?;
            Ok(r)
        })?
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        if let Some(why) = &self.blocked {
            bail!("хранилище не записывается: {why}");
        }
        self.check()?;
        with_lock(path, || write_json(path, self))?
    }

    pub fn issue(&mut self, client_pubkey_hex: String, ttl_secs: u64) -> ClientGrant {
        let g = ClientGrant::new(ttl_secs);
        self.clients.insert(client_pubkey_hex, g.clone());
        g
    }

    pub fn get(&self, client_pubkey_hex: &str) -> Option<&ClientGrant> {
        self.clients.get(client_pubkey_hex)
    }

    pub fn refresh(&mut self, client_pubkey_hex: &str, ttl_secs: u64) -> bool {
        if let Some(t) = self.clients.get_mut(client_pubkey_hex) {
            t.refresh(ttl_secs);
            true
        } else {
            false
        }
    }

    pub fn revoke(&mut self, client_pubkey_hex: &str) -> bool {
        self.clients.remove(client_pubkey_hex).is_some()
    }

    pub fn gc_expired(&mut self) -> usize {
        let before = self.clients.len();
        self.clients.retain(|_, t| !t.is_expired());
        before - self.clients.len()
    }
}

/// На клиентской стороне: наши якоря в порядке предпочтения. Секретов нет: только то, что было в QR, и порядок.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedAnchorStore {
    pub version: u32,
    pub anchors: Vec<PairedAnchorEntry>,
    #[serde(skip)]
    pub blocked: Option<String>,
}

impl Default for PairedAnchorStore {
    fn default() -> Self {
        Self { version: STORE_VERSION, anchors: Vec::new(), blocked: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairedAnchorEntry {
    pub payload: PairingPayload,
    /// Преферанс (меньше — выше в очереди). Не обязательно уникальный.
    pub preference: u32,
}

impl PairedAnchorStore {
    fn check(&self) -> Result<()> {
        if self.version != STORE_VERSION {
            bail!("версия {} не поддерживается", self.version);
        }
        if self.anchors.len() > MAX_ANCHORS {
            bail!("слишком много якорей ({} > {MAX_ANCHORS})", self.anchors.len());
        }
        let mut seen = std::collections::HashSet::new();
        for a in &self.anchors {
            a.payload.validate()?;
            if !seen.insert(a.payload.anchor_id) {
                bail!("якорь указан дважды");
            }
        }
        Ok(())
    }

    /// Разбор текста без ввода-вывода: одна проверка для чтения, очистки и копий (P18-3).
    fn parse(text: &str) -> StoreRead<Self> {
        match probe_version(text) {
            Err(e) => StoreRead::Corrupt(e),
            Ok(Some(STORE_VERSION)) => match serde_json::from_str::<Self>(text) {
                Ok(s) => match s.check() {
                    Ok(()) => StoreRead::Current(s),
                    Err(e) => StoreRead::Corrupt(format!("схема v2: {e}")),
                },
                Err(e) => StoreRead::Corrupt(format!("схема v2: {e}")),
            },
            Ok(Some(v)) => StoreRead::Unsupported(v),
            Ok(None) => match serde_json::from_str::<OldAnchors>(text) {
                Ok(o) if o.anchors.iter().any(|e| e.session.as_ref().is_some_and(|g| g.validate().is_err())) => StoreRead::Corrupt("схема v1: поля старой записи не той формы".into()),
                Ok(o) => {
                    let s = Self {
                        version: STORE_VERSION,
                        anchors: o.anchors.into_iter().map(|e| PairedAnchorEntry { payload: e.payload, preference: e.preference }).collect(),
                        blocked: None,
                    };
                    match s.check() {
                        Ok(()) => StoreRead::Migrated(s),
                        Err(e) => StoreRead::Corrupt(format!("схема v1: {e}")),
                    }
                }
                Err(e) => StoreRead::Corrupt(format!("схема v1: {e}")),
            },
        }
    }

    fn read_unlocked(path: &Path) -> StoreRead<Self> {
        match read_text(path) {
            Ok(None) => StoreRead::Missing,
            Ok(Some(t)) => Self::parse(&t),
            Err(e) => StoreRead::Io(e),
        }
    }

    pub fn read(path: &Path) -> StoreRead<Self> {
        match with_lock(path, || Self::read_unlocked(path)) {
            Ok(r) => r,
            Err(e) => StoreRead::Io(e.to_string()),
        }
    }

    fn blocked_with(reason: String) -> Self {
        Self { blocked: Some(reason), ..Self::default() }
    }

    fn load_unlocked(path: &Path) -> Self {
        match Self::read_unlocked(path) {
            StoreRead::Missing => Self::default(),
            StoreRead::Current(s) => s,
            StoreRead::Migrated(s) => match write_json(path, &s) {
                Ok(()) => {
                    eprintln!("[pairing] {}: старый формат заменён форматом v2 без секретов (адреса и порядок якорей сохранены; для права на якоре нужно новое сопряжение)", path.display());
                    s
                }
                Err(e) => {
                    eprintln!("[pairing] {}: СЕКРЕТЫ СТАРОГО ФОРМАТА ОСТАЛИСЬ НА ДИСКЕ: не удалось переписать файл ({e}); хранилище заблокировано", path.display());
                    Self::blocked_with(format!("секреты старого формата остались на диске: файл не переписан ({e})"))
                }
            },
            StoreRead::Unsupported(v) => Self::blocked_with(format!("версия формата {v} выше известной")),
            StoreRead::Corrupt(e) => Self::blocked_with(format!("файл повреждён: {e}")),
            StoreRead::Io(e) => Self::blocked_with(format!("файл не читается: {e}")),
        }
    }

    pub fn load_or_default(path: &Path) -> Self {
        with_lock(path, || Self::load_unlocked(path)).unwrap_or_else(|e| Self::blocked_with(format!("блокировка недоступна: {e}")))
    }

    /// Транзакция (см. `PairedClientStore::update`).
    pub fn update<R>(path: &Path, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        with_lock(path, || {
            let mut st = Self::load_unlocked(path);
            if let Some(why) = &st.blocked {
                bail!("хранилище не изменяется: {why}");
            }
            let r = f(&mut st)?;
            st.check()?;
            write_json(path, &st)?;
            Ok(r)
        })?
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        if let Some(why) = &self.blocked {
            bail!("хранилище не записывается: {why}");
        }
        self.check()?;
        with_lock(path, || write_json(path, self))?
    }

    pub fn add_or_update(&mut self, payload: PairingPayload, preference: u32) {
        if let Some(existing) = self.anchors.iter_mut().find(|e| e.payload.anchor_id == payload.anchor_id) {
            existing.payload = payload;
            existing.preference = preference;
        } else {
            self.anchors.push(PairedAnchorEntry { payload, preference });
        }
        self.anchors.sort_by_key(|e| e.preference);
    }

    pub fn primary(&self) -> Option<&PairedAnchorEntry> {
        self.anchors.first()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    Clients,
    Anchors,
}

/// Очистка текста файла хранилища для резервной копии и восстановления: возвращает **заново сериализованную проверенную модель**
/// формата v2 (никогда не исходные байты) или `None`, если файл нельзя разобрать по строгой схеме (такой файл в копию не кладётся).
pub fn sanitized_store_text(kind: StoreKind, bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.len() as u64 > MAX_STORE_BYTES {
        return None;
    }
    match kind {
        StoreKind::Clients => match PairedClientStore::read_text_model(text)? {
            m => serde_json::to_string_pretty(&m).ok(),
        },
        StoreKind::Anchors => match PairedAnchorStore::read_text_model(text)? {
            m => serde_json::to_string_pretty(&m).ok(),
        },
    }
}

impl PairedClientStore {
    fn read_text_model(text: &str) -> Option<Self> {
        match Self::parse(text) {
            StoreRead::Current(s) | StoreRead::Migrated(s) => Some(s),
            _ => None,
        }
    }
}

impl PairedAnchorStore {
    fn read_text_model(text: &str) -> Option<Self> {
        match Self::parse(text) {
            StoreRead::Current(s) | StoreRead::Migrated(s) => Some(s),
            _ => None,
        }
    }
}

pub fn default_paired_clients_path() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".yandi").join("paired_clients.json")
}

pub fn default_paired_anchors_path() -> PathBuf {
    // Hardening Step 6: CLI `--anchor-store <path>` имеет приоритет.
    if let Some(p) = crate::netlayer::packet::anchor_store_override() {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".yandi").join("paired_anchors.json")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ----------------- Tests -----------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Известные «секреты» старого формата: тесты проверяют, что их нет ни в новых файлах, ни в копиях.
    const OLD_RESUME_SECRET: &str = "11aa22bb33cc44dd55ee66ff77008899aabbccddeeff00112233445566778899";
    const OLD_SESSION_KEY: &str = "99887766554433221100ffeeddccbbaa99887766554433221100ffeeddccbbaa";
    const BIG_SESSION_ID: &str = "340282366920938463463374607431768211455"; // u128::MAX: старые номера не помещались в общее дерево JSON

    fn old_clients_json() -> String {
        format!(
            r#"{{"clients":{{"{pk}":{{"session_id":{BIG_SESSION_ID},"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":4102444800,"session_key_hex":"{OLD_SESSION_KEY}","node_id_hex":"{pk}"}}}}}}"#,
            pk = "ab".repeat(32)
        )
    }

    fn payload() -> PairingPayload {
        PairingPayload {
            anchor_id: HashId([0xAB; 32]),
            anchor_x25519_hex: hex::encode([0x33u8; 32]),
            fingerprint_hex: "abcdef".repeat(10) + "abcd",
            anchor_url: "wss://anchor.example:443/".into(),
        }
    }

    fn old_anchors_json() -> String {
        let p = serde_json::to_string(&payload()).unwrap();
        format!(
            r#"{{"anchors":[{{"payload":{p},"session":{{"session_id":{BIG_SESSION_ID},"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":4102444800,"session_key_hex":"{OLD_SESSION_KEY}"}},"preference":3}}]}}"#
        )
    }

    fn no_old_secrets(text: &str) {
        for needle in [OLD_RESUME_SECRET, OLD_SESSION_KEY, "resume_secret_hex", "session_key_hex", BIG_SESSION_ID] {
            assert!(!text.contains(needle), "в файле остался {needle}");
        }
    }

    #[test]
    fn pairing_payload_qr_roundtrip() {
        let p = payload();
        let back = PairingPayload::from_qr_string(&p.to_qr_string()).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn a_grant_is_unique_not_a_secret_and_expires() {
        let a = ClientGrant::new(100);
        let b = ClientGrant::new(100);
        assert_ne!(a.session_id_hex, b.session_id_hex);
        assert!(!a.is_expired());
        let mut t = ClientGrant::new(60);
        t.expires_at = 1;
        assert!(t.is_expired());
        t.refresh(60);
        assert!(!t.is_expired());
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("session_id_hex") && json.contains("expires_at") && !json.contains("secret") && !json.contains("key"), "{json}");
    }

    #[test]
    fn the_client_store_roundtrips_in_v2_without_any_secret_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        let mut st = PairedClientStore::default();
        let g = st.issue("cd".repeat(32), 3600);
        st.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"version\": 2") && !text.contains("secret") && !text.contains("session_key"), "{text}");
        match PairedClientStore::read(&path) {
            StoreRead::Current(back) => assert_eq!(back.get(&"cd".repeat(32)), Some(&g)),
            other => panic!("{other:?}"),
        }
        assert!(st.revoke(&"cd".repeat(32)) && !st.revoke(&"cd".repeat(32)));
        let mut e = PairedClientStore::default();
        e.clients.insert("x".into(), ClientGrant { session_id_hex: "0".repeat(32), expires_at: 1 });
        assert_eq!(e.gc_expired(), 1);
    }

    #[test]
    fn an_old_clients_file_is_rewritten_without_secrets_and_its_rights_are_not_carried_over() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        fs::write(&path, old_clients_json()).unwrap();
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Migrated(_)));
        let st = PairedClientStore::load_or_default(&path);
        assert!(st.clients.is_empty(), "право из старого файла новым не становится");
        assert!(st.blocked.is_none());
        let text = fs::read_to_string(&path).unwrap();
        no_old_secrets(&text);
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Current(_)), "после переписывания файл читается как v2");
    }

    #[test]
    fn an_old_anchors_file_keeps_the_public_data_and_order_but_loses_the_session() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_anchors.json");
        fs::write(&path, old_anchors_json()).unwrap();
        let st = PairedAnchorStore::load_or_default(&path);
        assert_eq!(st.anchors.len(), 1);
        assert_eq!(st.anchors[0].payload, payload());
        assert_eq!(st.anchors[0].preference, 3);
        no_old_secrets(&fs::read_to_string(&path).unwrap());
    }

    #[test]
    fn a_v2_file_with_old_secret_fields_is_refused_and_left_untouched() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        let text = format!(r#"{{"version":2,"clients":{{"{pk}":{{"session_id_hex":"{z}","expires_at":9,"resume_secret_hex":"{OLD_RESUME_SECRET}"}}}}}}"#, pk = "ab".repeat(32), z = "0".repeat(32));
        fs::write(&path, &text).unwrap();
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Corrupt(_)), "старое поле нельзя прочитать как новое");
        let st = PairedClientStore::load_or_default(&path);
        assert!(st.blocked.is_some() && st.clients.is_empty());
        assert!(st.save(&path).is_err(), "заблокированное хранилище не записывается");
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "файл не изменён");
    }

    #[test]
    fn corrupt_future_and_unreadable_files_are_not_replaced_by_an_empty_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_anchors.json");
        fs::write(&path, b"{not json").unwrap();
        assert!(matches!(PairedAnchorStore::read(&path), StoreRead::Corrupt(_)));
        let st = PairedAnchorStore::load_or_default(&path);
        assert!(st.blocked.is_some() && st.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{not json");
        fs::write(&path, br#"{"version":99,"anchors":[]}"#).unwrap();
        assert!(matches!(PairedAnchorStore::read(&path), StoreRead::Unsupported(99)));
        assert!(PairedAnchorStore::load_or_default(&path).blocked.is_some());
        // каталог на месте файла: ошибка ввода-вывода, а не «пусто»
        let d = dir.path().join("dir.json");
        fs::create_dir(&d).unwrap();
        assert!(matches!(PairedAnchorStore::read(&d), StoreRead::Io(_)));
        assert!(matches!(PairedAnchorStore::read(&dir.path().join("none.json")), StoreRead::Missing));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_not_followed() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.json");
        fs::write(&real, old_clients_json()).unwrap();
        let link = dir.path().join("paired_clients.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(PairedClientStore::read(&link), StoreRead::Io(_)));
        assert_eq!(fs::read_to_string(&real).unwrap(), old_clients_json(), "цель ссылки не тронута");
    }

    #[cfg(unix)]
    #[test]
    fn p18_6_a_fifo_in_place_of_the_file_is_refused_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempdir().unwrap();
        let fifo = dir.path().join("paired_clients.json");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(matches!(PairedClientStore::read(&fifo), StoreRead::Io(_)));
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true), "чтение FIFO не должно зависать");
    }

    #[test]
    fn backups_and_restores_sanitize_both_files_and_drop_what_cannot_be_read() {
        let c = sanitized_store_text(StoreKind::Clients, old_clients_json().as_bytes()).unwrap();
        no_old_secrets(&c);
        let a = sanitized_store_text(StoreKind::Anchors, old_anchors_json().as_bytes()).unwrap();
        no_old_secrets(&a);
        assert!(a.contains("\"preference\": 3"));
        // уже чистый v2 проходит как есть
        let v2 = serde_json::to_string_pretty(&PairedClientStore::default()).unwrap();
        assert_eq!(sanitized_store_text(StoreKind::Clients, v2.as_bytes()).unwrap(), v2);
        // мусор, будущая версия, v2 с секретным полем — в копию не попадают
        assert!(sanitized_store_text(StoreKind::Clients, b"\xff\xfe").is_none());
        assert!(sanitized_store_text(StoreKind::Anchors, b"{oops").is_none());
        assert!(sanitized_store_text(StoreKind::Clients, br#"{"version":9,"clients":{}}"#).is_none());
        assert!(sanitized_store_text(StoreKind::Clients, format!(r#"{{"version":2,"clients":{{}},"resume_secret_hex":"{OLD_RESUME_SECRET}"}}"#).as_bytes()).is_none());
    }


    // ---- замечания проверки Sol (круг 36): P16-1…5 ----

    fn anchors_v2_with(payload_json: &str) -> String {
        format!(r#"{{"version":2,"anchors":[{{"payload":{payload_json},"preference":1}}]}}"#)
    }

    #[test]
    fn p16_1_a_secret_hidden_in_a_nested_or_escaped_field_is_refused_everywhere() {
        let ok = serde_json::to_string(&payload()).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&ok).unwrap();
        // вложенное неизвестное поле, экранированное имя и произвольный текст в разрешённом поле
        v["resume_secret_hex"] = serde_json::json!(OLD_RESUME_SECRET);
        let nested = anchors_v2_with(&v.to_string());
        let escaped = anchors_v2_with(&ok.replace("anchor_url", "\\u0061nchor_url\\u0020\\u0073ecret").to_string());
        let free_text = anchors_v2_with(&ok.replace("wss://anchor.example:443/", &format!("wss://x/{OLD_SESSION_KEY} {OLD_RESUME_SECRET}")));
        for (name, text) in [("вложенное поле", nested), ("экранированное имя", escaped), ("секрет в тексте адреса", free_text)] {
            assert!(sanitized_store_text(StoreKind::Anchors, text.as_bytes()).is_none(), "{name}: в копию не пропускается");
            let dir = tempdir().unwrap();
            let path = dir.path().join("paired_anchors.json");
            fs::write(&path, &text).unwrap();
            assert!(matches!(PairedAnchorStore::read(&path), StoreRead::Corrupt(_)), "{name}: читатель отвергает");
        }
        // допустимый файл возвращается заново сериализованной моделью, а не исходным текстом
        let messy = "{ \"version\" : 2 ,\n\"anchors\":[ ] }".to_string();
        let clean = sanitized_store_text(StoreKind::Anchors, messy.as_bytes()).unwrap();
        assert_ne!(clean, messy, "возвращается заново сериализованная модель, а не исходный текст");
        assert!(serde_json::from_str::<PairedAnchorStore>(&clean).is_ok());
    }

    #[test]
    fn p16_1_public_fields_must_have_the_shape_of_what_they_are() {
        let mut p = payload();
        assert!(p.validate().is_ok());
        p.anchor_x25519_hex = format!("{OLD_RESUME_SECRET}00");
        assert!(p.validate().is_err(), "не 64 знака");
        let mut p = payload();
        p.fingerprint_hex = "Z".repeat(64);
        assert!(p.validate().is_err(), "не hex");
        let mut p = payload();
        p.anchor_url = "http://anchor/".into();
        assert!(p.validate().is_err(), "не wss");
        // граница гарантии: имя узла правильной формы может быть любой строкой из допустимых знаков — хранилище такое не распознаёт как секрет
        p.anchor_url = format!("wss://{}.example/", &OLD_RESUME_SECRET[..40]);
        assert!(p.validate().is_ok(), "секрет «правильной формы» внутри допустимого поля отличить нельзя; это сказано в документах");
        p.anchor_url = "wss://a b/".into();
        assert!(p.validate().is_err(), "пробел в адресе");
        assert!(PairingPayload::from_qr_string(&format!(r#"{{"anchor_id":"{}","anchor_x25519_hex":"{}","fingerprint_hex":"{}","anchor_url":"wss://a/","extra":1}}"#, hex::encode([1u8; 32]), "a".repeat(64), "b".repeat(64))).is_err(), "лишнее поле в QR");
    }

    #[test]
    fn p16_3_only_a_wellformed_v1_is_migrated_and_anything_else_is_corrupt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        for (name, text) in [
            ("пустой объект", "{}".to_string()),
            ("без clients", r#"{"foo":1}"#.to_string()),
            ("clients не объект", r#"{"clients":[1,2]}"#.to_string()),
            ("запись без секрета", format!(r#"{{"clients":{{"{pk}":{{"session_id":1,"expires_at":9}}}}}}"#, pk = "ab".repeat(32))),
            ("ключ клиента не hex", format!(r#"{{"clients":{{"zz":{{"session_id":1,"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":9}}}}}}"#)),
            ("лишнее поле в токене", format!(r#"{{"clients":{{"{pk}":{{"session_id":1,"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":9,"x":1}}}}}}"#, pk = "ab".repeat(32))),
            ("version 1", r#"{"version":1,"clients":{}}"#.to_string()),
        ] {
            fs::write(&path, &text).unwrap();
            assert!(!matches!(PairedClientStore::read(&path), StoreRead::Migrated(_) | StoreRead::Current(_)), "{name}: не принимается как v1/v2");
            assert_eq!(fs::read_to_string(&path).unwrap(), text, "{name}: файл не изменён");
        }
        fs::write(&path, old_clients_json()).unwrap();
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Migrated(_)), "настоящий v1 принимается");
        // старые якоря: несколько записей сохраняют порядок и равные preference; запись с неверной публичной частью — Corrupt
        let a1 = payload();
        let mut a2 = payload();
        a2.anchor_id = HashId([0xCD; 32]);
        let text = serde_json::json!({"anchors": [
            {"payload": a1, "preference": 2},
            {"payload": a2, "preference": 2, "session": {"session_id": 5, "resume_secret_hex": OLD_RESUME_SECRET, "expires_at": 1}},
        ]}).to_string();
        let apath = dir.path().join("paired_anchors.json");
        fs::write(&apath, &text).unwrap();
        let st = PairedAnchorStore::load_or_default(&apath);
        assert_eq!(st.anchors.len(), 2);
        assert!(st.anchors.iter().all(|e| e.preference == 2));
        no_old_secrets(&fs::read_to_string(&apath).unwrap());
        let bad = text.replace("wss://anchor.example:443/", "ftp://nope");
        fs::write(&apath, &bad).unwrap();
        assert!(matches!(PairedAnchorStore::read(&apath), StoreRead::Corrupt(_)));
    }

    #[cfg(unix)]
    #[test]
    fn p16_4_a_failed_cleanup_blocks_the_store_and_says_the_secrets_are_still_on_disk() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let sub = dir.path().join("ro");
        fs::create_dir(&sub).unwrap();
        let path = sub.join("paired_clients.json");
        fs::write(&path, old_clients_json()).unwrap();
        // перед сбоем блокировку можно взять: каталог пока доступен; убираем право записи после создания файла блокировки
        let _ = PairedClientStore::read(&path);
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o500)).unwrap();
        let st = PairedClientStore::load_or_default(&path);
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o700)).unwrap();
        let why = st.blocked.clone().expect("хранилище заблокировано");
        assert!(why.contains("остались на диске") || why.contains("блокировка недоступна") || why.contains("не переписан"), "{why}");
        assert!(st.save(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), old_clients_json(), "файл не изменён, секреты ещё на диске — и это сказано");
        // когда запись снова возможна, следующая попытка доводит очистку до конца
        PairedClientStore::update(&path, |_| Ok(())).unwrap();
        no_old_secrets(&fs::read_to_string(&path).unwrap());
    }

    #[test]
    fn p16_2_concurrent_updates_do_not_lose_rights() {
        let dir = tempdir().unwrap();
        let path = std::sync::Arc::new(dir.path().join("paired_clients.json"));
        let mut hs = Vec::new();
        for t in 0..8u8 {
            let path = path.clone();
            hs.push(std::thread::spawn(move || {
                for i in 0..10u8 {
                    let key = format!("{:02x}{:02x}{}", t, i, "0".repeat(60));
                    PairedClientStore::update(&path, |st| {
                        st.issue(key.clone(), 3600);
                        Ok(())
                    })
                    .unwrap();
                }
            }));
        }
        for h in hs {
            h.join().unwrap();
        }
        match PairedClientStore::read(&path) {
            StoreRead::Current(st) => assert_eq!(st.clients.len(), 80, "ни одно из 80 прав не потеряно"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn p16_5_a_failed_transaction_changes_nothing_and_limits_are_enforced() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        PairedClientStore::update(&path, |st| {
            st.issue("ab".repeat(32), 3600);
            Ok(())
        })
        .unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let r: Result<()> = PairedClientStore::update(&path, |st| {
            st.issue("cd".repeat(32), 3600);
            bail!("сбой посреди изменения")
        });
        assert!(r.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before, "прежнее состояние файла сохранено");
        // слишком много прав — ни записи, ни тихого усечения
        let r: Result<()> = PairedClientStore::update(&path, |st| {
            for i in 0..=MAX_CLIENTS {
                st.clients.insert(format!("{:064x}", i), ClientGrant { session_id_hex: "0".repeat(32), expires_at: u64::MAX });
            }
            Ok(())
        });
        assert!(r.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        // заблокированное (повреждённое) хранилище не изменяется транзакцией
        fs::write(&path, b"{oops").unwrap();
        assert!(PairedClientStore::update(&path, |_| Ok(())).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{oops");
    }


    // ---- замечания проверки Sol (круг 37): P17-1…5 ----

    #[test]
    fn p17_3_duplicate_keys_and_malformed_old_fields_are_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        let pk = "ab".repeat(32);
        let g = format!(r#"{{"session_id_hex":"{}","expires_at":9}}"#, "0".repeat(32));
        fs::write(&path, format!(r#"{{"version":2,"clients":{{"{pk}":{g},"{pk}":{g}}}}}"#)).unwrap();
        assert!(matches!(PairedClientStore::read(&path), StoreRead::Corrupt(_)), "повторяющийся ключ клиента");
        // старая запись: секретные поля неверной формы — не файл прежней сборки
        for bad in [
            format!(r#"{{"clients":{{"{pk}":{{"session_id":1,"resume_secret_hex":"zz","expires_at":9}}}}}}"#),
            format!(r#"{{"clients":{{"{pk}":{{"session_id":1,"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":9,"session_key_hex":"short"}}}}}}"#),
            format!(r#"{{"clients":{{"{pk}":{{"session_id":1,"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":9}},"{pk}":{{"session_id":2,"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":9}}}}}}"#),
        ] {
            fs::write(&path, &bad).unwrap();
            assert!(matches!(PairedClientStore::read(&path), StoreRead::Corrupt(_)), "{bad}");
        }
        // якорь дважды
        let a = serde_json::to_string(&payload()).unwrap();
        let apath = dir.path().join("paired_anchors.json");
        fs::write(&apath, format!(r#"{{"version":2,"anchors":[{{"payload":{a},"preference":1}},{{"payload":{a},"preference":2}}]}}"#)).unwrap();
        assert!(matches!(PairedAnchorStore::read(&apath), StoreRead::Corrupt(_)), "один якорь дважды");
    }

    #[test]
    fn p18_3_every_reader_classifies_the_same_malformed_bytes_the_same_way() {
        let pk = "ab".repeat(32);
        let a = serde_json::to_string(&payload()).unwrap();
        let bad_clients = [
            format!(r#"{{"clients":{{"{pk}":{{"session_id":1,"resume_secret_hex":"not-hex","expires_at":9}}}}}}"#),
            format!(r#"{{"clients":{{"{pk}":{{"session_id":1,"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":9,"session_key_hex":"short"}}}}}}"#),
        ];
        let bad_anchors = [
            format!(r#"{{"anchors":[{{"payload":{a},"session":{{"session_id":1,"resume_secret_hex":"zz","expires_at":9}},"preference":1}}]}}"#),
            format!(r#"{{"anchors":[{{"payload":{a},"session":{{"session_id":1,"resume_secret_hex":"{OLD_RESUME_SECRET}","expires_at":9,"session_key_hex":"short"}},"preference":1}}]}}"#),
        ];
        let dir = tempdir().unwrap();
        for (kind, name, list) in [(StoreKind::Clients, "paired_clients.json", &bad_clients), (StoreKind::Anchors, "paired_anchors.json", &bad_anchors)] {
            for bad in list.iter() {
                let path = dir.path().join(name);
                fs::write(&path, bad).unwrap();
                let read_corrupt = match kind {
                    StoreKind::Clients => matches!(PairedClientStore::read(&path), StoreRead::Corrupt(_)),
                    StoreKind::Anchors => matches!(PairedAnchorStore::read(&path), StoreRead::Corrupt(_)),
                };
                assert!(read_corrupt, "чтение: {bad}");
                assert!(sanitized_store_text(kind, bad.as_bytes()).is_none(), "копия/восстановление: {bad}");
                assert!(matches!(sanitize_file_in_place(kind, &path, true), Ok(Cleaned::Unparseable)), "очистка на месте: {bad}");
                assert_eq!(fs::read_to_string(&path).unwrap(), *bad, "испорченный файл очисткой не переписывается молча");
            }
        }
    }

    #[test]
    fn p17_1_the_url_is_parsed_by_a_real_parser() {
        for bad in ["wss://user:pw@host/", "wss://host/?token=1", "wss://host/#frag", "wss://host/some/path", "wss://host/ключ пробел", "ws://host/", "wss://ho st/", "https://host/", &format!("wss://{}/", "a".repeat(600))] {
            let mut p = payload();
            p.anchor_url = bad.to_string();
            assert!(p.validate().is_err(), "{bad}");
        }
        for ok in ["wss://anchor.example:443/", "wss://[2001:db8::1]:8766/", "wss://1.2.3.4/", "wss://anchor.example"] {
            let mut p = payload();
            p.anchor_url = ok.to_string();
            assert!(p.validate().is_ok(), "{ok}");
        }
    }

    #[test]
    fn p17_2_cleaning_in_place_runs_under_the_store_lock_and_never_follows_links() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paired_clients.json");
        assert_eq!(sanitize_file_in_place(StoreKind::Clients, &path, true).unwrap(), Cleaned::Absent);
        fs::write(&path, old_clients_json()).unwrap();
        assert_eq!(sanitize_file_in_place(StoreKind::Clients, &path, true).unwrap(), Cleaned::Rewritten);
        no_old_secrets(&fs::read_to_string(&path).unwrap());
        assert_eq!(sanitize_file_in_place(StoreKind::Clients, &path, true).unwrap(), Cleaned::AlreadyClean);
        fs::write(&path, b"{oops").unwrap();
        assert_eq!(sanitize_file_in_place(StoreKind::Clients, &path, true).unwrap(), Cleaned::Unparseable);
        assert_eq!(fs::read(&path).unwrap(), b"{oops", "неразбираемый файл не тронут");
        // очистка и выдача права одновременно: ни одно право не теряется
        let path = std::sync::Arc::new(dir.path().join("paired_clients.json"));
        fs::write(&*path, old_clients_json()).unwrap();
        let p2 = path.clone();
        let cleaner = std::thread::spawn(move || {
            for _ in 0..20 {
                let _ = sanitize_file_in_place(StoreKind::Clients, &p2, true);
            }
        });
        for i in 0..20u8 {
            let key = format!("{:02x}{}", i, "0".repeat(62));
            PairedClientStore::update(&path, |st| {
                st.issue(key.clone(), 3600);
                Ok(())
            })
            .unwrap();
        }
        cleaner.join().unwrap();
        match PairedClientStore::read(&path) {
            StoreRead::Current(st) => assert_eq!(st.clients.len(), 20),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn p17_4_a_normal_write_is_reported_durable() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.json");
        assert_eq!(crate::util::private_file::write_private_checked(&p, b"{}").unwrap(), crate::util::private_file::Committed::Durable);
    }


    /// Страж по исходникам: механизма возобновления и хранения рабочего ключа в коде больше нет.
    #[test]
    fn the_resume_mechanism_is_gone_from_the_sources() {
        let sources = [
            ("transport.rs", include_str!("transport.rs")),
            ("server.rs", include_str!("../web/server.rs")),
            ("node_backup.rs", include_str!("../node_backup.rs")),
            ("pairing.rs (до тестов)", include_str!("pairing.rs").split("#[cfg(test)]\nmod tests {").next().unwrap()),
        ];
        for (name, text) in sources {
            for banned in ["PKT_RESUME", "PKT_SESSION_ISSUE", "compute_resume_mac", "derive_resume_key", "new_with_session_key", "resume_secret()", "session_key()", "SessionToken", "handle_resume_packet", "handle_session_issue", "encode_resume", "decode_resume"] {
                assert!(!text.contains(banned), "{name}: осталось {banned}");
            }
        }
    }
}
