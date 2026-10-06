// src/netlayer/encryption.rs
//! Шифрование основной сети узлов (DHT, ретрансляция, прокси и туннели, станция/поезд).
//!
//! Оболочка над общим менеджером `crypto::handshake`: на каждое приветствие обе стороны делают свой ОДНОРАЗОВЫЙ X25519-ключ, ключ сеанса — из обмена одноразовыми
//! ключами (прямая секретность: кража долгоживущих ключей не раскрывает прошлый трафик). Пакеты шифрует общий модуль `crypto` (счётчики, окно повторов,
//! ключи направлений, AAD, потолок, затирание). Раньше здесь ключ сеанса выводился из X25519-ключа процесса, живущего всё время работы узла.
use crate::crypto::handshake::HandshakeManager;
use crate::netlayer::peer::PeerInfo;
use crate::util::HashId;

/// Менеджер сеансов основной сети.
pub struct EncryptionManager {
    inner: HandshakeManager,
}

impl Default for EncryptionManager {
    fn default() -> Self {
        Self { inner: HandshakeManager::default() }
    }
}

impl EncryptionManager {
    pub fn new(our_id: HashId) -> Self {
        Self { inner: HandshakeManager::new(our_id) }
    }

    // ── Рукопожатие ──────────────────────────────────────────────────────────

    /// Инициатор: свежий одноразовый ключ для приглашения с этим номером (номер приветствия). Возвращает публичный ключ для пакета-приглашения.
    pub fn generate_hello_ephemeral(&mut self, nonce: u64) -> [u8; 32] {
        self.inner.generate_hello_ephemeral(nonce)
    }

    /// Инициатор: пришло подтверждение с ключом ответчика; сеанс поднимается, секрет приглашения стирается.
    pub fn complete_hello_initiator(&mut self, request_nonce: u64, peer_id: HashId, their_pub: &[u8; 32]) -> Result<u64, String> {
        self.inner.complete_hello_initiator(request_nonce, peer_id, their_pub)
    }

    /// Ответчик: свой одноразовый ключ, сеанс поднят сразу. Возвращает публичный ключ для подтверждения.
    /// Ошибка «встречное знакомство» означает, что остаётся ключ рукопожатия узла с меньшим номером, а подтверждение отправлять не нужно.
    pub fn complete_hello_responder(&mut self, peer_id: HashId, their_pub: &[u8; 32]) -> Result<[u8; 32], String> {
        self.inner.complete_hello_responder(peer_id, their_pub)
    }

    // ── Сеанс ────────────────────────────────────────────────────────────────

    pub fn has_session(&self, peer: &PeerInfo) -> bool {
        self.inner.has_session(&peer.id)
    }

    /// Восстановить сеанс из сохранённого общего ключа (возобновление по QR-спариванию). Временная мера: ключ на диске не даёт прямой секретности;
    /// возобновление через новый обмен ключами — в плане (docs/CRYPTO_MAP.md, H3).
    pub fn restore_session(&mut self, peer_id: HashId, session_key: [u8; 32]) -> u64 {
        self.inner.restore_session(peer_id, session_key)
    }

    /// Общий ключ сеанса с узлом (для записи возобновления).
    pub fn session_key_bytes(&self, peer_id: HashId) -> Option<[u8; 32]> {
        self.inner.session_key_bytes(&peer_id)
    }

    /// Нужно ли договориться о новом ключе с этим узлом.
    pub fn needs_rekey(&self, peer: &PeerInfo) -> bool {
        self.inner.needs_rekey(&peer.id)
    }

    pub fn encrypt(&self, peer: &PeerInfo, data: &[u8]) -> Result<Vec<u8>, String> {
        self.inner.encrypt(&peer.id, data)
    }

    pub fn decrypt(&mut self, peer: &PeerInfo, data: &[u8]) -> Result<Vec<u8>, String> {
        self.inner.decrypt(&peer.id, data)
    }

    /// Номер отправителя из заголовка пакета, не расшифровывая.
    pub fn extract_peer_id(data: &[u8]) -> Result<HashId, String> {
        HandshakeManager::extract_peer_id(data)
    }

    pub fn decrypt_by_peer_id(&mut self, data: &[u8]) -> Result<(HashId, Vec<u8>), String> {
        self.inner.decrypt_by_peer_id(data)
    }

    /// Удалить сеансы без движения (дольше 5 минут).
    pub fn cleanup_stale_sessions(&mut self) {
        self.inner.cleanup_stale_sessions()
    }

    pub fn remove_session(&mut self, peer: &PeerInfo) {
        self.inner.remove_session(&peer.id)
    }

    pub fn session_count(&self) -> usize {
        self.inner.session_count()
    }
}

/// Простое шифрование ключом для прямого использования (например, в приватных туннелях).
pub fn encrypt_data(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Key, Nonce};
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce_bytes);
    let mut encrypted = cipher.encrypt(Nonce::from_slice(&nonce_bytes), data).map_err(|_| "Encryption failed")?;
    let mut result = nonce_bytes.to_vec();
    result.append(&mut encrypted);
    Ok(result)
}

/// Расшифровка для `encrypt_data`.
pub fn decrypt_data(encrypted_data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Key, Nonce};
    if encrypted_data.len() < 12 {
        return Err("Encrypted data too short".to_string());
    }
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    cipher.decrypt(Nonce::from_slice(nonce_bytes), ciphertext).map_err(|_| "Decryption failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NodeIdentity;

    /// Рукопожатие по пути основной сети: приглашение → подтверждение → оба с сеансом.
    fn handshake(a: &mut EncryptionManager, b: &mut EncryptionManager, ida: HashId, idb: HashId, nonce: u64) {
        let req = a.generate_hello_ephemeral(nonce);
        let ack = b.complete_hello_responder(ida, &req).unwrap();
        a.complete_hello_initiator(nonce, idb, &ack).unwrap();
    }

    fn two() -> (EncryptionManager, EncryptionManager, PeerInfo, PeerInfo) {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (mut a, mut b) = (EncryptionManager::new(ia.node_id()), EncryptionManager::new(ib.node_id()));
        handshake(&mut a, &mut b, ia.node_id(), ib.node_id(), 1);
        (a, b, PeerInfo::new(ia.node_id(), "127.0.0.1:9000"), PeerInfo::new(ib.node_id(), "127.0.0.1:9001"))
    }

    #[test]
    fn a_handshake_gives_both_sides_a_working_session() {
        let (a, mut b, pa, pb) = two();
        assert!(a.has_session(&pb) && b.has_session(&pa));
        let ct = a.encrypt(&pb, b"Hello, encrypted world!").unwrap();
        assert_eq!(b.decrypt(&pa, &ct).unwrap(), b"Hello, encrypted world!");
    }

    /// Прямая секретность: два рукопожатия между теми же узлами дают разные ключи, а секреты не остаются в менеджере.
    #[test]
    fn two_handshakes_give_different_keys_so_old_traffic_stays_closed() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a_id, b_id) = (ia.node_id(), ib.node_id());
        let mut keys = vec![];
        for n in 0..2 {
            let (mut a, mut b) = (EncryptionManager::new(a_id), EncryptionManager::new(b_id));
            handshake(&mut a, &mut b, a_id, b_id, n);
            keys.push(a.session_key_bytes(b_id).unwrap());
        }
        assert_ne!(keys[0], keys[1]);
    }

    /// Сосед перезапустился: новое рукопожатие принимается сразу.
    #[test]
    fn a_restarted_peer_agrees_on_a_new_key_at_once() {
        let (ia, ib) = (NodeIdentity::new(), NodeIdentity::new());
        let (a_id, b_id) = (ia.node_id(), ib.node_id());
        let (mut a, mut b) = (EncryptionManager::new(a_id), EncryptionManager::new(b_id));
        handshake(&mut a, &mut b, a_id, b_id, 1);
        let mut b2 = EncryptionManager::new(b_id);
        handshake(&mut a, &mut b2, a_id, b_id, 2);
        let pa = PeerInfo::new(a_id, "a");
        let pb = PeerInfo::new(b_id, "b");
        let ct = b2.encrypt(&pa, b"after restart").unwrap();
        assert_eq!(a.decrypt(&pb, &ct).unwrap(), b"after restart");
    }

    #[test]
    fn a_weak_peer_key_is_refused_and_no_session_appears() {
        let ia = NodeIdentity::new();
        let mut a = EncryptionManager::new(ia.node_id());
        let peer = PeerInfo::new(NodeIdentity::new().node_id(), "x");
        a.generate_hello_ephemeral(1);
        assert!(a.complete_hello_initiator(1, peer.id, &[0u8; 32]).is_err());
        assert!(a.complete_hello_responder(peer.id, &[0u8; 32]).is_err());
        assert!(!a.has_session(&peer));
    }

    #[test]
    fn test_restore_session_round_trip() {
        let (a, b, pa, pb) = two();
        let key_a = a.session_key_bytes(pb.id).unwrap();
        let key_b = b.session_key_bytes(pa.id).unwrap();
        assert_eq!(key_a, key_b, "session-key должен быть симметричен");
        let mut a2 = EncryptionManager::new(pa.id);
        let mut b2 = EncryptionManager::new(pb.id);
        a2.restore_session(pb.id, key_a);
        b2.restore_session(pa.id, key_b);
        let enc = a2.encrypt(&pb, b"resumed-after-restart").unwrap();
        assert_eq!(b2.decrypt(&pa, &enc).unwrap(), b"resumed-after-restart");
    }

    #[test]
    fn replayed_ciphertext_is_rejected_but_fresh_messages_still_work() {
        let (a, mut b, _pa, pb) = two();
        let captured = a.encrypt(&pb, b"transfer: 1 hour to bob").unwrap();
        assert!(b.decrypt_by_peer_id(&captured).is_ok());
        let replay = b.decrypt_by_peer_id(&captured);
        assert!(replay.unwrap_err().contains("REPLAY"));
        let second_real = a.encrypt(&pb, b"transfer: 1 hour to alice").unwrap();
        assert_ne!(captured, second_real);
        assert!(b.decrypt_by_peer_id(&second_real).is_ok());
    }

    #[test]
    fn packets_of_an_old_format_are_refused() {
        let (a, mut b, pa, pb) = two();
        let mut ct = a.encrypt(&pb, b"x").unwrap();
        ct[32] = 1;
        assert!(b.decrypt(&pa, &ct).unwrap_err().contains("unsupported packet format"));
    }

    #[test]
    fn simple_key_encryption_round_trips_and_rejects_tampering() {
        let key = [3u8; 32];
        let ct = encrypt_data(b"tunnel data", &key).unwrap();
        assert_eq!(decrypt_data(&ct, &key).unwrap(), b"tunnel data");
        let mut bad = ct.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(decrypt_data(&bad, &key).is_err());
        assert!(decrypt_data(&ct[..5], &key).is_err());
        assert!(decrypt_data(&ct, &[4u8; 32]).is_err());
    }
}
