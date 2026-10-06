// src/protocol/train.rs
//!
//! # Train (Поезд)
//!
//! Логическая передача данных. Один поезд = много вагонов.

use serde::{Serialize, Deserialize};
use std::collections::{HashMap, BTreeMap};  // ⚡ Добавляем BTreeMap для QUIC-style
use std::time::{Duration, Instant};

use super::Wagon;

/// ID поезда
pub type TrainId = u64;

/// Поезд - логическая сущность передачи данных
#[derive(Debug, Clone)]
pub struct Train {
    /// Уникальный ID поезда
    pub id: TrainId,

    /// ID станции-отправителя (You)
    pub source_station: crate::util::HashId,

    /// ID станции-получателя (I)
    pub dest_station: crate::util::HashId,

    /// Общее количество вагонов
    pub total_wagons: u32,

    /// ⚡ QUIC-style: принятые wagons по offset (сортированные!)
    pub wagons_by_offset: BTreeMap<u64, Wagon>,

    /// Для совместимости: wagon_num -> wagon (старый метод)
    pub wagons: HashMap<u32, Wagon>,

    /// 🔄 DUAL-PATH: клоны вагонов (is_clone=true)
    pub wagons_clone: HashMap<u32, Wagon>,

    /// ⚡ Максимальный полученный offset
    pub max_received_offset: u64,

    /// Состояние поезда
    pub state: TrainState,

    /// Время создания поезда
    pub created_at: Instant,

    /// Последняя активность
    pub last_activity: Instant,

    /// Таймаут сборки поезда
    pub timeout: Duration,
}

/// Состояние поезда
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainState {
    /// Поезд формируется
    Assembling,

    /// Все вагоны приняты, поезд собран
    Complete,

    /// Ошибка сборки (потерянные вагоны)
    Failed,

    /// Поезд отправлен
    Sent,
}

impl Train {
    /// Создать новый поезд для отправки
    pub fn new(
        source: crate::util::HashId,
        dest: crate::util::HashId,
        data: Vec<u8>,
    ) -> Self {
        let id = Self::generate_train_id();
        let total_wagons = Self::calculate_wagon_count(&data);

        Self {
            id,
            source_station: source,
            dest_station: dest,
            total_wagons,
            wagons_by_offset: BTreeMap::new(),
            wagons: HashMap::new(),
            wagons_clone: HashMap::new(),  // 🔄 DUAL-PATH: клоны
            max_received_offset: 0,
            state: TrainState::Assembling,
            created_at: Instant::now(),
            last_activity: Instant::now(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Создать пустой поезд для приёма
    pub fn new_receiving(
        train_id: TrainId,
        source: crate::util::HashId,
        total_wagons: u32,
    ) -> Self {
        Self {
            id: train_id,
            source_station: source,
            dest_station: crate::util::HashId::default(),
            total_wagons,
            wagons_by_offset: BTreeMap::new(),
            wagons: HashMap::new(),
            wagons_clone: HashMap::new(),  // 🔄 DUAL-PATH
            max_received_offset: 0,
            state: TrainState::Assembling,
            created_at: Instant::now(),
            last_activity: Instant::now(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Сгенерировать уникальный ID поезда
    pub fn generate_train_id() -> TrainId {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let random: u64 = rand::random();
        timestamp ^ random
    }

    /// Рассчитать количество вагонов для данных
    pub fn calculate_wagon_count(data: &[u8]) -> u32 {
        if data.is_empty() {
            return 1;
        }

        let max_wagon_size = Wagon::MAX_CARGO_SIZE;
        ((data.len() + max_wagon_size - 1) / max_wagon_size) as u32
    }

    /// Разбить данные на вагоны
    pub fn into_wagons(self) -> Vec<Wagon> {
        // Для этого нужны данные -暂时 не реализуем
        // Будет сделано в Station при отправке
        vec![]
    }

    /// 🔄 Добавить вагон к поезду (с поддержкой клонов!)
    /// Возвращает (complete, path0_lost) где:
    ///   - complete: true только при ПЕРЕХОДЕ в состояние Complete
    ///   - path0_lost: true если Path1 прибыл раньше Path0 (потеря Path0)
    pub fn add_wagon(&mut self, wagon: Wagon) -> Result<(bool, bool), TrainError> {
        if wagon.train_id != self.id {
            return Err(TrainError::WrongTrainId);
        }

        if wagon.total_wagons != self.total_wagons {
            return Err(TrainError::WagonCountMismatch);
        }

        // 🔍 DEBUG (читаем поля ДО move!)
        let wagon_num = wagon.wagon_num;
        let is_clone = wagon.is_clone;

        // 🔴 Path0 loss detection
        let path0_lost = if is_clone {
            // Path1 прибыл - проверяем был ли Path0
            !self.wagons.contains_key(&wagon_num)
        } else {
            false
        };

        // 🔄 DUAL-PATH: разделяем оригиналы и клоны
        if is_clone {
            // Клон - сохраняем как запасной
            self.wagons_clone.insert(wagon_num, wagon);
        } else {
            // Оригинал - сохраняем как основной
            self.wagons.insert(wagon_num, wagon);
        }

        self.last_activity = Instant::now();

        // 🔄 Проверяем: собран ли поезд (оригиналы + клоны)
        let mut total_received = 0;
        for i in 0..self.total_wagons {
            if self.wagons.contains_key(&i) || self.wagons_clone.contains_key(&i) {
                total_received += 1;
            }
        }

        // 🔄 Возвращаем (complete, path0_lost)
        let complete = total_received == self.total_wagons && self.state != TrainState::Complete;
        if complete {
            self.state = TrainState::Complete;
        }
        Ok((complete, path0_lost))
    }
    /// 🧹 Очистить клоны после сборки (освободить память!)
    pub fn cleanup_clones(&mut self) {
        if self.state != TrainState::Complete {
            return;
        }

        // Удаляем клоны которые дублируют оригиналы
        let mut clone_count = 0;
        for wagon_num in 0..self.total_wagons {
            if self.wagons.contains_key(&wagon_num) {
                // Есть оригинал - удаляем клон
                if self.wagons_clone.remove(&wagon_num).is_some() {
                    clone_count += 1;
                }
            }
        }

        if clone_count > 0 {
            println!("[TRAIN] 🧹 Cleaned up {} clone wagons (freed memory)", clone_count);
        }
    }
    pub fn assemble(&self) -> Result<Vec<u8>, TrainError> {
        eprintln!("[TRAIN] assemble() called: state={:?}, orig={}/{}, clone={}/{}, total={}",
                 self.state,
                 self.wagons.len(), self.total_wagons,
                 self.wagons_clone.len(), self.total_wagons,
                 self.total_wagons);

        if self.state != TrainState::Complete {
            eprintln!("[TRAIN] ❌ NOT Complete! Current state: {:?}", self.state);
            return Err(TrainError::Incomplete);
        }

        let mut data = Vec::with_capacity(self.total_wagons as usize * Wagon::MAX_CARGO_SIZE);

        // 🔄 Собираем вагоны по порядку (оригиналы + клоны)
        for i in 0..self.total_wagons {
            // Сначала ищем в оригиналах
            let wagon = if let Some(w) = self.wagons.get(&i) {
                eprintln!("[TRAIN] wagon #{} from ORIG", i);
                w
            }
            // Если нет в оригиналах - берём из клонов
            else if let Some(w) = self.wagons_clone.get(&i) {
                eprintln!("[TRAIN] wagon #{} from CLONE (backup!)", i);
                w
            }
            // Нет ни там ни там - ошибка
            else {
                eprintln!("[TRAIN] ❌ Missing wagon #{} (no orig, no clone)", i);
                return Err(TrainError::MissingWagon(i));
            };

            // Проверяем checksum
            if !wagon.verify() {
                eprintln!("[TRAIN] ❌ Checksum failed for wagon #{}", i);
                return Err(TrainError::ChecksumFailed(i));
            }

            data.extend_from_slice(&wagon.cargo);
        }

        eprintln!("[TRAIN] ✅ Assembled {} wagons, {} bytes ({} orig + {} clone used)",
                 self.total_wagons, data.len(),
                 self.wagons.len(), self.wagons_clone.len());
        Ok(data)
    }

    /// 🎉 Вычислить checksum собранного поезда (для ACK)
    pub fn calculate_checksum(&self) -> [u8; 32] {
        use sha2::{Sha256, Digest};

        let mut hasher = Sha256::new();

        // Хешируем wagons по порядку
        for i in 0..self.total_wagons {
            if let Some(wagon) = self.wagons.get(&i) {
                // Добавляем wagon_num и wagon data
                hasher.update(&wagon.wagon_num.to_be_bytes());
                hasher.update(&wagon.cargo);
            }
        }

        hasher.finalize().into()
    }

    /// ⚡ QUIC-style: собрать данные до заданного offset (partial assembly)
    pub fn assemble_up_to(&self, max_offset: u64) -> Result<Vec<u8>, TrainError> {
        // ⚡ Строим BTreeMap только при необходимости (редко!)
        let offset_map: BTreeMap<u64, &Wagon> = self.wagons
            .values()
            .map(|w| (w.offset, w))
            .collect();

        let mut data = Vec::new();
        let mut current_offset = 0u64;

        // Проходим по offset_map (уже отсортированы!)
        for (&offset, wagon) in &offset_map {
            // Пропускаем wagons за пределами max_offset
            if offset >= max_offset {
                break;
            }

            // Проверяем на пропуски
            if offset > current_offset {
                // Обнаружен пропуск: current_offset..offset
                return Err(TrainError::MissingRange(current_offset, offset));
            }

            // Проверяем checksum
            if !wagon.verify() {
                return Err(TrainError::ChecksumFailedAtOffset(offset));
            }

            // Добавляем данные
            data.extend_from_slice(&wagon.cargo);
            current_offset = offset + wagon.cargo.len() as u64;
        }

        Ok(data)
    }

    /// ⚡ QUIC-style: проверить, можно ли собрать до заданного offset
    pub fn can_assemble_up_to(&self, max_offset: u64) -> bool {
        // ⚡ Строим BTreeMap только при необходимости (редко!)
        let offset_map: BTreeMap<u64, &Wagon> = self.wagons
            .values()
            .map(|w| (w.offset, w))
            .collect();

        let mut current_offset = 0u64;

        for (&offset, wagon) in &offset_map {
            if offset >= max_offset {
                break;
            }

            if offset > current_offset {
                return false;  // Пропуск!
            }

            current_offset = offset + wagon.cargo.len() as u64;
        }

        current_offset >= max_offset
    }

    /// ⚡ QUIC-style: получить список пропусков (missing ranges)
    pub fn missing_ranges(&self) -> Vec<(u64, u64)> {
        // ⚡ Строим BTreeMap только при необходимости (редко!)
        let offset_map: BTreeMap<u64, &Wagon> = self.wagons
            .values()
            .map(|w| (w.offset, w))
            .collect();

        let mut missing = Vec::new();
        let mut current_offset = 0u64;

        // Сохраняем ссылку на последний wagon до цикла
        let last_wagon_opt = offset_map.values().last().copied();

        for (&offset, wagon) in &offset_map {
            if offset > current_offset {
                // Пропуск: current_offset..offset
                missing.push((current_offset, offset));
            }

            current_offset = offset + wagon.cargo.len() as u64;
        }

        // Проверяем, есть ли пропуск в конце
        if let Some(last_wagon) = last_wagon_opt {
            let expected_end = last_wagon.offset + last_wagon.cargo.len() as u64;
            if expected_end < self.max_received_offset {
                missing.push((expected_end, self.max_received_offset));
            }
        }

        missing
    }

    /// Проверить, истёк ли таймаут
    pub fn is_timeout(&self) -> bool {
        self.last_activity.elapsed() > self.timeout
    }

    /// Оценить размер поезда в байтах
    pub fn estimated_size(&self) -> usize {
        let orig_size: usize = self.wagons
            .values()
            .map(|w| w.cargo.len() + 128)
            .sum();
        let clone_size: usize = self.wagons_clone
            .values()
            .map(|w| w.cargo.len() + 128)
            .sum();
        orig_size + clone_size
    }

    /// Получить прогресс сборки (0.0 - 1.0)
    pub fn progress(&self) -> f64 {
        if self.total_wagons == 0 {
            return 1.0;
        }

        self.wagons.len() as f64 / self.total_wagons as f64
    }

    /// Получить список недостающих вагонов
    pub fn missing_wagons(&self) -> Vec<u32> {
        let mut missing = Vec::new();

        for i in 0..self.total_wagons {
            if !self.wagons.contains_key(&i) {
                missing.push(i);
            }
        }

        missing
    }
}

/// Ошибки поезда
#[derive(Debug, thiserror::Error)]
pub enum TrainError {
    #[error("ID поезда не совпадает")]
    WrongTrainId,

    #[error("Количество вагонов не совпадает")]
    WagonCountMismatch,

    #[error("Поезд не собран полностью")]
    Incomplete,

    #[error("Отсутствует вагон #{0}")]
    MissingWagon(u32),

    #[error("Checksum failed для вагона #{0}")]
    ChecksumFailed(u32),

    #[error("Таймаут сборки поезда")]
    Timeout,

    /// ⚡ QUIC-style: отсутствует диапазон данных
    #[error("Отсутствует диапазон данных: {0}..{1}")]
    MissingRange(u64, u64),

    /// ⚡ QUIC-style: checksum failed на offset
    #[error("Checksum failed на offset {0}")]
    ChecksumFailedAtOffset(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_train_creation() {
        let data = vec![1, 2, 3, 4, 5];
        let source = crate::util::HashId::default();
        let dest = crate::util::HashId::default();

        let train = Train::new(source, dest, data);

        assert_eq!(train.total_wagons, 1);
        assert_eq!(train.state, TrainState::Assembling);
    }

    #[test]
    fn test_wagon_count() {
        let small_data = vec![0u8; 1000];
        let train = Train::new(
            crate::util::HashId::default(),
            crate::util::HashId::default(),
            small_data,
        );
        assert_eq!(train.total_wagons, 1);

        let large_data = vec![0u8; 150_000]; // 150KB
        let train = Train::new(
            crate::util::HashId::default(),
            crate::util::HashId::default(),
            large_data,
        );
        // 150KB / 60KB (MAX_CARGO_SIZE=61440) = 3 вагона
        assert_eq!(train.total_wagons, 3);
    }

    #[test]
    fn test_add_wagon() {
        let mut train = Train::new_receiving(12345, crate::util::HashId::default(), 2);

        let wagon1 = Wagon::new(12345, 0, 2, 0, vec![1, 2, 3], 0);
        let wagon2 = Wagon::new(12345, 1, 2, 3, vec![4, 5, 6], 0);

        let _ = train.add_wagon(wagon1).unwrap();
        assert_eq!(train.progress(), 0.5);

        let _ = train.add_wagon(wagon2).unwrap();
        assert_eq!(train.state, TrainState::Complete);
        assert_eq!(train.progress(), 1.0);
    }

    #[test]
    fn test_assemble() {
        let mut train = Train::new_receiving(999, crate::util::HashId::default(), 2);

        let wagon1 = Wagon::new(999, 0, 2, 0, vec![1, 2, 3], 0);
        let wagon2 = Wagon::new(999, 1, 2, 3, vec![4, 5, 6], 0);

        let _ = train.add_wagon(wagon1).unwrap();
        let _ = train.add_wagon(wagon2).unwrap();

        let data = train.assemble().unwrap();
        assert_eq!(data, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_missing_wagons() {
        let mut train = Train::new_receiving(777, crate::util::HashId::default(), 5);

        let _ = train.add_wagon(Wagon::new(777, 0, 5, 0, vec![], 0)).unwrap();
        let _ = train.add_wagon(Wagon::new(777, 1, 5, 1, vec![], 0)).unwrap();
        // Пропускаем 2 и 3
        let _ = train.add_wagon(Wagon::new(777, 4, 5, 4, vec![], 0)).unwrap();

        let missing = train.missing_wagons();
        assert_eq!(missing, vec![2, 3]);
    }

    /// ⚡ Тест QUIC-style partial assembly
    #[test]
    fn test_quic_style_partial_assembly() {
        let mut train = Train::new_receiving(888, crate::util::HashId::default(), 5);

        // Добавляем wagons с offsets
        let _ = train.add_wagon(Wagon::new(888, 0, 5, 0, vec![1, 2, 3], 0)).unwrap();
        let _ = train.add_wagon(Wagon::new(888, 1, 5, 3, vec![4, 5, 6], 0)).unwrap();
        let _ = train.add_wagon(Wagon::new(888, 2, 5, 6, vec![7, 8, 9], 0)).unwrap();

        // Можно собрать до offset 9 (все данные)
        assert!(train.can_assemble_up_to(9));
        let data = train.assemble_up_to(9).unwrap();
        assert_eq!(data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    /// ⚡ Тест QUIC-style missing ranges detection
    #[test]
    fn test_quic_style_missing_ranges() {
        let mut train = Train::new_receiving(999, crate::util::HashId::default(), 5);

        // Добавляем wagons: 0..3, 6..9 (пропуск 3..6)
        let _ = train.add_wagon(Wagon::new(999, 0, 5, 0, vec![1, 2, 3], 0)).unwrap();
        let _ = train.add_wagon(Wagon::new(999, 2, 5, 6, vec![7, 8, 9], 0)).unwrap();

        // Проверяем missing ranges
        let missing = train.missing_ranges();
        assert_eq!(missing, vec![(3, 6)]);

        // Нельзя собрать до offset 9
        assert!(!train.can_assemble_up_to(9));

        // Можно собрать до offset 3
        assert!(train.can_assemble_up_to(3));
    }
}
