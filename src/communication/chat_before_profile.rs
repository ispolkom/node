// src/communication/chat.rs
//! Chat manager for P2P text messaging

use crate::communication::{
    ChatStorage, ChatMessage, MessageStatus, CommControlPacket, CommPacket,
    E2EEncryption,
};
use crate::util::HashId;
use crate::p2p::{P2PTransport, P2PPacket, P2PPacketType};
use std::sync::Arc;
use tokio::sync::mpsc;
use anyhow::Result;
use tracing::{info, error, debug};

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
}

impl ChatManager {
    /// Создать новый ChatManager
    pub fn new(
        my_node_id: HashId,
        transport: Arc<P2PTransport>,
    ) -> Result<Self> {
        let storage = ChatStorage::new(my_node_id)?;
        let e2e_encryption = Arc::new(E2EEncryption::new());
        let (incoming_tx, _incoming_rx) = mpsc::unbounded_channel();

        Ok(Self {
            my_node_id,
            storage,
            transport,
            e2e_encryption,
            incoming_tx,
            file_transfer_manager: None,
        })
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

        // 3. Проверить: нам ли?
        if msg.to != self.my_node_id {
            error!("❌ Message not for us! to={:?}, we={:?}", msg.to, self.my_node_id);
            return Err(anyhow::anyhow!("Message not for us"));
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

        Ok(())
    }

    /// Обработать CommPacket из transport
    pub async fn handle_comm_packet(&self, from: HashId, packet: CommPacket) -> Result<()> {
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
                debug!("📦 FileChunk from {}", hex::encode(&from.0[..8]));
                if let Some(ref ftm) = self.file_transfer_manager {
                    if let Ok(chunk) = serde_json::from_slice::<super::FileChunk>(&packet.data) {
                        if let Err(e) = ftm.handle_chunk(chunk).await {
                            error!("❌ Failed to handle chunk: {}", e);
                        }
                    }
                }
            }
            CommControlPacket::FileTransferEnd => {
                info!("🏁 FileTransferEnd from {}", hex::encode(&from.0[..8]));
            }
            CommControlPacket::FileMissing => {
                debug!("📭 FileMissing from {}", hex::encode(&from.0[..8]));
            }
            CommControlPacket::FileComplete => {
                info!("✅ FileComplete from {}", hex::encode(&from.0[..8]));
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_storage() {
        // TODO: добавить тесты
    }
}
