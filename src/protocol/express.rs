// src/protocol/express.rs
//!
//! # Express Train (Экспресс-поезд)
//!
//! Высокоприоритетные поезда для доставки недостающих вагонов.

use serde::{Serialize, Deserialize};
use std::collections::HashMap;

use super::{TrainId, Wagon, WagonFlags};

/// Приоритет поезда
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrainPriority {
    /// Экспресс-поезд (высокий приоритет)
    #[serde(rename = "express")]
    Express = 0,

    /// Обычный поезд (нормальный приоритет)
    #[serde(rename = "normal")]
    Normal = 1,

    /// Грузовой (низкий приоритет)
    #[serde(rename = "cargo")]
    Cargo = 2,
}

/// Стратегия отправки экспресс-поездов
#[derive(Debug, Clone, Copy)]
pub enum ExpressStrategy {
    /// Ленивая - ждать timeout, потом отправлять все missing сразу
    Lazy,

    /// Немедленная - отправлять missing сразу как обнаружили
    Eager,

    /// Батчинг - отправлять пачками с интервалом
    Batched(std::time::Duration),
}

/// Экспресс-поезд для доставки недостающих вагонов
#[derive(Debug, Clone)]
pub struct ExpressTrain {
    /// ID оригинального поезда
    pub original_train_id: TrainId,

    /// Приоритет (всегда Express)
    pub priority: TrainPriority,

    /// Недостающие вагоны
    pub wagons: Vec<Wagon>,

    /// Номера недостающих вагонов
    pub missing_numbers: Vec<u32>,
}

impl ExpressTrain {
    /// Создать экспресс-поезд из списка недостающих вагонов
    pub fn new(
        original_train_id: TrainId,
        missing_numbers: Vec<u32>,
        original_wagons: &HashMap<u32, Wagon>,
    ) -> Self {
        let wagons: Vec<Wagon> = missing_numbers
            .iter()
            .filter_map(|id| original_wagons.get(id).cloned())
            .map(|mut w| {
                // Меняем флаг на EXPRESS
                w.flags.insert(WagonFlags::EXPRESS);
                w
            })
            .collect();

        Self {
            original_train_id,
            priority: TrainPriority::Express,
            wagons,
            missing_numbers,
        }
    }

    /// Проверить, что это экспресс-поезд
    pub fn is_express(&self) -> bool {
        self.priority == TrainPriority::Express
    }

    /// Количество вагонов в экспресс-поезде
    pub fn len(&self) -> usize {
        self.wagons.len()
    }

    /// Проверить, пустой ли экспресс-поезд
    pub fn is_empty(&self) -> bool {
        self.wagons.is_empty()
    }
}

/// ACK/NACK сообщения
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrainAckMessage {
    /// Полный ACK - все вагоны получены
    #[serde(rename = "complete")]
    Complete {
        train_id: TrainId,
        checksum: [u8; 32],
    },

    /// NACK - есть недостающие вагоны
    #[serde(rename = "missing")]
    Missing {
        train_id: TrainId,
        missing_wagons: Vec<u32>,
        total_wagons: u32,
        received_wagons: u32,
    },

    /// Progress - промежуточный прогресс
    #[serde(rename = "progress")]
    Progress {
        train_id: TrainId,
        received: u32,
        total: u32,
        progress_percent: f32,
    },
}

impl TrainAckMessage {
    /// Создать Complete ACK
    pub fn complete(train_id: TrainId, checksum: [u8; 32]) -> Self {
        Self::Complete {
            train_id,
            checksum,
        }
    }

    /// Создать Missing NACK
    pub fn missing(
        train_id: TrainId,
        missing_wagons: Vec<u32>,
        total_wagons: u32,
        received_wagons: u32,
    ) -> Self {
        Self::Missing {
            train_id,
            missing_wagons,
            total_wagons,
            received_wagons,
        }
    }

    /// Создать Progress сообщение
    pub fn progress(train_id: TrainId, received: u32, total: u32) -> Self {
        let progress_percent = if total > 0 {
            (received as f32 / total as f32) * 100.0
        } else {
            100.0
        };

        Self::Progress {
            train_id,
            received,
            total,
            progress_percent,
        }
    }

    /// Получить ID поезда
    pub fn train_id(&self) -> TrainId {
        match self {
            Self::Complete { train_id, .. } => *train_id,
            Self::Missing { train_id, .. } => *train_id,
            Self::Progress { train_id, .. } => *train_id,
        }
    }

    /// Проверить, является ли сообщение Complete
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }

    /// Проверить, есть ли missing вагоны
    pub fn has_missing(&self) -> bool {
        matches!(self, Self::Missing { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_express_train_creation() {
        let mut wagons = HashMap::new();
        wagons.insert(0, Wagon::new(123, 0, 10, 0, vec![1, 2, 3], 0));
        wagons.insert(5, Wagon::new(123, 5, 10, 5, vec![4, 5, 6], 0));

        let express = ExpressTrain::new(123, vec![0, 5], &wagons);

        assert_eq!(express.original_train_id, 123);
        assert_eq!(express.len(), 2);
        assert!(express.wagons[0].flags.contains(WagonFlags::EXPRESS));
    }

    #[test]
    fn test_ack_messages() {
        let complete = TrainAckMessage::complete(456, [0u8; 32]);
        assert!(complete.is_complete());
        assert_eq!(complete.train_id(), 456);

        let missing = TrainAckMessage::missing(789, vec![1, 2, 3], 10, 7);
        assert!(missing.has_missing());
        assert_eq!(missing.train_id(), 789);

        let progress = TrainAckMessage::progress(999, 5, 10);
        assert_eq!(progress.train_id(), 999);
    }

    #[test]
    fn test_progress_calculation() {
        let progress = TrainAckMessage::progress(111, 5, 10);
        match progress {
            TrainAckMessage::Progress { progress_percent, .. } => {
                assert!((progress_percent - 50.0).abs() < 0.01);
            }
            _ => panic!("Expected Progress"),
        }
    }
}
