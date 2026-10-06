// src/communication/types.rs
//! Communication types

use crate::util::HashId;
use serde::{Serialize, Deserialize};

/// Статус сообщения
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageStatus {
    Pending,     // Ожидает доставки (peer offline)
    Shipping,    // В процессе отправки
    Delivered,   // Доставлено
    Failed,      // Ошибка доставки
    Read,        // Прочитано
}

/// Текстовое сообщение чата
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub msg_id: HashId,
    pub from: HashId,
    pub to: HashId,
    pub timestamp: u64,
    pub text: String,
    pub encrypted: bool,
    pub status: MessageStatus,
    pub edited: bool,
    pub edit_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<FileAttachment>,
}

/// Файл прикрепленный к сообщению
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAttachment {
    pub filename: String,
    pub size: u64,
    #[serde(rename = "mime_type")]
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>, // Base64 encoded (для превью/маленьких файлов)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_ref: Option<FileReference>, // Для файлов до 100 MB (чанкование в чате)
}

/// Ссылка на файл для чанкованной загрузки
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReference {
    pub file_id: String,     // Уникальный ID файла
    pub total_chunks: u32,   // Общее количество чанков
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_name: Option<String>, // Локальное имя файла на диске
}

/// Метаданные файла для начала чанкованной передачи
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunkStart {
    pub file_id: String,        // Уникальный ID файла
    pub filename: String,
    pub file_size: u64,
    pub mime_type: String,
    pub total_chunks: u32,      // Общее количество чанков
}

/// Один чанк файла
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    pub file_id: String,        // ID файла
    pub chunk_index: u32,       // Номер чанка (0-based)
    pub chunk_data: Vec<u8>,     // Base64 encoded chunk data
}

/// Диапазон чанков
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunkRange {
    pub start: u32,
    pub end: u32,
}

/// Список недостающих чанков после прохода
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMissing {
    pub file_id: String,
    pub missing_ranges: Vec<FileChunkRange>,
}

/// Сигнал завершения прохода передачи файла
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunkEnd {
    pub file_id: String,
}

/// Файл получен полностью и собран
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTransferComplete {
    pub file_id: String,
}

impl ChatMessage {
    /// Создать новое сообщение
    pub fn new(from: HashId, to: HashId, text: String) -> Self {
        Self {
            msg_id: HashId::new_random(),
            from,
            to,
            timestamp: now_ms(),
            text,
            encrypted: false, // Будет зашифровано перед отправкой
            status: MessageStatus::Pending,
            edited: false,
            edit_timestamp: None,
            attachment: None,
        }
    }

    /// Отметить как доставленное
    pub fn mark_delivered(&mut self) {
        self.status = MessageStatus::Delivered;
    }

    /// Отметить как прочитанное
    pub fn mark_read(&mut self) {
        self.status = MessageStatus::Read;
    }
}

/// Оффлайн сообщение для доставки
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessage {
    pub msg_id: HashId,
    pub to: HashId,
    pub message: ChatMessage,
    pub created_at: u64,
    pub shipped_at: Option<u64>,
    pub delivered_at: Option<u64>,
    pub delivery_attempts: u32,
    pub status: MessageStatus,
}

/// Тип контента сообщения (для future использования)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageContent {
    Text(String),
    Image(ImageMetadata),
    Link(LinkPreview),
    File(FileMetadata),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMetadata {
    pub thumbnail: String, // base64
    pub width: u32,
    pub height: u32,
    pub size: usize,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkPreview {
    pub url: String,
    pub title: String,
    pub description: String,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub filename: String,
    pub size: usize,
    pub file_hash: Vec<u8>,
}

/// Получить текущий timestamp в ms
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
