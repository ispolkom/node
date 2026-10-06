// src/socks5/proxy_protocol.rs
//! SOCKS5 Proxy Protocol over Stream Layer
//! ========================================
//!
//! Управляющий протокол для передачи destination:port через stream

use anyhow::{Result, anyhow};
use crate::socks5::protocol::Socks5Address;
use std::net::Ipv4Addr;

/// Типы сообщений прокси протокола
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProxyMsgType {
    ConnectRequest = 0x20,   // Запрос на подключение к destination:port
    ConnectResponse = 0x21,  // Ответ: успешно/ошибка
    Data = 0x22,             // Данные туннеля
    Close = 0x23,            // Закрытие туннеля
}

impl ProxyMsgType {
    pub fn from_byte(b: u8) -> Result<Self> {
        match b {
            0x20 => Ok(ProxyMsgType::ConnectRequest),
            0x21 => Ok(ProxyMsgType::ConnectResponse),
            0x22 => Ok(ProxyMsgType::Data),
            0x23 => Ok(ProxyMsgType::Close),
            _ => Err(anyhow!("Invalid proxy message type: {}", b)),
        }
    }

    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Коды ответа для ConnectResponse
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProxyConnectResult {
    Success = 0x00,
    GeneralFailure = 0x01,
    ConnectionNotAllowed = 0x02,
    NetworkUnreachable = 0x03,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    TtlExpired = 0x06,
}

impl ProxyConnectResult {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0x00 => ProxyConnectResult::Success,
            0x01 => ProxyConnectResult::GeneralFailure,
            0x02 => ProxyConnectResult::ConnectionNotAllowed,
            0x03 => ProxyConnectResult::NetworkUnreachable,
            0x04 => ProxyConnectResult::HostUnreachable,
            0x05 => ProxyConnectResult::ConnectionRefused,
            0x06 => ProxyConnectResult::TtlExpired,
            _ => ProxyConnectResult::GeneralFailure,
        }
    }

    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Connect Request: [TYPE:1][ADDR_TYPE:1][ADDR_DATA...]
#[derive(Debug, Clone)]
pub struct ConnectRequest {
    pub address: Socks5Address,
}

impl ConnectRequest {
    /// Максимальный размер ConnectRequest
    pub const MAX_SIZE: usize = 1 + 1 + 256 + 2; // TYPE + TYPE + domain(256) + port

    /// Создать новый запрос
    pub fn new(address: Socks5Address) -> Self {
        Self { address }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![ProxyMsgType::ConnectRequest.to_byte()];
        out.extend_from_slice(&self.address.to_bytes());
        out
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            return Err(anyhow!("Empty ConnectRequest"));
        }

        let msg_type = ProxyMsgType::from_byte(data[0])?;
        if msg_type != ProxyMsgType::ConnectRequest {
            return Err(anyhow!("Invalid message type: {:?}", msg_type));
        }

        if data.len() < 2 {
            return Err(anyhow!("ConnectRequest too short"));
        }

        let addr_type = crate::socks5::protocol::Socks5AddressType::from_byte(data[1])?;
        let (address, _) = Socks5Address::from_bytes(addr_type, &data[2..])?;

        Ok(Self { address })
    }
}

/// Connect Response: [TYPE:1][RESULT:1]
#[derive(Debug, Clone)]
pub struct ConnectResponse {
    pub result: ProxyConnectResult,
}

impl ConnectResponse {
    pub const SIZE: usize = 2;

    /// Создать ответ
    pub fn new(result: ProxyConnectResult) -> Self {
        Self { result }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        [ProxyMsgType::ConnectResponse.to_byte(), self.result.to_byte()]
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < Self::SIZE {
            return Err(anyhow!("ConnectResponse too short"));
        }

        let msg_type = ProxyMsgType::from_byte(data[0])?;
        if msg_type != ProxyMsgType::ConnectResponse {
            return Err(anyhow!("Invalid message type: {:?}", msg_type));
        }

        let result = ProxyConnectResult::from_byte(data[1]);

        Ok(Self { result })
    }
}

/// Data message: [TYPE:1][LEN:2][DATA:LEN]
#[derive(Debug, Clone)]
pub struct DataMessage {
    pub data: Vec<u8>,
}

impl DataMessage {
    /// Максимальный размер данных в одном сообщении
    pub const MAX_DATA_SIZE: usize = 4096;

    /// Создать сообщение с данными
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let len = self.data.len() as u16;
        let mut out = vec![ProxyMsgType::Data.to_byte()];
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 3 {
            return Err(anyhow!("DataMessage too short"));
        }

        let msg_type = ProxyMsgType::from_byte(data[0])?;
        if msg_type != ProxyMsgType::Data {
            return Err(anyhow!("Invalid message type: {:?}", msg_type));
        }

        let len = u16::from_be_bytes([data[1], data[2]]) as usize;

        if data.len() < 3 + len {
            return Err(anyhow!("DataMessage truncated: expected {} bytes, got {}", len, data.len() - 3));
        }

        Ok(Self {
            data: data[3..3+len].to_vec(),
        })
    }
}

/// Close message: [TYPE:1]
#[derive(Debug, Clone, Copy)]
pub struct CloseMessage;

impl CloseMessage {
    pub const SIZE: usize = 1;

    /// Serialize to bytes
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        [ProxyMsgType::Close.to_byte()]
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            return Err(anyhow!("CloseMessage empty"));
        }

        let msg_type = ProxyMsgType::from_byte(data[0])?;
        if msg_type != ProxyMsgType::Close {
            return Err(anyhow!("Invalid message type: {:?}", msg_type));
        }

        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_request_ipv4() {
        let addr = Socks5Address::Ipv4(Ipv4Addr::new(127, 0, 0, 1), 8080);
        let req = ConnectRequest::new(addr.clone());

        let bytes = req.to_bytes();
        let parsed = ConnectRequest::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.address, addr);
    }

    #[test]
    fn test_connect_response() {
        let resp = ConnectResponse::new(ProxyConnectResult::Success);
        let bytes = resp.to_bytes();

        assert_eq!(bytes.len(), ConnectResponse::SIZE);
        assert_eq!(bytes[0], ProxyMsgType::ConnectResponse.to_byte());
        assert_eq!(bytes[1], ProxyConnectResult::Success.to_byte());

        let parsed = ConnectResponse::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.result, ProxyConnectResult::Success);
    }

    #[test]
    fn test_data_message() {
        let data = b"Hello, World!".to_vec();
        let msg = DataMessage::new(data.clone());

        let bytes = msg.to_bytes();
        let parsed = DataMessage::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.data, data);
    }

    #[test]
    fn test_close_message() {
        let msg = CloseMessage;
        let bytes = msg.to_bytes();

        assert_eq!(bytes.len(), CloseMessage::SIZE);

        let parsed = CloseMessage::from_bytes(&bytes).unwrap();
        let _ = parsed;
    }
}
