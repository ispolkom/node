// src/web/api.rs
//!
//! # YANDI Web API
//!
//! REST API endpoints для управления нодой

use serde::{Deserialize, Serialize};

/// Статус ноды
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node_id: String,
    pub mode: String,
    pub status: String,
    pub peers: u32,
    pub uptime: String,
    pub sent: String,
    pub recv: String,
    pub speed: String,
}

/// Контакт
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub short_id: String,
    pub node_id: String,
    pub online: bool,
    pub added_at: Option<String>,
}

/// Шлюз
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gateway {
    pub id: String,
    pub name: String,
    pub short_id: String,
    pub node_id: String,
    pub country: String,
    pub latency_ms: u32,
    pub speed_mbps: Option<f64>,
    pub connected: bool,
    pub auto_connect: bool,
}

/// Настройки ноды
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSettings {
    pub node_mode: NodeModeSettings,
    pub gateway: GatewaySettings,
    pub network: NetworkSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeModeSettings {
    pub p2p_enabled: bool,
    pub gateway_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewaySettings {
    pub auto_start: bool,
    pub multi_port: bool,
    pub max_clients: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    pub discovery_port: u16,
    pub data_port: u16,
}

/// API Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub status: String,
    pub message: String,
}
