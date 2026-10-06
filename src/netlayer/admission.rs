//! Допуск новых узлов: защита от наплыва поддельных личностей.
//!
//! Создать личность ничего не стоит (ключ Ed25519 и подпись), поэтому подписанное приветствие от неизвестного узла само по себе ничего не доказывает.
//! Без ограничений каждое такое приветствие занимало место в таблице пиров, в DHT и в списке виденных одноразовых номеров: один злоумышленник мог
//! заполнить память узла. Здесь правила:
//! * известные узлы (уже в таблице) обновляются, но не чаще `per_id_per_minute` раз в минуту;
//! * новые личности: не больше `per_ip_new_per_minute` с одного адреса и `global_new_per_minute` на весь узел в минуту;
//! * таблица пиров не растёт выше `max_peers`; при заполнении вытесняется самый давний неприкреплённый узел (`pick_victim`), а если такого нет — отказ.
//! Все счётчики сами ограничены по размеру.
use crate::util::HashId;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct AdmissionConfig {
    pub max_peers: usize,
    pub per_ip_new_per_minute: u32,
    pub global_new_per_minute: u32,
    pub per_id_per_minute: u32,
    pub window: Duration,
    /// сколько записей о частоте хранить (счётчики сами не растут без предела)
    pub max_tracked: usize,
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self { max_peers: 2000, per_ip_new_per_minute: 10, global_new_per_minute: 300, per_id_per_minute: 20, window: Duration::from_secs(60), max_tracked: 20_000 }
    }
}

impl AdmissionConfig {
    /// Размер таблицы по мощности узла: чем слабее узел, тем меньше он держит.
    pub fn for_power(power: crate::util::NodePower) -> Self {
        Self { max_peers: power.max_connections() * 20, ..Self::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// известный узел шлёт приветствия слишком часто
    TooFrequent,
    /// с этого адреса слишком много новых личностей
    PerSourceNew,
    /// на весь узел слишком много новых личностей
    GlobalNew,
    /// таблица пиров полна и вытеснять некого
    TableFull,
}

impl Reject {
    pub fn name(&self) -> &'static str {
        match self {
            Reject::TooFrequent => "too-frequent",
            Reject::PerSourceNew => "per-source-new",
            Reject::GlobalNew => "global-new",
            Reject::TableFull => "table-full",
        }
    }
}

#[derive(Clone, Copy)]
struct Window {
    start: Instant,
    count: u32,
}

impl Window {
    /// Учесть событие; true, если лимит не превышен.
    fn hit(&mut self, now: Instant, window: Duration, limit: u32) -> bool {
        if now.duration_since(self.start) >= window {
            *self = Window { start: now, count: 0 };
        }
        self.count = self.count.saturating_add(1);
        self.count <= limit
    }
}

pub struct Admission {
    cfg: AdmissionConfig,
    per_ip: HashMap<IpAddr, Window>,
    per_id: HashMap<HashId, Window>,
    global: Window,
    rejected: u64,
    allowed: u64,
}

impl Admission {
    pub fn new(cfg: AdmissionConfig) -> Self {
        let now = Instant::now();
        Self { cfg, per_ip: HashMap::new(), per_id: HashMap::new(), global: Window { start: now, count: 0 }, rejected: 0, allowed: 0 }
    }

    pub fn config(&self) -> &AdmissionConfig {
        &self.cfg
    }

    pub fn rejected_total(&self) -> u64 {
        self.rejected
    }

    pub fn allowed_total(&self) -> u64 {
        self.allowed
    }

    fn prune<K: std::hash::Hash + Eq + Clone>(map: &mut HashMap<K, Window>, now: Instant, window: Duration, max: usize) {
        if map.len() > max {
            map.retain(|_, w| now.duration_since(w.start) < window * 2);
        }
        // если и после чистки слишком много — счётчики не копятся дальше: забываем самые давние
        if map.len() > max {
            let mut v: Vec<(K, Instant)> = map.iter().map(|(k, w)| (k.clone(), w.start)).collect();
            v.sort_by_key(|(_, t)| *t);
            for (k, _) in v.into_iter().take(map.len() - max / 2) {
                map.remove(&k);
            }
        }
    }

    /// Решение по приветствию. `known` — узел уже в таблице. `table_len` — размер таблицы.
    pub fn check(&mut self, ip: IpAddr, id: HashId, known: bool, table_len: usize, now: Instant) -> Result<(), Reject> {
        let r = self.decide(ip, id, known, table_len, now);
        if r.is_err() {
            self.rejected += 1;
        } else {
            self.allowed += 1;
        }
        r
    }

    fn decide(&mut self, ip: IpAddr, id: HashId, known: bool, table_len: usize, now: Instant) -> Result<(), Reject> {
        let (window, max) = (self.cfg.window, self.cfg.max_tracked);
        if known {
            Self::prune(&mut self.per_id, now, window, max);
            let w = self.per_id.entry(id).or_insert(Window { start: now, count: 0 });
            return if w.hit(now, window, self.cfg.per_id_per_minute) { Ok(()) } else { Err(Reject::TooFrequent) };
        }
        if table_len >= self.cfg.max_peers {
            return Err(Reject::TableFull);
        }
        Self::prune(&mut self.per_ip, now, window, max);
        // сначала смотрим оба лимита, не засчитывая отказанное: отказ одного не должен тратить квоту другого
        let ip_ok = {
            let w = self.per_ip.entry(ip).or_insert(Window { start: now, count: 0 });
            w.hit(now, window, self.cfg.per_ip_new_per_minute)
        };
        if !ip_ok {
            return Err(Reject::PerSourceNew);
        }
        if !self.global.hit(now, window, self.cfg.global_new_per_minute) {
            return Err(Reject::GlobalNew);
        }
        Ok(())
    }
}

/// Кого вытеснить, когда таблица полна: самый давний из неприкреплённых и не якорей, молчащий не меньше `min_idle_ms`.
/// Каждая запись: (номер, время последнего приветствия в мс, прикреплён ли, якорь ли).
pub fn pick_victim<I: IntoIterator<Item = (HashId, u128, bool, bool)>>(entries: I, now_ms: u128, min_idle_ms: u128) -> Option<HashId> {
    entries
        .into_iter()
        .filter(|(_, seen, pinned, anchor)| !*pinned && !*anchor && now_ms.saturating_sub(*seen) >= min_idle_ms)
        .min_by_key(|(_, seen, _, _)| *seen)
        .map(|(id, _, _, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }
    fn id(n: u32) -> HashId {
        let mut b = [0u8; 32];
        b[..4].copy_from_slice(&n.to_be_bytes());
        HashId(b)
    }

    #[test]
    fn one_source_cannot_add_unlimited_identities() {
        let mut a = Admission::new(AdmissionConfig::default());
        let now = Instant::now();
        let ok = (0..1000).filter(|i| a.check(ip(1), id(*i), false, 0, now).is_ok()).count();
        assert_eq!(ok, 10, "с одного адреса — не больше per_ip_new_per_minute новых личностей в минуту");
        assert_eq!(a.check(ip(1), id(5000), false, 0, now), Err(Reject::PerSourceNew));
        // другой адрес не пострадал
        assert!(a.check(ip(2), id(6000), false, 0, now).is_ok());
    }

    #[test]
    fn the_quota_renews_after_the_window() {
        let mut a = Admission::new(AdmissionConfig::default());
        let t0 = Instant::now();
        for i in 0..10 {
            assert!(a.check(ip(1), id(i), false, 0, t0).is_ok());
        }
        assert!(a.check(ip(1), id(99), false, 0, t0).is_err());
        assert!(a.check(ip(1), id(100), false, 0, t0 + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn many_sources_are_limited_together() {
        let mut a = Admission::new(AdmissionConfig { global_new_per_minute: 50, per_ip_new_per_minute: 10, ..Default::default() });
        let now = Instant::now();
        let mut ok = 0;
        for src in 0..200u8 {
            for k in 0..3u32 {
                if a.check(ip(src), id(src as u32 * 10 + k), false, 0, now).is_ok() {
                    ok += 1;
                }
            }
        }
        assert_eq!(ok, 50, "общий потолок на новые личности");
        assert!(a.check(ip(250), id(77777), false, 0, now) == Err(Reject::GlobalNew));
    }

    #[test]
    fn the_table_does_not_grow_past_its_ceiling_but_known_peers_still_refresh() {
        let mut a = Admission::new(AdmissionConfig { max_peers: 100, ..Default::default() });
        let now = Instant::now();
        assert_eq!(a.check(ip(1), id(1), false, 100, now), Err(Reject::TableFull));
        assert!(a.check(ip(2), id(2), true, 100, now).is_ok(), "уже известный узел обновляется и при полной таблице");
    }

    #[test]
    fn a_known_peer_cannot_flood_hellos() {
        let mut a = Admission::new(AdmissionConfig::default());
        let now = Instant::now();
        let ok = (0..500).filter(|_| a.check(ip(1), id(1), true, 5, now).is_ok()).count();
        assert_eq!(ok, 20);
        assert_eq!(a.check(ip(1), id(1), true, 5, now), Err(Reject::TooFrequent));
        assert!(a.check(ip(1), id(2), true, 5, now).is_ok(), "другой узел не пострадал");
    }

    #[test]
    fn a_refused_per_source_attempt_does_not_use_the_global_quota() {
        let mut a = Admission::new(AdmissionConfig { global_new_per_minute: 15, per_ip_new_per_minute: 10, ..Default::default() });
        let now = Instant::now();
        for i in 0..1000 {
            let _ = a.check(ip(1), id(i), false, 0, now);
        }
        // с одного адреса принято 10; остальные 990 отказаны по адресу и не съели общую квоту (осталось 5)
        let ok = (0..20u8).filter(|s| a.check(ip(100 + s), id(5000 + *s as u32), false, 0, now).is_ok()).count();
        assert_eq!(ok, 5);
    }

    #[test]
    fn counters_stay_bounded_under_a_flood_from_many_addresses() {
        let mut a = Admission::new(AdmissionConfig { max_tracked: 1000, per_ip_new_per_minute: 1, global_new_per_minute: u32::MAX, ..Default::default() });
        let now = Instant::now();
        for n in 0..100_000u32 {
            let src = IpAddr::V4(Ipv4Addr::from(n));
            let _ = a.check(src, id(n), false, 0, now);
        }
        assert!(a.per_ip.len() <= 1000 + 1, "счётчики адресов не растут без предела: {}", a.per_ip.len());
    }

    #[test]
    fn eviction_picks_the_oldest_unpinned_idle_peer() {
        let entries = vec![(id(1), 1000u128, false, false), (id(2), 500, true, false), (id(3), 900, false, true), (id(4), 700, false, false), (id(5), 99_000, false, false)];
        assert_eq!(pick_victim(entries.clone(), 100_000, 60_000), Some(id(4)), "прикреплённый и якорь не трогаются; из остальных — самый давний");
        assert_eq!(pick_victim(entries.clone(), 100_000, 200_000), None, "никто не молчал достаточно долго");
        assert_eq!(pick_victim(Vec::new(), 1, 1), None);
    }

    #[test]
    fn config_scales_with_power() {
        use crate::util::NodePower;
        assert_eq!(AdmissionConfig::for_power(NodePower::Low).max_peers, 200);
        assert_eq!(AdmissionConfig::for_power(NodePower::High).max_peers, 3000);
    }
}
