// src/communication/chat.rs
//! Chat manager for P2P text messaging

use crate::communication::{
    ChatMessage, ChatStorage, CommControlPacket, CommPacket, MessageStatus,
};
use crate::p2p::{P2PPacket, P2PPacketType, P2PTransport};
use crate::util::HashId;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info};

/// An outgoing message waiting for the recipient's ChatAck.
/// Delivery underneath (dual-path UDP) has no retransmission of its own: a packet can be lost, or the session keys of the two sides can
/// be out of step for a while (a new contact, a restarted node). So the message is kept here and SENT AGAIN, with growing pauses, until
/// the recipient confirms it — delivery "at least once"; the receiver ignores repeats (same `msg_id`). It is given up on only after
/// `MAX_DELIVERY_AGE`. The queue is also written to disk, so a restart does not lose it. See `spawn_delivery_timeout_task`.
struct PendingAck {
    peer: HashId,
    sent_at: Instant,
    /// unix seconds of the first send (survives a restart; `sent_at` does not)
    first_sent: u64,
    attempts: u32,
    next_try: Instant,
    msg: ChatMessage,
}

/// Pause before the n-th resend: 3, 6, 12, 24, 48 s, then every 60 s.
fn retry_delay(attempts: u32) -> Duration {
    Duration::from_secs((3u64 << attempts.saturating_sub(1).min(5)).min(60))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Менеджер чата
pub struct ChatManager {
    my_node_id: HashId,
    storage: ChatStorage,
    transport: Arc<P2PTransport>,
    /// Очередь входящих сообщений (для Web UI)
    incoming_tx: mpsc::UnboundedSender<ChatMessage>,
    /// File Transfer Manager (опционально)
    file_transfer_manager: Option<std::sync::Arc<super::FileTransferManager>>,
    /// Messages sent but not yet ACKed by the peer — see `PendingAck`.
    pending_acks: Arc<Mutex<HashMap<HashId, PendingAck>>>,
    /// the queue changed since it was last written to disk
    outbox_dirty: Arc<std::sync::atomic::AtomicBool>,
    /// ids of recent incoming messages (a resend of one we already have is acknowledged again but not stored twice)
    seen_incoming: Arc<
        Mutex<(
            std::collections::HashSet<HashId>,
            std::collections::VecDeque<HashId>,
        )>,
    >,
}

/// How long a message is retried before it is marked Failed.
const MAX_DELIVERY_AGE: Duration = Duration::from_secs(24 * 3600);
/// Most messages waiting in the outgoing queue.
const MAX_OUTBOX: usize = 2000;
/// Incoming message ids remembered to recognise repeats.
const SEEN_INCOMING_MAX: usize = 20_000;

impl ChatManager {
    /// Создать новый ChatManager
    pub fn new(my_node_id: HashId, transport: Arc<P2PTransport>) -> Result<Self> {
        let storage = ChatStorage::new(my_node_id)?;
        Self::new_inner(my_node_id, transport, storage)
    }

    /// Создать ChatManager с мастер-ключом (HKDF-derived chat encryption)
    pub fn new_with_master_key(
        my_node_id: HashId,
        transport: Arc<P2PTransport>,
        master_key: [u8; 32],
    ) -> Result<Self> {
        let storage = ChatStorage::new_with_key(my_node_id, master_key)?;
        Self::new_inner(my_node_id, transport, storage)
    }

    fn new_inner(
        my_node_id: HashId,
        transport: Arc<P2PTransport>,
        storage: ChatStorage,
    ) -> Result<Self> {
        let (incoming_tx, _incoming_rx) = mpsc::unbounded_channel();

        // messages that were still waiting for their recipients when the node last stopped
        let mut restored = HashMap::new();
        for e in storage.load_outbox() {
            let age = unix_now().saturating_sub(e.first_sent);
            if age < MAX_DELIVERY_AGE.as_secs() {
                restored.insert(
                    e.msg.msg_id,
                    PendingAck {
                        peer: e.peer,
                        sent_at: Instant::now()
                            .checked_sub(Duration::from_secs(age))
                            .unwrap_or_else(Instant::now),
                        first_sent: e.first_sent,
                        attempts: 0,
                        next_try: Instant::now() + Duration::from_secs(5),
                        msg: e.msg,
                    },
                );
            }
        }
        if !restored.is_empty() {
            info!(
                "📮 {} unconfirmed messages restored from the outgoing queue",
                restored.len()
            );
        }
        Ok(Self {
            my_node_id,
            storage,
            transport,
            incoming_tx,
            file_transfer_manager: None,
            pending_acks: Arc::new(Mutex::new(restored)),
            outbox_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            seen_incoming: Arc::new(Mutex::new((Default::default(), Default::default()))),
        })
    }

    /// Background task: messages left in `Shipping` past `CHAT_ACK_TIMEOUT`
    /// with no ChatAck are marked `Failed` so the sender actually finds out
    /// delivery didn't happen, instead of the message sitting silently
    /// unconfirmed forever. Mirrors Station's own `spawn_cleanup_task`.
    pub fn spawn_delivery_timeout_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                self.sweep_expired_acks().await;
            }
        });
    }

    /// One round of the outgoing queue: messages whose time has come are sent again; messages older than `MAX_DELIVERY_AGE`
    /// are marked `Failed`. (Split out so it can be called directly, e.g. from tests.)
    async fn sweep_expired_acks(&self) {
        let now = Instant::now();
        let mut due: Vec<(HashId, HashId, ChatMessage)> = Vec::new();
        let mut gave_up: Vec<(HashId, HashId)> = Vec::new();
        {
            let mut pending = self.pending_acks.lock().await;
            for (msg_id, p) in pending.iter_mut() {
                if now.duration_since(p.sent_at) > MAX_DELIVERY_AGE {
                    gave_up.push((*msg_id, p.peer));
                } else if now >= p.next_try {
                    // reserve the slot so that a slow send is not started twice
                    p.attempts += 1;
                    p.next_try = now + retry_delay(p.attempts);
                    due.push((*msg_id, p.peer, p.msg.clone()));
                }
            }
            for (msg_id, _) in &gave_up {
                pending.remove(msg_id);
            }
        }
        for (msg_id, peer) in &gave_up {
            if let Err(e) = self
                .storage
                .update_message_status(peer, msg_id, MessageStatus::Failed)
            {
                error!(
                    "❌ Failed to mark message {} as Failed: {}",
                    hex::encode(&msg_id.0[..8]),
                    e
                );
            } else {
                error!(
                    "⏰ Message {} to {} not delivered within {:?} — marked Failed",
                    hex::encode(&msg_id.0[..8]),
                    hex::encode(&peer.0[..8]),
                    MAX_DELIVERY_AGE
                );
            }
        }
        if !gave_up.is_empty() {
            self.outbox_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        for (msg_id, peer, msg) in due {
            match self.send_once(&msg).await {
                Ok(()) => debug!(
                    "🔁 resent message {} to {}",
                    hex::encode(&msg_id.0[..8]),
                    hex::encode(&peer.0[..8])
                ),
                Err(e) => debug!(
                    "🔁 resend of {} to {} failed (will retry): {}",
                    hex::encode(&msg_id.0[..8]),
                    hex::encode(&peer.0[..8]),
                    e
                ),
            }
            // the keys of the two sides may be out of step: renegotiate (rate-limited inside)
            self.transport.request_resync(peer).await;
            // and the way to the peer may be closed (a NAT that has forgotten us, a new outside address): ask for an introduction (rate-limited inside)
            self.transport.request_punch(peer).await;
        }
        self.persist_outbox_if_dirty().await;
    }

    /// Write the outgoing queue to disk if it changed.
    async fn persist_outbox_if_dirty(&self) {
        if !self
            .outbox_dirty
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let entries: Vec<super::storage::OutboxEntry> = self
            .pending_acks
            .lock()
            .await
            .values()
            .map(|p| super::storage::OutboxEntry {
                peer: p.peer,
                msg: p.msg.clone(),
                first_sent: p.first_sent,
            })
            .collect();
        if let Err(e) = self.storage.save_outbox(&entries) {
            error!("❌ could not write the outgoing queue: {}", e);
            self.outbox_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// One attempt to put a message on the wire.
    async fn send_once(&self, msg: &ChatMessage) -> Result<()> {
        let msg_data = serde_json::to_vec(msg)?;
        // P2PTransport applies the authenticated PFS session envelope to the
        // whole packet. Do not add the unavailable legacy E2E stub here.
        let p2p_packet =
            P2PPacket::new(P2PPacketType::ChatMessage, self.my_node_id, false, msg_data);
        self.transport
            .send_packet_dual_path(msg.to, p2p_packet)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Установить File Transfer Manager
    pub fn set_file_transfer_manager(
        &mut self,
        manager: std::sync::Arc<super::FileTransferManager>,
    ) {
        self.file_transfer_manager = Some(manager);
    }

    /// Отправить текстовое сообщение
    pub async fn send_message(&self, to: HashId, text: String) -> Result<ChatMessage> {
        self.send_message_with_attachment(to, text, None).await
    }

    /// Отправить сообщение с вложением
    pub async fn send_message_with_attachment(
        &self,
        to: HashId,
        text: String,
        attachment: Option<crate::communication::FileAttachment>,
    ) -> Result<ChatMessage> {
        info!("📤 Sending chat message to {}", hex::encode(&to.0[..8]));

        // 1. Создать сообщение
        let mut msg = ChatMessage::new(self.my_node_id, to, text.clone());
        msg.status = MessageStatus::Shipping;

        // Добавить attachment если есть
        if let Some(att) = attachment {
            info!("📎 With attachment: {} ({} bytes)", att.filename, att.size);
            msg.attachment = Some(att);
        }

        if self.pending_acks.lock().await.len() >= MAX_OUTBOX {
            return Err(anyhow::anyhow!(
                "too many messages are waiting for delivery"
            ));
        }

        // 2. Сохранить у себя (outgoing)
        self.storage.save_outgoing(&to, &msg)?;

        // 3-6. Отправить; что не ушло или не подтверждено — остаётся в очереди и отправляется снова (см. PendingAck)
        let first_try = self.send_once(&msg).await;
        let attempts = if first_try.is_ok() { 1 } else { 0 };
        match &first_try {
            Ok(()) => {
                msg.status = MessageStatus::Shipping;
                info!("✅ Message sent to {}", hex::encode(&to.0[..8]));
            }
            Err(e) => {
                msg.status = MessageStatus::Pending;
                info!(
                    "📮 Message to {} queued, will be delivered when the connection is ready: {}",
                    hex::encode(&to.0[..8]),
                    e
                );
                self.transport.request_punch(to).await; // no way to the peer yet: ask a mutual acquaintance to introduce us
            }
        }
        self.pending_acks.lock().await.insert(
            msg.msg_id,
            PendingAck {
                peer: to,
                sent_at: Instant::now(),
                first_sent: unix_now(),
                attempts,
                next_try: Instant::now() + retry_delay(attempts.max(1)),
                msg: msg.clone(),
            },
        );
        self.outbox_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);

        // 7. Обновить статус в файле
        self.storage
            .update_message_status(&to, &msg.msg_id, msg.status.clone())?;

        Ok(msg)
    }

    /// Обработать входящее сообщение
    pub async fn handle_incoming_message(
        &self,
        from: HashId,
        data: Vec<u8>,
    ) -> Result<ChatMessage> {
        debug!(
            "📨 Received chat message from {}",
            hex::encode(&from.0[..8])
        );

        // 1. Расшифровать
        // 2. Десериализовать
        let mut msg: ChatMessage = serde_json::from_slice(&data)?;

        // 3. Проверить: нам ли, от того ли, чьим ключом расшифровано, и в пределах размеров
        if let Err(why) = check_incoming(&msg, from, self.my_node_id) {
            error!(
                "❌ Chat message from {} refused: {}",
                hex::encode(&from.0[..8]),
                why
            );
            return Err(anyhow::anyhow!("Message refused: {}", why));
        }

        // A resend of a message we already have (our confirmation was lost): confirm again, store and show it only once.
        let first_time = {
            let mut seen = self.seen_incoming.lock().await;
            if seen.0.contains(&msg.msg_id) {
                false
            } else {
                seen.0.insert(msg.msg_id);
                seen.1.push_back(msg.msg_id);
                if seen.1.len() > SEEN_INCOMING_MAX {
                    if let Some(old) = seen.1.pop_front() {
                        seen.0.remove(&old);
                    }
                }
                true
            }
        };
        if !first_time {
            debug!(
                "♻️ repeat of message {} from {} — confirmed again, not stored twice",
                hex::encode(&msg.msg_id.0[..8]),
                hex::encode(&from.0[..8])
            );
            self.send_ack(from, msg.msg_id).await?;
            return Ok(msg);
        }

        // 4. Обновить статус
        msg.status = MessageStatus::Delivered;

        // 5. Сохранить у себя (incoming)
        self.storage.save_incoming(&from, &msg)?;

        info!("✅ Message saved from {}", hex::encode(&from.0[..8]));

        // 6. Отправить подтверждение доставки (ACK)
        self.send_ack(from, msg.msg_id).await?;

        // 7. Уведомить Web UI (через канал)
        let _ = self.incoming_tx.send(msg.clone());
        crate::mobile_api::publish(&msg);

        Ok(msg)
    }

    /// Отправить подтверждение получения
    async fn send_ack(&self, to: HashId, msg_id: HashId) -> Result<()> {
        let ack_data = serde_json::to_vec(&msg_id)?;

        // Упаковать в P2PPacket
        let p2p_packet = P2PPacket::new(
            P2PPacketType::ChatAck,
            self.my_node_id, // sender
            false,
            ack_data,
        );

        match self.transport.send_packet_dual_path(to, p2p_packet).await {
            Ok(_) => {}
            Err(e) => {
                error!("❌ Failed to send ACK: {}", e);
                return Err(anyhow::anyhow!("Failed to send ACK: {}", e));
            }
        }

        Ok(())
    }

    /// Обработать ACK подтверждение
    pub async fn handle_ack(&self, from: HashId, data: Vec<u8>) -> Result<()> {
        let msg_id: HashId = serde_json::from_slice(&data)?;

        debug!(
            "📬 Received ACK for message {} from {}",
            hex::encode(&msg_id.0[..8]),
            hex::encode(&from.0[..8])
        );

        // An acknowledgement makes the node decrypt and rewrite a whole history, so it must be worth it: for a message we are still
        // waiting on, only the peer it was sent to may confirm it; for anything else (e.g. after a restart) one per peer every 2 s.
        let owner = self.pending_acks.lock().await.get(&msg_id).map(|p| p.peer);
        match owner {
            Some(o) if o != from => return Ok(()),
            Some(_) => {}
            None => {
                if !ack_gate(&from) {
                    return Ok(());
                }
            }
        }

        // Обновить статус: Read
        self.storage
            .update_message_status(&from, &msg_id, MessageStatus::Read)?;

        // Confirmed delivered — no longer at risk of a timeout marking it Failed.
        // Only the peer the message was sent to can confirm it (someone else knowing the id must not).
        {
            let mut pending = self.pending_acks.lock().await;
            if pending.get(&msg_id).map_or(false, |p| p.peer == from) {
                pending.remove(&msg_id);
                self.outbox_dirty
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }

        Ok(())
    }

    /// Обработать CommPacket из transport
    pub async fn handle_comm_packet(&self, from: HashId, packet: CommPacket) -> Result<()> {
        println!(
            "[CHAT] 🔔 handle_comm_packet ENTRY, packet_type={:?}",
            packet.packet_type
        );
        match packet.packet_type {
            CommControlPacket::ChatMessage => {
                info!("💬 ChatMessage from {}", hex::encode(&from.0[..8]));

                // Расшифровать и обработать сообщение
                match self.handle_incoming_message(from, packet.data).await {
                    Ok(msg) => {
                        info!("✅ Chat message processed: {} bytes", msg.text.len());

                        // Отправить ACK
                        let ack_data = serde_json::to_vec(&msg.msg_id)?;
                        let ack_packet = P2PPacket::new(
                            P2PPacketType::ChatAck,
                            self.my_node_id, // sender
                            false,
                            ack_data,
                        );

                        if let Err(e) = self.transport.send_packet_dual_path(from, ack_packet).await
                        {
                            error!("❌ Failed to send ACK: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("❌ Failed to handle incoming message: {}", e);
                    }
                }
            }
            CommControlPacket::ChatAck => {
                info!("✅ ChatAck from {}", hex::encode(&from.0[..8]));
                // Обновить статус сообщения как Read
                if let Err(e) = self.handle_ack(from, packet.data).await {
                    error!("❌ Failed to handle ACK: {}", e);
                }
            }
            CommControlPacket::ChatRead => {
                info!("👁 ChatRead from {}", hex::encode(&from.0[..8]));
                // TODO: Обработать read receipt
            }
            CommControlPacket::ChatTyping => {
                debug!("⌨️  ChatTyping from {}", hex::encode(&from.0[..8]));
                // TODO: Показать индикатор "печатает..."
            }
            CommControlPacket::FileTransferStart => {
                info!("🚀 FileTransferStart from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    if let Ok(start_msg) =
                        serde_json::from_slice::<super::FileChunkStart>(&packet.data)
                    {
                        if let Err(e) = ftm.start_receiving(from, start_msg).await {
                            error!("❌ Failed to start receiving file: {}", e);
                        }
                    }
                } else {
                    debug!("⚠️  FileTransferManager not set");
                }
            }
            CommControlPacket::FileChunk => {
                info!("📦 FileChunk from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    // Данные уже бинарные, передаём напрямую
                    if let Err(e) = ftm.handle_chunk(from, packet.data).await {
                        error!("❌ Failed to handle chunk: {}", e);
                    }
                }
            }
            CommControlPacket::FileTransferEnd => {
                info!("🏁 FileTransferEnd from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    if let Ok(end_msg) = serde_json::from_slice::<super::FileChunkEnd>(&packet.data)
                    {
                        if let Err(e) = ftm.handle_transfer_end(from, &end_msg.file_id).await {
                            error!("❌ Failed to handle transfer end: {}", e);
                        }
                    }
                }
            }
            CommControlPacket::FileMissing => {
                debug!("📭 FileMissing from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    if let Ok(missing) = serde_json::from_slice::<super::FileMissing>(&packet.data)
                    {
                        if let Err(e) = ftm
                            .handle_missing(from, &missing.file_id, missing.missing_ranges)
                            .await
                        {
                            error!("❌ Failed to handle missing chunks: {}", e);
                        }
                    }
                }
            }
            CommControlPacket::FileComplete => {
                info!("✅ FileComplete from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    if let Ok(complete) =
                        serde_json::from_slice::<super::FileTransferComplete>(&packet.data)
                    {
                        if let Err(e) = ftm.handle_transfer_complete(from, &complete.file_id).await
                        {
                            error!("❌ Failed to handle file complete: {}", e);
                        }
                    }
                }
            }
            _ => {
                debug!(
                    "📨 Unknown CommPacket: {:?} from {}",
                    packet.packet_type,
                    hex::encode(&from.0[..8])
                );
            }
        }

        Ok(())
    }

    /// Загрузить историю чата
    pub fn load_history(&self, peer_id: &HashId, limit: usize) -> Result<Vec<ChatMessage>> {
        self.storage.load_history(peer_id, limit)
    }

    /// Очистить историю чата
    pub fn clear_history(&self, peer_id: &HashId) -> Result<()> {
        self.storage.clear_history(peer_id)
    }

    /// Очистить ВСЮ историю
    pub fn clear_all_history(&self) -> Result<()> {
        self.storage.clear_all()
    }

    /// Редактировать сообщение
    pub fn edit_message(&self, peer_id: &HashId, msg_id: &HashId, new_text: String) -> Result<()> {
        info!(
            "✏️ Editing message {} for peer {}",
            hex::encode(&msg_id.0[..8]),
            hex::encode(&peer_id.0[..8])
        );
        self.storage.update_message_text(peer_id, msg_id, new_text)
    }

    /// Удалить сообщение локально (только у себя)
    pub fn delete_message_local(&self, peer_id: &HashId, msg_id: &HashId) -> Result<()> {
        info!(
            "🗑️ Deleting message {} locally for peer {}",
            hex::encode(&msg_id.0[..8]),
            hex::encode(&peer_id.0[..8])
        );
        self.storage.delete_message(peer_id, msg_id)
    }

    /// Удалить сообщение для всех (отправить запрос на удаление)
    pub async fn delete_message_for_everyone(
        &self,
        peer_id: &HashId,
        msg_id: &HashId,
    ) -> Result<()> {
        info!(
            "🗑️ Deleting message {} for everyone with peer {}",
            hex::encode(&msg_id.0[..8]),
            hex::encode(&peer_id.0[..8])
        );

        // 1. Удалить локально
        self.storage.delete_message(peer_id, msg_id)?;

        // 2. Отправить запрос на удаление пиру
        use crate::communication::{CommControlPacket, CommPacket};

        let delete_data = serde_json::json!({
            "msg_id": hex::encode(&msg_id.0),
            "delete_for_everyone": true
        });

        let data_bytes = serde_json::to_vec(&delete_data)?;

        // Упаковать в P2PPacket
        let p2p_packet = P2PPacket::new(
            P2PPacketType::ChatDeleteMessage,
            self.my_node_id, // sender
            false,
            data_bytes,
        );

        // Отправить через P2P transport (Dual-Path!)
        self.transport
            .send_packet_dual_path(*peer_id, p2p_packet)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send delete request: {}", e))?;

        info!(
            "✅ Delete request sent to peer {}",
            hex::encode(&peer_id.0[..8])
        );
        Ok(())
    }

    /// Получить список всех чатов
    pub fn list_chats(&self) -> Result<Vec<HashId>> {
        self.storage.list_chats()
    }
}

// TODO: После тестов remove

/// At most one acknowledgement per peer every two seconds is allowed to touch the stored history when the message is not one we wait on.
fn ack_gate(from: &HashId) -> bool {
    use std::sync::{Mutex, OnceLock};
    static G: OnceLock<Mutex<HashMap<[u8; 32], std::time::Instant>>> = OnceLock::new();
    let mut m = G
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if m.len() > 2048 {
        m.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(2));
    }
    let ok = m
        .get(&from.0)
        .map_or(true, |t| t.elapsed() >= std::time::Duration::from_secs(2));
    if ok {
        m.insert(from.0, std::time::Instant::now());
    }
    ok
}

/// Most text (bytes) and most inline attachment data (base64 chars) accepted in one incoming chat message.
const MAX_INCOMING_TEXT: usize = 100_000;
const MAX_INCOMING_INLINE_DATA: usize = 6 * 1024 * 1024;

/// An incoming message must be addressed to us, claim to be from the peer whose key decrypted it, and stay in size.
fn check_incoming(
    msg: &ChatMessage,
    authenticated_sender: HashId,
    me: HashId,
) -> Result<(), &'static str> {
    if msg.to != me {
        return Err("not addressed to us");
    }
    if msg.from != authenticated_sender {
        return Err("sender does not match the key it was decrypted with");
    }
    if msg.text.len() > MAX_INCOMING_TEXT {
        return Err("text too long");
    }
    if msg
        .attachment
        .as_ref()
        .and_then(|a| a.data.as_ref())
        .map_or(false, |d| d.len() > MAX_INCOMING_INLINE_DATA)
    {
        return Err("inline attachment too large");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_msg(from: u8, to: u8, text: &str) -> ChatMessage {
        ChatMessage {
            msg_id: HashId([1; 32]),
            from: HashId([from; 32]),
            to: HashId([to; 32]),
            timestamp: 0,
            text: text.into(),
            encrypted: true,
            status: MessageStatus::Pending,
            edited: false,
            edit_timestamp: None,
            attachment: None,
        }
    }

    #[test]
    fn an_incoming_message_must_come_from_its_real_sender_and_stay_in_size() {
        let me = HashId([2; 32]);
        assert_eq!(
            check_incoming(&plain_msg(5, 2, "hi"), HashId([5; 32]), me),
            Ok(())
        );
        assert!(
            check_incoming(&plain_msg(6, 2, "hi"), HashId([5; 32]), me).is_err(),
            "peer 5 cannot speak as peer 6"
        );
        assert!(
            check_incoming(&plain_msg(5, 3, "hi"), HashId([5; 32]), me).is_err(),
            "addressed to someone else"
        );
        assert!(check_incoming(
            &plain_msg(5, 2, &"x".repeat(MAX_INCOMING_TEXT + 1)),
            HashId([5; 32]),
            me
        )
        .is_err());
    }
    use crate::core::NodeIdentity;
    use crate::p2p::P2PTransport;

    #[test]
    fn test_chat_storage() {
        // TODO: добавить тесты
    }

    /// p2p::P2PTransport's ports come from env vars, not constructor
    /// params (see with_handlers) — process-global state, so both
    /// delivery-timeout scenarios below share ONE ChatManager/transport
    /// instead of racing each other over the same env vars in parallel
    /// test threads.
    async fn test_chat_manager() -> ChatManager {
        std::env::set_var("YANDI_P2P_DISCOVERY_PORT", "19401");
        std::env::set_var("YANDI_P2P_DATA_PORT", "19402");
        let identity = NodeIdentity::new();
        let my_node_id = identity.node_id();
        let transport = P2PTransport::new(identity, 0)
            .await
            .expect("start transport");
        // чаты проверки — во временной папке, не в ~/.yandi/chats владельца
        let dir = std::env::temp_dir().join(format!("yandi-chat-test-{}", std::process::id()));
        let storage = ChatStorage::in_dir(my_node_id, dir).expect("storage");
        ChatManager::new_inner(my_node_id, transport, storage).expect("create ChatManager")
    }

    fn pending(peer: HashId, msg: ChatMessage, age: Duration, due_in: Duration) -> PendingAck {
        PendingAck {
            peer,
            sent_at: Instant::now().checked_sub(age).unwrap_or_else(Instant::now),
            first_sent: unix_now().saturating_sub(age.as_secs()),
            attempts: 1,
            next_try: Instant::now() + due_in,
            msg,
        }
    }

    /// The outgoing queue: a message nobody confirmed for a whole day is marked Failed; a confirmed one is left alone; a young
    /// unconfirmed one is NOT failed — it stays in the queue and is sent again (it used to be marked Failed after 45 seconds, and lost).
    #[tokio::test]
    async fn the_queue_fails_only_day_old_messages_and_keeps_young_ones_for_resending() {
        let cm = test_chat_manager().await;

        let old_peer = crate::util::HashId::new_random();
        let old_msg = ChatMessage::new(cm.my_node_id, old_peer, "hello?".to_string());
        cm.storage.save_outgoing(&old_peer, &old_msg).unwrap();
        cm.pending_acks.lock().await.insert(
            old_msg.msg_id,
            pending(
                old_peer,
                old_msg.clone(),
                MAX_DELIVERY_AGE + Duration::from_secs(1),
                Duration::from_secs(3600),
            ),
        );

        let young_peer = crate::util::HashId::new_random();
        let young_msg = ChatMessage::new(cm.my_node_id, young_peer, "still trying".to_string());
        cm.storage.save_outgoing(&young_peer, &young_msg).unwrap();
        cm.pending_acks.lock().await.insert(
            young_msg.msg_id,
            pending(
                young_peer,
                young_msg.clone(),
                Duration::from_secs(300),
                Duration::ZERO,
            ),
        );

        let acked_peer = crate::util::HashId::new_random();
        let acked_msg = ChatMessage::new(cm.my_node_id, acked_peer, "hi".to_string());
        cm.storage.save_outgoing(&acked_peer, &acked_msg).unwrap();
        cm.pending_acks.lock().await.insert(
            acked_msg.msg_id,
            pending(
                acked_peer,
                acked_msg.clone(),
                Duration::ZERO,
                Duration::from_secs(3600),
            ),
        );
        cm.handle_ack(acked_peer, serde_json::to_vec(&acked_msg.msg_id).unwrap())
            .await
            .unwrap();

        cm.sweep_expired_acks().await;

        let status = |peer: &HashId, id: &HashId| {
            cm.storage
                .load_history(peer, 10)
                .unwrap()
                .into_iter()
                .find(|m| m.msg_id == *id)
                .expect("message present")
                .status
        };
        assert_eq!(
            status(&old_peer, &old_msg.msg_id),
            MessageStatus::Failed,
            "a day-old unconfirmed message is given up on"
        );
        assert!(!cm.pending_acks.lock().await.contains_key(&old_msg.msg_id));
        assert_ne!(
            status(&young_peer, &young_msg.msg_id),
            MessageStatus::Failed,
            "a young message must not be failed"
        );
        let q = cm.pending_acks.lock().await;
        let y = q
            .get(&young_msg.msg_id)
            .expect("the young message stays in the queue");
        assert!(
            y.attempts >= 2 && y.next_try > Instant::now(),
            "it was tried again and its next try is later"
        );
        drop(q);
        assert_eq!(status(&acked_peer, &acked_msg.msg_id), MessageStatus::Read);
        assert!(!cm.pending_acks.lock().await.contains_key(&acked_msg.msg_id));

        // the queue is written to disk and read back (a restart does not lose it)
        let peer = crate::util::HashId::new_random();
        let msg = ChatMessage::new(cm.my_node_id, peer, "wait for me".to_string());
        cm.storage.save_outgoing(&peer, &msg).unwrap();
        cm.pending_acks.lock().await.insert(
            msg.msg_id,
            pending(
                peer,
                msg.clone(),
                Duration::from_secs(10),
                Duration::from_secs(3600),
            ),
        );
        cm.outbox_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        cm.persist_outbox_if_dirty().await;
        let back = cm.storage.load_outbox();
        assert_eq!(
            back.len(),
            2,
            "the young message from above and this one are both still waiting"
        );
        let mine = back
            .iter()
            .find(|e| e.msg.msg_id == msg.msg_id)
            .expect("the queued message was written");
        assert_eq!(mine.peer, peer);

        // a message arriving twice (the sender did not get our confirmation): one entry in the history
        let from = crate::util::HashId::new_random();
        let incoming = ChatMessage::new(from, cm.my_node_id, "once".to_string());
        let bytes = serde_json::to_vec(&incoming).unwrap();
        let _ = cm.handle_incoming_message(from, bytes.clone()).await;
        let _ = cm.handle_incoming_message(from, bytes).await;
        assert_eq!(
            cm.storage.load_history(&from, 10).unwrap().len(),
            1,
            "a resend is not stored twice"
        );
    }

    #[test]
    fn resend_pauses_grow_and_stop_at_a_minute() {
        let d: Vec<u64> = (1..=9).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(d, vec![3, 6, 12, 24, 48, 60, 60, 60, 60]);
    }
}
