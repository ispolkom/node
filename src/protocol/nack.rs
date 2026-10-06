// src/protocol/nack.rs
//!
//! # Wagon NACK (Negative Acknowledgment)
//!
//! Запрос на повторную отправку потерянных wagon-ов.
//! Аналог TCP SACK (Selective Acknowledgment) но для YTP.

use serde::{Serialize, Deserialize};

/// Причина запроса повторной отправки
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NackReason {
    /// Timeout - нет новых wagon-ов 5+ секунд
    Timeout = 1,

    /// Threshold - получено 90% wagon-ов, но не все
    Threshold = 2,

    /// Explicit request от получателя
    Explicit = 3,
}

/// Wagon NACK - запрос на повторную отправку потерянных wagon-ов
///
/// **Протокол:**
/// 1. RF node получает wagon-ы в разброс: [1,2,5,8...]
/// 2. RF node видит что missing: [3,4,6,7,9...]
/// 3. RF node отправляет NACK с missing_wagons
/// 4. NL node переотправляет только missing wagon-ы
/// 5. RF node собирает train
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WagonNack {
    /// ID поезда
    pub train_id: u64,

    /// Номера потерянных wagon-ов (отсортированы по возрастанию)
    pub missing_wagons: Vec<u16>,

    /// Причина NACK
    pub reason: NackReason,

    /// Timestamp когда NACK был создан
    pub timestamp: u64,
}

impl WagonNack {
    /// Создать новый NACK
    pub fn new(train_id: u64, missing_wagons: Vec<u16>, reason: NackReason) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            train_id,
            missing_wagons,
            reason,
            timestamp,
        }
    }

    /// Проверить валидность NACK
    pub fn is_valid(&self) -> bool {
        !self.missing_wagons.is_empty() && self.missing_wagons.len() <= 4096
    }

    /// Получить количество потерянных wagon-ов
    pub fn missing_count(&self) -> usize {
        self.missing_wagons.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nack_creation() {
        let nack = WagonNack::new(
            12345,
            vec![3, 4, 6, 7, 9],
            NackReason::Timeout,
        );

        assert_eq!(nack.train_id, 12345);
        assert_eq!(nack.missing_count(), 5);
        assert!(nack.is_valid());
    }

    #[test]
    fn test_nack_empty() {
        let nack = WagonNack::new(12345, vec![], NackReason::Timeout);
        assert!(!nack.is_valid());
    }

    #[test]
    fn test_nack_too_many() {
        let missing: Vec<u16> = (0..5000).collect();
        let nack = WagonNack::new(12345, missing, NackReason::Timeout);
        assert!(!nack.is_valid()); // > 4096 wagons
    }
}
