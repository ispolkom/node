// src/core/config.rs
//! Core Configuration
//! ==================

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::fs;
use anyhow::{Result, Context};
use tracing::info;

const CONFIG_FILE: &str = "yandi-config.yaml";
const CONFIG_ENV: &str = "YANDI_CONFIG";

/// Порты по умолчанию
const DEFAULT_WEB_UI_PORT: u16 = 9999;
const DEFAULT_HTTP_PROXY_PORT: u16 = 8080;
const DEFAULT_DISCOVERY_PORT: u16 = 9000;
const DEFAULT_DATA_PORT: u16 = 10000;
const DEFAULT_MOBILE_GATEWAY_PORT: u16 = 9111;
const DEFAULT_MOBILE_P2P_PORT: u16 = 9112;

/// Основная конфигурация YANDI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YandiConfig {
    /// Серверные настройки
    #[serde(default)]
    pub server: ServerConfig,

    /// Настройки портов
    #[serde(default)]
    pub ports: PortsConfig,

    /// Сетевые настройки
    #[serde(default)]
    pub network: NetworkConfig,

    /// WS-over-TLS bind (Hardening Step 1)
    #[serde(default)]
    pub ws: WsConfig,
}

impl Default for YandiConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            ports: PortsConfig::default(),
            network: NetworkConfig::default(),
            ws: WsConfig::default(),
        }
    }
}

/// WS-over-TLS server bind address (Hardening Step 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsConfig {
    #[serde(default = "default_ws_bind")]
    pub bind: String,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self { bind: default_ws_bind() }
    }
}

fn default_ws_bind() -> String { "0.0.0.0:8443".to_string() }

/// Серверные настройки
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// IP адрес для привязки
    #[serde(default = "default_bind_address")]
    pub bind_address: String,

    /// Уровень логирования
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            log_level: default_log_level(),
        }
    }
}

fn default_bind_address() -> String { "0.0.0.0".to_string() }
fn default_log_level() -> String { "info".to_string() }

/// Конфигурация всех портов
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortsConfig {
    /// Discovery Server (обнаружение нод)
    #[serde(default = "default_discovery_port")]
    pub discovery: u16,

    /// Data между нодами (проксирование)
    #[serde(default = "default_data_port")]
    pub data: u16,

    /// Mobile Gateway (мобильный → шлюз → интернет)
    #[serde(default = "default_mobile_gateway")]
    pub mobile_gateway: u16,

    /// Mobile P2P (мобильный ↔ мобильный)
    #[serde(default = "default_mobile_p2p")]
    pub mobile_p2p: u16,

    /// HTTP Proxy (браузер)
    #[serde(default = "default_http_proxy")]
    pub http_proxy: u16,

    /// Web UI (Admin interface)
    #[serde(default = "default_web_ui")]
    pub web_ui: u16,
}

impl Default for PortsConfig {
    fn default() -> Self {
        Self {
            discovery: default_discovery_port(),
            data: default_data_port(),
            mobile_gateway: default_mobile_gateway(),
            mobile_p2p: default_mobile_p2p(),
            http_proxy: default_http_proxy(),
            web_ui: default_web_ui(),
        }
    }
}

fn default_discovery_port() -> u16 { DEFAULT_DISCOVERY_PORT }
fn default_data_port() -> u16 { DEFAULT_DATA_PORT }
fn default_mobile_gateway() -> u16 { DEFAULT_MOBILE_GATEWAY_PORT }
fn default_mobile_p2p() -> u16 { DEFAULT_MOBILE_P2P_PORT }
fn default_http_proxy() -> u16 { DEFAULT_HTTP_PROXY_PORT }
fn default_web_ui() -> u16 { DEFAULT_WEB_UI_PORT }

/// Сетевые настройки
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Публичный IP
    #[serde(default)]
    pub public_ip: Option<String>,

    /// MTU для P2P
    #[serde(default = "default_mtu")]
    pub mtu: usize,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            public_ip: None,
            mtu: default_mtu(),
        }
    }
}

fn default_mtu() -> usize { 65536 }

impl YandiConfig {
    /// Загрузить конфиг из файла
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path();

        if config_path.exists() {
            let content = fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config: {:?}", config_path))?;

            let config: YandiConfig = serde_yaml::from_str(&content)
                .with_context(|| "Failed to parse YAML")?;

            info!("✅ Config loaded from: {:?}", config_path);
            Ok(config)
        } else {
            info!("⚠️  Config not found, using defaults");
            let default_config = YandiConfig::default();
            default_config.save()?;
            Ok(default_config)
        }
    }

    /// Сохранить конфиг в файл
    pub fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path();

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| "Failed to create config dir")?;
        }

        let content = serde_yaml::to_string(self)
            .with_context(|| "Failed to serialize config")?;

        fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config: {:?}", config_path))?;

        info!("💾 Config saved to: {:?}", config_path);
        Ok(())
    }

    /// Получить путь к конфигу
    fn get_config_path() -> PathBuf {
        if let Ok(env_path) = std::env::var(CONFIG_ENV) {
            return PathBuf::from(env_path);
        }

        let local_path = PathBuf::from(CONFIG_FILE);
        if local_path.exists() {
            return local_path;
        }

        if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(config_home).join("yandi").join(CONFIG_FILE);
        }

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".config").join("yandi").join(CONFIG_FILE)
    }

    /// Клиентская конфигурация (для Web UI и мобилки)
    pub fn to_client_config(&self) -> ClientConfig {
        ClientConfig {
            discovery_port: self.ports.discovery,
            gateway_port: self.ports.mobile_gateway,
            p2p_port: self.ports.mobile_p2p,
            http_proxy_port: self.ports.http_proxy,
        }
    }
}

/// Клиентская конфигурация
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub discovery_port: u16,
    pub gateway_port: u16,
    pub p2p_port: u16,
    pub http_proxy_port: u16,
}

/// Глобальная конфигурация (потокобезопасная)
use std::sync::RwLock;

static GLOBAL_CONFIG: RwLock<Option<YandiConfig>> = RwLock::new(None);

/// CLI override для ws-bind (Hardening Step 1). Если задан — имеет приоритет над config.
static WS_BIND_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

/// Установить CLI override для ws-bind (`--ws-bind <addr>`).
pub fn set_ws_bind_override(addr: String) {
    *WS_BIND_OVERRIDE.write().unwrap() = Some(addr);
}

/// Получить эффективный ws-bind: CLI override → config → default 0.0.0.0:8443.
pub fn effective_ws_bind() -> String {
    if let Some(o) = WS_BIND_OVERRIDE.read().unwrap().as_ref() {
        return o.clone();
    }
    if let Some(cfg) = GLOBAL_CONFIG.read().unwrap().as_ref() {
        return cfg.ws.bind.clone();
    }
    default_ws_bind()
}

/// Инициализировать глобальную конфигурацию
pub fn init_config() -> Result<()> {
    let config = YandiConfig::load()?;
    *GLOBAL_CONFIG.write().unwrap() = Some(config);
    Ok(())
}

/// Получить конфигурацию
pub fn get_config() -> YandiConfig {
    GLOBAL_CONFIG.read().unwrap()
        .as_ref()
        .expect("Config not initialized")
        .clone()
}

/// Обновить конфигурацию
pub fn update_config(config: YandiConfig) -> Result<()> {
    config.save()?;
    *GLOBAL_CONFIG.write().unwrap() = Some(config);
    Ok(())
}

/// Старая конфигурация для обратной совместимости
#[derive(Debug, Clone)]
pub struct NetConfig {
    pub max_packet_size: usize,
    pub transport_channel_size: usize,
    pub handshake_retries: u8,
}

impl NetConfig {
    pub fn default() -> Self {
        Self {
            max_packet_size: 2048,
            transport_channel_size: 100,
            handshake_retries: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_ports() {
        let config = YandiConfig::default();
        assert_eq!(config.ports.discovery, 9000);
        assert_eq!(config.ports.data, 10000);
        assert_eq!(config.ports.mobile_gateway, 9111);
        assert_eq!(config.ports.mobile_p2p, 9112);
        assert_eq!(config.ports.http_proxy, 8080);
        assert_eq!(config.ports.web_ui, 9999);
    }

    #[test]
    fn test_ws_bind_default() {
        let config = YandiConfig::default();
        assert_eq!(config.ws.bind, "0.0.0.0:8443");
    }

    #[test]
    fn test_ws_bind_parse_yaml() {
        let yaml = "ws:\n  bind: \"0.0.0.0:443\"\n";
        let cfg: YandiConfig = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(cfg.ws.bind, "0.0.0.0:443");
    }

    #[test]
    fn test_ws_bind_missing_yaml_uses_default() {
        let yaml = "server:\n  bind_address: \"0.0.0.0\"\n  log_level: info\n";
        let cfg: YandiConfig = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(cfg.ws.bind, "0.0.0.0:8443");
    }

    #[test]
    fn test_ws_bind_override_takes_priority() {
        set_ws_bind_override("0.0.0.0:9443".to_string());
        assert_eq!(effective_ws_bind(), "0.0.0.0:9443");
        // Сброс глобального override, чтобы не утекало в другие тесты
        *WS_BIND_OVERRIDE.write().unwrap() = None;
    }
}
