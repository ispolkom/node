//! Общее шифрование сеансов между узлами: пакеты (`session`, `store`), обмен ключами (`x25519`) и рукопожатие с одноразовыми ключами (`handshake`).
pub mod handshake;
pub mod sealed;
pub mod session;
pub mod store;
pub mod x25519;
