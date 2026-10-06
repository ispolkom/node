//! Сообщения в группах

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::util::HashId;
use super::group::GroupId;

/// Тип сообщения в группе
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GroupMessageType {
    Text(String),
    File {
        filename: String,
        size: u64,
        mime_type: String,
        file_id: String,
    },
    Voice {
        duration_secs: u32,
        file_id: String,
    },
    Image {
        width: u32,
        height: u32,
        file_id: String,
        thumbnail: Option<String>,
    },
    System {
        action: SystemAction,
    },
}

/// Системные действия
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemAction {
    MemberJoined { node_id: HashId, nickname: String },
    MemberLeft { node_id: HashId },
    MemberKicked { node_id: HashId, kicked_by: HashId },
    RoleChanged { node_id: HashId, new_role: String },
    GroupRenamed { old_name: String, new_name: String },
    SettingsChanged,
}

/// Сообщение в группе
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMessage {
    pub msg_id: HashId,
    pub group_id: GroupId,
    pub from: HashId,
    pub timestamp: u64,
    pub msg_type: GroupMessageType,
    pub reply_to: Option<HashId>,
    pub edited_at: Option<u64>,
    pub deleted: bool,
}

impl GroupMessage {
    pub fn new_text(group_id: GroupId, from: HashId, text: String) -> Self {
        use rand::RngCore;
        
        let mut msg_id_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut msg_id_bytes);
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            msg_id: HashId(msg_id_bytes),
            group_id,
            from,
            timestamp,
            msg_type: GroupMessageType::Text(text),
            reply_to: None,
            edited_at: None,
            deleted: false,
        }
    }
    
    pub fn new_system(group_id: GroupId, from: HashId, action: SystemAction) -> Self {
        use rand::RngCore;
        
        let mut msg_id_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut msg_id_bytes);
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            msg_id: HashId(msg_id_bytes),
            group_id,
            from,
            timestamp,
            msg_type: GroupMessageType::System { action },
            reply_to: None,
            edited_at: None,
            deleted: false,
        }
    }
}

/// Состояние синхронизации группы
#[derive(Debug, Clone)]
pub struct GroupSyncState {
    pub group_id: GroupId,
    pub last_message_id: Option<HashId>,
    pub last_sync_at: u64,
    pub local_messages: HashMap<HashId, GroupMessage>,
}

impl GroupSyncState {
    pub fn new(group_id: GroupId) -> Self {
        Self {
            group_id,
            last_message_id: None,
            last_sync_at: 0,
            local_messages: HashMap::new(),
        }
    }
    
    pub fn add_message(&mut self, msg: GroupMessage) {
        self.local_messages.insert(msg.msg_id, msg);
    }
}

// ============================================================
// Message Sync Methods
// ============================================================

impl GroupSyncState {
    /// Получить последние сообщения
    pub fn get_recent_messages(&self, limit: usize) -> Vec<GroupMessage> {
        let mut msgs: Vec<_> = self.local_messages.values().cloned().collect();
        msgs.sort_by_key(|m| m.timestamp);
        msgs.into_iter().rev().take(limit).collect()
    }
    
    /// Получить сообщения после определенного ID
    pub fn get_messages_after(&self, after_id: &HashId) -> Vec<GroupMessage> {
        let mut msgs: Vec<_> = self.local_messages.values().cloned().collect();
        msgs.sort_by_key(|m| m.timestamp);
        
        let start_index = msgs.iter().position(|m| m.msg_id == *after_id);
        
        match start_index {
            Some(idx) => msgs.into_iter().skip(idx + 1).collect(),
            None => Vec::new(),
        }
    }
    
    /// Получить количество непрочитанных
    pub fn unread_count(&self, last_read_id: Option<&HashId>) -> usize {
        match last_read_id {
            Some(id) => self.get_messages_after(id).len(),
            None => self.local_messages.len(),
        }
    }
}

/// Групповое сообщение с метаданными для синхронизации
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMessageSync {
    pub group_id: GroupId,
    pub messages: Vec<GroupMessage>,
    pub version: u64,
    pub sync_id: HashId,
}

impl GroupMessageSync {
    pub fn new(group_id: GroupId, messages: Vec<GroupMessage>) -> Self {
        use rand::RngCore;
        
        let mut sync_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut sync_bytes);
        
        Self {
            group_id,
            messages,
            version: 1,
            sync_id: HashId(sync_bytes),
        }
    }
}
