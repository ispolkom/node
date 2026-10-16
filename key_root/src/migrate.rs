//! The operations that change the key chain: `recover` (a new machine or a lost device key: the recovery password → the SAME root →
//! a new device wrapper) and a new recovery code.
//!
//! They are built so that a failure at any point leaves the owner's data openable: every original is backed up before it is
//! touched (a copy, never overwritten, never removed), every new file is written beside the old one and read back before it
//! replaces anything, and a failed verification restores the originals.
use crate::atomic::{backup_copy, write_atomic};
use crate::device::{DeviceKeyProvider, DeviceProtection};
use crate::error::{KeyRootError, Result};
use crate::identity_store::{open_identity, OpenContext};
use crate::keydir::{read_private_file, KeyDir};
use crate::machine::MachineContext;
use crate::root::RootDocument;
use crate::wrap::{KdfParams, KdfPolicy};
use std::fs;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct RecoveryReport {
    pub node_id: String,
    pub backups: Vec<PathBuf>,
    pub device_protection: DeviceProtection,
}

pub struct Recovery<'a> {
    pub dir: &'a KeyDir,
    pub port: u16,
    /// The machine the keys are being recovered ONTO (its id becomes the new device binding).
    pub machine: &'a dyn MachineContext,
    pub device: &'a dyn DeviceKeyProvider,
    pub password: &'a str,
    pub policy: &'a KdfPolicy,
}

impl Recovery<'_> {
    /// The password opens the root; the identity must open with that root (the semantic check that this really is the owner's
    /// identity, not merely a password that happened to authenticate); only then is a new device key made and `auth.json`
    /// re-wrapped. A wrong password, a damaged file or a missing identity changes nothing on disk.
    pub fn run(&self) -> Result<RecoveryReport> {
        let auth_path = self.dir.auth_file();
        let auth_bytes = read_private_file(&auth_path)?;
        let doc = RootDocument::parse(&auth_bytes)?;
        let root: Zeroizing<[u8; 32]> = doc.unlock_with_password(self.password, self.policy)?;
        let identity_bytes = read_private_file(&self.dir.identity_file(self.port))?;
        let ctx = OpenContext { root: Some(&root) };
        let material = open_identity(&identity_bytes, &ctx)?;

        let mut backups = vec![backup_copy(&auth_path, "before-recovery")?];
        let new_key = match self.device.load() {
            Ok(Some(_)) => self.device.replace()?,
            Ok(None) => self.device.create()?,
            Err(KeyRootError::Corrupt) | Err(KeyRootError::PermissionDenied) => {
                self.device.replace()?
            }
            Err(e) => return Err(e),
        };
        let next = doc.rewrapped_for_device(&root, &new_key, self.device.name(), self.machine)?;
        let next_bytes = next.to_json();
        write_atomic(&auth_path, &next_bytes, |w| {
            RootDocument::parse(w)
                .and_then(|d| d.unlock_with_device(&new_key, self.machine))
                .map(|r| *r == *root)
                .unwrap_or(false)
        })?;
        // the next start must work with no password: prove it from disk, through the provider
        let key_now = self.device.load()?.ok_or(KeyRootError::RecoveryRequired)?;
        let again = RootDocument::parse(&read_private_file(&auth_path)?)?
            .unlock_with_device(&key_now, self.machine)?;
        if *again != *root || !open_identity(&identity_bytes, &ctx)?.same_identity(&material) {
            let _ = write_atomic(&auth_path, &auth_bytes, |_| true);
            return Err(KeyRootError::Corrupt);
        }
        backups.retain(|b| fs::metadata(b).is_ok());
        Ok(RecoveryReport {
            node_id: hex::encode(&material.address[..8]),
            backups,
            device_protection: self.device.protection(),
        })
    }
}

/// Replace the recovery secret by a fresh, system-made recovery code, with the DEVICE opening the root (no old password is needed, and the
/// identity, the root and the chats are untouched). Two steps so that nothing changes until the owner has proved the code was copied:
/// `prepare` makes and returns the code, `commit` writes it only if the owner typed it back correctly.
pub struct NewRecoveryCode<'a> {
    pub dir: &'a KeyDir,
    pub machine: &'a dyn MachineContext,
    pub device: &'a dyn DeviceKeyProvider,
    pub params: KdfParams,
    pub policy: &'a KdfPolicy,
}

pub struct PendingRecovery {
    code: crate::recovery_code::RecoveryCode,
}

impl PendingRecovery {
    /// The code to show, once, and never to store.
    pub fn code(&self) -> &crate::recovery_code::RecoveryCode {
        &self.code
    }
}

impl NewRecoveryCode<'_> {
    pub fn prepare(&self) -> Result<PendingRecovery> {
        let bytes = read_private_file(&self.dir.auth_file())?;
        let doc = RootDocument::parse(&bytes)?;
        let key = self.device.load()?.ok_or(KeyRootError::RecoveryRequired)?;
        doc.unlock_with_device(&key, self.machine)?; // only the device may replace the recovery secret
        Ok(PendingRecovery {
            code: crate::recovery_code::RecoveryCode::generate(),
        })
    }

    /// Writes the new wrapper only if `typed` is the shown code. The previous `auth.json` is kept as a backup only while the write is
    /// in doubt: once the new document is verified the backup is removed, because it still holds the OLD recovery wrapper of the same
    /// root and would keep the replaced secret working for anyone who copies the directory.
    pub fn commit(&self, pending: &PendingRecovery, typed: &str) -> Result<()> {
        let typed = crate::recovery_code::RecoveryCode::parse(typed)?;
        if !typed.same_as(&pending.code) {
            return Err(KeyRootError::Invalid(
                "what you typed is not the code that was shown; nothing was changed",
            ));
        }
        let auth_path = self.dir.auth_file();
        let bytes = read_private_file(&auth_path)?;
        let doc = RootDocument::parse(&bytes)?;
        let key = self.device.load()?.ok_or(KeyRootError::RecoveryRequired)?;
        let root = doc.unlock_with_device(&key, self.machine)?;
        let next = doc.with_new_recovery(&root, pending.code.secret(), self.params, self.policy)?;
        let next_bytes = next.to_json();
        let backup = backup_copy(&auth_path, "before-new-code")?;
        write_atomic(&auth_path, &next_bytes, |w| {
            RootDocument::parse(w)
                .and_then(|d| {
                    let by_code = d.unlock_with_password(pending.code.secret(), self.policy)?;
                    let by_device = d.unlock_with_device(&key, self.machine)?;
                    Ok(*by_code == *root && *by_device == *root)
                })
                .unwrap_or(false)
        })?;
        // an I/O error here is reported: the old wrapper would stay readable beside the new one
        std::fs::remove_file(&backup).map_err(|_| KeyRootError::Io)?;
        Ok(())
    }
}
