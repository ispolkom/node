//! Конечный автомат рукопожатия версии 3 (docs/HANDSHAKE_HARDENING.md, 3б–3г) — чистая модель без ввода-вывода и часов: время приходит
//! параметром `now_ms`, сообщения — вызовами `receive`, результат — список действий. Нужна, чтобы проверить сходимость и отказоустойчивость
//! ДО встраивания в узел. Стойкость AEAD заменена в модели MAC, бюджеты проверки подписи и ведра токенов не моделируются (это уровень
//! транспорта); единый пул слотов, пути-кандидаты и порядок поколений моделируются.
//!
//! **Порядок поколений.** Каждый обмен имеет ключ порядка `(time_ms запроса, node_id инициатора)`; он подписан в запросе и одинаков у обеих сторон.
//! Поколение фиксируется только если ключ его обмена строго больше уже зафиксированного для этой пары (в любой роли): более старый или повторный
//! обмен, даже с верным подтверждением, отвергается. Так задержанные сообщения старых попыток и встречные обмены не могут вернуть прежнее поколение.
//! Фиксация новее отменяет только более старые состояния; более новые попытки не затрагиваются. Время запроса инициатора не меньше `пол + 1`,
//! поэтому честный инициатор не может быть «старее» уже зафиксированного обмена из-за отстающих часов.
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
/// Ключ порядка обмена: (время запроса, номер инициатора).
pub type OrderKey = (u64, [u8; 32]);

pub const HANDSHAKE_DEADLINE_MS: u64 = 10_000;
pub const LATE_WINDOW_MS: u64 = 30_000;
pub const OLD_RX_GRACE_MS: u64 = 30_000;
pub const OLD_RX_HARD_CAP_MS: u64 = 120_000;
pub const TIME_SKEW_MS: u64 = 300_000;
pub const MAX_PATHS: usize = 4;
pub const MAX_SLOTS: usize = 256;
pub const RESERVED_SLOTS: usize = 64; // только для пиров с действующим поколением или собственным обменом
pub const MAX_SLOTS_PER_IDENTITY: usize = 8;
pub const MAX_PENDING_PER_IDENTITY: usize = 4;
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
    /// Отправка уже идёт под этим поколением.
    pub tx_open: bool,
    pub since_ms: u64,
    pub key: OrderKey,
    /// Когда более новое поколение вытеснило это из роли действующего, и когда оно будет снято с приёма.
    replaced_at: Option<u64>,
    retire_after: Option<u64>,
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
    key: OrderKey,
}

enum OutPhase {
    WaitAck,
    WaitFinR { t: [u8; 32], keys: Keys, fin_i: Vec<u8>, ack_path: PathId, key: OrderKey },
}

struct Outgoing {
    tries: Vec<Try>,
    started: u64,
    path: PathId,
    phase: OutPhase,
    retx_done: usize,
    new_tries_done: usize,
}

impl Outgoing {
    fn newest_key(&self) -> OrderKey {
        self.tries.iter().map(|t| t.key).max().unwrap_or((0, [0; 32]))
    }
}

struct Late {
    key: OrderKey,
    t: [u8; 32],
    keys: Keys,
    expires: u64,
    ack_path: PathId,
}

enum IState {
    Pending,
    Completed { fin_r: Vec<u8>, retx_done: usize, until: u64, proof_path: PathId },
}

struct Incoming {
    key: OrderKey,
    h_r: [u8; 32],
    t: [u8; 32],
    keys: Keys,
    a: Vec<u8>,
    created: u64,
    paths: Vec<PathId>,
    state: IState,
}

struct Peer {
    /// Зафиксированные поколения от старых к новым; последнее — действующее.
    gens: Vec<Generation>,
    /// Наибольший ключ порядка зафиксированного обмена (в любой роли).
    floor: Option<OrderKey>,
    last_r_time: u64,
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
        Self { gens: Vec::new(), floor: None, last_r_time: 0, outgoing: None, late: Vec::new(), incoming: Vec::new(), confirm: None, want: false, last_path: 0, backoff_ms: 1_000, restart_at: None }
    }
    fn is_empty(&self) -> bool {
        self.gens.is_empty() && self.outgoing.is_none() && self.late.is_empty() && self.incoming.is_empty() && !self.want && self.confirm.is_none()
    }
    fn slots(&self) -> usize {
        self.incoming.len() + self.late.len() + usize::from(self.outgoing.is_some())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    Send { path: PathId, bytes: Vec<u8> },
    /// Поколение зафиксировано (приём под новым ключом разрешён).
    Committed { peer: PeerId, t: [u8; 32], role: Role, key: OrderKey },
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

    /// Действующее (самое новое зафиксированное) поколение с пиром.
    pub fn active(&self, peer: &PeerId) -> Option<&Generation> {
        self.peers.get(peer).and_then(|p| p.gens.last())
    }

    /// Поколение, которым узел отправляет данные сейчас: самое новое с открытой отправкой; `None` — данные ждут `KeyConfirm`.
    pub fn sending_generation(&self, peer: &PeerId) -> Option<[u8; 32]> {
        self.peers.get(peer)?.gens.iter().rev().find(|g| g.tx_open).map(|g| g.t)
    }

    /// Принимает ли узел пакеты под поколением `t`.
    pub fn accepts(&self, peer: &PeerId, t: &[u8; 32]) -> bool {
        self.peers.get(peer).is_some_and(|p| p.gens.iter().any(|g| &g.t == t))
    }

    pub fn slots_used(&self) -> usize {
        self.peers.values().map(Peer::slots).sum()
    }

    pub fn peer_entries(&self) -> usize {
        self.peers.len()
    }

    fn random<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0u8; N];
        self.rng.fill_bytes(&mut b);
        b
    }

    fn build_request(&mut self, now: u64, peer: &PeerId) -> Try {
        let ea_secret = StaticSecret::from(self.random::<32>());
        let nonce = self.random::<16>();
        let id = self.id;
        let p = self.peers.get_mut(peer).unwrap();
        // время запроса строго растёт и не меньше «пол + 1»: честный инициатор не бывает старее уже зафиксированного обмена
        let floor_t = p.floor.map(|f| f.0).unwrap_or(0);
        let time_ms = now.max(p.last_r_time.saturating_add(1)).max(floor_t.saturating_add(1));
        p.last_r_time = time_ms;
        let req = Request {
            context: self.context,
            node_id: self.id,
            signing_pub: self.sk.verifying_key().to_bytes(),
            ea: PublicKey::from(&ea_secret).to_bytes(),
            time_ms,
            nonce,
            expected_peer: *peer,
            capabilities: 0,
            jurisdiction: String::new(),
            name: String::new(),
        };
        let r = req.encode(&self.sk);
        Try { ea_secret, r, key: (time_ms, id) }
    }

    /// Снять истёкшее; вызывается перед каждой обработкой и таймером (контракт: `now` не убывает).
    fn expire(&mut self, now: u64) {
        for p in self.peers.values_mut() {
            if let Some(o) = p.outgoing.as_ref() {
                if now.saturating_sub(o.started) >= HANDSHAKE_DEADLINE_MS {
                    let o = p.outgoing.take().unwrap();
                    if let OutPhase::WaitFinR { t, keys, ack_path, key, .. } = o.phase {
                        p.late.push(Late { key, t, keys, expires: now.saturating_add(LATE_WINDOW_MS), ack_path });
                    }
                    if p.want {
                        p.restart_at = Some(now.saturating_add(p.backoff_ms));
                        p.backoff_ms = (p.backoff_ms.saturating_mul(2)).min(30_000);
                    }
                }
            }
            p.late.retain(|l| now < l.expires);
            p.incoming.retain(|i| match &i.state {
                IState::Pending => now.saturating_sub(i.created) < HANDSHAKE_DEADLINE_MS,
                IState::Completed { until, .. } => now < *until,
            });
            // прежние поколения: снимаются после срока приёма либо по жёсткому пределу после вытеснения
            let last = p.gens.len().saturating_sub(1);
            let mut ix = 0;
            p.gens.retain(|g| {
                let is_last = ix == last;
                ix += 1;
                is_last || !(g.retire_after.is_some_and(|r| now >= r) || g.replaced_at.is_some_and(|r| now >= r.saturating_add(OLD_RX_HARD_CAP_MS)))
            });
        }
        self.peers.retain(|_, p| !p.is_empty());
    }

    /// Запустить обмен с пиром. Повторный запуск при идущем обмене объединяется с ним.
    pub fn start(&mut self, now: u64, peer: PeerId, path: PathId) -> Vec<Out> {
        if peer == self.id {
            return vec![];
        }
        self.expire(now);
        let slots = self.slots_used();
        {
            let p = self.peers.entry(peer).or_insert_with(Peer::new);
            p.want = true;
            p.last_path = path;
            if p.outgoing.is_some() {
                return vec![];
            }
            if slots >= MAX_SLOTS {
                return vec![]; // пул полон: собственный обмен не вытесняет идущие (повторится по таймеру)
            }
        }
        let t = self.build_request(now, &peer);
        let p = self.peers.get_mut(&peer).unwrap();
        p.restart_at = None;
        let r = t.r.clone();
        p.outgoing = Some(Outgoing { tries: vec![t], started: now, path, phase: OutPhase::WaitAck, retx_done: 0, new_tries_done: 0 });
        vec![Out::Send { path, bytes: r }]
    }

    pub fn receive(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok((t, ctx)) = parse_header(bytes) else { return vec![] };
        if ctx != self.context {
            return vec![];
        }
        self.expire(now);
        match t {
            MsgType::Request => self.on_request(now, path, bytes),
            MsgType::Ack => self.on_ack(now, path, bytes),
            MsgType::FinI => self.on_fin_i(now, path, bytes),
            MsgType::FinR => self.on_fin_r(now, bytes),
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
        let key: OrderKey = (req.time_ms, req.node_id);
        let slots = self.slots_used();
        // повторы и отказы не создают записи о пире: она заводится только вместе с состоянием
        if let Some(p) = self.peers.get_mut(&peer) {
            if let Some(inc) = p.incoming.iter_mut().find(|i| i.h_r == h_r) {
                if !inc.paths.contains(&path) && inc.paths.len() < MAX_PATHS {
                    inc.paths.push(path);
                }
                return vec![Out::Send { path, bytes: inc.a.clone() }];
            }
            if p.floor.is_some_and(|f| key <= f) {
                return vec![]; // старше уже зафиксированного обмена: кандидатом не станет
            }
            let pending = p.incoming.iter().filter(|i| matches!(i.state, IState::Pending)).count();
            if p.slots() >= MAX_SLOTS_PER_IDENTITY || pending >= MAX_PENDING_PER_IDENTITY {
                return vec![];
            }
        }
        let known = self.peers.get(&peer).is_some_and(|p| !p.gens.is_empty() || p.outgoing.is_some());
        let limit = if known { MAX_SLOTS } else { MAX_SLOTS - RESERVED_SLOTS };
        if slots >= limit {
            return vec![]; // пул полон: новые запросы отвергаются, идущие обмены не вытесняются; резерв — для действующих пиров
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
        let p = self.peers.entry(peer).or_insert_with(Peer::new);
        p.incoming.push(Incoming { key, h_r, t, keys, a: a.clone(), created: now, paths: vec![path], state: IState::Pending });
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
        let key = tr.key;
        let fin_i = Fin { kind: MsgType::FinI, context: self.context, t, mac: kdf::fin_mac(&keys.kc_i, true, &t) }.encode();
        out.phase = OutPhase::WaitFinR { t, keys, fin_i: fin_i.clone(), ack_path: path, key };
        out.retx_done = 0;
        vec![Out::Send { path, bytes: fin_i }]
    }

    /// Зафиксировать поколение, если его ключ порядка строго больше зафиксированного. Отменяет только более старые состояния.
    fn commit(&mut self, now: u64, peer: PeerId, key: OrderKey, t: [u8; 32], keys: Keys, role: Role) -> bool {
        let p = self.peers.get_mut(&peer).unwrap();
        if p.floor.is_some_and(|f| key <= f) {
            return false;
        }
        p.floor = Some(key);
        for g in p.gens.iter_mut() {
            g.replaced_at.get_or_insert(now);
        }
        let tx_open = role == Role::Initiator;
        if tx_open {
            for g in p.gens.iter_mut() {
                g.retire_after = Some(now.saturating_add(OLD_RX_GRACE_MS));
            }
        }
        p.gens.push(Generation { t, role, keys, tx_open, since_ms: now, key, replaced_at: None, retire_after: None });
        // состояния старше и равные зафиксированному обмену больше не нужны; более новые попытки сохраняются
        p.late.retain(|l| l.key > key);
        p.incoming.retain(|i| i.key >= key);
        if p.outgoing.as_ref().is_some_and(|o| o.newest_key() <= key) {
            p.outgoing = None;
        }
        if p.outgoing.is_none() {
            p.want = false;
            p.restart_at = None;
            p.backoff_ms = 1_000;
        }
        true
    }

    fn on_fin_i(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok(fin) = Fin::decode(bytes) else { return vec![] };
        if fin.kind != MsgType::FinI {
            return vec![];
        }
        let found = self.peers.iter().find_map(|(pid, p)| p.incoming.iter().position(|i| i.t == fin.t).map(|ix| (*pid, ix)));
        let Some((peer, ix)) = found else { return vec![] };
        let p = self.peers.get_mut(&peer).unwrap();
        let inc = &p.incoming[ix];
        // MAC проверяется до любых изменений: неверный Fin не меняет состояние
        if !kdf::mac_eq(&kdf::fin_mac(&inc.keys.kc_i, true, &fin.t), &fin.mac) {
            return vec![];
        }
        match &inc.state {
            IState::Completed { fin_r, .. } => {
                // идемпотентный повтор: только пока поколение действующее
                let active = p.gens.last().is_some_and(|g| g.t == inc.t);
                return if active { vec![Out::Send { path, bytes: fin_r.clone() }] } else { vec![] };
            }
            IState::Pending => {}
        }
        if !inc.paths.contains(&path) {
            return vec![]; // Fin допускается только с пути-кандидата
        }
        let (key, t, keys) = (inc.key, inc.t, inc.keys.clone());
        if !self.commit(now, peer, key, t, keys.clone(), Role::Responder) {
            // обмен старше зафиксированного: запись снимается, подтверждение не отправляется
            let p = self.peers.get_mut(&peer).unwrap();
            p.incoming.retain(|i| i.t != t);
            return vec![];
        }
        let fin_r = Fin { kind: MsgType::FinR, context: self.context, t, mac: kdf::fin_mac(&keys.kc_r, false, &t) }.encode();
        let p = self.peers.get_mut(&peer).unwrap();
        if let Some(i) = p.incoming.iter_mut().find(|i| i.t == t) {
            i.state = IState::Completed { fin_r: fin_r.clone(), retx_done: 0, until: now.saturating_add(LATE_WINDOW_MS), proof_path: path };
        }
        vec![Out::Committed { peer, t, role: Role::Responder, key }, Out::Send { path, bytes: fin_r }]
    }

    fn on_fin_r(&mut self, now: u64, bytes: &[u8]) -> Vec<Out> {
        let Ok(fin) = Fin::decode(bytes) else { return vec![] };
        if fin.kind != MsgType::FinR {
            return vec![];
        }
        let found = self.peers.iter().find_map(|(pid, p)| {
            let cur = p.outgoing.as_ref().and_then(|o| match &o.phase {
                OutPhase::WaitFinR { t, keys, ack_path, key, .. } if *t == fin.t => Some((*key, keys.clone(), *ack_path)),
                _ => None,
            });
            let late = p.late.iter().find(|l| l.t == fin.t).map(|l| (l.key, l.keys.clone(), l.ack_path));
            cur.or(late).map(|x| (*pid, x))
        });
        let Some((peer, (key, keys, back_path))) = found else { return vec![] };
        if !kdf::mac_eq(&kdf::fin_mac(&keys.kc_r, false, &fin.t), &fin.mac) {
            return vec![];
        }
        if !self.commit(now, peer, key, fin.t, keys, Role::Initiator) {
            return vec![]; // поздний Fin вытесненной попытки отвергается независимо от верного MAC
        }
        let p = self.peers.get_mut(&peer).unwrap();
        p.confirm = Some((back_path, fin.t, now.saturating_add(2_000)));
        let tag = p.gens.last().unwrap().my_tag();
        vec![Out::Committed { peer, t: fin.t, role: Role::Initiator, key }, Out::Send { path: back_path, bytes: key_confirm_frame(self.context, &fin.t, &tag) }]
    }

    fn on_key_confirm(&mut self, now: u64, path: PathId, bytes: &[u8]) -> Vec<Out> {
        let Ok((_ctx, t, tag)) = parse_key_confirm(bytes) else { return vec![] };
        let context = self.context;
        let mut out = Vec::new();
        for p in self.peers.values_mut() {
            let Some(ix) = p.gens.iter().position(|g| g.t == t && g.peer_tag() == tag) else { continue };
            match p.gens[ix].role {
                Role::Responder => {
                    // первый аутентифицированный пакет инициатора под этим поколением: отправка переходит на него; ответ — свой KeyConfirm
                    if !p.gens[ix].tx_open {
                        p.gens[ix].tx_open = true;
                        for g in p.gens.iter_mut().take(ix) {
                            g.retire_after = Some(now.saturating_add(OLD_RX_GRACE_MS));
                        }
                    }
                    let my = p.gens[ix].my_tag();
                    out.push(Out::Send { path, bytes: key_confirm_frame(context, &t, &my) });
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

    /// Таймеры: повторы, новые обмены.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        self.expire(now);
        let mut out = Vec::new();
        let context = self.context;
        let peers: Vec<PeerId> = self.peers.keys().copied().collect();
        for peer in peers {
            let mut new_try = false;
            {
                let Some(p) = self.peers.get_mut(&peer) else { continue };
                if let Some(o) = p.outgoing.as_mut() {
                    let age = now.saturating_sub(o.started);
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
                        new_try = true;
                    }
                }
            }
            if new_try {
                let t = self.build_request(now, &peer);
                let p = self.peers.get_mut(&peer).unwrap();
                if let Some(o) = p.outgoing.as_mut() {
                    out.push(Out::Send { path: o.path, bytes: t.r.clone() });
                    o.tries.push(t);
                }
            }
            // --- новый обмен после неудачного: нарастающая задержка, пока поколение не зафиксировано
            let restart = {
                let Some(p) = self.peers.get_mut(&peer) else { continue };
                match p.restart_at {
                    Some(at) if p.want && p.outgoing.is_none() && now >= at => Some(p.last_path),
                    _ => None,
                }
            };
            if let Some(path) = restart {
                out.extend(self.start(now, peer, path));
            }
            let Some(p) = self.peers.get_mut(&peer) else { continue };
            // --- повтор KeyConfirm инициатора: пока ответчик не подтвердил переход (в узле эту роль играют обычный трафик и keepalive)
            if let Some((path, t, next)) = p.confirm.as_mut() {
                if now >= *next {
                    match p.gens.last().filter(|g| g.t == *t) {
                        Some(g) => {
                            out.push(Out::Send { path: *path, bytes: key_confirm_frame(context, t, &g.my_tag()) });
                            *next = now.saturating_add(2_000);
                        }
                        None => p.confirm = None, // поколение вытеснено более новым
                    }
                }
            }
            // --- повтор FinR ответчика до KeyConfirm: интервалы 1, 2, 4 секунды от фиксации; путь — путь верного FinI
            let unconfirmed = p.gens.last().is_some_and(|g| !g.tx_open);
            let active_t = p.gens.last().map(|g| g.t);
            for inc in p.incoming.iter_mut() {
                if let IState::Completed { fin_r, retx_done, until, proof_path } = &mut inc.state {
                    let committed_at = until.saturating_sub(LATE_WINDOW_MS);
                    if unconfirmed && Some(inc.t) == active_t && *retx_done < RETX_AT_MS.len() && now >= committed_at.saturating_add(RETX_AT_MS[*retx_done]) {
                        *retx_done += 1;
                        out.push(Out::Send { path: *proof_path, bytes: fin_r.clone() });
                    }
                }
            }
        }
        out
    }
}
