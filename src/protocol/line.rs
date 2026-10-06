// src/protocol/line.rs
//!
//! # Line (Изолированная линия)
//!
//! Одна линия в мультилинейной архитектуре YTP.
//! Каждая линия имеет свой depot, ordering queue и NACK handler.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, Duration};
use tokio::sync::{mpsc, Mutex};
use tracing::{info, debug, warn, error};

use crate::protocol::{
    Train, TrainId, Wagon, WagonNack, NackReason,
    TrainOrderingQueue
};
use crate::util::HashId;

/// Временное хранилище wagon-ов для линии
struct LineDepot {
    /// train_id → (wagons, total_wagons, received_count)
    trains: HashMap<TrainId, (HashMap<u16, Wagon>, u16, u16)>,
}

impl LineDepot {
    pub fn new() -> Self {
        Self {
            trains: HashMap::new(),
        }
    }

    /// Добавить wagon
    pub fn add_wagon(&mut self, wagon: Wagon) -> Result<Option<Train>, String> {
        let train_id = wagon.train_id;

        let entry = self.trains
            .entry(train_id)
            .or_insert_with(|| {
                (HashMap::new(), wagon.total_wagons as u16, 0)
            });

        let (wagons, total_wagons, received_count) = entry;
        wagons.insert(wagon.wagon_num as u16, wagon);
        *received_count += 1;

        // Проверяем completeness
        if *received_count == *total_wagons {
            let (wagons, _, _) = self.trains.remove(&train_id).unwrap();

            // Создаём Train из wagons
            // TODO: нужно собрать train, но это требует больше кода
            // Пока вернём None
            Ok(None)
        } else {
            Ok(None)
        }
    }

    pub fn train_count(&self) -> usize {
        self.trains.len()
    }
}

/// Изолированная линия для передачи поездов
///
/// **Назначение:**
/// - Хранить свой depot (для неполных поездов)
/// - Упорядочивать поезда по sequence_id
/// - Отправлять NACK для потерянных wagon-ов
/// - Переотправлять wagon-ы при получении NACK
pub struct Line {
    /// ID линии (0-9)
    id: u8,

    /// Депо для поездов этой линии
    depot: Arc<Mutex<LineDepot>>,

    /// Ordering queue (гарантирует порядок доставки)
    ordering: Arc<Mutex<TrainOrderingQueue>>,

    /// NACK sender (отправка NACK запросов)
    nack_tx: mpsc::Sender<(HashId, WagonNack)>,

    /// Retransmit storage (хранит отправленные wagons для повторной отправки)
    sent_wagons: Arc<Mutex<HashMap<TrainId, SentTrain>>>,
}

/// Хранилище отправленных wagon-ов для retransmission
#[derive(Debug)]
pub struct SentTrain {
    /// ID поезда
    train_id: TrainId,

    /// Target node (кому отправляли)
    pub target_node: HashId,

    /// Wagon-ы: wagon_num → wagon data (сериализованный)
    wagons: HashMap<u16, Vec<u8>>,

    /// Время отправки
    sent_time: Instant,
}

impl SentTrain {
    /// Создать новое хранилище для поезда
    pub fn new(train_id: TrainId, target_node: HashId) -> Self {
        Self {
            train_id,
            target_node,
            wagons: HashMap::new(),
            sent_time: Instant::now(),
        }
    }

    /// Добавить wagon
    pub fn add_wagon(&mut self, wagon_num: u16, wagon_data: Vec<u8>) {
        self.wagons.insert(wagon_num, wagon_data);
    }

    /// Получить wagon для переотправки
    pub fn get_wagon(&self, wagon_num: u16) -> Option<&Vec<u8>> {
        self.wagons.get(&wagon_num)
    }

    /// Проверить устарел ли train (TTL 60 секунд)
    pub fn is_expired(&self) -> bool {
        self.sent_time.elapsed() > std::time::Duration::from_secs(60)
    }
}

impl Line {
    /// Создать новую линию
    pub fn new(
        id: u8,
        nack_tx: mpsc::Sender<(HashId, WagonNack)>,
        response_tx: mpsc::Sender<Vec<u8>>,
    ) -> Self {
        let ordering = TrainOrderingQueue::with_defaults(id, response_tx);

        info!("🚂 [Line #{}] Created new line", id);

        Self {
            id,
            depot: Arc::new(Mutex::new(LineDepot::new())),
            ordering: Arc::new(Mutex::new(ordering)),
            nack_tx,
            sent_wagons: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Получить ID линии
    pub fn id(&self) -> u8 {
        self.id
    }

    /// Обработать входящий wagon
    pub async fn receive_wagon(&self, wagon: Wagon, _source: HashId) -> Result<(), Box<dyn std::error::Error>> {
        // Проверяем checksum
        if !wagon.verify() {
            warn!(
                "⚠️  [Line #{}] Invalid checksum for wagon #{}/{} of train #{}",
                self.id, wagon.wagon_num, wagon.total_wagons, wagon.train_id
            );
            return Ok(());
        }

        debug!(
            "📦 [Line #{}] Wagon #{}/{} of train #{} ({} KB)",
            self.id,
            wagon.wagon_num + 1,
            wagon.total_wagons,
            wagon.train_id,
            wagon.cargo.len() / 1024
        );

        // Добавляем wagon в depot (пока упрощённая версия - только храним)
        let mut depot = self.depot.lock().await;
        let _train_complete = depot.add_wagon(wagon)?;

        // TODO: когда train complete - собирать и отправлять в ordering queue

        Ok(())
    }

    /// Отправить NACK (для этой линии)
    pub async fn send_nack(
        &self,
        target_node: HashId,
        train_id: TrainId,
        missing: Vec<u16>,
        reason: NackReason,
    ) {
        let nack = WagonNack::new(train_id, missing, reason);

        debug!(
            "🔄 [Line #{}] Sending NACK for train #{}: {} missing wagons",
            self.id,
            train_id,
            nack.missing_count()
        );

        if let Err(e) = self.nack_tx.send((target_node, nack)).await {
            error!(
                "❌ [Line #{}] Failed to send NACK for train #{}: {}",
                self.id, train_id, e
            );
        }
    }

    /// Сохранить wagon для возможной retransmission
    pub async fn store_sent_wagon(&self, train_id: TrainId, wagon_num: u16, wagon_data: Vec<u8>, target_node: HashId) {
        let mut wagons = self.sent_wagons.lock().await;

        let sent_train = wagons
            .entry(train_id)
            .or_insert_with(|| SentTrain::new(train_id, target_node));

        sent_train.add_wagon(wagon_num, wagon_data);
    }

    /// Переотправить wagon (для этой линии)
    pub async fn retransmit_wagon(
        &self,
        train_id: TrainId,
        wagon_num: u16,
        transport: &Arc<crate::netlayer::P2PTransport>,
        target_node: HashId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Извлекаем из storage
        let wagons = self.sent_wagons.lock().await;

        if let Some(sent_train) = wagons.get(&train_id) {
            if let Some(wagon_bytes) = sent_train.get_wagon(wagon_num) {
                debug!(
                    "🔄 [Line #{}] Retransmitting wagon #{} of train #{} ({} bytes)",
                    self.id,
                    wagon_num,
                    train_id,
                    wagon_bytes.len()
                );

                // Переотправляем wagon
                let mut packet = vec![0x60u8]; // YTP Wagon prefix
                packet.extend_from_slice(wagon_bytes);

                transport
                    .send_encrypted(target_node, &packet)
                    .await
                    .map_err(|e| format!("Failed to retransmit wagon: {}", e))?;

                return Ok(());
            }
        }

        warn!(
            "⚠️  [Line #{}] Wagon #{} of train #{} not found in storage",
            self.id, wagon_num, train_id
        );

        Err(format!("Wagon not found in storage").into())
    }

    /// Получить статистику линии
    pub async fn get_stats(&self) -> LineStats {
        let depot = self.depot.lock().await;
        let ordering = self.ordering.lock().await;
        let sent_wagons = self.sent_wagons.lock().await;

        LineStats {
            line_id: self.id,
            pending_trains: depot.train_count(),
            queue_size: ordering.queue_size().await,
            next_sequence: ordering.next_sequence().await,
            stored_trains: sent_wagons.len(),
        }
    }

    /// Cleanup expired sent trains (каждые 30 секунд)
    pub async fn cleanup_expired_trains(&self) {
        let mut wagons = self.sent_wagons.lock().await;
        let before_count = wagons.len();

        // Удаляем устаревшие trains
        wagons.retain(|train_id, sent_train| {
            if sent_train.is_expired() {
                debug!(
                    "🧹 [Line #{}] Cleaning up expired train #{}",
                    self.id, train_id
                );
                false
            } else {
                true
            }
        });

        let after_count = wagons.len();
        if before_count > after_count {
            info!(
                "🧹 [Line #{}] Cleaned up {} expired trains ({} → {})",
                self.id,
                before_count - after_count,
                before_count,
                after_count
            );
        }
    }
}

/// Статистика линии
#[derive(Debug, Clone)]
pub struct LineStats {
    pub line_id: u8,
    pub pending_trains: usize,
    pub queue_size: usize,
    pub next_sequence: u64,
    pub stored_trains: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlayer::P2PTransport;

    #[tokio::test]
    async fn test_line_creation() {
        let (nack_tx, _) = mpsc::channel(100);
        let (resp_tx, _) = mpsc::channel(100);

        let line = Line::new(0, nack_tx, resp_tx);

        assert_eq!(line.id(), 0);
    }

    #[tokio::test]
    async fn test_sent_train_expiry() {
        let mut train = SentTrain::new(123, HashId::default());

        // Только что созданный - не expired
        assert!(!train.is_expired());

        // Если бы прошло 61 секунду - был бы expired
        // (но мы не можем подделать Instant в тесте)
    }
}
