// src/protocol/ordering.rs
//!
//! # Train Ordering Queue
//!
//! Очередь с упорядочиванием поездов по sequence_id.
//! Гарантирует, что поезда доставляются клиенту в правильном порядке.
//!
//! ## Bug fix (2026-09): passive gap-timeout deadlock
//!
//! Раньше `drain_ready()` вызывался ТОЛЬКО как побочный эффект `add_train()`
//! — то есть gap-timeout мог "сработать" исключительно в момент прихода
//! СЛЕДУЮЩЕГО поезда. Если после пропавшего поезда больше ничего не
//! приходит (конец передачи, пауза в трафике) — очередь зависает навсегда:
//! `next_sequence` никогда не продвигается, ничего не уходит в `tx`, ничто
//! это не перепроверяет. Живой пример: `test_ordering_with_gap` реально
//! вешался на 5+ минут вместо ожидаемых ~150мс — таймаут никогда сам не
//! проверялся, потому что после `sleep()` никто больше не звал `add_train`.
//!
//! Исправлено: очередь теперь держит состояние в `Arc<Mutex<...>>` и
//! запускает собственную фоновую задачу (`tokio::time::interval`), которая
//! периодически перепроверяет gap-timeout НЕЗАВИСИМО от новых поездов.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tracing::{info, debug, warn};

/// Внутреннее состояние очереди — вынесено отдельно от `TrainOrderingQueue`,
/// чтобы и `add_train()`, и фоновая задача-таймер могли делить его через
/// один `Arc<Mutex<...>>`, а не через `&mut self` (что раньше и было
/// причиной пассивности: без `&mut self` под рукой некому было
/// самостоятельно перепроверить таймаут).
struct QueueState {
    queue: BTreeMap<u64, (Vec<u8>, Instant)>,
    next_sequence: u64,
}

/// Упорядоченная очередь поездов
///
/// **Назначение:**
/// - Принимать поезда в произвольном порядке (out-of-order arrival)
/// - Доставлять поезда в строгом порядке по sequence_id
/// - Обрабатывать gaps (потерянные поезда) с timeout — АКТИВНО, не только
///   при приходе новых поездов (см. bug fix выше).
pub struct TrainOrderingQueue {
    /// ID линии (для логирования)
    line_id: u8,

    /// Общее состояние — доступно и `add_train()`, и фоновому таймеру.
    state: Arc<Mutex<QueueState>>,

    /// Channel для отправки упорядоченных данных
    tx: mpsc::Sender<Vec<u8>>,

    /// Timeout для gap recovery (сколько ждать потерянный поезд)
    timeout: Duration,
}

impl TrainOrderingQueue {
    /// Создать новую очередь упорядочивания.
    ///
    /// Сразу запускает фоновую задачу, которая проверяет gap-timeout
    /// каждые `timeout / 2` (но не реже, чем раз в 10мс — чтобы не
    /// уйти в busy-loop при очень маленьком timeout, например в тестах).
    pub fn new(
        line_id: u8,
        tx: mpsc::Sender<Vec<u8>>,
        timeout: Duration,
    ) -> Self {
        let state = Arc::new(Mutex::new(QueueState {
            queue: BTreeMap::new(),
            next_sequence: 0,
        }));

        let check_interval = (timeout / 2).max(Duration::from_millis(10));
        let bg_state = state.clone();
        let bg_tx = tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(check_interval);
            loop {
                ticker.tick().await;
                let mut st = bg_state.lock().await;
                if Self::drain_ready_locked(&mut st, line_id, &bg_tx, timeout).await {
                    // Channel закрыт (получатель отброшен) — фоновой
                    // задаче больше нечего делать, выходим, а не крутим
                    // interval вхолостую вечно.
                    break;
                }
            }
        });

        Self {
            line_id,
            state,
            tx,
            timeout,
        }
    }

    /// Создать очередь с стандартным timeout 5 секунд
    pub fn with_defaults(line_id: u8, tx: mpsc::Sender<Vec<u8>>) -> Self {
        Self::new(line_id, tx, Duration::from_secs(5))
    }

    /// Добавить поезд в очередь
    pub async fn add_train(&mut self, sequence_id: u64, data: Vec<u8>) {
        let timestamp = Instant::now();

        info!(
            "📦 [Line #{}] Train seq={} added to ordering queue ({} MB)",
            self.line_id,
            sequence_id,
            data.len() / 1_000_000
        );

        let mut st = self.state.lock().await;

        // Проверяем на duplicate
        if st.queue.contains_key(&sequence_id) {
            warn!(
                "⚠️  [Line #{}] Duplicate train seq={}, ignoring",
                self.line_id, sequence_id
            );
            return;
        }

        // Сохраняем в очередь (BTreeMap автоматически сортирует по ключу)
        st.queue.insert(sequence_id, (data, timestamp));

        // Пытаемся доставить все готовые поезда прямо сейчас (не ждём
        // следующего тика фонового таймера — тот нужен только для случая
        // "больше вообще ничего не пришло").
        Self::drain_ready_locked(&mut st, self.line_id, &self.tx, self.timeout).await;
    }

    /// Доставить все поезда в правильном порядке. Возвращает `true`, если
    /// канал получателя закрыт (вызывающему больше не имеет смысла
    /// продолжать пытаться отправлять/перепроверять).
    async fn drain_ready_locked(
        st: &mut QueueState,
        line_id: u8,
        tx: &mpsc::Sender<Vec<u8>>,
        timeout: Duration,
    ) -> bool {
        let now = Instant::now();

        loop {
            let Some((&seq_id, &(_, timestamp))) = st.queue.iter().next().map(|(k, v)| (k, v)) else {
                break;
            };

            if seq_id == st.next_sequence {
                // ✅ Это поезд который мы ждали!
                let (data, _) = st.queue.remove(&seq_id).expect("just peeked this key");

                info!(
                    "✅ [Line #{}] Delivering train seq={} to client ({} MB)",
                    line_id,
                    seq_id,
                    data.len() / 1_000_000
                );

                if let Err(e) = tx.send(data).await {
                    warn!(
                        "❌ [Line #{}] Failed to send train seq={} to client: {}",
                        line_id, seq_id, e
                    );
                    // Channel закрыт — дальше пытаться некому.
                    return true;
                }

                st.next_sequence += 1;

            } else if now.duration_since(timestamp) > timeout {
                // ⏰ Timeout! Сдаёмся ждать потерянный(е) поезд(а) ПЕРЕД
                // seq_id — но seq_id САМ уже у нас есть и его нужно
                // доставить, а не выбросить.
                //
                // BUG FIX (2026-09): раньше здесь стояло
                // `self.queue.remove(&seq_id); self.next_sequence = seq_id + 1;`
                // — это молча УДАЛЯЛО реально полученные данные вместо их
                // доставки: seq_id есть в очереди именно потому, что он
                // РЕАЛЬНО пришёл, timeout истёк для того, чего в очереди
                // вообще нет (пропущенных номеров перед ним). Правильное
                // gap-recovery — подтянуть next_sequence к seq_id (простить
                // пропущенные номера) и дать циклу ниже отдать seq_id через
                // обычную ветку доставки, а не молча его терять.
                warn!(
                    "⏰ [Line #{}] Timeout waiting for seq={}, giving up on it and catching up to seq={} (gap recovery — seq={} itself WILL still be delivered)",
                    line_id, st.next_sequence, seq_id, seq_id
                );

                st.next_sequence = seq_id;
                // Не break и не remove — следующая итерация цикла увидит
                // seq_id == next_sequence и доставит его нормальной веткой.

            } else if seq_id > st.next_sequence {
                // ⏳ Ещё не timeout, ждём — следующая проверка либо от
                // прихода нового поезда (add_train), либо от фонового
                // таймера, который перепроверит это же место без
                // необходимости в новом трафике.
                debug!(
                    "⏳ [Line #{}] Waiting for train seq={} (have seq={}, gap={})",
                    line_id,
                    st.next_sequence,
                    seq_id,
                    seq_id - st.next_sequence
                );
                break;

            } else {
                // seq_id < st.next_sequence (устаревший поезд, пропускаем)
                warn!(
                    "⚠️  [Line #{}] Stale train seq={} (expected seq={}), removing",
                    line_id, seq_id, st.next_sequence
                );
                st.queue.remove(&seq_id);
            }
        }

        false
    }

    /// Получить текущий размер очереди
    pub async fn queue_size(&self) -> usize {
        self.state.lock().await.queue.len()
    }

    /// Получить следующий ожидаемый sequence_id
    pub async fn next_sequence(&self) -> u64 {
        self.state.lock().await.next_sequence
    }

    /// Очистить очередь (например, при переподключении)
    pub async fn clear(&mut self) {
        let mut st = self.state.lock().await;
        let size = st.queue.len();
        st.queue.clear();
        st.next_sequence = 0;

        if size > 0 {
            info!(
                "🧹 [Line #{}] Cleared ordering queue (removed {} trains)",
                self.line_id, size
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_ordering_in_order() {
        let (tx, mut rx) = mpsc::channel(100);
        let mut queue = TrainOrderingQueue::with_defaults(0, tx);

        // Отправляем поезда по порядку
        queue.add_train(0, vec![1, 2, 3]).await;
        queue.add_train(1, vec![4, 5, 6]).await;
        queue.add_train(2, vec![7, 8, 9]).await;

        // Проверяем что получаем в правильном порядке
        assert_eq!(rx.recv().await.unwrap(), vec![1, 2, 3]);
        assert_eq!(rx.recv().await.unwrap(), vec![4, 5, 6]);
        assert_eq!(rx.recv().await.unwrap(), vec![7, 8, 9]);
    }

    #[tokio::test]
    async fn test_ordering_out_of_order() {
        let (tx, mut rx) = mpsc::channel(100);
        let mut queue = TrainOrderingQueue::with_defaults(0, tx);

        // Отправляем в разброс
        queue.add_train(2, vec![7, 8, 9]).await; // Прибывает первым
        queue.add_train(0, vec![1, 2, 3]).await; // Прибывает вторым
        queue.add_train(1, vec![4, 5, 6]).await; // Прибывает третьим

        // Но получаем в правильном порядке!
        assert_eq!(rx.recv().await.unwrap(), vec![1, 2, 3]);
        assert_eq!(rx.recv().await.unwrap(), vec![4, 5, 6]);
        assert_eq!(rx.recv().await.unwrap(), vec![7, 8, 9]);
    }

    #[tokio::test]
    async fn test_ordering_with_gap() {
        let (tx, mut rx) = mpsc::channel(100);
        let mut queue = TrainOrderingQueue::new(0, tx, Duration::from_millis(100));

        // Отправляем seq=0 и seq=2 (пропуская seq=1)
        queue.add_train(0, vec![1, 2, 3]).await;
        queue.add_train(2, vec![7, 8, 9]).await;

        // Получаем seq=0 сразу
        assert_eq!(rx.recv().await.unwrap(), vec![1, 2, 3]);

        // Ждём timeout для seq=1 — НИЧЕГО больше не добавляем в очередь,
        // это и есть ровно тот сценарий (тишина в трафике после пропажи
        // пакета), который раньше вешал очередь навсегда. Теперь фоновый
        // таймер должен сам обнаружить timeout без нового add_train().
        sleep(Duration::from_millis(150)).await;

        // После timeout получаем seq=2
        assert_eq!(rx.recv().await.unwrap(), vec![7, 8, 9]);
    }

    #[tokio::test]
    async fn test_queue_size() {
        let (tx, _) = mpsc::channel(100);
        let mut queue = TrainOrderingQueue::with_defaults(0, tx);

        assert_eq!(queue.queue_size().await, 0);

        // Добавляем поезд с gap
        queue.add_train(5, vec![1, 2, 3]).await;

        // Очередь должна содержать 1 поезд
        assert_eq!(queue.queue_size().await, 1);
        assert_eq!(queue.next_sequence().await, 0);
    }

    #[tokio::test]
    async fn test_background_timer_fires_without_new_arrivals() {
        // Регрессия на сам баг: очередь получает ОДИН поезд с gap'ом и
        // затем НИКОГДА не получает больше ничего — раньше это означало
        // "зависла навсегда", теперь фоновый таймер обязан сам разрулить
        // gap по истечении timeout.
        let (tx, mut rx) = mpsc::channel(100);
        let mut queue = TrainOrderingQueue::new(0, tx, Duration::from_millis(50));

        queue.add_train(3, vec![9, 9, 9]).await; // next_sequence=0, gap до 3

        let delivered = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("background timer must fire on its own, without any further add_train() calls")
            .unwrap();
        assert_eq!(delivered, vec![9, 9, 9]);
        assert_eq!(queue.next_sequence().await, 4);
    }
}
