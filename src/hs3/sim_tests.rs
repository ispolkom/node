//! Модель сети для автомата рукопожатия: две машины, очередь сообщений с задержками, потерей, дублированием и перестановкой, поддельные часы,
//! недоступные пути, повторы старых сообщений противником. Свойства: сходимость на одном `T` после возобновления доставки и инварианты
//! безопасности (ключи порядка зафиксированных поколений строго растут, у стороны нет «чужих» поколений).
use super::codec::*;
use super::machine::*;
use ed25519_dalek::SigningKey;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const BASE: u64 = 1_700_000_000_000;

struct Msg {
    at: u64,
    to: usize,
    path: PathId,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy)]
struct Blackout {
    from: usize, // чьи сообщения пропадают
    start: u64,
    end: u64,
}

struct Sim {
    now: u64,
    m: Vec<Machine>,
    q: Vec<Msg>,
    rng: StdRng,
    loss: f64,
    dup: f64,
    max_delay: u64,
    blackouts: Vec<Blackout>,
    /// Дополнительная задержка для сообщений заданного типа от заданной машины в окне: (тип, от кого, начало, конец, добавка).
    delays: Vec<(MsgType, usize, u64, u64, u64)>,
    /// Окно потерь, которое начинается в момент ПЕРВОЙ отправки сообщения заданного типа заданной машиной: (от кого, тип, длительность, начало).
    triggers: Vec<(usize, MsgType, u64, Option<u64>)>,
    /// Недоступные пути: сообщения на них пропадают в окне.
    dead_paths: Vec<(PathId, u64, u64)>,
    /// Принудительный путь исходящих сообщений машины (ответы приходят «с другого пути»).
    force_path: [Option<PathId>; 2],
    /// Как путь выглядит для получателя: (кто отправил, путь отправителя, путь у получателя).
    deliver_as: Vec<(usize, PathId, PathId)>,
    /// Потеря сообщений заданного типа от машины в окне.
    drop_types: Vec<(usize, MsgType, u64, u64)>,
    commits: Vec<Vec<([u8; 32], Role, OrderKey)>>,
    dropped: Vec<MsgType>,
    seen: Vec<Vec<u8>>,
    replay: f64,
}

fn machine(n: u8, ctx: u8) -> Machine {
    Machine::new(n as u64, SigningKey::from_bytes(&[n; 32]), ctx)
}

impl Sim {
    fn new(seed: u64) -> Self {
        let m = vec![machine(1, CTX_NETLAYER_UDP), machine(2, CTX_NETLAYER_UDP)];
        Self {
            now: 0,
            m,
            q: Vec::new(),
            rng: StdRng::seed_from_u64(seed),
            loss: 0.0,
            dup: 0.0,
            max_delay: 100,
            blackouts: vec![],
            delays: vec![],
            triggers: vec![],
            dead_paths: vec![],
            force_path: [None, None],
            deliver_as: vec![],
            drop_types: vec![],
            commits: vec![vec![], vec![]],
            dropped: vec![],
            seen: Vec::new(),
            replay: 0.0,
        }
    }

    fn ids(&self) -> (PeerId, PeerId) {
        (self.m[0].id(), self.m[1].id())
    }

    fn apply(&mut self, from: usize, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Committed { t, role, key, .. } => self.commits[from].push((t, role, key)),
                Out::Send { path, bytes } => self.send(from, path, bytes),
            }
        }
    }

    fn send(&mut self, from: usize, path: PathId, bytes: Vec<u8>) {
        let to = 1 - from;
        let path = self.force_path[from].unwrap_or(path);
        if self.seen.len() < 4000 {
            self.seen.push(bytes.clone());
        }
        let ty = parse_header(&bytes).map(|h| h.0).ok();
        for tr in self.triggers.iter_mut() {
            if tr.0 == from && Some(tr.1) == ty && tr.3.is_none() {
                tr.3 = Some(self.now);
            }
        }
        let now = self.now;
        let typed_drop = self.drop_types.iter().any(|d| d.0 == from && Some(d.1) == ty && now >= d.2 && now < d.3);
        let in_trigger = self.triggers.iter().any(|tr| tr.0 == from && tr.3.is_some_and(|s| now >= s && now < s + tr.2));
        if typed_drop || in_trigger || self.blackouts.iter().any(|b| b.from == from && now >= b.start && now < b.end) || self.dead_paths.iter().any(|d| d.0 == path && now >= d.1 && now < d.2) {
            if let Some(t) = ty {
                self.dropped.push(t);
            }
            return;
        }
        if self.rng.gen_bool(self.loss) {
            return;
        }
        let copies = if self.rng.gen_bool(self.dup) { 2 } else { 1 };
        for _ in 0..copies {
            let mut d = 10 + self.rng.gen_range(0..=self.max_delay);
            if let Some(t) = ty {
                for (dt, f, s, e, add) in &self.delays {
                    if *dt == t && *f == from && now >= *s && now < *e {
                        d += *add;
                    }
                }
            }
            let recv_path = self.deliver_as.iter().find(|x| x.0 == from && x.1 == path).map(|x| x.2).unwrap_or(path);
            self.q.push(Msg { at: now + d, to, path: recv_path, bytes: bytes.clone() });
        }
    }

    fn start(&mut self, who: usize, path: PathId) {
        let (a, b) = self.ids();
        let peer = if who == 0 { b } else { a };
        let outs = self.m[who].start(BASE + self.now, peer, path);
        self.apply(who, outs);
    }

    fn inject(&mut self, to: usize, path: PathId, bytes: Vec<u8>, delay: u64) {
        self.q.push(Msg { at: self.now + delay, to, path, bytes });
    }

    fn run_until(&mut self, end: u64) {
        while self.now < end {
            self.now += 50;
            let now = self.now;
            if self.replay > 0.0 && !self.seen.is_empty() && self.rng.gen_bool(self.replay) {
                let m = self.seen[self.rng.gen_range(0..self.seen.len())].clone();
                let to = self.rng.gen_range(0..2);
                let path = self.rng.gen_range(1..4);
                self.q.push(Msg { at: now + 5, to, path, bytes: m });
            }
            let mut due: Vec<Msg> = Vec::new();
            let mut rest = Vec::new();
            for m in self.q.drain(..) {
                if m.at <= now {
                    due.push(m);
                } else {
                    rest.push(m);
                }
            }
            self.q = rest;
            due.sort_by_key(|m| m.at);
            for m in due {
                let outs = self.m[m.to].receive(BASE + now, m.path, &m.bytes);
                self.apply(m.to, outs);
            }
            for i in 0..2 {
                let outs = self.m[i].tick(BASE + now);
                self.apply(i, outs);
            }
        }
    }

    /// Инварианты безопасности: ключи порядка зафиксированных поколений строго растут (нет возврата назад и повторной фиксации), у стороны
    /// нет «чужих» поколений.
    fn check_invariants(&self) -> Result<(), String> {
        for (i, c) in self.commits.iter().enumerate() {
            for w in c.windows(2) {
                if w[1].2 <= w[0].2 {
                    return Err(format!("машина {i}: ключ порядка не вырос ({:02x?} → {:02x?})", &w[0].0[..3], &w[1].0[..3]));
                }
            }
        }
        let (a, b) = self.ids();
        for (i, other) in [(0usize, 1usize), (1, 0)] {
            let peer = if i == 0 { b } else { a };
            if let Some(g) = self.m[i].active(&peer) {
                if !self.commits[other].iter().any(|(t, _, _)| *t == g.t) {
                    return Err(format!("машина {i} активна на поколении, которого другая сторона не фиксировала"));
                }
            }
        }
        Ok(())
    }

    fn converged(&self) -> bool {
        let (a, b) = self.ids();
        match (self.m[0].active(&b), self.m[1].active(&a)) {
            (Some(x), Some(y)) => x.t == y.t && x.tx_open && y.tx_open,
            _ => false,
        }
    }
}

#[test]
fn happy_path_converges_on_one_generation_with_mirrored_keys() {
    let mut s = Sim::new(1);
    s.start(0, 1);
    s.run_until(5_000);
    assert!(s.converged());
    let (a, b) = s.ids();
    let (ga, gb) = (s.m[0].active(&b).unwrap(), s.m[1].active(&a).unwrap());
    assert_eq!(ga.send_key(), gb.recv_key());
    assert_eq!(ga.recv_key(), gb.send_key());
    assert_ne!(ga.send_key(), ga.recv_key());
    assert_eq!(s.commits[0].len(), 1);
    assert_eq!(s.commits[1].len(), 1);
    assert_eq!(s.m[0].sending_generation(&b), Some(ga.t));
    s.check_invariants().unwrap();
}

/// P23-10: окно потерь начинается в момент первой отправки FinR и обязательно теряет его; проверяется, что поток действительно потерян.
#[test]
fn losing_the_last_flight_for_3_9_11_and_31_seconds_still_converges() {
    for loss_ms in [3_000u64, 9_000, 11_000, 31_000] {
        let mut s = Sim::new(loss_ms);
        s.triggers.push((1, MsgType::FinR, loss_ms, None));
        s.start(0, 1);
        s.run_until(120_000 + loss_ms);
        assert!(s.dropped.contains(&MsgType::FinR), "потеря {loss_ms} мс: FinR не был потерян, сценарий пуст");
        assert!(s.converged(), "потеря {loss_ms} мс: стороны не сошлись");
        s.check_invariants().unwrap();
    }
}

/// P23-1: поздний FinR старой попытки не уничтожает более новую попытку, которая завершается позже.
#[test]
fn a_late_finr_of_an_older_attempt_does_not_destroy_the_newer_one() {
    let mut s = Sim::new(11);
    s.max_delay = 10;
    s.delays.push((MsgType::FinR, 1, 0, 1_000, 11_150)); // FinR1 придёт около 11,2 с
    s.delays.push((MsgType::FinR, 1, 11_000, 12_000, 400)); // FinR2 придёт позже FinR1
    s.start(0, 1);
    s.run_until(120_000);
    assert!(s.converged());
    s.check_invariants().unwrap();
}

/// P23-2 (А): задержанный FinI старой попытки не перекрывает поколение новой попытки.
#[test]
fn a_delayed_fini_of_an_older_attempt_cannot_overwrite_a_newer_generation() {
    let mut s = Sim::new(12);
    s.max_delay = 10;
    s.delays.push((MsgType::Request, 0, 0, 500, 9_000)); // R1 доходит до B через 9 с
    s.delays.push((MsgType::FinI, 0, 9_000, 9_600, 3_500)); // FinI1 приходит после завершения второй попытки
    s.start(0, 1);
    s.run_until(120_000);
    assert!(s.converged());
    s.check_invariants().unwrap();
}

/// P23-2 (Б): поздний FinR при встречной роли не воскрешает проигравший обмен — проверяется в обеих ориентациях.
#[test]
fn a_late_finr_does_not_resurrect_a_losing_exchange_in_either_orientation() {
    for first in [0usize, 1] {
        let mut s = Sim::new(13 + first as u64);
        s.max_delay = 10;
        s.delays.push((MsgType::FinR, 1 - first, 0, 1_000, 10_100));
        s.start(first, 1);
        s.run_until(10_010);
        s.start(1 - first, 2);
        s.run_until(120_000);
        assert!(s.converged(), "ориентация {first}");
        s.check_invariants().unwrap();
    }
}

#[test]
fn simultaneous_exchanges_converge_on_one_generation() {
    for offset in [0u64, 20, 60, 150, 400, 1_500] {
        for seed in 0..5u64 {
            let mut s = Sim::new(offset * 10 + seed);
            s.start(0, 1);
            s.run_until(offset);
            s.start(1, 2);
            s.run_until(60_000);
            assert!(s.converged(), "встречные обмены (сдвиг {offset} мс, зерно {seed}) не сошлись");
            s.check_invariants().unwrap();
        }
    }
}

#[test]
fn a_replayed_request_from_another_path_does_not_stop_the_honest_exchange() {
    let mut s = Sim::new(3);
    let (_a, b) = s.ids();
    let outs = s.m[0].start(BASE, b, 1);
    let r = outs.iter().find_map(|o| if let Out::Send { bytes, .. } = o { Some(bytes.clone()) } else { None }).unwrap();
    s.inject(1, 99, r.clone(), 10);
    s.inject(1, 1, r, 2_000);
    s.run_until(40_000);
    assert!(s.converged(), "обмен сорван копией запроса");
}

#[test]
fn a_wrong_finished_changes_nothing() {
    let mut s = Sim::new(4);
    s.start(0, 1);
    s.run_until(5_000);
    assert!(s.converged());
    let (a, _b) = s.ids();
    let t = s.m[1].active(&a).unwrap().t;
    let bad = Fin { kind: MsgType::FinI, context: CTX_NETLAYER_UDP, t, mac: [0u8; 32] }.encode();
    let before = (s.m[1].active(&a).unwrap().t, s.m[1].slots_used());
    let outs = s.m[1].receive(BASE + 6_000, 1, &bad);
    assert!(outs.is_empty());
    assert_eq!(before, (s.m[1].active(&a).unwrap().t, s.m[1].slots_used()));
}

/// P23-5: ответ приходит с другого пути; прежний путь мёртв. FinI, FinR и KeyConfirm должны идти по пути, принятому в обмене.
#[test]
fn key_confirm_follows_the_accepted_path_and_the_old_path_may_be_dead() {
    let mut s = Sim::new(5);
    s.force_path[1] = Some(7); // все сообщения B приходят к A с пути 7
    s.deliver_as.push((0, 7, 1)); // а то, что A шлёт на путь 7, у B выглядит как прежний путь 1 (тот же источник)
    s.dead_paths.push((1, 200, u64::MAX)); // путь 1 умирает почти сразу после первого запроса
    s.start(0, 1);
    s.run_until(60_000);
    assert!(s.converged(), "KeyConfirm ушёл на мёртвый путь");
}

/// P23-6: два RX-only коммита подряд при потерянных FinR не вытесняют прежнее подтверждённое поколение отправки.
#[test]
fn two_unconfirmed_rx_commits_keep_the_confirmed_sending_generation() {
    let mut s = Sim::new(6);
    s.start(0, 1);
    s.run_until(5_000);
    assert!(s.converged());
    let (a, b) = s.ids();
    let k0 = s.m[0].active(&b).unwrap().t;
    // FinR от B пропадают: A начинает смену ключа, B фиксирует приём новых поколений, но подтвердить не может
    s.drop_types.push((1, MsgType::FinR, 5_000, 40_000)); // Ack доходят, FinR пропадают
    s.start(0, 1);
    s.run_until(26_000); // за это время A успевает начать вторую попытку, и B получает вторую FinI
    assert!(s.commits[1].len() >= 3, "сценарий не создал двух RX-only коммитов: {}", s.commits[1].len());
    assert_eq!(s.m[1].sending_generation(&a), Some(k0), "B потерял прежнее подтверждённое поколение отправки");
    assert!(s.m[1].accepts(&a, &k0), "B перестал принимать прежнее поколение");
    s.run_until(160_000);
    assert!(s.converged());
    s.check_invariants().unwrap();
}

/// P23-3: память состояния ограничена: пул не превышается, пустые записи о пирах снимаются.
#[test]
fn the_pool_and_the_peer_table_are_bounded() {
    let victim_sk = SigningKey::from_bytes(&[200u8; 32]);
    let mut v = Machine::new(1, victim_sk.clone(), CTX_NETLAYER_UDP);
    let vid = v.id();
    let mut attackers: Vec<Machine> = (0..40u8).map(|i| Machine::new(100 + i as u64, SigningKey::from_bytes(&[50 + i; 32]), CTX_NETLAYER_UDP)).collect();
    // каждый посторонний присылает по нескольку подписанных запросов; затем всё истекает
    for round in 0..3u64 {
        for a in attackers.iter_mut() {
            let outs = a.start(BASE + round * 100, vid, 1);
            for o in outs {
                if let Out::Send { bytes, .. } = o {
                    let _ = v.receive(BASE + round * 100 + 10, 1, &bytes);
                }
            }
        }
    }
    assert!(v.slots_used() <= MAX_SLOTS - RESERVED_SLOTS, "посторонние заняли резерв: {}", v.slots_used());
    assert!(v.slots_used() <= 40 * MAX_PENDING_PER_IDENTITY);
    let _ = v.tick(BASE + 20_000);
    assert_eq!(v.slots_used(), 0, "истёкшие записи остались");
    assert_eq!(v.peer_entries(), 0, "пустые записи о пирах не снимаются");
    // собственный исходящий обмен занимает слот
    let (a_id, _) = (attackers[0].id(), 0);
    let outs = v.start(BASE + 30_000, a_id, 1);
    assert!(!outs.is_empty());
    assert_eq!(v.slots_used(), 1);
}

/// P23-7: срок обмена обеспечивается автоматом (без ожидания отдельного tick) и не паникует на границах времени.
#[test]
fn an_expired_proof_is_refused_without_a_tick_and_the_clock_edges_do_not_panic() {
    // истёкший Pending: FinI через 100 с без tick отвергается
    let mut s = Sim::new(8);
    let (a, b) = s.ids();
    let outs = s.m[0].start(BASE, b, 1);
    let r = outs.iter().find_map(|o| if let Out::Send { bytes, .. } = o { Some(bytes.clone()) } else { None }).unwrap();
    let outs = s.m[1].receive(BASE + 10, 1, &r);
    let ack = outs.iter().find_map(|o| if let Out::Send { bytes, .. } = o { Some(bytes.clone()) } else { None }).unwrap();
    let outs = s.m[0].receive(BASE + 20, 1, &ack);
    let fin_i = outs.iter().find_map(|o| if let Out::Send { bytes, .. } = o { Some(bytes.clone()) } else { None }).unwrap();
    // границы 9999/10000/10001 мс от создания Pending у B (создан в BASE+10)
    for (dt, ok) in [(9_999u64, true), (10_000, false), (10_001, false)] {
        let mut b2 = Machine::new(2, SigningKey::from_bytes(&[2u8; 32]), CTX_NETLAYER_UDP);
        let _ = b2.receive(BASE + 10, 1, &r);
        let out = b2.receive(BASE + 10 + dt, 1, &fin_i);
        assert_eq!(!out.is_empty(), ok, "граница {dt} мс");
    }
    let _ = a;
    // крайнее значение часов: не паникует
    let mut m = machine(1, CTX_NETLAYER_UDP);
    let peer = machine(2, CTX_NETLAYER_UDP).id();
    let _ = m.start(u64::MAX - 10, peer, 1);
    let _ = m.tick(u64::MAX);
}

/// Много случайных сетей: потери, дубли, перестановки, отключения в обе стороны, встречные обмены; после тишины обязаны сойтись.
fn build_random(seed: u64) -> Sim {
    let mut s = Sim::new(seed);
    s.loss = (seed % 7) as f64 * 0.04;
    s.dup = (seed % 5) as f64 * 0.05;
    s.max_delay = 50 + (seed % 9) * 100;
    let mut r = StdRng::seed_from_u64(seed * 31 + 7);
    for _ in 0..r.gen_range(0..4) {
        let from = r.gen_range(0..2);
        let start = r.gen_range(0..40_000u64);
        s.blackouts.push(Blackout { from, start, end: start + r.gen_range(500..25_000u64) });
    }
    s.start(0, 1);
    if r.gen_bool(0.5) {
        s.run_until(r.gen_range(0..2_000u64));
        s.start(1, 2);
    }
    s
}

#[test]
fn random_networks_always_converge_after_delivery_resumes() {
    let mut failures = Vec::new();
    for seed in 0..300u64 {
        let mut s = build_random(seed);
        s.run_until(120_000);
        s.loss = 0.0;
        s.dup = 0.0;
        s.blackouts.clear();
        s.run_until(240_000);
        if let Err(e) = s.check_invariants() {
            failures.push((seed, e));
        } else if !s.converged() {
            failures.push((seed, "не сошлись".into()));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// Случайные сети с повторами старых сообщений противником (запросы, ответы, Fin, KeyConfirm, другие пути) и проверкой инвариантов.
fn replay_run(seeds: std::ops::Range<u64>) {
    let mut failures = Vec::new();
    for seed in seeds {
        let mut s = build_random(seed);
        s.replay = 0.05 + (seed % 4) as f64 * 0.05;
        s.run_until(120_000);
        s.loss = 0.0;
        s.dup = 0.0;
        s.blackouts.clear();
        s.run_until(300_000);
        if let Err(e) = s.check_invariants() {
            failures.push((seed, e));
        } else if !s.converged() {
            failures.push((seed, "не сошлись".into()));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// Быстрый вариант для обычного прогона (30 сетей).
#[test]
fn random_networks_with_replays_converge_and_keep_the_invariants() {
    replay_run(1000..1030);
}

/// Полный вариант: 600 сетей, около 25 минут в отладочной сборке. `cargo test --lib hs3 -- --ignored full_replay`.
#[test]
#[ignore]
fn full_replay_sweep() {
    replay_run(1000..1600);
}
