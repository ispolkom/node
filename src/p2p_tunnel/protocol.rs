// src/p2p_tunnel/protocol.rs
//! P2P Tunnel control protocol

use crate::p2p_tunnel::{TunnelRequest, TunnelResponse};
use serde::{Serialize, Deserialize};

/// Контрольные пакеты для P2P тоннеля (по аналогии с 0x20, 0x30, 0x40)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2PTunnelPacket {
    // P2P Tunnel control (0x80-0x8F)
    TunnelRequest = 0x80,    // Запрос на создание тоннеля
    TunnelAccept = 0x81,    // Принятие запроса
    TunnelReject = 0x82,    // Отклонение запроса
    TunnelClose = 0x83,     // Закрытие тоннеля
    TunnelData = 0x84,      // Данные тоннеля (voice/video/file)
    TunnelPing = 0x85,      // Проверка живости тоннеля
    TunnelPong = 0x86,      // Ответ на ping
}

impl P2PTunnelPacket {
    /// Преобразовать в байт
    pub fn as_byte(&self) -> u8 {
        *self as u8
    }

    /// Из байта
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x80 => Some(Self::TunnelRequest),
            0x81 => Some(Self::TunnelAccept),
            0x82 => Some(Self::TunnelReject),
            0x83 => Some(Self::TunnelClose),
            0x84 => Some(Self::TunnelData),
            0x85 => Some(Self::TunnelPing),
            0x86 => Some(Self::TunnelPong),
            _ => None,
        }
    }
}

/// Сериализованное сообщение для тоннеля
#[derive(Debug, Clone)]
pub struct TunnelPacket {
    pub packet_type: P2PTunnelPacket,
    pub data: Vec<u8>,
}

impl TunnelPacket {
    /// Создать пакет с данными
    pub fn new(packet_type: P2PTunnelPacket, data: Vec<u8>) -> Self {
        Self { packet_type, data }
    }

    /// Упаковать в байты для отправки
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![self.packet_type.as_byte()];
        bytes.extend_from_slice(&self.data);
        bytes
    }

    /// Распаковать из байтов
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }

        let packet_type = P2PTunnelPacket::from_byte(bytes[0])?;
        let data = bytes[1..].to_vec();

        Some(Self { packet_type, data })
    }

    /// Создать TunnelRequest пакет
    pub fn tunnel_request(req: &TunnelRequest) -> Result<Self, serde_json::Error> {
        let data = serde_json::to_vec(req)?;
        Ok(Self::new(P2PTunnelPacket::TunnelRequest, data))
    }

    /// Создать TunnelResponse пакет
    pub fn tunnel_response(resp: &TunnelResponse) -> Result<Self, serde_json::Error> {
        let data = serde_json::to_vec(resp)?;
        Ok(Self::new(P2PTunnelPacket::TunnelAccept, resp.accepted.then_some(data).unwrap_or_default()))
    }

    /// Создать TunnelClose пакет
    pub fn tunnel_close(reason: Option<&str>) -> Self {
        let data = reason.unwrap_or("").as_bytes().to_vec();
        Self::new(P2PTunnelPacket::TunnelClose, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_roundtrip() {
        let original = TunnelPacket {
            packet_type: P2PTunnelPacket::TunnelRequest,
            data: b"test".to_vec(),
        };

        let bytes = original.to_bytes();
        let decoded = TunnelPacket::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.packet_type, original.packet_type);
        assert_eq!(decoded.data, original.data);
    }
}
