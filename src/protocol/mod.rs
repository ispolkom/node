// src/protocol/mod.rs
//!
//! # YTP (You Train Protocol)
//!
//! Протокол передачи данных между You & I через виртуальные поезда.
//!
//! ## Концепция:
//! - **You** (client) ←→ **I** (gateway)
//! - **Train** = логическая передача данных (может быть 10MB+)
//! - **Wagon** = UDP пакет (макс 60KB)
//! - **Station** = нода сети YANDI
//!
//! ## Пример:
//! ```text
//! You → Train #7841 (200 вагонов, 10MB) → I
//! I → Собирает поезд → Делает запрос → YouTube
//! I → Train #7842 (150 вагонов, 7MB) → You
//! ```
//!
//! ## Преимущества:
//! - 🚂 Передача данных любого размера через UDP
//! - 🔒 End-to-end шифрование
//! - 🎯 Stealth (похоже на обычный UDP трафик)
//! - ⚡ Быстро (нет TCP handshake overhead)
//!
//! ## Типы пакетов YTP:
//! - **0x60** - YTP Wagon (вагон с данными)
//! - **0x61** - зарезервирован (подтверждений поездов в протоколе нет; пакет старого узла игнорируется)
//! - **0x70** - YTP BatchedWagon (несколько пакетов в одном wagon)

pub mod wagon;
pub mod train;
pub mod station;
pub mod ordering;  // 🚂 Train Ordering Queue
pub mod rate_controller;  // 🚀 Adaptive Rate Controller

pub use wagon::{Wagon, WagonFlags};
pub use train::{Train, TrainId, TrainState, TrainError};
pub use station::{Station, StationConfig, StationRole, StationError, BatchedPacket, BatchedWagon, DataCallback};
pub use ordering::TrainOrderingQueue;
pub use rate_controller::{RateController, RateAction};

// Re-exports
pub use crate::util::HashId;
