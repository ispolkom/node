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
//! модерации. Создать и удалить группу можно только на компьютере (страница узла).
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
    /// вступить можно по коду приглашения (его выдают владелец и модераторы)
    Invite,
    /// участников добавляет только владелец узла на компьютере
    Closed,
    /// любой телефон этого узла видит группу в списке и может вступить сам
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
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
    gid: String,
    kind: u8,
    seq: u64,
    to: Arc<Vec<String>>,
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

fn member_key(gid: &str, device_peer: &str) -> String {
    hex::encode(Sha256::digest(
        format!("yandi-group-member:{gid}:{device_peer}").as_bytes(),
    ))
}

fn valid_gid(id: &str) -> bool {
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
    dir: PathBuf,
    groups: BTreeMap<String, Group>,
    /// смещения строк журнала (индекс i — запись first_seq + i), строятся при первом обращении
    offsets: HashMap<String, Vec<u64>>,
}

impl Store {
    pub fn open(dir: PathBuf) -> Self {
        let groups: Vec<Group> = std::fs::read_to_string(dir.join("index.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            dir,
            groups: groups.into_iter().map(|g| (g.id.clone(), g)).collect(),
            offsets: HashMap::new(),
        }
    }

    fn save(&self) {
        let list: Vec<&Group> = self.groups.values().collect();
        if let Ok(s) = serde_json::to_vec_pretty(&list) {
            let _ = crate::util::private_file::write_private(&self.dir.join("index.json"), &s);
        }
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
        let name = clean(name, MAX_NAME);
        if name.is_empty() {
            return Err(bad("нужно название группы"));
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
            topic: clean(topic, MAX_TOPIC),
            kind,
            created_ms: now_ms(),
            members: Vec::new(),
            banned: Vec::new(),
            invites: Vec::new(),
            epoch: 0,
            key_boxes: BTreeMap::new(),
            rekey_needed: false,
            first_seq: 1,
            next_seq: 1,
            modlog: Vec::new(),
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
        g.log(PC, "create", None, None, None);
        let to = g.keys();
        self.groups.insert(id.clone(), g);
        self.save();
        hint(&id, HINT_CHANGED, 0, to);
        Ok(id)
    }

    pub fn update(
        &mut self,
        gid: &str,
        name: Option<&str>,
        topic: Option<&str>,
        kind: Option<Kind>,
    ) -> R<()> {
        let g = self.get_mut(gid)?;
        if let Some(n) = name {
            let n = clean(n, MAX_NAME);
            if n.is_empty() {
                return Err(bad("нужно название группы"));
            }
            g.name = n;
        }
        if let Some(t) = topic {
            g.topic = clean(t, MAX_TOPIC);
        }
        if let Some(k) = kind {
            if k != g.kind {
                g.invites.clear();
            }
            g.kind = k;
        }
        g.log(PC, "settings", None, None, None);
        let to = g.keys();
        self.save();
        hint(gid, HINT_CHANGED, 0, to);
        Ok(())
    }

    pub fn add_member(&mut self, gid: &str, device: &str) -> R<String> {
        let g = self.get_mut(gid)?;
        let key = member_key(gid, device);
        if g.member(&key).is_some() {
            return Err(Fail(StatusCode::CONFLICT, "уже участник"));
        }
        if g.members.len() >= MAX_MEMBERS {
            return Err(Fail(StatusCode::INSUFFICIENT_STORAGE, "группа заполнена"));
        }
        // добавление с компьютера снимает запрет
        g.banned.retain(|b| b.key != key);
        let alias = g.new_alias();
        g.members.push(Member {
            key,
            alias: alias.clone(),
            role: Role::Member,
            muted: false,
            joined_ms: now_ms(),
            member_pub: None,
        });
        g.log(PC, "add", Some(&alias), None, None);
        let to = g.keys();
        self.save();
        hint(gid, HINT_CHANGED, 0, to);
        Ok(alias)
    }

    /// Поставить владельцем группы другое устройство (например, если прежнее потеряно). Новый владелец должен сменить ключ.
    pub fn set_owner(&mut self, gid: &str, device: &str) -> R<String> {
        let g = self.get_mut(gid)?;
        let key = member_key(gid, device);
        if g.member(&key).is_none() {
            if g.members.len() >= MAX_MEMBERS {
                return Err(Fail(StatusCode::INSUFFICIENT_STORAGE, "группа заполнена"));
            }
            g.banned.retain(|b| b.key != key);
            let alias = g.new_alias();
            g.members.push(Member {
                key: key.clone(),
                alias,
                role: Role::Member,
                muted: false,
                joined_ms: now_ms(),
                member_pub: None,
            });
        }
        for m in g.members.iter_mut() {
            if m.role == Role::Owner {
                m.role = Role::Member;
            }
            if m.key == key {
                m.role = Role::Owner;
                m.muted = false;
            }
        }
        g.rekey_needed = true;
        let alias = g.member(&key).map(|m| m.alias.clone()).unwrap_or_default();
        g.log(PC, "owner", Some(&alias), None, None);
        let to = g.keys();
        self.save();
        hint(gid, HINT_CHANGED, 0, to);
        Ok(alias)
    }

    pub fn delete(&mut self, gid: &str) -> R<()> {
        let g = self.groups.remove(gid).ok_or_else(not_found)?;
        self.offsets.remove(gid);
        let _ = std::fs::remove_file(self.log_path(gid));
        self.save();
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
            Some(p) if !p.is_empty() => Some(
                b64_ok(p, 32)
                    .filter(|b| b.len() == 32)
                    .ok_or_else(|| bad("неверный ключ"))?,
            )
            .map(|_| p.to_string()),
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
            Kind::Closed => {
                return Err(forbidden(
                    "в закрытую группу добавляет только владелец узла",
                ))
            }
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
        self.save();
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
        self.save();
        hint(gid, HINT_CHANGED, 0, to);
        Ok(())
    }

    pub fn set_pub(&mut self, gid: &str, device: &str, member_pub: &str) -> R<()> {
        if b64_ok(member_pub, 32).map(|b| b.len()) != Some(32) {
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
        self.save();
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
        self.save();
        hint(gid, HINT_KEYS, epoch as u64, to);
        Ok(n)
    }

    pub fn my_keys(&self, gid: &str, device: &str) -> R<Value> {
        let (g, m) = self.me(gid, device)?;
        let boxes: Vec<Value> = g
            .key_boxes
            .iter()
            .filter_map(|(e, b)| b.get(&m.key).map(|x| json!({"epoch": e, "box": x})))
            .collect();
        let owner_pub = g.owner().and_then(|o| o.member_pub.clone());
        Ok(json!({"epoch": g.epoch, "owner_pub": owner_pub, "boxes": boxes}))
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
        self.save();
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

    /// То же со страницы узла: владелец компьютера может всё, кроме смены владельца (для неё `set_owner`).
    pub fn moderate_pc(
        &mut self,
        gid: &str,
        action: &str,
        alias: Option<&str>,
        seq: Option<u64>,
        reason: Option<&str>,
    ) -> R<()> {
        self.apply_mod(gid, PC, Role::Owner, action, alias, seq, reason)
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
            l.p.clear();
            l.deleted = true;
            let author = l.from.clone();
            self.rewrite(gid, &lines)
                .map_err(|_| Fail(StatusCode::INTERNAL_SERVER_ERROR, "не удалось стереть"))?;
            let g = self.get_mut(gid)?;
            g.log(by, "delete_entry", Some(&author), Some(seq), reason);
            let to = g.keys();
            self.save();
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
            self.save();
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
        if target.role == Role::Moderator && by_role != Role::Owner {
            return Err(forbidden("модератора может трогать только владелец группы"));
        }
        let mut notify = g.keys();
        match action {
            "mute" | "unmute" => {
                if let Some(m) = g.members.iter_mut().find(|m| m.key == target.key) {
                    m.muted = action == "mute";
                }
            }
            "promote" | "demote" => {
                if by_role != Role::Owner {
                    return Err(forbidden("назначает модераторов только владелец группы"));
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
        self.save();
        hint(gid, HINT_CHANGED, 0, notify);
        Ok(())
    }

    /// Код приглашения (для групп «по приглашению»): выдают владелец и модераторы. Узел хранит только хэш кода.
    pub fn invite(&mut self, gid: &str, device: &str, uses: u32, ttl_ms: Option<u64>) -> R<Value> {
        let (g, me) = self.me(gid, device)?;
        if g.kind != Kind::Invite {
            return Err(forbidden("в эту группу не вступают по приглашению"));
        }
        if me.role == Role::Member {
            return Err(forbidden("приглашает владелец группы или модератор"));
        }
        let by = me.alias.clone();
        self.make_invite(gid, &by, uses, ttl_ms)
    }

    pub fn invite_pc(&mut self, gid: &str, uses: u32, ttl_ms: Option<u64>) -> R<Value> {
        if self.get(gid)?.kind != Kind::Invite {
            return Err(forbidden("в эту группу не вступают по приглашению"));
        }
        self.make_invite(gid, PC, uses, ttl_ms)
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
        self.save();
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
                    "id": g.id, "name": g.name, "topic": g.topic, "kind": g.kind, "my_alias": m.alias, "role": m.role, "muted": m.muted,
                    "members": g.members.len(), "epoch": g.epoch, "first_seq": g.first_seq, "last_seq": g.next_seq - 1,
                    "rekey_needed": owner && g.rekey_needed, "pending_keys": pending,
                }))
            })
            .collect()
    }

    pub fn open_for(&self, device: &str) -> Vec<Value> {
        self.groups
            .values()
            .filter(|g| g.kind == Kind::Open)
            .filter(|g| {
                let k = member_key(&g.id, device);
                g.member(&k).is_none() && !g.banned.iter().any(|b| b.key == k)
            })
            .map(|g| json!({"id": g.id, "name": g.name, "topic": g.topic, "members": g.members.len()}))
            .collect()
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
                    "id": g.id, "name": g.name, "topic": g.topic, "kind": g.kind, "created_ms": g.created_ms, "epoch": g.epoch,
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
        for g in self.groups.values_mut() {
            let k = member_key(&g.id, device);
            if let Some(m) = g.drop_member(&k) {
                g.log(PC, "removed_device", Some(&m.alias), None, None);
                changed.push((g.id.clone(), g.keys()));
            }
        }
        if !changed.is_empty() {
            self.save();
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

fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> T {
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

pub(crate) async fn m_invite(
    axum::Extension(DeviceKey(dev)): axum::Extension<DeviceKey>,
    Path(gid): Path<String>,
    body: Option<Json<InviteReq>>,
) -> Response {
    let req = body.map(|b| b.0).unwrap_or_default();
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

#[derive(Deserialize)]
pub struct CreateReq {
    name: String,
    #[serde(default)]
    topic: String,
    kind: Kind,
    /// устройство-владелец группы (начало номера из списка устройств): у него будет ключ группы
    owner_device: String,
    #[serde(default)]
    members: Vec<String>,
}

pub async fn pc_create(Json(req): Json<CreateReq>) -> Response {
    reply((|| {
        let owner = device_by_prefix(&req.owner_device)?;
        let members = req
            .members
            .iter()
            .map(|m| device_by_prefix(m))
            .collect::<R<Vec<_>>>()?;
        let id = with_store(|s| s.create(&req.name, &req.topic, req.kind, &owner, &members))?;
        Ok(json!({"id": id}))
    })())
}

#[derive(Deserialize)]
pub struct UpdateReq {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    kind: Option<Kind>,
}

pub async fn pc_update(Path(gid): Path<String>, Json(req): Json<UpdateReq>) -> Response {
    reply((|| {
        checked(&gid)?;
        ok_json(with_store(|s| {
            s.update(&gid, req.name.as_deref(), req.topic.as_deref(), req.kind)
        }))
    })())
}

pub async fn pc_delete(Path(gid): Path<String>) -> Response {
    reply((|| {
        checked(&gid)?;
        ok_json(with_store(|s| s.delete(&gid)))
    })())
}

#[derive(Deserialize)]
pub struct DeviceReq {
    device: String,
}

pub async fn pc_add_member(Path(gid): Path<String>, Json(req): Json<DeviceReq>) -> Response {
    reply((|| {
        checked(&gid)?;
        let d = device_by_prefix(&req.device)?;
        let alias = with_store(|s| s.add_member(&gid, &d))?;
        Ok(json!({"alias": alias}))
    })())
}

pub async fn pc_set_owner(Path(gid): Path<String>, Json(req): Json<DeviceReq>) -> Response {
    reply((|| {
        checked(&gid)?;
        let d = device_by_prefix(&req.device)?;
        let alias = with_store(|s| s.set_owner(&gid, &d))?;
        Ok(json!({"alias": alias}))
    })())
}

pub async fn pc_mod(Path(gid): Path<String>, Json(req): Json<ModReq>) -> Response {
    reply((|| {
        checked(&gid)?;
        ok_json(with_store(|s| {
            s.moderate_pc(
                &gid,
                &req.action,
                req.alias.as_deref(),
                req.seq,
                req.reason.as_deref(),
            )
        }))
    })())
}

pub async fn pc_modlog(Path(gid): Path<String>) -> Response {
    reply((|| {
        checked(&gid)?;
        with_store(|s| s.modlog(&gid, None))
    })())
}

pub async fn pc_invite(Path(gid): Path<String>, body: Option<Json<InviteReq>>) -> Response {
    let req = body.map(|b| b.0).unwrap_or_default();
    reply((|| {
        checked(&gid)?;
        with_store(|s| {
            s.invite_pc(
                &gid,
                req.uses.unwrap_or(1),
                req.ttl_hours.map(|h| h.saturating_mul(3_600_000)),
            )
        })
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
    fn create_post_pull_and_survive_reopen() {
        let (d, mut s) = store();
        let gid = s
            .create("Семья", "", Kind::Closed, OWNER, &[BOB.into()])
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
        let closed = s.create("A", "", Kind::Closed, OWNER, &[]).unwrap();
        let open = s.create("B", "", Kind::Open, OWNER, &[]).unwrap();
        let inv = s.create("C", "", Kind::Invite, OWNER, &[]).unwrap();
        assert!(s.join(&closed, BOB, None, None).is_err());
        assert_eq!(s.open_for(BOB).len(), 1);
        s.join(&open, BOB, None, None).unwrap();
        assert!(s.open_for(BOB).is_empty());
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
        let gid = s.create("G", "", Kind::Closed, OWNER, &[]).unwrap();
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
            .create("G", "", Kind::Closed, OWNER, &[BOB.into()])
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
