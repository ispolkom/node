//! The root of YANDI's key chain (P1c-1).
//!
//! * [`root`] — one root key, two independent ways in: the device key (normal boot) and a recovery password (a new machine, a lost device).
//! * [`identity_store`] — the identity file formats and the loading policy: a failure to open an existing identity is never a reason to make a new one.
//! * [`migrate`] — legacy → new format on the machine where everything still opens, and recovery elsewhere; both keep the originals.
//! * [`unlock`] — how the node opens the root at start, and a read-only status.
//!
//! `/etc/machine-id` is public. Here it is only associated data (a binding label), never a key and never a source of entropy.
#![cfg(unix)]

pub mod atomic;
pub mod device;
pub mod error;
pub mod identity_store;
pub mod keydir;
pub mod legacy;
pub mod login;
pub mod machine;
pub mod migrate;
pub mod recovery_code;
pub mod root;
pub mod unlock;
pub mod wrap;

pub use device::{DeviceKeyProvider, DeviceProtection, FileDeviceKey};
pub use error::{KeyRootError, Result};
pub use identity_store::{
    load_or_initialize, IdentityFormat, IdentityMaterial, Loaded, OpenContext,
};
pub use keydir::KeyDir;
pub use machine::{FixedMachine, MachineContext, SystemMachine};
pub use migrate::{
    Migration, MigrationReport, NewRecoveryCode, PendingRecovery, Recovery, RecoveryReport,
};
pub use recovery_code::{recovery_secret, RecoveryCode};
pub use root::RootDocument;
pub use unlock::{status, unlock_root, KeyStoreStatus, RootSource};
pub use wrap::{derive_domain, KdfParams, KdfPolicy, DOMAIN_CORE, DOMAIN_IDENTITY};
pub use zeroize::Zeroizing;
