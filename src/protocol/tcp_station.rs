// src/protocol/tcp_station.rs
//!
//! # TCP Station (Станция TCP)
//!
//! Отправляет и принимает поезда поверх TCP. Полный аналог UDP Station,
//! но с надежным TCP транспортом.
//!
//! Особенности:
//! - Dual-Path с клонами (path 0, path 1)
//! - Дедупликация вагонов
//! - Игнорирование ACK/NACK
//! - Полное шифрование ECDH + AES-256-GCM
//! - Восстановление из клонов

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};

use crate::protocol::{Train, TrainId, TrainState, Wagon, WagonFlags};
use crate::protocol::train::TrainError;
use crate::util::HashId;
use crate::netlayer::encryption::EncryptionManager;
use tracing::{info, error, debug, warn};

/// ID TCP соединения
type TcpConnId = u64;

/// Конфигурация TCP станции
#[derive(Debug, Clone)]
pub struct TcpStationConfig {
    /// Таймаут сборки поезда
    pub train_timeout: Duration,

    /// Максимальный размер вагона
    pub max_wagon_size: usize,

    /// Размер буфера для отправки
    pub send_buffer_size: usize,

    /// Размер буфера для приёма
    pub recv_buffer_size: usize,

    /// Включить дедупликацию
    pub enable_dedup: bool,

    /// Размер окна дедупликации (количество вагонов)
    pub dedup_window_size: usize,
}

impl Default for TcpStationConfig {
    fn default() -> Self {
        Self {
            train_timeout: Duration::from_secs(30),
            max_wagon_size: 16 * 1024,  // 16KB
            send_buffer_size: 256 * 1024,  // 256KB
            recv_buffer_size: 256 * 1024,  // 256KB
            enable_dedup: true,
            dedup_window_size: 10_000,  // 10k вагонов
        }
    }
}

/// TCP Wagon (расширенная версия с дедупликацией)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpWagon {
    /// Базовый вагон
    #[serde(flatten)]
    pub base: Wagon,

    /// Уникальный ID вагона для дедупликации
    pub unique_id: u64,

    /// Timestamp создания (мс)
    pub timestamp_ms: u64,
}

impl TcpWagon {
    /// Создать новый TCP вагон
    pub fn new(
        train_id: u64,
        wagon_num: u32,
        total_wagons: u32,
        offset: u64,
        cargo: Vec<u8>,
        line_id: u8,
    ) -> Self {
        let base = Wagon::new(train_id, wagon_num, total_wagons, offset, cargo, line_id);

        // Генерируем уникальный ID
        let unique_id = Self::generate_unique_id(train_id, wagon_num, line_id);

        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self { base, unique_id, timestamp_ms }
    }

    /// Создать клон вагона (для Path1)
    pub fn new_clone(
        train_id: u64,
        wagon_num: u32,
        total_wagons: u32,
        offset: u64,
        cargo: Vec<u8>,
        line_id: u8,
    ) -> Self {
        let base = Wagon::with_clone(train_id, true, wagon_num, total_wagons, offset, cargo, line_id);

        let unique_id = Self::generate_unique_id(train_id, wagon_num, line_id);

        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self { base, unique_id, timestamp_ms }
    }

    /// Генерировать уникальный ID из комбинации параметров
    fn generate_unique_id(train_id: u64, wagon_num: u32, line_id: u8) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let mut hasher = DefaultHasher::new();
        train_id.hash(&mut hasher);
        wagon_num.hash(&mut hasher);
        line_id.hash(&mut hasher);
        hasher.finish()
    }

    /// Упаковать в байты
    pub fn to_bytes(&self) -> Result<Vec<u8>, anyhow::Error> {
        Ok(bincode::serialize(self)?)
    }

    /// Распаковать из байтов
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, anyhow::Error> {
        Ok(bincode::deserialize(bytes)?)
    }
}

/// TCP Train (расширенная версия с поддержкой клонов)
#[derive(Debug, Clone)]
pub struct TcpTrain {
    /// Базовый поезд
    pub base: Train,

    /// Полученные TCP вагоны (path 0)
    pub wagons_path0: HashMap<u32, TcpWagon>,

    /// Полученные клоны (path 1)
    pub wagons_path1: HashMap<u32, TcpWagon>,

    /// Дедупликация: уже полученные unique_id
    pub dedup_set: HashSet<u64>,
}

impl TcpTrain {
    /// Создать новый поезд для отправки
    pub fn new(source: HashId, dest: HashId, data: Vec<u8>) -> Self {
        let base = Train::new(source, dest, data);

        Self {
            base,
            wagons_path0: HashMap::new(),
            wagons_path1: HashMap::new(),
            dedup_set: HashSet::new(),
        }
    }

    /// Создать пустой поезд для приёма
    pub fn new_receiving(train_id: TrainId, source: HashId, total_wagons: u32) -> Self {
        let base = Train::new_receiving(train_id, source, total_wagons);

        Self {
            base,
            wagons_path0: HashMap::new(),
            wagons_path1: HashMap::new(),
            dedup_set: HashSet::new(),
        }
    }

    /// Добавить вагон с дедупликацией
    /// Возвращает (complete, path0_lost, was_duplicate)
    pub fn add_wagon(&mut self, wagon: TcpWagon) -> Result<(bool, bool, bool), TrainError> {
        // Проверяем дедупликацию
        if self.dedup_set.contains(&wagon.unique_id) {
            debug!("🔄 Duplicate wagon {} (train {}) - ignored", wagon.unique_id, wagon.base.train_id);
            return Ok((false, false, true));
        }

        // Добавляем в дедуп-множество
        self.dedup_set.insert(wagon.unique_id);

        // Сохраняем нужные поля ДО move
        let wagon_num = wagon.base.wagon_num;
        let is_clone = wagon.base.is_clone;

        // Определяем путь
        if is_clone {
            // Path1 (клон)
            self.wagons_path1.insert(wagon_num, wagon);
        } else {
            // Path0 (оригинал)
            self.wagons_path0.insert(wagon_num, wagon);
        }

        // Проверяем Path0 loss
        let path0_lost = is_clone && !self.wagons_path0.contains_key(&wagon_num);

        // Создаём временный wagon для добавления в базовый поезд
        // (нужно извлечь из соответствующего path)
        let base_wagon = if is_clone {
            self.wagons_path1.get(&wagon_num).map(|w| w.base.clone())
        } else {
            self.wagons_path0.get(&wagon_num).map(|w| w.base.clone())
        };

        if let Some(base) = base_wagon {
            // Добавляем в базовый поезд
            let (complete, _) = self.base.add_wagon(base)?;
            Ok((complete, path0_lost, false))
        } else {
            Ok((false, false, false))
        }
    }

    /// Собрать данные из поезда
    pub fn assemble(&self) -> Result<Vec<u8>, TrainError> {
        self.base.assemble()
    }

    /// Очистить клоны после сборки
    pub fn cleanup_clones(&mut self) {
        self.wagons_path1.clear();
        self.base.cleanup_clones();
    }
}

/// Callback для получения данных
pub type TcpDataCallback = Arc<dyn Fn(HashId, Vec<u8>) + Send + Sync>;

/// TCP Станция - полный аналог UDP Station для TCP
pub struct TcpStation {
    /// ID станции
    pub id: HashId,

    /// Конфигурация
    config: TcpStationConfig,

    /// Менеджер шифрования
    encryption: Arc<Mutex<EncryptionManager>>,

    /// Депо - принимает поезда
    depot: Arc<Mutex<TcpDepot>>,

    /// Активные TCP соединения
    connections: Arc<Mutex<HashMap<TcpConnId, TcpConnection>>>,

    /// Следующий ID соединения
    next_conn_id: Arc<AtomicU64>,

    /// Callback для получения данных
    data_callback: Arc<Mutex<Option<TcpDataCallback>>>,

    /// Counter для train_id
    train_counter: Arc<AtomicU64>,
}

impl TcpStation {
    /// Создать новую TCP станцию
    pub fn new(
        id: HashId,
        encryption: Arc<Mutex<EncryptionManager>>,
        config: TcpStationConfig,
    ) -> Self {
        let depot = Arc::new(Mutex::new(TcpDepot::new(config.train_timeout)));

        Self {
            id,
            config,
            encryption,
            depot,
            connections: Arc::new(Mutex::new(HashMap::new())),
            next_conn_id: Arc::new(AtomicU64::new(1)),
            data_callback: Arc::new(Mutex::new(None)),
            train_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Создать с дефолтной конфигурацией
    pub fn with_defaults(id: HashId, encryption: Arc<Mutex<EncryptionManager>>) -> Self {
        Self::new(id, encryption, TcpStationConfig::default())
    }

    /// Установить callback для получения данных
    pub async fn set_data_callback<F>(&self, callback: F)
    where
        F: Fn(HashId, Vec<u8>) + Send + Sync + 'static
    {
        let mut cb = self.data_callback.lock().await;
        *cb = Some(Arc::new(callback));
    }

    /// 🔄 DUAL-PATH: Отправить данные как поезд с клонированием на ОБА пути
    pub async fn send_train(
        &self,
        dest: HashId,
        tcp_stream: &mut TcpStream,
        data: Vec<u8>,
    ) -> Result<TrainId, anyhow::Error> {
        let train_id = self.train_counter.fetch_add(1, Ordering::SeqCst);
        let total_wagons = Train::calculate_wagon_count(&data);

        info!("🚂🔄 TCP_STATION[{}] DUAL-PATH train #{} ({} wagons, {} bytes) → {}",
              hex::encode(&self.id.0[..8]),
              train_id,
              total_wagons,
              data.len(),
              hex::encode(&dest.0[..8]));

        // Разбиваем на вагоны
        let wagon_size = self.config.max_wagon_size;
        let chunks: Vec<_> = data.chunks(wagon_size).enumerate().collect();

        // 🔄 DUAL-PATH: отправляем wagons на ОБЕИХ путях
        for (i, chunk) in chunks {
            let wagon_num = i as u32;
            let offset = (i * wagon_size) as u64;

            // Path0: оригинал
            let wagon_path0 = TcpWagon::new(
                train_id,
                wagon_num,
                total_wagons,
                offset,
                chunk.to_vec(),
                0,  // line_id = 0
            );

            // Path1: клон
            let wagon_path1 = TcpWagon::new_clone(
                train_id,
                wagon_num,
                total_wagons,
                offset,
                chunk.to_vec(),
                1,  // line_id = 1
            );

            // Отправляем оба вагона
            self.send_wagon_encrypted(dest, tcp_stream, &wagon_path0).await?;
            self.send_wagon_encrypted(dest, tcp_stream, &wagon_path1).await?;

            debug!("📦 [WAGON {}/{}] Path0 + Path1 sent ({} B)",
                   wagon_num + 1, total_wagons, chunk.len());
        }

        info!("✅ Train #{} sent on DUAL-PATH!", train_id);
        Ok(train_id)
    }

    /// Отправить вагон с шифрованием
    async fn send_wagon_encrypted(
        &self,
        dest: HashId,
        tcp_stream: &mut TcpStream,
        wagon: &TcpWagon,
    ) -> Result<()> {
        // Упаковываем вагон
        let wagon_bytes = wagon.to_bytes()
            .map_err(|e| anyhow!("Serialization error: {}", e))?;

        // Шифруем
        let encrypted = {
            let enc = self.encryption.lock().await;
            // Создаём временный PeerInfo для шифрования
            let peer = crate::netlayer::peer::PeerInfo::new(dest, "");
            enc.encrypt(&peer, &wagon_bytes)
                .map_err(|e| anyhow!("Encryption error: {}", e))?
        };

        // Отправляем длину + данные
        let len = encrypted.len() as u32;
        tcp_stream.write_all(&len.to_be_bytes()).await?;
        tcp_stream.write_all(&encrypted).await?;

        Ok(())
    }

    /// Принять вагон из TCP потока
    pub async fn receive_wagon(
        &self,
        tcp_stream: &mut TcpStream,
        source_id: HashId,
    ) -> Result<Option<TrainId>> {
        // Читаем длину
        let mut len_bytes = [0u8; 4];
        tcp_stream.read_exact(&mut len_bytes).await?;
        let len = u32::from_be_bytes(len_bytes) as usize;

        // Читаем зашифрованные данные
        let mut encrypted = vec![0u8; len];
        tcp_stream.read_exact(&mut encrypted).await?;

        // Расшифровываем
        let wagon_bytes = {
            let mut enc = self.encryption.lock().await;
            let peer = crate::netlayer::peer::PeerInfo::new(source_id, "");
            enc.decrypt(&peer, &encrypted)
                .map_err(|e| anyhow!("Decryption error: {}", e))?
        };

        // Десериализуем вагон
        let tcp_wagon = TcpWagon::from_bytes(&wagon_bytes)
            .map_err(|e| anyhow!("Deserialization error: {}", e))?;

        // Проверяем checksum
        if !tcp_wagon.base.verify() {
            warn!("⚠️ Wagon has invalid checksum! Dropping.");
            return Ok(None);
        }

        info!("📥 [WAGON {}/{}] from train #{} ({} KB)",
              tcp_wagon.base.wagon_num + 1,
              tcp_wagon.base.total_wagons,
              tcp_wagon.base.train_id,
              tcp_wagon.base.cargo.len() / 1024);

        // 🚇 Вызываем callback СРАЗУ
        {
            let cb = self.data_callback.lock().await;
            if let Some(callback) = cb.as_ref() {
                callback(source_id, tcp_wagon.base.cargo.clone());
            }
        }

        // Добавляем вагон в депо
        let mut depot = self.depot.lock().await;
        let (train_complete, _path0_lost, _was_dup) = depot.add_wagon(tcp_wagon)?;

        if train_complete {
            let train_id = depot.get_last_completed_train_id();
            info!("✅ Train #{} assembled!", train_id);
            Ok(Some(train_id))
        } else {
            Ok(None)
        }
    }

    /// Получить собранный поезд
    pub async fn get_train(&self, train_id: TrainId) -> Option<Vec<u8>> {
        let depot = self.depot.lock().await;
        depot.get_train_data(train_id)
    }
}

/// TCP Депо - хранит частично собранные поезда
struct TcpDepot {
    /// Поезда в процессе сборки
    trains: HashMap<TrainId, TcpTrain>,

    /// Таймаут сборки
    timeout: Duration,

    /// ID последнего завершённого поезда
    last_completed_id: Option<TrainId>,
}

impl TcpDepot {
    fn new(timeout: Duration) -> Self {
        Self {
            trains: HashMap::new(),
            timeout,
            last_completed_id: None,
        }
    }

    /// Добавить вагон к поезду
    /// Возвращает (complete, path0_lost, was_duplicate)
    fn add_wagon(&mut self, wagon: TcpWagon) -> Result<(bool, bool, bool), TrainError> {
        let train_id = wagon.base.train_id;

        // Получаем или создаём поезд
        let train = self.trains
            .entry(train_id)
            .or_insert_with(|| {
                TcpTrain::new_receiving(train_id, HashId::default(), wagon.base.total_wagons)
            });

        // Добавляем вагон
        train.add_wagon(wagon)
    }

    /// Получить данные поезда
    fn get_train_data(&self, train_id: TrainId) -> Option<Vec<u8>> {
        if let Some(train) = self.trains.get(&train_id) {
            if train.base.state == TrainState::Complete {
                train.assemble().ok()
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Получить ID последнего завершённого поезда
    fn get_last_completed_train_id(&self) -> TrainId {
        self.last_completed_id.unwrap_or(0)
    }
}

/// TCP соединение
pub struct TcpConnection {
    /// ID соединения
    pub id: TcpConnId,

    /// Удалённый peer
    pub peer_id: HashId,

    /// TCP stream
    pub stream: Arc<Mutex<TcpStream>>,

    /// Время создания
    pub created_at: Instant,

    /// Последняя активность
    pub last_activity: Arc<Mutex<Instant>>,
}

impl TcpConnection {
    pub fn new(id: TcpConnId, peer_id: HashId, stream: TcpStream) -> Self {
        let now = Instant::now();
        Self {
            id,
            peer_id,
            stream: Arc::new(Mutex::new(stream)),
            created_at: now,
            last_activity: Arc::new(Mutex::new(now)),
        }
    }

    /// Проверить не устарело ли соединение
    pub fn is_stale(&self, timeout: Duration) -> bool {
        self.last_activity.try_lock()
            .map(|last| last.elapsed() > timeout)
            .unwrap_or(false)
    }
}

/// Ошибки TCP станции
#[derive(Debug, thiserror::Error)]
pub enum TcpStationError {
    #[error("Ошибка сериализации: {0}")]
    SerializationError(String),

    #[error("Ошибка десериализации: {0}")]
    DeserializationError(String),

    #[error("Ошибка отправки: {0}")]
    SendError(String),

    #[error("Ошибка поезда: {0}")]
    TrainError(#[from] TrainError),

    #[error("Ошибка шифрования: {0}")]
    EncryptionError(String),
}
