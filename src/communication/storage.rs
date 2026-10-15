// src/communication/storage.rs
//! Local storage for chat history (encrypted JSON Lines format)

use crate::communication::ChatMessage;
use crate::util::HashId;
use std::path::PathBuf;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce, Key
};
use rand::RngCore;
use hkdf::Hkdf;
use sha2::Sha256;

/// Получить текущий timestamp в ms
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// One message waiting for its recipient's confirmation (see the outgoing queue in chat.rs).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutboxEntry {
    pub peer: HashId,
    pub msg: ChatMessage,
    /// when it was first sent (unix seconds): the queue gives up on a message a day after this
    pub first_sent: u64,
}

/// Largest history file accepted from one peer's incoming messages.
const MAX_CHAT_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Состояние ключа хранилища чатов.
#[derive(Clone, Copy)]
enum KeyMode {
    Master([u8; 32]),
    Locked,
}

impl Drop for ChatStorage {
    fn drop(&mut self) {
        self.lock();
    }
}

/// Представление сообщения на диске (bincode не терпит `skip_serializing_if`, поэтому отдельные структуры без него).
#[derive(Serialize, Deserialize)]
struct StoredRef {
    file_id: String,
    total_chunks: u32,
    local_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredAttachment {
    filename: String,
    size: u64,
    mime_type: String,
    data: Option<String>,
    file_ref: Option<StoredRef>,
}

#[derive(Serialize, Deserialize)]
struct StoredReply {
    msg_id: String,
    text: String,
}

/// Запись версии 3: версия 2 и ответ-цитата.
#[derive(Serialize, Deserialize)]
struct StoredMessage {
    msg_id: HashId,
    from: HashId,
    to: HashId,
    timestamp: u64,
    text: String,
    encrypted: bool,
    status: crate::communication::MessageStatus,
    edited: bool,
    edit_timestamp: Option<u64>,
    attachment: Option<StoredAttachment>,
    reply: Option<StoredReply>,
}

/// Хранилище чатов (локальное для каждой ноды) с AES-256-GCM шифрованием
pub struct ChatStorage {
    my_node_id: HashId,
    chats_dir: PathBuf,
    /// Ключ чатов: ключ закрытой среды или «заперто» (ничего не читается и не пишется).
    key_mode: std::sync::RwLock<KeyMode>,
    /// Все изменения файла чата идут под одним замком: «прочитать всё, удалить файл, записать заново» (смена статуса, правка, удаление)
    /// не должно пересекаться с дописыванием нового сообщения, иначе оно пропадает из истории.
    write_lock: std::sync::Mutex<()>,
}

impl ChatStorage {
    /// Создать новое хранилище: заперто, пока закрытая среда не выдаст ключ
    pub fn new(my_node_id: HashId) -> Result<Self> {
        let base_dir = dirs::home_dir()
            .expect("No home directory")
            .join(".yandi/chats");

        std::fs::create_dir_all(&base_dir)?;

        Ok(Self {
            my_node_id,
            chats_dir: base_dir,
            key_mode: std::sync::RwLock::new(KeyMode::Locked),
            write_lock: std::sync::Mutex::new(()),
        })
    }

    /// Хранилище в заданной папке с ключом проверки (для проверок: они не должны писать в домашнюю папку владельца).
    #[cfg(test)]
    pub(crate) fn in_dir(my_node_id: HashId, dir: std::path::PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { my_node_id, chats_dir: dir, key_mode: std::sync::RwLock::new(KeyMode::Master([0x5a; 32])), write_lock: std::sync::Mutex::new(()) })
    }

    /// Создать хранилище с мастер-ключом (HKDF-derived encryption key)
    pub fn new_with_key(my_node_id: HashId, master_key: [u8; 32]) -> Result<Self> {
        let base_dir = dirs::home_dir()
            .expect("No home directory")
            .join(".yandi/chats");

        std::fs::create_dir_all(&base_dir)?;

        Ok(Self {
            my_node_id,
            chats_dir: base_dir,
            key_mode: std::sync::RwLock::new(KeyMode::Master(master_key)),
            write_lock: std::sync::Mutex::new(()),
        })
    }

    /// Хранилище закрытой среды: пока ключ не выдан, чаты не читаются и не пишутся.
    pub fn new_locked(my_node_id: HashId) -> Result<Self> {
        let st = Self::new(my_node_id)?;
        st.lock();
        Ok(st)
    }

    #[cfg(test)]
    pub(crate) fn in_dir_locked(my_node_id: HashId, dir: std::path::PathBuf) -> Result<Self> {
        let st = Self::in_dir(my_node_id, dir)?;
        st.lock();
        Ok(st)
    }

    /// Выдать ключ закрытой среды (после разблокировки).
    pub fn set_master(&self, master_key: [u8; 32]) {
        *self.key_mode.write().unwrap_or_else(|e| e.into_inner()) = KeyMode::Master(master_key);
    }

    /// Отозвать ключ: дальше любое чтение и запись возвращает ошибку, файлы не трогаются.
    pub fn lock(&self) {
        let mut mode = self.key_mode.write().unwrap_or_else(|e| e.into_inner());
        if let KeyMode::Master(k) = &mut *mode {
            k.fill(0);
        }
        *mode = KeyMode::Locked;
    }

    pub fn is_locked(&self) -> bool {
        matches!(*self.key_mode.read().unwrap_or_else(|e| e.into_inner()), KeyMode::Locked)
    }

    /// Get AES-256-GCM encryption key.
    /// With master key: HKDF-SHA256(master_key, salt=node_id, info="yandi-chat-v2")
    fn get_encryption_key(&self) -> Result<[u8; 32]> {
        let mode = *self.key_mode.read().unwrap_or_else(|e| e.into_inner());
        match mode {
            KeyMode::Master(mk) => {
                let hk = Hkdf::<Sha256>::new(Some(&self.my_node_id.0), &mk);
                let mut key = [0u8; 32];
                hk.expand(b"yandi-chat-v2", &mut key).expect("HKDF expand failed");
                Ok(key)
            }
            KeyMode::Locked => Err(anyhow::anyhow!("закрытая среда заперта")),
        }
    }

    /// Ключ AES из ключа закрытой среды.
    fn key_for(&self, master: &[u8; 32]) -> [u8; 32] {
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(Some(&self.my_node_id.0), master)
            .expand(b"yandi-chat-v2", &mut key)
            .expect("HKDF expand failed");
        key
    }

    /// Открыть одну строку (nonce || шифртекст в hex) любым из ключей; возвращает открытый текст.
    fn open_with_any(candidates: &[[u8; 32]], hex_line: &str) -> Option<(usize, Vec<u8>)> {
        let bytes = hex::decode(hex_line.trim()).ok().filter(|b| b.len() >= 12 + 16)?;
        candidates.iter().enumerate().find_map(|(i, k)| {
            Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(k))
                .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
                .ok()
                .map(|p| (i, p))
        })
    }

    fn seal_line(key: &[u8; 32], plain: &[u8]) -> Result<String> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
            .encrypt(Nonce::from_slice(&nonce), plain)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;
        Ok(hex::encode([nonce.as_slice(), &ct].concat()))
    }

    /// Запись сообщения в истории: байт версии 2 и компактное бинарное представление (JSON на тех же сообщениях
    /// раздувал запись: каждый идентификатор — список из 32 чисел).
    fn encode_message(msg: &ChatMessage) -> Result<Vec<u8>> {
        let stored = StoredMessage {
            msg_id: msg.msg_id,
            from: msg.from,
            to: msg.to,
            timestamp: msg.timestamp,
            text: msg.text.clone(),
            encrypted: msg.encrypted,
            status: msg.status.clone(),
            edited: msg.edited,
            edit_timestamp: msg.edit_timestamp,
            attachment: msg.attachment.as_ref().map(|a| StoredAttachment {
                filename: a.filename.clone(),
                size: a.size,
                mime_type: a.mime_type.clone(),
                data: a.data.clone(),
                file_ref: a.file_ref.as_ref().map(|r| StoredRef {
                    file_id: r.file_id.clone(),
                    total_chunks: r.total_chunks,
                    local_name: r.local_name.clone(),
                }),
            }),
            reply: msg.reply_to.as_ref().map(|r| StoredReply {
                msg_id: r.msg_id.clone(),
                text: r.text.clone(),
            }),
        };
        let mut out = vec![3u8];
        out.extend(bincode::serialize(&stored)?);
        Ok(out)
    }

    fn decode_message(plain: &[u8]) -> Option<ChatMessage> {
        let stored = |m: StoredMessage| ChatMessage {
            msg_id: m.msg_id,
            from: m.from,
            to: m.to,
            timestamp: m.timestamp,
            text: m.text,
            encrypted: m.encrypted,
            status: m.status,
            edited: m.edited,
            edit_timestamp: m.edit_timestamp,
            attachment: m.attachment.map(|a| crate::communication::FileAttachment {
                filename: a.filename,
                size: a.size,
                mime_type: a.mime_type,
                data: a.data,
                file_ref: a.file_ref.map(|r| crate::communication::FileReference {
                    file_id: r.file_id,
                    total_chunks: r.total_chunks,
                    local_name: r.local_name,
                }),
            }),
            reply_to: m.reply.map(|r| crate::communication::ReplyRef {
                msg_id: r.msg_id,
                text: r.text,
            }),
        };
        match plain.first() {
            Some(3u8) => bincode::deserialize::<StoredMessage>(&plain[1..]).ok().map(stored),
            _ => None,
        }
    }

    /// Формат файла чата: признак `YCH2`, дальше записи `[длина u32 BE][nonce 12 | шифртекст]`.
    const MAGIC: &'static [u8; 4] = b"YCH2";
    const MAX_RECORD: usize = 16 * 1024 * 1024;

    fn seal_blob(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
            .encrypt(Nonce::from_slice(&nonce), plain)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;
        Ok([nonce.as_slice(), &ct].concat())
    }

    fn open_blob(candidates: &[[u8; 32]], blob: &[u8]) -> Option<(usize, Vec<u8>)> {
        if blob.len() < 12 + 16 {
            return None;
        }
        candidates.iter().enumerate().find_map(|(i, k)| {
            Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(k))
                .decrypt(Nonce::from_slice(&blob[..12]), &blob[12..])
                .ok()
                .map(|p| (i, p))
        })
    }

    /// Записи файла по порядку. Оборванный хвост даёт пустую запись (она не откроется: пропускается, а при строгом чтении — ошибка).
    fn read_blobs(path: &std::path::Path) -> Result<Vec<Vec<u8>>> {
        let raw = std::fs::read(path)?;
        if !raw.starts_with(Self::MAGIC) {
            return Err(anyhow::anyhow!("файл не в формате чата"));
        }
        let mut blobs = Vec::new();
        let mut at = Self::MAGIC.len();
        while at < raw.len() {
            if at + 4 > raw.len() {
                blobs.push(Vec::new());
                break;
            }
            let len = u32::from_be_bytes(raw[at..at + 4].try_into().unwrap()) as usize;
            at += 4;
            if len > Self::MAX_RECORD || at + len > raw.len() {
                blobs.push(Vec::new());
                break;
            }
            blobs.push(raw[at..at + len].to_vec());
            at += len;
        }
        Ok(blobs)
    }

    fn encode_blobs(blobs: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::from(&Self::MAGIC[..]);
        for b in blobs {
            out.extend_from_slice(&(b.len() as u32).to_be_bytes());
            out.extend_from_slice(b);
        }
        out
    }

    /// Длина файла без оборванного хвоста — только если хвост именно оборван (не хватает байт до конца записи или её заголовка).
    /// Запись с неверной длиной в середине файла — не обрыв, её не трогаем (иначе срезали бы всё, что за ней).
    fn torn_tail_start(raw: &[u8]) -> Option<usize> {
        let mut at = Self::MAGIC.len();
        while at < raw.len() {
            let start = at;
            if at + 4 > raw.len() {
                return Some(start);
            }
            let len = u32::from_be_bytes(raw[at..at + 4].try_into().unwrap()) as usize;
            at += 4;
            if len > Self::MAX_RECORD {
                return None;
            }
            if at + len > raw.len() {
                return Some(start);
            }
            at += len;
        }
        None
    }

    /// Дописать запись. Одной записью: оборванное питанием дописывание оставляет не больше одного неполного хвоста; при первой
    /// записи в файл за процесс такой хвост срезается — иначе новые записи легли бы за ним и читались бы как его продолжение.
    fn append_blob(path: &std::path::Path, blob: &[u8]) -> Result<()> {
        use std::io::Write;
        static CHECKED: std::sync::Mutex<Option<std::collections::HashSet<std::path::PathBuf>>> = std::sync::Mutex::new(None);
        let first = CHECKED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Default::default)
            .insert(path.to_path_buf());
        if first {
            if let Ok(raw) = std::fs::read(path) {
                if raw.starts_with(Self::MAGIC) {
                    if let Some(keep) = Self::torn_tail_start(&raw) {
                        std::fs::OpenOptions::new().write(true).open(path)?.set_len(keep as u64)?;
                    }
                }
            }
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        let mut chunk = Vec::new();
        if file.metadata()?.len() == 0 {
            chunk.extend_from_slice(Self::MAGIC);
        }
        chunk.extend_from_slice(&(blob.len() as u32).to_be_bytes());
        chunk.extend_from_slice(blob);
        file.write_all(&chunk)?;
        Ok(())
    }

    /// Перевести журнал статусов на `candidates[0]`; `Ok(false)` — есть строка, которая не открывается ни одним ключом (файл не меняется).
    fn rekey_status_log(path: &std::path::Path, candidates: &[[u8; 32]]) -> Result<bool> {
        let text = std::fs::read_to_string(path)?;
        let mut out = String::new();
        let mut changed = false;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match Self::open_with_any(candidates, line) {
                Some((0, _)) => {
                    out.push_str(line.trim());
                }
                Some((_, plain)) => {
                    out.push_str(&Self::seal_line(&candidates[0], &plain)?);
                    changed = true;
                }
                None => return Ok(false),
            }
            out.push('\n');
        }
        if changed {
            let tmp = path.with_extension("enc.rekey");
            crate::util::private_file::write_private(&tmp, out.as_bytes())?;
            std::fs::rename(&tmp, path)?;
        }
        Ok(true)
    }

    /// Перенос всех файлов чатов и очереди на ключ закрытой среды и переключение хранилища на него. Идемпотентно: строка, уже
    /// открывающаяся новым ключом, остаётся; прочие открываются рабочим ключом машины (`old`), которым узел работал до пароля владельца.
    /// Файл с неоткрываемой строкой не меняется (счёт таких файлов возвращается).
    /// Всё под замком записи: параллельное дописывание не может вклиниться между проверкой и заменой.
    pub fn rekey_to(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> Result<usize> {
        let _g = self.locked();
        let candidates = {
            let mut c = vec![self.key_for(&new)];
            if let Some(o) = old.as_ref() {
                c.push(self.key_for(o));
            }
            c
        };
        let mut failed = 0usize;
        if self.chats_dir.exists() {
            for entry in std::fs::read_dir(&self.chats_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_chat = name.starts_with("chat_") && name.ends_with(".enc");
                if !(is_chat || name == "outbox.enc") || !entry.file_type()?.is_file() {
                    continue;
                }
                let path = entry.path();
                if name.ends_with(".status.enc") {
                    // журнал статусов — построчный (hex), не YCH2: переводится построчно, неоткрываемая строка оставляет файл как есть
                    match Self::rekey_status_log(&path, &candidates) {
                        Ok(true) => {}
                        Ok(false) | Err(_) => failed += 1,
                    }
                    continue;
                }
                let blobs = match Self::read_blobs(&path) {
                    Ok(v) => v,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                };
                let mut changed = false;
                let mut out: Vec<Vec<u8>> = Vec::new();
                let mut ok = true;
                for blob in blobs.iter() {
                    match Self::open_blob(&candidates, blob) {
                        Some((0, _)) => out.push(blob.clone()),
                        Some((_, plain)) => match Self::seal_blob(&candidates[0], &plain) {
                            Ok(b) => {
                                changed = true;
                                out.push(b);
                            }
                            Err(_) => ok = false,
                        },
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
                let tmp = path.with_extension("enc.rekey");
                crate::util::private_file::write_private(&tmp, &Self::encode_blobs(&out))?;
                // проверка до замены: каждая запись новой копии открывается новым ключом
                let ok_new = match Self::read_blobs(&tmp) {
                    Ok(check) => {
                        check.len() == out.len()
                            && check.iter().all(|b| matches!(Self::open_blob(&candidates[..1], b), Some((0, _))))
                    }
                    Err(_) => false,
                };
                if !ok_new {
                    let _ = std::fs::remove_file(&tmp);
                    failed += 1;
                    continue;
                }
                std::fs::rename(&tmp, &path)?;
            }
        }
        // Хранилище переключается всегда: перенесённое читается только новым ключом, а неоткрываемые файлы не читались и раньше.
        *self.key_mode.write().unwrap_or_else(|e| e.into_inner()) = KeyMode::Master(new);
        Ok(failed)
    }

    /// Удалить все зашифрованные файлы чатов (после стирания закрытой среды): `chat_*.enc`, `outbox.enc`, журналы статусов.
    pub fn erase_encrypted(&self) {
        let _g = self.locked();
        if let Ok(rd) = std::fs::read_dir(&self.chats_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if (name.starts_with("chat_") && (name.ends_with(".enc") || name.contains(".enc."))) || name == "outbox.enc" || name.ends_with(".enc.rekey") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, ()> {
        // замок защищает только порядок операций с файлами; отравление (паника в другой задаче) данных не портит
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Журнал статусов чата: дописываемый файл рядом с историей. Смена статуса — одна короткая строка, история не переписывается;
    /// при чтении последний статус сообщения накладывается на историю, при любой перезаписи истории журнал сворачивается в неё.
    fn status_log_path(&self, peer_id: &HashId) -> PathBuf {
        self.chat_file_path_enc(peer_id).with_extension("status.enc")
    }

    /// Журнал свёртывается в историю, когда вырастает больше этого (около 270 смен статуса).
    const STATUS_LOG_FOLD_BYTES: u64 = 64 * 1024;

    fn append_status(&self, peer_id: &HashId, msg_id: &HashId, status: &crate::communication::MessageStatus) -> Result<()> {
        use std::io::Write;
        let key = self.get_encryption_key()?;
        let plain = serde_json::to_vec(&(msg_id, status))?;
        let line = Self::seal_line(&key, &plain)?;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(self.status_log_path(peer_id))?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Последний статус каждого сообщения из журнала. `strict`: строка, которая не открывается, — ошибка (чтобы не потерять её при перезаписи).
    fn read_status_log(
        &self,
        peer_id: &HashId,
        key: &[u8; 32],
        strict: bool,
    ) -> Result<std::collections::HashMap<HashId, crate::communication::MessageStatus>> {
        let mut out = std::collections::HashMap::new();
        let path = self.status_log_path(peer_id);
        if !path.exists() {
            return Ok(out);
        }
        for line in std::fs::read_to_string(path)?.lines().filter(|l| !l.trim().is_empty()) {
            let parsed = Self::open_with_any(std::slice::from_ref(key), line)
                .and_then(|(_, p)| serde_json::from_slice::<(HashId, crate::communication::MessageStatus)>(&p).ok());
            match parsed {
                Some((id, st)) => {
                    out.insert(id, st);
                }
                None if strict => return Err(anyhow::anyhow!("в журнале статусов есть записи, которые не открываются этим ключом; файл не изменён")),
                None => {}
            }
        }
        Ok(out)
    }

    /// Путь к зашифрованному файлу чата: имя по полному идентификатору узла.
    fn chat_file_path_enc(&self, peer_id: &HashId) -> PathBuf {
        self.chats_dir.join(format!("chat_{}.enc", hex::encode(peer_id.0)))
    }

    /// Сохранить исходящее сообщение (шифрованное)
    pub fn save_outgoing(&self, to: &HashId, msg: &ChatMessage) -> Result<()> {
        let _g = self.locked();
        let chat_file = self.chat_file_path_enc(to);
        self.append_encrypted_message(&chat_file, msg)
    }

    /// Сохранить входящее сообщение (шифрованное)
    pub fn save_incoming(&self, from: &HashId, msg: &ChatMessage) -> Result<()> {
        let _g = self.locked();
        let chat_file = self.chat_file_path_enc(from);
        // a peer cannot grow a history without limit
        if std::fs::metadata(&chat_file).map_or(false, |m| m.len() > MAX_CHAT_FILE_BYTES) {
            return Err(anyhow::anyhow!("chat history of this peer is full"));
        }
        self.append_encrypted_message(&chat_file, msg)
    }

    /// Добавить зашифрованное сообщение в файл
    fn append_encrypted_message(&self, chat_file: &PathBuf, msg: &ChatMessage) -> Result<()> {
        let plaintext = Self::encode_message(msg)?;
        let key_bytes = self.get_encryption_key()?;
        let blob = Self::seal_blob(&key_bytes, &plaintext)?;
        Self::append_blob(chat_file, &blob)
    }

    /// The outgoing queue (messages not yet confirmed by their recipients), kept encrypted in one file so that it survives a restart.
    pub fn save_outbox(&self, entries: &[OutboxEntry]) -> Result<()> {
        let _g = self.locked();
        let path = self.chats_dir.join("outbox.enc");
        // заперто — очередь на диске не трогаем (в том числе не стираем пустой)
        let key_bytes = self.get_encryption_key()?;
        if entries.is_empty() {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        let plaintext = serde_json::to_vec(entries)?;
        let blob = Self::seal_blob(&key_bytes, &plaintext)?;
        crate::util::private_file::write_private(&path, &Self::encode_blobs(&[blob]))?;
        Ok(())
    }

    pub fn load_outbox(&self) -> Vec<OutboxEntry> {
        let _g = self.locked();
        let Ok(blobs) = Self::read_blobs(&self.chats_dir.join("outbox.enc")) else { return Vec::new() };
        let Some(blob) = blobs.first() else { return Vec::new() };
        let Ok(key_bytes) = self.get_encryption_key() else { return Vec::new() };
        match Self::open_blob(&[key_bytes], blob) {
            Some((_, p)) => serde_json::from_slice(&p).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Загрузить историю чата (расшифрованную)
    pub fn load_history(&self, peer_id: &HashId, limit: usize) -> Result<Vec<ChatMessage>> {
        let _g = self.locked();
        self.load_history_unlocked(peer_id, limit)
    }

    fn load_history_unlocked(&self, peer_id: &HashId, limit: usize) -> Result<Vec<ChatMessage>> {
        self.read_history(peer_id, limit, false)
    }

    /// Для перезаписи файла: строка, которую не удалось открыть (чужой ключ, порча), прерывает операцию, чтобы не потерять её молча.
    fn load_history_strict(&self, peer_id: &HashId) -> Result<Vec<ChatMessage>> {
        self.read_history(peer_id, usize::MAX, true)
    }

    fn read_history(&self, peer_id: &HashId, limit: usize, strict: bool) -> Result<Vec<ChatMessage>> {
        let key_bytes = self.get_encryption_key()?;   // заперто — ошибка, а не «пустая история»
        let chat_file = self.chat_file_path_enc(peer_id);

        if !chat_file.exists() {
            return Ok(Vec::new());
        }

        let blobs = Self::read_blobs(&chat_file)?;
        let keys = [key_bytes];

        let mut messages = Vec::new();
        for blob in blobs.iter().rev().take(limit) {
            let opened = Self::open_blob(&keys, blob)
                .and_then(|(_, p)| Self::decode_message(&p));
            match opened {
                Some(msg) => messages.push(msg),
                None if strict => {
                    return Err(anyhow::anyhow!(
                        "в истории есть записи, которые не открываются этим ключом; файл не изменён"
                    ))
                }
                None => {}
            }
        }

        let statuses = self.read_status_log(peer_id, &key_bytes, strict)?;
        if !statuses.is_empty() {
            for m in messages.iter_mut() {
                if let Some(st) = statuses.get(&m.msg_id) {
                    m.status = st.clone();
                }
            }
        }
        Ok(messages)
    }

    /// Обновить статус сообщения: дописывается строка в журнал статусов (история не перезаписывается)
    pub fn update_message_status(
        &self,
        peer_id: &HashId,
        msg_id: &HashId,
        status: crate::communication::MessageStatus,
    ) -> Result<()> {
        let _g = self.locked();
        let chat_file = self.chat_file_path_enc(peer_id);

        if !chat_file.exists() {
            return Ok(());
        }

        self.append_status(peer_id, msg_id, &status)?;
        let big = std::fs::metadata(self.status_log_path(peer_id)).map_or(false, |m| m.len() > Self::STATUS_LOG_FOLD_BYTES);
        if big {
            // свёртка: история с наложенными статусами переписывается один раз, журнал после этого удаляется
            let messages = self.load_history_strict(peer_id)?;
            self.rewrite_all_messages(peer_id, &messages)?;
        }
        Ok(())
    }

    /// Перезаписать все сообщения (при редактировании/удалении). Вызывать только под замком. Пишет во временный файл и переименовывает:
    /// при сбое посередине остаётся целая прежняя история, а не обрезанная.
    fn rewrite_all_messages(&self, peer_id: &HashId, messages: &[ChatMessage]) -> Result<()> {
        let chat_file = self.chat_file_path_enc(peer_id);
        let tmp = chat_file.with_extension("enc.tmp");
        let _ = std::fs::remove_file(&tmp);
        // `messages` comes from load_history (newest first); the file is chronological, oldest first — write it back the way it was
        // (writing it in the given order reversed the whole history on every status update)
        for msg in messages.iter().rev() {
            self.append_encrypted_message(&tmp, msg)?;
        }
        if messages.is_empty() {
            let _ = std::fs::remove_file(&tmp);
            if chat_file.exists() {
                std::fs::remove_file(&chat_file)?;
            }
            let _ = std::fs::remove_file(self.status_log_path(peer_id));
            return Ok(());
        }
        std::fs::rename(&tmp, &chat_file)?;
        // статусы уже внутри новой истории; если выключится питание здесь, журнал при чтении наложит те же значения
        let _ = std::fs::remove_file(self.status_log_path(peer_id));
        Ok(())
    }

    /// Очистить историю чата с конкретным peer
    pub fn clear_history(&self, peer_id: &HashId) -> Result<()> {
        let _g = self.locked();
        let chat_file = self.chat_file_path_enc(peer_id);
        if chat_file.exists() {
            std::fs::remove_file(chat_file)?;
        }
        let _ = std::fs::remove_file(self.status_log_path(peer_id));
        Ok(())
    }

    /// Очистить всю историю
    pub fn clear_all(&self) -> Result<()> {
        let _g = self.locked();
        if self.chats_dir.exists() {
            std::fs::remove_dir_all(&self.chats_dir)?;
            std::fs::create_dir_all(&self.chats_dir)?;
        }
        Ok(())
    }

    /// Обновить текст сообщения (редактирование)
    pub fn update_message_text(
        &self,
        peer_id: &HashId,
        msg_id: &HashId,
        new_text: String,
    ) -> Result<()> {
        let _g = self.locked();
        let mut messages = self.load_history_strict(peer_id)?;
        let mut updated = false;
        
        for msg in messages.iter_mut() {
            if msg.msg_id == *msg_id {
                msg.text = new_text.clone();
                msg.edited = true;
                msg.edit_timestamp = Some(now_ms());
                updated = true;
                break;
            }
        }

        if updated {
            self.rewrite_all_messages(peer_id, &messages)?;
        }

        Ok(())
    }

    /// Удалить сообщение
    pub fn delete_message(&self, peer_id: &HashId, msg_id: &HashId) -> Result<()> {
        let _g = self.locked();
        let messages = self.load_history_strict(peer_id)?;
        let filtered: Vec<ChatMessage> = messages
            .into_iter()
            .filter(|msg| msg.msg_id != *msg_id)
            .collect();

        self.rewrite_all_messages(peer_id, &filtered)
    }

    /// Срок хранения истории чата: год (с запасом на високосный). Старше этого сообщения с узла удаляются.
    pub const RETENTION_MS: u64 = 366 * 24 * 60 * 60 * 1000;

    /// Удаляет из всех чатов сообщения старше срока хранения; возвращает, сколько удалено.
    /// Чат с записью, которая не открывается этим ключом, не трогается (как и при других перезаписях).
    pub fn prune_expired(&self, now_ms: u64) -> Result<usize> {
        let cutoff = now_ms.saturating_sub(Self::RETENTION_MS);
        let mut removed = 0usize;
        for peer in self.list_chats()? {
            let _g = self.locked();
            let Ok(messages) = self.load_history_strict(&peer) else { continue };
            let total = messages.len();
            let kept: Vec<ChatMessage> = messages.into_iter().filter(|m| m.timestamp >= cutoff).collect();
            if kept.len() < total {
                self.rewrite_all_messages(&peer, &kept)?;
                removed += total - kept.len();
            }
        }
        Ok(removed)
    }

    /// Получить список всех peer, с которыми был чат
    pub fn list_chats(&self) -> Result<Vec<HashId>> {
        let mut peers = Vec::new();
        
        if !self.chats_dir.exists() {
            return Ok(peers);
        }

        for entry in std::fs::read_dir(&self.chats_dir)? {
            let entry = entry?;
            let filename = entry.file_name();
            let filename_str = filename.to_string_lossy();
            
            if filename_str.starts_with("chat_") && filename_str.ends_with(".enc") {
                let short_id = &filename_str[5..filename_str.len()-4];
                let mut bytes = [0u8; 32];
                if let Ok(short_bytes) = hex::decode(short_id) {
                    if short_bytes.is_empty() || short_bytes.len() > 32 {
                        continue;
                    }
                    bytes[..short_bytes.len()].copy_from_slice(&short_bytes);
                    // an old short-named file and a full-named one of the same peer are one chat
                    if !peers.iter().any(|p: &HashId| p.0[..8] == bytes[..8]) {
                        peers.push(HashId(bytes));
                    }
                }
            }
        }

        Ok(peers)
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use crate::communication::MessageStatus;
    use std::sync::Arc;

    /// Найдено на живой сети: подтверждения доставки (приходят дублями) меняют статус через «прочитать всё, удалить файл, записать заново»;
    /// сообщение, дописанное в этот момент, пропадало из истории (в журнале «сохранено», в истории нет).
    #[test]
    fn two_peers_with_the_same_id_prefix_get_separate_histories() {
        let dir = tempfile::tempdir().unwrap();
        let me = HashId([1; 32]);
        let st = ChatStorage::in_dir(me, dir.path().to_path_buf()).unwrap();
        let a = HashId([7; 32]);
        let mut b = [7u8; 32];
        b[31] = 9; // the same first 8 bytes, a different peer
        let b = HashId(b);
        st.save_incoming(&a, &ChatMessage::new(a, me, "from a".into())).unwrap();
        st.save_incoming(&b, &ChatMessage::new(b, me, "from b".into())).unwrap();
        assert_eq!(st.load_history(&a, 10).unwrap().len(), 1);
        assert_eq!(st.load_history(&b, 10).unwrap().len(), 1);
        assert_eq!(st.list_chats().unwrap().len(), 1, "same prefix counts as one entry in the list");
    }

    #[test]
    fn a_reply_quote_survives_storage() {
        use crate::communication::ReplyRef;
        let dir = tempfile::tempdir().unwrap();
        let (me, peer) = (HashId([1; 32]), HashId([2; 32]));
        let st = ChatStorage::in_dir(me, dir.path().to_path_buf()).unwrap();
        let mut m = ChatMessage::new(me, peer, "ответ".into());
        m.reply_to = Some(ReplyRef { msg_id: "ab".repeat(32), text: "исходное".into() });
        st.save_outgoing(&peer, &m).unwrap();
        let got = st.load_history(&peer, 10).unwrap();
        assert_eq!(got[0].reply_to, m.reply_to);
    }

    #[test]
    fn a_reply_quote_is_cut_to_the_limit_and_a_bad_anchor_is_dropped() {
        use crate::communication::ReplyRef;
        let long = ReplyRef { msg_id: "AB".repeat(32), text: "я".repeat(500) }.sanitized().unwrap();
        assert_eq!(long.msg_id, "ab".repeat(32));
        assert_eq!(long.text.chars().count(), ReplyRef::MAX_TEXT_CHARS);
        assert!(ReplyRef { msg_id: "zz".into(), text: "x".into() }.sanitized().is_none());
    }

    #[test]
    fn a_peers_incoming_history_stops_growing_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let me = HashId([1; 32]);
        let peer = HashId([2; 32]);
        let st = ChatStorage::in_dir(me, dir.path().to_path_buf()).unwrap();
        let f = dir.path().join(format!("chat_{}.enc", hex::encode(peer.0)));
        std::fs::write(&f, vec![b'0'; (MAX_CHAT_FILE_BYTES + 1) as usize]).unwrap();
        assert!(st.save_incoming(&peer, &ChatMessage::new(peer, me, "x".into())).is_err());
    }

    #[test]
    fn a_status_update_keeps_the_order_of_the_history_and_a_repeated_one_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (me, peer) = (HashId([1; 32]), HashId([2; 32]));
        let st = ChatStorage::in_dir(me, dir.path().to_path_buf()).unwrap();
        let msgs: Vec<ChatMessage> = (0..5).map(|i| ChatMessage::new(peer, me, format!("m{i}"))).collect();
        for m in &msgs {
            st.save_incoming(&peer, m).unwrap();
        }
        let order = |st: &ChatStorage| st.load_history(&peer, usize::MAX).unwrap().iter().map(|m| m.text.clone()).collect::<Vec<_>>();
        let before = order(&st);
        for _ in 0..3 {
            st.update_message_status(&peer, &msgs[2].msg_id, MessageStatus::Read).unwrap();
            assert_eq!(order(&st), before, "a status update must not reorder the history");
        }
        let file = dir.path().join(format!("chat_{}.enc", hex::encode(peer.0)));
        let stamp = std::fs::metadata(&file).unwrap().modified().unwrap();
        st.update_message_status(&peer, &msgs[2].msg_id, MessageStatus::Read).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), stamp, "an unchanged status does not rewrite the file");
    }

    #[test]
    fn a_status_goes_to_the_log_not_into_the_history_and_the_log_folds_when_it_grows() {
        let dir = tempfile::tempdir().unwrap();
        let (me, peer) = (HashId([1; 32]), HashId([2; 32]));
        let st = ChatStorage::in_dir(me, dir.path().to_path_buf()).unwrap();
        let msgs: Vec<ChatMessage> = (0..20).map(|i| ChatMessage::new(me, peer, format!("m{i}"))).collect();
        for m in &msgs {
            st.save_outgoing(&peer, m).unwrap();
        }
        let file = dir.path().join(format!("chat_{}.enc", hex::encode(peer.0)));
        let log = dir.path().join(format!("chat_{}.status.enc", hex::encode(peer.0)));
        let main_before = std::fs::read(&file).unwrap();

        st.update_message_status(&peer, &msgs[3].msg_id, MessageStatus::Delivered).unwrap();
        st.update_message_status(&peer, &msgs[3].msg_id, MessageStatus::Read).unwrap();
        st.update_message_status(&peer, &msgs[7].msg_id, MessageStatus::Failed).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), main_before, "история не переписывалась");
        assert!(log.exists());
        let shown = st.load_history(&peer, usize::MAX).unwrap();
        let status_of = |id: &HashId| shown.iter().find(|m| m.msg_id == *id).unwrap().status.clone();
        assert_eq!(status_of(&msgs[3].msg_id), MessageStatus::Read);
        assert_eq!(status_of(&msgs[7].msg_id), MessageStatus::Failed);
        assert_eq!(shown.len(), 20);

        // растущий журнал сворачивается в историю: журнала нет, статусы на месте, порядок прежний
        for i in 0..2000 {
            let id = &msgs[i % 20].msg_id;
            let status = if i % 2 == 0 { MessageStatus::Delivered } else { MessageStatus::Read };
            st.update_message_status(&peer, id, status).unwrap();
        }
        assert!(!log.exists() || std::fs::metadata(&log).unwrap().len() <= 64 * 1024 + 1024);
        let after = st.load_history(&peer, usize::MAX).unwrap();
        assert_eq!(after.len(), 20);
        let texts: Vec<_> = after.iter().rev().map(|m| m.text.clone()).collect();
        assert_eq!(texts, (0..20).map(|i| format!("m{i}")).collect::<Vec<_>>());
        // удаление и очистка убирают журнал вместе с историей
        st.update_message_status(&peer, &msgs[1].msg_id, MessageStatus::Read).unwrap();
        assert!(log.exists());
        st.delete_message(&peer, &msgs[0].msg_id).unwrap();
        assert!(!log.exists(), "перезапись истории свернула журнал");
        st.update_message_status(&peer, &msgs[2].msg_id, MessageStatus::Read).unwrap();
        st.clear_history(&peer).unwrap();
        assert!(!log.exists() && !file.exists());
    }

    #[test]
    fn messages_are_never_lost_while_statuses_are_being_updated() {
        let dir = tempfile::tempdir().unwrap();
        let (me, peer) = (HashId([1; 32]), HashId([2; 32]));
        let st = Arc::new(ChatStorage::in_dir(me, dir.path().to_path_buf()).unwrap());
        let first = ChatMessage::new(peer, me, "first".into());
        st.save_incoming(&peer, &first).unwrap();

        const N: usize = 200;
        let writer = {
            let st = st.clone();
            std::thread::spawn(move || {
                for i in 0..N {
                    st.save_incoming(&peer, &ChatMessage::new(peer, me, format!("m{i}"))).unwrap();
                }
            })
        };
        let updaters: Vec<_> = (0..3).map(|_| {
            let (st, id) = (st.clone(), first.msg_id);
            std::thread::spawn(move || {
                for _ in 0..N {
                    st.update_message_status(&peer, &id, MessageStatus::Read).unwrap();
                }
            })
        }).collect();
        writer.join().unwrap();
        for u in updaters { u.join().unwrap(); }

        let got = st.load_history(&peer, usize::MAX).unwrap();
        let texts: std::collections::HashSet<_> = got.iter().map(|m| m.text.clone()).collect();
        assert_eq!(got.len(), N + 1, "потеряно сообщений: {}", N + 1 - got.len());
        for i in 0..N {
            assert!(texts.contains(&format!("m{i}")), "пропало m{i}");
        }
    }
}

#[cfg(test)]
mod closed_env_tests {
    use super::*;
    use crate::communication::ChatMessage;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("yandi-closed-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn history_older_than_a_year_is_removed_and_newer_kept_in_order() {
        let d = tempfile::tempdir().unwrap();
        let me = HashId([7u8; 32]);
        let st = ChatStorage::in_dir(me, d.path().to_path_buf()).unwrap();
        let peer = HashId([9u8; 32]);
        let now: u64 = 2_000_000_000_000;
        let day = 24 * 60 * 60 * 1000u64;
        let mut ids = Vec::new();
        for (i, age_days) in [400u64, 370, 200, 1].iter().enumerate() {
            let mut m = ChatMessage::new(peer, me, format!("m{i}"));
            m.timestamp = now - age_days * day;
            ids.push(m.msg_id);
            st.save_incoming(&peer, &m).unwrap();
        }
        assert_eq!(st.prune_expired(now).unwrap(), 2);
        let left = st.load_history(&peer, 10).unwrap();
        assert_eq!(left.len(), 2);
        // сначала новые (load_history отдаёт с конца), порядок сохранён
        assert_eq!(left[0].msg_id, ids[3]);
        assert_eq!(left[1].msg_id, ids[2]);
        assert_eq!(st.prune_expired(now).unwrap(), 0);
    }

    #[test]
    fn locked_storage_neither_reads_writes_nor_deletes() {
        let me = HashId::new_random();
        let peer = HashId::new_random();
        let d = dir("locked");
        let st = ChatStorage::in_dir_locked(me, d.clone()).unwrap();
        let msg = ChatMessage::new(peer, me, "секрет".into());
        assert!(st.save_incoming(&peer, &msg).is_err());
        assert!(st.load_history(&peer, 10).is_err());
        st.set_master([5; 32]);
        st.save_incoming(&peer, &msg).unwrap();
        let file = d.join(format!("chat_{}.enc", hex::encode(peer.0)));
        let before = std::fs::read(&file).unwrap();
        st.lock();
        assert!(st.load_history(&peer, 10).is_err());
        assert!(st.delete_message(&peer, &msg.msg_id).is_err());
        assert!(st.update_message_text(&peer, &msg.msg_id, "x".into()).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), before, "файл не изменён");
        st.set_master([5; 32]);
        assert_eq!(st.load_history(&peer, 10).unwrap().len(), 1);
    }

    #[test]
    fn rewrite_with_foreign_lines_refuses_instead_of_losing_them() {
        let me = HashId::new_random();
        let peer = HashId::new_random();
        let d = dir("mixed");
        let st = ChatStorage::in_dir(me, d.clone()).unwrap();
        st.set_master([1; 32]);
        let old = ChatMessage::new(peer, me, "под старым ключом".into());
        st.save_incoming(&peer, &old).unwrap();
        st.set_master([2; 32]);
        let new = ChatMessage::new(peer, me, "под новым".into());
        st.save_incoming(&peer, &new).unwrap();
        let file = d.join(format!("chat_{}.enc", hex::encode(peer.0)));
        let before = std::fs::read(&file).unwrap();
        // удаление «видимого» сообщения не должно потерять строку, которую этот ключ не открывает
        assert!(st.delete_message(&peer, &new.msg_id).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }

    #[test]
    fn a_torn_tail_does_not_swallow_the_records_appended_after_it() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.chat");
        let mut raw = Vec::from(&ChatStorage::MAGIC[..]);
        raw.extend_from_slice(&3u32.to_be_bytes());
        raw.extend_from_slice(b"one");
        raw.extend_from_slice(&100u32.to_be_bytes()); // оборванная запись: обещано 100 байт, записано 10
        raw.extend_from_slice(&[7u8; 10]);
        std::fs::write(&p, &raw).unwrap();
        ChatStorage::append_blob(&p, b"two").unwrap();
        ChatStorage::append_blob(&p, b"three").unwrap();
        let blobs = ChatStorage::read_blobs(&p).unwrap();
        assert_eq!(blobs, vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);
    }

    #[test]
    fn setting_the_password_moves_the_status_log_too() {
        let me = HashId::new_random();
        let a = HashId::new_random();
        let d = dir("rekey-status");
        let st = ChatStorage::in_dir(me, d.clone()).unwrap();
        st.set_master([7; 32]);
        let m = ChatMessage::new(me, a, "привет".into());
        st.save_outgoing(&a, &m).unwrap();
        st.update_message_status(&a, &m.msg_id, crate::communication::MessageStatus::Read).unwrap();
        assert!(d.join(format!("chat_{}.status.enc", hex::encode(a.0))).exists());
        assert_eq!(st.rekey_to(Some([7; 32]), [1; 32]).unwrap(), 0, "журнал статусов не считается сбойным файлом");
        let fresh = ChatStorage::in_dir_locked(me, d.clone()).unwrap();
        fresh.set_master([1; 32]);
        let h = fresh.load_history(&a, 10).unwrap();
        assert_eq!(h[0].status, crate::communication::MessageStatus::Read, "статус пережил перенос");
        fresh.update_message_status(&a, &m.msg_id, crate::communication::MessageStatus::Delivered).unwrap();
    }

    #[test]
    fn setting_the_password_moves_history_and_queue_from_the_working_key_and_keeps_unreadable_files() {
        let me = HashId::new_random();
        let (a, b) = (HashId::new_random(), HashId::new_random());
        let d = dir("rekey");
        let st = ChatStorage::in_dir(me, d.clone()).unwrap();
        st.set_master([7; 32]); // рабочий ключ машины, пока пароль не задан
        st.save_incoming(&a, &ChatMessage::new(a, me, "первое".into())).unwrap();
        st.save_incoming(&a, &ChatMessage::new(a, me, "второе".into())).unwrap();
        // чужой файл (ключ неизвестен)
        let alien = ChatStorage::in_dir(me, d.clone()).unwrap();
        alien.set_master([8; 32]);
        alien.save_incoming(&b, &ChatMessage::new(b, me, "чужое".into())).unwrap();
        let foreign = d.join(format!("chat_{}.enc", hex::encode(b.0)));
        let foreign_before = std::fs::read(&foreign).unwrap();

        assert_eq!(st.rekey_to(Some([7; 32]), [1; 32]).unwrap(), 1);
        assert_eq!(std::fs::read(&foreign).unwrap(), foreign_before);
        assert_eq!(st.load_history(&a, 10).unwrap().len(), 2, "перенесённый файл читается новым ключом");
        assert!(st.delete_message(&b, &HashId::new_random()).is_err(), "чужой файл не перезаписывается");
        std::fs::remove_file(&foreign).unwrap();
        assert_eq!(st.rekey_to(Some([7; 32]), [1; 32]).unwrap(), 0, "повтор");
        let fresh = ChatStorage::in_dir_locked(me, d.clone()).unwrap();
        fresh.set_master([1; 32]);
        assert_eq!(fresh.load_history(&a, 10).unwrap().len(), 2);
    }
}
