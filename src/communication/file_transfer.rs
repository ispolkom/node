// src/communication/file_transfer.rs
//! Chunked file transfer with ACK tracking

use crate::util::HashId;
use crate::p2p::{P2PTransport, P2PPacket, P2PPacketType};
use crate::communication::{CommPacket, CommControlPacket};
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use anyhow::Result;
use tokio::sync::{Mutex, watch};
use tracing::{info, error, debug, warn};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::{Aead, AeadCore, OsRng}};

fn encrypt_chunk(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, plaintext).expect("AES-GCM encrypt");
    let mut out = Vec::with_capacity(1 + 12 + ciphertext.len());
    out.push(0x01u8);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

fn decrypt_chunk(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 12 {
        return Err(anyhow::anyhow!("Encrypted chunk too short ({})", data.len()));
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let nonce = Nonce::from_slice(&data[..12]);
    cipher
        .decrypt(nonce, &data[12..])
        .map_err(|_| anyhow::anyhow!("AES-GCM authentication failed"))
}

/// Фиксированный размер P2P чанка для transport-safe MTU.
/// Этот размер должен совпадать с browser upload chunk size в Web UI/API.
pub const FILE_TRANSFER_CHUNK_SIZE: usize = 700;
const ACK_WAIT_SLICE_MS: u64 = 200;
const ACK_ROUND_TIMEOUT_MS: u64 = 2000;
const MAX_RETRY_ROUNDS: usize = 6;
const MAX_PRESTART_CHUNKS_PER_FILE: usize = 256;
const MAX_PRESTART_FILES: usize = 64;
/// Pre-start buffering: most bytes kept in total and how long a buffered chunk waits for its transfer to start.
const MAX_PRESTART_BYTES: usize = 4 * 1024 * 1024;
const PRESTART_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// Records of finished incoming transfers kept (a repeated end of a finished transfer is answered from them).
const MAX_COMPLETED_RECORDS: usize = 4096;
/// Most missing ranges named in one FileMissing message (the rest is reported in the next round).
const MAX_RANGES_PER_MESSAGE: usize = 1000;

/// Hard cap on a single incoming transfer's declared size. This
/// mechanism (700B chunks) is sized for chat-style attachments, not
/// bulk transfer — 200 MB is generous for that while keeping the
/// resulting `total_chunks`, and the Vec<bool> allocated for it in
/// `start_receiving`, bounded to a few hundred KB at most.
///
/// "Valid hostile peer" audit (2026-09-15/16): an already-authenticated
/// peer's own FileChunkStart carries `file_size`/`total_chunks` as
/// plain, self-reported fields with no prior validation at all. Before
/// this fix, a single ~130-byte message declaring `total_chunks =
/// u32::MAX` forced an immediate ~4 GB memory reservation (proven live,
/// see node/tests/valid_peer_resource_exhaustion_test.rs) — transport
/// authentication only proves who sent a message, never that its
/// content is safe to act on.
pub const MAX_FILE_TRANSFER_SIZE: u64 = 200 * 1024 * 1024;


/// Сохранить чекпоинт передачи в файл
fn save_checkpoint(file_id: &str, filename: &str, sent_chunks: u32, total_chunks: u32) -> Result<()> {
    let cache_dir = crate::communication::files_dir("cache");
    std::fs::create_dir_all(&cache_dir)?;
    let checkpoint_path = cache_dir.join(format!("transfer_{}.state", file_id));
    let content = format!("{}\n{}\n{}\n", filename, sent_chunks, total_chunks);
    std::fs::write(&checkpoint_path, content)?;
    debug!("💾 Checkpoint saved: {} chunks sent", sent_chunks);
    Ok(())
}

#[derive(Debug)]
struct CheckpointData {
    filename: String,
    sent_chunks: u32,
    total_chunks: u32,
}


/// Загрузить чекпоинт передачи из файла
fn load_checkpoint(file_id: &str) -> Result<Option<CheckpointData>> {
    let cache_dir = crate::communication::files_dir("cache");
    let checkpoint_path = cache_dir.join(format!("transfer_{}.state", file_id));
    if !checkpoint_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&checkpoint_path)?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() >= 3 {
        let filename = lines[0].to_string();
        let sent_chunks = lines[1].parse::<u32>().unwrap_or(0);
        let total_chunks = lines[2].parse::<u32>().unwrap_or(0);
        debug!("📂 Checkpoint loaded: {} ({} sent of {} chunks)", filename, sent_chunks, total_chunks);
        Ok(Some(CheckpointData { filename, sent_chunks, total_chunks }))
    } else {
        Ok(None)
    }
}

fn sanitize_filename(filename: &str) -> String {
    // SEC-08: strip path components, reject traversal and unicode path separators
    let name = std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");

    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    let trimmed = sanitized.trim_start_matches('.');
    let trimmed = trimmed.trim_end_matches(|c: char| c == '.' || c == ' ');

    // Windows device names (CON, NUL, COM1, ...) are not valid file names there, with or without an extension
    let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());

    if trimmed.is_empty() {
        "file".to_string()
    } else if reserved {
        format!("_{}", trimmed.chars().take(199).collect::<String>())
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn storage_filename(file_id: &str, filename: &str) -> String {
    format!("{}__{}", file_id, sanitize_filename(filename))
}

/// Chunk numbers a peer says it still misses: at most `MAX_MISSING_RANGES` ranges, clamped to the real chunk count, no duplicates.
const MAX_MISSING_RANGES: usize = 4096;

fn chunk_ranges_to_indices(ranges: &[crate::communication::FileChunkRange], total_chunks: u32) -> Vec<u32> {
    // keep the valid ranges (clamped to the file), merge the overlapping ones, then expand: the work is bounded by the number of
    // ranges plus the number of chunks, whatever the peer sends
    let mut valid: Vec<(u32, u32)> = ranges
        .iter()
        .take(MAX_MISSING_RANGES)
        .filter(|r| r.start <= r.end && r.start < total_chunks)
        .map(|r| (r.start, r.end.min(total_chunks - 1)))
        .collect();
    valid.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in valid {
        match merged.last_mut() {
            Some(last) if start <= last.1.saturating_add(1) => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged.into_iter().flat_map(|(start, end)| start..=end).collect()
}

fn collect_missing_ranges(received_chunks: &[bool]) -> Vec<crate::communication::FileChunkRange> {
    let mut ranges = Vec::new();
    let mut start: Option<u32> = None;

    for (idx, received) in received_chunks.iter().enumerate() {
        if !received {
            if start.is_none() {
                start = Some(idx as u32);
            }
        } else if let Some(range_start) = start.take() {
            ranges.push(crate::communication::FileChunkRange {
                start: range_start,
                end: idx as u32 - 1,
            });
        }
    }

    if let Some(range_start) = start {
        ranges.push(crate::communication::FileChunkRange {
            start: range_start,
            end: received_chunks.len() as u32 - 1,
        });
    }

    ranges
}

fn build_transfer_start_packet(
    my_node_id: HashId,
    file_id: &str,
    filename: &str,
    file_size: u64,
    mime_type: &str,
    total_chunks: u32,
) -> Result<P2PPacket> {
    let start_msg = crate::communication::FileChunkStart {
        file_id: file_id.to_string(),
        filename: filename.to_string(),
        file_size,
        mime_type: mime_type.to_string(),
        total_chunks,
    };
    let start_data = serde_json::to_vec(&start_msg)?;
    Ok(P2PPacket::new(
        P2PPacketType::FileTransferStart,
        my_node_id,
        false,
        start_data,
    ))
}


/// File Transfer Manager
pub struct FileTransferManager {
    my_node_id: HashId,
    transport: Arc<P2PTransport>,

    /// Отправляемые файлы (outgoing transfers) - в памяти

    /// Отправляемые файлы с диска (outgoing transfers) - большие файлы
    outgoing_disk: Mutex<HashMap<String, OutgoingTransferDisk>>,

    /// Принимаемые файлы (incoming transfers)
    incoming: Mutex<HashMap<String, IncomingTransfer>>,

    /// Недавно завершённые входящие передачи.
    /// Нужны, чтобы повторно ответить FileComplete, если финальный ACK потерялся.
    /// finished incoming transfers: id -> the peer that sent it (an id finished for one peer is not reusable by another)
    completed_incoming: Mutex<HashMap<String, HashId>>,

    /// Чанки, пришедшие раньше FileTransferStart из-за reorder в dual-path.
    prestart_chunks: Mutex<HashMap<String, Vec<BufferedChunk>>>,

}

/// Отправляемый файл (в памяти)
#[derive(Debug)]
struct OutgoingTransferDisk {
    filename: String,
    file_size: u64,
    mime_type: String,
    total_chunks: u32,
    file_id: String,
    /// who this file is being sent to: only this peer may report missing chunks or completion
    to_peer: HashId,
    file_path: std::path::PathBuf,
    last_checkpoint: u32,
    pending_missing: Option<Vec<u32>>,
    response_tx: watch::Sender<u64>,
    response_version: u64,
    remote_completed: bool,
}

/// Принимаемый файл
#[derive(Debug)]
struct IncomingTransfer {
    filename: String,
    file_size: u64,
    mime_type: String,
    total_chunks: u32,
    temp_path: std::path::PathBuf,
    final_path: std::path::PathBuf,
    received_chunks: Vec<bool>,
    received_count: u32,
    from_peer: HashId,
    last_activity: std::time::Instant,
}

/// Пределы приёма: собеседник не должен забирать у нас весь диск и память.
pub const MAX_INCOMING_PER_PEER: usize = 8;
pub const MAX_INCOMING_TOTAL: usize = 32;
const INCOMING_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);

/// `file_id` приходит от собеседника и попадает в имя файла на диске: только безопасные знаки.
pub fn valid_file_id(id: &str) -> bool {
    // "__" separates the id from the file name in the stored file's name, so an id must not contain it (otherwise two different
    // (id, name) pairs could map to one path)
    !id.is_empty() && id.len() <= 64 && !id.contains("__") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Можно ли начать ещё один приём. `existing` — (от кого, как давно была активность) по каждому идущему приёму.
pub fn incoming_admissible(existing: &[(HashId, std::time::Duration)], from: &HashId) -> bool {
    let live: Vec<_> = existing.iter().filter(|(_, idle)| *idle < INCOMING_STALE_AFTER).collect();
    live.len() < MAX_INCOMING_TOTAL && live.iter().filter(|(p, _)| p == from).count() < MAX_INCOMING_PER_PEER
}

#[derive(Debug, Clone)]
struct BufferedChunk {
    at: std::time::Instant,
    from: HashId,
    chunk_index: u32,
    total_chunks: u32,
    data: Vec<u8>,
}

impl FileTransferManager {
    /// Создать новый FileTransferManager
    pub fn new(my_node_id: HashId, transport: Arc<P2PTransport>) -> Self {
        Self {
            my_node_id,
            transport,
            outgoing_disk: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            completed_incoming: Mutex::new(HashMap::new()),
            prestart_chunks: Mutex::new(HashMap::new()),
        }
    }

    /// Начать отправку файла с диска
    pub async fn start_file_transfer_from_disk(
        &self,
        to: HashId,
        filename: String,
        file_path: std::path::PathBuf,
        mime_type: String,
    ) -> Result<String> {
        self.start_file_transfer_from_disk_with_id(to, None, filename, file_path, mime_type).await
    }

    /// Начать отправку файла с заранее известным идентификатором
    pub async fn start_file_transfer_from_disk_with_id(
        &self,
        to: HashId,
        explicit_file_id: Option<String>,
        filename: String,
        file_path: std::path::PathBuf,
        mime_type: String,
    ) -> Result<String> {
        let metadata = tokio::fs::metadata(&file_path).await?;
        let file_size = metadata.len();
        let total_chunks = ((file_size as usize + FILE_TRANSFER_CHUNK_SIZE - 1) / FILE_TRANSFER_CHUNK_SIZE) as u32;

        let file_id = explicit_file_id.unwrap_or_else(|| {
            format!(
                "{}_{:016x}",
                hex::encode(&self.my_node_id.0[..8]),
                rand::random::<u64>()
            )
        });

        info!("📤 Starting file transfer from disk: {} ({} bytes, {} chunks)",
            filename, file_size, total_chunks);

        let start_msg = crate::communication::FileChunkStart {
            file_id: file_id.clone(),
            filename: filename.clone(),
            file_size,
            mime_type: mime_type.clone(),
            total_chunks,
        };
        let start_data = serde_json::to_vec(&start_msg)?;
        let p2p_packet = P2PPacket::new(
            P2PPacketType::FileTransferStart,
            self.my_node_id,
            false,
            start_data,
        );
        self.transport.send_packet_dual_path(to, p2p_packet).await.map_err(|e| anyhow::anyhow!("{}", e))?;

        let (response_tx, _) = watch::channel(0u64);
        let transfer = OutgoingTransferDisk {
            filename,
            file_size,
            mime_type,
            total_chunks,
            file_id: file_id.clone(),
            to_peer: to,
            file_path: file_path.clone(),
            last_checkpoint: 0,
            pending_missing: None,
            response_tx,
            response_version: 0,
            remote_completed: false,
        };
        self.outgoing_disk.lock().await.insert(file_id.clone(), transfer);

        self.send_chunks(&to, &file_id).await?;

        Ok(file_id)
    }

    /// Зарегистрировать потоковую отправку: создаёт outgoing state и отправляет FileTransferStart,
    /// но не запускает проход по всем чанкам с диска.
    pub async fn register_streaming_transfer(
        &self,
        to: HashId,
        explicit_file_id: Option<String>,
        filename: String,
        file_path: std::path::PathBuf,
        mime_type: String,
        file_size: u64,
        total_chunks: u32,
    ) -> Result<String> {
        let file_id = explicit_file_id.unwrap_or_else(|| {
            format!(
                "{}_{:016x}",
                hex::encode(&self.my_node_id.0[..8]),
                rand::random::<u64>()
            )
        });

        {
            let outgoing_disk = self.outgoing_disk.lock().await;
            if outgoing_disk.contains_key(&file_id) {
                return Ok(file_id);
            }
        }

        info!(
            "📤 Starting streaming file transfer: {} ({} bytes, {} chunks)",
            filename, file_size, total_chunks
        );

        let start_msg = crate::communication::FileChunkStart {
            file_id: file_id.clone(),
            filename: filename.clone(),
            file_size,
            mime_type: mime_type.clone(),
            total_chunks,
        };
        let start_data = serde_json::to_vec(&start_msg)?;
        let p2p_packet = P2PPacket::new(
            P2PPacketType::FileTransferStart,
            self.my_node_id,
            false,
            start_data,
        );
        self.transport
            .send_packet_dual_path(to, p2p_packet)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        let (response_tx, _) = watch::channel(0u64);
        let transfer = OutgoingTransferDisk {
            filename,
            file_size,
            mime_type,
            total_chunks,
            file_id: file_id.clone(),
            to_peer: to,
            file_path,
            last_checkpoint: 0,
            pending_missing: None,
            response_tx,
            response_version: 0,
            remote_completed: false,
        };
        self.outgoing_disk.lock().await.insert(file_id.clone(), transfer);

        Ok(file_id)
    }

    /// Отправить чанки файла (из памяти)
    async fn send_chunks(&self, to: &HashId, file_id: &str) -> Result<()> {
        let mut next_chunk = 0;

        // Загружаем чекпоинт при старте
        if let Ok(Some(checkpoint)) = load_checkpoint(file_id) {
            let (current_filename, total) = {
                let outgoing_disk = self.outgoing_disk.lock().await;
                if let Some(transfer) = outgoing_disk.get(file_id) {
                    (transfer.filename.clone(), transfer.total_chunks)
                } else {
                    (String::new(), 0)
                }
            };
            if checkpoint.filename == current_filename && checkpoint.total_chunks == total {
                next_chunk = checkpoint.sent_chunks;
                info!("🔄 Resuming transfer from chunk {}", next_chunk);
            } else {
                let _ = std::fs::remove_file(
                    crate::communication::files_dir("cache")
                        .join(format!("transfer_{}.state", file_id))
                );
                info!("📁 Starting new transfer (old checkpoint mismatched)");
            }
        }

        let total_chunks = {
            let outgoing_disk = self.outgoing_disk.lock().await;
            let transfer = outgoing_disk.get(file_id).ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;
            transfer.total_chunks
        };

        while next_chunk < total_chunks {
            self.send_single_chunk_from_disk(to, file_id, next_chunk).await?;
            next_chunk += 1;
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

            // Сохраняем чекпоинт каждые 100 чанков
            if next_chunk % 100 == 0 {
                let filename = {
                    let outgoing_disk = self.outgoing_disk.lock().await;
                    if let Some(transfer) = outgoing_disk.get(file_id) {
                        transfer.filename.clone()
                    } else {
                        String::new()
                    }
                };
                let total = {
                    let outgoing_disk = self.outgoing_disk.lock().await;
                    if let Some(transfer) = outgoing_disk.get(file_id) {
                        transfer.total_chunks
                    } else {
                        0
                    }
                };
                if let Err(e) = save_checkpoint(file_id, &filename, next_chunk, total) {
                    warn!("Failed to save checkpoint: {}", e);
                }
            }
        }

        self.finalize_streaming_transfer(*to, file_id).await
    }


    /// Отправить один чанк с диска (бинарный формат)
    async fn send_single_chunk_from_disk(&self, to: &HashId, file_id: &str, chunk_index: u32) -> Result<()> {
        let (file_path, total_chunks) = {
            let outgoing_disk = self.outgoing_disk.lock().await;
            let transfer = outgoing_disk.get(file_id).ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;
            (transfer.file_path.clone(), transfer.total_chunks)
        };

        let start = (chunk_index as usize) * FILE_TRANSFER_CHUNK_SIZE;
        let mut file = tokio::fs::File::open(&file_path).await?;
        use tokio::io::{AsyncSeekExt, AsyncReadExt};
        file.seek(std::io::SeekFrom::Start(start as u64)).await?;

        let mut chunk_data = vec![0u8; FILE_TRANSFER_CHUNK_SIZE];
        let bytes_read = file.read(&mut chunk_data).await?;
        chunk_data.truncate(bytes_read);

        // Per-file AES-256-GCM application-layer encryption
        let file_key = self.transport.derive_file_key(to, file_id).await;
        let payload = match file_key {
            Some(ref key) => encrypt_chunk(key, &chunk_data),
            None => chunk_data.clone(),
        };

        let mut binary_data = Vec::new();
        let file_id_bytes = file_id.as_bytes();
        let file_id_len = file_id_bytes.len() as u8;
        binary_data.push(file_id_len);
        binary_data.extend_from_slice(file_id_bytes);
        binary_data.extend_from_slice(&chunk_index.to_be_bytes());
        binary_data.extend_from_slice(&total_chunks.to_be_bytes());
        binary_data.extend_from_slice(&payload);

        let p2p_packet = P2PPacket::new(
            P2PPacketType::FileChunk,
            self.my_node_id,
            false,
            binary_data,
        );  // total_parts=0, каждый чанк независим
        self.transport.send_packet_dual_path(*to, p2p_packet).await.map_err(|e| anyhow::anyhow!("{}", e))?;

        debug!("📦 Sent chunk {}/{} from disk ({} bytes)", chunk_index + 1, total_chunks, bytes_read);
        Ok(())
    }

    /// Отправить уже полученный chunk напрямую, без повторного чтения с диска.
    pub async fn send_streaming_chunk(
        &self,
        to: HashId,
        file_id: &str,
        chunk_index: u32,
        chunk_data: &[u8],
    ) -> Result<()> {
        let total_chunks = {
            let outgoing_disk = self.outgoing_disk.lock().await;
            let transfer = outgoing_disk
                .get(file_id)
                .ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;
            transfer.total_chunks
        };

        // Per-file AES-256-GCM application-layer encryption
        let file_key = self.transport.derive_file_key(&to, file_id).await;
        let payload = match file_key {
            Some(ref key) => encrypt_chunk(key, chunk_data),
            None => chunk_data.to_vec(),
        };

        let mut binary_data = Vec::with_capacity(1 + file_id.len() + 8 + payload.len());
        let file_id_bytes = file_id.as_bytes();
        let file_id_len = file_id_bytes.len() as u8;
        binary_data.push(file_id_len);
        binary_data.extend_from_slice(file_id_bytes);
        binary_data.extend_from_slice(&chunk_index.to_be_bytes());
        binary_data.extend_from_slice(&total_chunks.to_be_bytes());
        binary_data.extend_from_slice(&payload);

        let p2p_packet = P2PPacket::new(
            P2PPacketType::FileChunk,
            self.my_node_id,
            false,
            binary_data,
        );
        self.transport
            .send_packet_dual_path(to, p2p_packet)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        debug!(
            "📦 Streamed chunk {}/{} directly ({} bytes)",
            chunk_index + 1,
            total_chunks,
            chunk_data.len()
        );
        Ok(())
    }

    /// Отправить FileTransferEnd
    async fn send_file_transfer_end(&self, to: &HashId, file_id: &str) -> Result<()> {
        let end_msg = crate::communication::FileChunkEnd {
            file_id: file_id.to_string(),
        };
        let end_data = serde_json::to_vec(&end_msg)?;
        let p2p_packet = P2PPacket::new(
            P2PPacketType::FileTransferEnd,
            self.my_node_id,
            false,
            end_data,
        );
        self.transport.send_packet_dual_path(*to, p2p_packet).await.map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }


    /// Начать приём файла
    pub async fn start_receiving(&self, from: HashId, start: crate::communication::FileChunkStart) -> Result<()> {
        info!("📥 Receiving file: {} ({} bytes, {} chunks)",
            start.filename, start.file_size, start.total_chunks);

        // Never trust a peer's self-reported size/chunk-count before
        // allocating anything on their say-so — see MAX_FILE_TRANSFER_SIZE.
        if start.file_size > MAX_FILE_TRANSFER_SIZE {
            return Err(anyhow::anyhow!(
                "rejected file transfer from {}: declared file_size {} exceeds max {} bytes",
                hex::encode(&from.0[..8]), start.file_size, MAX_FILE_TRANSFER_SIZE
            ));
        }
        // Mirrors the sender's own formula exactly (send_file_from_disk,
        // line ~288) — including file_size=0 legitimately giving 0 chunks.
        let expected_total_chunks =
            ((start.file_size as usize + FILE_TRANSFER_CHUNK_SIZE - 1) / FILE_TRANSFER_CHUNK_SIZE) as u32;
        if start.total_chunks != expected_total_chunks {
            return Err(anyhow::anyhow!(
                "rejected file transfer from {}: declared total_chunks {} inconsistent with file_size {} (expected {})",
                hex::encode(&from.0[..8]), start.total_chunks, start.file_size, expected_total_chunks
            ));
        }

        if !valid_file_id(&start.file_id) {
            return Err(anyhow::anyhow!("rejected file transfer from {}: unsafe file_id", hex::encode(&from.0[..8])));
        }

        {
            let mut incoming = self.incoming.lock().await;
            // брошенные приёмы (давно без чанков) убираем вместе с недокачанным файлом
            let stale: Vec<String> = incoming.iter()
                .filter(|(_, t)| t.last_activity.elapsed() >= INCOMING_STALE_AFTER)
                .map(|(k, _)| k.clone()).collect();
            for k in stale {
                if let Some(t) = incoming.remove(&k) { let _ = std::fs::remove_file(&t.temp_path); }
            }
            match incoming.get(&start.file_id) {
                // тот же номер от другого узла — не перезаписываем чужую передачу
                Some(t) if t.from_peer != from => {
                    return Err(anyhow::anyhow!("rejected file transfer from {}: file_id already used by another peer", hex::encode(&from.0[..8])));
                }
                // повтор начала от того же узла — это возобновление, лимиты не трогаем
                Some(_) => {}
                None => {
                    let snapshot: Vec<(HashId, std::time::Duration)> =
                        incoming.values().map(|t| (t.from_peer, t.last_activity.elapsed())).collect();
                    if !incoming_admissible(&snapshot, &from) {
                        return Err(anyhow::anyhow!("rejected file transfer from {}: too many transfers in progress", hex::encode(&from.0[..8])));
                    }
                }
            }
            // a repeated start from the same peer: identical details change nothing (received data is kept); different details
            // replace the transfer, and the old partial file is deleted rather than left behind
            if let Some(t) = incoming.get_mut(&start.file_id) {
                if t.filename == start.filename && t.file_size == start.file_size && t.total_chunks == start.total_chunks {
                    t.last_activity = std::time::Instant::now();
                    return Ok(());
                }
                let _ = std::fs::remove_file(&t.temp_path);
                incoming.remove(&start.file_id);
            }
        }

        {
            let mut done = self.completed_incoming.lock().await;
            match done.get(&start.file_id) {
                Some(owner) if *owner != from => {
                    return Err(anyhow::anyhow!("rejected file transfer from {}: file_id already used by another peer", hex::encode(&from.0[..8])));
                }
                _ => {
                    done.remove(&start.file_id);
                }
            }
        }

        let downloads_dir = crate::communication::files_dir("downloads");
        std::fs::create_dir_all(&downloads_dir)?;
        let local_name = storage_filename(&start.file_id, &start.filename);
        let final_path = downloads_dir.join(&local_name);
        let temp_path = downloads_dir.join(format!("{}.part", &local_name));

        {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&temp_path)?;
            file.set_len(start.file_size)?;
        }

        let transfer = IncomingTransfer {
            filename: start.filename,
            file_size: start.file_size,
            mime_type: start.mime_type,
            total_chunks: start.total_chunks,
            temp_path,
            final_path,
            received_chunks: vec![false; start.total_chunks as usize],
            received_count: 0,
            from_peer: from,
            last_activity: std::time::Instant::now(),
        };
        let file_id = start.file_id.clone();
        self.incoming.lock().await.insert(file_id.clone(), transfer);

        if let Some(chunks) = self.prestart_chunks.lock().await.remove(&file_id) {
            info!("📦 Replaying {} buffered pre-start chunks for {}", chunks.len(), file_id);
            for chunk in chunks.into_iter().filter(|c| c.from == from) {
                self.apply_incoming_chunk(&file_id, chunk.chunk_index, chunk.total_chunks, &chunk.data).await?;
            }
        }
        Ok(())
    }

    pub async fn handle_chunk(&self, from: HashId, data: Vec<u8>) -> Result<bool> {
        if data.len() < 9 {
            return Ok(false);
        }

        let file_id_len = data[0] as usize;
        if data.len() < 1 + file_id_len + 8 {
            return Ok(false);
        }

        let file_id = String::from_utf8_lossy(&data[1..1 + file_id_len]).to_string();
        if !valid_file_id(&file_id) {
            return Ok(false);
        }
        let chunk_index = u32::from_be_bytes([
            data[1 + file_id_len], data[2 + file_id_len], data[3 + file_id_len], data[4 + file_id_len]
        ]);
        let total_chunks = u32::from_be_bytes([
            data[5 + file_id_len], data[6 + file_id_len], data[7 + file_id_len], data[8 + file_id_len]
        ]);
        let chunk_data = &data[9 + file_id_len..];

        {
            let incoming = self.incoming.lock().await;
            if !incoming.contains_key(&file_id) {
                drop(incoming);
                let mut prestart = self.prestart_chunks.lock().await;
                if !prestart.contains_key(&file_id) && prestart.len() >= MAX_PRESTART_FILES {
                    return Ok(false); // ждущих начала файлов слишком много
                }
                // chunks that wait for a transfer to start: expired ones go, one chunk is at most a chunk long (plus the
                        // encryption overhead), and the total is bounded
                prestart.values_mut().for_each(|v| v.retain(|c| c.at.elapsed() < PRESTART_TTL));
                prestart.retain(|_, v| !v.is_empty());
                let held: usize = prestart.values().flat_map(|v| v.iter()).map(|c| c.data.len()).sum();
                if chunk_data.len() > FILE_TRANSFER_CHUNK_SIZE + 64 || held + chunk_data.len() > MAX_PRESTART_BYTES {
                    return Ok(false);
                }
                let entry = prestart.entry(file_id.clone()).or_default();
                if entry.len() < MAX_PRESTART_CHUNKS_PER_FILE {
                    entry.push(BufferedChunk {
                        at: std::time::Instant::now(),
                        from,
                        chunk_index,
                        total_chunks,
                        data: chunk_data.to_vec(),
                    });
                    debug!("📥 Buffered pre-start chunk {}/{} for {}", chunk_index + 1, total_chunks, file_id);
                } else {
                    warn!("⚠️ Pre-start buffer full for {}, dropping chunk {}", file_id, chunk_index);
                }
                return Ok(false);
            }
        }

        // чанки принимаются только от того, кто начал эту передачу
        {
            let incoming = self.incoming.lock().await;
            if incoming.get(&file_id).map(|t| t.from_peer) != Some(from) {
                return Ok(false);
            }
        }
        self.apply_incoming_chunk(&file_id, chunk_index, total_chunks, chunk_data).await
    }

    async fn apply_incoming_chunk(
        &self,
        file_id: &str,
        chunk_index: u32,
        total_chunks: u32,
        chunk_data: &[u8],
    ) -> Result<bool> {
        // Decrypt per-file AES-256-GCM if present (prefix byte 0x01)
        let decrypted_buf: Vec<u8>;
        let plain_data: &[u8] = if chunk_data.first() == Some(&0x01) && chunk_data.len() > 13 {
            let from_peer = {
                let incoming = self.incoming.lock().await;
                incoming.get(file_id).map(|t| t.from_peer)
            };
            let peer = from_peer.ok_or_else(|| anyhow::anyhow!("No transfer record for {}", file_id))?;
            let key = self.transport.derive_file_key(&peer, file_id).await
                .ok_or_else(|| anyhow::anyhow!("No file key available for {}", file_id))?;
            decrypted_buf = decrypt_chunk(&key, &chunk_data[1..])?;
            &decrypted_buf
        } else {
            chunk_data
        };

        let mut incoming = self.incoming.lock().await;
        let Some(transfer) = incoming.get_mut(file_id) else {
            return Ok(false);
        };

        if transfer.total_chunks != total_chunks {
            warn!(
                "⚠️ total_chunks mismatch for {}: start={} chunk={} — chunk dropped",
                file_id,
                transfer.total_chunks,
                total_chunks
            );
            return Ok(false);
        }

        let idx = chunk_index as usize;
        if idx >= transfer.received_chunks.len() {
            return Ok(false);
        }
        // every chunk has exactly the length its place in the file demands: an empty chunk must not count as received, and a long
        // one must not spill over into its neighbour
        let offset = (chunk_index as u64) * (FILE_TRANSFER_CHUNK_SIZE as u64);
        let expected_len = transfer.file_size.saturating_sub(offset).min(FILE_TRANSFER_CHUNK_SIZE as u64) as usize;
        if plain_data.len() != expected_len {
            warn!("⚠️ chunk {} of {} has {} bytes, expected {} — dropped", chunk_index, file_id, plain_data.len(), expected_len);
            return Ok(false);
        }

        let write_result = (|| -> std::io::Result<()> {
            use std::io::{Seek, SeekFrom, Write};

            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open(&transfer.temp_path)?;
            let offset = (chunk_index as u64) * (FILE_TRANSFER_CHUNK_SIZE as u64);
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(plain_data)?;
            Ok(())
        })();

        if let Err(e) = write_result {
            return Err(anyhow::anyhow!("Failed to write incoming chunk: {}", e));
        }

        transfer.last_activity = std::time::Instant::now();
        if !transfer.received_chunks[idx] {
            transfer.received_chunks[idx] = true;
            transfer.received_count += 1;
        }
        debug!("📦 Received chunk {}/{}", chunk_index + 1, transfer.total_chunks);
        Ok(transfer.received_count == transfer.total_chunks)
    }

    async fn send_missing_ranges(
        &self,
        to: HashId,
        file_id: &str,
        mut missing_ranges: Vec<crate::communication::FileChunkRange>,
    ) -> Result<()> {
        // a message must fit the wire format: the rest is reported in the next round
        missing_ranges.truncate(MAX_RANGES_PER_MESSAGE);
        let payload = serde_json::to_vec(&crate::communication::FileMissing {
            file_id: file_id.to_string(),
            missing_ranges,
        })?;
        let packet = P2PPacket::new(
            P2PPacketType::FileMissing,
            self.my_node_id,
            false,
            payload,
        );
        self.transport.send_packet_dual_path(to, packet).await.map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }

    async fn send_transfer_complete(&self, to: HashId, file_id: &str) -> Result<()> {
        let payload = serde_json::to_vec(&crate::communication::FileTransferComplete {
            file_id: file_id.to_string(),
        })?;
        let packet = P2PPacket::new(
            P2PPacketType::FileComplete,
            self.my_node_id,
            false,
            payload,
        );
        self.transport.send_packet_dual_path(to, packet).await.map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }

    pub async fn handle_missing(
        &self,
        from: HashId,
        file_id: &str,
        missing_ranges: Vec<crate::communication::FileChunkRange>,
    ) -> Result<()> {
        let mut outgoing_disk = self.outgoing_disk.lock().await;
        if let Some(transfer) = outgoing_disk.get_mut(file_id).filter(|t| t.to_peer == from) {
            transfer.pending_missing = Some(chunk_ranges_to_indices(&missing_ranges, transfer.total_chunks));
            transfer.response_version += 1;
            let _ = transfer.response_tx.send(transfer.response_version);
        }
        Ok(())
    }

    pub async fn handle_transfer_complete(&self, from: HashId, file_id: &str) -> Result<()> {
        let mut outgoing_disk = self.outgoing_disk.lock().await;
        if let Some(transfer) = outgoing_disk.get_mut(file_id).filter(|t| t.to_peer == from) {
            transfer.remote_completed = true;
            transfer.pending_missing = None;
            transfer.response_version += 1;
            let _ = transfer.response_tx.send(transfer.response_version);
        }
        Ok(())
    }

    pub async fn handle_transfer_end(&self, from: HashId, file_id: &str) -> Result<()> {
        let maybe_transfer = {
            let incoming = self.incoming.lock().await;
            incoming.get(file_id).map(|transfer| {
                (
                    transfer.from_peer,
                    transfer.received_count == transfer.total_chunks,
                    collect_missing_ranges(&transfer.received_chunks),
                    transfer.filename.clone(),
                    transfer.mime_type.clone(),
                    transfer.temp_path.clone(),
                    transfer.final_path.clone(),
                    transfer.file_size,
                )
            })
        };

        let Some((owner, is_complete, missing_ranges, filename, mime_type, temp_path, final_path, file_size)) = maybe_transfer else {
            let already_completed = self.completed_incoming.lock().await.get(file_id) == Some(&from);
            if already_completed {
                info!("🔁 Re-sending FileComplete for already finalized transfer {}", file_id);
                self.send_transfer_complete(from, file_id).await?;
                return Ok(());
            }
            return Err(anyhow::anyhow!("Incoming transfer not found"));
        };
        // only the peer that is sending this file may end it, learn what is missing, or have it finalised
        if owner != from {
            return Err(anyhow::anyhow!("transfer end from a peer that does not own the transfer"));
        }

        if is_complete {
            let actual_size = std::fs::metadata(&temp_path)?.len();
            if actual_size != file_size {
                return Err(anyhow::anyhow!(
                    "Incoming file size mismatch for {}: expected {}, got {}",
                    file_id,
                    file_size,
                    actual_size
                ));
            }

            std::fs::rename(&temp_path, &final_path)?;
            info!(
                "✅ File received: {} ({} bytes, mime={})",
                filename,
                actual_size,
                mime_type
            );
            {
                let mut done = self.completed_incoming.lock().await;
                if done.len() >= MAX_COMPLETED_RECORDS {
                    done.clear(); // old records only serve to answer a repeated end of an already finished transfer
                }
                done.insert(file_id.to_string(), from);
            }
            self.send_transfer_complete(from, file_id).await?;
            self.incoming.lock().await.remove(file_id);
        } else {
            info!(
                "📭 Missing {} ranges for {} after transfer pass",
                missing_ranges.len(),
                file_id
            );
            self.send_missing_ranges(from, file_id, missing_ranges).await?;
        }

        Ok(())
    }

    async fn wait_for_transfer_feedback(&self, file_id: &str, timeout_ms: u64) -> Result<Option<Vec<u32>>> {
        let mut waited_ms = 0;
        loop {
            let (remote_completed, pending_missing, mut response_rx) = {
                let outgoing_disk = self.outgoing_disk.lock().await;
                let transfer = outgoing_disk
                    .get(file_id)
                    .ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;
                (
                    transfer.remote_completed,
                    transfer.pending_missing.clone(),
                    transfer.response_tx.subscribe(),
                )
            };

            if remote_completed {
                return Ok(Some(Vec::new()));
            }

            if let Some(missing) = pending_missing {
                let mut outgoing_disk = self.outgoing_disk.lock().await;
                if let Some(transfer) = outgoing_disk.get_mut(file_id) {
                    transfer.pending_missing = None;
                }
                return Ok(Some(missing));
            }

            if waited_ms >= timeout_ms {
                return Ok(None);
            }

            let slice = std::cmp::min(ACK_WAIT_SLICE_MS, timeout_ms - waited_ms);
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(slice),
                response_rx.changed()
            ).await;
            waited_ms += slice;
        }
    }

    /// Завершить потоковую отправку: послать end-of-pass, дождаться missing list или complete,
    /// при необходимости дослать недостающие chunk'и с диска и повторить цикл.
    pub async fn finalize_streaming_transfer(&self, to: HashId, file_id: &str) -> Result<()> {
        for round in 0..=MAX_RETRY_ROUNDS {
            self.send_file_transfer_end(&to, file_id).await?;
            let feedback = self.wait_for_transfer_feedback(file_id, ACK_ROUND_TIMEOUT_MS).await?;
            let Some(missing_chunks) = feedback else {
                continue;
            };

            if missing_chunks.is_empty() {
                self.outgoing_disk.lock().await.remove(file_id);
                return Ok(());
            }

            warn!(
                "⚠️ Retrying {} missing chunks for {} (round {}/{})",
                missing_chunks.len(),
                file_id,
                round + 1,
                MAX_RETRY_ROUNDS
            );

            for chunk_index in missing_chunks {
                self.send_single_chunk_from_disk(&to, file_id, chunk_index).await?;
                tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
            }
        }

        self.outgoing_disk.lock().await.remove(file_id);
        Err(anyhow::anyhow!(
            "Streaming file transfer incomplete for {} after {} retry rounds",
            file_id,
            MAX_RETRY_ROUNDS
        ))
    }

}

#[cfg(test)]
mod hostile_peer_tests {
    #[test]
    fn windows_device_names_are_not_used_as_file_names() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_filename("com1"), "_com1");
        assert_eq!(sanitize_filename("console.txt"), "console.txt");
    }

    #[test]
    fn two_different_id_and_name_pairs_cannot_share_one_stored_path() {
        // ("x", "a__b") and ("x__a", "b") would both be stored as "x__a__b"; the second id is not accepted at all
        assert!(valid_file_id("x"));
        assert!(!valid_file_id("x__a"));
        assert!(valid_file_id("0123456789abcdef_0011223344556677"), "the ids this program makes are fine");
    }

    #[test]
    fn many_overlapping_ranges_cost_no_more_than_the_file_has_chunks() {
        use crate::communication::FileChunkRange as R;
        let many: Vec<R> = (0..1000).map(|_| R { start: 0, end: 300_000 }).collect();
        let t = std::time::Instant::now();
        let out = chunk_ranges_to_indices(&many, 300_001);
        assert_eq!(out.len(), 300_001);
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
        // touching ranges merge, gaps stay
        let out = chunk_ranges_to_indices(&[R { start: 5, end: 6 }, R { start: 7, end: 8 }, R { start: 20, end: 21 }, R { start: 1, end: 2 }], 100);
        assert_eq!(out, vec![1, 2, 5, 6, 7, 8, 20, 21]);
    }

    #[test]
    fn missing_ranges_from_a_peer_are_clamped_deduplicated_and_capped() {
        use crate::communication::FileChunkRange as R;
        // a range of four billion chunks on a 10-chunk file expands to at most the 10 real chunks
        assert_eq!(chunk_ranges_to_indices(&[R { start: 0, end: u32::MAX }], 10), (0..10).collect::<Vec<u32>>());
        // reversed and out-of-file ranges are ignored, overlaps are not repeated
        assert_eq!(chunk_ranges_to_indices(&[R { start: 5, end: 2 }, R { start: 50, end: 60 }, R { start: 1, end: 3 }, R { start: 2, end: 4 }], 10), vec![1, 2, 3, 4]);
        // endless ranges do not allocate more than the file has
        let many: Vec<R> = (0..100_000).map(|_| R { start: 0, end: u32::MAX }).collect();
        assert_eq!(chunk_ranges_to_indices(&many, 10).len(), 10);
    }

    use super::*;

    #[test]
    fn file_id_cannot_escape_the_downloads_folder() {
        for bad in ["", "../x", "..", "a/b", "a\\b", "x\0y", "тест", &"a".repeat(65), "a b", "a.b"] {
            assert!(!valid_file_id(bad), "должен быть отвергнут: {bad:?}");
        }
        for ok in ["abc", "A-b_9", &"a".repeat(64), "550e8400-e29b-41d4-a716-446655440000"] {
            assert!(valid_file_id(ok), "{ok}");
        }
        // и имя файла на диске остаётся одним компонентом пути
        let name = storage_filename("abc-1", "../../etc/passwd");
        assert!(!name.contains('/') && !name.contains('\\') && !name.starts_with('.'));
    }

    #[test]
    fn incoming_limits_per_peer_total_and_stale() {
        let a = HashId([1; 32]);
        let fresh = std::time::Duration::from_secs(1);
        let mut v: Vec<(HashId, std::time::Duration)> = (0..MAX_INCOMING_PER_PEER).map(|_| (a, fresh)).collect();
        assert!(!incoming_admissible(&v, &a), "лимит на одного собеседника");
        assert!(incoming_admissible(&v, &HashId([2; 32])), "другой не страдает");
        // брошенные приёмы не занимают место
        for x in v.iter_mut() { x.1 = INCOMING_STALE_AFTER; }
        assert!(incoming_admissible(&v, &a));
        // общий потолок
        let many: Vec<_> = (0..MAX_INCOMING_TOTAL).map(|i| (HashId([(i % 250) as u8 + 3; 32]), fresh)).collect();
        assert!(!incoming_admissible(&many, &HashId([9; 32])), "общий потолок");
    }

    #[test]
    fn receiving_code_checks_file_id_owner_and_limits() {
        // сторож источника: эти проверки нельзя молча убрать
        let s = std::fs::read_to_string("src/communication/file_transfer.rs").unwrap();
        for needle in ["valid_file_id(&start.file_id)", "incoming_admissible(&snapshot", "t.from_peer != from", "!= Some(from)", "MAX_PRESTART_FILES"] {
            assert!(s.contains(needle), "пропала проверка: {needle}");
        }
    }
}
