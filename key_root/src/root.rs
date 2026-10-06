//! The root key and its two independent ways in.
//!
//! ```text
//!   device key (random, this device only) ─┐
//!        + machine id as associated data   ├─ unwrap ─▶ ROOT ─HKDF(domain)─▶ identity key, core key, …
//!   recovery password ─Argon2id─▶ KEK ─────┘
//! ```
//! `auth.json` format version 2 holds both wrappers. Nothing in it is a secret by itself: the device wrapper is useless without the
//! device key, the recovery wrapper without the password, and neither wrapper depends on the other.
use crate::error::{KeyRootError, Result};
use crate::machine::MachineContext;
use crate::wrap::{derive_kek, hex_fixed, open, random_bytes, root_id, seal, KdfParams, KdfPolicy};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const AUTH_FORMAT_V2: u32 = 2;
/// The shortest recovery password accepted for a new wrapper. It is a secret an attacker holding the files can guess offline, so it
/// has to be long; a passphrase of several words is the intent.
pub const MIN_RECOVERY_PASSWORD_CHARS: usize = 12;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DeviceWrapper {
    pub provider: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RecoveryWrapper {
    pub kdf: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RootDocument {
    pub version: u32,
    /// The web login hash, carried through unchanged (it is not part of the root key chain).
    pub login_hash: String,
    pub root_id: String,
    pub device: DeviceWrapper,
    pub recovery: RecoveryWrapper,
}

fn device_aad(root_id: &str, machine: &str) -> Vec<u8> {
    format!("yandi/root/device/v2|{root_id}|{machine}").into_bytes()
}

fn recovery_aad(root_id: &str, p: &KdfParams) -> Vec<u8> {
    // The machine id is deliberately NOT here: recovery must work on a machine that has never seen this one.
    format!(
        "yandi/root/recovery/v2|{root_id}|{}|{}|{}",
        p.memory_kib, p.iterations, p.parallelism
    )
    .into_bytes()
}

impl RootDocument {
    /// Wrap `root` for this device and for the recovery password.
    pub fn create(
        root: &[u8; 32],
        login_hash: &str,
        device_key: &[u8; 32],
        device_provider: &str,
        machine: &dyn MachineContext,
        recovery_password: &str,
        params: KdfParams,
        policy: &KdfPolicy,
    ) -> Result<RootDocument> {
        if recovery_password.chars().count() < MIN_RECOVERY_PASSWORD_CHARS {
            return Err(KeyRootError::Invalid("the recovery password is too short"));
        }
        let id = root_id(root);
        let salt = random_bytes::<32>();
        let kek = derive_kek(recovery_password.as_bytes(), &salt, &params, policy)?;
        let (rn, rc) = seal(&kek, &recovery_aad(&id, &params), root);
        let (dn, dc) = seal(device_key, &device_aad(&id, &machine.id()), root);
        Ok(RootDocument {
            version: AUTH_FORMAT_V2,
            login_hash: login_hash.to_owned(),
            root_id: id,
            device: DeviceWrapper {
                provider: device_provider.to_owned(),
                nonce: hex::encode(dn),
                ciphertext: hex::encode(dc),
            },
            recovery: RecoveryWrapper {
                kdf: "argon2id".into(),
                memory_kib: params.memory_kib,
                iterations: params.iterations,
                parallelism: params.parallelism,
                salt: hex::encode(salt),
                nonce: hex::encode(rn),
                ciphertext: hex::encode(rc),
            },
        })
    }

    /// Read `auth.json` of format version 2. A version this code does not know is `UnsupportedVersion`, decided from the version
    /// field alone (never by trying passwords).
    pub fn parse(bytes: &[u8]) -> Result<RootDocument> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| KeyRootError::Corrupt)?;
        let version = value
            .get("version")
            .and_then(|v| v.as_u64())
            .ok_or(KeyRootError::Corrupt)? as u32;
        if version != AUTH_FORMAT_V2 {
            return Err(KeyRootError::UnsupportedVersion(version));
        }
        let doc: RootDocument = serde_json::from_value(value).map_err(|_| KeyRootError::Corrupt)?;
        hex_fixed::<16>(&doc.root_id)?;
        hex_fixed::<12>(&doc.device.nonce)?;
        hex_fixed::<12>(&doc.recovery.nonce)?;
        hex_fixed::<32>(&doc.recovery.salt)?;
        if doc.device.ciphertext.len() != 96
            || doc.recovery.ciphertext.len() != 96
            || doc.recovery.kdf != "argon2id"
        {
            return Err(KeyRootError::Corrupt);
        }
        Ok(doc)
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("a plain struct serializes")
    }

    fn finish(&self, plain: Zeroizing<Vec<u8>>) -> Result<Zeroizing<[u8; 32]>> {
        let root = <[u8; 32]>::try_from(plain.as_slice()).map_err(|_| KeyRootError::Corrupt)?;
        let root = Zeroizing::new(root);
        if root_id(&root) != self.root_id {
            return Err(KeyRootError::Corrupt); // authenticated, yet not the root this file names
        }
        Ok(root)
    }

    /// Normal boot: the device key (and this machine) open the root. Any failure means "this device cannot do it by itself".
    pub fn unlock_with_device(
        &self,
        device_key: &[u8; 32],
        machine: &dyn MachineContext,
    ) -> Result<Zeroizing<[u8; 32]>> {
        let nonce = hex_fixed::<12>(&self.device.nonce)?;
        let ciphertext = hex::decode(&self.device.ciphertext).map_err(|_| KeyRootError::Corrupt)?;
        match open(
            device_key,
            &device_aad(&self.root_id, &machine.id()),
            &nonce,
            &ciphertext,
        ) {
            Ok(plain) => self.finish(plain),
            Err(KeyRootError::DecryptFailed) => Err(KeyRootError::RecoveryRequired),
            Err(e) => Err(e),
        }
    }

    /// Recovery, on any machine: the password opens the root.
    pub fn unlock_with_password(
        &self,
        password: &str,
        policy: &KdfPolicy,
    ) -> Result<Zeroizing<[u8; 32]>> {
        let params = KdfParams {
            memory_kib: self.recovery.memory_kib,
            iterations: self.recovery.iterations,
            parallelism: self.recovery.parallelism,
        };
        let salt = hex_fixed::<32>(&self.recovery.salt)?;
        let nonce = hex_fixed::<12>(&self.recovery.nonce)?;
        let ciphertext =
            hex::decode(&self.recovery.ciphertext).map_err(|_| KeyRootError::Corrupt)?;
        let kek = derive_kek(password.as_bytes(), &salt, &params, policy)?;
        match open(
            &kek,
            &recovery_aad(&self.root_id, &params),
            &nonce,
            &ciphertext,
        ) {
            Ok(plain) => self.finish(plain),
            Err(KeyRootError::DecryptFailed) => Err(KeyRootError::RecoveryFailed),
            Err(e) => Err(e),
        }
    }

    /// The same document with a NEW recovery secret (a recovery code, or a password): the old one stops working, the device wrapper, the
    /// login hash and the root are untouched. `root` must be this document's root.
    pub fn with_new_recovery(
        &self,
        root: &[u8; 32],
        secret: &str,
        params: KdfParams,
        policy: &KdfPolicy,
    ) -> Result<RootDocument> {
        if root_id(root) != self.root_id {
            return Err(KeyRootError::Corrupt);
        }
        let salt = random_bytes::<32>();
        let kek = derive_kek(secret.as_bytes(), &salt, &params, policy)?;
        let (n, c) = seal(&kek, &recovery_aad(&self.root_id, &params), root);
        let mut next = self.clone();
        next.recovery = RecoveryWrapper {
            kdf: "argon2id".into(),
            memory_kib: params.memory_kib,
            iterations: params.iterations,
            parallelism: params.parallelism,
            salt: hex::encode(salt),
            nonce: hex::encode(n),
            ciphertext: hex::encode(c),
        };
        Ok(next)
    }

    /// The same document with another web login hash (a login-password reset); nothing else changes.
    pub fn with_login_hash(&self, login_hash: &str) -> RootDocument {
        let mut next = self.clone();
        next.login_hash = login_hash.to_owned();
        next
    }

    /// The same document with the device wrapper replaced (a new device key, or a new machine); the recovery wrapper is untouched.
    pub fn rewrapped_for_device(
        &self,
        root: &[u8; 32],
        new_device_key: &[u8; 32],
        device_provider: &str,
        machine: &dyn MachineContext,
    ) -> Result<RootDocument> {
        if root_id(root) != self.root_id {
            return Err(KeyRootError::Corrupt);
        }
        let (n, c) = seal(
            new_device_key,
            &device_aad(&self.root_id, &machine.id()),
            root,
        );
        let mut next = self.clone();
        next.device = DeviceWrapper {
            provider: device_provider.to_owned(),
            nonce: hex::encode(n),
            ciphertext: hex::encode(c),
        };
        Ok(next)
    }
}
