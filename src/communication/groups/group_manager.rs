//! Менеджер групп

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, debug, warn};

use crate::util::HashId;
use super::group::{Group, GroupId, GroupMember, GroupRole, GroupSettings};
use crate::dht::group_record::SignedGroupRecord;
use crate::core::NodeIdentity;
use super::group_message::{GroupMessage, GroupMessageType, GroupSyncState};

/// Менеджер групп
pub struct GroupManager {
    /// Локальные группы (где текущая нода является участником)
    my_groups: Arc<Mutex<HashMap<GroupId, Group>>>,
    
    /// Состояния синхронизации групп
    sync_states: Arc<Mutex<HashMap<GroupId, GroupSyncState>>>,
    
    /// Пендинг приглашения
    pending_invites: Arc<Mutex<HashMap<GroupId, Vec<HashId>>>>,
}

impl GroupManager {
    pub fn new() -> Self {
        Self {
            my_groups: Arc::new(Mutex::new(HashMap::new())),
            sync_states: Arc::new(Mutex::new(HashMap::new())),
            pending_invites: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    /// Создать новую группу
    pub async fn create_group(
        &self,
        name: String,
        description: String,
        created_by: HashId,
        settings: GroupSettings,
    ) -> Group {
        let group = Group::new(name, description, created_by, settings);
        
        let mut groups = self.my_groups.lock().await;
        groups.insert(group.id, group.clone());
        
        // Создаем состояние синхронизации
        let mut states = self.sync_states.lock().await;
        states.insert(group.id, GroupSyncState::new(group.id));
        
        info!("📁 Group created: {} (id: {})", group.name, group.id);
        
        // Save to disk
        if let Err(e) = self.save_to_disk().await {
            warn!("Failed to save group to disk: {}", e);
        }
        
        group
    }
    
    /// Получить группу по ID
    pub async fn get_group(&self, group_id: &GroupId) -> Option<Group> {
        let groups = self.my_groups.lock().await;
        groups.get(group_id).cloned()
    }
    
    /// Получить все группы пользователя
    pub async fn get_my_groups(&self) -> Vec<Group> {
        let groups = self.my_groups.lock().await;
        groups.values().cloned().collect()
    }
    
    /// Добавить участника в группу
    pub async fn add_member(
        &self,
        group_id: &GroupId,
        member: GroupMember,
        added_by: &HashId,
    ) -> Result<(), String> {
        let mut groups = self.my_groups.lock().await;
        let group = groups.get_mut(group_id)
            .ok_or("Group not found")?;
        
        let role = group.get_role(added_by)
            .ok_or("Not a member")?;
        
        if !role.can_invite() {
            return Err("Not enough permissions".to_string());
        }
        
        if group.add_member(member) {
            info!("➕ Member added to group {}", group_id);
            let _ = self.save_to_disk().await;
            Ok(())
        } else {
            Err("Failed to add member".to_string())
        }
    }
    
    /// Удалить участника из группы
    pub async fn remove_member(
        &self,
        group_id: &GroupId,
        node_id: &HashId,
        removed_by: &HashId,
    ) -> Result<(), String> {
        let mut groups = self.my_groups.lock().await;
        let group = groups.get_mut(group_id)
            .ok_or("Group not found")?;
        
        let remover_role = group.get_role(removed_by)
            .ok_or("Not a member")?;
        
        if !remover_role.can_kick() {
            return Err("Not enough permissions".to_string());
        }
        
        if group.remove_member(node_id) {
            info!("➖ Member removed from group {}", group_id);
            let _ = self.save_to_disk().await;
            Ok(())
        } else {
            Err("Failed to remove member".to_string())
        }
    }
    
    /// Получить список участников группы
    pub async fn get_members(&self, group_id: &GroupId) -> Vec<GroupMember> {
        let groups = self.my_groups.lock().await;
        if let Some(group) = groups.get(group_id) {
            group.members.values().cloned().collect()
        } else {
            Vec::new()
        }
    }
    
    /// Получить количество участников
    pub async fn member_count(&self, group_id: &GroupId) -> usize {
        let groups = self.my_groups.lock().await;
        groups.get(group_id).map(|g| g.members.len()).unwrap_or(0)
    }
    
    /// Обновить настройки группы
    pub async fn update_settings(
        &self,
        group_id: &GroupId,
        updater: &HashId,
        f: impl FnOnce(&mut GroupSettings),
    ) -> Result<(), String> {
        let mut groups = self.my_groups.lock().await;
        let group = groups.get_mut(group_id)
            .ok_or("Group not found")?;
        
        let role = group.get_role(updater)
            .ok_or("Not a member")?;
        
        if !role.can_edit_settings() {
            return Err("Not enough permissions".to_string());
        }
        
        f(&mut group.settings);
        group.version += 1;
        
        info!("⚙️ Group settings updated: {}", group_id);
        let _ = self.save_to_disk().await;
        Ok(())
    }
    
    /// Отправить сообщение в группу
    pub async fn send_message(
        &self,
        group_id: &GroupId,
        from: HashId,
        msg_type: GroupMessageType,
    ) -> Result<GroupMessage, String> {
        let groups = self.my_groups.lock().await;
        let group = groups.get(group_id)
            .ok_or("Group not found")?;
        
        if !group.can_send_messages(&from) {
            return Err("Cannot send messages".to_string());
        }
        
        let msg = match msg_type {
            GroupMessageType::Text(text) => GroupMessage::new_text(*group_id, from, text),
            _ => {
                return Err("Message type not implemented yet".to_string());
            }
        };
        
        let mut states = self.sync_states.lock().await;
        if let Some(state) = states.get_mut(group_id) {
            state.add_message(msg.clone());
        }
        
        info!("💬 Message sent to group {}: {}", group_id, &msg.msg_id.to_hex()[..16]);
        
        Ok(msg)
    }
    
    /// Получить историю сообщений группы
    pub async fn get_messages(&self, group_id: &GroupId, limit: usize) -> Vec<GroupMessage> {
        let states = self.sync_states.lock().await;
        if let Some(state) = states.get(group_id) {
            let mut messages: Vec<_> = state.local_messages.values().cloned().collect();
            messages.sort_by_key(|m| m.timestamp);
            messages.into_iter().rev().take(limit).collect()
        } else {
            Vec::new()
        }
    }
    
    /// Очистить историю сообщений группы
    pub async fn clear_history(&self, group_id: &GroupId) -> Result<(), String> {
        let mut states = self.sync_states.lock().await;
        if let Some(state) = states.get_mut(group_id) {
            state.local_messages.clear();
            info!("🗑️ Chat history cleared for group {}", group_id);
            Ok(())
        } else {
            Err("Group not found".to_string())
        }
    }
    
    /// Save all groups to disk
    pub async fn save_to_disk(&self) -> Result<(), String> {
        let groups_dir = dirs::home_dir()
            .ok_or("No home directory")?
            .join(".yandi/data/groups");
        
        tokio::fs::create_dir_all(&groups_dir).await
            .map_err(|e| format!("Failed to create groups dir: {}", e))?;
        
        let groups = self.my_groups.lock().await;
        let groups_list: Vec<&Group> = groups.values().collect();
        
        let data = serde_json::json!({
            "groups": groups_list,
            "updated_at": chrono::Utc::now().to_rfc3339()
        });
        
        let groups_file = groups_dir.join("groups.json");
        let content = serde_json::to_string_pretty(&data)
            .map_err(|e| format!("Failed to serialize groups: {}", e))?;
        
        tokio::fs::write(&groups_file, content).await
            .map_err(|e| format!("Failed to write groups file: {}", e))?;
        
        Ok(())
    }
    
    /// Load groups from disk
    pub async fn load_from_disk(&self) -> Result<(), String> {
        let groups_dir = dirs::home_dir()
            .ok_or("No home directory")?
            .join(".yandi/data/groups");
        
        let groups_file = groups_dir.join("groups.json");
        if !groups_file.exists() {
            return Ok(());
        }
        
        let content = tokio::fs::read_to_string(&groups_file).await
            .map_err(|e| format!("Failed to read groups file: {}", e))?;
        
        let data: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse groups file: {}", e))?;
        
        let groups_array = data.get("groups").and_then(|v| v.as_array())
            .ok_or("Invalid groups file format")?;
        
        let mut groups = self.my_groups.lock().await;
        for group_value in groups_array {
            if let Ok(group) = serde_json::from_value::<Group>(group_value.clone()) {
                groups.insert(group.id, group);
            }
        }
        
        info!("📁 Loaded {} groups from disk", groups.len());
        Ok(())
    }
}

// ============================================================
// DHT Integration Methods
// ============================================================

use crate::netlayer::transport::P2PTransport;


impl GroupManager {
    /// Получить DHT ключ для группы в виде HashId
    pub fn dht_key_group(group_id: &GroupId) -> HashId {
        let key_str = format!("yandi:group:{}", group_id.to_hex());
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(key_str.as_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        HashId(bytes)
    }
    
    /// Загрузить группу из DHT (с проверкой подписи)
    pub async fn load_group_from_dht(
        &self,
        group_id: &GroupId,
        transport: &P2PTransport,
    ) -> Result<Option<Group>, String> {
        let key = Self::dht_key_group(group_id);
        
        match transport.dht_get(key).await {
            Some(value) => {
                let signed: SignedGroupRecord = serde_json::from_slice(&value)
                    .map_err(|e| format!("Deserialize error: {}", e))?;
                
                if !signed.verify() {
                    return Err("Invalid signature on group record".to_string());
                }
                
                let group = signed.get_group()?;
                
                let mut groups = self.my_groups.lock().await;
                groups.insert(group.id, group.clone());
                
                let mut states = self.sync_states.lock().await;
                states.insert(group.id, GroupSyncState::new(group.id));
                
                info!("📥 Group loaded from DHT: {} ({} members)", 
                    group.name, group.members.len());
                
                Ok(Some(group))
            }
            None => Ok(None),
        }
    }
    
    /// Синхронизировать группу с DHT
    pub async fn sync_group(
        &self,
        group_id: &GroupId,
        transport: &P2PTransport,
    ) -> Result<(), String> {
        let remote_group = match self.load_group_from_dht(group_id, transport).await? {
            Some(g) => g,
            None => return Err("Group not found in DHT".to_string()),
        };
        
        let mut groups = self.my_groups.lock().await;
        let local_group = groups.get_mut(group_id);
        
        match local_group {
            Some(local) => {
                if remote_group.version > local.version {
                    info!("🔄 Syncing group {}: local v{} -> remote v{}", 
                        group_id, local.version, remote_group.version);
                    *local = remote_group;
                    let _ = self.save_to_disk().await;
                }
            }
            None => {
                groups.insert(remote_group.id, remote_group);
                let _ = self.save_to_disk().await;
                info!("📥 New group synced from DHT: {}", group_id);
            }
        }
        
        Ok(())
    }
    
    /// Store group in DHT with signature (secure version)
    pub async fn store_group_in_dht_signed(
        &self,
        group: &Group,
        transport: &P2PTransport,
        identity: &NodeIdentity,
    ) -> Result<(), String> {
        let sequence = group.version;
        let signed = SignedGroupRecord::new(group, identity, sequence)?;
        let key = Self::dht_key_group(&group.id);
        let value = serde_json::to_vec(&signed)
            .map_err(|e| format!("Serialize error: {}", e))?;
        
        transport.dht_store(key, value).await;
        info!("📡 Signed group stored in DHT: {}", group.id);
        Ok(())
    }
}
