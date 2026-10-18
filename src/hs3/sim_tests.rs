//! Модель сети для автомата рукопожатия: две машины, очередь сообщений с задержками, потерей, дублированием и перестановкой, поддельные часы.
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
    commits: Vec<Vec<([u8; 32], Role)>>,
    /// Все отправленные сообщения (для повторов противника) и вероятность повтора случайного из них на каждом шаге.
    seen: Vec<Vec<u8>>,
    replay: f64,
}

fn machine(n: u8, ctx: u8) -> Machine {
    Machine::new(n as u64, SigningKey::from_bytes(&[n; 32]), ctx)
}

impl Sim {
    fn new(seed: u64) -> Self {
        let m = vec![machine(1, CTX_NETLAYER_UDP), machine(2, CTX_NETLAYER_UDP)];
        Self { now: 0, m, q: Vec::new(), rng: StdRng::seed_from_u64(seed), loss: 0.0, dup: 0.0, max_delay: 100, blackouts: vec![], delays: vec![], commits: vec![vec![], vec![]], seen: Vec::new(), replay: 0.0 }
    }

    fn ids(&self) -> (PeerId, PeerId) {
        (self.m[0].id(), self.m[1].id())
    }

    fn apply(&mut self, from: usize, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Committed { t, role, .. } => self.commits[from].push((t, role)),
                Out::Send { path, bytes } => self.send(from, path, bytes),
            }
        }
    }

    fn send(&mut self, from: usize, path: PathId, bytes: Vec<u8>) {
        let to = 1 - from;
        if self.seen.len() < 4000 {
            self.seen.push(bytes.clone());
        }
        if self.blackouts.iter().any(|b| b.from == from && self.now >= b.start && self.now < b.end) {
            return;
        }
        if self.rng.gen_bool(self.loss) {
            return;
        }
        let copies = if self.rng.gen_bool(self.dup) { 2 } else { 1 };
        let ty = parse_header(&bytes).map(|h| h.0).ok();
        for _ in 0..copies {
            let mut d = 10 + self.rng.gen_range(0..=self.max_delay);
            if let Some(t) = ty {
                for (dt, f, s, e, add) in &self.delays {
                    if *dt == t && *f == from && self.now >= *s && self.now < *e {
                        d += *add;
                    }
                }
            }
            self.q.push(Msg { at: self.now + d, to, path, bytes: bytes.clone() });
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

    /// Инварианты безопасности: каждое поколение фиксируется не больше одного раза (нет возврата назад), у стороны нет «чужих» поколений.
    fn check_invariants(&self) -> Result<(), String> {
        for (i, c) in self.commits.iter().enumerate() {
            let mut seen = std::collections::HashSet::new();
            for (t, _) in c {
                if !seen.insert(*t) {
                    return Err(format!("машина {i}: поколение {:02x?} зафиксировано дважды", &t[..3]));
                }
            }
        }
        let (a, b) = self.ids();
        for (i, other) in [(0usize, 1usize), (1, 0)] {
            let peer = if i == 0 { b } else { a };
            if let Some(g) = self.m[i].active(&peer) {
                if !self.commits[other].iter().any(|(t, _)| *t == g.t) {
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
}

#[test]
fn losing_the_last_flight_for_3_9_11_and_31_seconds_still_converges() {
    for loss_ms in [3_000u64, 9_000, 11_000, 31_000] {
        let mut s = Sim::new(loss_ms);
        // пропадают все сообщения ответчика (Ack, FinR) начиная с момента, когда он уже ответил на запрос
        s.blackouts.push(Blackout { from: 1, start: 300, end: 300 + loss_ms });
        s.start(0, 1);
        s.run_until(300 + loss_ms + 90_000);
        assert!(s.converged(), "потеря {loss_ms} мс: стороны не сошлись; commits={:?}/{:?}", s.commits[0].len(), s.commits[1].len());
    }
}

#[test]
fn a_late_finr_of_a_superseded_attempt_is_rejected_by_the_initiator() {
    let mut s = Sim::new(7);
    // FinR первой попытки задерживается на 25 с: инициатор к тому времени успеет зафиксировать вторую попытку
    s.delays.push((MsgType::FinR, 1, 0, 1_000, 25_000));
    s.start(0, 1);
    s.run_until(120_000);
    assert!(s.converged());
    // инициатор зафиксировал ровно одно поколение (не откатывался назад на первую попытку)
    assert_eq!(s.commits[0].len(), 1, "{:?}", s.commits[0].len());
    let (a, b) = s.ids();
    assert_eq!(s.m[0].active(&b).unwrap().t, s.m[1].active(&a).unwrap().t);
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
        }
    }
}

#[test]
fn a_replayed_request_from_another_path_does_not_stop_the_honest_exchange() {
    let mut s = Sim::new(3);
    let (_a, b) = s.ids();
    let outs = s.m[0].start(BASE, b, 1);
    let r = outs.iter().find_map(|o| if let Out::Send { bytes, .. } = o { Some(bytes.clone()) } else { None }).unwrap();
    // честный запрос задержан на 2 с, копия противника с другого пути приходит раньше
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
    let (a, b) = s.ids();
    let t = s.m[1].active(&a).unwrap().t;
    let bad = Fin { kind: MsgType::FinI, context: CTX_NETLAYER_UDP, t, mac: [0u8; 32] }.encode();
    let before = (s.m[1].active(&a).unwrap().t, s.m[1].slots_used());
    let outs = s.m[1].receive(BASE + 6_000, 1, &bad);
    assert!(outs.is_empty());
    assert_eq!(before, (s.m[1].active(&a).unwrap().t, s.m[1].slots_used()));
    let _ = b;
}

/// Много случайных сетей: потери, дубли, перестановки, отключения в обе стороны, встречные обмены; после тишины обязаны сойтись.
#[test]
fn random_networks_always_converge_after_delivery_resumes() {
    let mut failures = Vec::new();
    for seed in 0..300u64 {
        let mut s = Sim::new(seed);
        s.loss = (seed % 7) as f64 * 0.04; // 0…24 %
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
        // после 120 с сеть чистая: потери и отключения прекращаются
        s.run_until(120_000);
        s.loss = 0.0;
        s.dup = 0.0;
        s.blackouts.clear();
        s.run_until(240_000);
        if !s.converged() {
            failures.push(seed);
        }
    }
    assert!(failures.is_empty(), "не сошлись зёрна: {failures:?}");
}

#[cfg(test)]
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
#[ignore]
fn debug_seed() {
    for seed in [145u64, 243, 259] {
        let mut s = build_random(seed);
        s.run_until(120_000);
        s.loss = 0.0;
        s.dup = 0.0;
        s.blackouts.clear();
        s.run_until(240_000);
        let (a, b) = s.ids();
        let fmt = |g: Option<&Generation>| g.map(|g| format!("{:02x?} tx_open={} role={:?}", &g.t[..3], g.tx_open, g.role)).unwrap_or("нет".into());
        eprintln!("seed {seed}: A active: {} | B active: {} | commitsA={:?} commitsB={:?}", fmt(s.m[0].active(&b)), fmt(s.m[1].active(&a)),
            s.commits[0].iter().map(|c| (format!("{:02x?}", &c.0[..3]), c.1)).collect::<Vec<_>>(), s.commits[1].iter().map(|c| (format!("{:02x?}", &c.0[..3]), c.1)).collect::<Vec<_>>());
        eprintln!("   A sending={:?} B sending={:?}", s.m[0].sending_generation(&b).map(|t| format!("{:02x?}", &t[..3])), s.m[1].sending_generation(&a).map(|t| format!("{:02x?}", &t[..3])));
    }
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
