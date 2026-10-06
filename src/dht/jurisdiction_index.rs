// src/dht/jurisdiction_index.rs
//! Hardening Step 5: индекс `jurisdiction → Vec<(NodeId, PeerInfo-snapshot)>`.
//!
//! Используется чтобы find_anchors_by_jurisdiction вернул больше кандидатов чем
//! есть в локальной peer-table. Каждый anchor при announce'е (Hello с jurisdiction
//! TLV) обновляет соответствующую запись. TTL — 30 минут, чтобы убирать stale-нодов.
//!
//! Это lite-вариант полного DHT find_by_jurisdiction (с отдельным RPC-байтом и
//! Kademlia-style рекурсивным запросом — см. план §5 P3); здесь мы ограничиваемся
//! root-level индексом, обновляемым из локальных событий приёма announce'ов.

use crate::util::HashId;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct JurisdictionEntry {
    pub node_id: HashId,
    pub addr: String,
    pub last_seen: Instant,
}

/// In-memory индекс `country (uppercase ISO-2) → Vec<entry>`.
#[derive(Debug, Default)]
pub struct JurisdictionIndex {
    inner: RwLock<HashMap<String, Vec<JurisdictionEntry>>>,
    /// Hardening Step 5: TTL для записей. Старше — выкидываются при gc.
    ttl: Duration,
}

impl JurisdictionIndex {
    pub fn new() -> Self {
        Self { inner: RwLock::new(HashMap::new()), ttl: Duration::from_secs(30 * 60) }
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self { inner: RwLock::new(HashMap::new()), ttl }
    }

    /// Запомнить anchor с заданной jurisdiction. Вызывается при приёме Hello'а с TLV.
    pub fn announce(&self, country: &str, node_id: HashId, addr: String) {
        let key = country.to_uppercase();
        if key.is_empty() {
            return;
        }
        let mut map = self.inner.write().unwrap();
        let entries = map.entry(key).or_default();
        // Если запись для node_id уже есть — обновим last_seen.
        if let Some(e) = entries.iter_mut().find(|e| e.node_id == node_id) {
            e.addr = addr;
            e.last_seen = Instant::now();
        } else {
            entries.push(JurisdictionEntry { node_id, addr, last_seen: Instant::now() });
        }
    }

    /// Вернуть актуальный список anchor'ов с заданной jurisdiction.
    /// Сортировка: новые (recent) сначала.
    pub fn lookup(&self, country: &str) -> Vec<JurisdictionEntry> {
        let key = country.to_uppercase();
        let map = self.inner.read().unwrap();
        let mut out = match map.get(&key) {
            Some(v) => v.iter()
                .filter(|e| e.last_seen.elapsed() <= self.ttl)
                .cloned()
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        out
    }

    /// Удалить из индекса записи старше TTL. Возвращает количество удалённых entry.
    pub fn gc(&self) -> usize {
        let mut removed = 0;
        let mut map = self.inner.write().unwrap();
        for entries in map.values_mut() {
            let before = entries.len();
            entries.retain(|e| e.last_seen.elapsed() <= self.ttl);
            removed += before - entries.len();
        }
        map.retain(|_, v| !v.is_empty());
        removed
    }

    /// Удалить конкретный node_id из всех jurisdictions.
    pub fn remove(&self, node_id: &HashId) {
        let mut map = self.inner.write().unwrap();
        for entries in map.values_mut() {
            entries.retain(|e| e.node_id != *node_id);
        }
        map.retain(|_, v| !v.is_empty());
    }

    /// Сколько разных стран зарегистрировано.
    pub fn country_count(&self) -> usize {
        self.inner.read().unwrap().len()
    }
}

// ----------------- Tests -----------------

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(b: u8) -> HashId { HashId([b; 32]) }

    #[test]
    fn announce_and_lookup() {
        let idx = JurisdictionIndex::new();
        idx.announce("DE", nid(1), "10.0.0.1:8443".into());
        idx.announce("DE", nid(2), "10.0.0.2:8443".into());
        idx.announce("US", nid(3), "10.0.0.3:8443".into());

        let de = idx.lookup("DE");
        assert_eq!(de.len(), 2);
        let de_ids: Vec<_> = de.iter().map(|e| e.node_id).collect();
        assert!(de_ids.contains(&nid(1)));
        assert!(de_ids.contains(&nid(2)));

        let us = idx.lookup("us"); // case-insensitive
        assert_eq!(us.len(), 1);
        assert_eq!(us[0].node_id, nid(3));

        let nothing = idx.lookup("ZZ");
        assert!(nothing.is_empty());
    }

    #[test]
    fn dedup_on_repeat_announce() {
        let idx = JurisdictionIndex::new();
        idx.announce("DE", nid(1), "10.0.0.1:8443".into());
        idx.announce("DE", nid(1), "10.0.0.1:8443".into());
        idx.announce("DE", nid(1), "10.0.0.99:8443".into()); // address changed
        let de = idx.lookup("DE");
        assert_eq!(de.len(), 1);
        assert_eq!(de[0].addr, "10.0.0.99:8443");
    }

    #[test]
    fn ttl_expires() {
        let idx = JurisdictionIndex::with_ttl(Duration::from_millis(50));
        idx.announce("DE", nid(1), "10.0.0.1:8443".into());
        assert_eq!(idx.lookup("DE").len(), 1);
        std::thread::sleep(Duration::from_millis(70));
        assert_eq!(idx.lookup("DE").len(), 0);
        let cleaned = idx.gc();
        assert_eq!(cleaned, 1);
        assert_eq!(idx.country_count(), 0);
    }

    #[test]
    fn remove_node() {
        let idx = JurisdictionIndex::new();
        idx.announce("DE", nid(1), "a".into());
        idx.announce("DE", nid(2), "b".into());
        idx.announce("US", nid(1), "c".into());
        idx.remove(&nid(1));
        assert_eq!(idx.lookup("DE").len(), 1);
        assert_eq!(idx.lookup("US").len(), 0);
    }
}
