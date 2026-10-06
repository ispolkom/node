// src/protocol/wagon.rs
//!
//! # Wagon (Вагон)
//!
//! Единица передачи в YTP. Один вагон = один UDP пакет.

use serde::{Serialize, Deserialize};
use sha2::{Sha256, Digest};

/// Вагон поезда - единица передачи в YTP
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wagon {
    /// ID поезда (уникален для каждой передачи)
    pub train_id: u64,

    /// 🔄 Это клон? (для dual-path)
    pub is_clone: bool,

    /// Номер вагона (0..total_wagons) - сохранён для совместимости
    pub wagon_num: u32,

    /// Общее количество вагонов в поезде
    pub total_wagons: u32,

    /// ⚡ QUIC-style offset - смещение в потоке данных
    pub offset: u64,

    /// 🚂 ID линии (0-9 для мультилинейной архитектуры)
    pub line_id: u8,

    /// Размер груза в этом вагоне
    pub cargo_size: u32,

    /// Флаги вагона
    pub flags: WagonFlags,

    /// Контрольная сумма груза (SHA256)
    pub checksum: [u8; 32],

    /// Груз (данные)
    pub cargo: Vec<u8>,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WagonFlags: u8 {
        /// Первый вагон в поезде
        const FIRST = 0b00000001;

        /// Последний вагон в поезде
        const LAST  = 0b00000010;

        /// Продолжение (средний вагон)
        const CONT  = 0b00000100;

        /// Декои-вагон (пустышка для маскировки)
        const DECOY = 0b00001000;

        /// Экспресс-вагон (высокий приоритет)
        const EXPRESS = 0b00010000;
    }
}

// Ручная реализация Serialize/Deserialize для bitflags
impl Serialize for WagonFlags {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(self.bits())
    }
}

impl<'de> Deserialize<'de> for WagonFlags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bits = u8::deserialize(deserializer)?;
        Ok(WagonFlags::from_bits_truncate(bits))
    }
}

impl Default for WagonFlags {
    fn default() -> Self {
        Self::CONT
    }
}

impl Wagon {
    /// Минимальный размер груза в одном вагоне (backup-эталон: 800 байт).
    pub const MIN_CARGO_SIZE: usize = 800;

    /// Максимальный размер груза в одном вагоне — backup-эталон **60 KB**.
    /// IP-уровень делает фрагментацию на 1500-MTU прозрачно; модель транспорта —
    /// «избыточная отправка через 2 path + dedup», потеря IP-фрагмента
    /// перекрывается клоном на path1. См. NETWORK_EVOLUTION discussion с автором.
    pub const MAX_CARGO_SIZE: usize = 61440;

    /// Генерировать случайный размер wagon'а
    pub fn random_wagon_size() -> usize {
        use rand::Rng;
        rand::thread_rng().gen_range(Self::MIN_CARGO_SIZE..=Self::MAX_CARGO_SIZE)
    }

    /// Создать новый вагон
    pub fn new(
        train_id: u64,
        wagon_num: u32,
        total_wagons: u32,
        offset: u64,  // ⚡ QUIC-style offset
        cargo: Vec<u8>,
        line_id: u8,  // 🚂 ID линии
    ) -> Self {
        Self::with_clone(train_id, false, wagon_num, total_wagons, offset, cargo, line_id)
    }

    /// 🔄 Создать вагон с флагом is_clone
    pub fn with_clone(
        train_id: u64,
        is_clone: bool,
        wagon_num: u32,
        total_wagons: u32,
        offset: u64,
        cargo: Vec<u8>,
        line_id: u8,
    ) -> Self {
        let cargo_size = cargo.len() as u32;

        // Вычисляем флаги
        let flags = if total_wagons == 1 {
            WagonFlags::FIRST | WagonFlags::LAST
        } else if wagon_num == 0 {
            WagonFlags::FIRST
        } else if wagon_num == total_wagons - 1 {
            WagonFlags::LAST
        } else {
            WagonFlags::CONT
        };

        // Вычисляем checksum
        let checksum = Self::compute_checksum(&cargo);

        Self {
            train_id,
            is_clone,
            wagon_num,
            total_wagons,
            offset,
            line_id,
            cargo_size,
            flags,
            checksum,
            cargo,
        }
    }

    /// Создать вагон со случайным размером груза (для защиты от DPI fingerprinting)
    pub fn new_random_size(
        train_id: u64,
        wagon_num: u32,
        total_wagons: u32,
        offset: u64,
        cargo: Vec<u8>,
        line_id: u8,  // 🚂 ID линии
    ) -> Self {
        // Генерируем случайный максимальный размер
        let max_size = Self::random_wagon_size();

        // Обрезаем груз если нужно (или оставляем как есть)
        let truncated = if cargo.len() > max_size {
            cargo.into_iter().take(max_size).collect()
        } else {
            cargo
        };

        // Вычисляем флаги
        let flags = if total_wagons == 1 {
            // Единственный вагон = FIRST + LAST
            WagonFlags::FIRST | WagonFlags::LAST
        } else if wagon_num == 0 {
            WagonFlags::FIRST
        } else if wagon_num == total_wagons - 1 {
            WagonFlags::LAST
        } else {
            WagonFlags::CONT
        };

        // Вычисляем checksum
        let checksum = Self::compute_checksum(&truncated);

        Self {
            train_id,
            is_clone: false,
            wagon_num,
            total_wagons,
            offset,
            line_id,
            cargo_size: truncated.len() as u32,
            flags,
            checksum,
            cargo: truncated,
        }
    }

    /// Вычислить SHA256 от груза
    pub fn compute_checksum(data: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let mut checksum = [0u8; 32];
        checksum.copy_from_slice(&result);
        checksum
    }

    /// Проверить checksum
    pub fn verify(&self) -> bool {
        self.checksum == Self::compute_checksum(&self.cargo)
    }

    /// Размер вагона в байтах (сериализованный)
    pub fn serialized_size(&self) -> usize {
        // train_id (8) + wagon_num (4) + total_wagons (4) +
        // offset (8) + cargo_size (4) + flags (1) + checksum (32) + cargo.len()
        8 + 4 + 4 + 8 + 4 + 1 + 32 + self.cargo.len()
    }

    /// Создать decoy wagon (пустышка для маскировки)
    pub fn create_decoy(train_id: u64, wagon_num: u32) -> Self {
        let random_cargo = vec![0u8; 40000]; // 40KB случайных данных

        Self {
            train_id,
            is_clone: false,
            wagon_num,
            total_wagons: wagon_num + 1, // Декои всегда "last"
            offset: 0, // Decoy имеет offset 0 (игнорируется при сборке)
            line_id: 0, // Декои используют line 0
            cargo_size: random_cargo.len() as u32,
            flags: WagonFlags::DECOY | WagonFlags::LAST,
            checksum: Self::compute_checksum(&random_cargo),
            cargo: random_cargo,
        }
    }

    /// Упаковать вагон в байты для отправки по UDP
    pub fn to_bytes(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        // Сериализуем через bincode для компактности
        let bytes = bincode::serialize(self)?;
        Ok(bytes)
    }

    /// Распаковать вагон из байтов
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let wagon: Wagon = bincode::deserialize(bytes)?;
        Ok(wagon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wagon_creation() {
        let cargo = vec![1, 2, 3, 4, 5];
        let wagon = Wagon::new(12345, 0, 1, 0, cargo.clone(), 0);

        assert_eq!(wagon.train_id, 12345);
        assert_eq!(wagon.wagon_num, 0);
        assert_eq!(wagon.total_wagons, 1);
        assert_eq!(wagon.offset, 0);
        assert_eq!(wagon.line_id, 0);
        assert_eq!(wagon.cargo, cargo);
        assert!(wagon.flags.contains(WagonFlags::FIRST | WagonFlags::LAST));
    }

    #[test]
    fn test_checksum_verification() {
        let cargo = b"Hello, YANDI!";
        let wagon = Wagon::new(999, 0, 1, 0, cargo.to_vec(), 0);

        assert!(wagon.verify());
    }

    #[test]
    fn test_serialize_deserialize() {
        let cargo = vec![1, 2, 3, 4, 5];
        let wagon1 = Wagon::new(777, 5, 10, 5, cargo, 1);

        let bytes = wagon1.to_bytes().unwrap();
        let wagon2 = Wagon::from_bytes(&bytes).unwrap();

        assert_eq!(wagon1.train_id, wagon2.train_id);
        assert_eq!(wagon1.wagon_num, wagon2.wagon_num);
        assert_eq!(wagon1.offset, wagon2.offset);
        assert_eq!(wagon1.line_id, wagon2.line_id);
        assert_eq!(wagon1.cargo, wagon2.cargo);
    }

    #[test]
    fn test_max_cargo_size() {
        assert!(Wagon::MAX_CARGO_SIZE <= 65535); // Меньше макс UDP
    }

    #[test]
    fn test_random_wagon_sizes() {
        for _ in 0..100 {
            let size = Wagon::random_wagon_size();
            assert!(size >= Wagon::MIN_CARGO_SIZE);
            assert!(size <= Wagon::MAX_CARGO_SIZE);
        }
    }
}
