//! Media stream handling with encryption

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};
use crate::media::codecs::opus::{OpusEncoder, OpusDecoder};
use x25519_dalek::{EphemeralSecret, PublicKey};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::{Aead, KeyInit};
use rand::RngCore;
use sha2::{Sha256, Digest};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaType {
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum StreamState {
    Initializing = 0,
    Active = 1,
    Paused = 2,
    Ended = 3,
}

impl From<u64> for StreamState {
    fn from(v: u64) -> Self {
        match v {
            0 => StreamState::Initializing,
            1 => StreamState::Active,
            2 => StreamState::Paused,
            3 => StreamState::Ended,
            _ => StreamState::Initializing,
        }
    }
}

impl From<StreamState> for u64 {
    fn from(s: StreamState) -> Self {
        s as u64
    }
}

#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate: u32,
    pub frame_duration_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 1,
            bitrate: 64000,
            frame_duration_ms: 20,
        }
    }
}

pub struct MediaStream {
    id: u64,
    stream_type: MediaType,
    state: AtomicU8,
    remote_peer_id: String,
    created_at: Instant,
    audio_config: RwLock<Option<AudioConfig>>,
    audio_encoder: RwLock<Option<Arc<OpusEncoder>>>,
    audio_decoder: RwLock<Option<Arc<OpusDecoder>>>,
    packets_sent: AtomicU64,
    packets_received: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    media_encryption_key: RwLock<Option<[u8; 32]>>,
    media_decryption_key: RwLock<Option<[u8; 32]>>,
    ratchet_counter: AtomicU64,
}

impl MediaStream {
    pub fn new(id: u64, stream_type: MediaType, remote_peer_id: String) -> Self {
        Self {
            id,
            stream_type,
            state: AtomicU8::new(StreamState::Initializing as u8),
            remote_peer_id,
            created_at: Instant::now(),
            audio_config: RwLock::new(None),
            audio_encoder: RwLock::new(None),
            audio_decoder: RwLock::new(None),
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            media_encryption_key: RwLock::new(None),
            media_decryption_key: RwLock::new(None),
            ratchet_counter: AtomicU64::new(0),
        }
    }

    pub fn configure_audio(&self, config: AudioConfig) -> Result<(), String> {
        if self.stream_type != MediaType::Audio {
            return Err("Cannot configure audio on non-audio stream".to_string());
        }
        let encoder = OpusEncoder::new(config.sample_rate, config.channels, config.bitrate)?;
        let decoder = OpusDecoder::new(config.sample_rate, config.channels)?;
        *self.audio_config.write().unwrap() = Some(config);
        *self.audio_encoder.write().unwrap() = Some(Arc::new(encoder));
        *self.audio_decoder.write().unwrap() = Some(Arc::new(decoder));
        Ok(())
    }

    pub fn generate_ephemeral_key(&self) -> Result<[u8; 32], String> {
        let ephemeral = EphemeralSecret::random_from_rng(rand::thread_rng());
        let public_key = PublicKey::from(&ephemeral);
        Ok(*public_key.as_bytes())
    }

    pub fn establish_shared_secret(&self, remote_public: &[u8; 32]) -> Result<[u8; 32], String> {
        let mut hasher = Sha256::new();
        hasher.update(remote_public);
        let mut key = [0u8; 32];
        key.copy_from_slice(&hasher.finalize());
        *self.media_encryption_key.write().unwrap() = Some(key);
        *self.media_decryption_key.write().unwrap() = Some(key);
        Ok(key)
    }

    pub fn encrypt_media(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let key_opt = self.media_encryption_key.read().unwrap();
        let key = key_opt.as_ref().ok_or("Media encryption key not established")?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|e| format!("Encryption failed: {}", e))?;
        let mut result = nonce_bytes.to_vec();
        result.extend_from_slice(&ciphertext);
        let counter = self.ratchet_counter.fetch_add(1, Ordering::Relaxed);
        if counter % 100 == 0 && counter > 0 {
            let _ = self.rotate_ratchet();
        }
        Ok(result)
    }

    pub fn decrypt_media(&self, encrypted: &[u8]) -> Result<Vec<u8>, String> {
        if encrypted.len() < 12 {
            return Err("Encrypted data too short".to_string());
        }
        let key_opt = self.media_decryption_key.read().unwrap();
        let key = key_opt.as_ref().ok_or("Media decryption key not established")?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let nonce = Nonce::from_slice(&encrypted[0..12]);
        let ciphertext = &encrypted[12..];
        let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|e| format!("Decryption failed: {}", e))?;
        Ok(plaintext)
    }

    fn rotate_ratchet(&self) -> Result<(), String> {
        let old_key_opt = self.media_encryption_key.read().unwrap();
        let old_key = old_key_opt.as_ref().ok_or("No key to rotate")?;
        let mut hasher = Sha256::new();
        hasher.update(old_key);
        let mut new_key = [0u8; 32];
        new_key.copy_from_slice(&hasher.finalize());
        *self.media_encryption_key.write().unwrap() = Some(new_key);
        *self.media_decryption_key.write().unwrap() = Some(new_key);
        Ok(())
    }

    pub fn is_encrypted(&self) -> bool {
        self.media_encryption_key.read().unwrap().is_some()
    }

    pub fn send_audio(&self, pcm: &[i16]) -> Result<Vec<u8>, String> {
        let encoder_opt = self.audio_encoder.read().unwrap();
        let encoder = encoder_opt.as_ref().ok_or("Audio encoder not configured")?;
        let encoded = encoder.encode(pcm)?;
        let encrypted = if self.is_encrypted() { self.encrypt_media(&encoded)? } else { encoded };
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(encrypted.len() as u64, Ordering::Relaxed);
        Ok(encrypted)
    }

    pub fn receive_audio(&self, packet: &[u8]) -> Result<Vec<i16>, String> {
        let decoder_opt = self.audio_decoder.read().unwrap();
        let decoder = decoder_opt.as_ref().ok_or("Audio decoder not configured")?;
        let decrypted = if self.is_encrypted() { self.decrypt_media(packet)? } else { packet.to_vec() };
        let pcm = decoder.decode(&decrypted)?;
        self.packets_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received.fetch_add(packet.len() as u64, Ordering::Relaxed);
        Ok(pcm)
    }

    pub fn frame_size_samples(&self) -> Option<usize> {
        let config = self.audio_config.read().unwrap();
        config.as_ref().map(|c| (c.sample_rate * c.frame_duration_ms / 1000) as usize * c.channels as usize)
    }

    pub fn id(&self) -> u64 { self.id }
    pub fn stream_type(&self) -> MediaType { self.stream_type }
    pub fn remote_peer(&self) -> &str { &self.remote_peer_id }
    pub fn state(&self) -> StreamState { StreamState::from(self.state.load(Ordering::Relaxed) as u64) }
    pub fn set_state(&self, state: StreamState) { self.state.store(state as u8, Ordering::Relaxed); }
    pub fn is_active(&self) -> bool { self.state() == StreamState::Active }
    
    pub fn stats(&self) -> StreamStats {
        StreamStats {
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            packets_received: self.packets_received.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            duration: self.created_at.elapsed(),
            is_encrypted: self.is_encrypted(),
        }
    }
    
    pub fn audio_config(&self) -> Option<AudioConfig> {
        self.audio_config.read().unwrap().clone()
    }
}

#[derive(Debug, Clone)]
pub struct StreamStats {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub duration: Duration,
    pub is_encrypted: bool,
}