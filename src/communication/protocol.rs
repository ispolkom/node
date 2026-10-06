// src/communication/protocol.rs
//! Communication control packets

/// Контрольные пакеты для коммуникаций (по аналогии с 0x20, 0x30)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommControlPacket {
    // Чат (0xA0-0xAF) - избегаем конфликта с Proxy (0x40-0x4F)
    ChatMessage = 0xA0,        // Текстовое сообщение
    ChatAck = 0xA1,            // Подтверждение получения
    ChatRead = 0xA2,           // Прочитано (Read Receipt)
    ChatTyping = 0xA3,         // Печатает...
    ChatDeleteMessage = 0xA4,  // Удалить сообщение

    // Голос (0xB0-0xBF) - TODO
    VoiceCallRequest = 0xB0,
    VoiceCallAccept = 0xB1,
    VoiceCallEnd = 0xB2,
    VoiceCallReject = 0xB3,
    VoiceData = 0xB4,

    // Видео (0xC0-0xCF) - TODO
    VideoCallRequest = 0xC0,
    VideoCallAccept = 0xC1,
    VideoCallEnd = 0xC2,
    VideoCallReject = 0xC3,
    VideoData = 0xC4,

    // Файлы (0xD0-0xDF) - TODO
    FileTransferStart = 0xD0,
    FileChunk = 0xD1,
    FileTransferEnd = 0xD2,
    FileTransferCancel = 0xD3,
    FileMissing = 0xD4,
    FileComplete = 0xD5,
}

impl CommControlPacket {
    /// Преобразовать в байт
    pub fn as_byte(&self) -> u8 {
        *self as u8
    }

    /// Из байта
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0xA0 => Some(Self::ChatMessage),
            0xA1 => Some(Self::ChatAck),
            0xA2 => Some(Self::ChatRead),
            0xA3 => Some(Self::ChatTyping),

            0xB0 => Some(Self::VoiceCallRequest),
            0xB1 => Some(Self::VoiceCallAccept),
            0xB2 => Some(Self::VoiceCallEnd),
            0xB3 => Some(Self::VoiceCallReject),
            0xB4 => Some(Self::VoiceData),

            0xC0 => Some(Self::VideoCallRequest),
            0xC1 => Some(Self::VideoCallAccept),
            0xC2 => Some(Self::VideoCallEnd),
            0xC3 => Some(Self::VideoCallReject),
            0xC4 => Some(Self::VideoData),

            0xD0 => Some(Self::FileTransferStart),
            0xD1 => Some(Self::FileChunk),
            0xD2 => Some(Self::FileTransferEnd),
            0xD3 => Some(Self::FileTransferCancel),
            0xD4 => Some(Self::FileMissing),
            0xD5 => Some(Self::FileComplete),

            _ => None,
        }
    }
}

/// Сериализованное сообщение для передачи через P2P
#[derive(Debug, Clone)]
pub struct CommPacket {
    pub packet_type: CommControlPacket,
    pub data: Vec<u8>,
}

impl CommPacket {
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

        let packet_type = CommControlPacket::from_byte(bytes[0])?;
        let data = bytes[1..].to_vec();

        Some(Self { packet_type, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_roundtrip() {
        let original = CommPacket {
            packet_type: CommControlPacket::ChatMessage,
            data: b"Hello, world!".to_vec(),
        };

        let bytes = original.to_bytes();
        let decoded = CommPacket::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.packet_type, original.packet_type);
        assert_eq!(decoded.data, original.data);
    }
}

// Групповые сообщения (0xE0-0xEF)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupControlPacket {
    GroupMessage = 0xE0,        // Сообщение в группу
    GroupMessageAck = 0xE1,     // Подтверждение получения
    GroupInvite = 0xE2,         // Приглашение в группу
    GroupInviteAccept = 0xE3,   // Принятие приглашения
    GroupInviteReject = 0xE4,   // Отклонение приглашения
    GroupMemberJoin = 0xE5,     // Участник присоединился
    GroupMemberLeave = 0xE6,    // Участник вышел
    GroupMemberKick = 0xE7,     // Участник исключен
    GroupRoleChange = 0xE8,     // Смена роли
    GroupSettingsUpdate = 0xE9, // Обновление настроек
}

impl GroupControlPacket {
    pub fn as_byte(&self) -> u8 {
        *self as u8
    }

    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0xE0 => Some(Self::GroupMessage),
            0xE1 => Some(Self::GroupMessageAck),
            0xE2 => Some(Self::GroupInvite),
            0xE3 => Some(Self::GroupInviteAccept),
            0xE4 => Some(Self::GroupInviteReject),
            0xE5 => Some(Self::GroupMemberJoin),
            0xE6 => Some(Self::GroupMemberLeave),
            0xE7 => Some(Self::GroupMemberKick),
            0xE8 => Some(Self::GroupRoleChange),
            0xE9 => Some(Self::GroupSettingsUpdate),
            _ => None,
        }
    }
}

/// Групповое сообщение для передачи через P2P
#[derive(Debug, Clone)]
pub struct GroupPacket {
    pub packet_type: GroupControlPacket,
    pub data: Vec<u8>,
}

impl GroupPacket {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![self.packet_type.as_byte()];
        bytes.extend_from_slice(&self.data);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let packet_type = GroupControlPacket::from_byte(bytes[0])?;
        let data = bytes[1..].to_vec();
        Some(Self { packet_type, data })
    }
}
