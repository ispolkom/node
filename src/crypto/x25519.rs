//! Обмен ключами X25519 с проверкой слабых точек и затиранием секретов.
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// Секрет X25519 (32 случайных байта, затираются при удалении) и его публичный ключ.
pub struct SecretKey {
    secret: StaticSecret,
    pub public: [u8; 32],
}

impl SecretKey {
    pub fn generate() -> Self {
        let mut seed = Zeroizing::new([0u8; 32]);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut *seed);
        Self::from_seed(*seed)
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        let secret = StaticSecret::from(seed);
        let public = PublicKey::from(&secret).to_bytes();
        Self { secret, public }
    }

    /// Общий секрет с чужим публичным ключом. Отказ, если чужой ключ — слабая точка (общий секрет не зависел бы от нашего ключа).
    pub fn diffie_hellman(&self, their_public: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, String> {
        let shared = self.secret.diffie_hellman(&PublicKey::from(*their_public));
        if !shared.was_contributory() {
            return Err("peer public key is a weak (low-order) point".to_string());
        }
        Ok(Zeroizing::new(*shared.as_bytes()))
    }
}

/// Общий секрет → ключ сеанса. Та же формула, что использовалась раньше («YANDI session key»), поэтому сохранённые возобновления остаются понятны.
pub fn derive_master(shared: &[u8; 32]) -> [u8; 32] {
    use hkdf::Hkdf;
    use sha2::Sha256;
    let hk = Hkdf::<Sha256>::new(None, shared);
    let mut key = [0u8; 32];
    hk.expand(b"YANDI session key", &mut key).expect("hkdf 32 bytes");
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_get_the_same_secret() {
        let (a, b) = (SecretKey::generate(), SecretKey::generate());
        assert_eq!(*a.diffie_hellman(&b.public).unwrap(), *b.diffie_hellman(&a.public).unwrap());
        assert_ne!(a.public, b.public);
    }

    #[test]
    fn a_low_order_point_is_refused() {
        let a = SecretKey::generate();
        assert!(a.diffie_hellman(&[0u8; 32]).is_err(), "нулевая точка");
        let mut one = [0u8; 32];
        one[0] = 1;
        assert!(a.diffie_hellman(&one).is_err(), "точка порядка 1");
        // точка порядка 8 из известного списка слабых точек Curve25519
        let p8 = hex::decode("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800").unwrap();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&p8);
        assert!(a.diffie_hellman(&arr).is_err(), "точка малого порядка");
    }

    #[test]
    fn rfc7748_vector() {
        // RFC 7748, раздел 6.1: Alice/Bob
        let a = SecretKey::from_seed(hex::decode("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a").unwrap().try_into().unwrap());
        let b_pub: [u8; 32] = hex::decode("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f").unwrap().try_into().unwrap();
        assert_eq!(hex::encode(a.public), "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        assert_eq!(hex::encode(*a.diffie_hellman(&b_pub).unwrap()), "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
    }
}
