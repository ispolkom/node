//! Media module for voice and video communication
//! 
//! Provides codecs, session management, and WebRTC integration

pub mod codecs;
pub mod session;
pub mod signaling;
pub mod webrtc;

use std::sync::Arc;
use session::manager::MediaSessionManager;

/// Global media session manager instance
static MEDIA_MANAGER: once_cell::sync::Lazy<Arc<MediaSessionManager>> = 
    once_cell::sync::Lazy::new(|| Arc::new(MediaSessionManager::new()));

/// Get global media session manager
pub fn get_media_manager() -> Arc<MediaSessionManager> {
    MEDIA_MANAGER.clone()
}

/// Initialize media module
pub fn init() -> Result<(), String> {
    tracing::info!("Media module initialized");
    Ok(())
}

pub mod transport;

pub mod audio_capture;
pub mod p2p_bridge;
