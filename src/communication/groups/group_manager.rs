//! Менеджер групп

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

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

    /// Where groups.json lives (tests point it at a temporary folder; None = the owner's data folder)
    dir_override: Option<std::path::PathBuf>,
}

impl GroupManager {
    pub fn new() -> Self {
        Self {
            my_groups: Arc::new(Mutex::new(HashMap::new())),
            sync_states: Arc::new(Mutex::new(HashMap::new())),
            pending_invites: Arc::new(Mutex::new(HashMap::new())),
            dir_override: None,
        }
    }

    #[cfg(test)]
    fn in_dir(dir: std::path::PathBuf) -> Self {
        let mut m = Self::new();
        m.dir_override = Some(dir);
        m
    }

    fn groups_dir(&self) -> Result<std::path::PathBuf, String> {
        match &self.dir_override {
            Some(d) => Ok(d.clone()),
            None => Ok(dirs::home_dir().ok_or("No home directory")?.join(".yandi/data/groups")),
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

        // the locks must be released first: saving takes the groups lock itself
        drop(states);
        drop(groups);

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
            drop(groups);
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
            drop(groups);
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
        drop(groups);
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
        let groups_dir = self.groups_dir()?;
        
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
        let groups_dir = self.groups_dir()?;
        
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

                // The signature only proves that the key INSIDE the record signed it. The record must be about the group we
                // asked for, and it must be signed by the group's owner (a node id derived from that very key).
                check_group_record(&signed, &group, group_id)?;

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
        let mut changed = false;

        match local_group {
            Some(local) => {
                // a record for an existing group may only come from the same owner, and only a newer one replaces ours
                if remote_group.created_by != local.created_by {
                    return Err("Group record has another owner than the local group".to_string());
                }
                if remote_group.version > local.version {
                    info!("🔄 Syncing group {}: local v{} -> remote v{}", 
                        group_id, local.version, remote_group.version);
                    *local = remote_group;
                    changed = true;
                }
            }
            None => {
                groups.insert(remote_group.id, remote_group);
                info!("📥 New group synced from DHT: {}", group_id);
                changed = true;
            }
        }
        drop(groups);
        if changed {
            let _ = self.save_to_disk().await;
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

/// A group record from the DHT is acceptable only if it is about the requested group and signed by that group's owner,
/// whose node id must be derived from the signing key (an old random id proves nothing and is refused here).
fn check_group_record(signed: &crate::dht::group_record::SignedGroupRecord, group: &Group, wanted: &GroupId) -> Result<(), String> {
    if group.id != *wanted || signed.group_id != *wanted {
        return Err("Group record is about another group".to_string());
    }
    if !matches!(group.members.get(&group.created_by).map(|m| &m.role), Some(GroupRole::Owner)) {
        return Err("Group record has no owner".to_string());
    }
    if !crate::util::types::id_bound_to_key(&group.created_by.0, &signed.public_key) {
        return Err("Group record is not signed by the group's owner".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod dht_record_tests {
    use super::*;

    #[tokio::test]
    async fn creating_a_group_and_changing_its_members_does_not_hang_and_the_groups_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (owner, friend) = (HashId([1; 32]), HashId([2; 32]));
        let m = GroupManager::in_dir(dir.path().to_path_buf());
        let work = async {
            let g = m.create_group("g".into(), "d".into(), owner, GroupSettings::default()).await;
            m.add_member(&g.id, GroupMember::new(friend, "friend".into(), GroupRole::Member), &owner).await.unwrap();
            m.update_settings(&g.id, &owner, |_| {}).await.unwrap();
            m.remove_member(&g.id, &friend, &owner).await.unwrap();
            g.id
        };
        let id = tokio::time::timeout(std::time::Duration::from_secs(5), work).await.expect("a group operation hung (lock held across save)");
        let again = GroupManager::in_dir(dir.path().to_path_buf());
        again.load_from_disk().await.unwrap();
        assert!(again.get_group(&id).await.is_some(), "the group is still there after a restart");
    }

    #[test]
    fn a_group_with_members_can_be_written_to_json() {
        let owner = NodeIdentity::new();
        let group = Group::new("g".into(), "d".into(), owner.node_id(), GroupSettings::default());
        let r = serde_json::to_string(&group);
        assert!(r.is_ok(), "groups are saved to disk as JSON: {:?}", r.err());
        let back: Group = serde_json::from_str(&r.unwrap()).unwrap();
        assert_eq!(back.members.len(), 1);
    }
    use crate::dht::group_record::SignedGroupRecord;

    /// A record as the DHT would hand it over (signed by `key`); built by hand because `SignedGroupRecord::new` cannot serialize
    /// a group with members at all (JSON maps need string keys) — the DHT group sync has never worked, see the review notes.
    fn record(group: &Group, signer: &NodeIdentity, about: GroupId) -> SignedGroupRecord {
        SignedGroupRecord { group_id: about, group_data: Vec::new(), public_key: signer.signing_public_key, timestamp: 0, sequence: 1, signature: Vec::new() }
    }

    #[test]
    fn only_the_owner_of_a_group_can_publish_its_record() {
        let owner = NodeIdentity::new();
        let impostor = NodeIdentity::new();
        let group = Group::new("g".into(), "d".into(), owner.node_id(), GroupSettings::default());
        assert!(check_group_record(&record(&group, &owner, group.id), &group, &group.id).is_ok());
        // someone else signs a record for the same group
        assert!(check_group_record(&record(&group, &impostor, group.id), &group, &group.id).is_err());
        // a record for another group is refused even when the owner signed it
        assert!(check_group_record(&record(&group, &owner, GroupId::random()), &group, &group.id).is_err());
    }
}
