// src/communication/storage.rs
//! Local storage for chat history (encrypted JSON Lines format)

use crate::communication::ChatMessage;
use crate::util::HashId;
use std::path::PathBuf;
use anyhow::Result;
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
    /// прежний режим без мастер-ключа: ключ выводится из публичного node_id
    Legacy,
    Master([u8; 32]),
    Locked,
}

impl Drop for ChatStorage {
    fn drop(&mut self) {
        self.lock();
    }
}

/// Хранилище чатов (локальное для каждой ноды) с AES-256-GCM шифрованием
pub struct ChatStorage {
    my_node_id: HashId,
    chats_dir: PathBuf,
    /// Ключ чатов: прежний по node_id, ключ закрытой среды или «заперто» (ничего не читается и не пишется).
    key_mode: std::sync::RwLock<KeyMode>,
    /// Все изменения файла чата идут под одним замком: «прочитать всё, удалить файл, записать заново» (смена статуса, правка, удаление)
    /// не должно пересекаться с дописыванием нового сообщения, иначе оно пропадает из истории.
    write_lock: std::sync::Mutex<()>,
}

impl ChatStorage {
    /// Создать новое хранилище (legacy — key derived from node_id)
    pub fn new(my_node_id: HashId) -> Result<Self> {
        let base_dir = dirs::home_dir()
            .expect("No home directory")
            .join(".yandi/chats");

        std::fs::create_dir_all(&base_dir)?;

        Ok(Self {
            my_node_id,
            chats_dir: base_dir,
            key_mode: std::sync::RwLock::new(KeyMode::Legacy),
            write_lock: std::sync::Mutex::new(()),
        })
    }

    /// Хранилище в заданной папке (для проверок: они не должны писать в домашнюю папку владельца).
    #[cfg(test)]
    pub(crate) fn in_dir(my_node_id: HashId, dir: std::path::PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { my_node_id, chats_dir: dir, key_mode: std::sync::RwLock::new(KeyMode::Legacy), write_lock: std::sync::Mutex::new(()) })
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
    /// Legacy: raw node_id bytes (weak — node_id is public)
    fn get_encryption_key(&self) -> Result<[u8; 32]> {
        let mode = *self.key_mode.read().unwrap_or_else(|e| e.into_inner());
        match mode {
            KeyMode::Master(mk) => {
                let hk = Hkdf::<Sha256>::new(Some(&self.my_node_id.0), &mk);
                let mut key = [0u8; 32];
                hk.expand(b"yandi-chat-v2", &mut key).expect("HKDF expand failed");
                Ok(key)
            }
            KeyMode::Legacy => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&self.my_node_id.0[..32]);
                Ok(key)
            }
            KeyMode::Locked => Err(anyhow::anyhow!("закрытая среда заперта")),
        }
    }

    /// Ключ AES из мастер-ключа или (None) из публичного node_id, как в прежнем режиме.
    fn key_for(&self, master: Option<&[u8; 32]>) -> [u8; 32] {
        let mut key = [0u8; 32];
        match master {
            Some(mk) => {
                Hkdf::<Sha256>::new(Some(&self.my_node_id.0), mk)
                    .expand(b"yandi-chat-v2", &mut key)
                    .expect("HKDF expand failed");
            }
            None => key.copy_from_slice(&self.my_node_id.0[..32]),
        }
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

    /// Перенос всех файлов чатов и очереди на ключ закрытой среды и переключение хранилища на него. Идемпотентно: строка, уже
    /// открывающаяся новым ключом, остаётся; прочие открываются прежним мастер-ключом (`old`) или старым ключом из node_id.
    /// Файл с неоткрываемой строкой не меняется (счёт таких файлов возвращается).
    /// Всё под замком записи: параллельное дописывание не может вклиниться между проверкой и заменой.
    pub fn rekey_to(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> Result<usize> {
        let _g = self.locked();
        let candidates = {
            let mut c = vec![self.key_for(Some(&new))];
            if let Some(o) = old.as_ref() {
                c.push(self.key_for(Some(o)));
            }
            c.push(self.key_for(None));
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
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                };
                let mut changed = false;
                let mut out = Vec::new();
                let mut ok = true;
                for line in text.lines().filter(|l| !l.trim().is_empty()) {
                    match Self::open_with_any(&candidates, line) {
                        Some((0, _)) => out.push(line.trim().to_string()),
                        Some((_, plain)) => match Self::seal_line(&candidates[0], &plain) {
                            Ok(l) => {
                                changed = true;
                                out.push(l);
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
                let body = out.join("\n") + "\n";
                crate::util::private_file::write_private(&tmp, body.as_bytes())?;
                // проверка до замены: каждая строка новой копии открывается новым ключом
                let check = std::fs::read_to_string(&tmp)?;
                let all_new = check
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .all(|l| matches!(Self::open_with_any(&candidates[..1], l), Some((0, _))));
                if !all_new || check.lines().filter(|l| !l.trim().is_empty()).count() != out.len() {
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

    /// Получить путь к зашифрованному файлу чата
    /// The file is named by the FULL node id: with only a prefix, a peer whose id starts like another contact's would write into
    /// that contact's history. A history made under the old short name is adopted by the first peer that asks for it.
    fn chat_file_path_enc(&self, peer_id: &HashId) -> PathBuf {
        let full = self.chats_dir.join(format!("chat_{}.enc", hex::encode(peer_id.0)));
        if !full.exists() {
            let legacy = self.chats_dir.join(format!("chat_{}.enc", hex::encode(&peer_id.0[..8])));
            if legacy.exists() {
                let _ = std::fs::rename(&legacy, &full);
            }
        }
        full
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
        use std::fs::OpenOptions;
        use std::io::Write;

        let plaintext = serde_json::to_string(msg)?;
        let key_bytes = self.get_encryption_key()?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        let mut encrypted_data = nonce_bytes.to_vec();
        encrypted_data.extend_from_slice(&ciphertext);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(chat_file)?;

        writeln!(file, "{}", hex::encode(encrypted_data))?;
        Ok(())
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
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_slice()).map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;
        let mut out = nonce_bytes.to_vec();
        out.extend_from_slice(&ciphertext);
        crate::util::private_file::write_private(&path, hex::encode(out).as_bytes())?;
        Ok(())
    }

    pub fn load_outbox(&self) -> Vec<OutboxEntry> {
        let _g = self.locked();
        let Ok(text) = std::fs::read_to_string(self.chats_dir.join("outbox.enc")) else { return Vec::new() };
        let Ok(bytes) = hex::decode(text.trim()) else { return Vec::new() };
        if bytes.len() < 12 {
            return Vec::new();
        }
        let Ok(key_bytes) = self.get_encryption_key() else { return Vec::new() };
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
        match cipher.decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..]) {
            Ok(p) => serde_json::from_slice(&p).unwrap_or_default(),
            Err(_) => Vec::new(),
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

        let content = std::fs::read_to_string(chat_file)?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

        let mut messages = Vec::new();
        for line in content.lines().rev().take(limit) {
            let encrypted_hex = line.trim();
            if encrypted_hex.is_empty() {
                continue;
            }
            let opened = hex::decode(encrypted_hex)
                .ok()
                .filter(|b| b.len() >= 12)
                .and_then(|b| cipher.decrypt(Nonce::from_slice(&b[0..12]), &b[12..]).ok())
                .and_then(|p| serde_json::from_slice::<ChatMessage>(&p).ok());
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
    fn two_peers_with_the_same_id_prefix_get_separate_histories_and_old_files_are_adopted() {
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
        // a history written under the old short name is picked up by the full name
        let c = HashId([5; 32]);
        std::fs::write(dir.path().join(format!("chat_{}.enc", hex::encode(&c.0[..8]))), b"").unwrap();
        let _ = st.load_history(&c, 10);
        assert!(dir.path().join(format!("chat_{}.enc", hex::encode(c.0))).exists());
        assert!(!dir.path().join(format!("chat_{}.enc", hex::encode(&c.0[..8]))).exists());
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
    fn rekey_moves_old_master_and_legacy_lines_and_keeps_unreadable_files() {
        let me = HashId::new_random();
        let (a, b) = (HashId::new_random(), HashId::new_random());
        let d = dir("rekey");
        let st = ChatStorage::in_dir(me, d.clone()).unwrap();
        // legacy (ключ из node_id) и под прежним мастер-ключом, в одном файле
        let m1 = ChatMessage::new(a, me, "старое".into());
        st.save_incoming(&a, &m1).unwrap();
        st.set_master([7; 32]);
        let m2 = ChatMessage::new(a, me, "под мастером".into());
        st.save_incoming(&a, &m2).unwrap();
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
        assert_eq!(st.rekey_to(Some([7; 32]), [1; 32]).unwrap(), 0);
        assert_eq!(st.load_history(&a, 10).unwrap().len(), 2);
        let fresh = ChatStorage::in_dir_locked(me, d.clone()).unwrap();
        fresh.set_master([1; 32]);
        assert_eq!(fresh.load_history(&a, 10).unwrap().len(), 2);
        assert_eq!(st.rekey_to(Some([7; 32]), [1; 32]).unwrap(), 0, "повтор");
    }
}
