//! Компьютер как собеседник телефонов владельца: переписка «телефон ↔ этот узел» (веб-чат на странице узла).
//!
//! У узла своя пара ключей для телефонов (x25519 для шифрования, ed25519 для подписи связки), она лежит в `mobile_self.json` в папке данных
//! (права 0600). Телефон видит компьютер в контактах под номером узла, берёт его подписанную связку из `/mobile/pubkey/<номер узла>` и
//! шифрует, как для другого телефона (конверт E3, `e2e_crypto.dart`). Узел здесь — конечная точка переписки: он расшифровывает
//! сообщение и кладёт его в историю чата, которую показывает страница. Ответ со страницы узел шифрует открытым ключом телефона
//! (из подписанной связки, которую телефон зарегистрировал) и кладёт в очередь телефона. Открытым текстом между телефоном и узлом
//! ничего не ходит, кроме TLS.
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use x25519_dalek::{PublicKey, StaticSecret};

/// Формат конверта, общий с приложением: `[0xE3][32 одноразовый x25519][12 nonce][шифртекст + 16 метка]`, AAD = "YANDI-E2E-V2\0" ‖ одноразовый ключ.
const MAGIC: u8 = 0xE3;
const AAD_PREFIX: &[u8] = b"YANDI-E2E-V2\0";
const BUNDLE_DOMAIN: &[u8] = b"YANDI-MOBILE-KEYS-V1\0";

#[derive(Serialize, Deserialize)]
struct Stored {
    x25519_secret: String,
    ed25519_secret: String,
}

struct Keys {
    x_secret: StaticSecret,
    x_pub: [u8; 32],
    ed_pub: [u8; 32],
    sig: [u8; 64],
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

fn keys() -> &'static Keys {
    static K: OnceLock<Keys> = OnceLock::new();
    K.get_or_init(|| load_or_create(&crate::util::data_dir::data_dir().join("mobile_self.json")))
}

fn load_or_create(path: &std::path::Path) -> Keys {
    let stored: Option<([u8; 32], [u8; 32])> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Stored>(&s).ok())
        .and_then(|s| Some((unb64(&s.x25519_secret)?.try_into().ok()?, unb64(&s.ed25519_secret)?.try_into().ok()?)));
    let (x, e) = stored.unwrap_or_else(|| {
        let mut x = [0u8; 32];
        let mut e = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut x);
        rand::thread_rng().fill_bytes(&mut e);
        if let Ok(s) = serde_json::to_vec(&Stored { x25519_secret: b64(&x), ed25519_secret: b64(&e) }) {
            let _ = crate::util::private_file::write_private(path, &s);
        }
        (x, e)
    });
    make_keys(x, e)
}

fn make_keys(x: [u8; 32], e: [u8; 32]) -> Keys {
    use ed25519_dalek::{Signer, SigningKey};
    let x_secret = StaticSecret::from(x);
    let x_pub = PublicKey::from(&x_secret).to_bytes();
    let signing = SigningKey::from_bytes(&e);
    let ed_pub = signing.verifying_key().to_bytes();
    let sig = signing.sign(&[BUNDLE_DOMAIN, &ed_pub, &x_pub].concat()).to_bytes();
    Keys { x_secret, x_pub, ed_pub, sig }
}

/// Подписанная связка ключей узла для `/mobile/pubkey/<номер узла>` (тот же вид, что у телефонов).
pub fn bundle_json() -> serde_json::Value {
    let k = keys();
    serde_json::json!({"x25519_pub": b64(&k.x_pub), "ed25519_pub": b64(&k.ed_pub), "signature": b64(&k.sig)})
}

fn aad(eph: &[u8]) -> Vec<u8> {
    [AAD_PREFIX, eph].concat()
}

/// Зашифровать текст для телефона (его открытый x25519 из подписанной связки).
pub fn seal(text: &str, recipient_x25519: &[u8; 32]) -> Vec<u8> {
    let mut eph_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut eph_bytes);
    let eph = StaticSecret::from(eph_bytes);
    let eph_pub = PublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&PublicKey::from(*recipient_x25519));
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(shared.as_bytes()).expect("32-byte key");
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: text.as_bytes(), aad: &aad(&eph_pub) }).expect("aes-gcm encrypt");
    [&[MAGIC][..], &eph_pub, &nonce, &ct].concat()
}

/// Расшифровать конверт телефона, адресованный узлу. None — не конверт E3, чужой ключ, подделка.
pub fn open(blob: &[u8]) -> Option<String> {
    open_with(&keys().x_secret, blob)
}

fn open_with(secret: &StaticSecret, blob: &[u8]) -> Option<String> {
    if blob.len() <= 45 + 16 || blob[0] != MAGIC {
        return None;
    }
    let eph: [u8; 32] = blob[1..33].try_into().ok()?;
    let shared = secret.diffie_hellman(&PublicKey::from(eph));
    if shared.as_bytes().iter().all(|b| *b == 0) {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(shared.as_bytes()).ok()?;
    let plain = cipher.decrypt(Nonce::from_slice(&blob[33..45]), Payload { msg: &blob[45..], aad: &aad(&eph) }).ok()?;
    String::from_utf8(plain).ok()
}

/// Окно повторов для конвертов от телефонов (тот же конверт второй раз не принимаем): хэши последних конвертов в памяти.
pub fn first_time(blob: &[u8]) -> bool {
    use sha2::{Digest, Sha256};
    use std::collections::VecDeque;
    static SEEN: std::sync::Mutex<Option<(std::collections::HashSet<[u8; 32]>, VecDeque<[u8; 32]>)>> = std::sync::Mutex::new(None);
    let h: [u8; 32] = Sha256::digest(blob).into();
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let (set, order) = g.get_or_insert_with(Default::default);
    if !set.insert(h) {
        return false;
    }
    order.push_back(h);
    if order.len() > 4096 {
        if let Some(old) = order.pop_front() {
            set.remove(&old);
        }
    }
    true
}

// ── Файлы между телефоном и компьютером ──
//
// Та же схема, что между телефонами (`file_crypto.dart`, docs/CLIENT_WIRE.md «Файлы между телефонами»): свой AES-256-GCM-ключ на файл,
// кусок 256 КБ, на проводе `[12 nonce][шифртекст][16 метка]`, AAD = `yandi-file:v1|<номер передачи>|<номер куска>|<всего>`. Ключ, имя, размер
// и sha256 уходят зашифрованным предложением `\u0001yandi-file:{json}`. Узел здесь — конечная точка: принимает файл от телефона в папку
// загрузок страницы (`files/downloads`), отдаёт файл со страницы телефону через хранилище кусков `mobile_files`.

pub const FILE_MARKER: &str = "\u{1}yandi-file:";
const FILE_CHUNK: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
struct Offer {
    #[serde(default)]
    v: u32,
    tid: String,
    name: String,
    size: u64,
    chunks: u32,
    #[serde(default)]
    mime: String,
    key: String,
    #[serde(default)]
    sha: String,
}

fn file_aad(tid: &str, idx: u32, total: u32) -> Vec<u8> {
    format!("yandi-file:v1|{tid}|{idx}|{total}").into_bytes()
}

fn seal_chunk(key: &[u8; 32], tid: &str, idx: u32, total: u32, plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: &file_aad(tid, idx, total) }).expect("aes-gcm encrypt");
    [&nonce[..], &ct].concat()
}

fn open_chunk(key: &[u8], tid: &str, idx: u32, total: u32, wire: &[u8]) -> Option<Vec<u8>> {
    if wire.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher.decrypt(Nonce::from_slice(&wire[..12]), Payload { msg: &wire[12..], aad: &file_aad(tid, idx, total) }).ok()
}

/// Принятый от телефона файл: куда он лёг и как показать его на странице чата.
pub struct ReceivedFile {
    pub attachment: crate::communication::FileAttachment,
}

/// Телефон прислал предложение файла компьютеру: забрать куски из `mobile_files`, расшифровать, сверить размер и sha256, положить в
/// загрузки страницы. Куски с узла удаляются после успеха.
pub async fn receive_file(node_hex: &str, from_device: &str, offer_text: &str) -> Result<ReceivedFile, String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;
    let o: Offer = serde_json::from_str(offer_text.strip_prefix(FILE_MARKER).ok_or("не предложение файла")?).map_err(|e| e.to_string())?;
    let key = unb64(&o.key).filter(|k| k.len() == 32).ok_or("неверный ключ файла")?;
    let (chunks, size) = crate::mobile_files::ready_for(&o.tid, from_device, node_hex).await.ok_or("передача не найдена или не завершена")?;
    if chunks != o.chunks || size != o.size {
        return Err("предложение не совпадает с передачей на узле".into());
    }
    let file_id = format!("phone_{}", o.tid);
    let dir = crate::communication::files_dir("downloads");
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let stored = format!("{}__{}", file_id, crate::web::server::sanitize_storage_filename(&o.name));
    let part = dir.join(format!("{stored}.part"));
    let mut out = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let res: Result<(), String> = async {
        for i in 0..o.chunks {
            let wire = crate::mobile_files::read_chunk(&o.tid, i).await.ok_or(format!("нет куска {}", i + 1))?;
            let plain = open_chunk(&key, &o.tid, i, o.chunks, &wire).ok_or(format!("кусок {} не прошёл проверку: подменён или повреждён", i + 1))?;
            hash.update(&plain);
            total += plain.len() as u64;
            out.write_all(&plain).await.map_err(|e| e.to_string())?;
        }
        out.flush().await.map_err(|e| e.to_string())?;
        if total != o.size {
            return Err(format!("размер не совпал: {total} из {}", o.size));
        }
        if !o.sha.is_empty() && hex::encode(hash.finalize()) != o.sha.to_lowercase() {
            return Err("контрольная сумма не совпала".into());
        }
        Ok(())
    }
    .await;
    drop(out);
    if let Err(e) = res {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, dir.join(&stored)).await.map_err(|e| e.to_string())?;
    crate::mobile_files::drop_transfer(&o.tid).await;
    let mime = if o.mime.is_empty() { "application/octet-stream".to_string() } else { o.mime };
    Ok(ReceivedFile {
        attachment: crate::communication::FileAttachment {
            filename: o.name,
            size: o.size,
            mime_type: mime,
            data: None,
            file_ref: Some(crate::communication::FileReference { file_id, total_chunks: o.chunks, local_name: Some(stored) }),
        },
    })
}

/// Отправить файл со страницы телефону: зашифровать куски своим ключом, положить в `mobile_files` (от узла телефону) и вернуть текст
/// предложения (его надо запечатать ключом телефона и положить в его очередь).
pub async fn send_file(node_hex: &str, device: &str, path: &std::path::Path, name: &str, mime: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let size = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?.len();
    let chunks = (size.div_ceil(FILE_CHUNK as u64)).max(1) as u32;
    let tid = crate::mobile_files::node_begin(node_hex, device, size, chunks).await?;
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let mut f = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; FILE_CHUNK];
    let res: Result<(), String> = async {
        for i in 0..chunks {
            let want = (size - i as u64 * FILE_CHUNK as u64).min(FILE_CHUNK as u64) as usize;
            f.read_exact(&mut buf[..want]).await.map_err(|e| e.to_string())?;
            hash.update(&buf[..want]);
            crate::mobile_files::node_put(&tid, i, &seal_chunk(&key, &tid, i, chunks, &buf[..want])).await?;
        }
        crate::mobile_files::node_finish(&tid).await
    }
    .await;
    if let Err(e) = res {
        crate::mobile_files::drop_transfer(&tid).await;
        return Err(e);
    }
    let offer = Offer {
        v: 1,
        tid,
        name: name.to_string(),
        size,
        chunks,
        mime: if mime.is_empty() { "application/octet-stream".into() } else { mime.to_string() },
        key: b64(&key),
        sha: hex::encode(hash.finalize()),
    };
    Ok(format!("{FILE_MARKER}{}", serde_json::to_string(&offer).map_err(|e| e.to_string())?))
}

// ── Служебные конверты приложения (`receipt.dart` MsgChannel): статусы доставки и правки ──
//
// Всё ходит внутри того же шифрования, узел-собеседник разбирает их после расшифровки. Свои сообщения узел заворачивает в конверт с
// номером (cmid = msg_id сообщения в hex), чтобы телефон прислал «доставлено» и «прочитано»; входящим конвертам телефона узел отвечает
// «доставлено» сразу и «прочитано», когда владелец открыл переписку на странице.

const MSG_MARKER: &str = "\u{1}yandi-msg:";
const RCPT_MARKER: &str = "\u{1}yandi-rcpt:";
const CAP_MARKER: &str = "\u{1}yandi-cap:";
const EDIT_MARKER: &str = "\u{1}yandi-edit:";

pub enum Envelope {
    /// сообщение с номером отправителя
    Message { cmid: String, text: String },
    /// квитанция: «delivered» или «read» по номерам наших сообщений
    Receipt { read: bool, ids: Vec<String> },
    /// правка ранее присланного сообщения
    Edit { cmid: String, text: String },
    /// устройство умеет квитанции
    CapPing,
    /// обычный текст (старая сборка)
    Plain(String),
}

pub fn classify(text: String) -> Envelope {
    #[derive(Deserialize)]
    struct M {
        cmid: String,
        text: String,
    }
    #[derive(Deserialize)]
    struct R {
        kind: String,
        ids: Vec<String>,
    }
    if let Some(j) = text.strip_prefix(MSG_MARKER) {
        if let Ok(m) = serde_json::from_str::<M>(j) {
            if !m.cmid.is_empty() {
                return Envelope::Message { cmid: m.cmid, text: m.text };
            }
        }
    } else if let Some(j) = text.strip_prefix(RCPT_MARKER) {
        if let Ok(r) = serde_json::from_str::<R>(j) {
            if r.kind == "delivered" || r.kind == "read" {
                return Envelope::Receipt { read: r.kind == "read", ids: r.ids };
            }
        }
    } else if let Some(j) = text.strip_prefix(EDIT_MARKER) {
        if let Ok(m) = serde_json::from_str::<M>(j) {
            if !m.cmid.is_empty() {
                return Envelope::Edit { cmid: m.cmid, text: m.text };
            }
        }
    } else if text.starts_with(CAP_MARKER) {
        return Envelope::CapPing;
    }
    Envelope::Plain(text)
}

pub fn wrap_message(cmid: &str, text: &str) -> String {
    format!("{MSG_MARKER}{}", serde_json::json!({"v": 1, "cmid": cmid, "text": text}))
}

pub fn receipt(read: bool, ids: &[String]) -> String {
    format!("{RCPT_MARKER}{}", serde_json::json!({"kind": if read { "read" } else { "delivered" }, "ids": ids}))
}

pub fn edit(cmid: &str, text: &str) -> String {
    format!("{EDIT_MARKER}{}", serde_json::json!({"cmid": cmid, "text": text}))
}

pub fn cap_ping() -> String {
    format!("{CAP_MARKER}1")
}

/// Номер входящего сообщения телефона в истории узла: выводится из номера устройства и его cmid (по нему же находится правка).
pub fn incoming_id(device: &str, cmid: &str) -> crate::util::HashId {
    use sha2::{Digest, Sha256};
    crate::util::HashId(Sha256::digest(format!("yandi-phone-msg:{device}:{cmid}").as_bytes()).into())
}

/// Входящие конверты, на которые ещё не ушло «прочитано»: номер в истории → (устройство, cmid). Файл `mobile_self_unread.json`.
#[derive(Serialize, Deserialize, Clone)]
struct Unread {
    device: String,
    cmid: String,
}

static UNREAD: std::sync::Mutex<Option<std::collections::HashMap<String, Unread>>> = std::sync::Mutex::new(None);

fn with_unread<R>(f: impl FnOnce(&mut std::collections::HashMap<String, Unread>) -> R) -> R {
    let path = crate::util::data_dir::data_dir().join("mobile_self_unread.json");
    let mut g = UNREAD.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(|| std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    let r = f(map);
    if map.len() > 20000 {
        map.clear(); // старьё: «прочитано» по ним уже не нужно
    }
    if let Ok(s) = serde_json::to_vec(&*map) {
        let _ = crate::util::private_file::write_private(&path, &s);
    }
    r
}

pub fn remember_unread(local: &crate::util::HashId, device: &str, cmid: &str) {
    with_unread(|m| {
        m.insert(hex::encode(local.0), Unread { device: device.to_string(), cmid: cmid.to_string() });
    });
}

/// Забрать номера (cmid) входящих этого телефона, по которым ещё не отправлено «прочитано».
pub fn take_unread(device: &str) -> Vec<String> {
    with_unread(|m| {
        let keys: Vec<String> = m.iter().filter(|(_, u)| u.device == device).map(|(k, _)| k.clone()).collect();
        keys.into_iter().filter_map(|k| m.remove(&k)).map(|u| u.cmid).collect()
    })
}

/// Передачи файлов узел → телефон: номер передачи → (устройство, номер сообщения в истории). Когда телефон забрал файл и удалил передачу,
/// сообщение на странице становится «доставлено».
static FILE_MSGS: std::sync::Mutex<Option<std::collections::HashMap<String, (String, crate::util::HashId)>>> = std::sync::Mutex::new(None);

pub fn remember_file(tid: &str, device: &str, msg: crate::util::HashId) {
    FILE_MSGS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(tid.to_string(), (device.to_string(), msg));
}

pub fn take_file(tid: &str) -> Option<(String, crate::util::HashId)> {
    FILE_MSGS.lock().unwrap_or_else(|e| e.into_inner()).as_mut()?.remove(tid)
}

/// Номер передачи из текста предложения файла.
pub fn offer_tid(offer_text: &str) -> Option<String> {
    serde_json::from_str::<Offer>(offer_text.strip_prefix(FILE_MARKER)?).ok().map(|o| o.tid)
}

// ── Телефоны, спрятанные из списка веб-чата («Удалить» у телефона на странице). Сопряжение не трогается. ──

fn hidden_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("web_hidden_phones.json")
}

fn with_hidden<R>(f: impl FnOnce(&mut Vec<String>) -> R) -> R {
    static H: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);
    let mut g = H.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| std::fs::read_to_string(hidden_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    let before = list.clone();
    let r = f(list);
    if *list != before {
        if let Ok(s) = serde_json::to_vec(&*list) {
            let _ = crate::util::private_file::write_private(&hidden_path(), &s);
        }
    }
    r
}

/// Спрятать телефон из списка чата (номер устройства полностью или его начало, не короче 16 знаков).
pub fn hide_phone(id: &str) {
    if id.len() >= 16 {
        with_hidden(|l| {
            if !l.iter().any(|x| x == id) {
                l.push(id.to_string());
            }
        });
    }
}

pub fn is_hidden_phone(peer: &str) -> bool {
    with_hidden(|l| l.iter().any(|x| peer.starts_with(x.as_str())))
}

/// Спрятанный телефон написал — вернуть его в список, чтобы сообщение не потерялось незаметно.
pub fn unhide_phone(peer: &str) {
    with_hidden(|l| l.retain(|x| !peer.starts_with(x.as_str())));
}

// ── Звонки телефон ↔ страница узла ──
//
// Сигналы звонка (`\u0001yandi-call:` — invite/accept/offer/answer/ice/reject/busy/hangup, как в call_signal.dart) телефон шифрует для
// узла; узел их расшифровывает и отдаёт странице чата (она опрашивает очередь раз в секунду), а ответы страницы шифрует телефону. Звук и
// видео идут напрямую браузер ↔ телефон (WebRTC, DTLS-SRTP) — узел их не видит. Пока страница не открыта, звонок на компьютер получает
// «не на связи».

pub const CALL_MARKER: &str = "\u{1}yandi-call:";

struct CallQueue {
    seq: u64,
    events: std::collections::VecDeque<(u64, String, String)>,
    last_poll: Option<std::time::Instant>,
}

static CALLS: std::sync::Mutex<CallQueue> = std::sync::Mutex::new(CallQueue { seq: 0, events: std::collections::VecDeque::new(), last_poll: None });

/// Страница чата открыта (опрашивала очередь звонков последние несколько секунд)?
pub fn call_page_alive() -> bool {
    CALLS.lock().unwrap_or_else(|e| e.into_inner()).last_poll.map(|t| t.elapsed() < std::time::Duration::from_secs(6)).unwrap_or(false)
}

pub fn push_call(from_device: &str, text: &str) {
    let mut q = CALLS.lock().unwrap_or_else(|e| e.into_inner());
    q.seq += 1;
    let seq = q.seq;
    q.events.push_back((seq, from_device.to_string(), text.to_string()));
    while q.events.len() > 500 {
        q.events.pop_front();
    }
}

/// События после `after` для страницы: (номер, от кого, текст сигнала); и последний номер.
pub fn poll_calls(after: u64) -> (Vec<(u64, String, String)>, u64) {
    let mut q = CALLS.lock().unwrap_or_else(|e| e.into_inner());
    q.last_poll = Some(std::time::Instant::now());
    (q.events.iter().filter(|(s, _, _)| *s > after).cloned().collect(), q.seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_for_a_key_opens_only_with_it_and_tampering_is_refused() {
        let me = make_keys([1; 32], [2; 32]);
        let other = make_keys([3; 32], [4; 32]);
        let blob = seal("привет", &me.x_pub);
        assert_eq!(blob[0], MAGIC);
        assert_eq!(open_with(&me.x_secret, &blob).as_deref(), Some("привет"));
        assert!(open_with(&other.x_secret, &blob).is_none(), "чужой ключ");
        let mut bad = blob.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(open_with(&me.x_secret, &bad).is_none(), "метка не сходится");
        let mut bad_eph = blob.clone();
        bad_eph[5] ^= 1;
        assert!(open_with(&me.x_secret, &bad_eph).is_none(), "одноразовый ключ входит в AAD/DH");
        assert!(open_with(&me.x_secret, b"plain text that is long enough to pass the length check........").is_none());
    }

    #[test]
    fn the_bundle_signature_verifies_like_the_phone_checks_it() {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let k = make_keys([5; 32], [6; 32]);
        let vk = VerifyingKey::from_bytes(&k.ed_pub).unwrap();
        assert!(vk.verify(&[BUNDLE_DOMAIN, &k.ed_pub, &k.x_pub].concat(), &Signature::from_bytes(&k.sig)).is_ok());
    }

    #[test]
    fn keys_survive_a_restart() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("mobile_self.json");
        let a = load_or_create(&p);
        let b = load_or_create(&p);
        assert_eq!(a.x_pub, b.x_pub);
        assert_eq!(a.ed_pub, b.ed_pub);
    }

    #[test]
    fn file_chunks_open_only_in_their_place() {
        let key = [9u8; 32];
        let tid = "0123456789abcdef0123456789abcdef";
        let wire = seal_chunk(&key, tid, 1, 3, b"chunk");
        assert_eq!(open_chunk(&key, tid, 1, 3, &wire).as_deref(), Some(&b"chunk"[..]));
        assert!(open_chunk(&key, tid, 0, 3, &wire).is_none(), "другое место");
        assert!(open_chunk(&key, tid, 1, 2, &wire).is_none(), "обрезанная передача");
        assert!(open_chunk(&[8u8; 32], tid, 1, 3, &wire).is_none(), "чужой ключ");
    }

    #[test]
    fn service_envelopes_round_trip_like_the_app_writes_them() {
        assert!(matches!(classify(wrap_message("12", "hi")), Envelope::Message { cmid, text } if cmid == "12" && text == "hi"));
        assert!(matches!(classify(receipt(true, &["a".into()])), Envelope::Receipt { read: true, ids } if ids == ["a"]));
        assert!(matches!(classify(edit("7", "new")), Envelope::Edit { cmid, text } if cmid == "7" && text == "new"));
        assert!(matches!(classify(cap_ping()), Envelope::CapPing));
        assert!(matches!(classify("просто текст".into()), Envelope::Plain(_)));
        // как пишет Dart: '\u0001yandi-rcpt:{"kind":"delivered","ids":["1791"]}'
        assert!(matches!(classify("\u{1}yandi-rcpt:{\"kind\":\"delivered\",\"ids\":[\"1791\"]}".into()), Envelope::Receipt { read: false, .. }));
        assert_eq!(incoming_id("d", "1"), incoming_id("d", "1"));
        assert_ne!(incoming_id("d", "1"), incoming_id("e", "1"));
    }

    #[test]
    fn a_replayed_envelope_is_refused() {
        let blob = seal("x", &make_keys([7; 32], [8; 32]).x_pub);
        assert!(first_time(&blob));
        assert!(!first_time(&blob));
    }
}
