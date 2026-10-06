// src/observability/logging.rs
//! Logging Setup
//! ==============
//!
//! Simple logging configuration

use tracing::Level;
use tracing_subscriber::{
    fmt,
    EnvFilter,
};

/// Log level
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_tracing(&self) -> Level {
        match self {
            LogLevel::Trace => Level::TRACE,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Info => Level::INFO,
            LogLevel::Warn => Level::WARN,
            LogLevel::Error => Level::ERROR,
        }
    }
}

/// Initialize logging for YANDI
pub fn init_logging(level: LogLevel) -> Result<(), String> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            EnvFilter::new(level.as_tracing().to_string())
                .add_directive("yandi=debug".parse().unwrap())
                .add_directive("tokio=warn".parse().unwrap())
                .add_directive("hyper=warn".parse().unwrap())
                .add_directive("reqwest=warn".parse().unwrap())
        });

    let subscriber = fmt()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .with_env_filter(env_filter)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| format!("Failed to set logging subscriber: {}", e))?;

    Ok(())
}

/// Initialize logging for development (more verbose)
pub fn init_logging_dev() -> Result<(), String> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            EnvFilter::new("debug")
                .add_directive("yandi=trace".parse().unwrap())
                .add_directive("tokio=info".parse().unwrap())
        });

    let subscriber = fmt()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_env_filter(env_filter)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| format!("Failed to set logging subscriber: {}", e))?;

    Ok(())
}
