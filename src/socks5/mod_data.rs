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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Socks5TunnelData {
    /// ID туннеля ( = request_id из Socks5ProxyRequest)
    pub tunnel_id: u64,

    /// Данные (RAW bytes - любые данные приложения)
    pub data: Vec<u8>,

    /// Флаг закрытия туннеля
    pub close: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
}

impl Socks5TunnelData {
    const COMPACT_MAGIC: u8 = 0xD7;

    /// Compact form for the wire: `[0xD7][tunnel_id: u64 BE][close: u8][data...]`. JSON writes every byte of the data as a decimal number
    /// (three to four times the size, and slow to produce), so the bulk of the traffic of an exit used to cost three to four times the wagons.
    /// The first byte tells the forms apart: JSON begins with `{`.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(26 + self.data.len());
        v.push(if self.byte_offset.is_some() { 0xD8 } else { Self::COMPACT_MAGIC });
        v.extend_from_slice(&self.tunnel_id.to_be_bytes());
        if let Some(offset) = self.byte_offset {
            // Prevent the untyped train dispatcher from mistaking this for legacy
            // HTTP bincode (whose Vec length occupies bytes 8..16, little endian).
            v.extend_from_slice(&[0xFF; 8]);
            v.extend_from_slice(&offset.to_be_bytes());
        }
        v.push(self.close as u8);
        v.extend_from_slice(&self.data);
        v
    }

    /// Either form; `None` for anything else (the caller then tries other message types).
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.first() == Some(&0xD8) {
            if bytes.len() < 26 || bytes[25] > 1 || bytes[9..17] != [0xFF; 8] { return None; }
            return Some(Self {
                tunnel_id: u64::from_be_bytes(bytes[1..9].try_into().ok()?),
                close: bytes[25] == 1, data: bytes[26..].to_vec(),
                byte_offset: Some(u64::from_be_bytes(bytes[17..25].try_into().ok()?)),
            });
        }
        if bytes.first() == Some(&Self::COMPACT_MAGIC) {
            if bytes.len() < 10 || bytes[9] > 1 {
                return None;
            }
            return Some(Self { tunnel_id: u64::from_be_bytes(bytes[1..9].try_into().ok()?), byte_offset: None, close: bytes[9] == 1, data: bytes[10..].to_vec() });
        }
        serde_json::from_slice(bytes).ok()
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
            byte_offset: None,
        }
    }

    /// Создать пакет закрытия туннеля
    pub fn close(tunnel_id: u64) -> Self {
        Self {
            tunnel_id,
            data: Vec::new(),
            close: true,
            byte_offset: None,
        }
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    #[test]
    fn the_compact_form_round_trips_and_is_far_smaller_than_json() {
        let d = Socks5TunnelData::new(0x0102030405060708, (0..16384u32).map(|i| (i * 7) as u8).collect());
        let c = d.encode();
        assert_eq!(c.len(), 16384 + 10);
        let json = serde_json::to_vec(&d).unwrap();
        assert!(json.len() > 3 * c.len() / 2 * 2 - 1000, "json is about three to four times bigger ({} vs {})", json.len(), c.len());
        let back = Socks5TunnelData::parse(&c).unwrap();
        assert_eq!((back.tunnel_id, back.close, back.data), (d.tunnel_id, d.close, d.data.clone()));
        let close = Socks5TunnelData::close(9);
        let b = Socks5TunnelData::parse(&close.encode()).unwrap();
        assert!(b.close && b.tunnel_id == 9 && b.data.is_empty());
    }

    #[test]
    fn both_forms_are_read_and_broken_input_is_refused() {
        let d = Socks5TunnelData::new(5, vec![1, 2, 3]);
        assert_eq!(Socks5TunnelData::parse(&serde_json::to_vec(&d).unwrap()).unwrap().data, vec![1, 2, 3], "the old JSON form still parses");
        assert!(Socks5TunnelData::parse(&[0xD7]).is_none());
        assert!(Socks5TunnelData::parse(&[0xD7, 0, 0, 0, 0, 0, 0, 0, 1]).is_none(), "too short");
        let mut bad = d.encode();
        bad[9] = 2;
        assert!(Socks5TunnelData::parse(&bad).is_none(), "the close flag is 0 or 1");
        assert!(Socks5TunnelData::parse(b"not json").is_none());
        assert!(Socks5TunnelData::parse(&[]).is_none());
    }
}

#[cfg(test)]
mod stream_offset_tests {
    use super::*;
    use crate::netlayer::{packet::hello_caps, stream_order::Cursor};
    #[test]
    fn capability_selects_wire_and_both_generations_parse() {
        for caps in [0, hello_caps::TELESCOPING_HANDSHAKE, hello_caps::STREAM_OFFSET, u16::MAX] {
            let ordered = hello_caps::supports_stream_offsets(caps);
            let mut cursor = Cursor::new(ordered);
            let mut d = Socks5TunnelData::new(42, vec![0;64]);
            d.byte_offset = cursor.block(d.data.len()).unwrap();
            let wire = d.encode();
            assert_eq!(wire[0], if ordered { 0xD8 } else { 0xD7 });
            let parsed = Socks5TunnelData::parse(&wire).unwrap();
            assert_eq!(parsed.byte_offset, ordered.then_some(0));
            assert_eq!(parsed.data, d.data);
            if ordered {
                assert_eq!(&wire[9..17], &[0xFF;8]);
                // Dispatch checks HTTP bincode first. New SOCKS must never match it.
                assert!(crate::proxy::ProxyTunnelData::from_bincode(&wire).is_err());
                assert_eq!(wire.len(), 26+64);
            } else {
                assert_eq!(wire.len(), 10+64);
            }
            let mut close = Socks5TunnelData::close(42); close.byte_offset = cursor.end();
            let parsed = Socks5TunnelData::parse(&close.encode()).unwrap();
            assert_eq!(parsed.byte_offset, ordered.then_some(64)); assert!(parsed.close);
        }
    }
    #[test]
    fn broken_offset_headers_rejected() {
        let mut d = Socks5TunnelData::new(1, vec![]); d.byte_offset = Some(u64::MAX);
        let mut wire = d.encode();
        for len in 0..26 { assert!(Socks5TunnelData::parse(&wire[..len]).is_none()); }
        assert_eq!(Socks5TunnelData::parse(&wire).unwrap().byte_offset, Some(u64::MAX));
        wire[25] = 2; assert!(Socks5TunnelData::parse(&wire).is_none());
        wire[25] = 0; wire[9] = 0; assert!(Socks5TunnelData::parse(&wire).is_none());
    }
}
