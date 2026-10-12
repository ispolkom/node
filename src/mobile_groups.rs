//! Группы на узле владельца (`/mobile/groups...` для телефонов, `/api/mobile/groups...` для страницы узла; договорённость — docs/CLIENT_WIRE.md).
//!
//! Узел — сервер группы, но содержимого он не читает: ключ группы создаёт телефон-владелец группы и раздаёт участникам сам, запечатывая
//! его для каждого их ключом группы (узел хранит эти «конверты» как непрозрачные байты). Записи журнала шифруют телефоны.
//!
//! Что видит узел: название и тему (их задаёт владелец на компьютере), тип группы, псевдонимы участников, роли, кто когда писал и
//! сколько байт, журнал модерации. Настоящих имён и номеров устройств в данных групп нет: участник записан как хэш от номера группы и
//! номера устройства (у каждой группы свой), а показывается случайным псевдонимом этой группы. Имя, которое человек называет при
//! вступлении, он пишет зашифрованной записью в журнал.
//!
//! Журнал — только дописывание: у каждой записи номер (seq) по порядку, телефоны забирают «всё после N» (как лента новостей), а узел
//! присылает подключённым участникам короткий намёк «есть новое» (кадр 0x15 в сокете, без содержимого). Модераторы могут стереть
//! запись (байты удаляются с диска, номер остаётся с пометкой), заглушить, выгнать или запретить участника; всё это пишется в журнал
//! модерации. Создаёт группу телефон (он владелец и модерирует); удалить её целиком можно только на компьютере (страница узла, по паролю).
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mobile_api::{device_peer_of, DeviceKey};

const MAX_GROUPS: usize = 64;
const MAX_MEMBERS: usize = 256;
const MAX_NAME: usize = 64;
const MAX_TOPIC: usize = 256;
/// Самая большая запись журнала (шифртекст), байт.
const MAX_ENTRY: usize = 64 * 1024 + 256;
/// Журнал группы на диске не больше этого; старые записи уходят (номера сохраняются, `first_seq` растёт).
const MAX_JOURNAL_BYTES: u64 = 32 << 20;
/// Сколько записей и байт отдаём за один запрос.
const PULL_MAX: usize = 500;
const PULL_MAX_BYTES: usize = 4 << 20;
const MAX_MODLOG: usize = 1000;
const MAX_EPOCHS: usize = 64;
const MAX_BOX: usize = 1024;
const MAX_INVITES: usize = 32;
const INVITE_TTL_MS: u64 = 24 * 3600 * 1000;
const MAX_INVITE_TTL_MS: u64 = 30 * 24 * 3600 * 1000;
const MAX_REASON: usize = 200;

/// Кадр-намёк в сокете телефона: `[0x15][16 номер группы][1 что][8 seq LE]`.
pub const FT_GROUP_HINT: u8 = 0x15;
pub const HINT_LOG: u8 = 1;
pub const HINT_KEYS: u8 = 2;
pub const HINT_CHANGED: u8 = 3;
pub const HINT_DELETED: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Open,
    /// «По приглашению». Старые группы с типом «закрытая» читаются как эта (отдельного закрытого типа больше нет).
    #[serde(alias = "closed")]
    Invite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    /// Админ: настройки группы, правила, роли «модератор/участник», действия модератора; владельца и других админов не трогает.
    Admin,
    Moderator,
    Member,
}

#[derive(Clone, Serialize, Deserialize)]
struct Member {
    /// sha256("yandi-group-member:" группа ":" номер устройства), hex — номер устройства здесь не хранится
    key: String,
    alias: String,
    role: Role,
    #[serde(default)]
    muted: bool,
    joined_ms: u64,
    /// открытый ключ участника для этой группы (x25519, base64): им владелец запечатывает ключ группы
    #[serde(default)]
    member_pub: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Banned {
    key: String,
    alias: String,
    ts: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Invite {
    code_hash: String,
    expires_ms: u64,
    uses_left: u32,
    by: String,
}

/// Просьба вступить в группу «по приглашению»: человек просит владельца прислать приглашение. Номер устройства лежит тут, чтобы
/// владелец мог ответить ему личным сообщением; запись живёт неделю и стирается, когда владелец ответил.
#[derive(Clone, Serialize, Deserialize)]
struct JoinRequest {
    id: String,
    peer: String,
    name: String,
    ts: u64,
}

/// Тематика новой группы (короткая метка для каталога) и описание.
const MAX_NEW_TOPIC: usize = 40;
const MAX_ABOUT: usize = 300;
const MAX_REQUESTS: usize = 20;
/// Сколько последних событий стирания хранится для догоняющих участников.
const MAX_DELETION_EVENTS: usize = 5000;
const REQUEST_TTL_MS: u64 = 7 * 24 * 3600 * 1000;

#[derive(Clone, Serialize, Deserialize)]
struct ModEntry {
    ts: u64,
    /// псевдоним модератора или «компьютер» для действий со страницы узла
    by: String,
    action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Group {
    id: String,
    name: String,
    #[serde(default)]
    topic: String,
    /// необязательное описание (до 300 знаков); пусто — в карточке не показывается
    #[serde(default)]
    about: String,
    kind: Kind,
    created_ms: u64,
    members: Vec<Member>,
    #[serde(default)]
    banned: Vec<Banned>,
    #[serde(default)]
    invites: Vec<Invite>,
    /// текущее поколение ключа группы (его поднимает владелец, например после изгнания участника)
    #[serde(default)]
    epoch: u32,
    /// поколение → ключ участника → запечатанный ключ группы (base64, узел его не читает)
    #[serde(default)]
    key_boxes: BTreeMap<u32, HashMap<String, String>>,
    /// поколение → чьим ключом владельца (member_pub) оно запечатано: участник, бывший не в сети во время смены владельца, по нему
    /// понимает, какой ключ нужен для каждого старого конверта
    #[serde(default)]
    epoch_owner: BTreeMap<u32, String>,
    /// журнал стираний: (номер события, номер записи). Участник, пропустивший намёк (был не в сети), догоняет стирания по курсору
    #[serde(default)]
    deletions: Vec<(u64, u64)>,
    #[serde(default)]
    next_deletion: u64,
    /// кто-то покинул группу после последней смены ключа: владельцу стоит сменить ключ
    #[serde(default)]
    rekey_needed: bool,
    /// номер первой записи, которая ещё лежит в журнале, и номер следующей
    #[serde(default = "one")]
    first_seq: u64,
    #[serde(default = "one")]
    next_seq: u64,
    #[serde(default)]
    modlog: Vec<ModEntry>,
    #[serde(default)]
    requests: Vec<JoinRequest>,
}

fn one() -> u64 {
    1
}

impl Group {
    fn member(&self, key: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.key == key)
    }
    fn member_by_alias(&self, alias: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.alias == alias)
    }
    fn owner(&self) -> Option<&Member> {
        self.members.iter().find(|m| m.role == Role::Owner)
    }
    fn keys(&self) -> Vec<String> {
        self.members.iter().map(|m| m.key.clone()).collect()
    }
    fn log(
        &mut self,
        by: &str,
        action: &str,
        target: Option<&str>,
        seq: Option<u64>,
        reason: Option<&str>,
    ) {
        let reason = reason
            .map(|r| clean(r, MAX_REASON))
            .filter(|r| !r.is_empty());
        self.modlog.push(ModEntry {
            ts: now_ms(),
            by: by.to_string(),
            action: action.to_string(),
            target: target.map(str::to_string),
            seq,
            reason,
        });
        if self.modlog.len() > MAX_MODLOG {
            let drop = self.modlog.len() - MAX_MODLOG;
            self.modlog.drain(..drop);
        }
    }
    fn new_alias(&self) -> String {
        use rand::seq::SliceRandom;
        use rand::Rng;
        let mut rng = rand::thread_rng();
        loop {
            let a = format!(
                "{} {}-{:02}",
                ADJ.choose(&mut rng).unwrap(),
                NOUN.choose(&mut rng).unwrap(),
                rng.gen_range(0..100)
            );
            if self.member_by_alias(&a).is_none() && !self.banned.iter().any(|b| b.alias == a) {
                return a;
            }
        }
    }
    /// Убрать участника (ушёл, выгнан, запрещён): его конверты ключей больше не нужны, ключ стоит сменить.
    fn drop_member(&mut self, key: &str) -> Option<Member> {
        let pos = self.members.iter().position(|m| m.key == key)?;
        let m = self.members.remove(pos);
        for boxes in self.key_boxes.values_mut() {
            boxes.remove(key);
        }
        self.rekey_needed = true;
        Some(m)
    }
}

const ADJ: &[&str] = &[
    "Тихий",
    "Быстрый",
    "Серый",
    "Рыжий",
    "Ясный",
    "Смелый",
    "Мудрый",
    "Северный",
    "Лесной",
    "Ночной",
    "Зоркий",
    "Добрый",
    "Ловкий",
    "Тёплый",
    "Дальний",
    "Звёздный",
    "Горный",
    "Речной",
    "Снежный",
    "Весёлый",
];
const NOUN: &[&str] = &[
    "Лис",
    "Ёж",
    "Филин",
    "Волк",
    "Кот",
    "Барсук",
    "Бобр",
    "Сокол",
    "Олень",
    "Рысь",
    "Ворон",
    "Медведь",
    "Заяц",
    "Дельфин",
    "Журавль",
    "Енот",
    "Тюлень",
    "Лось",
    "Стриж",
    "Кит",
];

#[derive(Serialize, Deserialize)]
struct JournalLine {
    seq: u64,
    ts: u64,
    from: String,
    epoch: u32,
    /// шифртекст записи, base64; пусто у стёртой
    p: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
}

/// Намёк «в группе что-то изменилось» для подключённых телефонов-участников.
#[derive(Clone)]
pub struct Hint {
    pub(crate) gid: String,
    pub(crate) kind: u8,
    pub(crate) seq: u64,
    pub(crate) to: Arc<Vec<String>>,
}

impl Hint {
    /// Кадр для этого устройства, если намёк ему адресован.
    pub fn frame_for(&self, device_peer: &str) -> Option<Vec<u8>> {
        let key = member_key(&self.gid, device_peer);
        if !self.to.iter().any(|k| *k == key) {
            return None;
        }
        let gid = hex::decode(&self.gid).ok()?;
        let mut f = Vec::with_capacity(26);
        f.push(FT_GROUP_HINT);
        f.extend_from_slice(&gid);
        f.push(self.kind);
        f.extend_from_slice(&self.seq.to_le_bytes());
        Some(f)
    }
}

fn hints() -> &'static tokio::sync::broadcast::Sender<Hint> {
    static H: OnceLock<tokio::sync::broadcast::Sender<Hint>> = OnceLock::new();
    H.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

pub fn subscribe() -> tokio::sync::broadcast::Receiver<Hint> {
    hints().subscribe()
}

fn hint(gid: &str, kind: u8, seq: u64, to: Vec<String>) {
    if !to.is_empty() {
        let _ = hints().send(Hint {
            gid: gid.to_string(),
            kind,
            seq,
            to: Arc::new(to),
        });
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Номер участника в группе для устройства (нужен клиенту групп на узле, чтобы узнавать свои намёки).
pub(crate) fn member_key_for(gid: &str, device_peer: &str) -> String {
    member_key(gid, device_peer)
}

fn member_key(gid: &str, device_peer: &str) -> String {
    hex::encode(Sha256::digest(
        format!("yandi-group-member:{gid}:{device_peer}").as_bytes(),
    ))
}

pub(crate) fn valid_gid(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn clean(s: &str, max: usize) -> String {
    s.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}

fn b64_ok(s: &str, max: usize) -> Option<Vec<u8>> {
    use base64::Engine;
    if s.len() > max * 4 / 3 + 4 {
        return None;
    }
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .ok()
        .filter(|b| !b.is_empty() && b.len() <= max)
}

/// Открытый ключ участника годится для обмена ключом группы: 32 байта и не «слабая» точка X25519 (нулевая или малого порядка — с ней общий
/// секрет всегда нулевой, и раздача ключа владельцем срывалась бы).
fn valid_member_pub(b64: &str) -> bool {
    let Some(b) = b64_ok(b64, 32) else { return false };
    let Ok(arr) = <[u8; 32]>::try_from(b.as_slice()) else { return false };
    let probe = x25519_dalek::StaticSecret::random_from_rng(rand::thread_rng());
    !probe.diffie_hellman(&x25519_dalek::PublicKey::from(arr)).as_bytes().iter().all(|x| *x == 0)
}

/// Автор записи — тот же, кто действует (своё можно стереть всегда)?
fn by_alias_is(by: &str, author: &str) -> bool {
    by == author
}

fn random_hex(n: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Ошибка операции: код ответа и текст для человека.
#[derive(Debug)]
pub struct Fail(pub StatusCode, pub &'static str);

type R<T> = Result<T, Fail>;

fn forbidden(msg: &'static str) -> Fail {
    Fail(StatusCode::FORBIDDEN, msg)
}

fn not_found() -> Fail {
    Fail(StatusCode::NOT_FOUND, "нет такой группы")
}

fn bad(msg: &'static str) -> Fail {
    Fail(StatusCode::BAD_REQUEST, msg)
}

/// Все группы узла: описание в `index.json`, журнал каждой — `<id>.log` (по записи JSON в строке) в папке `groups`.
pub struct Store {
    /// индекс не открылся (повреждён/не читается): изменения запрещены, чтобы не перезаписать его пустым
    broken: Option<String>,
    dir: PathBuf,
    groups: BTreeMap<String, Group>,
    /// смещения строк журнала (индекс i — запись first_seq + i), строятся при первом обращении
    offsets: HashMap<String, Vec<u64>>,
}

impl Store {
    pub fn open(dir: PathBuf) -> Self {
        let path = dir.join("index.json");
        // нет файла — пусто; файл есть, но не читается или повреждён — НЕ начинаем с пустого и ничего не перезаписываем
        let (groups, broken): (Vec<Group>, Option<String>) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(g) => (g, None),
                Err(e) => (Vec::new(), Some(format!("{} повреждён ({e}); файл не тронут", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
            Err(e) => (Vec::new(), Some(format!("{} не читается ({e}); файл не тронут", path.display()))),
        };
        if let Some(b) = &broken {
            eprintln!("[groups] ⚠️  хранилище групп: {b}. Изменения групп отключены до восстановления.");
        }
        Self {
            dir,
            groups: groups.into_iter().map(|g| (g.id.clone(), g)).collect(),
            offsets: HashMap::new(),
            broken,
        }
    }

    /// Сохранить описание групп. Ошибка записи (диск заполнен, нет прав) и повреждённый при открытии индекс — это ошибка операции, а не
    /// «успех только в памяти» (после перезапуска изменение пропало бы).
    fn save(&self) -> R<()> {
        if self.broken.is_some() {
            return Err(Fail(StatusCode::SERVICE_UNAVAILABLE, "хранилище групп повреждено; изменения отключены до восстановления"));
        }
        let list: Vec<&Group> = self.groups.values().collect();
        let s = serde_json::to_vec_pretty(&list).map_err(|_| Fail(StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить группы"))?;
        crate::util::private_file::write_private(&self.dir.join("index.json"), &s)
            .map_err(|_| Fail(StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить группы (диск?)"))
    }

    fn log_path(&self, gid: &str) -> PathBuf {
        self.dir.join(format!("{gid}.log"))
    }

    fn get(&self, gid: &str) -> R<&Group> {
        self.groups.get(gid).ok_or_else(not_found)
    }

    fn get_mut(&mut self, gid: &str) -> R<&mut Group> {
        self.groups.get_mut(gid).ok_or_else(not_found)
    }

    /// Участник группы по устройству.
    fn me<'a>(&'a self, gid: &str, device: &str) -> R<(&'a Group, &'a Member)> {
        let g = self.get(gid)?;
        let m = g
            .member(&member_key(gid, device))
            .ok_or_else(|| forbidden("вы не участник этой группы"))?;
        Ok((g, m))
    }

    fn offsets(&mut self, gid: &str) -> &mut Vec<u64> {
        if !self.offsets.contains_key(gid) {
            let mut v = Vec::new();
            let path = self.log_path(gid);
            if let Ok(f) = std::fs::File::open(&path) {
                let mut pos = 0u64;
                let mut r = BufReader::new(f);
                let mut line = Vec::new();
                while let Ok(n) = r.read_until(b'\n', &mut line) {
                    if n == 0 {
                        break;
                    }
                    if !line.ends_with(b"\n") {
                        // недописанная строка (узел упал посреди записи): отрезаем, иначе следующая запись склеится с ней
                        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
                            let _ = f.set_len(pos);
                        }
                        break;
                    }
                    v.push(pos);
                    pos += n as u64;
                    line.clear();
                }
            }
            // номер следующей записи берём из журнала: описание могло не успеть сохраниться после дописывания
            if let Some(g) = self.groups.get_mut(gid) {
                g.next_seq = g.first_seq + v.len() as u64;
            }
            self.offsets.insert(gid.to_string(), v);
        }
        self.offsets.get_mut(gid).unwrap()
    }

    fn read_lines(&mut self, gid: &str) -> Vec<JournalLine> {
        let Ok(s) = std::fs::read_to_string(self.log_path(gid)) else {
            return Vec::new();
        };
        s.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn rewrite(&mut self, gid: &str, lines: &[JournalLine]) -> std::io::Result<()> {
        let mut out = Vec::new();
        for l in lines {
            out.extend(serde_json::to_vec(l).unwrap_or_default());
            out.push(b'\n');
        }
        crate::util::private_file::write_private(&self.log_path(gid), &out)?;
        self.offsets.remove(gid);
        Ok(())
    }

    // ── владелец узла (компьютер) ──

    pub fn create(
        &mut self,
        name: &str,
        topic: &str,
        kind: Kind,
        owner_device: &str,
        members: &[String],
    ) -> R<String> {
        self.create_as(name, topic, kind, owner_device, members, false)
    }

    /// Сколько групп создал и ведёт это устройство.
    pub fn owned_count(&self, device: &str) -> usize {
        self.groups
            .values()
            .filter(|g| g.owner().is_some_and(|o| o.key == member_key(&g.id, device)))
            .count()
    }

    /// `by_phone`: группу создал телефон-владелец (в журнале модерации это записано его псевдонимом, а не страницей узла).
    pub fn create_as(
        &mut self,
        name: &str,
        topic: &str,
        kind: Kind,
        owner_device: &str,
        members: &[String],
        by_phone: bool,
    ) -> R<String> {
        let name = clean(name, MAX_NAME);
        if name.is_empty() {
            return Err(bad("нужно название группы"));
        }
        if by_phone && clean(topic, MAX_NEW_TOPIC).is_empty() {
            return Err(bad("нужна тематика группы"));
        }
        if self.groups.len() >= MAX_GROUPS {
            return Err(Fail(
                StatusCode::INSUFFICIENT_STORAGE,
                "слишком много групп",
            ));
        }
        let id = random_hex(16);
        let mut g = Group {
            id: id.clone(),
            name,
            topic: clean(topic, if by_phone { MAX_NEW_TOPIC } else { MAX_TOPIC }),
            about: String::new(),
            kind,
            created_ms: now_ms(),
            members: Vec::new(),
            banned: Vec::new(),
            invites: Vec::new(),
            epoch: 0,
            key_boxes: BTreeMap::new(),
            epoch_owner: BTreeMap::new(),
            deletions: Vec::new(),
            next_deletion: 0,
            rekey_needed: false,
            first_seq: 1,
            next_seq: 1,
            modlog: Vec::new(),
            requests: Vec::new(),
        };
        let owner = Member {
            key: member_key(&id, owner_device),
            alias: g.new_alias(),
            role: Role::Owner,
            muted: false,
            joined_ms: now_ms(),
            member_pub: None,
        };
        g.members.push(owner);
        for d in members {
            let key = member_key(&id, d);
            if g.member(&key).is_none() {
                let alias = g.new_alias();
                g.members.push(Member {
                    key,
                    alias,
                    role: Role::Member,
                    muted: false,
                    joined_ms: now_ms(),
                    member_pub: None,
                });
            }
        }
        g.members.truncate(MAX_MEMBERS);
        let actor = if by_phone { g.members[0].alias.clone() } else { PC.to_string() };
        g.log(&actor, "create", None, None, None);
        let to = g.keys();
        self.groups.insert(id.clone(), g);
        self.save()?;
        hint(&id, HINT_CHANGED, 0, to);
        Ok(id)
    }


    /// Настройки группы меняют владелец и админ: название, тематика, описание. Название и тематика не бывают пустыми.
    pub fn update_settings(&mut self, gid: &str, device: &str, name: Option<&str>, topic: Option<&str>, about: Option<&str>) -> R<()> {
        let (_, me) = self.me(gid, device)?;
        if me.role != Role::Owner && me.role != Role::Admin {
            return Err(forbidden("настройки группы меняют владелец и админ"));
        }
        let by = me.alias.clone();
        let g = self.get_mut(gid)?;
        if let Some(n) = name {
            let n = clean(n, MAX_NAME);
            if n.is_empty() {
                return Err(bad("нужно название группы"));
            }
            g.name = n;
        }
        if let Some(t) = topic {
            let t = clean(t, MAX_NEW_TOPIC);
            if t.is_empty() {
                return Err(bad("нужна тематика группы"));
            }
            g.topic = t;
        }
        if let Some(a) = about {
            g.about = clean(a, MAX_ABOUT);
        }
        g.log(&by, "settings", None, None, None);
        let to = g.keys();
        self.save()?;
        hint(gid, HINT_CHANGED, 0, to);
        Ok(())
    }



    /// Владелец передаёт группу участнику (по псевдониму). Прежний владелец становится админом, новый должен создать новое поколение ключа
    /// (`rekey_needed`). Участнику нужен ключ группы (`member_pub`), иначе он не сможет раздавать ключи.
    pub fn transfer_owner(&mut self, gid: &str, device: &str, to_alias: &str) -> R<()> {
        let (_, me) = self.me(gid, device)?;
        if me.role != Role::Owner {
            return Err(forbidden("передать группу может только её владелец"));
        }
        let me_key = me.key.clone();
        let me_alias = me.alias.clone();
        let g = self.get_mut(gid)?;
        let target = g.member_by_alias(to_alias).cloned().ok_or_else(|| Fail(StatusCode::NOT_FOUND, "нет такого участника"))?;
        if target.key == me_key {
            return Err(bad("группа уже ваша"));
        }
        if target.member_pub.is_none() {
            return Err(Fail(StatusCode::CONFLICT, "у участника ещё нет ключа группы (он должен открыть приложение)"));
        }
        for m in g.members.iter_mut() {
            if m.key == me_key {
                m.role = Role::Admin;
            }
            if m.key == target.key {
                m.role = Role::Owner;
                m.muted = false;
            }
        }
        g.rekey_needed = true;
        g.log(&me_alias, "transfer", Some(&target.alias), None, None);
        let to = g.keys();
        self.save()?;
        hint(gid, HINT_CHANGED, 0, to.clone());
        hint(gid, HINT_KEYS, 0, to);
        Ok(())
    }

    pub fn delete(&mut self, gid: &str) -> R<()> {
        let g = self.groups.remove(gid).ok_or_else(not_found)?;
        self.offsets.remove(gid);
        let _ = std::fs::remove_file(self.log_path(gid));
        crate::mobile_group_files::drop_group(gid);
        self.save()?;
        hint(gid, HINT_DELETED, 0, g.keys());
        Ok(())
    }

    // ── участники (телефоны) ──

    pub fn join(
        &mut self,
        gid: &str,
        device: &str,
        code: Option<&str>,
        member_pub: Option<&str>,
    ) -> R<String> {
        let member_pub = match member_pub {
            Some(p) if !p.is_empty() => {
                if !valid_member_pub(p) {
                    return Err(bad("неверный ключ"));
                }
                Some(p.to_string())
            }
            _ => None,
        };
        let g = self.get_mut(gid)?;
        let key = member_key(gid, device);
        if g.member(&key).is_some() {
            return Err(Fail(StatusCode::CONFLICT, "уже участник"));
        }
        if g.banned.iter().any(|b| b.key == key) {
            return Err(forbidden("вход в группу запрещён"));
        }
        if g.members.len() >= MAX_MEMBERS {
            return Err(Fail(StatusCode::INSUFFICIENT_STORAGE, "группа заполнена"));
        }
        let mut via = None;
        match g.kind {
            Kind::Open => {}
            Kind::Invite => {
                let h = hex::encode(Sha256::digest(
                    code.unwrap_or("").trim().to_uppercase().as_bytes(),
                ));
                let now = now_ms();
                g.invites.retain(|i| i.expires_ms > now && i.uses_left > 0);
                let Some(inv) = g.invites.iter_mut().find(|i| i.code_hash == h) else {
                    return Err(forbidden("неверное или просроченное приглашение"));
                };
                inv.uses_left -= 1;
                via = Some(inv.by.clone());
                g.invites.retain(|i| i.uses_left > 0);
            }
        }
        let alias = g.new_alias();
        g.requests.retain(|r| r.peer != device);   // вступил — просьба о приглашении больше не нужна
        g.members.push(Member {
            key,
            alias: alias.clone(),
            role: Role::Member,
            muted: false,
            joined_ms: now_ms(),
            member_pub,
        });
        g.log(
            via.as_deref().unwrap_or(&alias),
            if via.is_some() { "join_invite" } else { "join" },
            Some(&alias),
            None,
            None,
        );
        let to = g.keys();
        self.save()?;
        hint(gid, HINT_CHANGED, 0, to);
        Ok(alias)
    }

    pub fn leave(&mut self, gid: &str, device: &str) -> R<()> {
        let (_, m) = self.me(gid, device)?;
        if m.role == Role::Owner {
            return Err(forbidden(
                "владелец не может выйти: передайте группу на компьютере",
            ));
        }
        let key = m.key.clone();
        let g = self.get_mut(gid)?;
        let m = g.drop_member(&key).unwrap();
        g.log(&m.alias, "leave", Some(&m.alias), None, None);
        let mut to = g.keys();
        to.push(key);
        self.save()?;
        hint(gid, HINT_CHANGED, 0, to);
        Ok(())
    }

    pub fn set_pub(&mut self, gid: &str, device: &str, member_pub: &str) -> R<()> {
        if !valid_member_pub(member_pub) {
            return Err(bad("неверный ключ"));
        }
        let key = self.me(gid, device)?.1.key.clone();
        let g = self.get_mut(gid)?;
        if let Some(m) = g.members.iter_mut().find(|m| m.key == key) {
            m.member_pub = Some(member_pub.to_string());
        }
        // ключ участника сменился: прежние конверты ему не открыть
        for boxes in g.key_boxes.values_mut() {
            boxes.remove(&key);
        }
        let owner = g.owner().map(|o| vec![o.key.clone()]).unwrap_or_default();
        self.save()?;
        hint(gid, HINT_KEYS, 0, owner);
        Ok(())
    }

    /// Владелец кладёт запечатанный ключ группы для участников (по псевдонимам). Поколение больше текущего — смена ключа.
    pub fn put_keys(
        &mut self,
        gid: &str,
        device: &str,
        epoch: u32,
        boxes: &HashMap<String, String>,
    ) -> R<usize> {
        if self.me(gid, device)?.1.role != Role::Owner {
            return Err(forbidden("ключи раздаёт только владелец группы"));
        }
        let g = self.get_mut(gid)?;
        if epoch < g.epoch {
            return Err(Fail(StatusCode::CONFLICT, "поколение ключа устарело"));
        }
        let mut put = Vec::new();
        for (alias, b) in boxes {
            let Some(m) = g.member_by_alias(alias) else {
                continue;
            };
            if b64_ok(b, MAX_BOX).is_none() {
                return Err(bad("неверный конверт ключа"));
            }
            put.push((m.key.clone(), b.clone()));
        }
        if epoch > g.epoch {
            g.epoch = epoch;
            g.rekey_needed = false;
        }
        if let Some(p) = g.owner().and_then(|o| o.member_pub.clone()) {
            g.epoch_owner.insert(epoch, p);
        }
        let slot = g.key_boxes.entry(epoch).or_default();
        let n = put.len();
        let to: Vec<String> = put.iter().map(|(k, _)| k.clone()).collect();
        for (k, b) in put {
            slot.insert(k, b);
        }
        while g.key_boxes.len() > MAX_EPOCHS {
            let first = *g.key_boxes.keys().next().unwrap();
            g.key_boxes.remove(&first);
        }
        self.save()?;
        hint(gid, HINT_KEYS, epoch as u64, to);
        Ok(n)
    }

    pub fn my_keys(&self, gid: &str, device: &str) -> R<Value> {
        let (g, m) = self.me(gid, device)?;
        let boxes: Vec<Value> = g
            .key_boxes
            .iter()
            .filter_map(|(e, b)| b.get(&m.key).map(|x| json!({"epoch": e, "box": x, "owner_pub": g.epoch_owner.get(e)})))
            .collect();
        let owner_pub = g.owner().and_then(|o| o.member_pub.clone());
        // first_epoch — самое старое поколение, конверты которого узел ещё хранит: более старые вытеснены навсегда (MAX_EPOCHS)
        let first_epoch = g.key_boxes.keys().min().copied().unwrap_or(g.epoch);
        Ok(json!({"epoch": g.epoch, "owner_pub": owner_pub, "boxes": boxes, "first_epoch": first_epoch}))
    }

    pub fn append(&mut self, gid: &str, device: &str, epoch: u32, payload_b64: &str) -> R<u64> {
        if b64_ok(payload_b64, MAX_ENTRY).is_none() {
            return Err(Fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "запись пустая или слишком большая",
            ));
        }
        self.me(gid, device)?;
        let _ = self.offsets(gid); // индекс (и верный номер следующей записи) — до дописывания
        let (g, m) = self.me(gid, device)?;
        if m.muted {
            return Err(forbidden("вам запрещено писать в эту группу"));
        }
        if epoch > g.epoch {
            return Err(bad("такого поколения ключа ещё нет"));
        }
        let alias = m.alias.clone();
        let seq = g.next_seq;
        let line = JournalLine {
            seq,
            ts: now_ms(),
            from: alias,
            epoch,
            p: payload_b64.to_string(),
            deleted: false,
        };
        let mut bytes = serde_json::to_vec(&line).unwrap_or_default();
        bytes.push(b'\n');
        let path = self.log_path(gid);
        let pos = (|| -> std::io::Result<u64> {
            std::fs::create_dir_all(&self.dir)?;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
            }
            let pos = f.seek(SeekFrom::End(0))?;
            f.write_all(&bytes)?;
            Ok(pos)
        })()
        .map_err(|_| Fail(StatusCode::INTERNAL_SERVER_ERROR, "не удалось записать"))?;
        self.offsets(gid).push(pos);
        let g = self.get_mut(gid)?;
        g.next_seq = seq + 1;
        let to = g.keys();
        if pos + bytes.len() as u64 > MAX_JOURNAL_BYTES {
            self.trim(gid);
        }
        self.save()?;
        hint(gid, HINT_LOG, seq, to);
        Ok(seq)
    }

    /// Журнал вырос сверх предела: оставляем последние записи на три четверти предела.
    fn trim(&mut self, gid: &str) {
        let lines = self.read_lines(gid);
        let mut keep = 0usize;
        let mut size = 0u64;
        for l in lines.iter().rev() {
            size += serde_json::to_vec(l)
                .map(|v| v.len() as u64 + 1)
                .unwrap_or(0);
            if size > MAX_JOURNAL_BYTES * 3 / 4 {
                break;
            }
            keep += 1;
        }
        let rest = &lines[lines.len() - keep..];
        if self.rewrite(gid, rest).is_ok() {
            if let Ok(g) = self.get_mut(gid) {
                g.first_seq = rest.first().map(|l| l.seq).unwrap_or(g.next_seq);
            }
        }
    }

    pub fn pull(&mut self, gid: &str, device: &str, after: u64, limit: usize) -> R<Value> {
        self.me(gid, device)?;
        let _ = self.offsets(gid);
        let (g, _) = self.me(gid, device)?;
        let (first, next) = (g.first_seq, g.next_seq);
        let start = after.saturating_add(1).max(first);
        let mut out = Vec::new();
        if start < next {
            let idx = (start - first) as usize;
            let offs = self.offsets(gid);
            if let Some(&pos) = offs.get(idx) {
                if let Ok(mut f) = std::fs::File::open(self.log_path(gid)) {
                    if f.seek(SeekFrom::Start(pos)).is_ok() {
                        let mut bytes = 0usize;
                        for l in BufReader::new(f).lines() {
                            let Ok(l) = l else { break };
                            if out.len() >= limit.clamp(1, PULL_MAX)
                                || (bytes > 0 && bytes + l.len() > PULL_MAX_BYTES)
                            {
                                break;
                            }
                            bytes += l.len();
                            if let Ok(e) = serde_json::from_str::<JournalLine>(&l) {
                                out.push(json!({"seq": e.seq, "ts": e.ts, "from": e.from, "epoch": e.epoch, "payload_b64": e.p, "deleted": e.deleted}));
                            }
                        }
                    }
                }
            }
        }
        Ok(json!({"first_seq": first, "last_seq": next - 1, "entries": out}))
    }

    /// Действие модератора. Модератор — владелец группы или назначенный им; модератор не трогает других модераторов и владельца.
    pub fn moderate(
        &mut self,
        gid: &str,
        device: &str,
        action: &str,
        alias: Option<&str>,
        seq: Option<u64>,
        reason: Option<&str>,
    ) -> R<()> {
        let (_, me) = self.me(gid, device)?;
        let me = me.clone();
        if me.role == Role::Member {
            return Err(forbidden("нужны права модератора"));
        }
        self.apply_mod(gid, &me.alias, me.role, action, alias, seq, reason)
    }


    #[allow(clippy::too_many_arguments)]
    fn apply_mod(
        &mut self,
        gid: &str,
        by: &str,
        by_role: Role,
        action: &str,
        alias: Option<&str>,
        seq: Option<u64>,
        reason: Option<&str>,
    ) -> R<()> {
        if action == "delete_entry" {
            let seq = seq.ok_or_else(|| bad("нужен номер записи"))?;
            let mut lines = self.read_lines(gid);
            let Some(l) = lines.iter_mut().find(|l| l.seq == seq) else {
                return Err(Fail(StatusCode::NOT_FOUND, "нет такой записи"));
            };
            if l.deleted {
                return Ok(());
            }
            // стирать чужую запись можно по той же матрице прав, что и трогать автора: модератор — записи рядовых, админ — рядовых и
            // модераторов, владелец — любые (запись ушедшего участника — любой модератор)
            let author_role = self.get(gid)?.member_by_alias(&l.from).map(|m| m.role);
            let allowed = by_alias_is(by, &l.from)
                || match author_role {
                    None | Some(Role::Member) => true,
                    Some(Role::Moderator) => by_role == Role::Owner || by_role == Role::Admin,
                    Some(Role::Admin) => by_role == Role::Owner,
                    Some(Role::Owner) => false,
                };
            if !allowed {
                return Err(forbidden("эту запись может стереть только тот, кто выше автора по роли"));
            }
            l.p.clear();
            l.deleted = true;
            let author = l.from.clone();
            self.rewrite(gid, &lines)
                .map_err(|_| Fail(StatusCode::INTERNAL_SERVER_ERROR, "не удалось стереть"))?;
            let g = self.get_mut(gid)?;
            g.next_deletion += 1;
            let n = g.next_deletion;
            g.deletions.push((n, seq));
            if g.deletions.len() > MAX_DELETION_EVENTS {
                let drop = g.deletions.len() - MAX_DELETION_EVENTS;
                g.deletions.drain(..drop);
            }
            g.log(by, "delete_entry", Some(&author), Some(seq), reason);
            let to = g.keys();
            self.save()?;
            hint(gid, HINT_LOG, seq, to);
            return Ok(());
        }
        if action == "unban" {
            let alias = alias.ok_or_else(|| bad("нужен псевдоним"))?;
            let g = self.get_mut(gid)?;
            let before = g.banned.len();
            g.banned.retain(|b| b.alias != alias);
            if g.banned.len() == before {
                return Err(Fail(StatusCode::NOT_FOUND, "нет такого запрета"));
            }
            g.log(by, "unban", Some(alias), None, reason);
            self.save()?;
            return Ok(());
        }
        let alias = alias.ok_or_else(|| bad("нужен псевдоним"))?;
        let g = self.get_mut(gid)?;
        let target = g
            .member_by_alias(alias)
            .cloned()
            .ok_or_else(|| Fail(StatusCode::NOT_FOUND, "нет такого участника"))?;
        if target.role == Role::Owner {
            return Err(forbidden("владельца группы нельзя"));
        }
        if target.role == Role::Admin && by_role != Role::Owner {
            return Err(forbidden("админа может трогать только владелец группы"));
        }
        if target.role == Role::Moderator && by_role != Role::Owner && by_role != Role::Admin {
            return Err(forbidden("модератора могут трогать владелец и админ"));
        }
        let mut notify = g.keys();
        match action {
            "mute" | "unmute" => {
                if let Some(m) = g.members.iter_mut().find(|m| m.key == target.key) {
                    m.muted = action == "mute";
                }
            }
            "make_admin" => {
                if by_role != Role::Owner {
                    return Err(forbidden("админов назначает только владелец группы"));
                }
                if let Some(m) = g.members.iter_mut().find(|m| m.key == target.key) {
                    m.role = Role::Admin;
                }
            }
            "promote" | "demote" => {
                if by_role != Role::Owner && by_role != Role::Admin {
                    return Err(forbidden("роли назначают владелец и админ"));
                }
                if let Some(m) = g.members.iter_mut().find(|m| m.key == target.key) {
                    m.role = if action == "promote" {
                        Role::Moderator
                    } else {
                        Role::Member
                    };
                }
            }
            "kick" => {
                g.drop_member(&target.key);
            }
            "ban" => {
                g.drop_member(&target.key);
                g.banned.push(Banned {
                    key: target.key.clone(),
                    alias: target.alias.clone(),
                    ts: now_ms(),
                });
            }
            _ => return Err(bad("неизвестное действие")),
        }
        g.log(by, action, Some(alias), None, reason);
        if !notify.contains(&target.key) {
            notify.push(target.key);
        }
        self.save()?;
        hint(gid, HINT_CHANGED, 0, notify);
        Ok(())
    }

    /// Код приглашения (для групп «по приглашению»): выдают владелец и модераторы. Узел хранит только хэш кода.
    pub fn invite(&mut self, gid: &str, device: &str, uses: u32, ttl_ms: Option<u64>) -> R<Value> {
        let (g, me) = self.me(gid, device)?;
        if g.kind == Kind::Open {
            return Err(forbidden("в открытую группу вступают без приглашения"));
        }
        if me.role == Role::Member {
            return Err(forbidden("приглашает владелец группы или модератор"));
        }
        let by = me.alias.clone();
        self.make_invite(gid, &by, uses, ttl_ms)
    }


    fn make_invite(&mut self, gid: &str, by: &str, uses: u32, ttl_ms: Option<u64>) -> R<Value> {
        let code = invite_code();
        let expires_ms = now_ms()
            + ttl_ms
                .unwrap_or(INVITE_TTL_MS)
                .clamp(60_000, MAX_INVITE_TTL_MS);
        let g = self.get_mut(gid)?;
        let now = now_ms();
        g.invites.retain(|i| i.expires_ms > now && i.uses_left > 0);
        if g.invites.len() >= MAX_INVITES {
            g.invites.remove(0);
        }
        g.invites.push(Invite {
            code_hash: hex::encode(Sha256::digest(code.as_bytes())),
            expires_ms,
            uses_left: uses.clamp(1, 50),
            by: by.to_string(),
        });
        g.log(by, "invite", None, None, None);
        self.save()?;
        Ok(json!({"code": code, "group_id": gid, "expires_ms": expires_ms}))
    }

    // ── что показывать ──

    pub fn list_for(&self, device: &str) -> Vec<Value> {
        self.groups
            .values()
            .filter_map(|g| {
                let m = g.member(&member_key(&g.id, device))?;
                let owner = m.role == Role::Owner;
                let pending: Vec<&str> = if owner {
                    g.members.iter().filter(|x| x.member_pub.is_some() && !g.key_boxes.get(&g.epoch).is_some_and(|b| b.contains_key(&x.key))).map(|x| x.alias.as_str()).collect()
                } else {
                    Vec::new()
                };
                Some(json!({
                    "id": g.id, "name": g.name, "topic": g.topic, "about": g.about, "kind": g.kind, "my_alias": m.alias, "role": m.role, "muted": m.muted,
                    "members": g.members.len(), "epoch": g.epoch, "first_seq": g.first_seq, "last_seq": g.next_seq - 1,
                    "rekey_needed": owner && g.rekey_needed, "pending_keys": pending,
                    "requests": if m.role != Role::Member { g.requests.len() } else { 0 },
                }))
            })
            .collect()
    }

    pub fn set_about(&mut self, gid: &str, about: &str) {
        if let Some(g) = self.groups.get_mut(gid) {
            g.about = clean(about, MAX_ABOUT);
            if let Err(Fail(_, e)) = self.save() { eprintln!("[groups] ⚠️  {e}"); }
        }
    }

    /// Группы этого узла, куда устройство ещё не вступило: открытые (вступить сразу) и «по приглашению» (можно попросить приглашение).
    pub fn open_for(&self, device: &str) -> Vec<Value> {
        self.groups
            .values()
            .filter(|g| {
                let k = member_key(&g.id, device);
                g.member(&k).is_none() && !g.banned.iter().any(|b| b.key == k)
            })
            .map(|g| json!({"id": g.id, "name": g.name, "topic": g.topic, "about": g.about, "kind": g.kind, "members": g.members.len(), "requested": g.requests.iter().any(|r| r.peer == device)}))
            .collect()
    }

    /// Просьба о приглашении: владелец и модераторы получат намёк и увидят её в списке запросов.
    pub fn request_join(&mut self, gid: &str, device: &str, name: &str) -> R<()> {
        let g = self.get_mut(gid)?;
        if g.kind != Kind::Invite {
            return Err(bad("запросить приглашение можно только в группу «по приглашению»"));
        }
        let k = member_key(gid, device);
        if g.member(&k).is_some() {
            return Err(Fail(StatusCode::CONFLICT, "уже участник"));
        }
        if g.banned.iter().any(|b| b.key == k) {
            return Err(forbidden("вход в группу запрещён"));
        }
        let now = now_ms();
        g.requests.retain(|r| now.saturating_sub(r.ts) < REQUEST_TTL_MS);
        if g.requests.iter().any(|r| r.peer == device) {
            return Err(Fail(StatusCode::CONFLICT, "просьба уже отправлена"));
        }
        if g.requests.len() >= MAX_REQUESTS {
            return Err(Fail(StatusCode::TOO_MANY_REQUESTS, "у группы слишком много неразобранных просьб"));
        }
        g.requests.push(JoinRequest { id: random_hex(8), peer: device.to_string(), name: clean(name, 64), ts: now });
        let to: Vec<String> = g.members.iter().filter(|m| m.role != Role::Member).map(|m| m.key.clone()).collect();
        self.save()?;
        hint(gid, HINT_CHANGED, 0, to);
        Ok(())
    }

    /// Просьбы старше недели стираются на диске вместе с номером устройства заявителя (а не только скрываются в ответах).
    pub fn expire_requests(&mut self) -> R<()> {
        if self.broken.is_some() {
            // индекс не прочитан: уборка не выполнена (и повреждённый файл не перезаписываем), об этом надо сказать, а не молчать
            return Err(Fail(StatusCode::SERVICE_UNAVAILABLE, "хранилище групп повреждено: просроченные просьбы не стёрты"));
        }
        let now = now_ms();
        let mut changed = false;
        for g in self.groups.values_mut() {
            let before = g.requests.len();
            g.requests.retain(|r| now.saturating_sub(r.ts) < REQUEST_TTL_MS);
            changed |= g.requests.len() != before;
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    /// Неразобранные просьбы: видят владелец и модераторы.
    pub fn requests_of(&mut self, gid: &str, device: &str) -> R<Value> {
        let role = self.me(gid, device)?.1.role;
        if role == Role::Member {
            return Err(forbidden("просьбы видят владелец и модераторы"));
        }
        // просроченные стираются на диске и здесь (иначе чтение «съедало» их только в памяти, и таймер потом не видел изменений)
        self.expire_requests().ok();
        let g = self.get_mut(gid)?;
        let list: Vec<Value> = g.requests.iter().map(|r| json!({"id": r.id, "peer_id": r.peer, "name": r.name, "ts": r.ts})).collect();
        Ok(json!({"requests": list}))
    }

    /// Просьба разобрана (приглашение отправлено или отказано): запись и номер устройства стираются.
    pub fn resolve_request(&mut self, gid: &str, device: &str, rid: &str) -> R<()> {
        let role = self.me(gid, device)?.1.role;
        if role == Role::Member {
            return Err(forbidden("просьбы разбирают владелец и модераторы"));
        }
        let g = self.get_mut(gid)?;
        let before = g.requests.len();
        g.requests.retain(|r| r.id != rid);
        if g.requests.len() == before {
            return Err(not_found());
        }
        self.save()?;
        Ok(())
    }

    pub fn members(&self, gid: &str, device: &str) -> R<Value> {
        let (g, me) = self.me(gid, device)?;
        let cur = g.key_boxes.get(&g.epoch);
        let list: Vec<Value> = g
            .members
            .iter()
            .map(|m| json!({"alias": m.alias, "role": m.role, "muted": m.muted, "joined_ms": m.joined_ms, "member_pub": m.member_pub, "has_key": cur.is_some_and(|b| b.contains_key(&m.key))}))
            .collect();
        let banned: Vec<&str> = if me.role != Role::Member {
            g.banned.iter().map(|b| b.alias.as_str()).collect()
        } else {
            Vec::new()
        };
        Ok(json!({"members": list, "banned": banned}))
    }

    /// Стирания после курсора: `{deleted:[номера записей], cursor}`. Если курсор старее хранимого журнала стираний — `resync: true`
    /// (участнику стоит перечитать свои записи).
    pub fn deletions_after(&self, gid: &str, device: &str, after: u64) -> R<Value> {
        let (g, _) = self.me(gid, device)?;
        let oldest = g.deletions.first().map(|(n, _)| *n).unwrap_or(g.next_deletion + 1);
        let list: Vec<u64> = g.deletions.iter().filter(|(n, _)| *n > after).map(|(_, s)| *s).collect();
        Ok(json!({"deleted": list, "cursor": g.next_deletion, "resync": after + 1 < oldest && after < g.next_deletion}))
    }

    pub fn modlog(&self, gid: &str, device: Option<&str>) -> R<Value> {
        let g = match device {
            Some(d) => self.me(gid, d)?.0,
            None => self.get(gid)?,
        };
        Ok(json!({"modlog": g.modlog}))
    }

    /// Для страницы узла: группы, участники по псевдонимам и какие устройства можно добавить.
    pub fn overview(&self, devices: &[(String, String)]) -> Vec<Value> {
        self.groups
            .values()
            .map(|g| {
                let addable: Vec<Value> = devices
                    .iter()
                    .filter(|(peer, _)| g.member(&member_key(&g.id, peer)).is_none())
                    .map(|(peer, name)| json!({"id": &peer[..16], "name": name}))
                    .collect();
                let bytes = std::fs::metadata(self.log_path(&g.id)).map(|m| m.len()).unwrap_or(0);
                json!({
                    "id": g.id, "name": g.name, "topic": g.topic, "about": g.about, "kind": g.kind, "created_ms": g.created_ms, "epoch": g.epoch,
                    "entries": g.next_seq - g.first_seq, "last_seq": g.next_seq - 1, "journal_bytes": bytes, "rekey_needed": g.rekey_needed,
                    "members": g.members.iter().map(|m| json!({"alias": m.alias, "role": m.role, "muted": m.muted, "joined_ms": m.joined_ms})).collect::<Vec<_>>(),
                    "banned": g.banned.iter().map(|b| b.alias.clone()).collect::<Vec<_>>(),
                    "addable": addable,
                })
            })
            .collect()
    }

    /// Устройство забыто узлом: убираем его из всех групп (хэш у каждой группы свой, поэтому пересчитываем).
    pub fn forget_device(&mut self, device: &str) {
        let mut changed = Vec::new();
        let mut had_request = false;
        for g in self.groups.values_mut() {
            let n = g.requests.len();
            g.requests.retain(|r| r.peer != device);
            had_request |= g.requests.len() != n;
            let k = member_key(&g.id, device);
            if let Some(m) = g.drop_member(&k) {
                g.log(PC, "removed_device", Some(&m.alias), None, None);
                changed.push((g.id.clone(), g.keys()));
            }
        }
        if had_request && changed.is_empty() {
            if let Err(Fail(_, e)) = self.save() { eprintln!("[groups] ⚠️  {e}"); }
        }
        if !changed.is_empty() {
            if let Err(Fail(_, e)) = self.save() { eprintln!("[groups] ⚠️  {e}"); }
            for (gid, to) in changed {
                hint(&gid, HINT_CHANGED, 0, to);
            }
        }
    }
}

const PC: &str = "компьютер";

/// Код приглашения `XXXX-XXXX-XXXX`: 12 знаков без похожих букв, 60 бит.
fn invite_code() -> String {
    use rand::Rng;
    const A: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..14)
        .map(|i| {
            if i == 4 || i == 9 {
                '-'
            } else {
                A[rng.gen_range(0..A.len())] as char
            }
        })
        .collect::<String>()
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Участник группы для хранилища файлов групп: (его номер в группе, роль, заглушён ли).
pub(crate) fn member_state(gid: &str, device: &str) -> Option<(String, Role, bool)> {
    let key = member_key(gid, device);
    with_store(|s| s.get(gid).ok().and_then(|g| g.member(&key).map(|m| (key.clone(), m.role, m.muted))))
}

/// Уборка по таймеру (из main): просроченные просьбы о вступлении.
pub fn cleanup_requests() {
    if let Err(e) = with_store(|s| s.expire_requests()) {
        eprintln!("[groups] просроченные просьбы не стёрты: {e:?}");
    }
}

pub(crate) fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| Store::open(crate::util::data_dir::data_dir().join("groups")));
    f(s)
}

pub fn forget_device(device: &str) {
    with_store(|s| s.forget_device(device));
}

fn reply(r: R<Value>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(Fail(code, msg)) => (code, Json(json!({"error": msg}))).into_response(),
    }
}

fn ok_json<T>(r: R<T>) -> R<Value> {
    r.map(|_| json!({"ok": true}))
}

fn device(dev: &str) -> R<String> {
    device_peer_of(dev).ok_or(Fail(StatusCode::UNAUTHORIZED, "нет устройства"))
}

fn checked(gid: &str) -> R<()> {
    if valid_gid(gid) {
        Ok(())
    } else {
        Err(not_found())
    }
}

// ── /mobile/groups (токен устройства) ──

pub(crate) async fn m_list(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
) -> Response {
    reply(device(&dev).map(|me| json!({"groups": with_store(|s| s.list_for(&me))})))
}

pub(crate) async fn m_open(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
) -> Response {
    reply(device(&dev).map(|me| json!({"groups": with_store(|s| s.open_for(&me))})))
}

#[derive(Deserialize)]
pub struct JoinReq {
    #[serde(default)]
    invite_code: Option<String>,
    #[serde(default)]
    member_pub: Option<String>,
}

pub(crate) async fn m_join(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Json(req): Json<JoinReq>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        let alias = with_store(|s| {
            s.join(
                &gid,
                &me,
                req.invite_code.as_deref(),
                req.member_pub.as_deref(),
            )
        })?;
        Ok(json!({"alias": alias}))
    })())
}

pub(crate) async fn m_leave(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        ok_json(with_store(|s| s.leave(&gid, &me)))
    })())
}

pub(crate) async fn m_members(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        with_store(|s| s.members(&gid, &me))
    })())
}

#[derive(Deserialize)]
pub struct PubReq {
    member_pub: String,
}

pub(crate) async fn m_pub(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Json(req): Json<PubReq>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        ok_json(with_store(|s| s.set_pub(&gid, &me, &req.member_pub)))
    })())
}

#[derive(Deserialize)]
pub struct KeysReq {
    epoch: u32,
    boxes: HashMap<String, String>,
}

pub(crate) async fn m_keys_get(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        with_store(|s| s.my_keys(&gid, &me))
    })())
}

pub(crate) async fn m_keys_put(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Json(req): Json<KeysReq>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        let n = with_store(|s| s.put_keys(&gid, &me, req.epoch, &req.boxes))?;
        Ok(json!({"stored": n}))
    })())
}

pub(crate) async fn m_log_get(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        let after = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
        let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(200);
        with_store(|s| s.pull(&gid, &me, after, limit))
    })())
}

#[derive(Deserialize)]
pub struct PostReq {
    #[serde(default)]
    epoch: u32,
    payload_b64: String,
}

pub(crate) async fn m_log_post(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Json(req): Json<PostReq>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        let seq = with_store(|s| s.append(&gid, &me, req.epoch, &req.payload_b64))?;
        Ok(json!({"seq": seq}))
    })())
}

/// Больше групп одно устройство вести не может (защита узла от захламления).
const MAX_OWNED_PER_DEVICE: usize = 10;

#[derive(Deserialize)]
pub struct MobileCreateReq {
    name: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    about: String,
    kind: String,
}

/// Телефон создаёт группу и становится её владельцем: ключ группы и права раздаёт он, а не владелец узла.
pub(crate) async fn m_create(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Json(req): Json<MobileCreateReq>,
) -> Response {
    reply((|| {
        let me = device(&dev)?;
        let kind = match req.kind.as_str() {
            "open" => Kind::Open,
            "invite" => Kind::Invite,
            _ => return Err(bad("тип группы: «open» (открытая) или «invite» (по приглашению)")),
        };
        let id = with_store(|s| {
            if s.owned_count(&me) >= MAX_OWNED_PER_DEVICE {
                return Err(Fail(StatusCode::TOO_MANY_REQUESTS, "вы уже ведёте максимум групп"));
            }
            let id = s.create_as(&req.name, &req.topic, kind, &me, &[], true)?;
            s.set_about(&id, &req.about);
            Ok(id)
        })?;
        Ok(json!({"id": id}))
    })())
}

#[derive(Deserialize)]
pub struct ModReq {
    action: String,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    seq: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
}

pub(crate) async fn m_mod(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Json(req): Json<ModReq>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        ok_json(with_store(|s| {
            s.moderate(
                &gid,
                &me,
                &req.action,
                req.alias.as_deref(),
                req.seq,
                req.reason.as_deref(),
            )
        }))
    })())
}

pub(crate) async fn m_modlog(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        with_store(|s| s.modlog(&gid, Some(&me)))
    })())
}

#[derive(Deserialize, Default)]
pub struct InviteReq {
    #[serde(default)]
    uses: Option<u32>,
    #[serde(default)]
    ttl_hours: Option<u64>,
}

/// Необязательное тело запроса: пустое — значения по умолчанию, а неверный JSON или неверные типы — ошибка 400 (а не тихие умолчания).
fn optional_json<T: serde::de::DeserializeOwned + Default>(body: &[u8]) -> Result<T, Response> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": format!("неверные данные: {e}")}))).into_response())
}

pub(crate) async fn m_invite(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let req: InviteReq = match optional_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        with_store(|s| {
            s.invite(
                &gid,
                &me,
                req.uses.unwrap_or(1),
                req.ttl_hours.map(|h| h.saturating_mul(3_600_000)),
            )
        })
    })())
}

#[derive(Deserialize)]
pub struct AfterQ {
    #[serde(default)]
    after: u64,
}

/// Стирания модераторов после курсора (для участника, который пропустил намёки).
pub(crate) async fn m_deleted(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    axum::extract::Query(q): axum::extract::Query<AfterQ>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        with_store(|s| s.deletions_after(&gid, &me, q.after))
    })())
}

#[derive(Deserialize)]
pub struct TransferReq {
    alias: String,
}

/// Владелец передаёт группу участнику.
pub(crate) async fn m_transfer(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Json(req): Json<TransferReq>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        ok_json(with_store(|s| s.transfer_owner(&gid, &me, &req.alias)))
    })())
}

#[derive(Deserialize)]
pub struct SettingsReq {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    about: Option<String>,
}

/// Владелец меняет название, тематику и описание группы.
pub(crate) async fn m_settings(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    Json(req): Json<SettingsReq>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        ok_json(with_store(|s| s.update_settings(&gid, &me, req.name.as_deref(), req.topic.as_deref(), req.about.as_deref())))
    })())
}

#[derive(Deserialize, Default)]
pub struct RequestReq {
    #[serde(default)]
    name: String,
}

/// Просьба прислать приглашение в группу «по приглашению».
pub(crate) async fn m_request(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let req: RequestReq = match optional_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        ok_json(with_store(|s| s.request_join(&gid, &me, &req.name)))
    })())
}

pub(crate) async fn m_requests(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        with_store(|s| s.requests_of(&gid, &me))
    })())
}

pub(crate) async fn m_request_done(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path((gid, rid)): Path<(String, String)>,
) -> Response {
    reply((|| {
        checked(&gid)?;
        let me = device(&dev)?;
        ok_json(with_store(|s| s.resolve_request(&gid, &me, &rid)))
    })())
}

// ── /api/mobile/groups (страница узла на этом компьютере) ──

fn device_by_prefix(id: &str) -> R<String> {
    crate::mobile_api::device_peer_by_prefix(id)
        .ok_or(Fail(StatusCode::NOT_FOUND, "нет такого устройства"))
}

pub async fn pc_list() -> Response {
    let devices = crate::mobile_api::device_list();
    reply(Ok(
        json!({"groups": with_store(|s| s.overview(&devices)), "devices": devices.iter().map(|(p, n)| json!({"id": &p[..16], "name": n})).collect::<Vec<_>>()}),
    ))
}

pub async fn pc_delete(Path(gid): Path<String>) -> Response {
    reply((|| {
        checked(&gid)?;
        ok_json(with_store(|s| s.delete(&gid)))
    })())
}


#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path().join("groups"));
        (d, s)
    }

    const OWNER: &str = "aa";
    const BOB: &str = "bb";
    const EVE: &str = "ee";

    #[test]
    fn deletions_and_sealer_of_each_generation_are_kept_for_late_members() {
        let (_d, mut s) = store();
        let gid = s.create_as("Г", "т", Kind::Open, OWNER, &[], true).unwrap();
        s.set_pub(&gid, OWNER, &b64(&x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::random_from_rng(rand::thread_rng())).to_bytes())).unwrap();
        s.join(&gid, BOB, None, Some(&b64(&x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::random_from_rng(rand::thread_rng())).to_bytes()))).unwrap();
        let bob = s.list_for(BOB)[0]["my_alias"].as_str().unwrap().to_string();
        let owner_pub = s.members(&gid, OWNER).unwrap()["members"].as_array().unwrap().iter().find(|m| m["role"] == "owner").unwrap()["member_pub"].clone();
        let mut boxes = HashMap::new();
        boxes.insert(bob.clone(), b64(b"box1"));
        s.put_keys(&gid, OWNER, 1, &boxes).unwrap();
        let k = s.my_keys(&gid, BOB).unwrap();
        assert_eq!(k["boxes"][0]["owner_pub"], owner_pub, "у конверта — ключ того, кто его запечатал");
        // стирания по курсору
        let a = s.append(&gid, OWNER, 1, &b64(b"one")).unwrap();
        let b = s.append(&gid, OWNER, 1, &b64(b"two")).unwrap();
        assert_eq!(s.deletions_after(&gid, BOB, 0).unwrap()["deleted"].as_array().unwrap().len(), 0);
        s.moderate(&gid, OWNER, "delete_entry", None, Some(a), None).unwrap();
        let d1 = s.deletions_after(&gid, BOB, 0).unwrap();
        assert_eq!(d1["deleted"], json!([a]));
        let cur = d1["cursor"].as_u64().unwrap();
        s.moderate(&gid, OWNER, "delete_entry", None, Some(b), None).unwrap();
        assert_eq!(s.deletions_after(&gid, BOB, cur).unwrap()["deleted"], json!([b]), "только новое после курсора");
        assert!(s.deletions_after(&gid, EVE, 0).is_err(), "посторонний не видит");
    }

    #[test]
    fn a_broken_or_unwritable_index_is_an_error_not_a_silent_success() {
        // аудит Codex №7
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("groups");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.json"), b"{broken").unwrap();
        let mut s = Store::open(dir.clone());
        assert!(s.create_as("Г", "т", Kind::Open, OWNER, &[], true).is_err(), "повреждённый индекс: изменения запрещены");
        assert_eq!(std::fs::read(dir.join("index.json")).unwrap(), b"{broken", "и он не перезаписан пустым");
        // каталог групп — обычный файл: запись невозможна → ошибка, а не «успех в памяти»
        let file_dir = d.path().join("not_a_dir");
        std::fs::write(&file_dir, b"x").unwrap();
        let mut s2 = Store::open(file_dir);
        assert!(s2.create_as("Г", "т", Kind::Open, OWNER, &[], true).is_err());
    }

    #[test]
    fn deleting_an_entry_follows_the_role_of_its_author() {
        // аудит Codex, раунд 2 (R2-2): модератор не стирает запись владельца
        let (_d, mut s) = store();
        let gid = s.create_as("Г", "т", Kind::Open, OWNER, &[], true).unwrap();
        for d in [BOB, EVE, "cc"] {
            s.join(&gid, d, None, None).unwrap();
        }
        let alias = |s: &Store, d: &str| s.list_for(d)[0]["my_alias"].as_str().unwrap().to_string();
        let (bob, eve) = (alias(&s, BOB), alias(&s, EVE));
        s.moderate(&gid, OWNER, "promote", Some(&bob), None, None).unwrap(); // Боб — модератор
        s.moderate(&gid, OWNER, "make_admin", Some(&eve), None, None).unwrap(); // Ева — админ
        let by_owner = s.append(&gid, OWNER, 0, &b64(b"o")).unwrap();
        let by_admin = s.append(&gid, EVE, 0, &b64(b"a")).unwrap();
        let by_mod = s.append(&gid, BOB, 0, &b64(b"m")).unwrap();
        let by_member = s.append(&gid, "cc", 0, &b64(b"x")).unwrap();
        assert!(s.moderate(&gid, BOB, "delete_entry", None, Some(by_owner), None).is_err(), "модератор — не запись владельца");
        assert!(s.moderate(&gid, BOB, "delete_entry", None, Some(by_admin), None).is_err(), "модератор — не запись админа");
        assert!(s.moderate(&gid, EVE, "delete_entry", None, Some(by_owner), None).is_err(), "админ — не запись владельца");
        assert!(s.moderate(&gid, BOB, "delete_entry", None, Some(by_member), None).is_ok(), "модератор — запись рядового");
        assert!(s.moderate(&gid, BOB, "delete_entry", None, Some(by_mod), None).is_ok(), "своя запись");
        assert!(s.moderate(&gid, OWNER, "delete_entry", None, Some(by_admin), None).is_ok(), "владелец — запись админа");
    }

    #[test]
    fn weak_member_keys_are_refused() {
        let (_d, mut s) = store();
        let gid = s.create_as("Г", "т", Kind::Open, OWNER, &[], true).unwrap();
        assert!(s.join(&gid, BOB, None, Some(&b64(&[0u8; 32]))).is_err(), "нулевой ключ");
        let mut order8 = [0u8; 32];
        order8[0] = 1; // точка порядка 1 (u = 1)
        assert!(s.join(&gid, BOB, None, Some(&b64(&order8))).is_err(), "точка малого порядка");
        s.join(&gid, BOB, None, None).unwrap();
        assert!(s.set_pub(&gid, BOB, &b64(&[0u8; 32])).is_err());
        let good = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::random_from_rng(rand::thread_rng())).to_bytes();
        s.set_pub(&gid, BOB, &b64(&good)).unwrap();
    }

    #[test]
    fn owner_hands_the_group_over() {
        let (_d, mut s) = store();
        let gid = s.create_as("Г", "т", Kind::Open, OWNER, &[], true).unwrap();
        s.join(&gid, BOB, None, Some(&b64(&[7u8; 32]))).unwrap();
        s.join(&gid, EVE, None, None).unwrap();
        let alias = |s: &Store, d: &str| s.list_for(d)[0]["my_alias"].as_str().unwrap().to_string();
        let (owner, bob, eve) = (alias(&s, OWNER), alias(&s, BOB), alias(&s, EVE));
        assert!(s.transfer_owner(&gid, BOB, &bob).is_err(), "не владелец не передаёт");
        assert!(s.transfer_owner(&gid, OWNER, &owner).is_err(), "самому себе");
        assert!(s.transfer_owner(&gid, OWNER, "нет такого").is_err());
        assert!(s.transfer_owner(&gid, OWNER, &eve).is_err(), "участнику без ключа группы нельзя");
        s.transfer_owner(&gid, OWNER, &bob).unwrap();
        assert_eq!(s.list_for(BOB)[0]["role"], "owner");
        assert_eq!(s.list_for(OWNER)[0]["role"], "admin", "прежний владелец становится админом");
        assert_eq!(s.list_for(BOB)[0]["rekey_needed"], true, "новый владелец должен сменить ключ");
        assert_eq!(s.list_for(OWNER)[0]["rekey_needed"], false, "а прежнему это уже не видно");
        // права поменялись местами
        assert!(s.transfer_owner(&gid, OWNER, &eve).is_err(), "прежний владелец больше не передаёт");
        assert!(s.moderate(&gid, OWNER, "kick", Some(&bob), None, None).is_err(), "и не трогает нового владельца");
        assert!(s.moderate(&gid, BOB, "demote", Some(&owner), None, None).is_ok(), "новый владелец может разжаловать прежнего");
        assert_eq!(s.modlog(&gid, Some(BOB)).unwrap()["modlog"].as_array().unwrap().iter().filter(|x| x["action"] == "transfer").count(), 1);
    }

    #[test]
    fn admin_tier_follows_the_matrix() {
        let (_d, mut s) = store();
        let gid = s.create_as("Г", "т", Kind::Open, OWNER, &[], true).unwrap();
        for d in [BOB, EVE, "cc", "dd"] {
            s.join(&gid, d, None, None).unwrap();
        }
        let alias = |s: &Store, d: &str| s.list_for(d)[0]["my_alias"].as_str().unwrap().to_string();
        let (bob, eve, cc, dd) = (alias(&s, BOB), alias(&s, EVE), alias(&s, "cc"), alias(&s, "dd"));
        // владелец назначает админа и модератора
        s.moderate(&gid, OWNER, "make_admin", Some(&bob), None, None).unwrap();
        s.moderate(&gid, OWNER, "promote", Some(&eve), None, None).unwrap();
        assert_eq!(s.list_for(BOB)[0]["role"], "admin");
        // админ: действия модератора, роли «модератор/участник», настройки — но не админов и не владельца
        s.moderate(&gid, BOB, "mute", Some(&cc), None, None).unwrap();
        s.moderate(&gid, BOB, "demote", Some(&eve), None, None).unwrap();
        s.moderate(&gid, BOB, "promote", Some(&eve), None, None).unwrap();
        assert!(s.moderate(&gid, BOB, "make_admin", Some(&cc), None, None).is_err(), "админов назначает только владелец");
        let owner_alias = alias(&s, OWNER);
        assert!(s.moderate(&gid, BOB, "kick", Some(&owner_alias), None, None).is_err(), "владельца нельзя");
        s.update_settings(&gid, BOB, Some("Новое"), None, None).unwrap();
        // модератор: рядовых — да; админа, другого модератора и роли — нет
        assert!(s.moderate(&gid, EVE, "mute", Some(&dd), None, None).is_ok());
        assert!(s.moderate(&gid, EVE, "kick", Some(&bob), None, None).is_err(), "админа модератор не трогает");
        assert!(s.moderate(&gid, EVE, "promote", Some(&dd), None, None).is_err(), "роли модератор не назначает");
        assert!(s.update_settings(&gid, EVE, Some("Взлом"), None, None).is_err());
        // рядовой ничего
        assert!(s.moderate(&gid, "dd", "mute", Some(&cc), None, None).is_err());
        // админ не трогает другого админа; владелец — может
        s.moderate(&gid, OWNER, "make_admin", Some(&eve), None, None).unwrap();
        assert!(s.moderate(&gid, BOB, "kick", Some(&eve), None, None).is_err(), "админа трогает только владелец");
        s.moderate(&gid, OWNER, "demote", Some(&eve), None, None).unwrap();
        assert_eq!(s.list_for(EVE)[0]["role"], "member");
    }

    #[test]
    fn owner_changes_group_settings() {
        let (_d, mut s) = store();
        let gid = s.create_as("Старое", "Тема", Kind::Invite, OWNER, &[], true).unwrap();
        s.join(&gid, BOB, None, None).ok();
        assert!(s.update_settings(&gid, BOB, Some("Взлом"), None, None).is_err(), "не владелец не меняет");
        assert!(s.update_settings(&gid, OWNER, Some("  "), None, None).is_err(), "название не бывает пустым");
        assert!(s.update_settings(&gid, OWNER, None, Some(""), None).is_err(), "тематика не бывает пустой");
        s.update_settings(&gid, OWNER, Some("Новое"), Some("Садоводство"), Some("про грядки")).unwrap();
        let g = &s.list_for(OWNER)[0];
        assert_eq!((g["name"].as_str(), g["topic"].as_str(), g["about"].as_str()), (Some("Новое"), Some("Садоводство"), Some("про грядки")));
        s.update_settings(&gid, OWNER, None, None, Some("")).unwrap();
        assert_eq!(s.list_for(OWNER)[0]["about"], "", "описание можно очистить");
        assert_eq!(s.modlog(&gid, Some(OWNER)).unwrap()["modlog"].as_array().unwrap().last().unwrap()["action"], "settings");
    }

    #[test]
    fn week_old_requests_are_wiped_from_disk_with_the_device_number() {
        let (d, mut s) = store();
        let inv = s.create("Клуб", "", Kind::Invite, OWNER, &[]).unwrap();
        s.request_join(&inv, EVE, "Ева").unwrap();
        s.groups.get_mut(&inv).unwrap().requests[0].ts = now_ms() - REQUEST_TTL_MS - 1000;
        s.save().unwrap();
        s.expire_requests().unwrap();
        let again = Store::open(d.path().join("groups"));
        assert!(again.groups[&inv].requests.is_empty(), "после перезапуска просьбы нет");
        assert!(!std::fs::read_to_string(d.path().join("groups").join("index.json")).unwrap().contains(&format!("\"peer\":\"{EVE}\"")), "номера заявителя на диске нет");
    }

    #[test]
    fn reading_requests_wipes_expired_ones_from_disk_and_a_broken_index_reports_it() {
        let (d, mut s) = store();
        let inv = s.create("Клуб", "", Kind::Invite, OWNER, &[]).unwrap();
        s.request_join(&inv, EVE, "Ева").unwrap();
        s.groups.get_mut(&inv).unwrap().requests[0].ts = now_ms() - REQUEST_TTL_MS - 1000;
        s.save().unwrap();
        assert!(s.requests_of(&inv, OWNER).unwrap()["requests"].as_array().unwrap().is_empty());
        assert!(Store::open(d.path().join("groups")).groups[&inv].requests.is_empty(), "чтение стёрло и на диске");
        std::fs::write(d.path().join("groups").join("index.json"), b"{broken").unwrap();
        assert!(Store::open(d.path().join("groups")).expire_requests().is_err(), "при повреждённом индексе уборка честно не выполнена");
    }

    #[test]
    fn join_requests_for_invite_groups() {
        let (_d, mut s) = store();
        let open = s.create("Двор", "", Kind::Open, OWNER, &[]).unwrap();
        let inv = s.create("Клуб", "", Kind::Invite, OWNER, &[]).unwrap();
        let seen: Vec<String> = s.open_for(EVE).iter().map(|g| g["name"].as_str().unwrap().to_string()).collect();
        assert!(seen.contains(&"Двор".to_string()) && seen.contains(&"Клуб".to_string()), "в списке и открытая, и «по приглашению»");
        assert!(s.request_join(&open, EVE, "Ева").is_err(), "в открытую просто вступают");
        s.request_join(&inv, EVE, "Ева").unwrap();
        assert!(s.request_join(&inv, EVE, "Ева").is_err(), "повторная просьба");
        assert_eq!(s.open_for(EVE).iter().find(|g| g["id"] == inv.as_str()).unwrap()["requested"], true);
        assert!(s.requests_of(&inv, EVE).is_err(), "посторонний не видит просьб");
        s.join(&inv, BOB, None, None).ok();
        let r = s.requests_of(&inv, OWNER).unwrap();
        assert_eq!(r["requests"][0]["name"], "Ева");
        assert_eq!(s.list_for(OWNER).iter().find(|g| g["id"] == inv.as_str()).unwrap()["requests"], 1);
        let rid = r["requests"][0]["id"].as_str().unwrap().to_string();
        assert!(s.resolve_request(&inv, EVE, &rid).is_err());
        s.resolve_request(&inv, OWNER, &rid).unwrap();
        assert_eq!(s.requests_of(&inv, OWNER).unwrap()["requests"].as_array().unwrap().len(), 0);
        // забытое устройство уносит и свою просьбу
        s.request_join(&inv, EVE, "Ева").unwrap();
        s.forget_device(EVE);
        assert_eq!(s.requests_of(&inv, OWNER).unwrap()["requests"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn create_post_pull_and_survive_reopen() {
        let (d, mut s) = store();
        let gid = s
            .create("Семья", "", Kind::Invite, OWNER, &[BOB.into()])
            .unwrap();
        assert_eq!(s.list_for(OWNER)[0]["role"], "owner");
        assert_eq!(s.list_for(BOB)[0]["role"], "member");
        assert!(s.list_for(EVE).is_empty());
        assert_eq!(s.append(&gid, OWNER, 0, &b64(b"one")).unwrap(), 1);
        assert_eq!(s.append(&gid, BOB, 0, &b64(b"two")).unwrap(), 2);
        assert!(
            s.append(&gid, EVE, 0, &b64(b"x")).is_err(),
            "посторонний не пишет"
        );
        assert!(s.pull(&gid, EVE, 0, 10).is_err(), "посторонний не читает");
        let p = s.pull(&gid, BOB, 1, 10).unwrap();
        assert_eq!(p["entries"].as_array().unwrap().len(), 1);
        assert_eq!(p["entries"][0]["payload_b64"], b64(b"two"));
        assert_eq!(p["last_seq"], 2);
        // после перезапуска узла всё на месте, номера продолжаются
        drop(s);
        let mut s = Store::open(d.path().join("groups"));
        assert_eq!(
            s.pull(&gid, OWNER, 0, 10).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(s.append(&gid, OWNER, 0, &b64(b"three")).unwrap(), 3);
        // на диске нет номеров устройств
        let raw = std::fs::read_to_string(d.path().join("groups/index.json")).unwrap();
        assert!(!raw.contains(&format!("\"{BOB}\"")));
    }

    #[test]
    fn joining_depends_on_group_kind() {
        let (_d, mut s) = store();
        let other = s.create("A", "", Kind::Invite, OWNER, &[]).unwrap();
        let open = s.create("B", "", Kind::Open, OWNER, &[]).unwrap();
        let inv = s.create("C", "", Kind::Invite, OWNER, &[]).unwrap();
        assert!(s.join(&other, BOB, None, None).is_err(), "в группу «по приглашению» без кода не войти");
        assert_eq!(s.open_for(BOB).len(), 3, "в списке все три: открытая (вступить) и две «по приглашению» (попросить)");
        s.join(&open, BOB, None, None).unwrap();
        assert_eq!(s.open_for(BOB).len(), 2);
        assert!(s.join(&inv, BOB, Some("WRONG"), None).is_err());
        assert!(
            s.invite(&inv, BOB, 1, None).is_err(),
            "не участник не приглашает"
        );
        let code = s.invite(&inv, OWNER, 1, None).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            code.len() == 14 && code.split('-').all(|p| p.len() == 4),
            "{code}"
        );
        s.join(&inv, BOB, Some(&code.to_lowercase()), None).unwrap();
        assert!(
            s.join(&inv, EVE, Some(&code), None).is_err(),
            "код одноразовый"
        );
    }

    #[test]
    fn invitations_are_issued_by_staff_only() {
        let (_d, mut s) = store();
        let gid = s
            .create("Закрытая", "", Kind::Invite, OWNER, &[BOB.into(), EVE.into(), "cc".into()])
            .unwrap();
        let alias = |s: &Store, dev: &str| s.list_for(dev)[0]["my_alias"].as_str().unwrap().to_string();
        let (bob, eve) = (alias(&s, BOB), alias(&s, EVE));
        s.moderate(&gid, OWNER, "make_admin", Some(&bob), None, None).unwrap();
        s.moderate(&gid, OWNER, "promote", Some(&eve), None, None).unwrap();
        assert!(s.invite(&gid, OWNER, 1, None).is_ok(), "владелец");
        assert!(s.invite(&gid, BOB, 1, None).is_ok(), "админ");
        assert!(s.invite(&gid, EVE, 1, None).is_ok(), "модератор выдаёт приглашение по просьбе");
        assert!(s.invite(&gid, "cc", 1, None).is_err(), "рядовой не выдаёт");
        // модератор: стирает записи рядового, банит и исключает рядового; админа не трогает
        let seq = s.append(&gid, "cc", 0, &b64(b"x")).unwrap();
        assert!(s.moderate(&gid, EVE, "delete_entry", None, Some(seq), None).is_ok());
        let cc = alias(&s, "cc");
        assert!(s.moderate(&gid, EVE, "ban", Some(&cc), None, None).is_ok());
        assert!(s.moderate(&gid, EVE, "kick", Some(&bob), None, None).is_err());
    }

    #[test]
    fn moderation_rules_and_log() {
        let (_d, mut s) = store();
        let gid = s
            .create("G", "", Kind::Open, OWNER, &[BOB.into(), EVE.into()])
            .unwrap();
        let alias =
            |s: &Store, dev: &str| s.list_for(dev)[0]["my_alias"].as_str().unwrap().to_string();
        let (bob, eve) = (alias(&s, BOB), alias(&s, EVE));
        assert!(
            s.moderate(&gid, BOB, "mute", Some(&eve), None, None)
                .is_err(),
            "участник не модерирует"
        );
        s.moderate(&gid, OWNER, "promote", Some(&bob), None, None)
            .unwrap();
        s.moderate(&gid, BOB, "mute", Some(&eve), None, Some("спам"))
            .unwrap();
        assert!(s.append(&gid, EVE, 0, &b64(b"x")).is_err());
        s.moderate(&gid, BOB, "unmute", Some(&eve), None, None)
            .unwrap();
        let seq = s.append(&gid, EVE, 0, &b64(b"bad")).unwrap();
        s.moderate(&gid, BOB, "delete_entry", None, Some(seq), None)
            .unwrap();
        let p = s.pull(&gid, OWNER, 0, 10).unwrap();
        assert_eq!(p["entries"][0]["deleted"], true);
        assert_eq!(p["entries"][0]["payload_b64"], "");
        assert!(
            s.moderate(&gid, BOB, "promote", Some(&eve), None, None)
                .is_err(),
            "модератор не назначает модераторов"
        );
        s.moderate(&gid, BOB, "ban", Some(&eve), None, None)
            .unwrap();
        assert!(
            s.join(&gid, EVE, None, None).is_err(),
            "запрещённый не вернётся"
        );
        assert!(s.list_for(OWNER)[0]["rekey_needed"].as_bool().unwrap());
        let owner = alias(&s, OWNER);
        assert!(s
            .moderate(&gid, BOB, "kick", Some(&owner), None, None)
            .is_err());
        assert!(s
            .moderate(&gid, OWNER, "unban", Some(&eve), None, None)
            .is_ok());
        s.join(&gid, EVE, None, None).unwrap();
        let log = s.modlog(&gid, Some(BOB)).unwrap();
        let actions: Vec<&str> = log["modlog"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap())
            .collect();
        assert_eq!(
            actions,
            [
                "create",
                "promote",
                "mute",
                "unmute",
                "delete_entry",
                "ban",
                "unban",
                "join"
            ]
        );
    }

    #[test]
    fn keys_are_sealed_per_member_and_only_owner_hands_them_out() {
        let (_d, mut s) = store();
        let gid = s.create("G", "", Kind::Open, OWNER, &[BOB.into()]).unwrap();
        let k = b64(&[1u8; 32]);
        s.set_pub(&gid, BOB, &k).unwrap();
        let bob = s.list_for(BOB)[0]["my_alias"].as_str().unwrap().to_string();
        assert_eq!(s.list_for(OWNER)[0]["pending_keys"][0], bob.as_str());
        let boxes: HashMap<String, String> = [(bob.clone(), b64(b"sealed"))].into();
        assert!(s.put_keys(&gid, BOB, 1, &boxes).is_err());
        assert_eq!(s.put_keys(&gid, OWNER, 1, &boxes).unwrap(), 1);
        let mine = s.my_keys(&gid, BOB).unwrap();
        assert_eq!(mine["epoch"], 1);
        assert_eq!(mine["boxes"][0]["box"], b64(b"sealed"));
        assert!(s.list_for(OWNER)[0]["pending_keys"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(
            s.put_keys(&gid, OWNER, 0, &boxes).is_err(),
            "поколение не идёт назад"
        );
        assert!(
            s.append(&gid, BOB, 2, &b64(b"x")).is_err(),
            "будущего поколения нет"
        );
        s.leave(&gid, BOB).unwrap();
        assert!(s.my_keys(&gid, BOB).is_err());
    }

    #[test]
    fn journal_is_trimmed_but_numbers_keep_going() {
        let (_d, mut s) = store();
        let gid = s.create("G", "", Kind::Invite, OWNER, &[]).unwrap();
        let big = b64(&vec![7u8; MAX_ENTRY - 256]);
        let n = (MAX_JOURNAL_BYTES / big.len() as u64) + 5;
        for _ in 0..n {
            s.append(&gid, OWNER, 0, &big).unwrap();
        }
        let p = s.pull(&gid, OWNER, 0, 5).unwrap();
        let first = p["first_seq"].as_u64().unwrap();
        assert!(first > 1);
        assert_eq!(p["entries"][0]["seq"].as_u64().unwrap(), first);
        assert_eq!(p["last_seq"].as_u64().unwrap(), n);
        assert!(std::fs::metadata(s.log_path(&gid)).unwrap().len() <= MAX_JOURNAL_BYTES);
        // после обрезки индекс верный: дописывание и чтение с середины
        let seq = s.append(&gid, OWNER, 0, &b64(b"tail")).unwrap();
        assert_eq!(
            s.pull(&gid, OWNER, seq - 1, 5).unwrap()["entries"][0]["payload_b64"],
            b64(b"tail")
        );
    }

    #[test]
    fn deleting_and_forgetting() {
        let (d, mut s) = store();
        let gid = s
            .create("G", "", Kind::Invite, OWNER, &[BOB.into()])
            .unwrap();
        s.append(&gid, BOB, 0, &b64(b"x")).unwrap();
        s.forget_device(BOB);
        assert!(s.list_for(BOB).is_empty());
        s.delete(&gid).unwrap();
        assert!(s.list_for(OWNER).is_empty());
        assert!(!d.path().join(format!("groups/{gid}.log")).exists());
    }

    #[test]
    fn hint_reaches_only_members() {
        let gid = "0123456789abcdef0123456789abcdef";
        let h = Hint {
            gid: gid.into(),
            kind: HINT_LOG,
            seq: 9,
            to: Arc::new(vec![member_key(gid, BOB)]),
        };
        let f = h.frame_for(BOB).unwrap();
        assert_eq!(f.len(), 26);
        assert_eq!((f[0], f[17]), (FT_GROUP_HINT, HINT_LOG));
        assert_eq!(u64::from_le_bytes(f[18..26].try_into().unwrap()), 9);
        assert!(h.frame_for(EVE).is_none());
    }
}
