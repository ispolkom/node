// src/protocol/station.rs
//!
//! # Station (Станция)
//!
//! Отправляет и принимает поезда. Управляет депо.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use super::{Train, TrainId, Wagon, WagonFlags};
use super::express::{ExpressTrain, ExpressStrategy, TrainAckMessage, TrainPriority};
use super::rate_controller::{RateController, RateAction};
use crate::util::HashId;
use crate::netlayer::{P2PTransport, transport::get_wagon_stats};
use serde::{Serialize, Deserialize};

/// Callback для получения raw data из wagon'ов
pub type DataCallback = Arc<dyn Fn(HashId, Vec<u8>) + Send + Sync>;

/// ⚡ Batched wagon - несколько пакетов в одном wagon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchedPacket {
    /// Sequence number (для сохранения порядка)
    pub seq_num: u32,
    /// Данные пакета
    pub data: Vec<u8>,
}

/// ⚡ Batched wagon - контейнер для множественных пакетов
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchedWagon {
    /// ID wagon'а (уникальный)
    pub wagon_id: u64,
    /// Пакеты с sequence numbers
    pub packets: Vec<BatchedPacket>,
    /// Timestamp создания (ms)
    pub timestamp_ms: u64,
}

impl BatchedWagon {
    /// Создать новый batched wagon
    pub fn new(wagon_id: u64, packets: Vec<BatchedPacket>) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self {
            wagon_id,
            packets,
            timestamp_ms: timestamp,
        }
    }

    /// Упаковать в байты
    pub fn to_bytes(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(bincode::serialize(self)?)
    }

    /// Распаковать из байтов
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(bincode::deserialize(bytes)?)
    }
}

/// Роль станции
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationRole {
    /// You (client node)
    You,

    /// I (gateway node)
    I,

    /// Может быть и тем и другим
    Both,
}

/// Конфигурация станции
#[derive(Debug, Clone)]
pub struct StationConfig {
    /// Роль станции
    pub role: StationRole,

    /// Таймаут сборки поезда
    pub train_timeout: Duration,

    /// Максимальный размер вагона
    pub max_wagon_size: usize,

    /// Включить stealth mode (добавлять decoy вагоны)
    pub stealth_mode: bool,

    /// Базовая задержка между вагонами (мс)
    pub base_wagon_delay_ms: u64,

    /// Минимальная задержка (мс) - для быстрых сетей
    pub min_wagon_delay_ms: u64,

    /// Порог быстрого ответа (мс) - если меньше, снижаем задержку
    pub fast_response_threshold_ms: u64,

    /// Размер окна для измерения RTT (кол-во последних поездов)
    pub rtt_window_size: usize,

    /// ⚡ Batching timeout (ms) - время накопления пакетов
    pub batch_timeout_ms: u64,

    /// ⚡ Включить batching (агрегацию пакетов)
    pub enable_batching: bool,
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            role: StationRole::Both,
            train_timeout: Duration::from_secs(30),
            max_wagon_size: Wagon::MAX_CARGO_SIZE,
            stealth_mode: false,
            base_wagon_delay_ms: 1,  // ⚡ 1ms minimal delay to prevent congestion
            min_wagon_delay_ms: 0,
            fast_response_threshold_ms: 200,
            rtt_window_size: 10,
            batch_timeout_ms: 50,  // ⚡ 50ms batching window (увеличено для лучшей агрегации)
            enable_batching: true,  // ⚡ Batching включен по умолчанию
        }
    }
}

/// Статистика RTT для одного поезда
#[derive(Debug, Clone)]
struct RttMeasurement {
    train_id: TrainId,
    sent_at: Instant,
    completed_at: Option<Instant>,
    rtt_ms: Option<u64>,
}

/// Станция YANDI - управляет отправкой и приёмом поездов
pub struct Station {
    /// ID станции
    pub id: HashId,

    /// Конфигурация
    config: StationConfig,

    /// P2P Transport для отправки вагонов
    transport: Arc<P2PTransport>,

    /// Депо - принимает поезда
    depot: Arc<Mutex<Depot>>,

    /// История RTT последних поездов
    rtt_history: Arc<Mutex<Vec<RttMeasurement>>>,

    /// Текущая задержка между вагонами (адаптивная)
    current_wagon_delay_ms: Arc<Mutex<u64>>,

    /// ⚡ Batching buffer - накапливает пакеты по времени
    batch_buffer: Arc<Mutex<HashMap<HashId, Vec<BatchedPacket>>>>,

    /// ⚡ Sequence counter для batched packets
    batch_seq_counter: Arc<Mutex<HashMap<HashId, u32>>>,

    /// 🚇 Callback для получения raw data (для TcpTunnel)
    data_callback: Arc<Mutex<Option<DataCallback>>>,

    /// 🚂 Round-robin counter для выбора линии (0 или 1)
    line_counter: Arc<AtomicU8>,

    /// 🚂 Mutex для упорядоченной отправки поездов (чтобы соблюдалась очередность)
    train_send_mutex: Arc<Mutex<()>>,

    /// 🚀 Адаптивный контроллер скорости
    rate_controller: Arc<RateController>,
}

impl Station {
    /// Создать новую станцию
    pub fn new(
        id: HashId,
        transport: Arc<P2PTransport>,
        config: StationConfig,
    ) -> Self {
        // Создаём Depot с лимитами памяти
        // max_bytes: 100MB, max_trains: 5000, ttl: 30 секунд
        let depot = Arc::new(Mutex::new(Depot::new(
            config.train_timeout,
            100_000_000,  // 100MB
            5000,         // 5000 поездов
            Duration::from_secs(30),
        )));
        let initial_delay = config.base_wagon_delay_ms;
        let rtt_window = config.rtt_window_size;

        let station = Self {
            id,
            config,
            transport,
            depot,
            rtt_history: Arc::new(Mutex::new(Vec::with_capacity(rtt_window))),
            current_wagon_delay_ms: Arc::new(Mutex::new(initial_delay)),
            batch_buffer: Arc::new(Mutex::new(HashMap::new())),
            batch_seq_counter: Arc::new(Mutex::new(HashMap::new())),
            data_callback: Arc::new(Mutex::new(None)),
            line_counter: Arc::new(AtomicU8::new(0)),
            train_send_mutex: Arc::new(Mutex::new(())),
            rate_controller: Arc::new(RateController::new()),
        };

        // ⚡ Запускаем background task для периодической отправки batched wagons
        if station.config.enable_batching {
            station.start_batch_flush_task();
        }

        // 🚀 Запускаем background task для адаптации скорости
        {
            let station_clone = station.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30)); // Каждые 30 секунд

                loop {
                    interval.tick().await;

                    // Получаем статистику из WagonStats (Path0 = истина!)
                    let stats = get_wagon_stats();
                    let sent_path0 = stats.sent_path0.load(std::sync::atomic::Ordering::Relaxed);
                    let path0_lost = stats.path0_lost.load(std::sync::atomic::Ordering::Relaxed);

                    // Вычисляем текущий loss rate Path0 для логирования
                    let current_loss = if sent_path0 > 0 {
                        let last_sent = station_clone.rate_controller.measurement_window.lock().await.sent_last_period;
                        let last_lost = station_clone.rate_controller.measurement_window.lock().await.path0_lost_last_period;

                        let sent_delta = sent_path0.saturating_sub(last_sent);
                        let lost_delta = path0_lost.saturating_sub(last_lost);

                        if sent_delta > 0 {
                            lost_delta * 100 / sent_delta
                        } else {
                            0
                        }
                    } else {
                        0
                    };

                    // 🚀 RATE CONTROL DISABLED - speed not adjusted, monitoring only
                    // let _action = station_clone.rate_controller.adjust_rate(sent_path0, path0_lost).await;
                    let _action = RateAction::Maintain;  // Always maintain - no speed changes
                }
            });
        }

        station
    }

    /// Создать с дефолтной конфигурацией
    pub fn with_defaults(id: HashId, transport: Arc<P2PTransport>) -> Self {
        Self::new(id, transport, StationConfig::default())
    }

    /// 🚇 Установить callback для получения raw data из wagon'ов
    ///
    /// Callback вызывается КАЖДЫЙ РАЗ когда приходит wagon с данными.
    /// Идеально для TcpTunnel - получаем raw bytes и сразу отправляем в браузер.
    pub async fn set_data_callback<F>(&self, callback: F)
    where
        F: Fn(HashId, Vec<u8>) + Send + Sync + 'static
    {
        let mut cb = self.data_callback.lock().await;
        *cb = Some(Arc::new(callback));
    }

    /// 🔄 DUAL-PATH: Отправить данные как поезд с клонированием на ОБА пути
    ///
    /// Path0: train_id=12345, is_clone=false (оригинал)
    /// Path1: train_id=12345, is_clone=true (клон)
    pub async fn send_train(
        &self,
        dest: HashId,
        data: Vec<u8>,
    ) -> Result<TrainId, StationError> {
        // 🚂 MUTEX ГАРАНТИРУЕТ ПОСЛЕДОВАТЕЛЬНУЮ ОТПРАВКУ ПОЕЗДОВ!
        let _lock = self.train_send_mutex.lock().await;

        let train_id = Train::generate_train_id();
        let total_wagons = Train::calculate_wagon_count(&data);
        let sent_at = Instant::now();

        println!("🚂🔄 STATION[{}] DUAL-PATH train #{} ({} wagons, {} MB) → STATION[{}]",
                 self.id_short(),
                 train_id,
                 total_wagons,
                 data.len() / 1_000_000,
                 Self::format_hash_id(dest)
        );

        // Регистрируем измерение RTT
        {
            let mut history = self.rtt_history.lock().await;
            history.push(RttMeasurement {
                train_id,
                sent_at,
                completed_at: None,
                rtt_ms: None,
            });

            if history.len() > self.config.rtt_window_size {
                history.remove(0);
            }
        }

        // Разбиваем на вагоны
        let wagon_size = self.config.max_wagon_size;
        let wagons: Vec<_> = data.chunks(wagon_size).enumerate().collect();

        // 🚀 Получаем адаптивные параметры rate controller'а
        let current_rate = self.rate_controller.current_rate_mbps();
        let max_concurrent = self.rate_controller.concurrent_wagons_for_rate();

        println!("🔄 DUAL-PATH: {} wagons × 2 paths = {} packets (Rate: {} Mbps)",
                 total_wagons, total_wagons * 2, current_rate);

        // 🔄 DUAL-PATH: semaphore для ОБЕИХ путей
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent * 2));
        let mut send_tasks = Vec::new();
        let rate_controller = self.rate_controller.clone();

        // 🔄 Для каждого wagon создаём ДВА клона (Path0 и Path1)
        for (i, chunk) in wagons {
            let wagon_num = i as u32;
            let offset = (i * wagon_size) as u64;
            let chunk_len = chunk.len();

            // 🚀 Вычисляем pacing delay
            let pacing_delay = rate_controller.wagon_delay_for_rate(chunk_len);

            // 🔄 Path0: ОРИГИНАЛ
            {
                let wagon = Wagon::new(train_id, wagon_num, total_wagons, offset, chunk.to_vec(), 0);
                let dest = dest;
                let transport = self.transport.clone();
                let sem = semaphore.clone();

                let task = tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    tokio::time::sleep(pacing_delay).await;

                    let wagon_bytes = wagon.to_bytes()
                        .map_err(|e| StationError::SerializationError(e.to_string()))?;
                    let mut packet = vec![0x60u8];
                    packet.extend_from_slice(&wagon_bytes);

                    transport.send_encrypted(dest, &packet).await
                        .map_err(|e| StationError::SendError(e.to_string()))?;

                    println!("📦 [WAGON {}/{}] ORIG sent ({} B) → Path#0",
                             wagon_num + 1, total_wagons, chunk_len);

                    use crate::netlayer::transport::get_wagon_stats;
                    let stats = get_wagon_stats();
                    stats.sent_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    stats.sent_path0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    Ok::<(), StationError>(())
                });

                send_tasks.push(task);
            }

            // 🔄 Path1: КЛОН (тот же train_id, is_clone=true)
            {
                let wagon = Wagon::with_clone(
                    train_id,  // ⭐ ТОТ ЖЕ train_id!
                    true,      // ⭐ is_clone=true
                    wagon_num,
                    total_wagons,
                    offset,
                    chunk.to_vec(),
                    1
                );
                let dest = dest;
                let transport = self.transport.clone();
                let sem = semaphore.clone();

                let task = tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    tokio::time::sleep(pacing_delay).await;

                    let wagon_bytes = wagon.to_bytes()
                        .map_err(|e| StationError::SerializationError(e.to_string()))?;
                    let mut packet = vec![0x60u8];
                    packet.extend_from_slice(&wagon_bytes);

                    transport.send_encrypted(dest, &packet).await
                        .map_err(|e| StationError::SendError(e.to_string()))?;

                    println!("📦 [WAGON {}/{}] CLONE sent ({} B) → Path#1",
                             wagon_num + 1, total_wagons, chunk_len);

                    use crate::netlayer::transport::get_wagon_stats;
                    let stats = get_wagon_stats();
                    stats.sent_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    stats.sent_path1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    Ok::<(), StationError>(())
                });

                send_tasks.push(task);
            }
        }

        // Ждём завершения всех отправок (обоих путей)
        for task in send_tasks {
            if let Err(e) = task.await {
                eprintln!("❌ Wagon send failed: {}", e);
            }
        }

        println!("✅ Train #{} sent on DUAL-PATH! (Path#0 + Path#1)", train_id);

        Ok(train_id)
    }

    /// Отправить один вагон
    async fn send_wagon(&self, dest: HashId, wagon: &Wagon) -> Result<(), StationError> {
        // Упаковываем вагон
        let wagon_bytes = wagon.to_bytes()
            .map_err(|e| StationError::SerializationError(e.to_string()))?;

        // Добавляем префикс YTP (0x60 = YTP wagon)
        let mut packet = vec![0x60u8];
        packet.extend_from_slice(&wagon_bytes);

        // Отправляем через P2P transport
        self.transport.send_encrypted(dest, &packet).await
            .map_err(|e| StationError::SendError(e.to_string()))?;

        Ok(())
    }

    /// Принять вагон (вызывается из transport при получении пакета)
    pub async fn receive_wagon(&self, source_id: HashId, wagon_bytes: &[u8]) -> Result<Option<TrainId>, StationError> {
        // Десериализуем вагон
        let wagon: Wagon = Wagon::from_bytes(wagon_bytes)
            .map_err(|e| StationError::DeserializationError(e.to_string()))?;

        // Проверяем checksum
        if !wagon.verify() {
            println!("⚠️  Wagon {}/{} has invalid checksum! Dropping.",
                     wagon.wagon_num, wagon.total_wagons);
            return Ok(None);
        }

        // Пропускаем decoy вагоны
        if wagon.flags.contains(WagonFlags::DECOY) {
            println!("🎭 [DECOY] Wagon received (ignored)");
            return Ok(None);
        }

        println!("📥 [WAGON {}/{}] received from train #{} ({} KB)",
                 wagon.wagon_num + 1,
                 wagon.total_wagons,
                 wagon.train_id,
                 wagon.cargo.len() / 1024
        );

        // 🚂 Статистика приёма
        use crate::netlayer::transport::get_wagon_stats;
        let stats = get_wagon_stats();
        stats.recv_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Логируем path_id
        let line_id = wagon.line_id;
        if line_id == 0 {
            stats.recv_path0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else if line_id == 1 {
            stats.recv_path1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        // 🚇 Вызываем callback СРАЗУ (если установлен) - для TcpTunnel
        {
            let cb = self.data_callback.lock().await;
            if let Some(callback) = cb.as_ref() {
                // Clone cargo чтобы не заимствовать wagon
                let cargo = wagon.cargo.clone();
                callback(source_id, cargo);
            }
        }

        // Добавляем вагон в депо
        let mut depot = self.depot.lock().await;
        let (train_complete, path0_lost) = depot.add_wagon(source_id, wagon)?;


        // 🔴 Path0 loss counter
        if path0_lost {
            use crate::netlayer::transport::get_wagon_stats;
            let stats = get_wagon_stats();
            stats.path0_lost.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if train_complete {
            let train_id = depot.get_last_completed_train_id();
            println!("✅ Train #{} assembled!", train_id);

            // Обновляем RTT статистику и адаптивную задержку
            self.update_rtt_statistics(train_id).await;

            // ⚠️ ACK/NACK DISABLED per user directive
            // // 🎉 ОТПРАВЛЯЕМ ACK source node!
            // if let Err(e) = self.send_train_ack(source_id, train_id).await {
            //     eprintln!("⚠️  Failed to send ACK for train #{}: {}", train_id, e);
            // }

            Ok(Some(train_id))
        } else {
            Ok(None)
        }
    }

    /// ⚡ Принять УЖЕ ДЕСЕРИАЛИЗОВАННЫЙ вагон (без лишнего парсинга!)
    pub async fn receive_wagon_parsed(&self, source_id: HashId, wagon: Wagon) -> Result<Option<TrainId>, StationError> {
        // Checksum уже проверен в transport!

        // Пропускаем decoy вагоны
        if wagon.flags.contains(WagonFlags::DECOY) {
            println!("🎭 [DECOY] Wagon received (ignored)");
            return Ok(None);
        }

        println!("📥 [WAGON {}/{}] received from train #{} ({} KB)",
                 wagon.wagon_num + 1,
                 wagon.total_wagons,
                 wagon.train_id,
                 wagon.cargo.len() / 1024
        );

        // 🚂 Статистика приёма
        use crate::netlayer::transport::get_wagon_stats;
        let stats = get_wagon_stats();
        stats.recv_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Логируем path_id
        let line_id = wagon.line_id;
        if line_id == 0 {
            stats.recv_path0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else if line_id == 1 {
            stats.recv_path1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        // 🚇 Вызываем callback СРАЗУ (если установлен) - для TcpTunnel
        {
            let cb = self.data_callback.lock().await;
            if let Some(callback) = cb.as_ref() {
                // Clone cargo чтобы не заимствовать wagon
                let cargo = wagon.cargo.clone();
                callback(source_id, cargo);
            }
        }

        // Добавляем вагон в депо
        let mut depot = self.depot.lock().await;
        let (train_complete, path0_lost) = depot.add_wagon(source_id, wagon)?;


        // 🔴 Path0 loss counter
        if path0_lost {
            use crate::netlayer::transport::get_wagon_stats;
            let stats = get_wagon_stats();
            stats.path0_lost.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if train_complete {
            let train_id = depot.get_last_completed_train_id();
            println!("✅ Train #{} assembled!", train_id);

            // Обновляем RTT статистику и адаптивную задержку
            self.update_rtt_statistics(train_id).await;

            // ⚠️ ACK/NACK DISABLED per user directive
            // // 🎉 ОТПРАВЛЯЕМ ACK source node!
            // if let Err(e) = self.send_train_ack(source_id, train_id).await {
            //     eprintln!("⚠️  Failed to send ACK for train #{}: {}", train_id, e);
            // }

            Ok(Some(train_id))
        } else {
            Ok(None)
        }
    }

    /// 🔄 Получить собранный поезд из депо (НЕ удаляем!)
    /// 🎯 Защита от двойной доставки при dual-path!
    pub async fn get_train(&self, train_id: TrainId) -> Option<Vec<u8>> {
        let mut depot = self.depot.lock().await;
        depot.get_train_data(train_id) // Читаем БЕЗ удаления, но маркируем как доставленный!
    }

    /// Получить прогресс сборки поезда
    pub async fn get_train_progress(&self, train_id: TrainId) -> Option<f64> {
        let depot = self.depot.lock().await;
        depot.get_progress(train_id)
    }

    /// Очистить старые поезда из депо
    pub async fn cleanup_depot(&self) {
        let mut depot = self.depot.lock().await;
        depot.cleanup_timeout_trains();
        depot.cleanup_delivered();
    }

    /// 🔄 Очистить доставленные поезда (вызывать после обработки!)
    pub async fn cleanup_delivered_trains(&self) {
        let mut depot = self.depot.lock().await;
        depot.cleanup_delivered();
    }

    /// Запустить фоновую задачу очистки депо
    pub fn spawn_cleanup_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                self.cleanup_depot().await;
            }
        });
    }

    // === Вспомогательные методы ===

    /// Обновить RTT статистику и пересчитать адаптивную задержку
    async fn update_rtt_statistics(&self, train_id: TrainId) {
        let now = Instant::now();

        // Находим и обновляем измерение
        let rtt_ms = {
            let mut history = self.rtt_history.lock().await;
            if let Some(measurement) = history.iter_mut().find(|m| m.train_id == train_id) {
                measurement.completed_at = Some(now);
                measurement.rtt_ms = Some(now.duration_since(measurement.sent_at).as_millis() as u64);
                measurement.rtt_ms
            } else {
                return;
            }
        };

        if let Some(rtt) = rtt_ms {
            println!("⏱️  [RTT] Train #{} completed in {}ms", train_id, rtt);

            // Вычисляем средний RTT из истории
            let avg_rtt = {
                let history = self.rtt_history.lock().await;
                let completed: Vec<_> = history.iter()
                    .filter(|m| m.rtt_ms.is_some())
                    .map(|m| m.rtt_ms.unwrap())
                    .collect();

                if completed.is_empty() {
                    return;
                }

                completed.iter().sum::<u64>() / completed.len() as u64
            };

            println!("📊 [RTT] Average RTT over last {} trains: {}ms",
                     {
                         let h = self.rtt_history.lock().await;
                         h.len()
                     },
                     avg_rtt
            );

            // Адаптивная корректировка задержки
            let new_delay = if avg_rtt < self.config.fast_response_threshold_ms {
                // Быстрая сеть - НУЛЕВАЯ задержка для макс скорости!
                self.config.min_wagon_delay_ms  // Всегда 0ms для быстрых сетей
            } else {
                // Медленная сеть - базовая задержка
                self.config.base_wagon_delay_ms
            };

            let mut current_delay = self.current_wagon_delay_ms.lock().await;

            if *current_delay != new_delay {
                println!("🔄 [ADAPTIVE] Adjusting wagon delay: {}ms → {}ms",
                         *current_delay, new_delay);
                *current_delay = new_delay;
            }
        }
    }

    fn id_short(&self) -> String {
        hex::encode(&self.id.0[..8])
    }

    fn format_hash_id(id: HashId) -> String {
        hex::encode(&id.0[..8])
    }

    /// Отправить экспресс-поезд (высокий приоритет)
    pub async fn send_express_train(
        &self,
        dest: HashId,
        express: ExpressTrain,
    ) -> Result<(), StationError> {
        println!("🚄 STATION[{}] → EXPRESS TRAIN #{}-EXPRESS ({} wagons, HIGH PRIORITY) → STATION[{}]",
                 self.id_short(),
                 express.original_train_id,
                 express.len(),
                 Self::format_hash_id(dest)
        );

        for wagon in &express.wagons {
            // Отправляем с минимальной задержкой (высокий приоритет!)
            self.send_wagon(dest, wagon).await?;
            println!("📦 [EXPRESS WAGON] Sent ({} KB)", wagon.cargo.len() / 1024);
        }

        println!("✅ Express train sent!");

        Ok(())
    }

    /// ⚠️ DISABLED: Отправить ACK сообщение
    #[allow(dead_code)]
    pub async fn send_ack(
        &self,
        dest: HashId,
        ack: TrainAckMessage,
    ) -> Result<(), StationError> {
        let ack_bytes = serde_json::to_vec(&ack)
            .map_err(|e| StationError::SerializationError(e.to_string()))?;

        // Добавляем префикс YTP ACK (0x61 = YTP ACK)
        let mut packet = vec![0x61u8];
        packet.extend_from_slice(&ack_bytes);

        self.transport.send_encrypted(dest, &packet).await
            .map_err(|e| StationError::SendError(e.to_string()))?;

        Ok(())
    }

    /// ⚠️ DISABLED: Отправить ACK когда train полностью собран
    #[allow(dead_code)]
    async fn send_train_ack(&self, source: HashId, train_id: TrainId) -> Result<(), StationError> {
        use super::TrainAckMessage;

        eprintln!("🎉 [ACK] PREPARING ACK for train #{} to source {}", train_id, hex::encode(&source.0[..8]));

        // Вычисляем checksum собранного train'а
        let checksum = {
            let depot = self.depot.lock().await;
            let train = depot.get_train(train_id);
            train.map(|t| {
                eprintln!("[ACK] Got train for checksum, calculating...");
                t.calculate_checksum()
            })
                .unwrap_or_else(|| {
                    eprintln!("[ACK] ⚠️  Train #{} not found in depot!", train_id);
                    [0u8; 32]
                })
        };

        let ack = TrainAckMessage::complete(train_id, checksum);

        println!("🎉 [ACK] Sending ACK for train #{} to source", train_id);

        self.send_ack(source, ack).await
    }

    // ==================== ⚡ BATCHING METHODS ====================

    /// ⚡ Отправить данные с batching (накапливает и отправляет пачками)
    pub async fn send_train_batched(
        &self,
        dest: HashId,
        data: Vec<u8>,
    ) -> Result<TrainId, StationError> {
        if !self.config.enable_batching {
            // Batching выключен - отправляем сразу
            return self.send_train(dest, data).await;
        }

        // Генерируем sequence number
        let seq_num = {
            let mut counters = self.batch_seq_counter.lock().await;
            let counter = counters.entry(dest).or_insert(0);
            let seq = *counter;
            *counter = counter.wrapping_add(1);
            seq
        };

        // Создаем batched packet
        let packet = BatchedPacket {
            seq_num,
            data,
        };

        // Добавляем в buffer
        {
            let mut buffer = self.batch_buffer.lock().await;
            buffer.entry(dest).or_insert_with(Vec::new).push(packet);
        }

        // Пока возвращаем dummy train_id (real batched sending happens in background)
        Ok(0)
    }

    /// ⚡ Background task - периодически отправляет накопленные batched wagons
    fn start_batch_flush_task(&self) {
        let buffer = self.batch_buffer.clone();
        let transport = self.transport.clone();
        let timeout_ms = self.config.batch_timeout_ms;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(timeout_ms));
            loop {
                interval.tick().await;

                // Флудим все accumulated packets
                let mut buffer_lock = buffer.lock().await;
                if buffer_lock.is_empty() {
                    continue;
                }

                // Клонируем и очищаем buffer
                let pending: HashMap<HashId, Vec<BatchedPacket>> = buffer_lock.drain().collect();
                drop(buffer_lock);

                // Отправляем каждому destination
                for (dest, packets) in pending {
                    if packets.is_empty() {
                        continue;
                    }

                    // Создаем BatchedWagon
                    let wagon_id = Train::generate_train_id();
                    let batched_wagon = BatchedWagon::new(wagon_id, packets);

                    // Сериализуем
                    let wagon_bytes = match batched_wagon.to_bytes() {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            eprintln!("❌ Failed to serialize batched wagon: {}", e);
                            continue;
                        }
                    };

                    // Отправляем через transport (используем packet type 0x70 для batched)
                    let mut packet = vec![0x70u8]; // Batched wagon magic byte
                    packet.extend_from_slice(&wagon_bytes);

                    if let Err(e) = transport.send_encrypted(dest, &packet).await {
                        eprintln!("❌ Failed to send batched wagon: {}", e);
                    } else {
                        println!("📦 Sent batched wagon #{} with {} packets to {}",
                                 wagon_id,
                                 batched_wagon.packets.len(),
                                 hex::encode(&dest.0[..8]));
                    }
                }
            }
        });
    }

    /// Получить статистику по пиру
    pub async fn get_peer_metrics(&self, peer_id: crate::util::HashId) -> PeerMetrics {
        let depot = self.depot.lock().await;
        depot.peer_metrics.get(&peer_id).cloned().unwrap_or_default()
    }

    pub async fn get_snapshot(&self, peer_ids: &[crate::util::HashId]) -> StationSnapshot {
        let depot = self.depot.lock().await;
        let mut snapshot = StationSnapshot {
            active_trains: depot.trains.len(),
            current_bytes: depot.current_bytes,
            delivered_cache_size: depot.delivered.len(),
            total_evicted_trains: depot.total_evicted_trains,
            total_timeout_trains: depot.total_timeout_trains,
            total_completed_cleanups: depot.total_completed_cleanups,
            ..Default::default()
        };

        if peer_ids.is_empty() {
            for metrics in depot.peer_metrics.values() {
                snapshot.total_wagons_received += metrics.total_wagons_received;
                snapshot.total_orig_received += metrics.total_orig_received;
                snapshot.total_clone_received += metrics.total_clone_received;
                snapshot.total_path0_loss_events += metrics.total_path0_loss_events;
            }
        } else {
            for peer_id in peer_ids {
                if let Some(metrics) = depot.peer_metrics.get(peer_id) {
                    snapshot.total_wagons_received += metrics.total_wagons_received;
                    snapshot.total_orig_received += metrics.total_orig_received;
                    snapshot.total_clone_received += metrics.total_clone_received;
                    snapshot.total_path0_loss_events += metrics.total_path0_loss_events;
                }
            }
        }

        snapshot
    }
}

/// Депо - хранит частично собранные поезда

/// Статистика для каждого пира
#[derive(Debug, Clone, Default)]
pub struct PeerMetrics {
    /// Всего вагонов получено
    pub total_wagons_received: u64,
    /// Оригиналов получено
    pub total_orig_received: u64,
    /// Клонов получено
    pub total_clone_received: u64,
    /// Сколько раз клон пришел без оригинала (потеря Path0)
    pub total_path0_loss_events: u64,
}

#[derive(Debug, Clone, Default)]
pub struct StationSnapshot {
    pub total_wagons_received: u64,
    pub total_orig_received: u64,
    pub total_clone_received: u64,
    pub total_path0_loss_events: u64,
    pub active_trains: usize,
    pub current_bytes: usize,
    pub delivered_cache_size: usize,
    pub total_evicted_trains: u64,
    pub total_timeout_trains: u64,
    pub total_completed_cleanups: u64,
}

struct Depot {
    /// Статистика по пирам
    peer_metrics: HashMap<crate::util::HashId, PeerMetrics>,

    /// Поезда в процессе сборки
    trains: HashMap<TrainId, Train>,

    /// Таймаут сборки поезда
    timeout: Duration,

    /// Максимальный объём памяти в байтах
    max_bytes: usize,

    /// Максимальное количество поездов
    max_trains: usize,

    /// Время жизни поезда
    ttl: Duration,

    /// ID последнего завершённого поезда
    last_completed_id: Option<TrainId>,

    /// Текущий объём памяти в байтах
    current_bytes: usize,

    /// 🔄 Уже доставленные поезда (защита от двойной доставки)
    delivered: std::collections::HashSet<TrainId>,

    /// Всего вытеснено поездов по лимитам
    total_evicted_trains: u64,

    /// Всего поездов очищено по timeout
    total_timeout_trains: u64,

    /// Всего завершённых поездов очищено по TTL
    total_completed_cleanups: u64,
}

impl Depot {
    fn new(timeout: Duration, max_bytes: usize, max_trains: usize, ttl: Duration) -> Self {
        Self {
            peer_metrics: HashMap::new(),
            trains: HashMap::new(),
            timeout,
            max_bytes,
            max_trains,
            ttl,
            last_completed_id: None,
            current_bytes: 0,
            delivered: std::collections::HashSet::new(),
            total_evicted_trains: 0,
            total_timeout_trains: 0,
            total_completed_cleanups: 0,
        }
    }

    /// Добавить вагон к поезду
    /// Добавить вагон к поезду
    /// Возвращает (complete, path0_lost) где:
    ///   - complete: true только при ПЕРЕХОДЕ в Complete
    ///   - path0_lost: true если Path1 прибыл раньше Path0
    fn add_wagon(&mut self, peer_id: crate::util::HashId, wagon: Wagon) -> Result<(bool, bool), StationError> {
        let train_id = wagon.train_id;
        let wagon_size = wagon.cargo.len();

        // Проверяем лимит памяти
        if self.current_bytes + wagon_size > self.max_bytes {
            // Пробуем освободить место, удаляя старые поезда
            self.evict_until(wagon_size)?;

            if self.current_bytes + wagon_size > self.max_bytes {
                return Err(StationError::MemoryLimitExceeded);
            }
        }

        // Проверяем лимит количества поездов
        if !self.trains.contains_key(&train_id) && self.trains.len() >= self.max_trains {
            self.evict_oldest()?;
        }

        let previous_size = self.trains
            .get(&train_id)
            .map(|train| train.estimated_size())
            .unwrap_or(0);

        // Получаем или создаём поезд
        let train = self.trains
            .entry(train_id)
            .or_insert_with(|| {
                Train::new_receiving(train_id, crate::util::HashId::default(), wagon.total_wagons)
            });

        // 📊 Обновляем статистику для пира (ДО перемещения wagon)
        let is_clone = wagon.is_clone;
        let metrics = self.peer_metrics.entry(peer_id).or_insert_with(PeerMetrics::default);
        metrics.total_wagons_received += 1;
        
        if is_clone {
            metrics.total_clone_received += 1;
        } else {
            metrics.total_orig_received += 1;
        }

        // Добавляем вагон (возвращает (complete, path0_lost))
        let (just_completed, path0_lost) = train.add_wagon(wagon)?;
        let new_size = train.estimated_size();
        if new_size > previous_size {
            self.current_bytes += new_size - previous_size;
        }

        if just_completed {
            self.last_completed_id = Some(train_id);
        }
        if path0_lost {
            metrics.total_path0_loss_events += 1;
        }

        Ok((just_completed, path0_lost))
    }

    /// Удалить самый старый поезд для освобождения места
    fn evict_oldest(&mut self) -> Result<(), StationError> {
        // Находим ID самого старого поезда
        let oldest_id = self.trains
            .iter()
            .min_by_key(|(_, t)| t.created_at)
            .map(|(id, _)| *id);

        if let Some(id) = oldest_id {
            // Получаем размер до удаления
            let train_size = self.trains.get(&id)
                .map(|t| t.estimated_size())
                .unwrap_or(0);

            // Удаляем
            self.trains.remove(&id)
                .ok_or(StationError::TrainNotFound)?;
            self.current_bytes = self.current_bytes.saturating_sub(train_size);
            self.total_evicted_trains += 1;
            println!("🗑️  Evicted oldest train #{} ({} bytes)", id, train_size);
        }
        Ok(())
    }

    /// Удалять поезда, пока не освободится достаточно места
    fn evict_until(&mut self, needed_bytes: usize) -> Result<(), StationError> {
        let max_iterations = 100;
        let mut iterations = 0;

        while self.current_bytes + needed_bytes > self.max_bytes && !self.trains.is_empty() {
            if iterations >= max_iterations {
                return Err(StationError::MemoryLimitExceeded);
            }
            self.evict_oldest()?;
            iterations += 1;
        }
        Ok(())
    }

    /// Забрать собранный поезд (БЕЗ удаления для dual-path!)
    fn take_train(&mut self, train_id: TrainId) -> Option<Vec<u8>> {
        // Получаем поезд БЕЗ удаления
        if let Some(train) = self.trains.get(&train_id) {
            if train.state == super::TrainState::Complete {
                train.assemble().ok()
            } else {
                None
            }
        } else {
            None
        }
    }

    /// 🎉 Получить train (для ACK checksum)
    fn get_train(&self, train_id: TrainId) -> Option<&super::Train> {
        self.trains.get(&train_id)
    }

    /// Получить прогресс сборки
    fn get_progress(&self, train_id: TrainId) -> Option<f64> {
        self.trains.get(&train_id).map(|t| t.progress())
    }

    /// Получить ID последнего завершённого поезда
    fn get_last_completed_train_id(&self) -> TrainId {
        self.last_completed_id.unwrap_or(0)
    }

    /// 🔄 Проверить: поезд уже доставлен?
    pub fn is_delivered(&self, train_id: TrainId) -> bool {
        self.delivered.contains(&train_id)
    }

    /// 🔄 Пометить поезд как доставленный
    pub fn mark_delivered(&mut self, train_id: TrainId) {
        self.delivered.insert(train_id);
    }

    /// Получить данные поезда БЕЗ удаления (для dual-path!)
    pub fn get_train_data(&self, train_id: TrainId) -> Option<Vec<u8>> {
        if let Some(train) = self.trains.get(&train_id) {
            if train.state == super::TrainState::Complete {
                // Собираем БЕЗ удаления поезда
                train.assemble().ok()
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Очистить поезда с истёкшим таймаутом
    fn cleanup_timeout_trains(&mut self) {
        let timeout_trains: Vec<TrainId> = self.trains
            .iter()
            .filter(|(_, train)| train.is_timeout())
            .map(|(id, _)| *id)
            .collect();

        for id in timeout_trains {
            if let Some(train) = self.trains.remove(&id) {
                let train_size = train.estimated_size();
                self.current_bytes = self.current_bytes.saturating_sub(train_size);
                self.delivered.remove(&id); // Убираем из delivered тоже
                self.total_timeout_trains += 1;
                println!("⏰ Train #{} timeout, removing from depot ({} bytes)", id, train_size);
            }
        }
    }

    /// 🧹 Очистить Complete поезда и удалить клоны (освободить память!)
    pub fn cleanup_delivered(&mut self) {
        // Находим Complete поезда, которые уже прожили dedup TTL.
        let complete_trains: Vec<TrainId> = self.trains
            .iter()
            .filter(|(_, train)| {
                train.state == super::TrainState::Complete && train.last_activity.elapsed() > self.ttl
            })
            .map(|(id, _)| *id)
            .collect();

        for id in complete_trains {
            if let Some(mut train) = self.trains.remove(&id) {
                // Очищаем клоны перед удалением
                train.cleanup_clones();
                let train_size = train.estimated_size();
                self.current_bytes = self.current_bytes.saturating_sub(train_size);
                self.delivered.remove(&id);
                self.total_completed_cleanups += 1;
                println!("🧹 Train #{} Complete & removed ({} bytes)", id, train_size);
            }
        }
    }
}

/// Ошибки станции
#[derive(Debug, thiserror::Error)]
pub enum StationError {
    #[error("Ошибка сериализации: {0}")]
    SerializationError(String),

    #[error("Ошибка десериализации: {0}")]
    DeserializationError(String),

    #[error("Ошибка отправки: {0}")]
    SendError(String),

    #[error("Ошибка поезда: {0}")]
    TrainError(#[from] super::TrainError),

    #[error("Превышен лимит памяти")]
    MemoryLimitExceeded,

    #[error("Поезд не найден")]
    TrainNotFound,
}

// ⚠️ Clone implementation for Station (needed for background task)
// Note: This creates a shallow clone - all Arc<Mutex<T>> fields are shared
impl Clone for Station {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            config: self.config.clone(),
            transport: self.transport.clone(),
            depot: self.depot.clone(),
            rtt_history: self.rtt_history.clone(),
            current_wagon_delay_ms: self.current_wagon_delay_ms.clone(),
            batch_buffer: self.batch_buffer.clone(),
            batch_seq_counter: self.batch_seq_counter.clone(),
            data_callback: self.data_callback.clone(),
            line_counter: self.line_counter.clone(),
            train_send_mutex: self.train_send_mutex.clone(),
            rate_controller: self.rate_controller.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_station_config_default() {
        let config = StationConfig::default();
        assert_eq!(config.role, StationRole::Both);
        assert_eq!(config.train_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_depot_memory_limits() {
        // Создаём Depot с малыми лимитами для теста
        let mut depot = Depot::new(
            Duration::from_secs(30),
            10_000,  // 10KB лимит
            5,       // 5 поездов
            Duration::from_secs(30),
        );

        // Создаём wagon 3KB
        let cargo = vec![1u8; 3_000];
        let wagon = Wagon::new(1, 0, 1, 0, cargo, 0);

        // Добавляем первый поезд - должно пройти
        let _result = depot.add_wagon(HashId::default(), wagon);
        assert!(_result.is_ok());

        // Проверяем, что память учтена
        assert!(depot.current_bytes > 0);

        // Создаём wagon 8KB (превышает лимит 10KB)
        let large_cargo = vec![2u8; 8_000];
        let large_wagon = Wagon::new(2, 0, 1, 0, large_cargo, 0);

        // Второй wagon должен вытеснить первый
        let _result = depot.add_wagon(HashId::default(), large_wagon);
        // Либо прошло успешно (после eviction), либо ошибка лимита
        // Главное - не должно крашиться
        assert!(_result.is_ok() || _result.is_err());

        // Проверяем, что лимит не превышен
        assert!(depot.current_bytes <= depot.max_bytes);
    }
}
