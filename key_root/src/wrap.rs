//! The primitives: AES-256-GCM sealing with associated data, Argon2id for passwords, HKDF for domain separation.
use crate::error::{KeyRootError, Result};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use zeroize::Zeroizing;

/// Key-derivation domains. `core/v1` is the one the Node already gives the Python Core (unchanged); the others are new.
pub const DOMAIN_IDENTITY: &str = "yandi/identity/v1";
pub const DOMAIN_CORE: &str = "yandi/core/v1";
pub const DOMAIN_ROOT_ID: &str = "yandi/root-id/v1";

pub fn derive_domain(root: &[u8; 32], domain: &str) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, root)
        .expand(domain.as_bytes(), out.as_mut_slice())
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    out
}

/// A short public fingerprint of a root key: it lets a file say which root it belongs to without helping anyone find it.
pub fn root_id(root: &[u8; 32]) -> String {
    hex::encode(&derive_domain(root, DOMAIN_ROOT_ID)[..16])
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> ([u8; 12], Vec<u8>) {
    let nonce = random_bytes::<12>();
    let cipher = Aes256Gcm::new_from_slice(key).expect("a 32-byte key");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("AES-GCM encryption of a small buffer cannot fail");
    (nonce, ciphertext)
}

pub fn open(
    key: &[u8; 32],
    aad: &[u8],
    nonce: &[u8; 12],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| KeyRootError::Corrupt)?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| KeyRootError::DecryptFailed)
}

/// Argon2id settings, stored with the wrapper so that they can be raised later without a format change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes: about half a second per guess on the machine that made it, and 64 MiB per guess for an attacker.
    pub const RECOMMENDED: KdfParams = KdfParams {
        memory_kib: 65536,
        iterations: 3,
        parallelism: 1,
    };
}

/// The floor below which a stored parameter set is refused (a file edited to make guessing cheap must not be accepted).
#[derive(Clone, Copy, Debug)]
pub struct KdfPolicy {
    pub min_memory_kib: u32,
    pub min_iterations: u32,
}

impl KdfPolicy {
    /// What the node and the tool use: at least the project's existing Argon2id cost (32 MiB, 2 passes).
    pub const fn production() -> Self {
        KdfPolicy {
            min_memory_kib: 32768,
            min_iterations: 2,
        }
    }

    /// For tests only (a cheap KDF keeps the test suite fast). Nothing in the node or the tool ever uses it.
    pub const fn for_tests() -> Self {
        KdfPolicy {
            min_memory_kib: 8,
            min_iterations: 1,
        }
    }
}

/// Password → key encryption key. Argon2id, never a plain hash.
pub fn derive_kek(
    password: &[u8],
    salt: &[u8; 32],
    params: &KdfParams,
    policy: &KdfPolicy,
) -> Result<Zeroizing<[u8; 32]>> {
    use argon2::{Algorithm, Argon2, Params, Version};
    if params.memory_kib < policy.min_memory_kib
        || params.iterations < policy.min_iterations
        || params.parallelism == 0
        || params.parallelism > 16
    {
        return Err(KeyRootError::WeakParameters);
    }
    let p = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(32),
    )
    .map_err(|_| KeyRootError::WeakParameters)?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, p)
        .hash_password_into(password, salt, out.as_mut_slice())
        .map_err(|_| KeyRootError::WeakParameters)?;
    Ok(out)
}

pub(crate) fn hex_fixed<const N: usize>(text: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(text).map_err(|_| KeyRootError::Corrupt)?;
    <[u8; N]>::try_from(bytes.as_slice()).map_err(|_| KeyRootError::Corrupt)
}
