// src/communication/chat.rs
//! Chat manager for P2P text messaging

use crate::communication::{
    ChatStorage, ChatMessage, MessageStatus, CommControlPacket, CommPacket,
    E2EEncryption,
};
use crate::util::HashId;
use crate::p2p::{P2PTransport, P2PPacket, P2PPacketType};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use anyhow::Result;
use tracing::{info, error, debug};

/// A message sitting in `Shipping`, waiting for the peer's ChatAck.
/// Delivery underneath (Station dual-path) is fire-and-forget with no
/// retransmission — if both copies of a wagon are lost, the train times
/// out silently and no ACK/NACK signals it. Without this, a message could
/// sit in `Shipping` forever with no visible failure. See
/// `spawn_delivery_timeout_task`.
struct PendingAck {
    peer: HashId,
    sent_at: Instant,
}

/// Менеджер чата
pub struct ChatManager {
    my_node_id: HashId,
    storage: ChatStorage,
    transport: Arc<P2PTransport>,
    e2e_encryption: Arc<E2EEncryption>,
    /// Очередь входящих сообщений (для Web UI)
    incoming_tx: mpsc::UnboundedSender<ChatMessage>,
    /// File Transfer Manager (опционально)
    file_transfer_manager: Option<std::sync::Arc<super::FileTransferManager>>,
    /// Messages sent but not yet ACKed by the peer — see `PendingAck`.
    pending_acks: Arc<Mutex<HashMap<HashId, PendingAck>>>,
}

/// How long to wait for a ChatAck before marking a message Failed.
/// Station's own train_timeout is 30s; this gives a margin for dual-path
/// completion + the ACK's own round trip before giving up.
const CHAT_ACK_TIMEOUT: Duration = Duration::from_secs(45);

impl ChatManager {
    /// Создать новый ChatManager
    pub fn new(
        my_node_id: HashId,
        transport: Arc<P2PTransport>,
    ) -> Result<Self> {
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
        let e2e_encryption = Arc::new(E2EEncryption::new());
        let (incoming_tx, _incoming_rx) = mpsc::unbounded_channel();

        Ok(Self {
            my_node_id,
            storage,
            transport,
            e2e_encryption,
            incoming_tx,
            file_transfer_manager: None,
            pending_acks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Background task: messages left in `Shipping` past `CHAT_ACK_TIMEOUT`
    /// with no ChatAck are marked `Failed` so the sender actually finds out
    /// delivery didn't happen, instead of the message sitting silently
    /// unconfirmed forever. Mirrors Station's own `spawn_cleanup_task`.
    pub fn spawn_delivery_timeout_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                self.sweep_expired_acks().await;
            }
        });
    }

    /// One sweep of `pending_acks`: anything older than `CHAT_ACK_TIMEOUT`
    /// gets marked `Failed` and dropped from tracking. Split out from
    /// `spawn_delivery_timeout_task` so it's directly callable (incl. from
    /// tests) without waiting on the real interval.
    async fn sweep_expired_acks(&self) {
        let now = Instant::now();
        let expired: Vec<(HashId, HashId)> = {
            let pending = self.pending_acks.lock().await;
            pending
                .iter()
                .filter(|(_, p)| now.duration_since(p.sent_at) > CHAT_ACK_TIMEOUT)
                .map(|(msg_id, p)| (*msg_id, p.peer))
                .collect()
        };
        if expired.is_empty() {
            return;
        }
        let mut pending = self.pending_acks.lock().await;
        for (msg_id, peer) in expired {
            pending.remove(&msg_id);
            if let Err(e) = self.storage.update_message_status(&peer, &msg_id, MessageStatus::Failed) {
                error!("❌ Failed to mark message {} as Failed: {}", hex::encode(&msg_id.0[..8]), e);
            } else {
                error!("⏰ Message {} to {} never ACKed within {:?} — marked Failed",
                    hex::encode(&msg_id.0[..8]), hex::encode(&peer.0[..8]), CHAT_ACK_TIMEOUT);
            }
        }
    }


    /// Установить File Transfer Manager
    pub fn set_file_transfer_manager(&mut self, manager: std::sync::Arc<super::FileTransferManager>) {
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

        // 2. Сохранить у себя (outgoing)
        self.storage.save_outgoing(&to, &msg)?;

        // 3. Подготовить данные для отправки
        let msg_data = serde_json::to_vec(&msg)?;

        // 4. Зашифровать (E2E)
        let encrypted = self.e2e_encryption.encrypt_for_peer(to, &msg_data).await?;

        // 5. Упаковать в P2PPacket (с sender ID!)
        let p2p_packet = P2PPacket::new(
            P2PPacketType::ChatMessage,
            self.my_node_id,  // sender = полный CID
            false,  // encrypted = false (E2E уже зашифрован)
            encrypted,  // payload = зашифрованные данные
        );

        // 6. Отправить через P2P transport (Dual-Path!)
        match self.transport.send_packet_dual_path(to, p2p_packet).await {
            Ok(_) => {
                msg.status = MessageStatus::Shipping;  // В процессе доставки
                info!("✅ Message sent to {}", hex::encode(&to.0[..8]));
                // Track until ChatAck arrives — see spawn_delivery_timeout_task.
                self.pending_acks.lock().await.insert(msg.msg_id, PendingAck { peer: to, sent_at: Instant::now() });
            }
            Err(e) => {
                msg.status = MessageStatus::Pending;
                error!("❌ Failed to send message: {}", e);
                // TODO: Сохранить в pending outbox
                return Err(anyhow::anyhow!("Failed to send message: {}", e));
            }
        }

        // 7. Обновить статус в файле
        self.storage.update_message_status(&to, &msg.msg_id, msg.status.clone())?;

        Ok(msg)
    }

    /// Обработать входящее сообщение
    pub async fn handle_incoming_message(&self, from: HashId, data: Vec<u8>) -> Result<ChatMessage> {
        debug!("📨 Received chat message from {}", hex::encode(&from.0[..8]));

        // 1. Расшифровать
        let decrypted = self.e2e_encryption.decrypt_from_peer(from, &data).await?;

        // 2. Десериализовать
        let mut msg: ChatMessage = serde_json::from_slice(&decrypted)?;

        // 3. Проверить: нам ли, от того ли, чьим ключом расшифровано, и в пределах размеров
        if let Err(why) = check_incoming(&msg, from, self.my_node_id) {
            error!("❌ Chat message from {} refused: {}", hex::encode(&from.0[..8]), why);
            return Err(anyhow::anyhow!("Message refused: {}", why));
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

        Ok(msg)
    }

    /// Отправить подтверждение получения
    async fn send_ack(&self, to: HashId, msg_id: HashId) -> Result<()> {
        let ack_data = serde_json::to_vec(&msg_id)?;

        // Упаковать в P2PPacket
        let p2p_packet = P2PPacket::new(
            P2PPacketType::ChatAck,
            self.my_node_id,  // sender
            false,
            ack_data,
        );

        match self.transport.send_packet_dual_path(to, p2p_packet).await {
            Ok(_) => {},
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

        debug!("📬 Received ACK for message {} from {}",
            hex::encode(&msg_id.0[..8]),
            hex::encode(&from.0[..8])
        );

        // Обновить статус: Read
        self.storage.update_message_status(&from, &msg_id, MessageStatus::Read)?;

        // Confirmed delivered — no longer at risk of a timeout marking it Failed.
        // Only the peer the message was sent to can confirm it (someone else knowing the id must not).
        {
            let mut pending = self.pending_acks.lock().await;
            if pending.get(&msg_id).map_or(false, |p| p.peer == from) {
                pending.remove(&msg_id);
            }
        }

        Ok(())
    }

    /// Обработать CommPacket из transport
    pub async fn handle_comm_packet(&self, from: HashId, packet: CommPacket) -> Result<()> {
        println!("[CHAT] 🔔 handle_comm_packet ENTRY, packet_type={:?}", packet.packet_type);
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
                            self.my_node_id,  // sender
                            false,
                            ack_data,
                        );

                        if let Err(e) = self.transport.send_packet_dual_path(from, ack_packet).await {
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
                    if let Ok(start_msg) = serde_json::from_slice::<super::FileChunkStart>(&packet.data) {
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
                    if let Ok(end_msg) = serde_json::from_slice::<super::FileChunkEnd>(&packet.data) {
                        if let Err(e) = ftm.handle_transfer_end(from, &end_msg.file_id).await {
                            error!("❌ Failed to handle transfer end: {}", e);
                        }
                    }
                }
            }
            CommControlPacket::FileMissing => {
                debug!("📭 FileMissing from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    if let Ok(missing) = serde_json::from_slice::<super::FileMissing>(&packet.data) {
                        if let Err(e) = ftm.handle_missing(from, &missing.file_id, missing.missing_ranges).await {
                            error!("❌ Failed to handle missing chunks: {}", e);
                        }
                    }
                }
            }
            CommControlPacket::FileComplete => {
                info!("✅ FileComplete from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    if let Ok(complete) = serde_json::from_slice::<super::FileTransferComplete>(&packet.data) {
                        if let Err(e) = ftm.handle_transfer_complete(from, &complete.file_id).await {
                            error!("❌ Failed to handle file complete: {}", e);
                        }
                    }
                }
            }
            _ => {
                debug!("📨 Unknown CommPacket: {:?} from {}", packet.packet_type, hex::encode(&from.0[..8]));
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
        info!("✏️ Editing message {} for peer {}", hex::encode(&msg_id.0[..8]), hex::encode(&peer_id.0[..8]));
        self.storage.update_message_text(peer_id, msg_id, new_text)
    }

    /// Удалить сообщение локально (только у себя)
    pub fn delete_message_local(&self, peer_id: &HashId, msg_id: &HashId) -> Result<()> {
        info!("🗑️ Deleting message {} locally for peer {}", hex::encode(&msg_id.0[..8]), hex::encode(&peer_id.0[..8]));
        self.storage.delete_message(peer_id, msg_id)
    }

    /// Удалить сообщение для всех (отправить запрос на удаление)
    pub async fn delete_message_for_everyone(&self, peer_id: &HashId, msg_id: &HashId) -> Result<()> {
        info!("🗑️ Deleting message {} for everyone with peer {}", hex::encode(&msg_id.0[..8]), hex::encode(&peer_id.0[..8]));

        // 1. Удалить локально
        self.storage.delete_message(peer_id, msg_id)?;

        // 2. Отправить запрос на удаление пиру
        use crate::communication::{CommPacket, CommControlPacket};

        let delete_data = serde_json::json!({
            "msg_id": hex::encode(&msg_id.0),
            "delete_for_everyone": true
        });

        let data_bytes = serde_json::to_vec(&delete_data)?;

        // Упаковать в P2PPacket
        let p2p_packet = P2PPacket::new(
            P2PPacketType::ChatDeleteMessage,
            self.my_node_id,  // sender
            false,
            data_bytes,
        );

        // Отправить через P2P transport (Dual-Path!)
        self.transport.send_packet_dual_path(*peer_id, p2p_packet).await
            .map_err(|e| anyhow::anyhow!("Failed to send delete request: {}", e))?;

        info!("✅ Delete request sent to peer {}", hex::encode(&peer_id.0[..8]));
        Ok(())
    }

    /// Получить список всех чатов
    pub fn list_chats(&self) -> Result<Vec<HashId>> {
        self.storage.list_chats()
    }
}

// TODO: После тестов remove

/// Most text (bytes) and most inline attachment data (base64 chars) accepted in one incoming chat message.
const MAX_INCOMING_TEXT: usize = 100_000;
const MAX_INCOMING_INLINE_DATA: usize = 6 * 1024 * 1024;

/// An incoming message must be addressed to us, claim to be from the peer whose key decrypted it, and stay in size.
fn check_incoming(msg: &ChatMessage, authenticated_sender: HashId, me: HashId) -> Result<(), &'static str> {
    if msg.to != me {
        return Err("not addressed to us");
    }
    if msg.from != authenticated_sender {
        return Err("sender does not match the key it was decrypted with");
    }
    if msg.text.len() > MAX_INCOMING_TEXT {
        return Err("text too long");
    }
    if msg.attachment.as_ref().and_then(|a| a.data.as_ref()).map_or(false, |d| d.len() > MAX_INCOMING_INLINE_DATA) {
        return Err("inline attachment too large");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_msg(from: u8, to: u8, text: &str) -> ChatMessage {
        ChatMessage { msg_id: HashId([1; 32]), from: HashId([from; 32]), to: HashId([to; 32]), timestamp: 0, text: text.into(), encrypted: true, status: MessageStatus::Pending, edited: false, edit_timestamp: None, attachment: None }
    }

    #[test]
    fn an_incoming_message_must_come_from_its_real_sender_and_stay_in_size() {
        let me = HashId([2; 32]);
        assert_eq!(check_incoming(&plain_msg(5, 2, "hi"), HashId([5; 32]), me), Ok(()));
        assert!(check_incoming(&plain_msg(6, 2, "hi"), HashId([5; 32]), me).is_err(), "peer 5 cannot speak as peer 6");
        assert!(check_incoming(&plain_msg(5, 3, "hi"), HashId([5; 32]), me).is_err(), "addressed to someone else");
        assert!(check_incoming(&plain_msg(5, 2, &"x".repeat(MAX_INCOMING_TEXT + 1)), HashId([5; 32]), me).is_err());
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

    /// Covers both delivery-timeout scenarios in one test (see
    /// test_chat_manager's doc comment for why they share one instance):
    ///
    /// 1. A message that never gets a ChatAck must eventually be marked
    ///    Failed — before this fix it stayed in Shipping forever with no
    ///    way for the sender to know delivery silently didn't happen
    ///    (Station's dual-path send has no retransmission/ACK of its own).
    /// 2. A message that DOES get ACKed in time must NOT be touched by
    ///    the timeout sweep — the fix must not turn reliable, timely
    ///    delivery into a false failure.
    #[tokio::test]
    async fn delivery_timeout_marks_only_the_truly_unacked_message_failed() {
        let cm = test_chat_manager().await;

        let unacked_peer = crate::util::HashId::new_random();
        let unacked_msg = ChatMessage::new(cm.my_node_id, unacked_peer, "hello?".to_string());
        cm.storage.save_outgoing(&unacked_peer, &unacked_msg).unwrap();
        // Simulate "sent long ago, no ACK ever arrived" without a real sleep.
        cm.pending_acks.lock().await.insert(
            unacked_msg.msg_id,
            PendingAck { peer: unacked_peer, sent_at: Instant::now() - CHAT_ACK_TIMEOUT - Duration::from_secs(1) },
        );

        let acked_peer = crate::util::HashId::new_random();
        let acked_msg = ChatMessage::new(cm.my_node_id, acked_peer, "hi".to_string());
        cm.storage.save_outgoing(&acked_peer, &acked_msg).unwrap();
        cm.pending_acks.lock().await.insert(
            acked_msg.msg_id,
            PendingAck { peer: acked_peer, sent_at: Instant::now() },
        );
        // Real ACK arrives promptly for this one.
        let ack_data = serde_json::to_vec(&acked_msg.msg_id).unwrap();
        cm.handle_ack(acked_peer, ack_data).await.unwrap();

        cm.sweep_expired_acks().await;

        let unacked_history = cm.storage.load_history(&unacked_peer, 10).unwrap();
        let unacked_stored = unacked_history.iter().find(|m| m.msg_id == unacked_msg.msg_id).expect("message present");
        assert_eq!(unacked_stored.status, MessageStatus::Failed, "never-ACKed message must be marked Failed");
        assert!(!cm.pending_acks.lock().await.contains_key(&unacked_msg.msg_id));

        let acked_history = cm.storage.load_history(&acked_peer, 10).unwrap();
        let acked_stored = acked_history.iter().find(|m| m.msg_id == acked_msg.msg_id).expect("message present");
        assert_eq!(acked_stored.status, MessageStatus::Read, "promptly-ACKed message must stay Read, never Failed");
        assert!(!cm.pending_acks.lock().await.contains_key(&acked_msg.msg_id));
    }
}
