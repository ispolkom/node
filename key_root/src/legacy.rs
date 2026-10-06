//! `auth.json` format version 1 (what the node wrote until now): the master key encrypted under Argon2id(`"YANDI_MACHINE:" +
//! machine id`). The key protecting it is derived from a PUBLIC value, so this format is readable by anyone holding the file and
//! the machine id — it is read here only to be migrated, and while it is the node's format the node keeps working exactly as before.
use crate::error::{KeyRootError, Result};
use crate::identity_store::legacy_key;
use crate::machine::MachineContext;
use crate::wrap::{hex_fixed, open, seal};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const AUTH_FORMAT_V1: u32 = 1;

#[derive(Serialize, Deserialize)]
struct MasterKeyEncrypted {
    machine_salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
struct LegacyAuth {
    version: u32,
    login_hash: String,
    master_key_encrypted: MasterKeyEncrypted,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AuthFormat {
    LegacyV1,
    RootV2,
}

/// Decide the format from the version field alone (never by trying keys).
pub fn auth_format(bytes: &[u8]) -> Result<AuthFormat> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| KeyRootError::Corrupt)?;
    match value.get("version").and_then(|v| v.as_u64()) {
        Some(1) => Ok(AuthFormat::LegacyV1),
        Some(2) => Ok(AuthFormat::RootV2),
        Some(v) => Err(KeyRootError::UnsupportedVersion(v as u32)),
        None => Err(KeyRootError::Corrupt),
    }
}

fn machine_passphrase(machine: &dyn MachineContext) -> Vec<u8> {
    format!("YANDI_MACHINE:{}", machine.id()).into_bytes()
}

fn parse(bytes: &[u8]) -> Result<LegacyAuth> {
    serde_json::from_slice(bytes).map_err(|_| KeyRootError::Corrupt)
}

pub fn legacy_login_hash(bytes: &[u8]) -> Result<String> {
    Ok(parse(bytes)?.login_hash)
}

/// The master key of a legacy `auth.json`, opened with the (public) machine id. A different machine id is `DecryptFailed`.
pub fn legacy_master_key(
    bytes: &[u8],
    machine: &dyn MachineContext,
) -> Result<Zeroizing<[u8; 32]>> {
    let a = parse(bytes)?;
    if a.version != AUTH_FORMAT_V1 {
        return Err(KeyRootError::UnsupportedVersion(a.version));
    }
    let salt = hex_fixed::<32>(&a.master_key_encrypted.machine_salt)?;
    let nonce = hex_fixed::<12>(&a.master_key_encrypted.nonce)?;
    let ciphertext =
        hex::decode(&a.master_key_encrypted.ciphertext).map_err(|_| KeyRootError::Corrupt)?;
    let key = legacy_key(&machine_passphrase(machine), &salt)?;
    let plain = open(&key, b"", &nonce, &ciphertext)?;
    let root = <[u8; 32]>::try_from(plain.as_slice()).map_err(|_| KeyRootError::Corrupt)?;
    Ok(Zeroizing::new(root))
}

/// Write a legacy `auth.json` (for tests and for fixtures of the migration).
pub fn seal_legacy_auth(
    master_key: &[u8; 32],
    login_hash: &str,
    machine: &dyn MachineContext,
) -> Result<Vec<u8>> {
    let salt = crate::wrap::random_bytes::<32>();
    let key = legacy_key(&machine_passphrase(machine), &salt)?;
    let (n, c) = seal(&key, b"", master_key);
    let a = LegacyAuth {
        version: AUTH_FORMAT_V1,
        login_hash: login_hash.to_owned(),
        master_key_encrypted: MasterKeyEncrypted {
            machine_salt: hex::encode(salt),
            nonce: hex::encode(n),
            ciphertext: hex::encode(c),
        },
    };
    Ok(serde_json::to_vec_pretty(&a).expect("a plain struct serializes"))
}
