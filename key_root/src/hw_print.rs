//! Отпечаток железа узла: четыре части (плата, процессор, сетевые карты, диски).
//!
//! Ключ данных запечатывается четырьмя обёртками — по одной на каждые три части
//! из четырёх. Узел открывается сам, пока совпадают любые три части: замена одной
//! карты или диска его не ломает. Совпали две и меньше (диск унесли на другой ПК) —
//! обёртки не открываются, и нужен пароль или сид-фраза.
//!
//! Части читаются без root из `/sys`, `/proc`, `/dev/disk/by-id`. IP не используется.
//! Значения не хранятся: на диске только обёртки, соль и хэш полного отпечатка (чтобы тихо обновить слепок,
//! когда изменилась одна часть).

use std::fs;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

/// Цена одной обёртки в бою: 64 МиБ, 3 прохода (около четверти секунды), четыре обёртки при каждом старте.
pub const COST_MEM_KIB: u32 = 64 * 1024;
pub const COST_PASSES: u32 = 3;

pub const PARTS: usize = 4;
const NAMES: [&str; PARTS] = ["board", "cpu", "nics", "disks"];

/// Четыре части отпечатка; в каждой — отсортированный список значений.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct HwPrint {
    pub parts: [Vec<String>; PARTS],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Wrap {
    pub nonce: [u8; 12],
    #[serde(with = "hex_vec")]
    pub ct: Vec<u8>,
}

/// То, что лежит на диске: соль и четыре обёртки ключа данных.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sealed {
    pub salt: [u8; 16],
    pub mem_kib: u32,
    pub passes: u32,
    /// sha256(соль, полный отпечаток) в hex: показывает, что слепок уже свежий
    pub full: String,
    pub wraps: Vec<Wrap>,
}

mod hex_vec {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(s).map_err(serde::de::Error::custom)
    }
}

impl HwPrint {
    /// Отпечаток этого компьютера.
    pub fn collect() -> HwPrint {
        HwPrint::collect_from(Path::new("/"))
    }

    /// Чтение из произвольного корня (в тестах — поддельное дерево).
    pub fn collect_from(root: &Path) -> HwPrint {
        let read = |rel: &str| {
            fs::read_to_string(root.join(rel))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };

        let board: Vec<String> = ["board_vendor", "board_name", "product_name", "sys_vendor"]
            .iter()
            .map(|n| read(&format!("sys/class/dmi/id/{n}")))
            .filter(|v| !v.is_empty())
            .collect();

        let cpu: Vec<String> = fs::read_to_string(root.join("proc/cpuinfo"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| v.trim().to_string())
            })
            .filter(|v| !v.is_empty())
            .into_iter()
            .collect();

        // Физическая карта имеет `device`; docker, veth, tun и мосты — нет.
        let mut nics = Vec::new();
        if let Ok(rd) = fs::read_dir(root.join("sys/class/net")) {
            for e in rd.flatten() {
                if !e.path().join("device").exists() {
                    continue;
                }
                let mac = fs::read_to_string(e.path().join("address"))
                    .map(|s| s.trim().to_lowercase())
                    .unwrap_or_default();
                if mac.is_empty() || mac.chars().all(|c| c == '0' || c == ':') {
                    continue;
                }
                nics.push(mac);
            }
        }

        // Постоянные диски: ata-/nvme-/scsi- с моделью и серийником; разделы,
        // wwn-дубликаты и USB-флешки не считаются.
        let mut disks = Vec::new();
        if let Ok(rd) = fs::read_dir(root.join("dev/disk/by-id")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                let fixed = n.starts_with("ata-")
                    || n.starts_with("nvme-")
                    || n.starts_with("scsi-");
                if fixed && !n.contains("-part") {
                    disks.push(n);
                }
            }
        }

        let mut p = HwPrint { parts: [board, cpu, nics, disks] };
        for v in p.parts.iter_mut() {
            v.sort();
            v.dedup();
        }
        p
    }

    /// Сколько частей удалось прочитать.
    pub fn filled(&self) -> usize {
        self.parts.iter().filter(|p| !p.is_empty()).count()
    }

    /// Достаточно ли частей, чтобы привязка имела смысл (три из четырёх).
    pub fn usable(&self) -> bool {
        self.filled() >= PARTS - 1
    }

    fn canonical(&self, i: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(NAMES[i].as_bytes());
        for v in &self.parts[i] {
            out.push(0);
            out.extend_from_slice(v.as_bytes());
        }
        out
    }

    fn full_digest(&self, salt: &[u8; 16]) -> String {
        let mut h = Sha256::new();
        h.update(salt);
        for i in 0..PARTS {
            let c = self.canonical(i);
            h.update((c.len() as u32).to_be_bytes());
            h.update(&c);
        }
        hex::encode(h.finalize())
    }

    /// Ключ обёртки для набора «все части, кроме `skip`».
    fn subset_key(&self, skip: usize, salt: &[u8; 16], mem_kib: u32, passes: u32) -> [u8; 32] {
        let mut input = Vec::new();
        for i in 0..PARTS {
            if i == skip {
                continue;
            }
            let c = self.canonical(i);
            input.extend_from_slice(&(c.len() as u32).to_be_bytes());
            input.extend_from_slice(&c);
        }
        // Подбор комбинаций дорогой: Argon2id с памятью `mem_kib`.
        let params = Params::new(mem_kib, passes, 1, Some(32)).expect("argon2 params");
        let mut key = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(&input, salt, &mut key)
            .expect("argon2");
        input.zeroize();
        key
    }

    /// Запечатать ключ данных под этот отпечаток (цена подбора — боевая).
    pub fn seal(&self, data_key: &[u8; 32]) -> Sealed {
        self.seal_with_cost(data_key, COST_MEM_KIB, COST_PASSES)
    }

    /// То же с явной ценой (тесты берут меньшую).
    pub fn seal_with_cost(&self, data_key: &[u8; 32], mem_kib: u32, passes: u32) -> Sealed {
        let mut salt = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let mut wraps = Vec::with_capacity(PARTS);
        for skip in 0..PARTS {
            let mut key = self.subset_key(skip, &salt, mem_kib, passes);
            let cipher = Aes256Gcm::new_from_slice(&key).expect("key");
            key.zeroize();
            let mut nonce = [0u8; 12];
            rand::rngs::OsRng.fill_bytes(&mut nonce);
            let ct = cipher
                .encrypt(Nonce::from_slice(&nonce), data_key.as_slice())
                .expect("aes-gcm");
            wraps.push(Wrap { nonce, ct });
        }
        Sealed { salt, mem_kib, passes, full: self.full_digest(&salt), wraps }
    }
}

impl Sealed {
    /// Слепок уже соответствует этому отпечатку (ни одна часть не изменилась).
    pub fn is_current(&self, hw: &HwPrint) -> bool {
        self.full == hw.full_digest(&self.salt)
    }

    /// Открыть ключ данных текущим железом. `None` — совпало меньше трёх частей.
    pub fn open(&self, hw: &HwPrint) -> Option<[u8; 32]> {
        // файл не может «удешевить» подбор ниже разумного минимума и не может потребовать лишнего
        if self.wraps.len() != PARTS
            || self.mem_kib < 8 * 1024
            || self.mem_kib > 1024 * 1024
            || self.passes == 0
            || self.passes > 16
        {
            return None;
        }
        for skip in 0..PARTS {
            let mut key = hw.subset_key(skip, &self.salt, self.mem_kib, self.passes);
            let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
            key.zeroize();
            let w = &self.wraps[skip];
            if let Ok(mut pt) = cipher.decrypt(Nonce::from_slice(&w.nonce), w.ct.as_slice()) {
                if pt.len() == 32 {
                    let mut out = [0u8; 32];
                    out.copy_from_slice(&pt);
                    pt.zeroize();
                    return Some(out);
                }
                pt.zeroize();
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print() -> HwPrint {
        HwPrint {
            parts: [
                vec!["Gigabyte".into(), "Z390 UD".into()],
                vec!["Intel i5-8500".into()],
                vec!["00:e0:53:40:1d:d8".into(), "d8:5e:d3:22:cb:ae".into()],
                vec!["ata-KINGSTON_X".into(), "ata-WDC_Y".into()],
            ],
        }
    }

    #[test]
    fn same_hardware_opens_and_one_changed_part_still_opens() {
        let key = [7u8; 32];
        let sealed = print().seal_with_cost(&key, 8 * 1024, 1);
        assert_eq!(sealed.open(&print()), Some(key));

        for part in 0..PARTS {
            let mut changed = print();
            changed.parts[part] = vec!["другое".into()];
            assert_eq!(sealed.open(&changed), Some(key), "часть {part}");
        }
    }

    #[test]
    fn two_changed_parts_do_not_open() {
        let sealed = print().seal_with_cost(&[9u8; 32], 8 * 1024, 1);
        for a in 0..PARTS {
            for b in a + 1..PARTS {
                let mut changed = print();
                changed.parts[a] = vec!["чужое".into()];
                changed.parts[b] = vec!["чужое".into()];
                assert_eq!(sealed.open(&changed), None, "части {a}+{b}");
            }
        }
    }

    #[test]
    fn collects_without_root_from_a_fake_tree_and_skips_virtual_things() {
        let d = tempfile::tempdir().unwrap();
        let w = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        };
        w("sys/class/dmi/id/board_name", "Z390 UD\n");
        w("sys/class/dmi/id/board_vendor", "Gigabyte\n");
        w("proc/cpuinfo", "processor : 0\nmodel name\t: Intel i5\n");
        w("sys/class/net/enp3s0/address", "AA:BB:CC:00:00:01\n");
        w("sys/class/net/enp3s0/device/x", "");
        w("sys/class/net/docker0/address", "02:42:00:00:00:01\n");
        w("sys/class/net/lo/address", "00:00:00:00:00:00\n");
        w("dev/disk/by-id/ata-SSD_123", "");
        w("dev/disk/by-id/ata-SSD_123-part1", "");
        w("dev/disk/by-id/wwn-0x5000", "");
        w("dev/disk/by-id/usb-Flash_1", "");

        let p = HwPrint::collect_from(d.path());
        assert_eq!(p.parts[0], vec!["Gigabyte", "Z390 UD"]);
        assert_eq!(p.parts[1], vec!["Intel i5"]);
        assert_eq!(p.parts[2], vec!["aa:bb:cc:00:00:01"]);
        assert_eq!(p.parts[3], vec!["ata-SSD_123"]);
        assert!(p.usable());
    }

    #[test]
    fn empty_machine_is_not_usable_and_sealed_survives_serde() {
        assert!(!HwPrint::default().usable());
        let s = print().seal_with_cost(&[1u8; 32], 8 * 1024, 1);
        let back: Sealed = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.open(&print()), Some([1u8; 32]));
    }
}
