// src/p2p/encryption_manager.rs
//! Шифрование канала переписки, файлов и звонков (`p2p::P2PTransport`): общий менеджер рукопожатия с одноразовыми ключами (`crypto::handshake`).
pub use crate::crypto::handshake::HandshakeManager as EncryptionManager;
