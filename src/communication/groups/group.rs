//! Структуры данных для групп

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

use crate::util::HashId;

/// Уникальный идентификатор группы
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId(pub [u8; 32]);

impl GroupId {
    pub fn from_hex(hex: &str) -> Result<Self, String> {
        let bytes = hex::decode(hex).map_err(|e| format!("Invalid hex: {}", e))?;
        if bytes.len() != 32 {
            return Err(format!("Expected 32 bytes, got {}", bytes.len()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    
    pub fn random() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.to_hex()[..16])
    }
}

/// Роль участника в группе
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupRole {
    Owner,      // Создатель группы
    Admin,      // Администратор
    Moderator,  // Модератор
    Member,     // Обычный участник
    Restricted, // Ограниченный (только чтение)
}

impl GroupRole {
    pub fn can_invite(&self) -> bool {
        matches!(self, GroupRole::Owner | GroupRole::Admin | GroupRole::Moderator)
    }
    
    pub fn can_kick(&self) -> bool {
        matches!(self, GroupRole::Owner | GroupRole::Admin)
    }
    
    pub fn can_edit_settings(&self) -> bool {
        matches!(self, GroupRole::Owner | GroupRole::Admin)
    }
}

/// Участник группы
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMember {
    pub node_id: HashId,
    pub short_id: String,
    pub nickname: String,
    pub role: GroupRole,
    pub joined_at: u64,
    pub last_seen: u64,
    pub is_active: bool,
}

impl GroupMember {
    pub fn new(node_id: HashId, nickname: String, role: GroupRole) -> Self {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            node_id,
            short_id: hex::encode(&node_id.0[..8]),
            nickname,
            role,
            joined_at: now,
            last_seen: now,
            is_active: true,
        }
    }
}

/// Настройки группы
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSettings {
    pub is_private: bool,
    pub is_encrypted: bool,
    pub max_members: usize,
    pub allow_files: bool,
    pub allow_voice: bool,
    pub message_ttl_seconds: u64,
    pub join_approval: bool,
}

impl Default for GroupSettings {
    fn default() -> Self {
        Self {
            is_private: true,
            is_encrypted: true,
            max_members: 0,
            allow_files: true,
            allow_voice: true,
            message_ttl_seconds: 86400 * 30,
            join_approval: true,
        }
    }
}

/// Основная структура группы
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub description: String,
    pub avatar_hash: Option<String>,
    pub created_by: HashId,
    pub created_at: u64,
    pub settings: GroupSettings,
    pub members: HashMap<HashId, GroupMember>,
    pub version: u64,
}

impl Group {
    pub fn new(name: String, description: String, created_by: HashId, settings: GroupSettings) -> Self {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let owner = GroupMember::new(created_by, "Owner".to_string(), GroupRole::Owner);
        
        let mut members = HashMap::new();
        members.insert(created_by, owner);
        
        Self {
            id: GroupId::random(),
            name,
            description,
            avatar_hash: None,
            created_by,
            created_at: now,
            settings,
            members,
            version: 1,
        }
    }
    
    pub fn is_member(&self, node_id: &HashId) -> bool {
        self.members.contains_key(node_id)
    }
    
    pub fn get_role(&self, node_id: &HashId) -> Option<GroupRole> {
        self.members.get(node_id).map(|m| m.role)
    }
    
    pub fn can_send_messages(&self, node_id: &HashId) -> bool {
        match self.get_role(node_id) {
            Some(GroupRole::Restricted) => false,
            Some(_) => true,
            None => false,
        }
    }
    
    pub fn add_member(&mut self, member: GroupMember) -> bool {
        if self.settings.max_members > 0 && self.members.len() >= self.settings.max_members {
            return false;
        }
        
        if !self.members.contains_key(&member.node_id) {
            self.members.insert(member.node_id, member);
            self.version += 1;
            true
        } else {
            false
        }
    }
    
    pub fn remove_member(&mut self, node_id: &HashId) -> bool {
        if self.created_by == *node_id {
            return false; // Нельзя удалить создателя
        }
        
        if self.members.remove(node_id).is_some() {
            self.version += 1;
            true
        } else {
            false
        }
    }
    
    pub fn set_role(&mut self, node_id: &HashId, new_role: GroupRole) -> bool {
        if self.created_by == *node_id {
            return false; // Нельзя изменить роль создателя
        }
        
        if let Some(member) = self.members.get_mut(node_id) {
            member.role = new_role;
            self.version += 1;
            true
        } else {
            false
        }
    }
}

// ============================================================
// Преобразование между GroupId и HashId
// ============================================================

impl From<GroupId> for crate::util::HashId {
    fn from(group_id: GroupId) -> Self {
        crate::util::HashId(group_id.0)
    }
}

impl From<crate::util::HashId> for GroupId {
    fn from(hash_id: crate::util::HashId) -> Self {
        GroupId(hash_id.0)
    }
}

impl GroupId {
    /// Преобразовать в HashId для DHT
    pub fn as_hash_id(&self) -> crate::util::HashId {
        crate::util::HashId(self.0)
    }
    
    /// Создать из HashId
    pub fn from_hash_id(hash_id: &crate::util::HashId) -> Self {
        GroupId(hash_id.0)
    }
}

// ============================================================
// Преобразование между GroupId и HashId
// ============================================================
