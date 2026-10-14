//! Группы на компьютере: узел как УЧАСТНИК (а не админ-обход).
//!
//! Владелец узла живёт за ПК, и ему нужны свои группы и группы телефонов, куда его пригласили. Для групп компьютер — такое же устройство,
//! как телефон: у него свой ключ в каждой группе, он вступает по приглашению или просьбе, создаёт группы и владеет ими. Содержимое он
//! расшифровывает только в тех группах, где состоит сам; в остальных оно недоступно (ключ группы раздаёт владелец).
//!
//! Формат шифрования байт-в-байт тот же, что в приложении (`group_crypto.dart`, см. docs/CLIENT_WIRE.md и контрольные векторы в тестах).
//! Всё на диске лежит в закрытом виде: состояние (ключи групп, закрытые ключи участника) и расшифрованная лента шифруются AES-256-GCM
//! ключом, выведенным из мастер-ключа узла (тем же, что защищает историю чатов), поэтому каталог данных без мастер-пароля ничего не
//! даёт. Узел обращается к хранилищу групп прямо в процессе, без сети.
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::mobile_groups::with_store;

pub const KIND_TEXT: u8 = 1;
pub const KIND_IMAGE: u8 = 2;
pub const KIND_FILE: u8 = 3;
pub const KIND_AUDIO: u8 = 4;
pub const KIND_VIDEO: u8 = 5;
pub const KIND_LINK: u8 = 6;
pub const KIND_SYSTEM: u8 = 7;
/// Голос «одобряю / не одобряю» за запись: `{s: номер записи, v: 1 | -1 | 0}`; в ленту не попадает, считается в монетки под записью.
pub const KIND_REACTION: u8 = 8;
const KINDS: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const VERSION: u8 = 1;

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

fn arr32(v: &[u8]) -> Option<[u8; 32]> {
    v.try_into().ok()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ── шифрование групп (как в приложении) ──

fn box_key(my_seed: &[u8; 32], their_pub: &[u8; 32], gid: &str, epoch: u32) -> Result<[u8; 32], String> {
    let shared = StaticSecret::from(*my_seed).diffie_hellman(&PublicKey::from(*their_pub));
    if shared.as_bytes().iter().all(|b| *b == 0) {
        return Err("нулевой общий секрет".into());
    }
    let hk = Hkdf::<Sha256>::new(Some(b"yandi-group-box:v1"), shared.as_bytes());
    let mut out = [0u8; 32];
    hk.expand(format!("{gid}|{epoch}").as_bytes(), &mut out).map_err(|_| "hkdf".to_string())?;
    Ok(out)
}

fn aead_seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = Aes256Gcm::new_from_slice(key).expect("key").encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad }).expect("aes-gcm");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    out
}

fn aead_open(key: &[u8; 32], aad: &[u8], wire: &[u8]) -> Option<Vec<u8>> {
    if wire.len() < 12 + 16 {
        return None;
    }
    Aes256Gcm::new_from_slice(key).ok()?.decrypt(Nonce::from_slice(&wire[..12]), Payload { msg: &wire[12..], aad }).ok()
}

/// Владелец запечатывает ключ группы для участника.
pub fn seal_key(group_key: &[u8; 32], gid: &str, epoch: u32, owner_seed: &[u8; 32], member_pub: &[u8; 32]) -> Result<Vec<u8>, String> {
    let k = box_key(owner_seed, member_pub, gid, epoch)?;
    Ok(aead_seal(&k, format!("yandi-group-key:v1|{gid}|{epoch}").as_bytes(), group_key))
}

/// Участник достаёт ключ группы; конверт должен быть от владельца с этим открытым ключом.
pub fn open_key(wire: &[u8], gid: &str, epoch: u32, my_seed: &[u8; 32], owner_pub: &[u8; 32]) -> Result<[u8; 32], String> {
    let k = box_key(my_seed, owner_pub, gid, epoch)?;
    let plain = aead_open(&k, format!("yandi-group-key:v1|{gid}|{epoch}").as_bytes(), wire).ok_or("конверт не от владельца")?;
    arr32(&plain).ok_or_else(|| "неверный ключ группы".to_string())
}

fn entry_aad(gid: &str, epoch: u32, from: &str, kind: u8) -> Vec<u8> {
    format!("yandi-group-entry:v1|{gid}|{epoch}|{from}|{kind}").into_bytes()
}

pub fn encrypt_entry(key: &[u8; 32], gid: &str, epoch: u32, from: &str, kind: u8, body: &Value) -> Result<Vec<u8>, String> {
    if !KINDS.contains(&kind) {
        return Err("неверный вид записи".into());
    }
    // случайный номер записи внутри шифртекста: повтор того же шифртекста получатель узнаёт и отбрасывает (как в приложении)
    let mut body = body.clone();
    if body.get("i").is_none() {
        let mut id = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut id);
        body["i"] = json!(hex::encode(id));
    }
    let sealed = aead_seal(key, &entry_aad(gid, epoch, from, kind), body.to_string().as_bytes());
    let mut out = vec![VERSION, kind];
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub fn decrypt_entry(key: &[u8; 32], payload: &[u8], gid: &str, epoch: u32, from: &str) -> Result<(u8, Value), String> {
    if payload.len() < 2 + 12 + 16 || payload[0] != VERSION || !KINDS.contains(&payload[1]) {
        return Err("не запись группы".into());
    }
    let kind = payload[1];
    let plain = aead_open(key, &entry_aad(gid, epoch, from, kind), &payload[2..]).ok_or("запись не прошла проверку")?;
    let v: Value = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
    if !v.is_object() {
        return Err("неверное тело".into());
    }
    Ok((kind, v))
}

// ── закрытое хранилище на диске ──

struct Vault {
    dir: PathBuf,
    key: [u8; 32],
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.key.fill(0);
    }
}

impl Vault {
    fn new(dir: PathBuf, master: &[u8; 32]) -> Vault {
        let hk = Hkdf::<Sha256>::new(Some(b"yandi-groups-pc-vault:v1"), master);
        let mut key = [0u8; 32];
        hk.expand(b"state+log", &mut key).expect("hkdf");
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        Vault { dir, key }
    }

    fn seal(&self, aad: &str, plain: &[u8]) -> String {
        b64(&aead_seal(&self.key, aad.as_bytes(), plain))
    }

    fn open(&self, aad: &str, text: &str) -> Option<Vec<u8>> {
        aead_open(&self.key, aad.as_bytes(), &unb64(text.trim())?)
    }

    fn write_private(&self, name: &str, data: &[u8]) -> std::io::Result<()> {
        crate::util::private_file::write_private(&self.dir.join(name), data)
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct GState {
    seed: String,
    pubk: String,
    #[serde(default)]
    pub_sent: bool,
    #[serde(default)]
    owner_pub: Option<String>,
    #[serde(default)]
    keys: BTreeMap<u32, String>,
    #[serde(default)]
    last_seq: u64,
    #[serde(default)]
    want_name: Option<String>,
    #[serde(default)]
    ignored: Vec<String>,
    /// курсор событий стирания на узле (до какого стирания догнал)
    #[serde(default)]
    del_cursor: u64,
    /// человек в этой группе сам скрыл ник и картинку: по умолчанию они показываются
    #[serde(default)]
    card_off: bool,
    /// отпечаток визитки, которую я уже выложил в группе (пусто — ничего не выкладывалось)
    #[serde(default)]
    card_sent: String,
    /// подпись «это я» в этой группе уже приложена
    #[serde(default)]
    proof_sent: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    #[serde(default)]
    groups: HashMap<String, GState>,
    /// группа → название, которое я видел (чтобы показать «удалена на узле», когда её не стало)
    #[serde(default)]
    known: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    seq: u64,
    ts: u64,
    epoch: u32,
    kind: u8,
    from: String,
    body: Value,
    #[serde(default)]
    deleted: bool,
}

struct Client {
    vault: Vault,
    state: State,
    logs: HashMap<String, Vec<Entry>>,
    rules: HashMap<String, Vec<u8>>,
    names: HashMap<String, HashMap<String, String>>,
    problems: HashSet<String>,
    /// группы, у которых часть ленты на диске не открылась (повреждение)
    damaged: HashSet<String>,
}

static CLIENT: OnceLock<Mutex<Option<Client>>> = OnceLock::new();

fn cell() -> &'static Mutex<Option<Client>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

/// Запуск: ключ хранилища выводится из мастер-ключа узла. Без мастер-ключа группы на ПК не работают (ничего не пишется открытым).
pub fn init(master: [u8; 32]) {
    let vault = Vault::new(crate::util::data_dir::data_dir().join("groups_pc"), &master);
    match load_state(&vault) {
        Ok(state) => {
            *cell().lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Client { vault, state, logs: HashMap::new(), rules: HashMap::new(), names: HashMap::new(), problems: HashSet::new(), damaged: HashSet::new() });
        }
        Err(e) => {
            // существующее состояние не открывается — НЕ начинаем с пустого (иначе новые ключи участника и перезапись): группы на ПК
            // выключены до восстановления, файлы не тронуты
            eprintln!("[groups-pc] ⚠️  {e}");
            *INIT_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
        }
    }
}

static INIT_ERROR: Mutex<Option<String>> = Mutex::new(None);
static LOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Среда стёрта: состояние групп ПК и зашифрованные ленты без ключа бесполезны — удаляются (группы начнутся с чистого листа).
pub fn erase_encrypted() {
    let dir = crate::util::data_dir::data_dir().join("groups_pc");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Перенос файлов `groups_pc` с прежнего мастер-ключа на ключ закрытой среды. Идемпотентно: что уже открывается новым ключом, остаётся.
/// Файл, часть которого не открывается ни одним ключом, не меняется (возвращается число таких файлов). Клиент должен быть заперт.
pub fn rekey(old: Option<[u8; 32]>, new: [u8; 32]) -> Result<usize, String> {
    let dir = crate::util::data_dir::data_dir().join("groups_pc");
    rekey_dir(&dir, old, new)
}

fn rekey_dir(dir: &Path, old: Option<[u8; 32]>, new: [u8; 32]) -> Result<usize, String> {
    if !dir.exists() {
        return Ok(0);
    }
    let vnew = Vault::new(dir.to_path_buf(), &new);
    let vold = old.map(|o| Vault::new(dir.to_path_buf(), &o));
    let mut failed = 0usize;
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // aad: у состояния «state», у ленты группы — её номер (имя файла без «.log»)
        let (aad, per_line) = if name == "state.enc" {
            ("state".to_string(), false)
        } else if let Some(gid) = name.strip_suffix(".log") {
            (gid.to_string(), true)
        } else {
            continue;
        };
        let path = entry.path();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
        let reseal = |line: &str| -> Option<(bool, String)> {
            if vnew.open(&aad, line).is_some() {
                return Some((false, line.trim().to_string()));
            }
            let plain = vold.as_ref()?.open(&aad, line)?;
            Some((true, vnew.seal(&aad, &plain)))
        };
        let mut out = Vec::new();
        let mut changed = false;
        let mut ok = true;
        if per_line {
            for l in text.lines().filter(|l| !l.trim().is_empty()) {
                match reseal(l) {
                    Some((c, o)) => {
                        changed |= c;
                        out.push(o);
                    }
                    None => ok = false,
                }
            }
        } else {
            match reseal(text.trim()) {
                Some((c, o)) => {
                    changed = c;
                    out.push(o);
                }
                None => ok = false,
            }
        }
        if !ok {
            failed += 1;
            continue;
        }
        if !changed {
            continue;
        }
        // до замены файла: каждая строка нового вида открывается новым ключом
        if !out.iter().all(|l| vnew.open(&aad, l).is_some()) {
            failed += 1;
            continue;
        }
        let body = if per_line { out.iter().map(|l| format!("{l}\n")).collect::<String>() } else { out.concat() };
        vnew.write_private(&name, body.as_bytes()).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(failed)
}

/// Закрытая среда: убрать ключ и расшифрованное состояние из памяти; файлы на диске не трогаются, фоновая синхронизация ждёт.
pub fn lock() {
    LOCKED.store(true, std::sync::atomic::Ordering::SeqCst);
    *cell().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Выдать ключ области после разблокировки: состояние загружается заново.
pub fn unlock(master: [u8; 32]) {
    *INIT_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = None;
    init(master);
    LOCKED.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Явно «запертое» начальное состояние (закрытая среда ещё не открыта после запуска).
pub fn start_locked() {
    LOCKED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Состояние хранилища: нет файла — пустое; файл есть, но не открывается (повреждён, другой мастер-ключ, ошибка чтения) — ошибка.
fn load_state(vault: &Vault) -> Result<State, String> {
    let path = vault.dir.join("state.enc");
    match std::fs::read_to_string(&path) {
        Ok(t) => vault
            .open("state", &t)
            .and_then(|b| serde_json::from_slice::<State>(&b).ok())
            .ok_or_else(|| format!("состояние групп ПК ({}) не открывается: повреждено или другой мастер-ключ; файл не тронут", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(format!("состояние групп ПК не читается ({e}); файл не тронут")),
    }
}

fn with_client<T>(f: impl FnOnce(&mut Client) -> Result<T, String>) -> Result<T, String> {
    let mut g = cell().lock().unwrap_or_else(|e| e.into_inner());
    match g.as_mut() {
        Some(c) => f(c),
        None if LOCKED.load(std::sync::atomic::Ordering::SeqCst) => Err("закрытая среда заперта: введите пароль владельца".into()),
        None => Err(INIT_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| "группы на компьютере недоступны: нет мастер-ключа узла".into())),
    }
}

fn pc_device() -> Result<String, String> {
    crate::mobile_api::node_peer_hex().ok_or_else(|| "узел ещё запускается".to_string())
}

fn fail(e: crate::mobile_groups::Fail) -> String {
    e.1.to_string()
}

impl Client {
    fn save(&self) -> bool {
        let r = serde_json::to_vec(&self.state)
            .map_err(|e| e.to_string())
            .and_then(|j| self.vault.write_private("state.enc", self.vault.seal("state", &j).as_bytes()).map_err(|e| e.to_string()));
        if let Err(e) = &r {
            eprintln!("[groups-pc] ⚠️  состояние групп не сохранено: {e}");
        }
        r.is_ok()
    }

    fn gstate(&mut self, gid: &str) -> &mut GState {
        if !self.state.groups.contains_key(gid) {
            let mut seed = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut seed);
            let pubk = PublicKey::from(&StaticSecret::from(seed)).to_bytes();
            self.state.groups.insert(gid.to_string(), GState { seed: b64(&seed), pubk: b64(&pubk), ..Default::default() });
            let _ = self.save();
        }
        self.state.groups.get_mut(gid).expect("gstate")
    }

    fn log_name(gid: &str) -> String {
        format!("{gid}.log")
    }

    fn load_log(&mut self, gid: &str) -> &mut Vec<Entry> {
        if !self.logs.contains_key(gid) {
            let text = std::fs::read_to_string(self.vault.dir.join(Self::log_name(gid))).unwrap_or_default();
            let mut list = Vec::new();
            let mut bad = 0usize;
            for l in text.lines().filter(|l| !l.trim().is_empty()) {
                match self.vault.open(gid, l).and_then(|b| serde_json::from_slice::<Entry>(&b).ok()) {
                    Some(e) => list.push(e),
                    None => bad += 1,
                }
            }
            if bad > 0 {
                eprintln!("[groups-pc] ⚠️  в ленте группы {gid} не открылось строк: {bad} (повреждение на диске)");
                self.damaged.insert(gid.to_string());
            }
            self.logs.insert(gid.to_string(), list);
        }
        self.logs.get_mut(gid).expect("log")
    }

    /// Дописывает расшифрованные записи (строка — запись, под тем же ключом хранилища).
    fn append_log(&mut self, gid: &str, entries: &[Entry]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut text = match std::fs::read_to_string(self.vault.dir.join(Self::log_name(gid))) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("лента группы не читается: {e}")), // не перезаписываем то, что не смогли прочитать
        };
        for e in entries {
            if let Ok(j) = serde_json::to_vec(e) {
                text.push_str(&self.vault.seal(gid, &j));
                text.push('\n');
            }
        }
        self.vault.write_private(&Self::log_name(gid), text.as_bytes()).map_err(|e| format!("лента группы не сохранена: {e}"))?;
        let log = self.load_log(gid);
        for e in entries {
            if !log.iter().any(|x| x.seq == e.seq) {
                log.push(e.clone());
            }
        }
        log.sort_by_key(|e| e.seq);
        Ok(())
    }

    fn group_key(&self, gid: &str, epoch: u32) -> Option<[u8; 32]> {
        self.state.groups.get(gid)?.keys.get(&epoch).and_then(|k| unb64(k)).and_then(|k| arr32(&k))
    }

    fn seed(&self, gid: &str) -> Option<[u8; 32]> {
        self.state.groups.get(gid).and_then(|g| unb64(&g.seed)).and_then(|s| arr32(&s))
    }
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

fn u(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

// ── синхронизация ──

/// Список групп, ключи, забор журнала. Идёт в самом узле, без сети; вызывается по таймеру и по намёку группы.
pub fn sync_all() {
    let _ = with_client(|c| {
        let dev = pc_device()?;
        let list: Vec<Value> = with_store(|st| st.list_for(&dev));
        // «потерянные» группы: были в списке, теперь нет
        let listed: HashSet<String> = list.iter().map(|g| s(g, "id")).collect();
        let mut changed = false;
        for g in &list {
            let (id, name) = (s(g, "id"), s(g, "name"));
            if c.state.known.get(&id) != Some(&name) {
                c.state.known.insert(id, name);
                changed = true;
            }
        }
        if changed {
            let _ = c.save();
        }
        let card = pc_card();
        for g in &list {
            let gid = s(g, "id");
            let _ = sync_group(c, &dev, g, &gid, &card);
        }
        let _ = listed;
        Ok(())
    });
}

/// Визитка ПК из единого профиля узла: (ник, маленькая картинка в base64, отпечаток). Пустой профиль — пустой отпечаток.
/// Картинка пересобирается только когда сменились ник или сами байты картинки (обход групп идёт часто).
fn pc_card() -> (String, String, String) {
    use sha2::Digest;
    static LAST: Mutex<Option<(String, (String, String, String))>> = Mutex::new(None);
    let short = crate::core::profile::UserProfile::load().ok().flatten().map(|p| p.short_id).unwrap_or_default();
    let nick = crate::mobile_profile::snapshot(&short)["nick"].as_str().unwrap_or("").to_string();
    let raw = crate::mobile_profile::avatar_bytes(&short).map(|(b, _)| b);
    let key = format!("{nick}\u{0}{}", raw.as_ref().map(|b| hex::encode(Sha256::digest(b))).unwrap_or_default());
    if let Some((k, card)) = LAST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if *k == key {
            return card.clone();
        }
    }
    let png = raw.and_then(|b| crate::mobile_profile::sanitize_small(&b));
    let avatar = png.map(|b| base64::engine::general_purpose::STANDARD.encode(b)).unwrap_or_default();
    let print = if nick.is_empty() && avatar.is_empty() { String::new() } else { hex::encode(Sha256::digest(format!("{nick}\u{0}{avatar}").as_bytes())) };
    let card = (nick, avatar, print);
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, card.clone()));
    card
}

fn sync_group(c: &mut Client, dev: &str, g: &Value, gid: &str, card: &(String, String, String)) -> Result<(), String> {
    let (role, alias, epoch) = (s(g, "role"), s(g, "my_alias"), g["epoch"].as_u64().unwrap_or(0) as u32);
    // 0) визитка по умолчанию: пока человек сам не скрыл её в этой группе, ник и картинка профиля показываются
    {
        let gs = c.gstate(gid);
        if !gs.card_off && gs.card_sent != card.2 {
            let previous = std::mem::replace(&mut gs.card_sent, card.2.clone());
            if with_store(|st| st.set_card(gid, dev, &card.0, &card.1)).is_ok() {
                let _ = c.save();
            } else {
                c.gstate(gid).card_sent = previous;
            }
        }
    }
    // 0б) подпись «это я»: ключом «компьютера как собеседника» под (группа, мой групповой ключ); один раз
    if !c.gstate(gid).proof_sent && c.gstate(gid).pub_sent {
        use base64::Engine;
        let pubk = base64::engine::general_purpose::STANDARD.decode(&c.gstate(gid).pubk).unwrap_or_default();
        let mut msg = b"YANDI-GROUP-CARD-V1\0".to_vec();
        msg.extend_from_slice(gid.as_bytes());
        msg.push(0);
        msg.extend_from_slice(&pubk);
        if let Some(sig) = crate::mobile_self::sign_identity(&msg) {
            let proof = base64::engine::general_purpose::STANDARD.encode(sig);
            if with_store(|st| st.set_proof(gid, dev, &proof)).is_ok() {
                c.gstate(gid).proof_sent = true;
                let _ = c.save();
            }
        }
    }
    // 1) мой ключ группы опубликован на узле
    if !c.gstate(gid).pub_sent {
        let p = c.gstate(gid).pubk.clone();
        with_store(|st| st.set_pub(gid, dev, &p)).map_err(fail)?;
        c.gstate(gid).pub_sent = true;
        let _ = c.save();
    }
    // 2) ключи: владелец раздаёт, участник забирает
    if role == "owner" {
        let _ = collect_keys(c, dev, gid, true);
        provision(c, dev, g, gid, &alias, epoch)?;
    } else {
        collect_keys(c, dev, gid, false)?;
    }
    // 3) журнал и стирания, пропущенные без сети
    pull(c, dev, gid, g)?;
    let _ = sync_deletions(c, dev, gid);
    // 4) правила и имена
    rebuild_meta(c, dev, gid, &role, &alias);
    // 5) моё имя в группе — первой записью, когда появится ключ
    announce_name(c, dev, gid, &alias);
    Ok(())
}

fn current_epoch(dev: &str, gid: &str) -> u32 {
    with_store(|st| st.list_for(dev)).iter().find(|g| s(g, "id") == gid).map(|g| g["epoch"].as_u64().unwrap_or(0) as u32).unwrap_or(0)
}

fn provision(c: &mut Client, dev: &str, g: &Value, gid: &str, my_alias: &str, epoch: u32) -> Result<(), String> {
    let owner_seed = c.seed(gid).ok_or("нет ключа")?;
    let members = with_store(|st| st.members(gid, dev)).map_err(fail)?;
    let list: Vec<Value> = members["members"].as_array().cloned().unwrap_or_default();
    let others: Vec<&Value> = list.iter().filter(|m| s(m, "alias") != my_alias && m["member_pub"].is_string()).collect();
    let existing = if epoch == 0 { None } else { c.group_key(gid, epoch) };
    let rekey = g["rekey_needed"].as_bool().unwrap_or(false);
    let (new_epoch, key, targets): (u32, [u8; 32], Vec<&Value>) = match (existing, rekey) {
        (Some(k), false) => {
            let t: Vec<&Value> = others.iter().copied().filter(|m| !m["has_key"].as_bool().unwrap_or(false)).collect();
            if t.is_empty() {
                return Ok(());
            }
            (epoch, k, t)
        }
        _ => {
            let mut k = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            (epoch + 1, k, others)
        }
    };
    let mut boxes: HashMap<String, String> = HashMap::new();
    for m in targets {
        let Some(mp) = m["member_pub"].as_str().and_then(unb64).and_then(|p| arr32(&p)) else { continue };
        // негодный ключ одного участника не должен останавливать раздачу остальным
        if let Ok(b) = seal_key(&key, gid, new_epoch, &owner_seed, &mp) {
            boxes.insert(s(m, "alias"), b64(&b));
        }
    }
    c.gstate(gid).keys.insert(new_epoch, b64(&key));
    let _ = c.save();
    with_store(|st| st.put_keys(gid, dev, new_epoch, &boxes)).map_err(fail)?;
    Ok(())
}

/// Следующий ключ владельца по цепочке передач: служебная запись `handover` с ключом из `wanted`, написанная тем, чей ключ закреплён
/// (его member_pub по списку узла совпадает с закреплённым). Сам новый владелец и рядовой участник такую запись не подтвердят.
/// Граница: узел в сговоре с участником может выдать чужую запись за запись прежнего владельца (личных подписей нет) — см. аудит №5.
fn handover_next(log: &[Entry], members: &[(String, String, String)], pinned: &str, wanted: &HashSet<String>) -> Option<String> {
    log.iter().find_map(|e| {
        if e.deleted || e.kind != KIND_SYSTEM || e.body["op"] != "handover" {
            return None;
        }
        let p = e.body["pub"].as_str()?;
        if !wanted.contains(p) || p == pinned {
            return None;
        }
        let author_pub = members.iter().find(|(alias, _, _)| alias == &e.from).map(|(_, _, p)| p.as_str())?;
        (author_pub == pinned).then(|| p.to_string())
    })
}

/// Забрать свои конверты ключа группы. Каждый запечатан ключом того, кто был владельцем в его поколении (узел отдаёт это для каждого
/// конверта). Если запечатавший — не закреплённый владелец, иду по цепочке записей «владение передано» от закреплённого (даже если
/// пропустил несколько передач). Без такой записи ключи не принимаю. `as_owner` — я сам теперь владелец и добираю старые поколения.
fn collect_keys(c: &mut Client, dev: &str, gid: &str, as_owner: bool) -> Result<(), String> {
    let my_seed = c.seed(gid).ok_or("нет ключа")?;
    let k = with_store(|st| st.my_keys(gid, dev)).map_err(fail)?;
    let top_owner = k["owner_pub"].as_str().map(str::to_string);
    let mut pinned = match c.gstate(gid).owner_pub.clone() {
        Some(p) => p,
        None => {
            if as_owner {
                return Ok(());
            }
            let Some(p) = top_owner.clone() else { return Ok(()) };
            c.gstate(gid).owner_pub = Some(p.clone()); // закрепляю при первом получении
            let _ = c.save();
            p
        }
    };
    let mut boxes: Vec<Value> = k["boxes"].as_array().cloned().unwrap_or_default();
    boxes.sort_by_key(|b| u(b, "epoch"));
    let mut problem = false;
    for _round in 0..8 {
        let missing: Vec<&Value> = boxes.iter().filter(|b| c.group_key(gid, u(b, "epoch") as u32).is_none()).collect();
        if missing.is_empty() {
            break;
        }
        let mut opened = false;
        let mut wanted: HashSet<String> = HashSet::new();
        for b in missing {
            let Some(sealer) = b["owner_pub"].as_str().map(str::to_string).or_else(|| top_owner.clone()) else { continue };
            if sealer != pinned {
                wanted.insert(sealer);
                continue;
            }
            let epoch = u(b, "epoch") as u32;
            let opened_key = b["box"].as_str().and_then(unb64).zip(unb64(&pinned).and_then(|p| arr32(&p))).map(|(w, op)| open_key(&w, gid, epoch, &my_seed, &op));
            match opened_key {
                Some(Ok(key)) => {
                    c.gstate(gid).keys.insert(epoch, b64(&key));
                    let _ = c.save();
                    opened = true;
                }
                _ => problem = true, // конверт не от закреплённого владельца или повреждён — не принимаем
            }
        }
        if opened {
            continue; // новые ключи — можно дочитать ленту и найти следующую передачу
        }
        if wanted.is_empty() {
            break;
        }
        if let Some(g) = with_store(|st| st.list_for(dev)).into_iter().find(|g| s(g, "id") == gid) {
            let _ = pull(c, dev, gid, &g);
        }
        let members: Vec<(String, String, String)> = with_store(|st| st.members(gid, dev))
            .ok()
            .and_then(|m| m["members"].as_array().map(|l| l.iter().map(|x| (s(x, "alias"), s(x, "role"), s(x, "member_pub"))).collect()))
            .unwrap_or_default();
        let log = c.load_log(gid).clone();
        match handover_next(&log, &members, &pinned, &wanted) {
            Some(next) => {
                pinned = next.clone();
                c.gstate(gid).owner_pub = Some(next);
                let _ = c.save();
            }
            None => {
                problem = true;
                break;
            }
        }
    }
    if !as_owner {
        if problem {
            c.problems.insert(gid.to_string());
        } else {
            c.problems.remove(gid);
        }
    }
    Ok(())
}

/// Номера стёртых записей во всём журнале узла. Узел отдаёт журнал страницами (не больше 500 записей), поэтому проходим его до конца.
fn deleted_in_log(mut page_after: impl FnMut(u64) -> Result<Value, String>) -> Result<Vec<u64>, String> {
    let (mut out, mut after) = (Vec::new(), 0);
    loop {
        let p = page_after(after)?;
        let page = p["entries"].as_array().cloned().unwrap_or_default();
        let Some(last) = page.iter().filter_map(|e| e["seq"].as_u64()).max().filter(|&l| l > after) else { break };
        out.extend(page.iter().filter(|e| e["deleted"].as_bool().unwrap_or(false)).filter_map(|e| e["seq"].as_u64()));
        after = last;
    }
    Ok(out)
}

/// Догнать стирания модераторов, пропущенные без намёка (по курсору событий стирания на узле).
fn sync_deletions(c: &mut Client, dev: &str, gid: &str) -> Result<bool, String> {
    let cur = c.gstate(gid).del_cursor;
    let r = with_store(|st| st.deletions_after(gid, dev, cur)).map_err(fail)?;
    let mut seqs: Vec<u64> = r["deleted"].as_array().map(|l| l.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
    if r["resync"].as_bool().unwrap_or(false) {
        seqs.extend(deleted_in_log(|after| with_store(|st| st.pull(gid, dev, after, 500)).map_err(fail))?);
    }
    let mut changed = false;
    {
        let log = c.load_log(gid);
        for e in log.iter_mut() {
            if seqs.contains(&e.seq) && !e.deleted {
                e.deleted = true;
                e.body = json!({});
                changed = true;
            }
        }
    }
    if changed {
        let snapshot = c.load_log(gid).clone();
        let text: String = snapshot.iter().filter_map(|e| serde_json::to_vec(e).ok()).map(|j| c.vault.seal(gid, &j) + "\n").collect();
        c.vault.write_private(&Client::log_name(gid), text.as_bytes()).map_err(|e| format!("пометки стираний не сохранены: {e}"))?;
    }
    c.gstate(gid).del_cursor = r["cursor"].as_u64().unwrap_or(cur);
    let _ = c.save();
    Ok(changed)
}

fn pull(c: &mut Client, dev: &str, gid: &str, g: &Value) -> Result<(), String> {
    let mut after = c.gstate(gid).last_seq;
    let first = u(g, "first_seq");
    if first > after + 1 {
        after = first - 1; // старое на узле уже стёрто
    }
    loop {
        let p = with_store(|st| st.pull(gid, dev, after, 200)).map_err(fail)?;
        let entries: Vec<Value> = p["entries"].as_array().cloned().unwrap_or_default();
        if entries.is_empty() {
            break;
        }
        let mut batch = Vec::new();
        let mut blocked = false;
        let mut first_epoch: Option<u64> = None;
        for e in &entries {
            let (seq, ts, epoch, from) = (u(e, "seq"), u(e, "ts"), u(e, "epoch") as u32, s(e, "from"));
            let payload = e["payload_b64"].as_str().unwrap_or("");
            if e["deleted"].as_bool().unwrap_or(false) || payload.is_empty() {
                batch.push(Entry { seq, ts, epoch, kind: KIND_SYSTEM, from, body: json!({}), deleted: true });
            } else {
                let Some(key) = c.group_key(gid, epoch) else {
                    // поколение старше всех, что хранит узел, — его конверт вытеснен навсегда: запись помечаем недоступной и читаем дальше
                    let first = *first_epoch.get_or_insert_with(|| with_store(|st| st.my_keys(gid, dev)).ok().and_then(|k| k["first_epoch"].as_u64()).unwrap_or(0));
                    if (epoch as u64) < first {
                        batch.push(Entry { seq, ts, epoch, kind: KIND_SYSTEM, from, body: json!({"op": "lost"}), deleted: false });
                        after = seq;
                        continue;
                    }
                    blocked = true; // ключа ещё нет — придёт позже, ничего не теряем
                    break;
                };
                let decoded = unb64(payload).ok_or("плохой base64").and_then(|raw| decrypt_entry(&key, &raw, gid, epoch, &from).map_err(|_| "не прошла проверка"));
                let mut entry = match decoded {
                    Ok((kind, body)) => Entry { seq, ts, epoch, kind, from, body, deleted: false },
                    Err(_) => Entry { seq, ts, epoch, kind: KIND_SYSTEM, from, body: json!({"op": "bad"}), deleted: false },
                };
                // повтор уже полученной записи (тот же шифртекст под новым номером) — не показываем и не обрабатываем
                if let Some(id) = entry.body.get("i").and_then(|v| v.as_str()).map(str::to_string) {
                    let seen = batch.iter().any(|x: &Entry| x.body.get("i").and_then(|v| v.as_str()) == Some(id.as_str()))
                        || c.load_log(gid).iter().any(|x| x.body.get("i").and_then(|v| v.as_str()) == Some(id.as_str()));
                    if seen {
                        entry = Entry { kind: KIND_SYSTEM, body: json!({"op": "dup"}), ..entry };
                    }
                }
                batch.push(entry);
            }
            after = seq;
        }
        // курсор двигаем, только если записи надёжно легли на диск: иначе после перезапуска они были бы пропущены навсегда
        c.append_log(gid, &batch)?;
        c.gstate(gid).last_seq = after;
        if !c.save() {
            return Err("состояние групп не сохранено".into());
        }
        if blocked || entries.len() < 200 {
            break;
        }
    }
    Ok(())
}

fn rebuild_meta(c: &mut Client, dev: &str, gid: &str, role: &str, my_alias: &str) {
    // правила группы принимаются от владельца и админов (по их псевдонимам в текущем составе)
    let managers: HashSet<String> = with_store(|st| st.members(gid, dev))
        .ok()
        .and_then(|m| m["members"].as_array().map(|l| l.iter().filter(|x| matches!(s(x, "role").as_str(), "owner" | "admin")).map(|x| s(x, "alias")).collect()))
        .unwrap_or_default();
    let mut managers = managers;
    if role == "owner" || role == "admin" {
        managers.insert(my_alias.to_string());
    }
    let log = c.load_log(gid).clone();
    let mut names: HashMap<String, String> = HashMap::new();
    let mut rules: Option<Vec<u8>> = None;
    for e in log.iter().filter(|e| e.kind == KIND_SYSTEM && !e.deleted) {
        match e.body["op"].as_str() {
            Some("name") => {
                if let Some(n) = e.body["n"].as_str() {
                    names.insert(e.from.clone(), n.trim().to_string());
                }
            }
            Some("rules") if managers.contains(&e.from) => {
                rules = Some(e.body["deny"].as_array().map(|a| a.iter().filter_map(|k| k.as_u64()).map(|k| k as u8).collect()).unwrap_or_default());
            }
            _ => {}
        }
    }
    c.names.insert(gid.to_string(), names);
    if let Some(r) = rules {
        c.rules.insert(gid.to_string(), r);
    }
}

fn announce_name(c: &mut Client, dev: &str, gid: &str, alias: &str) {
    let Some(want) = c.gstate(gid).want_name.clone() else { return };
    let epoch = current_epoch(dev, gid);
    let Some(key) = c.group_key(gid, epoch) else { return };
    if let Ok(payload) = encrypt_entry(&key, gid, epoch, alias, KIND_SYSTEM, &json!({"op": "name", "n": want})) {
        if with_store(|st| st.append(gid, dev, epoch, &b64(&payload))).is_ok() {
            c.gstate(gid).want_name = None;
            let _ = c.save();
        }
    }
}

/// Запись «изменена» (например, стёрта модератором): перечитать именно её.
pub fn refresh_entry(gid: &str, seq: u64) {
    let _ = with_client(|c| {
        let dev = pc_device()?;
        let p = with_store(|st| st.pull(gid, &dev, seq.saturating_sub(1), 1)).map_err(fail)?;
        for e in p["entries"].as_array().cloned().unwrap_or_default() {
            if u(&e, "seq") == seq && (e["deleted"].as_bool().unwrap_or(false) || e["payload_b64"].as_str().unwrap_or("").is_empty()) {
                let log = c.load_log(gid);
                if let Some(x) = log.iter_mut().find(|x| x.seq == seq) {
                    x.deleted = true;
                    x.body = json!({});
                }
                let snapshot = log.clone();
                let text: String = snapshot.iter().filter_map(|e| serde_json::to_vec(e).ok()).map(|j| c.vault.seal(gid, &j) + "\n").collect();
                if let Err(e) = c.vault.write_private(&Client::log_name(gid), text.as_bytes()) {
                    eprintln!("[groups-pc] ⚠️  пометка стёртой записи не сохранена: {e}");
                }
            }
        }
        Ok(())
    });
}

static NUDGE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Подтолкнуть синхронизацию групп сейчас (например, профиль изменился: визитка в группах должна обновиться сразу, а не по таймеру).
pub fn nudge() {
    NUDGE.notify_one();
}

/// Фоновая задача: синхронизация по таймеру и по намёкам групп.
pub fn spawn_background() {
    crate::supervisor::supervise("groups-pc-sync", crate::supervisor::Policy::restart(), || async move {
        let mut rx = crate::mobile_groups::subscribe();
        let dev_wait = std::time::Duration::from_secs(2);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(12)) => {
                    let _ = tokio::task::spawn_blocking(sync_all).await;
                }
                _ = NUDGE.notified() => {
                    let _ = tokio::task::spawn_blocking(sync_all).await;
                }
                h = rx.recv() => {
                    match h {
                        Ok(h) => {
                            let Ok(dev) = pc_device() else { tokio::time::sleep(dev_wait).await; continue };
                            if h.to.iter().any(|k| k == &crate::mobile_groups::member_key_for(&h.gid, &dev)) {
                                let (gid, seq, kind) = (h.gid.clone(), h.seq, h.kind);
                                let _ = tokio::task::spawn_blocking(move || {
                                    if kind == crate::mobile_groups::HINT_LOG && seq > 0 && last_seq(&gid) >= seq {
                                        refresh_entry(&gid, seq);
                                    } else {
                                        sync_all();
                                    }
                                }).await;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { let _ = tokio::task::spawn_blocking(sync_all).await; }
                        Err(_) => tokio::time::sleep(dev_wait).await,
                    }
                }
            }
        }
    });
}

fn last_seq(gid: &str) -> u64 {
    with_client(|c| Ok(c.state.groups.get(gid).map(|g| g.last_seq).unwrap_or(0))).unwrap_or(0)
}

// ── операции для веб-страницы ──

fn kind_allowed(c: &Client, gid: &str, kind: u8) -> bool {
    kind == KIND_SYSTEM || !c.rules.get(gid).map(|d| d.contains(&kind)).unwrap_or(false)
}

fn list_value(c: &mut Client, dev: &str) -> Value {
    let list: Vec<Value> = with_store(|st| st.list_for(dev));
    let listed: HashSet<String> = list.iter().map(|g| s(g, "id")).collect();
    let groups: Vec<Value> = list
        .iter()
        .map(|g| {
            let gid = s(g, "id");
            let epoch = g["epoch"].as_u64().unwrap_or(0) as u32;
            let mut v = g.clone();
            v["has_key"] = json!(c.group_key(&gid, epoch).is_some());
            v["problem"] = json!(c.problems.contains(&gid));
            v["deny"] = json!(c.rules.get(&gid).cloned().unwrap_or_default());
            v
        })
        .collect();
    let gone: Vec<Value> = c.state.known.iter().filter(|(id, _)| !listed.contains(*id)).map(|(id, n)| json!({"id": id, "name": n})).collect();
    let live = crate::mobile_api::live_owner_peers();
    let catalog: Vec<Value> = with_store(|st| st.open_for_live(dev, &live));
    json!({"groups": groups, "gone": gone, "catalog": catalog})
}

pub fn list() -> Result<Value, String> {
    let dev = pc_device()?;
    with_client(|c| Ok(list_value(c, &dev)))
}

/// Создать группу: владельцем становится компьютер. kind: `invite` | `open`.
pub fn create(name: &str, topic: &str, about: &str, kind: &str) -> Result<String, String> {
    let dev = pc_device()?;
    let kind = match kind {
        "open" => crate::mobile_groups::Kind::Open,
        "invite" => crate::mobile_groups::Kind::Invite,
        _ => return Err("тип группы: открытая или по приглашению".into()),
    };
    let gid = with_store(|st| {
        if st.owned_count(&dev) >= 10 {
            return Err(crate::mobile_groups::Fail(axum::http::StatusCode::TOO_MANY_REQUESTS, "вы уже ведёте максимум групп"));
        }
        let id = st.create_as(name, topic, kind, &dev, &[], true)?;
        st.set_about(&id, about);
        Ok(id)
    })
    .map_err(fail)?;
    sync_all(); // создаёт первое поколение ключа
    Ok(gid)
}

/// Вступить: открытая группа или по коду. Имя, которое человек называет, пишется в журнал зашифрованной записью, когда придёт ключ.
pub fn join(gid: &str, code: Option<&str>, name: &str) -> Result<(), String> {
    let dev = pc_device()?;
    with_client(|c| {
        let p = c.gstate(gid).pubk.clone();
        with_store(|st| st.join(gid, &dev, code, Some(&p))).map_err(fail)?;
        let g = c.gstate(gid);
        g.pub_sent = true;
        g.want_name = Some(name.trim().to_string());
        let _ = c.save();
        Ok(())
    })?;
    sync_all();
    Ok(())
}

pub fn request_invite(gid: &str, name: &str) -> Result<(), String> {
    let dev = pc_device()?;
    with_store(|st| st.request_join(gid, &dev, name)).map_err(fail)
}

fn ui_body(body: &Value) -> Value {
    let mut public = body.clone();
    // File decryption belongs to the daemon. Neither browser nor desktop needs
    // the attachment key; download_file reads the original internal log body.
    if let Some(object) = public.as_object_mut() {
        object.remove("k");
    }
    public
}

/// Монетки: запись → (баланс, мой голос). Один голос на человека (последний по порядку), голос за свою запись и за несуществующую
/// не считается.
fn tally_votes(log: &[Entry], my: &str) -> HashMap<u64, (i64, i64)> {
    let author: HashMap<u64, &str> = log.iter().filter(|e| e.kind != KIND_REACTION).map(|e| (e.seq, e.from.as_str())).collect();
    let mut latest: HashMap<u64, HashMap<&str, i64>> = HashMap::new();
    for e in log.iter().filter(|e| e.kind == KIND_REACTION && !e.deleted) {
        let (Some(t), Some(v)) = (e.body["s"].as_u64(), e.body["v"].as_i64()) else { continue };
        if !(-1..=1).contains(&v) || author.get(&t).map_or(true, |a| *a == e.from) {
            continue;
        }
        latest.entry(t).or_default().insert(e.from.as_str(), v);
    }
    latest.into_iter().map(|(t, m)| (t, (m.values().sum(), m.get(my).copied().unwrap_or(0)))).collect()
}

/// Проголосовать за запись: 1 — одобряю, -1 — не одобряю; тот же голос ещё раз снимает его.
pub fn vote(gid: &str, seq: u64, v: i64) -> Result<(), String> {
    if !(-1..=1).contains(&v) || v == 0 {
        return Err("голос — 1 или -1".into());
    }
    let dev = pc_device()?;
    let my = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).map(|g| s(&g, "my_alias")).ok_or("группа недоступна")?;
    let cur = with_client(|c| Ok(tally_votes(c.load_log(gid), &my).get(&seq).map(|x| x.1).unwrap_or(0)))?;
    send(gid, KIND_REACTION, json!({"s": seq, "v": if cur == v { 0 } else { v }})).map(|_| ())
}

/// Лента группы: видимые записи (стёртые, служебные и запрещённые правилами не показываются) и сведения о группе.
pub fn messages(gid: &str, after: u64) -> Result<Value, String> {
    let dev = pc_device()?;
    with_client(|c| {
        let info = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
        let my = s(&info, "my_alias");
        let mut names = c.names.get(gid).cloned().unwrap_or_default();
        names.extend(card_nicks(gid, &dev)); // раскрытый ник важнее имени «при вступлении»
        let deny = c.rules.get(gid).cloned().unwrap_or_default();
        let log = c.load_log(gid).clone();
        let tally = tally_votes(&log, &my);
        let items: Vec<Value> = log
            .iter()
            .filter(|e| e.seq > after && !e.deleted && e.kind != KIND_SYSTEM && e.kind != KIND_REACTION && !deny.contains(&e.kind))
            .map(|e| {
                let (coins, mine_vote) = tally.get(&e.seq).map(|(c, m)| (json!(c), json!(m))).unwrap_or((Value::Null, json!(0)));
                json!({"seq": e.seq, "ts": e.ts, "from": e.from, "author": names.get(&e.from).cloned().unwrap_or_else(|| e.from.clone()), "mine": e.from == my, "kind": e.kind, "body": ui_body(&e.body), "coins": coins, "my_vote": mine_vote})
            })
            .collect();
        let epoch = info["epoch"].as_u64().unwrap_or(0) as u32;
        Ok(json!({"group": info, "has_key": c.group_key(gid, epoch).is_some(), "problem": c.problems.contains(gid), "damaged": c.damaged.contains(gid), "deny": deny, "messages": items}))
    })
}

/// Отправить запись от компьютера. Запрещённый правилами группы вид отвергается здесь же (как в приложении).
pub fn send(gid: &str, kind: u8, body: Value) -> Result<u64, String> {
    let dev = pc_device()?;
    with_client(|c| {
        let info = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(&g, "id") == gid).ok_or("группа недоступна")?;
        if info["muted"].as_bool().unwrap_or(false) {
            return Err("вы заглушены в этой группе".into());
        }
        if !KINDS.contains(&kind) {
            return Err("неверный вид записи".into());
        }
        if kind == KIND_LINK {
            return Err("ссылки в YANDI не передаются".into());
        }
        // политика содержимого: ни в тексте, ни в подписи ссылок, а у вложений — расширение из белого списка
        for field in ["t", "caption", "text"] {
            if let Some(t) = body.get(field).and_then(|v| v.as_str()) {
                crate::content_policy::check_text(t)?;
            }
        }
        if matches!(kind, KIND_IMAGE | KIND_FILE | KIND_AUDIO | KIND_VIDEO) {
            for field in ["name", "filename", "file_name"] {
                if let Some(n) = body.get(field).and_then(|v| v.as_str()) {
                    crate::content_policy::attachment_allowed(n)?;
                }
            }
        }
        if !kind_allowed(c, gid, kind) {
            return Err("в этой группе запрещён такой вид сообщений".into());
        }
        let epoch = info["epoch"].as_u64().unwrap_or(0) as u32;
        let key = c.group_key(gid, epoch).ok_or("ключ группы ещё не получен — подождите владельца")?;
        let payload = encrypt_entry(&key, gid, epoch, &s(&info, "my_alias"), kind, &body)?;
        with_store(|st| st.append(gid, &dev, epoch, &b64(&payload))).map_err(fail)
    })
}

/// Владелец: запретить виды записей (служебная запись видна только участникам).
pub fn set_rules(gid: &str, deny: &[u8]) -> Result<(), String> {
    let dev = pc_device()?;
    with_client(|c| {
        let info = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
        if !matches!(s(&info, "role").as_str(), "owner" | "admin") {
            return Err("правила задают владелец и админ группы".into());
        }
        let epoch = info["epoch"].as_u64().unwrap_or(0) as u32;
        let key = c.group_key(gid, epoch).ok_or("ключ группы не создан")?;
        let mut d: Vec<u8> = deny.iter().copied().filter(|k| KINDS.contains(k) && *k != KIND_SYSTEM && *k != KIND_TEXT).collect();
        d.sort_unstable();
        d.dedup();
        let payload = encrypt_entry(&key, gid, epoch, &s(&info, "my_alias"), KIND_SYSTEM, &json!({"op": "rules", "deny": d}))?;
        with_store(|st| st.append(gid, &dev, epoch, &b64(&payload))).map_err(fail)?;
        c.rules.insert(gid.to_string(), d);
        Ok(())
    })
}

pub fn leave(gid: &str) -> Result<(), String> {
    let dev = pc_device()?;
    with_store(|st| st.leave(gid, &dev)).map_err(fail)?;
    with_client(|c| {
        c.state.groups.remove(gid);
        c.state.known.remove(gid);
        c.logs.remove(gid);
        let _ = std::fs::remove_file(c.vault.dir.join(Client::log_name(gid)));
        let _ = c.save();
        Ok(())
    })
}

/// Убрать локальные данные группы, которой на узле больше нет.
pub fn forget_local(gid: &str) -> Result<(), String> {
    with_client(|c| {
        c.state.groups.remove(gid);
        c.state.known.remove(gid);
        c.logs.remove(gid);
        let _ = std::fs::remove_file(c.vault.dir.join(Client::log_name(gid)));
        let _ = c.save();
        Ok(())
    })
}

/// Владелец передаёт группу участнику: сначала в ленту кладётся служебная запись «владение передано» (с ключом нового владельца), затем узел
/// меняет роли. Прежний владелец становится админом; новый создаст новое поколение ключа.
pub fn transfer_owner(gid: &str, alias: &str) -> Result<(), String> {
    let dev = pc_device()?;
    with_client(|c| {
        let info = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
        if s(&info, "role") != "owner" {
            return Err("передать группу может только её владелец".into());
        }
        let members = with_store(|st| st.members(gid, &dev)).map_err(fail)?;
        let target = members["members"].as_array().and_then(|l| l.iter().find(|x| s(x, "alias") == alias).cloned()).ok_or("нет такого участника")?;
        let pubk = target["member_pub"].as_str().ok_or("у участника ещё нет ключа группы (он должен открыть приложение)")?.to_string();
        let epoch = info["epoch"].as_u64().unwrap_or(0) as u32;
        let key = c.group_key(gid, epoch).ok_or("ключ группы не создан")?;
        let payload = encrypt_entry(&key, gid, epoch, &s(&info, "my_alias"), KIND_SYSTEM, &json!({"op": "handover", "to": alias, "pub": pubk}))?;
        with_store(|st| st.append(gid, &dev, epoch, &b64(&payload))).map_err(fail)?;
        with_store(|st| st.transfer_owner(gid, &dev, alias)).map_err(fail)
    })?;
    sync_all();
    Ok(())
}

/// Владелец меняет название, тематику и описание своей группы.
pub fn update_settings(gid: &str, name: Option<&str>, topic: Option<&str>, about: Option<&str>) -> Result<(), String> {
    let dev = pc_device()?;
    with_store(|st| st.update_settings(gid, &dev, name, topic, about)).map_err(fail)
}

/// Владелец включает автоочистку неактивных участников своей группы (0 — выключить).
pub fn set_auto_clean(gid: &str, days: u32) -> Result<u32, String> {
    let dev = pc_device()?;
    with_store(|st| st.set_auto_clean(gid, &dev, days)).map_err(fail)
}

/// Ники, которые участники сами раскрыли в группе: псевдоним → ник.
fn card_nicks(gid: &str, dev: &str) -> HashMap<String, String> {
    let members = with_store(|st| st.members(gid, dev)).ok().and_then(|m| m["members"].as_array().cloned()).unwrap_or_default();
    let mut out: HashMap<String, String> =
        members.iter().filter_map(|x| x["nick"].as_str().filter(|n| !n.is_empty()).map(|n| (s(x, "alias"), n.to_string()))).collect();
    out.extend(recognised(gid, &members)); // имя моего контакта сильнее ника
    out
}

/// Подпись «это я» в визитке участника: проверяется ключами сопряжённых устройств; чужой ключ её не проверит.
/// Сообщение — то же, что подписывает телефон: «YANDI-GROUP-CARD-V1\0» + группа + 0 + групповой ключ участника.
fn recognised(gid: &str, members: &[Value]) -> HashMap<String, String> {
    use base64::Engine;
    use ed25519_dalek::Verifier;
    let keys: Vec<(ed25519_dalek::VerifyingKey, String)> = crate::mobile_api::device_ed_keys()
        .into_iter()
        .filter_map(|(raw, name)| Some((ed25519_dalek::VerifyingKey::from_bytes(&raw).ok()?, format!("📱 {name}"))))
        .collect();
    let mut found = HashMap::new();
    if keys.is_empty() {
        return found;
    }
    for x in members {
        let (Some(proof), Some(pubb)) = (x["proof"].as_str(), x["member_pub"].as_str()) else { continue };
        let (Ok(sig), Ok(pubk)) = (base64::engine::general_purpose::STANDARD.decode(proof), base64::engine::general_purpose::STANDARD.decode(pubb)) else { continue };
        let Ok(sig) = <[u8; 64]>::try_from(sig.as_slice()) else { continue };
        let sig = ed25519_dalek::Signature::from_bytes(&sig);
        let mut msg = b"YANDI-GROUP-CARD-V1\0".to_vec();
        msg.extend_from_slice(gid.as_bytes());
        msg.push(0);
        msg.extend_from_slice(&pubk);
        if let Some((_, name)) = keys.iter().find(|(k, _)| k.verify(&msg, &sig).is_ok()) {
            found.insert(s(x, "alias"), name.clone());
        }
    }
    found
}

/// Раскрыть (или спрятать) ник и аватар компьютера в группе — из единого профиля узла.
pub fn share_card(gid: &str, on: bool) -> Result<(), String> {
    let dev = pc_device()?;
    let card = pc_card();
    let (nick, avatar) = if on { (card.0.clone(), card.1.clone()) } else { (String::new(), String::new()) };
    with_store(|st| st.set_card(gid, &dev, &nick, &avatar)).map_err(fail)?;
    // явный выбор человека запоминается: «скрыть» не отменяется автоматически, «показать» снова следует за профилем
    with_client(|c| {
        let gs = c.gstate(gid);
        gs.card_off = !on;
        gs.card_sent = if on { card.2.clone() } else { String::new() };
        c.save();
        Ok(())
    })
}

/// Картинка визитки участника (PNG) по псевдониму; нет визитки или картинки — `None`.
pub fn member_avatar(gid: &str, alias: &str) -> Result<Option<Vec<u8>>, String> {
    let dev = pc_device()?;
    let b64 = with_store(|st| st.card_avatar(gid, &dev, alias)).map_err(fail)?;
    Ok(b64.and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()))
}

/// Участники группы с отображаемыми именами (кто как назвался) и просьбы вступить (для владельца и модераторов).
pub fn info(gid: &str) -> Result<Value, String> {
    let dev = pc_device()?;
    with_client(|c| {
        let g = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
        let m = with_store(|st| st.members(gid, &dev)).map_err(fail)?;
        let names = c.names.get(gid).cloned().unwrap_or_default();
        let nicks = card_nicks(gid, &dev);
        let members: Vec<Value> = m["members"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|x| json!({"alias": s(x, "alias"), "nick": nicks.get(&s(x, "alias")).cloned().unwrap_or_default(), "name": names.get(&s(x, "alias")).cloned().unwrap_or_default(), "role": s(x, "role"), "muted": x["muted"], "has_key": x["has_key"]}))
            .collect();
        let staff = s(&g, "role") != "member";
        let requests = if staff && s(&g, "kind") == "invite" {
            let skip: HashSet<String> = c.state.groups.get(gid).map(|g| g.ignored.iter().cloned().collect()).unwrap_or_default();
            let all = with_store(|st| st.requests_of(gid, &dev)).map(|v| v["requests"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
            let mut keep = Vec::new();
            for r in all {
                if skip.contains(&s(&r, "peer_id")) {
                    let rid = s(&r, "id");
                    let _ = with_store(|st| st.resolve_request(gid, &dev, &rid)); // «больше не напоминать»: тихо закрываем
                } else {
                    keep.push(r);
                }
            }
            keep
        } else {
            Vec::new()
        };
        let modlog = with_store(|st| st.modlog(gid, Some(&dev))).map(|v| v["modlog"].clone()).unwrap_or(Value::Null);
        let banned = if staff { m["banned"].clone() } else { Value::Null };
        let deny = c.rules.get(gid).cloned().unwrap_or_default();
        Ok(json!({"group": g, "members": members, "requests": requests, "modlog": modlog, "banned": banned, "deny": deny}))
    })
}

/// Действие модератора/владельца (заглушить, выгнать, запретить, назначить…) или стирание записи.
pub fn moderate(gid: &str, action: &str, alias: Option<&str>, seq: Option<u64>) -> Result<(), String> {
    let dev = pc_device()?;
    with_store(|st| st.moderate(gid, &dev, action, alias, seq, None)).map_err(fail)?;
    if action == "delete_entry" {
        if let Some(q) = seq {
            let _ = with_client(|c| {
                let log = c.load_log(gid);
                if let Some(x) = log.iter_mut().find(|x| x.seq == q) {
                    x.deleted = true;
                    x.body = json!({});
                }
                Ok(())
            });
            refresh_entry(gid, q);
        }
    }
    Ok(())
}

/// Одноразовый код приглашения (группа «по приглашению»).
pub fn invite_code(gid: &str) -> Result<Value, String> {
    let dev = pc_device()?;
    with_store(|st| st.invite(gid, &dev, 1, Some(24 * 3_600_000))).map_err(fail)
}

fn invite_card(gid: &str, code: &str, name: &str) -> String {
    format!("\u{1}yandi-gi:{}", json!({"g": gid, "c": code, "n": name}))
}

/// Отправить приглашение телефону (личным зашифрованным сообщением с карточкой). Для открытой группы кода нет.
pub async fn invite_device(gid: &str, peer: &str) -> Result<(), String> {
    let dev = pc_device()?;
    let g = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
    if s(&g, "role") == "member" {
        return Err("приглашать могут владелец и модераторы".into());
    }
    let kind = s(&g, "kind");
    let code = match kind.as_str() {
        "open" => String::new(),
        _ => invite_code(gid)?["code"].as_str().unwrap_or("").to_string(),
    };
    crate::mobile_api::pc_send_to_device(peer, invite_card(gid, &code, &s(&g, "name"))).await.map(|_| ())
}

/// Ответ на просьбу вступить: accept — приглашение уходит просившему личным сообщением; never — ещё и «больше не напоминать».
pub async fn answer_request(gid: &str, rid: &str, accept: bool, never: bool) -> Result<(), String> {
    let dev = pc_device()?;
    let all = with_store(|st| st.requests_of(gid, &dev)).map_err(fail)?;
    let r = all["requests"].as_array().and_then(|l| l.iter().find(|x| s(x, "id") == rid).cloned()).ok_or("просьбы уже нет")?;
    let peer = s(&r, "peer_id");
    if accept {
        invite_device(gid, &peer).await?;
    }
    if never {
        with_client(|c| {
            let g = c.gstate(gid);
            if !g.ignored.contains(&peer) {
                g.ignored.push(peer.clone());
            }
            let _ = c.save();
            Ok(())
        })?;
    }
    with_store(|st| st.resolve_request(gid, &dev, rid)).map_err(fail)
}

/// Телефоны этого узла (кому можно отправить приглашение).
pub fn devices() -> Value {
    json!(crate::mobile_api::device_list().iter().map(|(p, n)| json!({"id": p, "name": n})).collect::<Vec<_>>())
}

// ── Файлы групп (скрепка на странице узла) ──
// Формат тот же, что в приложении (`file_crypto.dart`, `group_files.dart`): куски по 256 КБ, свой ключ на файл,
// кусок = [12 случайное число][шифртекст][16 метка], AAD = `yandi-file:v1|<номер файла>|<номер куска>|<всего кусков>`;
// запись журнала несёт `{f, n, z, c, k, h}` (номер на узле, имя, размер, кусков, ключ, sha256).
const FILE_CHUNK: usize = 256 * 1024;

fn file_aad(fid: &str, idx: u32, total: u32) -> Vec<u8> {
    format!("yandi-file:v1|{fid}|{idx}|{total}").into_bytes()
}

/// Вид записи по имени файла: так работают разделы «Картинки / Аудио / Видео / Файлы» (как в приложении).
pub fn kind_for_name(name: &str) -> u8 {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic" => KIND_IMAGE,
        "mp3" | "ogg" | "oga" | "opus" | "wav" | "flac" | "m4a" | "aac" => KIND_AUDIO,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "3gp" => KIND_VIDEO,
        _ => KIND_FILE,
    }
}

/// Залить файл в группу от компьютера и написать о нём запись. Запрещённый правилами вид отвергается здесь же.
pub async fn upload_file(gid: &str, name: &str, data: Vec<u8>) -> Result<u64, String> {
    use sha2::Digest;
    let dev = pc_device()?;
    let name: String = name.chars().filter(|c| !c.is_control() && *c != '/' && *c != '\\').take(120).collect();
    let name = if name.trim().is_empty() { "file".to_string() } else { name };
    // политика содержимого: разрешённое расширение и содержимое, соответствующее ему (исполняемое под любым именем отклоняется)
    crate::content_policy::sniff_ok(&name, &data[..data.len().min(64)])?;
    let kind = kind_for_name(&name);
    // проверки до заливки: участник, не заглушён, вид разрешён, ключ есть
    with_client(|c| {
        let info = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
        if info["muted"].as_bool().unwrap_or(false) {
            return Err("вы заглушены в этой группе".to_string());
        }
        if !kind_allowed(c, gid, kind) {
            return Err("в этой группе запрещён такой вид файлов".to_string());
        }
        let epoch = info["epoch"].as_u64().unwrap_or(0) as u32;
        c.group_key(gid, epoch).ok_or("ключ группы ещё не получен — подождите владельца")?;
        Ok(())
    })?;
    if data.is_empty() {
        return Err("файл пустой".into());
    }
    let size = data.len() as u64;
    let total = data.len().div_ceil(FILE_CHUNK) as u32;
    let fid = crate::mobile_group_files::pc_begin(gid, &dev, size, total).await?;
    let mut fkey = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut fkey);
    for (i, part) in data.chunks(FILE_CHUNK).enumerate() {
        let wire = aead_seal(&fkey, &file_aad(&fid, i as u32, total), part);
        if let Err(e) = crate::mobile_group_files::pc_put(gid, &dev, &fid, i as u32, &wire).await {
            let _ = crate::mobile_group_files::pc_remove(gid, &dev, &fid).await;
            return Err(e);
        }
    }
    crate::mobile_group_files::pc_done(gid, &dev, &fid).await?;
    let h = hex::encode(sha2::Sha256::digest(&data));
    let body = json!({"f": fid, "n": name, "z": size, "c": total, "k": b64(&fkey), "h": h});
    match send(gid, kind, body) {
        Ok(seq) => Ok(seq),
        Err(e) => {
            let _ = crate::mobile_group_files::pc_remove(gid, &dev, &fid).await;
            Err(e)
        }
    }
}

/// Скачать, расшифровать и проверить файл записи №seq. Подмена, перестановка или обрезка кусков и несовпадение суммы — ошибка.
pub async fn download_file(gid: &str, seq: u64) -> Result<(String, Vec<u8>), String> {
    use sha2::Digest;
    let dev = pc_device()?;
    let body = with_client(|c| {
        let log = c.load_log(gid).clone();
        let deny = c.rules.get(gid).cloned().unwrap_or_default();
        let e = log.iter().find(|e| e.seq == seq && !e.deleted).ok_or("нет такой записи")?;
        if deny.contains(&e.kind) {
            return Err("в этой группе запрещён такой вид файлов".to_string());
        }
        Ok(e.body.clone())
    })?;
    let fid = body["f"].as_str().ok_or("в записи нет файла")?.to_string();
    let name = body["n"].as_str().unwrap_or("file").to_string();
    let size = body["z"].as_u64().unwrap_or(0);
    let total = body["c"].as_u64().unwrap_or(0) as u32;
    let key = body["k"].as_str().and_then(unb64).and_then(|k| arr32(&k)).ok_or("неверный ключ файла")?;
    if total == 0 || size == 0 || size > crate::mobile_group_files::MAX_GROUP_FILE {
        return Err("неверное описание файла".into());
    }
    let mut out = Vec::with_capacity(size as usize);
    for i in 0..total {
        let wire = crate::mobile_group_files::pc_chunk(gid, &dev, &fid, i).await?;
        let part = aead_open(&key, &file_aad(&fid, i, total), &wire).ok_or("кусок файла повреждён или подменён")?;
        out.extend_from_slice(&part);
    }
    if out.len() as u64 != size || body["h"].as_str() != Some(hex::encode(sha2::Sha256::digest(&out)).as_str()) {
        return Err("файл повреждён или подменён".into());
    }
    Ok((name, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coins_count_one_vote_per_person_and_ignore_self_votes() {
        let e = |seq: u64, kind: u8, from: &str, body: Value| Entry { seq, ts: 0, epoch: 1, kind, from: from.into(), body, deleted: false };
        let log = vec![
            e(1, KIND_TEXT, "Автор", json!({"t": "идея"})),
            e(2, KIND_REACTION, "Боб", json!({"s": 1, "v": 1})),
            e(3, KIND_REACTION, "Вера", json!({"s": 1, "v": 1})),
            e(4, KIND_REACTION, "Автор", json!({"s": 1, "v": 1})),   // за себя — не считается
            e(5, KIND_REACTION, "Боб", json!({"s": 1, "v": -1})),   // передумал: заменяет прежний
            e(6, KIND_REACTION, "Вера", json!({"s": 99, "v": 1})),  // такой записи нет
            e(7, KIND_REACTION, "Вера", json!({"s": 1, "v": 7})),   // неверное значение
        ];
        let t = tally_votes(&log, "Боб");
        assert_eq!(t.get(&1), Some(&(0, -1)), "Вера +1, Боб −1");
        assert!(t.get(&99).is_none());
    }

    #[test]
    fn desktop_file_view_does_not_export_decryption_keys_or_mutate_daemon_log() {
        let original = json!({"f": "file-id", "n": "photo.png", "z": 10, "c": 1, "k": "secret-file-key", "h": "hash", "i": "message-id"});
        let view = ui_body(&original);
        assert!(view.get("k").is_none());
        for field in ["f", "n", "z", "c", "h", "i"] { assert_eq!(view[field], original[field]); }
        assert_eq!(original["k"], "secret-file-key", "daemon download retains its key");
        assert_eq!(ui_body(&json!({"t": "hello"})), json!({"t": "hello"}));
    }

    fn vector() -> Value {
        serde_json::from_str(include_str!("../tests/data/group_vector.json")).unwrap()
    }

    fn v32(v: &Value, k: &str) -> [u8; 32] {
        arr32(&unb64(v[k].as_str().unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn dart_vectors_are_readable_here() {
        let v = vector();
        let gid = v["gid"].as_str().unwrap();
        // ключ группы, запечатанный приложением владельца для участника, открывается здесь
        let key = open_key(&unb64(v["box"].as_str().unwrap()).unwrap(), gid, 3, &v32(&v, "member_seed"), &v32(&v, "owner_pub")).unwrap();
        assert_eq!(key, v32(&v, "key"));
        // запись, зашифрованная приложением, читается здесь
        let (kind, body) = decrypt_entry(&key, &unb64(v["entry"].as_str().unwrap()).unwrap(), gid, 3, v["from"].as_str().unwrap()).unwrap();
        assert_eq!(kind, KIND_TEXT);
        assert_eq!(body["t"], v["text"]);
        // открытые ключи по seed совпадают с теми, что посчитало приложение
        assert_eq!(PublicKey::from(&StaticSecret::from(v32(&v, "owner_seed"))).to_bytes(), v32(&v, "owner_pub"));
        assert_eq!(PublicKey::from(&StaticSecret::from(v32(&v, "member_seed"))).to_bytes(), v32(&v, "member_pub"));
    }

    #[test]
    fn what_we_write_is_what_the_phone_would_read_and_tampering_fails() {
        let v = vector();
        let gid = v["gid"].as_str().unwrap();
        let key = v32(&v, "key");
        let boxed = seal_key(&key, gid, 5, &v32(&v, "owner_seed"), &v32(&v, "member_pub")).unwrap();
        assert_eq!(open_key(&boxed, gid, 5, &v32(&v, "member_seed"), &v32(&v, "owner_pub")).unwrap(), key);
        assert!(open_key(&boxed, gid, 6, &v32(&v, "member_seed"), &v32(&v, "owner_pub")).is_err(), "другое поколение");
        assert!(open_key(&boxed, gid, 5, &v32(&v, "owner_seed"), &v32(&v, "owner_pub")).is_err(), "чужой получатель");
        assert!(open_key(&boxed, gid, 5, &v32(&v, "member_seed"), &v32(&v, "member_pub")).is_err(), "подменённый ключ владельца");
        let e = encrypt_entry(&key, gid, 2, "Лис-1", KIND_TEXT, &json!({"t": "hi"})).unwrap();
        assert_eq!(e[0], 1);
        assert_eq!(e[1], KIND_TEXT);
        assert!(decrypt_entry(&key, &e, gid, 2, "Лис-2").is_err(), "автор подменён");
        assert!(decrypt_entry(&key, &e, gid, 3, "Лис-1").is_err(), "поколение");
        let mut swapped = e.clone();
        swapped[1] = KIND_IMAGE;
        assert!(decrypt_entry(&key, &swapped, gid, 2, "Лис-1").is_err(), "вид подменён узлом");
        let mut flipped = e.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert!(decrypt_entry(&key, &flipped, gid, 2, "Лис-1").is_err());
        assert!(encrypt_entry(&key, gid, 2, "x", 99, &json!({})).is_err());
    }

    #[test]
    fn handover_chain_is_followed_only_from_the_pinned_owner() {
        let members = vec![
            ("Старый".to_string(), "admin".to_string(), "OLD".to_string()),
            ("Новый".to_string(), "owner".to_string(), "NEW".to_string()),
            ("Рядовой".to_string(), "member".to_string(), "MEM".to_string()),
        ];
        let h = |from: &str, pubk: &str| Entry { seq: 1, ts: 0, epoch: 1, kind: KIND_SYSTEM, from: from.to_string(), body: json!({"op": "handover", "to": "Новый", "pub": pubk}), deleted: false };
        let want: HashSet<String> = ["NEW".to_string()].into_iter().collect();
        assert_eq!(handover_next(&[h("Старый", "NEW")], &members, "OLD", &want).as_deref(), Some("NEW"), "от закреплённого (прежнего) владельца");
        assert_eq!(handover_next(&[h("Рядовой", "NEW")], &members, "OLD", &want), None, "рядовой не подтверждает");
        assert_eq!(handover_next(&[h("Новый", "NEW")], &members, "OLD", &want), None, "сам новый себя не подтверждает");
        assert_eq!(handover_next(&[h("Старый", "NEW")], &members, "OLD", &HashSet::new()), None, "этот ключ не нужен");
        let mut gone = h("Старый", "NEW");
        gone.deleted = true;
        assert_eq!(handover_next(&[gone], &members, "OLD", &want), None, "стёртая запись не считается");
        // каждая запись несёт свой случайный номер (защита от повторов)
        let key = [7u8; 32];
        let a = decrypt_entry(&key, &encrypt_entry(&key, "g", 1, "x", KIND_TEXT, &json!({"t": "a"})).unwrap(), "g", 1, "x").unwrap().1;
        let b = decrypt_entry(&key, &encrypt_entry(&key, "g", 1, "x", KIND_TEXT, &json!({"t": "a"})).unwrap(), "g", 1, "x").unwrap().1;
        assert!(a["i"].as_str().is_some() && a["i"] != b["i"]);
    }

    #[test]
    fn file_chunks_match_the_app_format() {
        let v: Value = serde_json::from_str(include_str!("../tests/data/group_file_vector.json")).unwrap();
        let key = arr32(&unb64(v["key"].as_str().unwrap()).unwrap()).unwrap();
        let (fid, idx, total) = (v["fid"].as_str().unwrap(), v["idx"].as_u64().unwrap() as u32, v["total"].as_u64().unwrap() as u32);
        let wire = unb64(v["chunk"].as_str().unwrap()).unwrap();
        // кусок, зашифрованный приложением, открывается здесь
        assert_eq!(aead_open(&key, &file_aad(fid, idx, total), &wire).unwrap(), v["plain"].as_str().unwrap().as_bytes());
        // не на своём месте (другой номер, другое число кусков, другой файл) — не открывается
        assert!(aead_open(&key, &file_aad(fid, idx + 1, total), &wire).is_none());
        assert!(aead_open(&key, &file_aad(fid, idx, total + 1), &wire).is_none());
        assert!(aead_open(&key, &file_aad("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd", idx, total), &wire).is_none());
        // свой кусок — того же вида (12 + шифртекст + 16)
        let mine = aead_seal(&key, &file_aad(fid, 0, 1), b"abc");
        assert_eq!(mine.len(), 12 + 3 + 16);
        assert_eq!(kind_for_name("Фото.JPG"), KIND_IMAGE);
        assert_eq!(kind_for_name("song.mp3"), KIND_AUDIO);
        assert_eq!(kind_for_name("clip.mp4"), KIND_VIDEO);
        assert_eq!(kind_for_name("doc.pdf"), KIND_FILE);
        assert_eq!(kind_for_name("noext"), KIND_FILE);
    }

    #[test]
    fn damaged_or_foreign_state_is_not_replaced_with_an_empty_one() {
        // аудит Codex №9: повреждённое состояние или чужой мастер-ключ — ошибка, а не тихий старт с пустого (новые ключи, перезапись)
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::new(dir.path().join("v"), &[1u8; 32]);
        assert!(load_state(&v).unwrap().groups.is_empty(), "файла нет — пустое состояние");
        std::fs::write(v.dir.join("state.enc"), b"garbage").unwrap();
        assert!(load_state(&v).is_err(), "мусор — ошибка");
        assert_eq!(std::fs::read(v.dir.join("state.enc")).unwrap(), b"garbage", "файл не тронут");
        let mut st = State::default();
        st.known.insert("g".into(), "Группа".into());
        let sealed = v.seal("state", &serde_json::to_vec(&st).unwrap());
        std::fs::write(v.dir.join("state.enc"), &sealed).unwrap();
        assert_eq!(load_state(&v).unwrap().known.get("g").map(String::as_str), Some("Группа"));
        let other = Vault::new(dir.path().join("v"), &[2u8; 32]);
        assert!(load_state(&other).is_err(), "чужой мастер-ключ — ошибка, а не пустое состояние");
    }

    #[test]
    fn deletions_are_found_on_every_page_of_the_log() {
        // журнал из 1300 записей, стёрты 3, 777 и 1299; узел отдаёт не больше 500 за раз
        let log: Vec<Value> = (1..=1300u64).map(|n| json!({"seq": n, "deleted": ([3, 777, 1299].contains(&n))})).collect();
        let mut calls = 0;
        let found = deleted_in_log(|after| {
            calls += 1;
            Ok(json!({"entries": log.iter().filter(|e| e["seq"].as_u64().unwrap() > after).take(500).cloned().collect::<Vec<_>>()}))
        })
        .unwrap();
        assert_eq!(found, vec![3, 777, 1299]);
        assert_eq!(calls, 4, "три страницы и пустая");
    }

    #[test]
    fn vault_is_closed_without_the_master_key() {
        let dir = tempfile::tempdir().unwrap();
        let a = Vault::new(dir.path().join("v"), &[1u8; 32]);
        let b = Vault::new(dir.path().join("v"), &[2u8; 32]);
        let sealed = a.seal("gid", b"secret message");
        assert!(!sealed.contains("secret"));
        assert_eq!(a.open("gid", &sealed).unwrap(), b"secret message");
        assert!(b.open("gid", &sealed).is_none(), "чужой мастер-ключ");
        assert!(a.open("other", &sealed).is_none(), "строка привязана к своей группе");
    }

    #[test]
    fn locked_client_refuses_with_a_clear_reason_and_keeps_nothing() {
        lock();
        let err = with_client(|_| Ok(())).unwrap_err();
        assert!(err.contains("заперта"), "{err}");
        assert!(cell().lock().unwrap_or_else(|e| e.into_inner()).is_none());
        LOCKED.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn rekey_moves_state_and_logs_and_is_repeatable_and_refuses_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new, other) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        let v = Vault::new(dir.path().to_path_buf(), &old);
        v.write_private("state.enc", v.seal("state", br#"{"groups":{}}"#).as_bytes()).unwrap();
        let log = format!("{}\n{}\n", v.seal("g1", b"{\"a\":1}"), v.seal("g1", b"{\"a\":2}"));
        v.write_private("g1.log", log.as_bytes()).unwrap();
        // лента, которую этот ключ не открывает: файл остаётся как есть
        let alien = Vault::new(dir.path().to_path_buf(), &other);
        let foreign = format!("{}\n", alien.seal("g2", b"{}"));
        alien.write_private("g2.log", foreign.as_bytes()).unwrap();

        assert_eq!(rekey_dir(dir.path(), Some(old), new).unwrap(), 1);
        let n = Vault::new(dir.path().to_path_buf(), &new);
        assert!(n.open("state", &std::fs::read_to_string(dir.path().join("state.enc")).unwrap()).is_some());
        let moved = std::fs::read_to_string(dir.path().join("g1.log")).unwrap();
        assert_eq!(moved.lines().filter(|l| n.open("g1", l).is_some()).count(), 2);
        assert!(v.open("state", &std::fs::read_to_string(dir.path().join("state.enc")).unwrap()).is_none(), "старый ключ больше не открывает");
        assert_eq!(std::fs::read_to_string(dir.path().join("g2.log")).unwrap(), foreign);
        // повтор ничего не ломает
        assert_eq!(rekey_dir(dir.path(), Some(old), new).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(dir.path().join("g1.log")).unwrap(), moved);
    }
}
