//! The root of YANDI's key chain (P1c-1).
//!
//! * [`root`] — one root key, two independent ways in: the device key (normal boot) and a recovery password (a new machine, a lost device).
//! * [`identity_store`] — the identity file formats and the loading policy: a failure to open an existing identity is never a reason to make a new one.
//! * [`migrate`] — recovery on a new machine or after a lost device key, and a new recovery code; both keep the originals.
//! * [`unlock`] — how the node opens the root at start, and a read-only status.
//!
//! `/etc/machine-id` is public. Here it is only associated data (a binding label), never a key and never a source of entropy.
#![cfg(unix)]

pub mod atomic;
pub mod device;
pub mod error;
pub mod hw_print;
pub mod identity_store;
pub mod keydir;
pub mod login;
pub mod machine;
pub mod migrate;
pub mod recovery_code;
pub mod root;
pub mod seed;
pub mod unlock;
pub mod wrap;

pub use device::{DeviceKeyProvider, DeviceProtection, FileDeviceKey, HardwareDeviceKey};
pub use error::{KeyRootError, Result};
pub use identity_store::{
    load_or_initialize, IdentityFormat, IdentityMaterial, Loaded, OpenContext,
};
pub use keydir::KeyDir;
pub use machine::{FixedMachine, MachineContext, SystemMachine};
pub use migrate::{
    NewRecoveryCode, PendingRecovery, Recovery, RecoveryReport,
};
pub use recovery_code::{recovery_secret, RecoveryCode};
pub use root::RootDocument;
pub use seed::{looks_like_seed, Seed, SeedError};
pub use unlock::{status, unlock_root, KeyStoreStatus, RootSource};
pub use wrap::{derive_domain, KdfParams, KdfPolicy, DOMAIN_CORE, DOMAIN_IDENTITY};
pub use zeroize::Zeroizing;
