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

/// Largest history file accepted from one peer's incoming messages.
const MAX_CHAT_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Хранилище чатов (локальное для каждой ноды) с AES-256-GCM шифрованием
pub struct ChatStorage {
    my_node_id: HashId,
    chats_dir: PathBuf,
    /// Master key for HKDF-derived chat encryption (None = legacy node_id-based key)
    master_key: Option<[u8; 32]>,
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
            master_key: None,
            write_lock: std::sync::Mutex::new(()),
        })
    }

    /// Хранилище в заданной папке (для проверок: они не должны писать в домашнюю папку владельца).
    #[cfg(test)]
    pub(crate) fn in_dir(my_node_id: HashId, dir: std::path::PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { my_node_id, chats_dir: dir, master_key: None, write_lock: std::sync::Mutex::new(()) })
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
            master_key: Some(master_key),
            write_lock: std::sync::Mutex::new(()),
        })
    }

    /// Get AES-256-GCM encryption key.
    /// With master_key: HKDF-SHA256(master_key, salt=node_id, info="yandi-chat-v2")
    /// Without master_key: raw node_id bytes (legacy, weak — node_id is public)
    fn get_encryption_key(&self) -> [u8; 32] {
        if let Some(mk) = &self.master_key {
            let hk = Hkdf::<Sha256>::new(Some(&self.my_node_id.0), mk);
            let mut key = [0u8; 32];
            hk.expand(b"yandi-chat-v2", &mut key).expect("HKDF expand failed");
            key
        } else {
            let mut key = [0u8; 32];
            key.copy_from_slice(&self.my_node_id.0[..32]);
            key
        }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, ()> {
        // замок защищает только порядок операций с файлами; отравление (паника в другой задаче) данных не портит
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
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
        let key_bytes = self.get_encryption_key();
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

    /// Загрузить историю чата (расшифрованную)
    pub fn load_history(&self, peer_id: &HashId, limit: usize) -> Result<Vec<ChatMessage>> {
        let _g = self.locked();
        self.load_history_unlocked(peer_id, limit)
    }

    fn load_history_unlocked(&self, peer_id: &HashId, limit: usize) -> Result<Vec<ChatMessage>> {
        let chat_file = self.chat_file_path_enc(peer_id);

        if !chat_file.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(chat_file)?;
        let key_bytes = self.get_encryption_key();
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

        let mut messages = Vec::new();
        for line in content.lines().rev().take(limit) {
            let encrypted_hex = line.trim();
            let encrypted_bytes = match hex::decode(encrypted_hex) {
                Ok(b) => b,
                Err(_) => continue,
            };

            if encrypted_bytes.len() < 12 {
                continue;
            }

            let nonce = Nonce::from_slice(&encrypted_bytes[0..12]);
            let ciphertext = &encrypted_bytes[12..];

            let plaintext = match cipher.decrypt(nonce, ciphertext) {
                Ok(p) => p,
                Err(_) => continue,
            };

            if let Ok(msg) = serde_json::from_slice(&plaintext) {
                messages.push(msg);
            }
        }

        Ok(messages)
    }

    /// Обновить статус сообщения (перезаписывает весь файл с шифрованием)
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

        let mut messages = self.load_history_unlocked(peer_id, usize::MAX)?;
        let mut updated = false;
        
        for msg in messages.iter_mut() {
            if msg.msg_id == *msg_id {
                msg.status = status;
                updated = true;
                break;
            }
        }

        if updated {
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
        for msg in messages {
            self.append_encrypted_message(&tmp, msg)?;
        }
        if messages.is_empty() {
            let _ = std::fs::remove_file(&tmp);
            if chat_file.exists() {
                std::fs::remove_file(&chat_file)?;
            }
            return Ok(());
        }
        std::fs::rename(&tmp, &chat_file)?;
        Ok(())
    }

    /// Очистить историю чата с конкретным peer
    pub fn clear_history(&self, peer_id: &HashId) -> Result<()> {
        let _g = self.locked();
        let chat_file = self.chat_file_path_enc(peer_id);
        if chat_file.exists() {
            std::fs::remove_file(chat_file)?;
        }
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
        let mut messages = self.load_history_unlocked(peer_id, usize::MAX)?;
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
        let messages = self.load_history_unlocked(peer_id, usize::MAX)?;
        let filtered: Vec<ChatMessage> = messages
            .into_iter()
            .filter(|msg| msg.msg_id != *msg_id)
            .collect();

        self.rewrite_all_messages(peer_id, &filtered)
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
