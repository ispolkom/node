//! # YANDI Groups Module
//!
//! Децентрализованные группы на базе DHT
//! Аналог Telegram/WhatsApp групп

pub mod group;
pub mod group_manager;
pub mod group_message;

pub use group::{Group, GroupId, GroupMember, GroupRole, GroupSettings};
pub use group_manager::GroupManager;
pub use group_message::{GroupMessage, GroupMessageType, GroupSyncState};
