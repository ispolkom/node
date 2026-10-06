// src/p2p_tunnel/types.rs
//! P2P Tunnel types

use crate::util::HashId;
use serde::{Serialize, Deserialize};

/// Тип P2P тоннеля (для какого сервиса)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelType {
    /// Голосовой звонок (VoIP)
    Voice = 1,

    /// Видеосвязь
    Video = 2,

    /// Передача файлов
    FileTransfer = 3,

    /// Игры P2P (низкая задержка)
    Gaming = 4,

    /// Универсальный P2P тоннель
    Generic = 5,
}

/// Кодек для голоса/видео
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecType {
    /// Opus для голоса (8-64 kbps)
    Opus,

    /// H264 для видео
    H264,

    /// VP8 для видео
    VP8,

    /// VP9 для видео
    VP9,

    /// RAW (без сжатия)
    Raw,
}

/// Запрос на создание тоннеля
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelRequest {
    pub tunnel_type: TunnelType,
    pub codec: Option<CodecType>,
    pub initiator: HashId,
    pub timestamp: u64,
}

/// Ответ на запрос тоннеля
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelResponse {
    pub accepted: bool,
    pub reason: Option<String>,
    pub responder: HashId,
    pub timestamp: u64,
}

/// Статус P2P тоннеля
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelStatus {
    /// Запрошен (отправлен request, ждём ответ)
    Requested,

    /// Установлен (both sides agreed)
    Established,

    /// Отклонён
    Rejected,

    /// Закрыт
    Closed,

    /// Ошибка
    Error,
}

/// Информация об активном тоннеле
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub tunnel_id: HashId,
    pub peer: HashId,
    pub tunnel_type: TunnelType,
    pub status: TunnelStatus,
    pub created_at: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}
