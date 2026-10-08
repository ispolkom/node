//! Автоматический выбор выхода (решение владельца 2026-09-29: пользователь ничего не выбирает вручную).
//!
//! Из каталога карточек (`network_offers`) берутся узлы, которые сейчас выпускают в интернет, с которыми есть живая связь,
//! по желаемой стране (если задана). Порядок — по качеству: известная страна лучше неизвестной, мощность, задержка.
//! Выходы **чередуются**: недавно использованный уходит в конец очереди, а узел, на котором только что была неудача, на время
//! отдыхает. Страна, взятая из карточки, — ещё не проверенная (`country_source`); проверка извне — следующий шаг.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use crate::network_offers::NodeOffer;

/// Сколько последних выходов помнить для чередования.
pub const RECENT: usize = 8;
/// Сколько секунд узел отдыхает после неудачи (удваивается при повторных, до часа).
pub const REST_SECS: u64 = 60;
const REST_MAX_SECS: u64 = 3600;

fn power_rank(p: &str) -> u8 {
    match p {
        "high" => 2,
        "medium" => 1,
        _ => 0,
    }
}

/// Годные выходы по порядку качества. `connected` — узлы с живой связью (номера в hex), `me` — свой номер.
pub fn rank(offers: &[NodeOffer], connected: &HashSet<String>, me: &str, country: Option<&str>) -> Vec<NodeOffer> {
    let want = country.map(|c| c.to_ascii_uppercase());
    let mut v: Vec<NodeOffer> = offers
        .iter()
        .filter(|o| o.exit && o.can_exit && o.node_id != me && connected.contains(&o.node_id))
        .filter(|o| match &want {
            Some(w) => o.country.as_deref() == Some(w.as_str()),
            None => true,
        })
        .cloned()
        .collect();
    v.sort_by(|a, b| {
        (b.country.is_some(), power_rank(&b.power), std::cmp::Reverse(b.latency_ms.unwrap_or(u32::MAX)))
            .cmp(&(a.country.is_some(), power_rank(&a.power), std::cmp::Reverse(a.latency_ms.unwrap_or(u32::MAX))))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    v
}

#[derive(Default)]
struct State {
    ranked: Vec<String>,
    recent: VecDeque<String>,
    /// узел → (до какого времени отдыхает, сколько неудач подряд)
    rest: HashMap<String, (u64, u32)>,
    /// страна из карточки (то, что узел о себе заявил)
    claimed: HashMap<String, Option<String>>,
    /// страна, измеренная по реальному выходу в интернет
    measured: HashMap<String, String>,
    want: Option<String>,
}

impl State {
    /// Измеренная страна не совпала с заявленной или с нужной — выход обманул, ему больше не верим.
    fn liar(&self, id: &str) -> bool {
        match self.measured.get(id) {
            None => false,
            Some(m) => self.claimed.get(id).and_then(|c| c.as_deref()).map(|c| c != m).unwrap_or(false) || self.want.as_deref().map(|w| w != m).unwrap_or(false),
        }
    }
    fn verified(&self, id: &str) -> bool {
        self.measured.contains_key(id) && !self.liar(id)
    }
    /// Годные по порядку качества, проверенные впереди, обманувшие — вне списка.
    fn usable(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ranked.iter().filter(|i| !self.liar(i)).cloned().collect();
        v.sort_by_key(|i| !self.verified(i)); // устойчиво: порядок качества внутри групп сохраняется
        v
    }
}

/// Очередь выходов: обновляется по каталогу, выдаёт следующий по кругу.
#[derive(Default)]
pub struct ExitPool {
    s: Mutex<State>,
}

impl ExitPool {
    /// Заменить список годных выходов (в порядке качества).
    pub fn update(&self, ranked: &[NodeOffer]) {
        let mut s = self.s.lock().unwrap_or_else(|e| e.into_inner());
        s.ranked = ranked.iter().map(|o| o.node_id.clone()).collect();
        let keep: HashSet<String> = s.ranked.iter().cloned().collect();
        s.rest.retain(|k, _| keep.contains(k));
        s.measured.retain(|k, _| keep.contains(k));
        s.claimed = ranked.iter().map(|o| (o.node_id.clone(), o.country.clone())).collect();
    }

    /// Следующий выход: первый годный, которого нет среди недавних; если все недавние — самый давно использованный.
    /// Отдыхающие после неудачи пропускаются, пока есть другие.
    pub fn next(&self, now: u64) -> Option<String> {
        let mut s = self.s.lock().unwrap_or_else(|e| e.into_inner());
        let awake = |s: &State, id: &String| s.rest.get(id).map(|(until, _)| *until <= now).unwrap_or(true);
        let usable = s.usable();
        let pick = usable
            .iter()
            .find(|id| awake(&s, id) && !s.recent.contains(*id))
            .cloned()
            .or_else(|| {
                // все годные уже были в последних — берём использованный раньше всех
                s.recent.iter().find(|id| usable.contains(id) && awake(&s, id)).cloned()
            })
            .or_else(|| usable.iter().find(|id| awake(&s, id)).cloned());
        if let Some(id) = &pick {
            s.recent.retain(|x| x != id);
            s.recent.push_back(id.clone());
            while s.recent.len() > RECENT {
                s.recent.pop_front();
            }
        }
        pick
    }

    /// Страна, нужная владельцу (`auto-NL`): измеренная иная — выход не годится.
    pub fn set_want(&self, country: Option<String>) {
        self.s.lock().unwrap_or_else(|e| e.into_inner()).want = country;
    }

    /// Измеренная страна выхода. Возвращает `true`, если она подтвердила заявленную (или заявленной не было).
    pub fn measured(&self, id: &str, country: &str) -> bool {
        let mut s = self.s.lock().unwrap_or_else(|e| e.into_inner());
        s.measured.insert(id.to_string(), country.to_ascii_uppercase());
        !s.liar(id)
    }

    /// Выходы, страну которых ещё не измеряли (не обманувшие), по порядку качества.
    pub fn unmeasured(&self) -> Vec<String> {
        let s = self.s.lock().unwrap_or_else(|e| e.into_inner());
        s.ranked.iter().filter(|i| !s.measured.contains_key(*i)).cloned().collect()
    }

    /// Полный номер выхода по началу номера (для закреплённого соединения — измерения).
    pub fn pinned(&self, prefix: &str) -> Option<String> {
        let s = self.s.lock().unwrap_or_else(|e| e.into_inner());
        s.ranked.iter().find(|i| i.starts_with(prefix)).cloned()
    }

    pub fn is_verified(&self, id: &str) -> bool {
        self.s.lock().unwrap_or_else(|e| e.into_inner()).verified(id)
    }

    pub fn is_liar(&self, id: &str) -> bool {
        self.s.lock().unwrap_or_else(|e| e.into_inner()).liar(id)
    }

    /// Выход не сработал: отдохнуть (60 с, потом вдвое дольше при каждой неудаче подряд, до часа).
    pub fn failed(&self, id: &str, now: u64) {
        let mut s = self.s.lock().unwrap_or_else(|e| e.into_inner());
        let n = s.rest.get(id).map(|(_, n)| *n).unwrap_or(0);
        let wait = (REST_SECS << n.min(6)).min(REST_MAX_SECS);
        s.rest.insert(id.to_string(), (now + wait, n + 1));
    }

    /// Выход сработал: неудачи забыты.
    pub fn worked(&self, id: &str) {
        self.s.lock().unwrap_or_else(|e| e.into_inner()).rest.remove(id);
    }

    pub fn len(&self) -> usize {
        self.s.lock().unwrap_or_else(|e| e.into_inner()).ranked.len()
    }
}

/// Запустить обновление очереди: каждые 20 с — каталог карточек → годные выходы; новому выходу один раз отправляется команда
/// запуска шлюза (0x34). Возвращает очередь для прокси.
pub fn start_pool(transport: std::sync::Arc<crate::netlayer::transport::P2PTransport>, country: Option<String>) -> std::sync::Arc<ExitPool> {
    let pool = std::sync::Arc::new(ExitPool::default());
    pool.set_want(country.clone());
    let p = pool.clone();
    crate::supervisor::supervise("exit_pool", crate::supervisor::Policy::restart(), move || {
        let (transport, country, p) = (transport.clone(), country.clone(), p.clone());
        async move {
        let me = transport.identity().node_id().to_hex();
        let mut told: HashSet<String> = HashSet::new();
        loop {
            let (offers, _) = crate::network_offers::directory_snapshot(None);
            let peers = transport.get_peers().await;
            let connected: HashSet<String> = peers.iter().map(|x| x.id.to_hex()).collect();
            let ranked = rank(&offers, &connected, &me, country.as_deref());
            p.update(&ranked);
            for o in ranked.iter().take(8) {
                if told.insert(o.node_id.clone()) {
                    if let Some(x) = peers.iter().find(|x| x.id.to_hex() == o.node_id) {
                        let _ = transport.send_encrypted(x.id, &[0x34u8]).await;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
    }
    });
    pool
}

// ---------------------------------------------------------------- проверка страны выхода

/// Куда выход ходит за своей страной: служба отвечает двумя буквами страны того, кто спрашивает. Для проверок можно подменить
/// (`YANDI_GEO_PROBE=адрес:порт`, ответ — по любому пути).
fn geo_target() -> (String, u16) {
    if let Ok(v) = std::env::var("YANDI_GEO_PROBE") {
        if let Some((h, p)) = v.rsplit_once(':') {
            if let Ok(p) = p.parse() {
                return (h.to_string(), p);
            }
        }
    }
    ("ipinfo.io".to_string(), 80)
}

/// Разобрать ответ службы: двухбуквенная страна в теле.
pub fn parse_geo_reply(reply: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(reply);
    let body = text.split("\r\n\r\n").nth(1)?;
    crate::network_offers::normalise_country(body.trim())
}

/// Спросить страну через закреплённый выход: соединение с локальным прокси под именем `yandi.<начало номера>`.
pub async fn probe_country(proxy: std::net::SocketAddr, password: &str, exit_id: &str) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let work = async {
        let mut s = tokio::net::TcpStream::connect(proxy).await.map_err(|e| format!("прокси: {e}"))?;
        s.write_all(&[5, 1, 2]).await.map_err(|e| e.to_string())?;
        let mut r = [0u8; 2];
        s.read_exact(&mut r).await.map_err(|e| e.to_string())?;
        if r != [5, 2] {
            return Err("прокси не просит пароль".to_string());
        }
        let user = format!("yandi.{}", &exit_id[..16.min(exit_id.len())]);
        let mut auth = vec![1, user.len() as u8];
        auth.extend_from_slice(user.as_bytes());
        auth.push(password.len() as u8);
        auth.extend_from_slice(password.as_bytes());
        s.write_all(&auth).await.map_err(|e| e.to_string())?;
        s.read_exact(&mut r).await.map_err(|e| e.to_string())?;
        if r[1] != 0 {
            return Err("пароль не принят".into());
        }
        let (host, port) = geo_target();
        let mut req = vec![5, 1, 0, 3, host.len() as u8];
        req.extend_from_slice(host.as_bytes());
        req.extend_from_slice(&port.to_be_bytes());
        s.write_all(&req).await.map_err(|e| e.to_string())?;
        let mut rep = [0u8; 10];
        s.read_exact(&mut rep).await.map_err(|e| e.to_string())?;
        if rep[1] != 0 {
            return Err(format!("выход не соединил: {}", rep[1]));
        }
        s.write_all(format!("GET /country HTTP/1.1\r\nHost: {host}\r\nUser-Agent: curl/8\r\nConnection: close\r\n\r\n").as_bytes()).await.map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        let _ = (&mut s).take(4096).read_to_end(&mut buf).await;
        parse_geo_reply(&buf).ok_or_else(|| "ответ без страны".to_string())
    };
    tokio::time::timeout(std::time::Duration::from_secs(25), work).await.map_err(|_| "время вышло".to_string())?
}

/// Фоновая проверка: каждые 15 с одному ещё не измеренному выходу задаётся вопрос «из какой ты страны». Обманувший (измеренная
/// страна не совпала с заявленной) выпадает из очереди; не ответивший пробуется снова не раньше чем через 10 минут.
pub fn start_verifier(pool: std::sync::Arc<ExitPool>, proxy: std::net::SocketAddr, password: String) {
    crate::supervisor::supervise("exit_verifier", crate::supervisor::Policy::restart(), move || {
        let (pool, password) = (pool.clone(), password.clone());
        async move {
        let mut retry_after: HashMap<String, u64> = HashMap::new();
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        loop {
            let now = crate::network_offers::now_secs();
            if let Some(id) = pool.unmeasured().into_iter().find(|i| retry_after.get(i).map(|t| *t <= now).unwrap_or(true)) {
                match probe_country(proxy, &password, &id).await {
                    Ok(c) => {
                        let ok = pool.measured(&id, &c);
                        println!("[exit] {}: измеренная страна {c} — {}", &id[..8], if ok { "заявленная подтверждена" } else { "НЕ совпала, выход исключён" });
                    }
                    Err(e) => {
                        println!("[exit] {}: страну измерить не удалось ({e})", &id[..8]);
                        retry_after.insert(id, now + 600);
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        }
    }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(n: u8, country: Option<&str>, power: &str, latency: Option<u32>, exit: bool) -> NodeOffer {
        NodeOffer {
            v: 1,
            node_id: format!("{n:02x}").repeat(32),
            key: String::new(),
            country: country.map(String::from),
            country_source: "ip_lookup".into(),
            public_ip: true,
            power: power.into(),
            cpu_cores: 4,
            ram_gb: 8,
            latency_ms: latency,
            addr: vec![],
            exit,
            can_exit: exit,
            relay: false,
            dynamic_ip: false, p2p: 0,
            issued: 0,
            expires: 0,
            sig: String::new(),
        }
    }
    fn id(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }
    fn all(ns: &[u8]) -> HashSet<String> {
        ns.iter().map(|n| id(*n)).collect()
    }

    #[test]
    fn only_connected_exits_of_the_wanted_country_never_myself_ordered_by_quality() {
        let offers = vec![
            offer(1, Some("NL"), "medium", Some(50), true),
            offer(2, Some("NL"), "high", Some(90), true),
            offer(3, Some("NL"), "high", Some(20), true),
            offer(4, Some("DE"), "high", Some(5), true),
            offer(5, Some("NL"), "high", Some(1), false), // не выпускает
            offer(6, Some("NL"), "high", Some(1), true),  // нет связи
            offer(7, Some("NL"), "high", Some(1), true),  // это я
            offer(8, None, "high", Some(1), true),
        ];
        let conn = all(&[1, 2, 3, 4, 5, 7, 8]);
        let nl: Vec<String> = rank(&offers, &conn, &id(7), Some("nl")).iter().map(|o| o.node_id[..2].to_string()).collect();
        assert_eq!(nl, vec!["03", "02", "01"], "power first, then the lower delay");
        let any: Vec<String> = rank(&offers, &conn, &id(7), None).iter().map(|o| o.node_id[..2].to_string()).collect();
        assert_eq!(any, vec!["04", "03", "02", "01", "08"], "a known country beats an unknown one");
        assert!(rank(&offers, &conn, &id(7), Some("FR")).is_empty());
    }

    #[test]
    fn exits_take_turns_and_the_recent_one_goes_to_the_back() {
        let p = ExitPool::default();
        p.update(&[offer(1, Some("NL"), "high", None, true), offer(2, Some("NL"), "high", None, true), offer(3, Some("NL"), "high", None, true)]);
        let picks: Vec<String> = (0..7).map(|i| p.next(i).unwrap()[..2].to_string()).collect();
        assert_eq!(picks, vec!["01", "02", "03", "01", "02", "03", "01"]);
        assert_eq!(ExitPool::default().next(0), None, "nobody to pick");
    }

    #[test]
    fn a_failed_exit_rests_longer_each_time_and_a_success_forgives() {
        let p = ExitPool::default();
        p.update(&[offer(1, Some("NL"), "high", None, true), offer(2, Some("NL"), "high", None, true)]);
        p.failed(&id(1), 100);
        assert_eq!(p.next(101).unwrap()[..2], *"02");
        assert_eq!(p.next(102).unwrap()[..2], *"02", "the only awake one is used again");
        assert_eq!(p.next(160).unwrap()[..2], *"01", "after 60 s it is back");
        p.failed(&id(1), 200);
        p.failed(&id(1), 201); // два раза подряд — отдых 240 с (до 441)
        assert_eq!(p.next(300).unwrap()[..2], *"02");
        assert_eq!(p.next(440).unwrap()[..2], *"02");
        assert_eq!(p.next(441).unwrap()[..2], *"01");
        p.failed(&id(1), 450);
        p.worked(&id(1));
        p.failed(&id(1), 500); // успех обнулил счёт: снова 60 с (до 560)
        assert_eq!(p.next(559).unwrap()[..2], *"02");
        assert_eq!(p.next(560).unwrap()[..2], *"01");
        // всё отдыхает — лучше никого, чем сломанный
        let q = ExitPool::default();
        q.update(&[offer(1, None, "high", None, true)]);
        q.failed(&id(1), 0);
        assert_eq!(q.next(1), None);
    }

    #[test]
    fn rest_is_capped_and_gone_exits_are_forgotten() {
        let p = ExitPool::default();
        p.update(&[offer(1, None, "high", None, true)]);
        for _ in 0..20 {
            p.failed(&id(1), 0);
        }
        assert!(p.next(REST_MAX_SECS).is_some(), "never longer than an hour");
        p.update(&[]);
        p.update(&[offer(1, None, "high", None, true)]);
        assert!(p.next(1).is_some(), "left the directory and came back: clean slate");
    }

    #[test]
    fn a_measured_country_that_differs_from_the_claim_throws_the_exit_out_and_verified_ones_come_first() {
        let p = ExitPool::default();
        p.update(&[offer(1, Some("NL"), "high", None, true), offer(2, Some("NL"), "medium", None, true), offer(3, Some("DE"), "low", None, true)]);
        assert_eq!(p.unmeasured().len(), 3);
        assert!(!p.measured(&id(1), "ru"), "claimed NL, really RU: a liar");
        assert!(p.is_liar(&id(1)));
        assert!(p.measured(&id(3), "de"), "case does not matter");
        let picks: Vec<String> = (0..4).map(|i| p.next(i).unwrap()[..2].to_string()).collect();
        assert_eq!(picks, vec!["03", "02", "03", "02"], "the verified one first; the liar never; unmeasured still usable");
        // нужная страна иная, чем измеренная
        let q = ExitPool::default();
        q.set_want(Some("NL".into()));
        q.update(&[offer(1, None, "high", None, true)]);
        assert!(!q.measured(&id(1), "DE"), "no claim, but the owner wants NL and it is DE");
        assert_eq!(q.next(0), None);
        // карточка без страны, страна не нужна — измерение просто подтверждает
        let r = ExitPool::default();
        r.update(&[offer(1, None, "high", None, true)]);
        assert!(r.measured(&id(1), "FR") && r.is_verified(&id(1)));
        // вышел из каталога — измерение забыто
        r.update(&[]);
        r.update(&[offer(1, None, "high", None, true)]);
        assert!(!r.is_verified(&id(1)));
        assert_eq!(r.pinned(&id(1)[..8]), Some(id(1)));
        assert_eq!(r.pinned("ffffffff"), None);
    }

    #[test]
    fn the_geo_reply_is_two_letters_in_the_body_and_nothing_else() {
        assert_eq!(parse_geo_reply(b"HTTP/1.1 200 OK\r\nA: b\r\n\r\nNL\n"), Some("NL".into()));
        assert_eq!(parse_geo_reply(b"HTTP/1.1 200 OK\r\n\r\nnl"), Some("NL".into()));
        assert_eq!(parse_geo_reply(b"HTTP/1.1 200 OK\r\n\r\n<html>blocked</html>"), None);
        assert_eq!(parse_geo_reply(b"HTTP/1.1 200 OK\r\n\r\n"), None);
        assert_eq!(parse_geo_reply(b"garbage"), None);
    }
}
