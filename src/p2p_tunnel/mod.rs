// src/p2p_tunnel/mod.rs
//! Pure P2P Tunnel (Peer-to-Peer)
//! ==============================
//!
//! Чистый P2P тоннель точка-точка БЕЗ выхода в интернет.
//! Используется для:
//! - 📞 Голосовые звонки (VoIP)
//! - 📹 Видеосвязь
//! - 📎 Обмен файлами P2P
//! - 🎮 Игры P2P
//!
//! ## Поток данных:
//!
//! ```text
//! [Node A] ←──────── P2P (E2E encrypted) → [Node B]
//!    ↓                                          ↓
//! P2PTunnel                              P2PTunnel
//!    ↓                                          ↓
//!  VoIP/File/Game                        VoIP/File/Game
//! ```
//!
//! ## Управляющие пакеты:
//!
//! - **0x80** - TunnelRequest (запрос на создание тоннеля)
//! - **0x81** - TunnelAccept (принятие запроса)
//! - **0x82** - TunnelReject (отклонение)
//! - **0x83** - TunnelClose (закрытие тоннеля)

pub mod types;
pub mod protocol;
pub mod tunnel;
pub mod manager;

pub use types::*;
pub use protocol::*;
pub use tunnel::*;
pub use manager::*;
