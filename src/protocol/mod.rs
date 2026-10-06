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
//! - **0x61** - YTP ACK/NACK (подтверждение/запрос повторной отправки)
//! - **0x70** - YTP BatchedWagon (несколько пакетов в одном wagon)

pub mod wagon;
pub mod train;
pub mod station;
pub mod express;
pub mod tcp_tunnel;  // ⚡ Raw TCP Tunnel
pub mod tcp_tunnel_exit;  // 🚇 TcpTunnel Exit Handler
pub mod tcp_station;  // 🚇 TCP Station (Dual-Path, Clones)
pub mod tcp_transport;  // 🚇 TCP Transport (Full UDP Transport Analog)
pub mod tcp_tunnel_exit_v2;  // 🚇 TCP Tunnel Exit V2 (Full Dual-Path Support)
pub mod nack;  // 🔄 Wagon NACK (Negative Acknowledgment)
pub mod ordering;  // 🚂 Train Ordering Queue
pub mod line;  // 🚂 Line (изолированная линия)
pub mod rate_controller;  // 🚀 Adaptive Rate Controller

pub use wagon::{Wagon, WagonFlags};
pub use train::{Train, TrainId, TrainState, TrainError};
pub use station::{Station, StationConfig, StationRole, StationError, BatchedPacket, BatchedWagon, DataCallback};
pub use express::{ExpressTrain, ExpressStrategy, TrainPriority, TrainAckMessage};
pub use tcp_tunnel::TcpTunnel;
pub use tcp_tunnel_exit::TcpTunnelExitHandler;
pub use tcp_station::{TcpStation, TcpStationConfig, TcpWagon, TcpTrain, TcpConnection, TcpDataCallback, TcpStationError};
pub use tcp_transport::{TcpTransport, TcpPacketType, TcpPacketHeader, TcpPeerSession, HandshakePacket, TcpTransportStats};
pub use tcp_tunnel_exit_v2::{TcpTunnelV2, TcpTunnelExitHandlerV2};
pub use nack::{WagonNack, NackReason};
pub use ordering::TrainOrderingQueue;
pub use line::{Line, LineStats, SentTrain};
pub use rate_controller::{RateController, RateAction};

// Re-exports
pub use crate::util::HashId;
