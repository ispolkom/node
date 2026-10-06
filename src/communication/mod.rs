// src/communication/mod.rs
//! P2P Communication Module
//! ========================
//!
//! Децентрализованные P2P коммуникации:
//! - 💬 Текстовый чат
//! - 📞 Голосовые звонки (TODO)
//! - 📹 Видеозвонки (TODO)
//! - 📎 Передача файлов (TODO)
//!
//! ## Архитектура
//!
//! **Хранение**: Каждая нода хранит свою копию переписки
//! - ~/.yandi/chats/chat_PEERID.jsonl - история чата
//! - ~/.yandi/files/incoming/ - полученные файлы
//!
//! **Шифрование**: E2E для каждой пары peers
//!
//! **Оффлайн сообщения**: Pending → Shipped → Delivered

pub mod types;
pub mod storage;
pub mod groups;
pub mod protocol;
pub mod chat;
pub mod encryption;
pub mod file_transfer;

pub use types::*;
pub use storage::*;
pub use protocol::*;
pub use protocol::{GroupPacket, GroupControlPacket};
pub use chat::*;
pub use encryption::*;
pub use file_transfer::*;

/// Папка файлов переписки этого узла: `uploads` (отправляемые), `downloads` (принятые), `cache` (части при передаче). Лежит в папке
/// данных узла (у каждого пользователя и у каждой копии тренировочной сети — своя); раньше была жёстко `/home/iam/yandi/…`.
pub fn files_dir(kind: &str) -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("files").join(kind)
}
