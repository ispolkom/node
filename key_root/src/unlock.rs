//! How the node gets its root key at start, and the report of what state the key directory is in.
use crate::device::DeviceKeyProvider;
use crate::error::{KeyRootError, Result};
use crate::keydir::{read_private_file, KeyDir};
use crate::legacy::{auth_format, legacy_master_key, AuthFormat};
use crate::machine::MachineContext;
use crate::root::RootDocument;
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RootSource {
    /// Legacy format: opened with the public machine id (no user secret involved).
    LegacyMachineId,
    /// Format v2: opened by this device's key.
    Device,
}

/// Open the root key the way the node does at start. Never creates, changes or removes anything.
///
/// * legacy `auth.json` (v1) → the machine-id path, exactly as before;
/// * `auth.json` v2 → the device key; if this device has none, or it does not open the wrapper (another machine, a copy without
///   the device key), the answer is `RecoveryRequired` — not an empty result, not a fresh key.
pub fn unlock_root(
    dir: &KeyDir,
    machine: &dyn MachineContext,
    device: &dyn DeviceKeyProvider,
) -> Result<(Zeroizing<[u8; 32]>, RootSource)> {
    let bytes = read_private_file(&dir.auth_file())?;
    match auth_format(&bytes)? {
        AuthFormat::LegacyV1 => match legacy_master_key(&bytes, machine) {
            Ok(k) => Ok((k, RootSource::LegacyMachineId)),
            Err(KeyRootError::DecryptFailed) => Err(KeyRootError::RecoveryRequired),
            Err(e) => Err(e),
        },
        AuthFormat::RootV2 => {
            let doc = RootDocument::parse(&bytes)?;
            match device.load()? {
                Some(key) => Ok((doc.unlock_with_device(&key, machine)?, RootSource::Device)),
                None => Err(KeyRootError::RecoveryRequired),
            }
        }
    }
}

/// A read-only description of the key directory, for `yandi-keys status` and for the node's start-up message.
#[derive(Debug)]
pub struct KeyStoreStatus {
    pub auth: std::result::Result<Option<AuthFormat>, KeyRootError>,
    pub identity: std::result::Result<Option<crate::identity_store::IdentityFormat>, KeyRootError>,
    pub other_identity_files: Vec<String>,
}

pub fn status(dir: &KeyDir, port: u16) -> KeyStoreStatus {
    let auth = match read_private_file(&dir.auth_file()) {
        Ok(b) => auth_format(&b).map(Some),
        Err(KeyRootError::NotFound) => Ok(None),
        Err(e) => Err(e),
    };
    let identity = match read_private_file(&dir.identity_file(port)) {
        Ok(b) => crate::identity_store::identity_format(&b).map(Some),
        Err(KeyRootError::NotFound) => Ok(None),
        Err(e) => Err(e),
    };
    let own = format!("node_identity_{port}.json");
    let other_identity_files = dir
        .identity_artifacts()
        .into_iter()
        .filter(|n| *n != own)
        .collect();
    KeyStoreStatus {
        auth,
        identity,
        other_identity_files,
    }
}
