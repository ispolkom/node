//! Данные туннеля, пришедшие раньше, чем клиентская сторона опубликовала его приёмник.
//!
//! Шлюз/выход подтверждает соединение и сразу начинает слать данные (приветствие SSH и т. п.); клиент публикует приёмник в
//! таблице туннелей только после того, как записал подтверждение приложению. Кусок, пришедший между этими моментами, раньше
//! молча терялся. Здесь он ждёт (с жёстким пределом) и передаётся приёмнику при публикации. Неизвестные туннели ничего не занимают.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Сколько кусков и байт ждёт установки одного туннеля.
pub const MAX_CHUNKS: usize = 64;
pub const MAX_BYTES: usize = 1 << 20;

/// Кусок: данные, признак закрытия, абсолютное смещение.
pub type Chunk = (Vec<u8>, bool, u64);

#[derive(Default)]
pub struct EarlyTunnels {
    map: Mutex<HashMap<u64, Vec<Chunk>>>,
}

impl EarlyTunnels {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Туннель устанавливается: отметка снимается при выходе из установки (в том числе по ошибке).
    pub fn begin(self: &Arc<Self>, id: u64) -> Establishing {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Vec::new());
        Establishing { owner: self.clone(), id }
    }

    /// Принять кусок для устанавливаемого туннеля; `false` — туннель не устанавливается или предел исчерпан.
    pub fn offer(&self, id: u64, data: Vec<u8>, close: bool, offset: u64) -> bool {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let Some(buf) = m.get_mut(&id) else { return false };
        let bytes: usize = buf.iter().map(|c| c.0.len()).sum();
        if buf.len() >= MAX_CHUNKS || bytes + data.len() > MAX_BYTES {
            return false;
        }
        buf.push((data, close, offset));
        true
    }

    /// Забрать накопленное (при публикации приёмника).
    pub fn take(&self, id: u64) -> Vec<Chunk> {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&id).unwrap_or_default()
    }

    /// Устанавливается ли туннель (для списков «ожидаемых»: он жив до публикации приёмника).
    pub fn is_establishing(&self, id: u64) -> bool {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&id)
    }
}

pub struct Establishing {
    owner: Arc<EarlyTunnels>,
    id: u64,
}

impl Drop for Establishing {
    fn drop(&mut self) {
        self.owner.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_chunks_wait_in_order_within_limits_and_unknown_tunnels_are_ignored() {
        let e = EarlyTunnels::new();
        assert!(!e.offer(1, vec![1], false, 0), "не устанавливается — не принимается");
        let g = e.begin(1);
        for i in 0..MAX_CHUNKS {
            assert!(e.offer(1, vec![0; 4], false, i as u64 * 4));
        }
        assert!(!e.offer(1, vec![0; 4], false, 0), "предел кусков");
        let got = e.take(1);
        assert_eq!(got.len(), MAX_CHUNKS);
        assert_eq!(got[5].2, 20);
        assert!(!e.is_establishing(1), "после take отметки нет");
        drop(g);
        let g = e.begin(2);
        assert!(e.is_establishing(2));
        assert!(!e.offer(2, vec![0; MAX_BYTES + 1], false, 0), "предел байт");
        drop(g);
        assert!(!e.is_establishing(2), "отметка снята при выходе");
    }
}
