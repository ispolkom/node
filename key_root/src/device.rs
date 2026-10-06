//! The device key: a random 32-byte secret that opens the root on THIS device without asking the user for anything.
//!
//! What protects it depends on the backend, and the backend says so (`DeviceProtection`). The only backend today is a private file:
//! it keeps the key from other users and from a copy of the encrypted files taken without it, but not from anyone who can read
//! the owner's whole home directory. An operating-system key store or a hardware module can be added behind the same trait
//! (none is available offline in this build; nothing else changes when it is).
use crate::atomic::{backup_copy, create_new_file, write_atomic};
use crate::error::{KeyRootError, Result};
use crate::keydir::read_private_file;
use crate::wrap::random_bytes;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceProtection {
    /// A file readable only by the owner, next to the data it protects.
    FileOwnerOnly,
    OsKeyStore,
    Hardware,
}

impl DeviceProtection {
    pub fn describe(&self) -> &'static str {
        match self {
            DeviceProtection::FileOwnerOnly => "a private file: keeps the key from other users and from copies of the encrypted files, not from whoever can read the owner's home directory",
            DeviceProtection::OsKeyStore => "the operating system's key store",
            DeviceProtection::Hardware => "a hardware-backed key",
        }
    }
}

pub trait DeviceKeyProvider {
    fn name(&self) -> &'static str;
    fn protection(&self) -> DeviceProtection;
    /// The device key, or `None` if this device has none (a new machine, a lost key). Never creates one.
    fn load(&self) -> Result<Option<Zeroizing<[u8; 32]>>>;
    /// A brand-new key; refuses if one already exists.
    fn create(&self) -> Result<Zeroizing<[u8; 32]>>;
    /// Replace the key (recovery): the old one is kept as a backup, never destroyed.
    fn replace(&self) -> Result<Zeroizing<[u8; 32]>>;
}

pub struct FileDeviceKey {
    path: PathBuf,
}

impl FileDeviceKey {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileDeviceKey { path: path.into() }
    }
}

impl DeviceKeyProvider for FileDeviceKey {
    fn name(&self) -> &'static str {
        "file-v1"
    }

    fn protection(&self) -> DeviceProtection {
        DeviceProtection::FileOwnerOnly
    }

    fn load(&self) -> Result<Option<Zeroizing<[u8; 32]>>> {
        match read_private_file(&self.path) {
            Ok(bytes) => {
                let key =
                    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| KeyRootError::Corrupt)?;
                Ok(Some(Zeroizing::new(key)))
            }
            Err(KeyRootError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn create(&self) -> Result<Zeroizing<[u8; 32]>> {
        let key = Zeroizing::new(random_bytes::<32>());
        create_new_file(&self.path, key.as_slice())?;
        Ok(key)
    }

    fn replace(&self) -> Result<Zeroizing<[u8; 32]>> {
        if self.path.exists() {
            backup_copy(&self.path, "replaced")?;
        }
        let key = Zeroizing::new(random_bytes::<32>());
        write_atomic(&self.path, key.as_slice(), |bytes| bytes == key.as_slice())?;
        Ok(key)
    }
}
