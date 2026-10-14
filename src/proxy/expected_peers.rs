//! Кому принадлежит запрос: ответ и данные туннеля принимаются только от узла, которому запрос был отправлен.
//! Без этого любой связанный узел мог бы подставить чужой ответ, угадав порядковый номер запроса.

use crate::util::types::HashId;
use std::collections::HashMap;
use std::sync::Mutex;

/// Максимум одновременно отслеживаемых запросов; больше — самые старые записи отбрасываются вместе с запросом.
const MAX_TRACKED: usize = 16_384;

#[derive(Default)]
pub struct ExpectedPeers {
    owners: Mutex<HashMap<u64, HashId>>,
}

impl ExpectedPeers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Запомнить, от кого ждём ответ и данные по запросу `id`.
    pub fn expect(&self, id: u64, peer: HashId) {
        let mut m = self.owners.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= MAX_TRACKED {
            // защита от разрастания: лишнее — это уже закрытые запросы, их потеря безопасна (данные без записи отбрасываются)
            m.clear();
        }
        m.insert(id, peer);
    }

    /// Пришло ли от нужного узла? Нет записи — тоже «нет»: незнакомый запрос не принимается.
    pub fn is_from_owner(&self, id: u64, from: &HashId) -> bool {
        let m = self.owners.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&id).map_or(false, |p| p == from)
    }

    /// Запрос закончен.
    pub fn forget(&self, id: u64) {
        self.owners.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// Оставить только записи, у которых есть живой запрос (вызывается при регистрации нового).
    pub fn retain_live(&self, mut live: impl FnMut(u64) -> bool) {
        self.owners.lock().unwrap_or_else(|e| e.into_inner()).retain(|id, _| live(*id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_addressed_peer_is_accepted() {
        let e = ExpectedPeers::new();
        let (a, b) = (HashId([1; 32]), HashId([2; 32]));
        e.expect(7, a);
        assert!(e.is_from_owner(7, &a));
        assert!(!e.is_from_owner(7, &b), "чужой узел с тем же номером не принимается");
        assert!(!e.is_from_owner(8, &a), "незнакомый номер не принимается");
        e.forget(7);
        assert!(!e.is_from_owner(7, &a));
    }
}
