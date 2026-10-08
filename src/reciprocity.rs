//! Взаимность (решение владельца 2026-09-29): узел, который мог бы помогать сети, но отказывается, теряет доступ к чужой помощи —
//! с честным предупреждением, а не тихо.
//!
//! * Кто «может помогать»: по его же карточке (`can_exit`: публичный адрес и достаточная мощность). Телефоны, слабые машины и узлы
//!   без публичного адреса помогать не могут — к ним правило не применяется никогда.
//! * Кто «помогает»: карточка говорит `exit = true` (выход открыт владельцем).
//! * Новичку и неизвестному (карточки нет) — доверие авансом. Тому, кто может, но не помогает, — льготный объём `GRACE_BYTES`;
//!   дальше отказ с понятной причиной. Доверенные узлы (друзья) правилу не подчиняются: это не рынок, а помощь своим.
//! * Учёт — у каждого свой: сколько байт этот узел пропустил для каждого узла. Карточке верим не до конца (она заявлена самим
//!   узлом), поэтому льгота общая, а не бесконечная: заявленное «помогаю» без дела проверяется следующим шагом (расписки).
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::network_offers::NodeOffer;

/// Сколько байт можно получить, ничего не отдавая сети (100 МБ).
pub const GRACE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// отказ: причина человеческими словами
    Deny(&'static str),
}

pub const WHY_DENIED: &str = "узел мог бы выпускать других в интернет, но у него это выключено; лимит бесплатной помощи исчерпан — включите выход (вкладка «Настройки»)";

/// Чистое правило. `served` — сколько байт этот узел уже пропустил для спрашивающего.
pub fn judge(card: Option<&NodeOffer>, served: u64, trusted: bool) -> Verdict {
    if trusted {
        return Verdict::Allow;
    }
    match card {
        None => Verdict::Allow,
        Some(c) if !c.can_exit || c.exit => Verdict::Allow,
        Some(_) if served < GRACE_BYTES => Verdict::Allow,
        Some(_) => Verdict::Deny(WHY_DENIED),
    }
}

fn ledger() -> &'static Mutex<Ledger> {
    static L: OnceLock<Mutex<Ledger>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Ledger::load()))
}

#[derive(Default)]
struct Ledger {
    served: HashMap<String, u64>,
    dirty_since: Option<std::time::Instant>,
}

fn path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("ledger.json")
}

impl Ledger {
    fn load() -> Self {
        let served = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str::<HashMap<String, u64>>(&s).ok()).unwrap_or_default();
        Ledger { served, dirty_since: None }
    }
    fn add(&mut self, node: &str, n: u64) {
        let e = self.served.entry(node.to_string()).or_insert(0);
        *e = e.saturating_add(n);
        // на диск — не чаще раза в 10 секунд
        let t = *self.dirty_since.get_or_insert_with(std::time::Instant::now);
        if t.elapsed().as_secs() >= 10 {
            self.save();
        }
    }
    fn save(&mut self) {
        if self.served.len() > 20_000 {
            // не растём бесконечно: самых малых забываем
            let mut v: Vec<(String, u64)> = self.served.drain().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v.truncate(10_000);
            self.served = v.into_iter().collect();
        }
        let p = path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, serde_json::to_vec(&self.served).unwrap_or_default());
        self.dirty_since = None;
    }
}

/// Узел пропустил для `node` ещё `n` байт.
pub fn record_served(node: &[u8; 32], n: usize) {
    ledger().lock().unwrap_or_else(|e| e.into_inner()).add(&hex::encode(node), n as u64);
}

pub fn served_to(node: &[u8; 32]) -> u64 {
    ledger().lock().unwrap_or_else(|e| e.into_inner()).served.get(&hex::encode(node)).copied().unwrap_or(0)
}

/// Сбросить учёт на диск (при остановке).
pub fn flush() {
    let mut l = ledger().lock().unwrap_or_else(|e| e.into_inner());
    if l.dirty_since.is_some() {
        l.save();
    }
}

/// Раз в 30 секунд сбрасывать учёт на диск (чтобы остаток после последней передачи не пропал).
pub fn start() {
    crate::supervisor::supervise("reciprocity_flush", crate::supervisor::Policy::restart(), move || {
        async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            flush();
        }
    }
    });
}

/// Правило для живого узла: карточка из каталога + свой учёт.
pub fn verdict_for(node: &[u8; 32], trusted: bool) -> Verdict {
    let card = crate::network_offers::offer_of(&hex::encode(node));
    judge(card.as_ref(), served_to(node), trusted)
}

/// Что сказать владельцу о нём самом: он мог бы помогать, но выключил выход — сеть будет ограничивать его.
pub fn warning_for_owner() -> Option<String> {
    let me = crate::network_offers::own()?;
    (me.can_exit && crate::exit_policy::mode() == crate::exit_policy::ExitMode::Off).then(|| format!("Ваш узел мог бы выпускать других в интернет, но выход выключен. Другие узлы вправе ограничить вам помощь, когда вы получите от них больше {} МБ. Включите выход, чтобы этого не было.", GRACE_BYTES / 1024 / 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(can_exit: bool, exit: bool) -> NodeOffer {
        NodeOffer { v: 1, node_id: "00".repeat(32), key: String::new(), country: None, country_source: "unknown".into(), public_ip: can_exit, power: "high".into(), cpu_cores: 4, ram_gb: 8, latency_ms: None, addr: vec![], exit, can_exit, relay: false, dynamic_ip: false, p2p: 0, issued: 0, expires: 0, sig: String::new() }
    }

    #[test]
    fn only_a_node_that_could_help_but_refuses_is_limited_and_only_after_the_grace() {
        let refuses = card(true, false);
        assert_eq!(judge(Some(&refuses), 0, false), Verdict::Allow, "a new one gets a head start");
        assert_eq!(judge(Some(&refuses), GRACE_BYTES - 1, false), Verdict::Allow);
        assert_eq!(judge(Some(&refuses), GRACE_BYTES, false), Verdict::Deny(WHY_DENIED));
        assert_eq!(judge(Some(&card(true, true)), u64::MAX, false), Verdict::Allow, "it helps — no limit");
        assert_eq!(judge(Some(&card(false, false)), u64::MAX, false), Verdict::Allow, "a phone or a weak machine cannot help — never limited");
        assert_eq!(judge(None, u64::MAX, false), Verdict::Allow, "no card — no judgement");
        assert_eq!(judge(Some(&refuses), u64::MAX, true), Verdict::Allow, "friends are not a market");
    }

    #[test]
    fn the_ledger_counts_per_node() {
        let a = [0xA1u8; 32];
        let before = served_to(&a);
        record_served(&a, 1000);
        record_served(&a, 24);
        assert_eq!(served_to(&a), before + 1024);
        assert_eq!(served_to(&[0xA2u8; 32]), 0);
    }
}
