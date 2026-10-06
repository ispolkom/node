// src/p2p/mod.rs
//!
//! # P2P Transport Layer
//!
//! Выделенный транспорт для P2P коммуникаций с большими пакетами:
//! - 💬 Чат (0xA0-0xAF)
//! - 📁 Файлы (0xD0-0xDF)
//! - 📞 Звонки (0xB0-0xBF) - TODO
//! - 📹 Видеозвонки (0xC0-0xCF) - TODO
//!
//! ## Отличия от netlayer/transport.rs:
//! - **MTU: 65536 bytes** (64 KB) вместо 1200
//! - **Порт: 9999** (данные) вместо 10000
//! - **Пакеты:** 0xA0-0xDF (Communication) вместо 0x30-0x5F (Proxy)
//! - **Без прокси** - только P2P коммуникации
//!
//! ## Архитектура:
//! ```text
//! [Браузер/WebUI]
//!      ↓
//! [ChatManager, FileTransferManager]
//!      ↓
//! [P2PTransport (MTU 65536)]
//!      ↓
//! [UDP порт 9999]
//!      ↓
//! [Peer UDP порт 9999]
//!      ↓
//! [ChatManager, FileTransferManager]
//! ```

pub mod transport;
pub mod packet;
pub mod peer;
pub mod encryption;
pub mod hello;
pub mod encryption_manager;

// Re-exports
pub use transport::P2PTransport;
pub use packet::{P2PPacket, P2PPacketType, P2P_PACKET_HEADER_LEN};
pub use peer::{P2PPeer, P2PNatStatus};
pub use encryption::P2PEncryption;
