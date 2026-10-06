//! The two operations that change the key chain: `migrate` (legacy → root v2 + identity v3, on the machine where everything still
//! opens) and `recover` (a new machine or a lost device key: the recovery password → the SAME root → a new device wrapper).
//!
//! Both are built so that a failure at any point leaves the owner's data openable: every original is backed up before it is
//! touched (a copy, never overwritten, never removed), every new file is written beside the old one and read back before it
//! replaces anything, and a failed verification restores the originals.
use crate::atomic::{backup_copy, write_atomic};
use crate::device::{DeviceKeyProvider, DeviceProtection};
use crate::error::{KeyRootError, Result};
use crate::identity_store::{
    identity_format, open_identity, seal_identity_v3, IdentityFormat, IdentityMaterial, OpenContext,
};
use crate::keydir::{read_private_file, KeyDir};
use crate::legacy::{auth_format, legacy_login_hash, legacy_master_key, AuthFormat};
use crate::machine::MachineContext;
use crate::root::RootDocument;
use crate::wrap::{KdfParams, KdfPolicy};
use std::fs;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct MigrationReport {
    pub node_id: String,
    pub backups: Vec<PathBuf>,
    pub device_key_created: bool,
    pub device_protection: DeviceProtection,
    /// True if an earlier run had already written the new identity file and only `auth.json` was left to convert.
    pub resumed: bool,
}

pub struct Migration<'a> {
    pub dir: &'a KeyDir,
    pub port: u16,
    pub machine: &'a dyn MachineContext,
    /// `YANDI_KEY_PASSWORD` of the running environment: the legacy identity may have been encrypted with it.
    pub env_password: Option<&'a str>,
    pub device: &'a dyn DeviceKeyProvider,
    pub recovery_password: &'a str,
    pub params: KdfParams,
    pub policy: &'a KdfPolicy,
}

impl Migration<'_> {
    /// Nothing is created, changed or removed unless the legacy master key AND the legacy identity both open.
    pub fn run(&self) -> Result<MigrationReport> {
        let auth_path = self.dir.auth_file();
        let identity_path = self.dir.identity_file(self.port);
        let auth_bytes = read_private_file(&auth_path)?;
        match auth_format(&auth_bytes)? {
            AuthFormat::LegacyV1 => {}
            AuthFormat::RootV2 => {
                return Err(KeyRootError::Refused(
                    "this key directory is already migrated",
                ))
            }
        }
        let root = legacy_master_key(&auth_bytes, self.machine)?;
        let login_hash = legacy_login_hash(&auth_bytes)?;
        let identity_bytes = read_private_file(&identity_path)?;
        let already_v3 = identity_format(&identity_bytes)? == IdentityFormat::RootV3;
        let ctx = OpenContext {
            machine: self.machine,
            env_password: self.env_password,
            root: Some(&root),
        };
        let material = open_identity(&identity_bytes, &ctx)?; // the legacy identity must open, or nothing happens
                                                              // a dry run of the recovery wrapper's cost and the password rule, before anything is written
        RootDocument::create(
            &root,
            &login_hash,
            &[0u8; 32],
            "dry-run",
            self.machine,
            self.recovery_password,
            self.params,
            self.policy,
        )?;

        let (device_key, device_key_created) = match self.device.load()? {
            Some(k) => (k, false),
            None => (self.device.create()?, true),
        };
        let doc = RootDocument::create(
            &root,
            &login_hash,
            &device_key,
            self.device.name(),
            self.machine,
            self.recovery_password,
            self.params,
            self.policy,
        )?;
        let doc_bytes = doc.to_json();
        let new_identity = seal_identity_v3(&root, &material);
        // in memory first: both ways in must open the root, and the identity must open with it
        self.check(&doc_bytes, &new_identity, &root, &material)?;

        let mut backups = vec![backup_copy(&auth_path, "legacy")?];
        if !already_v3 {
            backups.push(backup_copy(&identity_path, "legacy")?);
        }
        let committed = (|| -> Result<()> {
            if !already_v3 {
                write_atomic(&identity_path, &new_identity, |w| {
                    open_identity(w, &ctx)
                        .map(|m| m.same_identity(&material))
                        .unwrap_or(false)
                })?;
            }
            write_atomic(&auth_path, &doc_bytes, |w| RootDocument::parse(w).is_ok())?;
            // and once more from disk, through the device provider as the node will use it
            let on_disk_auth = read_private_file(&auth_path)?;
            let on_disk_identity = read_private_file(&identity_path)?;
            let key_now = self.device.load()?.ok_or(KeyRootError::RecoveryRequired)?;
            self.check_disk(&on_disk_auth, &on_disk_identity, &key_now, &root, &material)
        })();
        if let Err(e) = committed {
            // restore the originals (the backups are copies of them; the in-memory bytes are the originals themselves)
            let _ = write_atomic(&auth_path, &auth_bytes, |_| true);
            if !already_v3 {
                let _ = write_atomic(&identity_path, &identity_bytes, |_| true);
            }
            return Err(e);
        }
        Ok(MigrationReport {
            node_id: hex::encode(&material.address[..8]),
            backups,
            device_key_created,
            device_protection: self.device.protection(),
            resumed: already_v3,
        })
    }

    fn check(
        &self,
        doc_bytes: &[u8],
        identity_bytes: &[u8],
        root: &[u8; 32],
        material: &IdentityMaterial,
    ) -> Result<()> {
        let doc = RootDocument::parse(doc_bytes)?;
        let key = self.device.load()?.ok_or(KeyRootError::RecoveryRequired)?;
        let by_device = doc.unlock_with_device(&key, self.machine)?;
        let by_password = doc.unlock_with_password(self.recovery_password, self.policy)?;
        if *by_device != *root || *by_password != *root {
            return Err(KeyRootError::Corrupt);
        }
        let ctx = OpenContext {
            machine: self.machine,
            env_password: None,
            root: Some(root),
        };
        if !open_identity(identity_bytes, &ctx)?.same_identity(material) {
            return Err(KeyRootError::Corrupt);
        }
        Ok(())
    }

    fn check_disk(
        &self,
        auth: &[u8],
        identity: &[u8],
        device_key: &[u8; 32],
        root: &[u8; 32],
        material: &IdentityMaterial,
    ) -> Result<()> {
        let doc = RootDocument::parse(auth)?;
        if *doc.unlock_with_device(device_key, self.machine)? != *root
            || *doc.unlock_with_password(self.recovery_password, self.policy)? != *root
        {
            return Err(KeyRootError::Corrupt);
        }
        let ctx = OpenContext {
            machine: self.machine,
            env_password: None,
            root: Some(root),
        };
        if !open_identity(identity, &ctx)?.same_identity(material) {
            return Err(KeyRootError::Corrupt);
        }
        Ok(())
    }
}

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
        match auth_format(&auth_bytes)? {
            AuthFormat::RootV2 => {}
            AuthFormat::LegacyV1 => return Err(KeyRootError::Refused("a legacy key directory has no recovery wrapper; migrate it first, on the machine where it still opens")),
        }
        let doc = RootDocument::parse(&auth_bytes)?;
        let root: Zeroizing<[u8; 32]> = doc.unlock_with_password(self.password, self.policy)?;
        let identity_bytes = read_private_file(&self.dir.identity_file(self.port))?;
        let ctx = OpenContext {
            machine: self.machine,
            env_password: None,
            root: Some(&root),
        };
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
        match auth_format(&bytes)? {
            AuthFormat::RootV2 => {}
            AuthFormat::LegacyV1 => {
                return Err(KeyRootError::Refused(
                    "a legacy key directory has no recovery wrapper; migrate it first",
                ))
            }
        }
        let doc = RootDocument::parse(&bytes)?;
        let key = self.device.load()?.ok_or(KeyRootError::RecoveryRequired)?;
        doc.unlock_with_device(&key, self.machine)?; // only the device may replace the recovery secret
        Ok(PendingRecovery {
            code: crate::recovery_code::RecoveryCode::generate(),
        })
    }

    /// Writes the new wrapper only if `typed` is the shown code. The previous `auth.json` is kept as a backup.
    pub fn commit(&self, pending: &PendingRecovery, typed: &str) -> Result<PathBuf> {
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
        Ok(backup)
    }
}
