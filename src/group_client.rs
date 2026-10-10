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
use std::path::PathBuf;
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
const KINDS: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];
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

    fn write_private(&self, name: &str, data: &[u8]) {
        let _ = crate::util::private_file::write_private(&self.dir.join(name), data);
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
}

static CLIENT: OnceLock<Mutex<Option<Client>>> = OnceLock::new();

fn cell() -> &'static Mutex<Option<Client>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

/// Запуск: ключ хранилища выводится из мастер-ключа узла. Без мастер-ключа группы на ПК не работают (ничего не пишется открытым).
pub fn init(master: [u8; 32]) {
    let vault = Vault::new(crate::util::data_dir::data_dir().join("groups_pc"), &master);
    let state = std::fs::read_to_string(vault.dir.join("state.enc"))
        .ok()
        .and_then(|t| vault.open("state", &t))
        .and_then(|b| serde_json::from_slice::<State>(&b).ok())
        .unwrap_or_default();
    *cell().lock().unwrap_or_else(|e| e.into_inner()) = Some(Client { vault, state, logs: HashMap::new(), rules: HashMap::new(), names: HashMap::new(), problems: HashSet::new() });
}

fn with_client<T>(f: impl FnOnce(&mut Client) -> Result<T, String>) -> Result<T, String> {
    let mut g = cell().lock().unwrap_or_else(|e| e.into_inner());
    match g.as_mut() {
        Some(c) => f(c),
        None => Err("группы на компьютере недоступны: нет мастер-ключа узла".into()),
    }
}

fn pc_device() -> Result<String, String> {
    crate::mobile_api::node_peer_hex().ok_or_else(|| "узел ещё запускается".to_string())
}

fn fail(e: crate::mobile_groups::Fail) -> String {
    e.1.to_string()
}

impl Client {
    fn save(&self) {
        if let Ok(j) = serde_json::to_vec(&self.state) {
            self.vault.write_private("state.enc", self.vault.seal("state", &j).as_bytes());
        }
    }

    fn gstate(&mut self, gid: &str) -> &mut GState {
        if !self.state.groups.contains_key(gid) {
            let mut seed = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut seed);
            let pubk = PublicKey::from(&StaticSecret::from(seed)).to_bytes();
            self.state.groups.insert(gid.to_string(), GState { seed: b64(&seed), pubk: b64(&pubk), ..Default::default() });
            self.save();
        }
        self.state.groups.get_mut(gid).expect("gstate")
    }

    fn log_name(gid: &str) -> String {
        format!("{gid}.log")
    }

    fn load_log(&mut self, gid: &str) -> &mut Vec<Entry> {
        if !self.logs.contains_key(gid) {
            let list: Vec<Entry> = std::fs::read_to_string(self.vault.dir.join(Self::log_name(gid)))
                .map(|t| t.lines().filter_map(|l| self.vault.open(gid, l)).filter_map(|b| serde_json::from_slice::<Entry>(&b).ok()).collect())
                .unwrap_or_default();
            self.logs.insert(gid.to_string(), list);
        }
        self.logs.get_mut(gid).expect("log")
    }

    /// Дописывает расшифрованные записи (строка — запись, под тем же ключом хранилища).
    fn append_log(&mut self, gid: &str, entries: &[Entry]) {
        if entries.is_empty() {
            return;
        }
        let mut text = std::fs::read_to_string(self.vault.dir.join(Self::log_name(gid))).unwrap_or_default();
        for e in entries {
            if let Ok(j) = serde_json::to_vec(e) {
                text.push_str(&self.vault.seal(gid, &j));
                text.push('\n');
            }
        }
        self.vault.write_private(&Self::log_name(gid), text.as_bytes());
        let log = self.load_log(gid);
        for e in entries {
            if !log.iter().any(|x| x.seq == e.seq) {
                log.push(e.clone());
            }
        }
        log.sort_by_key(|e| e.seq);
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
            c.save();
        }
        for g in &list {
            let gid = s(g, "id");
            let _ = sync_group(c, &dev, g, &gid);
        }
        let _ = listed;
        Ok(())
    });
}

fn sync_group(c: &mut Client, dev: &str, g: &Value, gid: &str) -> Result<(), String> {
    let (role, alias, epoch) = (s(g, "role"), s(g, "my_alias"), g["epoch"].as_u64().unwrap_or(0) as u32);
    // 1) мой ключ группы опубликован на узле
    if !c.gstate(gid).pub_sent {
        let p = c.gstate(gid).pubk.clone();
        with_store(|st| st.set_pub(gid, dev, &p)).map_err(fail)?;
        c.gstate(gid).pub_sent = true;
        c.save();
    }
    // 2) ключи: владелец раздаёт, участник забирает
    if role == "owner" {
        let _ = collect_keys(c, dev, gid, true);
        provision(c, dev, g, gid, &alias, epoch)?;
    } else {
        collect_keys(c, dev, gid, false)?;
    }
    // 3) журнал
    pull(c, dev, gid, g)?;
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
        boxes.insert(s(m, "alias"), b64(&seal_key(&key, gid, new_epoch, &owner_seed, &mp)?));
    }
    c.gstate(gid).keys.insert(new_epoch, b64(&key));
    c.save();
    with_store(|st| st.put_keys(gid, dev, new_epoch, &boxes)).map_err(fail)?;
    Ok(())
}

/// Передача владения подтверждена изнутри группы? Закреплённый ключ владельца меняется только если: новый владелец (по списку узла) называет
/// именно этот ключ, а в ленте есть служебная запись `handover` на его псевдоним с этим ключом от владельца или админа.
/// Запись рядового участника ничего не меняет, а подмена ключа самим узлом остаётся нераспознанной (тот же уровень доверия, что при вступлении).
fn handover_endorsed(log: &[Entry], members: &[(String, String, String)], owner_pub: &str) -> bool {
    let Some((owner_alias, _, _)) = members.iter().find(|(_, role, p)| role == "owner" && p == owner_pub) else { return false };
    log.iter().any(|e| {
        !e.deleted
            && e.kind == KIND_SYSTEM
            && e.body["op"] == "handover"
            && e.body["to"].as_str() == Some(owner_alias.as_str())
            && e.body["pub"].as_str() == Some(owner_pub)
            && &e.from != owner_alias // сам новый владелец себя подтвердить не может
            && members.iter().any(|(alias, role, _)| alias == &e.from && (role == "owner" || role == "admin"))
    })
}

fn collect_keys(c: &mut Client, dev: &str, gid: &str, as_owner: bool) -> Result<(), String> {
    let my_seed = c.seed(gid).ok_or("нет ключа")?;
    let k = with_store(|st| st.my_keys(gid, dev)).map_err(fail)?;
    // владелец тоже забирает недостающие старые поколения: их запечатал прежний владелец, берём его закреплённый ключ
    let owner_pub_s = if as_owner {
        match c.state.groups.get(gid).and_then(|g| g.owner_pub.clone()) {
            Some(p) => p,
            None => return Ok(()),
        }
    } else {
        let Some(p) = k["owner_pub"].as_str().map(|x| x.to_string()) else { return Ok(()) };
        p
    };
    let Some(owner_pub) = unb64(&owner_pub_s).and_then(|p| arr32(&p)) else { return Ok(()) };
    // ключ владельца закрепляется при первом получении: потом узел не подсунет другой
    if !as_owner { match c.gstate(gid).owner_pub.clone() {
        None => {
            c.gstate(gid).owner_pub = Some(owner_pub_s);
            c.save();
        }
        Some(pinned) if pinned != owner_pub_s => {
            // возможно, владение передали: сначала дочитываем ленту (старые ключи у нас есть) и ищем подтверждение изнутри группы
            if let Some(g) = with_store(|st| st.list_for(dev)).into_iter().find(|g| s(g, "id") == gid) {
                let _ = pull(c, dev, gid, &g);
            }
            let members: Vec<(String, String, String)> = with_store(|st| st.members(gid, dev))
                .ok()
                .and_then(|m| m["members"].as_array().map(|l| l.iter().map(|x| (s(x, "alias"), s(x, "role"), s(x, "member_pub"))).collect()))
                .unwrap_or_default();
            let log = c.load_log(gid).clone();
            if handover_endorsed(&log, &members, &owner_pub_s) {
                c.gstate(gid).owner_pub = Some(owner_pub_s.clone());
                c.save();
            } else {
                c.problems.insert(gid.to_string());
                return Ok(());
            }
        }
        _ => {}
    } }
    if !as_owner {
        c.problems.remove(gid);
    }
    for b in k["boxes"].as_array().cloned().unwrap_or_default() {
        let epoch = u(&b, "epoch") as u32;
        if c.group_key(gid, epoch).is_some() {
            continue;
        }
        match b["box"].as_str().and_then(unb64).map(|w| open_key(&w, gid, epoch, &my_seed, &owner_pub)) {
            Some(Ok(key)) => {
                c.gstate(gid).keys.insert(epoch, b64(&key));
                c.save();
            }
            _ => {
                if !as_owner {
                    c.problems.insert(gid.to_string());
                }
            }
        }
    }
    Ok(())
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
        for e in &entries {
            let (seq, ts, epoch, from) = (u(e, "seq"), u(e, "ts"), u(e, "epoch") as u32, s(e, "from"));
            let payload = e["payload_b64"].as_str().unwrap_or("");
            if e["deleted"].as_bool().unwrap_or(false) || payload.is_empty() {
                batch.push(Entry { seq, ts, epoch, kind: KIND_SYSTEM, from, body: json!({}), deleted: true });
            } else {
                let Some(key) = c.group_key(gid, epoch) else {
                    blocked = true; // ключа ещё нет — придёт позже, ничего не теряем
                    break;
                };
                let decoded = unb64(payload).ok_or("плохой base64").and_then(|raw| decrypt_entry(&key, &raw, gid, epoch, &from).map_err(|_| "не прошла проверка"));
                batch.push(match decoded {
                    Ok((kind, body)) => Entry { seq, ts, epoch, kind, from, body, deleted: false },
                    Err(_) => Entry { seq, ts, epoch, kind: KIND_SYSTEM, from, body: json!({"op": "bad"}), deleted: false },
                });
            }
            after = seq;
        }
        c.append_log(gid, &batch);
        c.gstate(gid).last_seq = after;
        c.save();
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
            c.save();
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
                c.vault.write_private(&Client::log_name(gid), text.as_bytes());
            }
        }
        Ok(())
    });
}

/// Фоновая задача: синхронизация по таймеру и по намёкам групп.
pub fn spawn_background() {
    tokio::spawn(async move {
        let mut rx = crate::mobile_groups::subscribe();
        let dev_wait = std::time::Duration::from_secs(2);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(12)) => {
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
    let catalog: Vec<Value> = with_store(|st| st.open_for(dev));
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
        c.save();
        Ok(())
    })?;
    sync_all();
    Ok(())
}

pub fn request_invite(gid: &str, name: &str) -> Result<(), String> {
    let dev = pc_device()?;
    with_store(|st| st.request_join(gid, &dev, name)).map_err(fail)
}

/// Лента группы: видимые записи (стёртые, служебные и запрещённые правилами не показываются) и сведения о группе.
pub fn messages(gid: &str, after: u64) -> Result<Value, String> {
    let dev = pc_device()?;
    with_client(|c| {
        let info = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
        let my = s(&info, "my_alias");
        let names = c.names.get(gid).cloned().unwrap_or_default();
        let deny = c.rules.get(gid).cloned().unwrap_or_default();
        let log = c.load_log(gid).clone();
        let items: Vec<Value> = log
            .iter()
            .filter(|e| e.seq > after && !e.deleted && e.kind != KIND_SYSTEM && !deny.contains(&e.kind))
            .map(|e| json!({"seq": e.seq, "ts": e.ts, "from": e.from, "author": names.get(&e.from).cloned().unwrap_or_else(|| e.from.clone()), "mine": e.from == my, "kind": e.kind, "body": e.body}))
            .collect();
        let epoch = info["epoch"].as_u64().unwrap_or(0) as u32;
        Ok(json!({"group": info, "has_key": c.group_key(gid, epoch).is_some(), "problem": c.problems.contains(gid), "deny": deny, "messages": items}))
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
        c.save();
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
        c.save();
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

/// Участники группы с отображаемыми именами (кто как назвался) и просьбы вступить (для владельца и модераторов).
pub fn info(gid: &str) -> Result<Value, String> {
    let dev = pc_device()?;
    with_client(|c| {
        let g = with_store(|st| st.list_for(&dev)).into_iter().find(|g| s(g, "id") == gid).ok_or("группа недоступна")?;
        let m = with_store(|st| st.members(gid, &dev)).map_err(fail)?;
        let names = c.names.get(gid).cloned().unwrap_or_default();
        let members: Vec<Value> = m["members"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|x| json!({"alias": s(x, "alias"), "name": names.get(&s(x, "alias")).cloned().unwrap_or_default(), "role": s(x, "role"), "muted": x["muted"], "has_key": x["has_key"]}))
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
            c.save();
            Ok(())
        })?;
    }
    with_store(|st| st.resolve_request(gid, &dev, rid)).map_err(fail)
}

/// Телефоны этого узла (кому можно отправить приглашение).
pub fn devices() -> Value {
    json!(crate::mobile_api::device_list().iter().map(|(p, n)| json!({"id": p, "name": n})).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn handover_is_trusted_only_when_endorsed_from_inside_by_a_manager() {
        let new_pub = b64(&[9u8; 32]);
        let members = |new_role: &str| vec![
            ("Старый".to_string(), "admin".to_string(), b64(&[1u8; 32])),
            ("Новый".to_string(), new_role.to_string(), new_pub.clone()),
            ("Рядовой".to_string(), "member".to_string(), b64(&[3u8; 32])),
        ];
        let entry = |from: &str, to: &str, pubk: &str| Entry { seq: 1, ts: 0, epoch: 1, kind: KIND_SYSTEM, from: from.to_string(), body: json!({"op": "handover", "to": to, "pub": pubk}), deleted: false };
        assert!(handover_endorsed(&[entry("Старый", "Новый", &new_pub)], &members("owner"), &new_pub), "от прежнего владельца (теперь админ)");
        assert!(!handover_endorsed(&[entry("Рядовой", "Новый", &new_pub)], &members("owner"), &new_pub), "запись рядового не считается");
        assert!(!handover_endorsed(&[], &members("owner"), &new_pub), "без записи нельзя");
        assert!(!handover_endorsed(&[entry("Старый", "Новый", &new_pub)], &members("member"), &new_pub), "новый не владелец по списку узла");
        assert!(!handover_endorsed(&[entry("Новый", "Новый", &new_pub)], &members("owner"), &new_pub), "сам новый владелец себя не подтверждает");
        assert!(!handover_endorsed(&[entry("Старый", "Рядовой", &new_pub)], &members("owner"), &new_pub), "запись про другого");
        assert!(!handover_endorsed(&[entry("Старый", "Новый", &b64(&[5u8; 32]))], &members("owner"), &new_pub), "запись с другим ключом");
        let mut gone = entry("Старый", "Новый", &new_pub);
        gone.deleted = true;
        assert!(!handover_endorsed(&[gone], &members("owner"), &new_pub), "стёртая запись не считается");
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
}
