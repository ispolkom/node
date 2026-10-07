//! История внешнего адреса узла. Адрес «статический», пока он не менялся; две смены за 30 дней — адрес «динамический», и узел сам
//! объявляет это в своей карточке (она тогда живёт недолго, и сеть чаще перепроверяет такой адрес). Метка возвращается к «статический»
//! сама, когда смены уходят за границу месяца. Файл хранит только последний адрес и моменты смен, не весь перечень адресов.
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

pub const WINDOW_SECS: u64 = 30 * 24 * 3600;
pub const CHANGES_FOR_DYNAMIC: usize = 2;
const KEEP_CHANGES: usize = 16;

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpHistory {
    pub last: Option<String>,
    pub changes: Vec<u64>,
}

impl IpHistory {
    /// Новое наблюдение адреса; true — адрес сменился (первое наблюдение сменой не считается).
    pub fn observe(&mut self, ip: &str, now: u64) -> bool {
        if ip.is_empty() {
            return false;
        }
        let changed = matches!(&self.last, Some(p) if p != ip);
        if changed {
            self.changes.push(now);
            if self.changes.len() > KEEP_CHANGES {
                self.changes.remove(0);
            }
        }
        self.last = Some(ip.to_string());
        changed
    }

    pub fn is_dynamic(&self, now: u64) -> bool {
        self.changes.iter().filter(|t| now.saturating_sub(**t) < WINDOW_SECS).count() >= CHANGES_FOR_DYNAMIC
    }
}

fn state() -> &'static Mutex<IpHistory> {
    static S: OnceLock<Mutex<IpHistory>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(load()))
}

fn path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("ip_history.json")
}

fn load() -> IpHistory {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(h: &IpHistory) {
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(b) = serde_json::to_vec(h) {
        let _ = crate::util::private_file::write_private(&p, &b);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Запомнить только что определённый внешний адрес (вызывать при запуске и при каждой проверке).
pub fn observe(ip: &str) {
    let mut g = state().lock().unwrap_or_else(|e| e.into_inner());
    let before = g.clone();
    if g.observe(ip, now_secs()) {
        println!("[ip] внешний адрес сменился; смен за месяц: {}", g.changes.iter().filter(|t| now_secs().saturating_sub(**t) < WINDOW_SECS).count());
    }
    if *g != before {
        save(&g);
    }
}

pub fn is_dynamic() -> bool {
    state().lock().unwrap_or_else(|e| e.into_inner()).is_dynamic(now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    const DAY: u64 = 24 * 3600;

    #[test]
    fn the_first_sighting_and_a_single_change_keep_the_address_static() {
        let mut h = IpHistory::default();
        assert!(!h.observe("1.1.1.1", 100));
        assert!(!h.observe("1.1.1.1", 200));
        assert!(!h.is_dynamic(300));
        assert!(h.observe("2.2.2.2", 1000)); // оборудование провайдера сменили — один раз
        assert!(!h.is_dynamic(1000 + DAY));
    }

    #[test]
    fn two_changes_within_a_month_make_it_dynamic_and_a_quiet_month_makes_it_static_again() {
        let mut h = IpHistory::default();
        h.observe("1.1.1.1", 0);
        h.observe("2.2.2.2", 5 * DAY);
        h.observe("3.3.3.3", 20 * DAY);
        assert!(h.is_dynamic(21 * DAY));
        assert!(h.is_dynamic(34 * DAY)); // первая смена уже старше месяца? 29 дней — ещё в окне
        assert!(!h.is_dynamic(36 * DAY)); // осталась одна смена в окне
    }

    #[test]
    fn two_changes_far_apart_do_not_count() {
        let mut h = IpHistory::default();
        h.observe("1.1.1.1", 0);
        h.observe("2.2.2.2", 10 * DAY);
        h.observe("3.3.3.3", 50 * DAY);
        assert!(!h.is_dynamic(51 * DAY));
    }

    #[test]
    fn an_empty_address_is_not_an_observation() {
        let mut h = IpHistory::default();
        h.observe("1.1.1.1", 0);
        assert!(!h.observe("", 10));
        assert_eq!(h.last.as_deref(), Some("1.1.1.1"));
    }
}
