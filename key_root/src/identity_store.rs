//! The node identity file and the policy for loading it.
//!
//! * v3 — private keys under `HKDF(root, "yandi/identity/v1")`; the root comes from `auth.json` v2 and opens by device or recovery
//!
//! The rule of this module: **a failure to open an existing identity is never a reason to make a new one.** `load_or_initialize`
//! creates only when nothing identity-like exists in the key directory, and it never overwrites.
use crate::atomic::create_new_file;
use crate::error::{KeyRootError, Result};
use crate::keydir::{read_private_file, KeyDir};
use crate::wrap::{derive_domain, hex_fixed, open, root_id, seal, DOMAIN_IDENTITY};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const IDENTITY_FORMAT_V3: u32 = 3;

/// Everything of a node identity that is stored, with the private parts wiped when dropped.
pub struct IdentityMaterial {
    pub address: [u8; 32],
    pub public_key: [u8; 32],
    pub signing_public_key: [u8; 32],
    pub created_at: String,
    pub private_key: Zeroizing<[u8; 32]>,
    pub signing_private_key: Zeroizing<[u8; 32]>,
}

impl IdentityMaterial {
    pub fn same_identity(&self, other: &IdentityMaterial) -> bool {
        self.address == other.address
            && self.public_key == other.public_key
            && self.signing_public_key == other.signing_public_key
            && *self.private_key == *other.private_key
            && *self.signing_private_key == *other.signing_private_key
    }

    fn secret_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut v = Zeroizing::new(Vec::with_capacity(64));
        v.extend_from_slice(self.private_key.as_slice());
        v.extend_from_slice(self.signing_private_key.as_slice());
        v
    }
}

impl std::fmt::Debug for IdentityMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "IdentityMaterial(node {}…, keys redacted)",
            hex::encode(&self.address[..4])
        )
    }
}

// ── v3 ───────────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredV3 {
    version: u32,
    address: String,
    public_key: String,
    signing_public_key: String,
    created_at: String,
    root_id: String,
    nonce: String,
    ciphertext: String,
}

fn v3_aad(s: &StoredV3) -> Vec<u8> {
    format!(
        "yandi/identity-file/v3|{}|{}|{}|{}",
        s.address, s.public_key, s.signing_public_key, s.root_id
    )
    .into_bytes()
}

pub fn seal_identity_v3(root: &[u8; 32], m: &IdentityMaterial) -> Vec<u8> {
    let mut s = StoredV3 {
        version: IDENTITY_FORMAT_V3,
        address: hex::encode(m.address),
        public_key: hex::encode(m.public_key),
        signing_public_key: hex::encode(m.signing_public_key),
        created_at: m.created_at.clone(),
        root_id: root_id(root),
        nonce: String::new(),
        ciphertext: String::new(),
    };
    let key = derive_domain(root, DOMAIN_IDENTITY);
    let (n, c) = seal(&key, &v3_aad(&s), &m.secret_bytes());
    s.nonce = hex::encode(n);
    s.ciphertext = hex::encode(c);
    serde_json::to_vec_pretty(&s).expect("a plain struct serializes")
}

fn open_v3(root: &[u8; 32], value: serde_json::Value) -> Result<IdentityMaterial> {
    let s: StoredV3 = serde_json::from_value(value).map_err(|_| KeyRootError::Corrupt)?;
    let address = hex_fixed::<32>(&s.address)?;
    let public_key = hex_fixed::<32>(&s.public_key)?;
    let signing_public_key = hex_fixed::<32>(&s.signing_public_key)?;
    let nonce = hex_fixed::<12>(&s.nonce)?;
    hex_fixed::<16>(&s.root_id)?;
    let ciphertext = hex::decode(&s.ciphertext).map_err(|_| KeyRootError::Corrupt)?;
    let key = derive_domain(root, DOMAIN_IDENTITY);
    let plain = open(&key, &v3_aad(&s), &nonce, &ciphertext)?;
    split_secret(plain, address, public_key, signing_public_key, s.created_at)
}

fn split_secret(
    plain: Zeroizing<Vec<u8>>,
    address: [u8; 32],
    public_key: [u8; 32],
    signing_public_key: [u8; 32],
    created_at: String,
) -> Result<IdentityMaterial> {
    if plain.len() != 64 {
        return Err(KeyRootError::Corrupt);
    }
    let mut private_key = Zeroizing::new([0u8; 32]);
    let mut signing_private_key = Zeroizing::new([0u8; 32]);
    private_key.copy_from_slice(&plain[..32]);
    signing_private_key.copy_from_slice(&plain[32..]);
    Ok(IdentityMaterial {
        address,
        public_key,
        signing_public_key,
        created_at,
        private_key,
        signing_private_key,
    })
}

// ── the format decision and the loading policy ─────────────────────────────────────────────────────────────────

/// What the file is, decided from its version field alone.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum IdentityFormat {
    RootV3,
}

pub fn identity_format(bytes: &[u8]) -> Result<IdentityFormat> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| KeyRootError::Corrupt)?;
    if !value.is_object() {
        return Err(KeyRootError::Corrupt);
    }
    match value.get("version").and_then(|v| v.as_u64()) {
        Some(3) => Ok(IdentityFormat::RootV3),
        Some(v) => Err(KeyRootError::UnsupportedVersion(v as u32)),
        None => Err(KeyRootError::Corrupt),
    }
}

/// Everything needed to open an identity.
pub struct OpenContext<'a> {
    /// The root, once the device or the recovery password has produced it.
    pub root: Option<&'a [u8; 32]>,
}

/// Open the identity file bytes. Pure: no file is created, changed or removed. Without a root it is `RecoveryRequired`.
pub fn open_identity(bytes: &[u8], ctx: &OpenContext) -> Result<IdentityMaterial> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| KeyRootError::Corrupt)?;
    match identity_format(bytes)? {
        IdentityFormat::RootV3 => match ctx.root {
            Some(root) => open_v3(root, value),
            None => Err(KeyRootError::RecoveryRequired),
        },
    }
}

/// Read and open the identity of `port` from the key directory. `NotFound` only if the file really is absent.
pub fn load_identity(dir: &KeyDir, port: u16, ctx: &OpenContext) -> Result<IdentityMaterial> {
    let bytes = read_private_file(&dir.identity_file(port))?;
    open_identity(&bytes, ctx)
}

pub struct Loaded {
    pub identity: IdentityMaterial,
    /// True if this call created the identity (a first run); false if it opened an existing one.
    pub created: bool,
}

/// The only entry the node uses. Existing and openable → loaded. Existing and NOT openable → an error, and nothing on disk changes.
/// Absent → created, but only when no identity-like file of any port exists in the directory, and never over an existing file.
pub fn load_or_initialize(
    dir: &KeyDir,
    port: u16,
    ctx: &OpenContext,
    make: impl FnOnce() -> IdentityMaterial,
) -> Result<Loaded> {
    let path = dir.identity_file(port);
    match read_private_file(&path) {
        Ok(bytes) => Ok(Loaded {
            identity: open_identity(&bytes, ctx)?,
            created: false,
        }),
        Err(KeyRootError::NotFound) => {
            let others = dir.identity_artifacts();
            if !others.is_empty() {
                return Err(KeyRootError::Refused("identity files exist here but not for this port; refusing to create a second identity"));
            }
            let root = ctx.root.ok_or(KeyRootError::RecoveryRequired)?;
            let identity = make();
            create_new_file(&path, &seal_identity_v3(root, &identity))?; // create_new: an existing file (a race) is an error, never overwritten
            Ok(Loaded {
                identity,
                created: true,
            })
        }
        Err(e) => Err(e),
    }
}
