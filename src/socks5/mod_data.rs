// src/socks5/mod_data.rs
//! SOCKS5 Proxy Data Structures
//! ============================
//!
//! Analogous to HTTP Proxy structures (ProxyRequest, ProxyResponse, ProxyTunnelData)

use serde::{Deserialize, Serialize};

/// SOCKS5 CONNECT Request (аналог ProxyRequest из proxy/mod.rs)
///
/// Отправляется от SOCKS5 клиента к exit node для установки соединения
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Socks5ProxyRequest {
    /// Уникальный ID запроса (также используется как tunnel_id)
    pub request_id: u64,

    /// Целевой хост (domain или IP)
    pub target_host: String,

    /// Целевой порт
    pub target_port: u16,

    /// SOCKS5 команда (CONNECT, BIND, UDP ASSOCIATE)
    pub command: u8, // Socks5Command as u8
}

/// SOCKS5 CONNECT Response (аналог части ProxyResponse)
///
/// Отправляется от exit node обратно к клиенту
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Socks5ProxyResponse {
    /// Должен совпадать с request_id
    pub request_id: u64,

    /// Статус соединения (0 = успех, иначе код ошибки SOCKS5)
    pub status: u8,

    /// Привязанный адрес (опционально)
    pub bound_addr: Option<String>,
}

/// SOCKS5 Tunnel Data (аналог ProxyTunnelData из proxy/mod.rs)
///
/// Используется для би-направленной передачи данных в туннеле
#[derive(Debug, Clone)]
pub struct Socks5TunnelData {
    /// ID туннеля ( = request_id из Socks5ProxyRequest)
    pub tunnel_id: u64,

    /// Данные (RAW bytes - любые данные приложения)
    pub data: Vec<u8>,

    /// Флаг закрытия туннеля
    pub close: bool,
    /// Absolute byte offset in this direction; close carries the final offset.
    pub byte_offset: u64,
}

impl Socks5TunnelData {
    pub fn encode(&self) -> Vec<u8> {
        crate::netlayer::tunnel_wire::encode(crate::netlayer::tunnel_wire::SOCKS5,
            self.tunnel_id, self.byte_offset, self.close, &self.data)
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (tunnel_id, byte_offset, close, data) = crate::netlayer::tunnel_wire::decode(
            bytes, crate::netlayer::tunnel_wire::SOCKS5)?;
        Some(Self { tunnel_id, byte_offset, close, data: data.to_vec() })
    }
}

impl Socks5ProxyRequest {
    /// Создать новый CONNECT запрос
    pub fn new_connect(request_id: u64, target_host: String, target_port: u16) -> Self {
        Self {
            request_id,
            target_host,
            target_port,
            command: 0x01, // CONNECT
        }
    }

    /// Получить target как "host:port" строку
    pub fn target_addr(&self) -> String {
        format!("{}:{}", self.target_host, self.target_port)
    }
}

impl Socks5ProxyResponse {
    /// Создать успешный ответ
    pub fn success(request_id: u64) -> Self {
        Self {
            request_id,
            status: 0x00, // Success
            bound_addr: None,
        }
    }

    /// Создать ответ с ошибкой
    pub fn error(request_id: u64, error_code: u8) -> Self {
        Self {
            request_id,
            status: error_code,
            bound_addr: None,
        }
    }

    /// Проверить успешность
    pub fn is_success(&self) -> bool {
        self.status == 0x00
    }
}

impl Socks5TunnelData {
    /// Создать пакет с данными
    pub fn new(tunnel_id: u64, data: Vec<u8>) -> Self {
        Self {
            tunnel_id,
            data,
            close: false,
            byte_offset: 0,
        }
    }

    /// Создать пакет закрытия туннеля
    pub fn close(tunnel_id: u64) -> Self {
        Self {
            tunnel_id,
            data: Vec::new(),
            close: true,
            byte_offset: 0,
        }
    }
}
