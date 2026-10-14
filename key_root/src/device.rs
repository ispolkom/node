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
    /// Sealed to the machine's hardware identifiers (board, CPU, network cards, disks): opens on this hardware by itself,
    /// not on a copy of the disk. Root on the running machine can still read it.
    HardwareBound,
}

impl DeviceProtection {
    pub fn describe(&self) -> &'static str {
        match self {
            DeviceProtection::FileOwnerOnly => "a private file: keeps the key from other users and from copies of the encrypted files, not from whoever can read the owner's home directory",
            DeviceProtection::OsKeyStore => "the operating system's key store",
            DeviceProtection::Hardware => "a hardware-backed key",
            DeviceProtection::HardwareBound => "sealed to this machine's hardware (board, CPU, network cards, disks): a copy of the disk does not open it elsewhere; root on the running machine still can",
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
    pub(crate) path: PathBuf,
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

/// The device key sealed to the hardware fingerprint (`hw_print`): any three of four parts (board, CPU, network cards, disks)
/// must match, so replacing one card or disk does not break the node, while a disk copied to another computer does not open.
/// Where the hardware cannot be read well enough (containers, some mini PCs) it falls back to the private file, and says so
/// through `protection()`. An older `device.key` file is sealed on first load and removed once the seal is verified.
pub struct HardwareDeviceKey {
    seal_path: PathBuf,
    file: FileDeviceKey,
    print: Box<dyn Fn() -> crate::hw_print::HwPrint + Send + Sync>,
    mem_kib: u32,
    passes: u32,
}

impl HardwareDeviceKey {
    pub fn new(seal_path: impl Into<PathBuf>, file_path: impl Into<PathBuf>) -> Self {
        Self::with_print(
            seal_path,
            file_path,
            Box::new(crate::hw_print::HwPrint::collect),
            crate::hw_print::COST_MEM_KIB,
            crate::hw_print::COST_PASSES,
        )
    }

    /// With an explicit fingerprint source and cost (tests, tools acting for another machine).
    pub fn with_print(
        seal_path: impl Into<PathBuf>,
        file_path: impl Into<PathBuf>,
        print: Box<dyn Fn() -> crate::hw_print::HwPrint + Send + Sync>,
        mem_kib: u32,
        passes: u32,
    ) -> Self {
        HardwareDeviceKey {
            seal_path: seal_path.into(),
            file: FileDeviceKey::new(file_path),
            print,
            mem_kib,
            passes,
        }
    }

    fn write_seal(&self, key: &[u8; 32], replace: bool) -> Result<()> {
        let now = (self.print)();
        let sealed = now.seal_with_cost(key, self.mem_kib, self.passes);
        let bytes = serde_json::to_vec(&sealed).map_err(|_| KeyRootError::Corrupt)?;
        if replace {
            write_atomic(&self.seal_path, &bytes, |b| {
                serde_json::from_slice::<crate::hw_print::Sealed>(b)
                    .ok()
                    .and_then(|s| s.open(&now))
                    .as_ref()
                    == Some(key)
            })
        } else {
            create_new_file(&self.seal_path, &bytes)
        }
    }
}

impl DeviceKeyProvider for HardwareDeviceKey {
    fn name(&self) -> &'static str {
        "hardware-v1"
    }

    fn protection(&self) -> DeviceProtection {
        if self.seal_path.exists() || (self.print)().usable() {
            DeviceProtection::HardwareBound
        } else {
            DeviceProtection::FileOwnerOnly
        }
    }

    fn load(&self) -> Result<Option<Zeroizing<[u8; 32]>>> {
        match read_private_file(&self.seal_path) {
            Ok(bytes) => {
                let sealed: crate::hw_print::Sealed =
                    serde_json::from_slice(&bytes).map_err(|_| KeyRootError::Corrupt)?;
                let now = (self.print)();
                match sealed.open(&now) {
                    Some(key) => {
                        // изменилась одна часть: слепок тихо обновляется под текущее железо
                        if !sealed.is_current(&now) {
                            let _ = self.write_seal(&key, true);
                        }
                        Ok(Some(Zeroizing::new(key)))
                    }
                    // чужое железо: ключ устройства здесь нет, нужен пароль или код восстановления
                    None => Ok(None),
                }
            }
            Err(KeyRootError::NotFound) => {
                let Some(key) = self.file.load()? else {
                    return Ok(None);
                };
                // прежний файловый ключ: запечатываем, и только после проверки убираем открытый файл
                if (self.print)().usable() && self.write_seal(&key, false).is_ok() {
                    let sealed_ok = read_private_file(&self.seal_path)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<crate::hw_print::Sealed>(&b).ok())
                        .and_then(|s| s.open(&(self.print)()))
                        .as_ref()
                        == Some(&*key);
                    if sealed_ok {
                        let _ = std::fs::remove_file(&self.file.path);
                    }
                }
                Ok(Some(key))
            }
            Err(e) => Err(e),
        }
    }

    fn create(&self) -> Result<Zeroizing<[u8; 32]>> {
        if !(self.print)().usable() {
            return self.file.create();
        }
        let key = Zeroizing::new(random_bytes::<32>());
        self.write_seal(&key, false)?;
        Ok(key)
    }

    fn replace(&self) -> Result<Zeroizing<[u8; 32]>> {
        if !(self.print)().usable() {
            return self.file.replace();
        }
        if self.seal_path.exists() {
            backup_copy(&self.seal_path, "replaced")?;
        }
        let key = Zeroizing::new(random_bytes::<32>());
        self.write_seal(&key, true)?;
        Ok(key)
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;
    use crate::hw_print::HwPrint;
    use std::sync::{Arc, Mutex};

    fn print(nic: &str) -> HwPrint {
        HwPrint {
            parts: [
                vec!["Gigabyte".into(), "Z390".into()],
                vec!["i5".into()],
                vec![nic.into()],
                vec!["ata-SSD_1".into()],
            ],
        }
    }

    fn provider(
        dir: &std::path::Path,
        now: Arc<Mutex<HwPrint>>,
    ) -> HardwareDeviceKey {
        HardwareDeviceKey::with_print(
            dir.join("device.seal"),
            dir.join("device.key"),
            Box::new(move || now.lock().unwrap().clone()),
            8 * 1024,
            1,
        )
    }

    #[test]
    fn seals_to_the_hardware_follows_one_change_and_refuses_another_machine() {
        let d = tempfile::tempdir().unwrap();
        let now = Arc::new(Mutex::new(print("aa:aa")));
        let p = provider(d.path(), now.clone());
        assert!(p.load().unwrap().is_none());
        let key = p.create().unwrap();
        assert_eq!(*p.load().unwrap().unwrap(), *key);
        assert!(!d.path().join("device.key").exists(), "ключ не лежит открытым файлом");

        // заменили сетевую карту: узел поднимается сам и слепок обновляется
        *now.lock().unwrap() = print("bb:bb");
        assert_eq!(*p.load().unwrap().unwrap(), *key);
        let sealed: crate::hw_print::Sealed =
            serde_json::from_slice(&std::fs::read(d.path().join("device.seal")).unwrap()).unwrap();
        assert!(sealed.is_current(&print("bb:bb")));

        // другой компьютер (плата, процессор, карты и диски другие): ключа нет, нужен пароль
        let other = Arc::new(Mutex::new(HwPrint {
            parts: [
                vec!["ASUS".into()],
                vec!["Ryzen".into()],
                vec!["cc:cc".into()],
                vec!["nvme-X".into()],
            ],
        }));
        let moved = provider(d.path(), other);
        assert!(moved.load().unwrap().is_none());
    }

    #[test]
    fn an_old_key_file_is_sealed_and_removed_and_unreadable_hardware_falls_back_to_the_file() {
        let d = tempfile::tempdir().unwrap();
        let now = Arc::new(Mutex::new(print("aa:aa")));
        let old = FileDeviceKey::new(d.path().join("device.key")).create().unwrap();
        let p = provider(d.path(), now);
        assert_eq!(*p.load().unwrap().unwrap(), *old);
        assert!(!d.path().join("device.key").exists());
        assert_eq!(*p.load().unwrap().unwrap(), *old, "дальше открывается уже по печати");

        // контейнер без железа: ключ остаётся файлом, и это видно по защите
        let d2 = tempfile::tempdir().unwrap();
        let bare = provider(d2.path(), Arc::new(Mutex::new(HwPrint::default())));
        let k = bare.create().unwrap();
        assert!(d2.path().join("device.key").exists());
        assert_eq!(bare.protection(), DeviceProtection::FileOwnerOnly);
        assert_eq!(*bare.load().unwrap().unwrap(), *k);
    }
}
