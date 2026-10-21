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

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
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

/// Наибольший размер сообщения (JSON) для одного пакета: полезная нагрузка пакета ограничена u16, остаток — на шифрование и заголовки.
const MAX_WIRE_MESSAGE: usize = 60_000;

fn fits_one_packet(msg: &ChatMessage) -> bool {
    serde_json::to_vec(msg).map_or(false, |b| b.len() <= MAX_WIRE_MESSAGE)
}

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

    /// Закрытая среда: чаты не читаются и не пишутся, пока владелец не введёт пароль ([`ChatManager::unlock`]).
    pub fn new_locked(my_node_id: HashId, transport: Arc<P2PTransport>) -> Result<Self> {
        let storage = ChatStorage::new_locked(my_node_id)?;
        Self::new_inner(my_node_id, transport, storage)
    }

    /// Выдать ключ закрытой среды и поднять очередь неподтверждённых сообщений с диска.
    pub async fn unlock(&self, master_key: [u8; 32]) {
        self.storage.set_master(master_key);
        let restored = Self::restore_outbox(&self.storage);
        let mut pending = self.pending_acks.lock().await;
        for (id, p) in restored {
            pending.entry(id).or_insert(p);
        }
    }

    /// Отозвать ключ. Очередь сначала записывается (пока ключ есть), затем ключ и расшифрованные копии в памяти стираются.
    pub async fn lock(&self) {
        self.persist_outbox_if_dirty().await;
        self.storage.lock();
        self.pending_acks.lock().await.clear();
        self.outbox_dirty.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Перенос чатов и очереди на ключ закрытой среды (см. [`ChatStorage::rekey_to`]); возвращает число файлов с ошибкой.
    pub fn rekey_to(&self, old: Option<[u8; 32]>, new: [u8; 32]) -> Result<usize> {
        self.storage.rekey_to(old, new)
    }

    /// Среда стёрта: зашифрованные файлы чатов (история, очередь, журналы статусов) без ключа бесполезны — удаляются.
    pub fn erase_encrypted(&self) {
        self.storage.erase_encrypted();
    }

    pub fn is_locked(&self) -> bool {
        self.storage.is_locked()
    }

    /// messages that were still waiting for their recipients when the node last stopped
    fn restore_outbox(storage: &ChatStorage) -> HashMap<HashId, PendingAck> {
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
        restored
    }

    fn new_inner(
        my_node_id: HashId,
        transport: Arc<P2PTransport>,
        storage: ChatStorage,
    ) -> Result<Self> {
        let (incoming_tx, _incoming_rx) = mpsc::unbounded_channel();

        let restored = Self::restore_outbox(&storage);
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
            let mut last_prune = Instant::now() - Duration::from_secs(24 * 3600);
            loop {
                interval.tick().await;
                self.sweep_expired_acks().await;
                // заглушки удалённых пользователей, отложенные при запертой среде
                if !self.storage.is_locked() {
                    self.apply_pending_tombstones();
                }
                // история одного чата — максимум год: раз в сутки (и при старте) удаляем более старое
                if last_prune.elapsed() >= Duration::from_secs(24 * 3600) && !self.storage.is_locked() {
                    last_prune = Instant::now();
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    if let Ok(n) = self.storage.prune_expired(now_ms) {
                        if n > 0 {
                            println!("💬 История: удалено сообщений старше года: {n}");
                        }
                    }
                }
            }
        });
    }

    /// One round of the outgoing queue: messages whose time has come are sent again; messages older than `MAX_DELIVERY_AGE`
    /// are marked `Failed`. (Split out so it can be called directly, e.g. from tests.)
    async fn sweep_expired_acks(&self) {
        if self.storage.is_locked() {
            return; // при запертой среде очередь не отправляется (она и так пуста после lock)
        }
        let now = Instant::now();
        // поколение запираний — до снятия снимка: lock/unlock во время отправки не оживляет снимок прошлого сеанса
        let generation = self.storage.lock_generation();
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
        self.send_due(due, generation).await;
        self.persist_outbox_if_dirty().await;
    }

    /// Снять неподтверждённые исходящие с очереди повторов и из долговечного outbox: [peer] / [msg] — сузить выбор (оба None — всё).
    async fn cancel_pending(&self, peer: Option<&HashId>, msg: Option<&HashId>) {
        let removed = {
            let mut pending = self.pending_acks.lock().await;
            let before = pending.len();
            pending.retain(|id, p| !(peer.is_none_or(|x| p.peer == *x) && msg.is_none_or(|m| id == m)));
            before != pending.len()
        };
        if removed {
            self.outbox_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            self.persist_outbox_if_dirty().await;
        }
    }

    /// Отправить снимок «пора повторить». Перед каждой отправкой проверяется замок среды: снимок взят на открытой среде,
    /// а запирание, случившееся во время отправки предыдущих, не должно выпускать оставшиеся копии. Возвращает число отправок.
    async fn send_due(&self, due: Vec<(HashId, HashId, ChatMessage)>, generation: u64) -> usize {
        let mut sent = 0usize;
        for (msg_id, peer, msg) in due {
            if self.storage.is_locked() || self.storage.lock_generation() != generation {
                break;
            }
            // сообщение могли удалить, пока шла предыдущая отправка: актуальность проверяется под очередью перед каждой отправкой
            if !self.pending_acks.lock().await.contains_key(&msg_id) {
                continue;
            }
            sent += 1;
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
        sent
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

    /// Запечатать содержимое на ключ личности собеседника. Нет проверенного ключа — отказ (сообщение остаётся в очереди), открытого пути нет.
    async fn seal_for(&self, to: HashId, plain: &[u8]) -> Result<Vec<u8>> {
        let Some(pk) = self.transport.peer_signing_key(&to).await else {
            return Err(anyhow::anyhow!("ключ собеседника ещё не получен: сообщение ждёт соединения"));
        };
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk).map_err(|_| anyhow::anyhow!("недопустимый ключ собеседника"))?;
        crate::crypto::sealed::seal(&self.transport.sealing_key(), &vk, &self.my_node_id.0, &to.0, now_ms(), plain)
    }

    /// Открыть присланное содержимое: конверт обязан быть от `from` и адресован нам; прежний открытый формат не принимается.
    async fn open_from(&self, from: HashId, data: &[u8]) -> Result<Vec<u8>> {
        let Some(pk) = self.transport.peer_signing_key(&from).await else {
            return Err(anyhow::anyhow!("ключ отправителя неизвестен"));
        };
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk).map_err(|_| anyhow::anyhow!("недопустимый ключ отправителя"))?;
        let (_, plain) = crate::crypto::sealed::open(&self.transport.sealing_key(), &vk, &from.0, &self.my_node_id.0, now_ms(), data)?;
        Ok(plain)
    }

    /// One attempt to put a message on the wire.
    async fn send_once(&self, msg: &ChatMessage) -> Result<()> {
        let msg_data = self.seal_for(msg.to, &serde_json::to_vec(msg)?).await?;
        // содержимое запечатано на ключ получателя (E2E), поверх него транспорт добавляет сеансовое шифрование
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

    /// Служебное сообщение (профиль и т.п.): одна попытка, без записи в историю и без очереди повторов.
    pub async fn send_service(&self, to: HashId, text: String) -> Result<()> {
        let msg = ChatMessage::new(self.my_node_id, to, text);
        self.send_once(&msg).await
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
        self.send_message_full(to, text, attachment, None).await
    }

    /// Отправить сообщение с вложением и/или ответом-цитатой
    pub async fn send_message_full(
        &self,
        to: HashId,
        text: String,
        attachment: Option<crate::communication::FileAttachment>,
        reply_to: Option<crate::communication::ReplyRef>,
    ) -> Result<ChatMessage> {
        info!("📤 Sending chat message to {}", hex::encode(&to.0[..8]));

        // политика содержимого: ссылки не передаются, вложения только из белого списка
        crate::content_policy::check_text(&text).map_err(|e| anyhow::anyhow!(e))?;
        if let Some(att) = attachment.as_ref() {
            crate::content_policy::attachment_allowed(&att.filename).map_err(|e| anyhow::anyhow!(e))?;
        }

        // 1. Создать сообщение
        let mut msg = ChatMessage::new(self.my_node_id, to, text.clone());
        msg.status = MessageStatus::Shipping;
        msg.reply_to = reply_to.and_then(|r| r.sanitized());

        // Добавить attachment если есть
        if let Some(att) = attachment {
            info!("📎 With attachment: {} ({} bytes)", att.filename, att.size);
            msg.attachment = Some(att);
        }

        // заведомо не уйдёт одним пакетом (длина полезной нагрузки пакета — u16): отказ до сохранения и постановки в очередь, иначе
        // запись застряла бы в outbox и при каждой отправке ломала бы фоновый цикл повторов
        if !fits_one_packet(&msg) {
            return Err(anyhow::anyhow!("сообщение слишком длинное для отправки собеседнику (не более {MAX_WIRE_MESSAGE} байт)"));
        }

        if self.pending_acks.lock().await.len() >= MAX_OUTBOX {
            return Err(anyhow::anyhow!(
                "too many messages are waiting for delivery"
            ));
        }

        // 2. Сохранить у себя (outgoing)
        self.storage.save_outgoing(&to, &msg)?;

        // Ожидание подтверждения заводится ДО первой отправки: быстрый ACK, пришедший, пока отправка ещё в сети, находит запись и
        // снимает её, а не теряется (иначе запись появилась бы потом и сообщение отправлялось бы повторно, а статус Read
        // затирался бы на Shipping).
        {
            let mut pending = self.pending_acks.lock().await;
            // среду могли запереть, пока отправка ждала очередь: возвращать в неё открытый текст нельзя
            // (проверка под замком очереди: lock() сначала отзывает ключ, потом чистит очередь)
            if self.storage.is_locked() {
                return Err(anyhow::anyhow!("закрытая среда заперта: сообщение не поставлено в очередь"));
            }
            pending.insert(
                msg.msg_id,
                PendingAck {
                    peer: to,
                    sent_at: Instant::now(),
                    first_sent: unix_now(),
                    attempts: 0,
                    next_try: Instant::now() + retry_delay(1),
                    msg: msg.clone(),
                },
            );
        }
        self.outbox_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);

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
        // запись могла быть снята подтверждением, пришедшим во время отправки: тогда статус уже Read, и его не понижаем
        let still_waiting = {
            let mut pending = self.pending_acks.lock().await;
            match pending.get_mut(&msg.msg_id) {
                Some(p) => {
                    p.attempts = attempts;
                    p.next_try = Instant::now() + retry_delay(attempts.max(1));
                    p.msg.status = msg.status.clone();
                    true
                }
                None => false,
            }
        };
        if still_waiting {
            self.outbox_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
            // 7. Обновить статус в файле
            self.storage
                .update_message_status(&to, &msg.msg_id, msg.status.clone())?;
        } else if self.storage.is_locked() {
            // среду заперли, пока шла отправка: очередь очищена замком, историю обновить нечем
            return Err(anyhow::anyhow!("закрытая среда заперта: сообщение не поставлено в очередь"));
        } else {
            msg.status = MessageStatus::Read;
        }

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
        msg.reply_to = msg.reply_to.take().and_then(|r| r.sanitized());
        // старый или чужой клиент мог прислать ссылку или запрещённое вложение: у нас они не показываются
        msg.text = crate::content_policy::strip_links(&msg.text);
        if let Some(att) = msg.attachment.as_ref() {
            if crate::content_policy::attachment_allowed(&att.filename).is_err() {
                msg.attachment = None;
                if msg.text.trim().is_empty() {
                    msg.text = "[вложение не принято: такой тип файла в YANDI не передаётся]".into();
                }
            }
        }

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
        if let Err(e) = self.storage.save_incoming(&from, &msg) {
            // не сохранили (например, закрытая среда заперта): без подтверждения отправитель повторит, а повтор не должен считаться дублем
            let mut seen = self.seen_incoming.lock().await;
            seen.0.remove(&msg.msg_id);
            seen.1.retain(|id| *id != msg.msg_id);
            return Err(e);
        }

        info!("✅ Message saved from {}", hex::encode(&from.0[..8]));

        // профиль контакта (ник, аватар): разбираем на узле; телефоны получат то же сообщение сами
        for reply in crate::profile_sync::on_incoming(&from, &msg.text) {
            let _ = self.send_service(from, reply).await;
        }

        // смайл-реакция собеседника на сообщение: запоминаем, в переписке её не показываем
        crate::chat_reactions::on_incoming(&from, &msg.text);

        // 7. Уведомить Web UI (через канал)
        let _ = self.incoming_tx.send(msg.clone());
        crate::mobile_api::publish(&msg);

        // 6. Подтверждение доставки (ACK) — ПОСЛЕ локальных действий: номер уже считается виденным, и повторная доставка (ветка
        // дубля) их не повторит; сетевой сбой ACK не должен лишать телефоны события, а реакцию/профиль — применения.
        self.send_ack(from, msg.msg_id).await?;

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
                let opened = match self.open_from(from, &packet.data).await {
                    Ok(plain) => Ok(plain),
                    Err(e) => Err(e),
                };
                let handled = match opened {
                    Ok(plain) => self.handle_incoming_message(from, plain).await,
                    Err(e) => Err(e),
                };
                match handled {
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
            CommControlPacket::ChatDeleteMessage => {
                if let Err(e) = self.handle_remote_delete(from, &packet.data) {
                    debug!("🗑️ delete request from {} ignored: {}", hex::encode(&from.0[..8]), e);
                }
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

    /// Сохранить в историю сообщение, пришедшее не по сети узлов (с телефона владельца, см. `mobile_self`): его покажет страница чата.
    pub fn store_local_incoming(&self, from: &HashId, msg: &ChatMessage) -> Result<()> {
        self.storage.save_incoming(from, msg)
    }

    /// Сохранить в историю сообщение, отправленное со страницы телефону владельца.
    pub fn store_local_outgoing(&self, to: &HashId, msg: &ChatMessage) -> Result<()> {
        self.storage.save_outgoing(to, msg)
    }

    /// Поднять статус сообщения (квитанция телефона владельца): только вверх — «доставлено» не затирает «прочитано».
    pub fn store_local_status(&self, peer_id: &HashId, msg_id: &HashId, status: MessageStatus) -> Result<()> {
        fn rank(s: &MessageStatus) -> u8 {
            match s {
                MessageStatus::Failed => 0,
                MessageStatus::Pending | MessageStatus::Shipping => 1,
                MessageStatus::Delivered => 2,
                MessageStatus::Read => 3,
            }
        }
        let cur = self.storage.load_history(peer_id, usize::MAX)?.into_iter().find(|m| m.msg_id == *msg_id).map(|m| m.status);
        match cur {
            Some(c) if rank(&c) < rank(&status) => self.storage.update_message_status(peer_id, msg_id, status),
            _ => Ok(()),
        }
    }

    /// Загрузить историю чата
    pub fn load_history(&self, peer_id: &HashId, limit: usize) -> Result<Vec<ChatMessage>> {
        self.storage.load_history(peer_id, limit)
    }

    /// Удалённый пользователь: его сообщения в чате с ПК становятся заглушкой. Среда заперта — номер ждёт в отложенном
    /// списке (без секретов) и применяется, как только среда откроется (см. `spawn_delivery_timeout_task`).
    pub async fn tombstone_peer(&self, peer_id: &HashId) {
        self.cancel_pending(Some(peer_id), None).await;
        crate::chat_reactions::forget_peer(&hex::encode(&peer_id.0[..8]));
        match self.storage.tombstone_author(peer_id) {
            Ok(_) => {}
            Err(_) => pending_tombstones_add(peer_id),
        }
    }

    fn apply_pending_tombstones(&self) {
        let pending = pending_tombstones();
        if pending.is_empty() {
            return;
        }
        let mut left = Vec::new();
        for p in pending {
            if self.storage.tombstone_author(&p).is_err() {
                left.push(p);
            }
        }
        pending_tombstones_save(&left);
    }

    /// Очистить историю чата
    pub async fn clear_history(&self, peer_id: &HashId) -> Result<()> {
        self.cancel_pending(Some(peer_id), None).await; // неподтверждённые исходящие тоже снимаются с очереди повторов
        self.storage.clear_history(peer_id)?;
        // реакции этой переписки не должны пережить саму историю
        crate::chat_reactions::forget_peer(&hex::encode(&peer_id.0[..8]));
        Ok(())
    }

    /// Очистить ВСЮ историю
    pub async fn clear_all_history(&self) -> Result<()> {
        self.cancel_pending(None, None).await;
        self.storage.clear_all()?;
        crate::chat_reactions::forget_all();
        Ok(())
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
    pub async fn delete_message_local(&self, peer_id: &HashId, msg_id: &HashId) -> Result<()> {
        info!(
            "🗑️ Deleting message {} locally for peer {}",
            hex::encode(&msg_id.0[..8]),
            hex::encode(&peer_id.0[..8])
        );
        self.cancel_pending(Some(peer_id), Some(msg_id)).await;
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

        // 1. Снять с очереди повторов (иначе удалённое содержимое ушло бы адресату позже) и удалить локально
        self.cancel_pending(Some(peer_id), Some(msg_id)).await;
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

    /// «Удалить для всех» от собеседника: стирается только сообщение, которое написал сам этот собеседник (`from` — проверенный
    /// узел-отправитель пакета), и только в переписке с ним.
    fn handle_remote_delete(&self, from: HashId, data: &[u8]) -> Result<()> {
        #[derive(serde::Deserialize)]
        struct Req {
            msg_id: String,
            #[serde(default)]
            delete_for_everyone: bool,
        }
        let req: Req = serde_json::from_slice(data)?;
        if !req.delete_for_everyone {
            return Err(anyhow::anyhow!("not a delete-for-everyone request"));
        }
        let msg_id = HashId::from_hex(&req.msg_id).map_err(|_| anyhow::anyhow!("bad msg_id"))?;
        let authored = self
            .storage
            .load_history(&from, usize::MAX)?
            .iter()
            .any(|m| m.msg_id == msg_id && m.from == from);
        if !authored {
            return Err(anyhow::anyhow!("no such message written by this peer"));
        }
        self.storage.delete_message(&from, &msg_id)
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
    #[test]
    fn a_message_too_long_for_one_packet_is_refused_before_it_is_queued() {
        let id = HashId([1u8; 32]);
        let short = ChatMessage::new(id, id, "привет".into());
        assert!(fits_one_packet(&short));
        let long = ChatMessage::new(id, id, "x".repeat(65_536));
        assert!(!fits_one_packet(&long));
    }

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
            reply_to: None,
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
    async fn test_chat_manager() -> (ChatManager, tokio::sync::MutexGuard<'static, ()>) {
        // порты берутся из переменных окружения процесса: проверки, которым нужен транспорт, идут строго по одной
        static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let guard = SERIAL.lock().await;
        // у каждой проверки свои порты: транспорт прежней проверки может ещё не освободить сокеты
        static NEXT_PORT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(19401);
        let base = NEXT_PORT.fetch_add(2, std::sync::atomic::Ordering::SeqCst);
        std::env::set_var("YANDI_P2P_DISCOVERY_PORT", base.to_string());
        std::env::set_var("YANDI_P2P_DATA_PORT", (base + 1).to_string());
        let identity = NodeIdentity::new();
        let my_node_id = identity.node_id();
        let transport = P2PTransport::new(identity, 0)
            .await
            .expect("start transport");
        // чаты проверки — во временной папке, не в ~/.yandi/chats владельца
        let dir = std::env::temp_dir().join(format!("yandi-chat-test-{}", std::process::id()));
        let storage = ChatStorage::in_dir(my_node_id, dir).expect("storage");
        (ChatManager::new_inner(my_node_id, transport, storage).expect("create ChatManager"), guard)
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
    async fn a_deleted_or_cleared_message_leaves_the_retry_queue_and_the_outbox() {
        let (cm, _serial) = test_chat_manager().await;
        let mk = |peer: HashId, text: &str| {
            let m = ChatMessage::new(cm.my_node_id, peer, text.to_string());
            (m.msg_id, m)
        };
        let (pa, pb) = (crate::util::HashId::new_random(), crate::util::HashId::new_random());
        let (a1, m_a1) = mk(pa, "a1");
        let (a2, m_a2) = mk(pa, "a2");
        let (b1, m_b1) = mk(pb, "b1");
        for (peer, id, m) in [(pa, a1, &m_a1), (pa, a2, &m_a2), (pb, b1, &m_b1)] {
            cm.storage.save_outgoing(&peer, m).unwrap();
            cm.pending_acks.lock().await.insert(id, pending(peer, m.clone(), Duration::from_secs(5), Duration::from_secs(3600)));
        }
        // удалили одно сообщение «у себя»: оно уходит из очереди, остальные остаются
        cm.delete_message_local(&pa, &a1).await.unwrap();
        {
            let q = cm.pending_acks.lock().await;
            assert!(!q.contains_key(&a1) && q.contains_key(&a2) && q.contains_key(&b1));
        }
        // очистили переписку с pa: её сообщения уходят из очереди, чужие остаются
        cm.clear_history(&pa).await.unwrap();
        {
            let q = cm.pending_acks.lock().await;
            assert!(!q.contains_key(&a2) && q.contains_key(&b1));
        }
        // снимок «пора повторить», взятый до удаления, не должен отправиться
        assert_eq!(cm.send_due(vec![(a2, pa, m_a2.clone())], cm.storage.lock_generation()).await, 0);
        cm.clear_all_history().await.unwrap();
        assert!(cm.pending_acks.lock().await.is_empty());
    }

    #[tokio::test]
    async fn the_queue_fails_only_day_old_messages_and_keeps_young_ones_for_resending() {
        let (cm, _serial) = test_chat_manager().await;

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

        // «удалить для всех»: чужое сообщение (написанное нами) собеседник стереть не может, своё — может
        let ours = ChatMessage::new(cm.my_node_id, from, "наше".to_string());
        cm.storage.save_outgoing(&from, &ours).unwrap();
        let del = |id: &HashId| crate::communication::CommPacket {
            packet_type: crate::communication::CommControlPacket::ChatDeleteMessage,
            data: serde_json::to_vec(&serde_json::json!({"msg_id": hex::encode(id.0), "delete_for_everyone": true})).unwrap(),
        };
        cm.handle_comm_packet(from, del(&ours.msg_id)).await.unwrap();
        assert_eq!(cm.storage.load_history(&from, 10).unwrap().len(), 2, "чужое сообщение не стёрто");
        cm.handle_comm_packet(from, del(&incoming.msg_id)).await.unwrap();
        let left = cm.storage.load_history(&from, 10).unwrap();
        assert_eq!(left.len(), 1, "своё сообщение собеседника стёрто по его просьбе");
        assert_eq!(left[0].msg_id, ours.msg_id);
    }

    /// Закрытая среда: пока заперто, входящее не принимается и не считается «уже виденным»; после разблокировки повтор сохраняется.
    #[tokio::test(flavor = "multi_thread")]
    async fn locked_chat_does_not_swallow_incoming_and_keeps_outbox() {
        let (cm, _serial) = test_chat_manager().await;
        cm.storage.set_master([9; 32]);
        let peer = crate::util::HashId::new_random();
        let queued = ChatMessage::new(cm.my_node_id, peer, "в очереди".to_string());
        cm.storage
            .save_outbox(&[super::super::storage::OutboxEntry { peer, msg: queued.clone(), first_sent: unix_now() }])
            .unwrap();
        cm.lock().await;
        assert!(cm.is_locked());

        let from = crate::util::HashId::new_random();
        let incoming = ChatMessage::new(from, cm.my_node_id, "пока заперто".to_string());
        let bytes = serde_json::to_vec(&incoming).unwrap();
        assert!(cm.handle_incoming_message(from, bytes.clone()).await.is_err());
        assert!(cm.storage.save_outbox(&[]).is_err(), "пустая очередь не стирает файл у запертого хранилища");

        // отправка, прервавшая lock, могла оставить в очереди открытый текст: запертая очередь не отправляется
        let stray = ChatMessage::new(cm.my_node_id, peer, "секрет".to_string());
        cm.pending_acks.lock().await.insert(stray.msg_id, pending(peer, stray.clone(), Duration::from_secs(5), Duration::ZERO));
        cm.sweep_expired_acks().await;
        assert_eq!(cm.pending_acks.lock().await[&stray.msg_id].attempts, 1, "запертая очередь не отправляется");
        cm.pending_acks.lock().await.clear();
        // снимок, взятый до lock, после него не отправляется
        assert_eq!(cm.send_due(vec![(stray.msg_id, peer, stray.clone())], cm.storage.lock_generation()).await, 0, "после lock копии из снимка не уходят");

        cm.unlock([9; 32]).await;
        assert_eq!(cm.pending_acks.lock().await.len(), 1, "очередь поднята с диска после разблокировки");
        // снимок прошлого сеанса не оживает после полного цикла lock/unlock
        let old_session = cm.storage.lock_generation();
        cm.lock().await;
        cm.unlock([9; 32]).await;
        assert_eq!(cm.send_due(vec![(stray.msg_id, peer, stray.clone())], old_session).await, 0, "снимок до lock/unlock не отправляется");
        let _ = cm.handle_incoming_message(from, bytes).await;
        assert_eq!(cm.storage.load_history(&from, 10).unwrap().len(), 1, "повтор после отказа сохранён, а не принят за дубль");
    }

    #[test]
    fn resend_pauses_grow_and_stop_at_a_minute() {
        let d: Vec<u64> = (1..=9).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(d, vec![3, 6, 12, 24, 48, 60, 60, 60, 60]);
    }

    /// E2E v1: содержимое чата запечатывается на ключ собеседника; чужой, подмена отправителя и прежний открытый вид отвергаются.
    #[tokio::test]
    async fn chat_content_is_sealed_end_to_end_and_plaintext_is_refused() {
        let (cm, _g) = test_chat_manager().await;
        let peer_key = ed25519_dalek::SigningKey::from_bytes(&[77u8; 32]);
        let peer = HashId([77u8; 32]);
        let other = HashId([78u8; 32]);
        // ключа собеседника ещё нет: отправка отказывает, открытого пути нет
        assert!(cm.seal_for(peer, b"x").await.is_err());
        assert!(cm.open_from(peer, b"{\"text\":\"hi\"}").await.is_err());
        cm.transport.test_add_peer_key(peer, peer_key.verifying_key().to_bytes()).await;
        // исходящее: запечатано, не содержит текста, открывается ключом собеседника только для него
        let env = cm.seal_for(peer, "секрет".as_bytes()).await.unwrap();
        assert!(!env.windows("секрет".len()).any(|w| w == "секрет".as_bytes()));
        let me_vk = cm.transport.sealing_key().verifying_key();
        let (_, back) = crate::crypto::sealed::open(&peer_key, &me_vk, &cm.my_node_id.0, &peer.0, now_ms(), &env).unwrap();
        assert_eq!(back, "секрет".as_bytes());
        // входящее: от собеседника открывается, прежний открытый JSON — нет, чужой отправитель — нет
        let to_me = crate::crypto::sealed::seal(&peer_key, &me_vk, &peer.0, &cm.my_node_id.0, now_ms(), b"hello").unwrap();
        assert_eq!(cm.open_from(peer, &to_me).await.unwrap(), b"hello");
        assert!(cm.open_from(peer, b"{\"text\":\"hi\"}").await.is_err(), "открытый вид принят");
        cm.transport.test_add_peer_key(other, ed25519_dalek::SigningKey::from_bytes(&[78u8; 32]).verifying_key().to_bytes()).await;
        assert!(cm.open_from(other, &to_me).await.is_err(), "конверт, подписанный другим, принят от третьего");
    }

    /// E2E для сигнализации звонков: принимается только запечатанный подписанный сигнал с верным типом; открытый, чужой и с подменённым типом — нет.
    #[tokio::test]
    async fn call_signals_are_accepted_only_sealed_signed_and_with_the_right_type() {
        use crate::p2p::P2PPacketType as T;
        let (cm, _g) = test_chat_manager().await;
        let peer_key = ed25519_dalek::SigningKey::from_bytes(&[91u8; 32]);
        let peer = HashId([91u8; 32]);
        let me_vk = cm.transport.sealing_key().verifying_key();
        let make = |ty: T, body: &[u8]| {
            let mut plain = vec![ty.to_byte()];
            plain.extend_from_slice(body);
            crate::crypto::sealed::seal(&peer_key, &me_vk, &peer.0, &cm.my_node_id.0, now_ms(), &plain).unwrap()
        };
        let offer = make(T::VoiceData, br#"{"sdp":"v=0 a=fingerprint:sha-256 AA"}"#);
        // ключа собеседника нет — отвергается
        assert!(cm.transport.open_signal(peer, T::VoiceData, &offer).await.is_none());
        cm.transport.test_add_peer_key(peer, peer_key.verifying_key().to_bytes()).await;
        assert_eq!(cm.transport.open_signal(peer, T::VoiceData, &offer).await.unwrap(), br#"{"sdp":"v=0 a=fingerprint:sha-256 AA"}"#);
        // подмена внешнего типа (конверт от VoiceData выдан за запрос на видео) и открытый текст
        assert!(cm.transport.open_signal(peer, T::VideoCallRequest, &offer).await.is_none());
        assert!(cm.transport.open_signal(peer, T::VoiceData, br#"{"sdp":"x"}"#).await.is_none());
        // конверт другого отправителя, выданный за пакет от peer
        let evil = ed25519_dalek::SigningKey::from_bytes(&[92u8; 32]);
        let mut plain = vec![T::VoiceData.to_byte()];
        plain.extend_from_slice(b"{}");
        let forged = crate::crypto::sealed::seal(&evil, &me_vk, &peer.0, &cm.my_node_id.0, now_ms(), &plain).unwrap();
        assert!(cm.transport.open_signal(peer, T::VoiceData, &forged).await.is_none());
    }

    /// Какие пакеты идут только запечатанными; служебные пакеты самого транспорта и куски файла (свой ключ) — нет.
    #[test]
    fn the_sealed_packet_set_covers_control_traffic_only() {
        use crate::p2p::P2PPacketType as T;
        use crate::p2p::transport::must_seal;
        for t in [T::ChatAck, T::ChatRead, T::ChatTyping, T::ChatDeleteMessage, T::VoiceCallRequest, T::VoiceData, T::VideoCallReject, T::VideoData, T::FileTransferStart, T::FileTransferEnd, T::FileTransferCancel, T::FileMissing, T::FileComplete] {
            assert!(must_seal(t), "{t:?} должен идти запечатанным");
        }
        for t in [T::ChatMessage, T::FileChunk, T::PunchReq, T::PunchIntro, T::RelayReq, T::RelayGrant, T::PathChallenge, T::PathResponse] {
            assert!(!must_seal(t), "{t:?} не должен запечатываться этим слоем");
        }
    }

    /// P26-1: тот же конверт сигнала во второй раз не принимается, а просроченный по короткому окну отвергается.
    #[tokio::test]
    async fn a_replayed_or_stale_call_signal_is_refused() {
        use crate::p2p::P2PPacketType as T;
        let (cm, _g) = test_chat_manager().await;
        let peer_key = ed25519_dalek::SigningKey::from_bytes(&[93u8; 32]);
        let peer = HashId([93u8; 32]);
        cm.transport.test_add_peer_key(peer, peer_key.verifying_key().to_bytes()).await;
        let me_vk = cm.transport.sealing_key().verifying_key();
        let make = |at: u64| {
            let mut plain = vec![T::VoiceCallRequest.to_byte()];
            plain.extend_from_slice(br#"{"call_id":"abc"}"#);
            crate::crypto::sealed::seal(&peer_key, &me_vk, &peer.0, &cm.my_node_id.0, at, &plain).unwrap()
        };
        let env = make(now_ms());
        assert!(cm.transport.open_signal(peer, T::VoiceCallRequest, &env).await.is_some());
        assert!(cm.transport.open_signal(peer, T::VoiceCallRequest, &env).await.is_none(), "повтор принят");
        let old = make(now_ms() - crate::p2p::transport::SIGNAL_WINDOW_MS - 1_000);
        assert!(cm.transport.open_signal(peer, T::VoiceCallRequest, &old).await.is_none(), "просроченный сигнал принят");
    }

    /// P26-2: поток запечатанных пакетов от одного собеседника упирается в бюджет дорогих проверок.
    #[test]
    fn expensive_opens_per_peer_are_budgeted() {
        let mut g = crate::p2p::transport::SealGuard::default();
        let peer = HashId([5u8; 32]);
        let t0 = std::time::Instant::now();
        let allowed = (0..1000).filter(|_| g.allow(peer, t0)).count();
        assert!(allowed <= 200, "пропущено {allowed} подряд");
        assert!(g.allow(HashId([6u8; 32]), t0), "другой собеседник не должен страдать");
        assert!(g.allow(peer, t0 + std::time::Duration::from_secs(1)), "бюджет восстанавливается");
    }
}

/// Отложенные заглушки: номера удалённых устройств (не секрет), ждущие открытия закрытой среды.
fn pending_tombstones_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("tombstones_pending.json")
}

fn pending_tombstones() -> Vec<HashId> {
    std::fs::read_to_string(pending_tombstones_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|h| hex::decode(h).ok().and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()).map(HashId))
        .collect()
}

fn pending_tombstones_save(l: &[HashId]) {
    let p = pending_tombstones_path();
    if l.is_empty() {
        let _ = std::fs::remove_file(p);
        return;
    }
    let v: Vec<String> = l.iter().map(|h| hex::encode(h.0)).collect();
    let _ = crate::util::private_file::write_private(&p, serde_json::to_string(&v).unwrap_or_default().as_bytes());
}

fn pending_tombstones_add(peer: &HashId) {
    let mut l = pending_tombstones();
    if !l.contains(peer) {
        l.push(*peer);
    }
    pending_tombstones_save(&l);
}
