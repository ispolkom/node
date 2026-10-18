//! Конечный автомат рукопожатия версии 3 (docs/HANDSHAKE_HARDENING.md, 3б–3г) — чистая модель без ввода-вывода и часов: время приходит
//! параметром `now_ms`, сообщения — вызовами `receive`, результат — список действий. Нужна, чтобы проверить сходимость и отказоустойчивость
//! ДО встраивания в узел. Стойкость AEAD заменена в модели MAC (тег `KeyConfirm` — HMAC), бюджеты проверки подписи и ведра токенов не
//! моделируются (это уровень транспорта); пул слотов и пути-кандидаты моделируются.
use super::codec::*;
use super::kdf::{self, Keys};
use ed25519_dalek::SigningKey;
use hmac::{Hmac, Mac};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use sha2::Sha256;
use std::collections::HashMap;
use x25519_dalek::{PublicKey, StaticSecret};

pub type PeerId = [u8; 32];
pub type PathId = u32;

pub const HANDSHAKE_DEADLINE_MS: u64 = 10_000;
pub const LATE_WINDOW_MS: u64 = 30_000;
pub const OLD_RX_GRACE_MS: u64 = 30_000;
pub const OLD_RX_HARD_CAP_MS: u64 = 120_000;
pub const TIME_SKEW_MS: u64 = 300_000;
pub const MAX_PATHS: usize = 4;
pub const MAX_SLOTS: usize = 256;
pub const MAX_SLOTS_PER_IDENTITY: usize = 8;
const RETX_AT_MS: [u64; 3] = [1_000, 3_000, 7_000]; // повторы с отступами 1, 2, 4 секунды от начала
const NEW_TRY_AT_MS: [u64; 2] = [4_000, 8_000]; // свежий запрос (новые Ea и nonce), если ответа нет

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

/// Поколение ключей пары.
#[derive(Clone)]
pub struct Generation {
    pub t: [u8; 32],
    pub role: Role,
    keys: Keys,
    /// Отправка уже идёт под этим поколением (иначе — под прежним).
    pub tx_open: bool,
    pub since_ms: u64,
}

impl Generation {
    /// Тег `KeyConfirm`, который отправляет сторона с ролью `sender` (в модели вместо AES-GCM пустого текста — MAC под её ключом отправки).
    fn tag_of(&self, sender: Role) -> [u8; 16] {
        let key = match sender {
            Role::Initiator => &self.keys.i2r,
            Role::Responder => &self.keys.r2i,
        };
        h16(key, b"keyconfirm", &self.t)
    }
    fn my_tag(&self) -> [u8; 16] {
        self.tag_of(self.role)
    }
    fn peer_tag(&self) -> [u8; 16] {
        self.tag_of(if self.role == Role::Initiator { Role::Responder } else { Role::Initiator })
    }
    /// Ключ отправки AES (для проверок в тестах: у пары он зеркален).
    pub fn send_key(&self) -> [u8; 32] {
        match self.role {
            Role::Initiator => *self.keys.i2r,
            Role::Responder => *self.keys.r2i,
        }
    }
    pub fn recv_key(&self) -> [u8; 32] {
        match self.role {
            Role::Initiator => *self.keys.r2i,
            Role::Responder => *self.keys.i2r,
        }
    }
}

struct Try {
    ea_secret: StaticSecret,
    r: Vec<u8>,
}

enum OutPhase {
    WaitAck,
    WaitFinR { t: [u8; 32], keys: Keys, fin_i: Vec<u8>, ack_path: PathId },
}

struct Outgoing {
    attempt_no: u64,
    tries: Vec<Try>,
    started: u64,
    path: PathId,
    phase: OutPhase,
    retx_done: usize,
    new_tries_done: usize,
}

struct Late {
    attempt_no: u64,
    t: [u8; 32],
    keys: Keys,
    expires: u64,
    path: PathId,
}

enum IState {
    Pending,
    /// Верный `FinI` получен, но `FinR` удерживается: собственный обмен с приоритетом ещё идёт (3б.4, 3б.8).
    Held,
    Completed { fin_r: Vec<u8>, retx_done: usize, until: u64, superseded: bool },
}

struct Incoming {
    h_r: [u8; 32],
    t: [u8; 32],
    keys: Keys,
    a: Vec<u8>,
    created: u64,
    paths: Vec<PathId>,
    state: IState,
}

struct Peer {
    active: Option<Generation>,
    /// Прежнее поколение приёма: (поколение, предел по времени, жёсткий предел).
    old: Option<(Generation, u64, u64)>,
    attempt_ctr: u64,
    committed_attempt: u64,
    outgoing: Option<Outgoing>,
    late: Vec<Late>,
    incoming: Vec<Incoming>,
    /// Повтор `KeyConfirm` инициатора, пока ответчик не подтвердил переход: (путь, T, следующая отправка).
    confirm: Option<(PathId, [u8; 32], u64)>,
    /// Инициатор должен довести обмен до зафиксированного поколения: после тайм-аута начинается новый обмен с нарастающей задержкой.
    want: bool,
    last_path: PathId,
    backoff_ms: u64,
    restart_at: Option<u64>,
}

impl Peer {
    fn new() -> Self {
        Self { active: None, old: None, attempt_ctr: 0, committed_attempt: 0, outgoing: None, late: Vec::new(), incoming: Vec::new(), confirm: None, want: false, last_path: 0, backoff_ms: 1_000, restart_at: None }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    Send { path: PathId, bytes: Vec<u8> },
    /// Поколение зафиксировано (приём под новым ключом разрешён).
    Committed { peer: PeerId, t: [u8; 32], role: Role },
}

pub struct Machine {
    id: [u8; 32],
    sk: SigningKey,
    context: u8,
    rng: StdRng,
    peers: HashMap<PeerId, Peer>,
}

fn h16(key: &[u8; 32], label: &[u8], t: &[u8; 32]) -> [u8; 16] {
    let mut m = Hmac::<Sha256>::new_from_slice(key).unwrap();
    m.update(label);
    m.update(t);
    let full: [u8; 32] = m.finalize().into_bytes().into();
    let mut out = [0u8; 16];
    out.copy_from_slice(&full[..16]);
    out
}

impl Machine {
    pub fn new(seed: u64, signing: SigningKey, context: u8) -> Self {
        let id = crate::util::types::derive_node_id(&signing.verifying_key().to_bytes());
        Self { id, sk: signing, context, rng: StdRng::seed_from_u64(seed), peers: HashMap::new() }
    }

    pub fn id(&self) -> PeerId {
        self.id
    }

    pub fn active(&self, peer: &PeerId) -> Option<&Generation> {
        self.peers.get(peer).and_then(|p| p.active.as_ref())
    }

    /// Поколение, которым узел отправляет данные сейчас: новое, если отправка открыта, иначе прежнее.
    pub fn sending_generation(&self, peer: &PeerId) -> Option<[u8; 32]> {
        let p = self.peers.get(peer)?;
        match (&p.active, &p.old) {
            (Some(a), _) if a.tx_open => Some(a.t),
            (_, Some((o, _, _))) => Some(o.t),
            (Some(a), None) => {
                let _ = a;
                None // первое рукопожатие: данные ждут KeyConfirm
            }
            _ => None,
        }
    }

    /// Принимает ли узел пакеты под поколением `t` (новое или прежнее в пределах срока).
    pub fn accepts(&self, peer: &PeerId, t: &[u8; 32], now: u64) -> bool {
        let Some(p) = self.peers.get(peer) else { return false };
        p.active.as_ref().is_some_and(|a| &a.t == t) || p.old.as_ref().is_some_and(|(o, until, hard)| &o.t == t && now < *until.min(hard))
    }

    pub fn slots_used(&self) -> usize {
        self.peers.values().map(|p| p.incoming.len() + p.late.len()).sum()
    }

    fn random<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0u8; N];
        self.rng.fill_bytes(&mut b);
        b
    }

    fn build_request(&mut self, now: u64, peer: &PeerId) -> Try {
        let ea_secret = StaticSecret::from(self.random::<32>());
        let req = Request {
            context: self.context,
            node_id: self.id,
            signing_pub: self.sk.verifying_key().to_bytes(),
            ea: PublicKey::from(&ea_secret).to_bytes(),
            time_ms: now,
            nonce: self.random::<16>(),
            expected_peer: *peer,
            capabilities: 0,
            jurisdiction: String::new(),
            name: String::new(),
        };
        let r = req.encode(&self.sk);
        Try { ea_secret, r }
    }

    /// Запустить обмен с пиром. Повторный запуск при идущем обмене объединяется с ним.
    pub fn start(&mut self, now: u64, peer: PeerId, path: PathId) -> Vec<Out> {
        if peer == self.id {
            return vec![];
        }
        {
            let p = self.peers.entry(peer).or_insert_with(Peer::new);
            p.want = true;
            p.last_path = path;
            if p.outgoing.is_some() {
                return vec![];
            }
        }
        let t = self.build_request(now, &peer);
        let p = self.peers.get_mut(&peer).unwrap();
        p.attempt_ctr += 1;
        p.restart_at = None;
        let r = t.r.clone();
        p.outgoing = Some(Outgoing { attempt_no: p.attempt_ctr, tries: vec![t], started: now, path, phase: OutPhase::WaitAck, retx_done: 0, new_tries_done: 0 });
        vec![Out::Send { path, bytes: r }]
    }

    pub fn receive(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok((t, ctx)) = parse_header(bytes) else { return vec![] };
        if ctx != self.context {
            return vec![];
        }
        match t {
            MsgType::Request => self.on_request(now, path, bytes),
            MsgType::Ack => self.on_ack(now, path, bytes),
            MsgType::FinI => self.on_fin_i(now, path, bytes),
            MsgType::FinR => self.on_fin_r(now, path, bytes),
            MsgType::KeyConfirm => self.on_key_confirm(now, path, bytes),
        }
    }

    fn time_ok(&self, now: u64, t: u64) -> bool {
        now.abs_diff(t) <= TIME_SKEW_MS
    }

    fn on_request(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok(req) = Request::decode(bytes) else { return vec![] };
        if req.node_id == self.id || !self.time_ok(now, req.time_ms) {
            return vec![];
        }
        let zero = [0u8; 32];
        if req.expected_peer != self.id && !(req.expected_peer == zero && self.context == CTX_NETLAYER_UDP) {
            return vec![];
        }
        let h_r = kdf::hash_request(bytes);
        let peer = req.node_id;
        let slots = self.slots_used();
        let pr = self.peers.entry(peer).or_insert_with(Peer::new);
        if let Some(inc) = pr.incoming.iter_mut().find(|i| i.h_r == h_r) {
            // повтор того же запроса: сохранённый ответ уходит на этот путь; путь становится кандидатом, если есть место, но занятые не вытесняются
            if !inc.paths.contains(&path) && inc.paths.len() < MAX_PATHS {
                inc.paths.push(path);
            }
            return match inc.state {
                IState::Completed { superseded: true, .. } => vec![],
                _ => vec![Out::Send { path, bytes: inc.a.clone() }],
            };
        }
        if slots >= MAX_SLOTS || pr.incoming.len() + pr.late.len() >= MAX_SLOTS_PER_IDENTITY {
            return vec![]; // пул полон: новые запросы отвергаются, идущие обмены не вытесняются
        }
        let eb_secret = StaticSecret::from(self.random::<32>());
        let eb = PublicKey::from(&eb_secret).to_bytes();
        let dh = eb_secret.diffie_hellman(&PublicKey::from(req.ea));
        if !dh.was_contributory() {
            return vec![];
        }
        let nonce = self.random::<16>();
        let ack = Ack {
            context: self.context,
            node_id: self.id,
            signing_pub: self.sk.verifying_key().to_bytes(),
            eb,
            time_ms: now,
            nonce,
            h_r,
            expected_initiator: req.node_id,
            capabilities: 0,
            jurisdiction: String::new(),
            name: String::new(),
        };
        let a = ack.encode(&self.sk);
        let t = kdf::transcript(bytes, &a);
        let Some(keys) = kdf::derive(&t, &dh.to_bytes(), self.context) else { return vec![] };
        let pr = self.peers.get_mut(&peer).unwrap();
        pr.incoming.push(Incoming { h_r, t, keys, a: a.clone(), created: now, paths: vec![path], state: IState::Pending });
        vec![Out::Send { path, bytes: a }]
    }

    fn on_ack(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok(ack) = Ack::decode(bytes) else { return vec![] };
        let peer = ack.node_id;
        if peer == self.id || !self.time_ok(now, ack.time_ms) || ack.expected_initiator != self.id {
            return vec![];
        }
        let Some(p) = self.peers.get_mut(&peer) else { return vec![] };
        let Some(out) = p.outgoing.as_mut() else { return vec![] };
        if !matches!(out.phase, OutPhase::WaitAck) {
            return vec![]; // повторный ответ не создаёт новый FinI
        }
        let Some(tr) = out.tries.iter().find(|t| kdf::hash_request(&t.r) == ack.h_r) else { return vec![] };
        let dh = tr.ea_secret.diffie_hellman(&PublicKey::from(ack.eb));
        if !dh.was_contributory() {
            return vec![];
        }
        let t = kdf::transcript(&tr.r, bytes);
        let Some(keys) = kdf::derive(&t, &dh.to_bytes(), self.context) else { return vec![] };
        let fin_i = Fin { kind: MsgType::FinI, context: self.context, t, mac: kdf::fin_mac(&keys.kc_i, true, &t) }.encode();
        out.phase = OutPhase::WaitFinR { t, keys, fin_i: fin_i.clone(), ack_path: path };
        out.retx_done = 0;
        vec![Out::Send { path, bytes: fin_i }]
    }

    fn on_fin_i(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok(fin) = Fin::decode(bytes) else { return vec![] };
        if fin.kind != MsgType::FinI {
            return vec![];
        }
        let my_id = self.id;
        let context = self.context;
        let found = self.peers.iter().find_map(|(pid, p)| p.incoming.iter().position(|i| i.t == fin.t).map(|ix| (*pid, ix)));
        let Some((peer, ix)) = found else { return vec![] };
        let p = self.peers.get_mut(&peer).unwrap();
        let inc = &p.incoming[ix];
        // MAC проверяется до любых изменений: неверный Fin не меняет состояние
        if !kdf::mac_eq(&kdf::fin_mac(&inc.keys.kc_i, true, &fin.t), &fin.mac) {
            return vec![];
        }
        match &inc.state {
            IState::Completed { fin_r, superseded, .. } => {
                return if *superseded { vec![] } else { vec![Out::Send { path, bytes: fin_r.clone() }] };
            }
            IState::Held => return vec![],
            IState::Pending => {}
        }
        if !inc.paths.contains(&path) {
            return vec![]; // Fin допускается только с пути-кандидата
        }
        // удержание: свой исходящий обмен с приоритетом (меньший node_id) ещё идёт
        let outgoing_in_flight = p.outgoing.is_some();
        if outgoing_in_flight && my_id < peer {
            p.incoming[ix].state = IState::Held;
            return vec![];
        }
        let _ = context;
        self.commit_incoming(now, peer, ix, path)
    }

    fn commit_incoming(&mut self, now: u64, peer: PeerId, ix: usize, path: PathId) -> Vec<Out> {
        let p = self.peers.get_mut(&peer).unwrap();
        let (t, keys) = {
            let i = &p.incoming[ix];
            (i.t, i.keys.clone())
        };
        let fin_r = Fin { kind: MsgType::FinR, context: self.context, t, mac: kdf::fin_mac(&keys.kc_r, false, &t) }.encode();
        // более ранние завершённые обмены этого пира вытеснены
        for (j, other) in p.incoming.iter_mut().enumerate() {
            if j != ix {
                if let IState::Completed { superseded, .. } = &mut other.state {
                    *superseded = true;
                }
            }
        }
        let gen = Generation { t, role: Role::Responder, keys, tx_open: false, since_ms: now };
        if let Some(prev) = p.active.take() {
            p.old = Some((prev, u64::MAX, now + OLD_RX_HARD_CAP_MS));
        }
        p.active = Some(gen);
        p.incoming[ix].state = IState::Completed { fin_r: fin_r.clone(), retx_done: 0, until: now + LATE_WINDOW_MS, superseded: false };
        // собственный исходящий обмен проигран: поздние состояния более старых попыток в этой роли не нужны
        vec![Out::Committed { peer, t, role: Role::Responder }, Out::Send { path, bytes: fin_r }]
    }

    fn on_fin_r(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok(fin) = Fin::decode(bytes) else { return vec![] };
        if fin.kind != MsgType::FinR {
            return vec![];
        }
        let _ = path;
        let found = self.peers.iter().find_map(|(pid, p)| {
            let cur = p.outgoing.as_ref().and_then(|o| match &o.phase {
                OutPhase::WaitFinR { t, keys, .. } if *t == fin.t => Some((o.attempt_no, keys.clone(), o.path)),
                _ => None,
            });
            let late = p.late.iter().find(|l| l.t == fin.t).map(|l| (l.attempt_no, l.keys.clone(), l.path));
            cur.or(late).map(|x| (*pid, x))
        });
        let Some((peer, (attempt_no, keys, back_path))) = found else { return vec![] };
        if !kdf::mac_eq(&kdf::fin_mac(&keys.kc_r, false, &fin.t), &fin.mac) {
            return vec![];
        }
        let p = self.peers.get_mut(&peer).unwrap();
        if attempt_no < p.committed_attempt {
            return vec![]; // поздний Fin вытесненной попытки отвергается независимо от верного MAC
        }
        p.committed_attempt = attempt_no;
        p.want = false;
        p.backoff_ms = 1_000;
        p.restart_at = None;
        let gen = Generation { t: fin.t, role: Role::Initiator, keys, tx_open: true, since_ms: now };
        if let Some(prev) = p.active.take() {
            p.old = Some((prev, now + OLD_RX_GRACE_MS, now + OLD_RX_HARD_CAP_MS));
        }
        p.active = Some(gen.clone());
        p.outgoing = None;
        p.late.retain(|l| l.attempt_no > attempt_no);
        // проигравший встречный обмен (удержанный FinR) отменяется: FinR для него никогда не уйдёт
        p.incoming.retain(|i| !matches!(i.state, IState::Held));
        p.confirm = Some((back_path, fin.t, now + 2_000));
        let kc = key_confirm_frame(self.context, &fin.t, &gen.my_tag());
        vec![Out::Committed { peer, t: fin.t, role: Role::Initiator }, Out::Send { path: back_path, bytes: kc }]
    }

    fn on_key_confirm(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok((_ctx, t, tag)) = parse_key_confirm(bytes) else { return vec![] };
        let context = self.context;
        let mut out = Vec::new();
        for p in self.peers.values_mut() {
            let Some(a) = p.active.as_mut() else { continue };
            if a.t != t || a.peer_tag() != tag {
                continue;
            }
            match a.role {
                Role::Responder => {
                    // первый аутентифицированный пакет инициатора под новым ключом: отправка переходит на новое поколение; ответ — свой KeyConfirm
                    if !a.tx_open {
                        a.tx_open = true;
                        if let Some((_, until, _)) = p.old.as_mut() {
                            *until = now + OLD_RX_GRACE_MS;
                        }
                    }
                    out.push(Out::Send { path, bytes: key_confirm_frame(context, &t, &a.my_tag()) });
                }
                Role::Initiator => {
                    // ответчик подтвердил переход: повторы прекращаются
                    if p.confirm.as_ref().is_some_and(|c| c.1 == t) {
                        p.confirm = None;
                    }
                }
            }
        }
        out
    }

    /// Таймеры: повторы, истечение сроков, снятие удержания.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        let mut out = Vec::new();
        let my_id = self.id;
        let context = self.context;
        let peers: Vec<PeerId> = self.peers.keys().copied().collect();
        for peer in peers {
            // --- исходящий обмен
            let mut new_try: Option<Try> = None;
            {
                let p = self.peers.get_mut(&peer).unwrap();
                if let Some(o) = p.outgoing.as_mut() {
                    let age = now.saturating_sub(o.started);
                    if age >= HANDSHAKE_DEADLINE_MS {
                        if let OutPhase::WaitFinR { t, keys, .. } = &o.phase {
                            p.late.push(Late { attempt_no: o.attempt_no, t: *t, keys: keys.clone(), expires: now + LATE_WINDOW_MS, path: o.path });
                        }
                        p.outgoing = None;
                        if p.want {
                            p.restart_at = Some(now + p.backoff_ms);
                            p.backoff_ms = (p.backoff_ms * 2).min(30_000);
                        }
                    } else {
                        if o.retx_done < RETX_AT_MS.len() && age >= RETX_AT_MS[o.retx_done] {
                            o.retx_done += 1;
                            match &o.phase {
                                OutPhase::WaitAck => {
                                    for t in &o.tries {
                                        out.push(Out::Send { path: o.path, bytes: t.r.clone() });
                                    }
                                }
                                OutPhase::WaitFinR { fin_i, ack_path, .. } => out.push(Out::Send { path: *ack_path, bytes: fin_i.clone() }),
                            }
                        }
                        if matches!(o.phase, OutPhase::WaitAck) && o.new_tries_done < NEW_TRY_AT_MS.len() && age >= NEW_TRY_AT_MS[o.new_tries_done] {
                            o.new_tries_done += 1;
                            new_try = Some(Try { ea_secret: StaticSecret::from([0u8; 32]), r: vec![] }); // заполнитель, заменяется ниже
                        }
                    }
                }
            }
            if new_try.is_some() {
                let t = self.build_request(now, &peer);
                let p = self.peers.get_mut(&peer).unwrap();
                if let Some(o) = p.outgoing.as_mut() {
                    out.push(Out::Send { path: o.path, bytes: t.r.clone() });
                    o.tries.push(t);
                }
            }
            // --- новый обмен после неудачного: нарастающая задержка, пока поколение не зафиксировано
            let restart = {
                let p = self.peers.get_mut(&peer).unwrap();
                match p.restart_at {
                    Some(at) if p.want && p.outgoing.is_none() && now >= at => Some(p.last_path),
                    _ => None,
                }
            };
            if let Some(path) = restart {
                out.extend(self.start(now, peer, path));
            }
            let p = self.peers.get_mut(&peer).unwrap();
            // --- повтор KeyConfirm инициатора: пока ответчик не подтвердил переход (в узле эту роль играют обычный трафик и keepalive)
            if let Some((path, t, next)) = p.confirm.as_mut() {
                if now >= *next {
                    match p.active.as_ref().filter(|a| a.t == *t) {
                        Some(a) => {
                            out.push(Out::Send { path: *path, bytes: key_confirm_frame(context, t, &a.my_tag()) });
                            *next = now + 2_000;
                        }
                        None => p.confirm = None, // поколение вытеснено более новым
                    }
                }
            }
            // --- поздние состояния инициатора
            p.late.retain(|l| now < l.expires);
            // --- входящие
            let mut to_release: Vec<usize> = Vec::new();
            for (ix, inc) in p.incoming.iter_mut().enumerate() {
                match &mut inc.state {
                    IState::Pending | IState::Held => {
                        if now.saturating_sub(inc.created) >= HANDSHAKE_DEADLINE_MS {
                            inc.state = IState::Completed { fin_r: vec![], retx_done: 99, until: 0, superseded: true }; // пометка на удаление
                        }
                    }
                    IState::Completed { fin_r, retx_done, until, superseded } => {
                        // повтор FinR до KeyConfirm: интервалы 1, 2, 4 секунды от фиксации
                        if !*superseded && *retx_done < RETX_AT_MS.len() && now >= (*until - LATE_WINDOW_MS) + RETX_AT_MS[*retx_done] {
                            *retx_done += 1;
                            if let Some(a) = p.active.as_ref().filter(|a| a.t == inc.t && !a.tx_open) {
                                let _ = a;
                                for pa in &inc.paths {
                                    out.push(Out::Send { path: *pa, bytes: fin_r.clone() });
                                }
                            }
                        }
                    }
                }
            }
            p.incoming.retain(|i| match &i.state {
                IState::Completed { until, .. } => now < *until,
                _ => true,
            });
            // --- снятие удержания: приоритетный собственный обмен завершился или истёк
            if p.outgoing.is_none() {
                for (ix, inc) in p.incoming.iter().enumerate() {
                    if matches!(inc.state, IState::Held) {
                        to_release.push(ix);
                    }
                }
            }
            let _ = my_id;
            for ix in to_release {
                // возможны сдвиги индексов после retain: ищем заново по состоянию
                let p = self.peers.get_mut(&peer).unwrap();
                if let Some(ix2) = p.incoming.iter().position(|i| matches!(i.state, IState::Held)) {
                    let path = p.incoming[ix2].paths[0];
                    out.extend(self.commit_incoming(now, peer, ix2, path));
                }
                let _ = ix;
            }
            // --- прежнее поколение приёма
            let p = self.peers.get_mut(&peer).unwrap();
            if let Some((_, until, hard)) = &p.old {
                if now >= (*until).min(*hard) {
                    p.old = None;
                }
            }
        }
        out
    }
}
