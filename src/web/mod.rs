// src/web/mod.rs
//!
//! # YANDI Web UI
//!
//! Локальный веб-сервер для управления нодой

pub mod server;
pub mod admin;
pub mod api;
pub mod auth;
pub mod peers;

pub use server::{WebServer, NodeInfo};

pub mod media_api;

#[cfg(unix)]
pub mod first_setup;

#[cfg(target_os = "linux")]
mod ui_socket;
