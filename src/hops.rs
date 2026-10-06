//! Цепочки через несколько узлов с шифрованием слоями («луковая» маршрутизация): шаг 4 плана сети.
//!
//! Идея: отправитель строит цепочку из узлов `вход → средний → … → выход`. Каждый узел знает только соседей по цепочке:
//! вход знает отправителя, но не цель; выход знает цель, но не отправителя; средний — ни того ни другого. Содержимое в пути
//! зашифровано слоями: каждый узел снимает свой слой и передаёт дальше ровно такой же по размеру пакет.
//!
//! * **Построение — по одному узлу (телескопом).** Отправитель делает рукопожатие X25519 с первым узлом, потом просит его
//!   «продлить» цепочку к следующему и делает рукопожатие со следующим через первого, и так далее. Ответ каждого узла
//!   **подписан его долгим ключом** (ключ из карточки узла): средний узел не может подсунуть вместо следующего себя.
//!   Ключи слоёв — по одноразовым ключам обеих сторон (пересылаемая запись не расшифруется позже даже при утечке долгих ключей).
//! * **Ячейки фиксированного размера** (`BODY` байт): по размеру не видно ни вида данных, ни места в цепочке. Слой — потоковое
//!   шифрование по номеру ячейки (без привязки к порядку доставки); «моя ли это ячейка» узел узнаёт по короткой метке
//!   (`MAC`) внутри; чужая ячейка (метка не сошлась) идёт дальше как есть.
//! * **Повторы отбрасываются** окном номеров; номера ячеек в обратную сторону разделены по узлам-отправителям, чтобы один и тот
//!   же ключ никогда не шифровал две разные ячейки с одним номером.
//! * Здесь — чистое ядро без сети и без часов: на входе пришедшие пакеты, на выходе пакеты к отправке. Его гоняет проверка
//!   в памяти; в узел оно подключается отдельным слоем (`hops_net`).
use std::collections::HashMap;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey};

pub const PKT_CREATE: u8 = 0xD9;
pub const PKT_CREATED: u8 = 0xDA;
pub const PKT_CELL: u8 = 0xDB;
pub const PKT_DESTROY: u8 = 0xDC;

/// Размер тела ячейки (одинаков на каждом узле цепочки).
pub const BODY: usize = 992;
const HEAD: usize = 15; // [метка 2 нуля][MAC 8][команда 1][поток 2][длина 2]
/// Сколько полезных байт в одной ячейке.
pub const DATA_MAX: usize = BODY - HEAD;

pub const CMD_EXTEND: u8 = 0x01;
pub const CMD_BEGIN: u8 = 0x02;
pub const CMD_DATA: u8 = 0x03;
pub const CMD_END: u8 = 0x04;
pub const CMD_EXTENDED: u8 = 0x81;
pub const CMD_CONNECTED: u8 = 0x82;
/// узел не смог связаться со следующим (его нет среди соседей)
pub const CMD_EXTEND_FAILED: u8 = 0x83;

/// Сколько цепочек узел держит одновременно (остальным — отказ).
pub const MAX_CIRCUITS: usize = 4096;
/// Цепочка без движения дольше этого срока забывается (секунды).
pub const IDLE_SECS: u64 = 600;

pub type NodeId = [u8; 32];

/// Основная связь дополняет пакеты нулями до своего размера и не снимает их: у каждого вида пакета длина известна, лишнее срезаем.
pub fn normalize(pkt: &[u8]) -> Option<&[u8]> {
    let want = match *pkt.first()? {
        PKT_CREATE => 9 + 32,
        PKT_CREATED => 9 + 32 + 64,
        PKT_CELL => 9 + 8 + BODY,
        PKT_DESTROY => 9 + 1,
        _ => return None,
    };
    (pkt.len() >= want).then(|| &pkt[..want])
}

/// Пакет к отправке соседу.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub to: NodeId,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
struct HopKeys {
    kf: [u8; 32],
    kb: [u8; 32],
    mf: [u8; 32],
    mb: [u8; 32],
}

fn transcript(hop_id: &NodeId, x: &[u8; 32], y: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"yandi-hops-v1\0");
    h.update(hop_id);
    h.update(x);
    h.update(y);
    h.finalize().into()
}

fn derive(shared: &[u8; 32], t: &[u8; 32]) -> HopKeys {
    let hk = Hkdf::<Sha256>::new(Some(t), shared);
    let mut okm = [0u8; 128];
    hk.expand(b"yandi-hops-v1 keys", &mut okm).expect("128 bytes fit");
    let part = |i: usize| -> [u8; 32] { okm[i * 32..(i + 1) * 32].try_into().unwrap() };
    HopKeys { kf: part(0), kb: part(1), mf: part(2), mb: part(3) }
}

fn xor_layer(key: &[u8; 32], seq: u64, dir: u8, body: &mut [u8; BODY]) {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(&seq.to_le_bytes());
    h.update(&[dir]);
    let mut ks = [0u8; BODY];
    h.finalize_xof().fill(&mut ks);
    for (b, k) in body.iter_mut().zip(ks.iter()) {
        *b ^= k;
    }
}

fn mac(key: &[u8; 32], seq: u64, dir: u8, body: &[u8; BODY]) -> [u8; 8] {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(&seq.to_le_bytes());
    h.update(&[dir]);
    h.update(&body[10..]);
    h.finalize().as_bytes()[..8].try_into().unwrap()
}

fn seal(key: &[u8; 32], seq: u64, dir: u8, cmd: u8, stream: u16, data: &[u8]) -> Option<[u8; BODY]> {
    if data.len() > DATA_MAX {
        return None;
    }
    let mut b = [0u8; BODY];
    b[10] = cmd;
    b[11..13].copy_from_slice(&stream.to_be_bytes());
    b[13..15].copy_from_slice(&(data.len() as u16).to_be_bytes());
    b[HEAD..HEAD + data.len()].copy_from_slice(data);
    let m = mac(key, seq, dir, &b);
    b[2..10].copy_from_slice(&m);
    Some(b)
}

/// Если ячейка (после снятия слоя) адресована владельцу ключа — её содержимое.
fn recognize(key: &[u8; 32], seq: u64, dir: u8, b: &[u8; BODY]) -> Option<(u8, u16, Vec<u8>)> {
    if b[0] != 0 || b[1] != 0 || mac(key, seq, dir, b)[..] != b[2..10] {
        return None;
    }
    let len = u16::from_be_bytes([b[13], b[14]]) as usize;
    if len > DATA_MAX {
        return None;
    }
    Some((b[10], u16::from_be_bytes([b[11], b[12]]), b[HEAD..HEAD + len].to_vec()))
}

/// Окно уже виденных номеров (повторы и слишком старые отбрасываются).
#[derive(Default, Clone)]
struct Window {
    top: u64,
    bits: u64,
    any: bool,
}

impl Window {
    fn accept(&mut self, n: u64) -> bool {
        if !self.any {
            *self = Window { top: n, bits: 1, any: true };
            return true;
        }
        if n > self.top {
            let shift = n - self.top;
            self.bits = if shift >= 64 { 0 } else { self.bits << shift };
            self.bits |= 1;
            self.top = n;
            return true;
        }
        let back = self.top - n;
        if back >= 64 || self.bits & (1 << back) != 0 {
            return false;
        }
        self.bits |= 1 << back;
        true
    }
}

fn rand8() -> u64 {
    rand::random::<u64>()
}

fn cell_packet(cid: u64, seq: u64, body: &[u8; BODY]) -> Vec<u8> {
    let mut p = Vec::with_capacity(1 + 16 + BODY);
    p.push(PKT_CELL);
    p.extend_from_slice(&cid.to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(body);
    p
}

fn small(kind: u8, cid: u64, rest: &[u8]) -> Vec<u8> {
    let mut p = vec![kind];
    p.extend_from_slice(&cid.to_be_bytes());
    p.extend_from_slice(rest);
    p
}

// ================================================================== узел-посредник и выход

/// Ячейка, дошедшая до своего узла (для верхнего слоя: соединиться с целью, передать данные…).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Cell { handle: Handle, cmd: u8, stream: u16, data: Vec<u8> },
    /// цепочка закрыта (соседом или по простою) — верхний слой убирает свои соединения
    Closed { handle: Handle },
}

/// Как верхний слой называет цепочку на этом узле.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(pub NodeId, pub u64);

struct Relay {
    prev: (NodeId, u64),
    next: Option<(NodeId, u64)>,
    extending: bool,
    keys: HopKeys,
    fwd: Window,
    bwd: HashMap<u32, Window>,
    /// метка узла в номерах обратных ячеек (случайная, у каждого узла своя)
    tag: u32,
    counter: u32,
    last: u64,
}

/// Узел как участник чужих цепочек.
pub struct Router {
    me: NodeId,
    key: SigningKey,
    by_prev: HashMap<(NodeId, u64), Relay>,
    /// (сосед следом, номер на этом звене) → (сосед позади, номер на том звене)
    by_next: HashMap<(NodeId, u64), (NodeId, u64)>,
}

impl Router {
    pub fn new(me: NodeId, key: SigningKey) -> Self {
        Router { me, key, by_prev: HashMap::new(), by_next: HashMap::new() }
    }

    pub fn circuits(&self) -> usize {
        self.by_prev.len()
    }

    /// Принять пакет цепочки от соседа. `now` — секунды (для забывания простаивающих).
    pub fn on_packet(&mut self, from: NodeId, pkt: &[u8], now: u64) -> (Vec<Out>, Vec<Event>) {
        let mut out = vec![];
        let mut ev = vec![];
        let Some(pkt) = normalize(pkt) else { return (out, ev) };
        let cid = u64::from_be_bytes(pkt[1..9].try_into().unwrap());
        match pkt[0] {
            PKT_CREATE if pkt.len() == 9 + 32 => self.on_create(from, cid, pkt[9..41].try_into().unwrap(), now, &mut out),
            PKT_CREATED if pkt.len() == 9 + 32 + 64 => self.on_created(from, cid, &pkt[9..], now, &mut out),
            PKT_CELL if pkt.len() == 9 + 8 + BODY => {
                let seq = u64::from_be_bytes(pkt[9..17].try_into().unwrap());
                let body: [u8; BODY] = pkt[17..].try_into().unwrap();
                self.on_cell(from, cid, seq, body, now, &mut out, &mut ev)
            }
            PKT_DESTROY => self.on_destroy(from, cid, &mut out, &mut ev),
            _ => {}
        }
        (out, ev)
    }

    fn on_create(&mut self, from: NodeId, cid: u64, x: [u8; 32], now: u64, out: &mut Vec<Out>) {
        if self.by_prev.len() >= MAX_CIRCUITS || self.by_prev.contains_key(&(from, cid)) {
            out.push(Out { to: from, bytes: small(PKT_DESTROY, cid, &[0]) });
            return;
        }
        let secret = EphemeralSecret::random_from_rng(rand::thread_rng());
        let y = PublicKey::from(&secret).to_bytes();
        let shared = secret.diffie_hellman(&PublicKey::from(x)).to_bytes();
        let t = transcript(&self.me, &x, &y);
        let sig = self.key.sign(&t).to_bytes();
        self.by_prev.insert(
            (from, cid),
            Relay { prev: (from, cid), next: None, extending: false, keys: derive(&shared, &t), fwd: Window::default(), bwd: HashMap::new(), tag: rand::random(), counter: 0, last: now },
        );
        let mut rest = y.to_vec();
        rest.extend_from_slice(&sig);
        out.push(Out { to: from, bytes: small(PKT_CREATED, cid, &rest) });
    }

    /// Ответ следующего узла на наше «продление» → сообщить отправителю обратной ячейкой.
    fn on_created(&mut self, from: NodeId, cid_out: u64, y_sig: &[u8], now: u64, out: &mut Vec<Out>) {
        let Some(prev) = self.by_next.get(&(from, cid_out)).copied() else { return };
        let Some(r) = self.by_prev.get_mut(&prev) else { return };
        if !r.extending {
            return;
        }
        r.extending = false;
        r.last = now;
        let data = y_sig.to_vec();
        if let Some(o) = Self::originate(r, CMD_EXTENDED, 0, &data) {
            out.push(o);
        }
    }

    fn originate(r: &mut Relay, cmd: u8, stream: u16, data: &[u8]) -> Option<Out> {
        let seq = ((r.tag as u64) << 32) | r.counter as u64;
        r.counter = r.counter.checked_add(1)?;
        let mut body = seal(&r.keys.mb, seq, 1, cmd, stream, data)?;
        xor_layer(&r.keys.kb, seq, 1, &mut body);
        Some(Out { to: r.prev.0, bytes: cell_packet(r.prev.1, seq, &body) })
    }

    #[allow(clippy::too_many_arguments)]
    fn on_cell(&mut self, from: NodeId, cid: u64, seq: u64, mut body: [u8; BODY], now: u64, out: &mut Vec<Out>, ev: &mut Vec<Event>) {
        // вперёд (от отправителя): звено «позади»
        if self.by_prev.contains_key(&(from, cid)) {
            let r = self.by_prev.get_mut(&(from, cid)).unwrap();
            if seq >> 32 != 0 || !r.fwd.accept(seq) {
                return;
            }
            r.last = now;
            xor_layer(&r.keys.kf, seq, 0, &mut body);
            if let Some((cmd, stream, data)) = recognize(&r.keys.mf, seq, 0, &body) {
                if cmd == CMD_EXTEND {
                    if data.len() != 64 || r.next.is_some() || r.extending {
                        return;
                    }
                    let next: NodeId = data[..32].try_into().unwrap();
                    let cid_out = rand8();
                    r.next = Some((next, cid_out));
                    r.extending = true;
                    let prev = r.prev;
                    self.by_next.insert((next, cid_out), prev);
                    out.push(Out { to: next, bytes: small(PKT_CREATE, cid_out, &data[32..]) });
                } else {
                    ev.push(Event::Cell { handle: Handle(from, cid), cmd, stream, data });
                }
            } else if let Some((next, cid_out)) = r.next {
                out.push(Out { to: next, bytes: cell_packet(cid_out, seq, &body) });
            }
            return;
        }
        // назад (от выхода к отправителю): звено «впереди»
        if let Some(prev) = self.by_next.get(&(from, cid)).copied() {
            if let Some(r) = self.by_prev.get_mut(&prev) {
                if !r.bwd.entry((seq >> 32) as u32).or_default().accept(seq & 0xFFFF_FFFF) {
                    return;
                }
                r.last = now;
                xor_layer(&r.keys.kb, seq, 1, &mut body);
                out.push(Out { to: r.prev.0, bytes: cell_packet(r.prev.1, seq, &body) });
            }
        }
    }

    fn on_destroy(&mut self, from: NodeId, cid: u64, out: &mut Vec<Out>, ev: &mut Vec<Event>) {
        if let Some(r) = self.by_prev.remove(&(from, cid)) {
            if let Some((n, c)) = r.next {
                self.by_next.remove(&(n, c));
                out.push(Out { to: n, bytes: small(PKT_DESTROY, c, &[1]) });
            }
            ev.push(Event::Closed { handle: Handle(from, cid) });
        } else if let Some(prev) = self.by_next.remove(&(from, cid)) {
            if self.by_prev.remove(&prev).is_some() {
                out.push(Out { to: prev.0, bytes: small(PKT_DESTROY, prev.1, &[1]) });
                ev.push(Event::Closed { handle: Handle(prev.0, prev.1) });
            }
        }
    }

    /// Пакет «продлить» до `next` не ушёл (нет связи с ним): сообщить отправителю и забыть звено.
    pub fn extend_failed(&mut self, next: NodeId, cid_out: u64) -> Vec<Out> {
        let Some(prev) = self.by_next.remove(&(next, cid_out)) else { return vec![] };
        let Some(r) = self.by_prev.get_mut(&prev) else { return vec![] };
        r.next = None;
        r.extending = false;
        Self::originate(r, CMD_EXTEND_FAILED, 0, &[]).into_iter().collect()
    }

    /// Ответ отправителю с этого узла (выход отвечает на `BEGIN`, присылает данные).
    pub fn reply(&mut self, h: Handle, cmd: u8, stream: u16, data: &[u8]) -> Option<Out> {
        let r = self.by_prev.get_mut(&(h.0, h.1))?;
        Self::originate(r, cmd, stream, data)
    }

    /// Закрыть цепочку с этого узла.
    pub fn close(&mut self, h: Handle) -> Vec<Out> {
        let mut out = vec![];
        let mut ev = vec![];
        self.on_destroy(h.0, h.1, &mut out, &mut ev);
        if out.is_empty() {
            out.push(Out { to: h.0, bytes: small(PKT_DESTROY, h.1, &[1]) });
        }
        out
    }

    /// Забыть цепочки, на которых давно ничего не было.
    pub fn gc(&mut self, now: u64) -> Vec<Out> {
        let old: Vec<(NodeId, u64)> = self.by_prev.iter().filter(|(_, r)| now.saturating_sub(r.last) > IDLE_SECS).map(|(k, _)| *k).collect();
        let mut out = vec![];
        for (from, cid) in old {
            let mut ev = vec![];
            self.on_destroy(from, cid, &mut out, &mut ev);
            out.push(Out { to: from, bytes: small(PKT_DESTROY, cid, &[2]) });
        }
        out
    }
}

// ================================================================== отправитель

#[derive(Clone)]
pub struct PathHop {
    pub id: NodeId,
    /// долгий ключ подписи узла из его карточки: им проверяется, что рукопожатие делает именно он
    pub key: VerifyingKey,
}

#[derive(Debug, PartialEq)]
pub enum ClientEvent {
    /// цепочка построена целиком
    Ready,
    /// пришла ячейка от узла цепочки с номером `hop`
    Cell { hop: usize, cmd: u8, stream: u16, data: Vec<u8> },
    /// цепочка разрушена соседом
    Closed,
    /// построение не удалось
    Failed(&'static str),
}

/// Цепочка со стороны отправителя.
pub struct Circuit {
    path: Vec<PathHop>,
    keys: Vec<HopKeys>,
    pending: Option<(EphemeralSecret, [u8; 32])>,
    cid: u64,
    fwd_seq: u64,
    bwd: HashMap<u32, Window>,
    done: bool,
}

impl Circuit {
    pub fn new(path: Vec<PathHop>) -> Self {
        Circuit { path, keys: vec![], pending: None, cid: rand8(), fwd_seq: 0, bwd: HashMap::new(), done: false }
    }

    /// Номер цепочки на звене к первому узлу (по нему пришедший пакет узнаётся как «наш»).
    pub fn cid(&self) -> u64 {
        self.cid
    }

    pub fn first_hop(&self) -> NodeId {
        self.path[0].id
    }

    pub fn is_ready(&self) -> bool {
        self.done
    }

    pub fn len(&self) -> usize {
        self.path.len()
    }

    /// Начать построение: пакет первому узлу.
    pub fn begin(&mut self) -> Out {
        let (secret, x) = Self::fresh();
        self.pending = Some((secret, x));
        Out { to: self.path[0].id, bytes: small(PKT_CREATE, self.cid, &x) }
    }

    fn fresh() -> (EphemeralSecret, [u8; 32]) {
        let secret = EphemeralSecret::random_from_rng(rand::thread_rng());
        let x = PublicKey::from(&secret).to_bytes();
        (secret, x)
    }

    fn finish_handshake(&mut self, y_sig: &[u8]) -> Result<(), &'static str> {
        let (secret, x) = self.pending.take().ok_or("unexpected")?;
        let idx = self.keys.len();
        let hop = self.path.get(idx).ok_or("unexpected")?;
        if y_sig.len() != 96 {
            return Err("bad reply");
        }
        let y: [u8; 32] = y_sig[..32].try_into().unwrap();
        let sig = Signature::from_bytes(y_sig[32..].try_into().unwrap());
        let t = transcript(&hop.id, &x, &y);
        hop.key.verify(&t, &sig).map_err(|_| "bad signature")?;
        let shared = secret.diffie_hellman(&PublicKey::from(y)).to_bytes();
        self.keys.push(derive(&shared, &t));
        Ok(())
    }

    /// Следующий шаг построения: ячейка «продли до следующего» последнему готовому узлу — или готово.
    fn next_step(&mut self) -> Result<(Vec<Out>, Vec<ClientEvent>), &'static str> {
        if self.keys.len() == self.path.len() {
            self.done = true;
            return Ok((vec![], vec![ClientEvent::Ready]));
        }
        let (secret, x) = Self::fresh();
        self.pending = Some((secret, x));
        let mut data = self.path[self.keys.len()].id.to_vec();
        data.extend_from_slice(&x);
        let o = self.send_to(self.keys.len() - 1, CMD_EXTEND, 0, &data).ok_or("cell")?;
        Ok((vec![o], vec![]))
    }

    /// Принять пакет от первого узла.
    pub fn on_packet(&mut self, from: NodeId, pkt: &[u8]) -> (Vec<Out>, Vec<ClientEvent>) {
        let fail = |e| (vec![], vec![ClientEvent::Failed(e)]);
        let Some(pkt) = normalize(pkt) else { return (vec![], vec![]) };
        if from != self.path[0].id || u64::from_be_bytes(pkt[1..9].try_into().unwrap()) != self.cid {
            return (vec![], vec![]);
        }
        match pkt[0] {
            PKT_CREATED if self.keys.is_empty() => {
                if let Err(e) = self.finish_handshake(&pkt[9..]) {
                    return fail(e);
                }
                self.next_step().unwrap_or_else(fail)
            }
            PKT_CELL if pkt.len() == 9 + 8 + BODY => {
                let seq = u64::from_be_bytes(pkt[9..17].try_into().unwrap());
                let mut body: [u8; BODY] = pkt[17..].try_into().unwrap();
                for i in 0..self.keys.len() {
                    xor_layer(&self.keys[i].kb, seq, 1, &mut body);
                    if let Some((cmd, stream, data)) = recognize(&self.keys[i].mb, seq, 1, &body) {
                        if !self.bwd.entry((seq >> 32) as u32).or_default().accept(seq & 0xFFFF_FFFF) {
                            return (vec![], vec![]);
                        }
                        if cmd == CMD_EXTEND_FAILED && i + 1 == self.keys.len() && self.pending.is_some() {
                            self.pending = None;
                            return fail("next hop unreachable");
                        }
                        if cmd == CMD_EXTENDED && i + 1 == self.keys.len() && self.pending.is_some() {
                            if let Err(e) = self.finish_handshake(&data) {
                                return fail(e);
                            }
                            return self.next_step().unwrap_or_else(fail);
                        }
                        return (vec![], vec![ClientEvent::Cell { hop: i, cmd, stream, data }]);
                    }
                }
                (vec![], vec![])
            }
            PKT_DESTROY => (vec![], vec![ClientEvent::Closed]),
            _ => (vec![], vec![]),
        }
    }

    /// Ячейка узлу цепочки с номером `hop` (обычно последнему — выходу).
    pub fn send_to(&mut self, hop: usize, cmd: u8, stream: u16, data: &[u8]) -> Option<Out> {
        let keys = self.keys.get(hop)?.clone();
        let seq = self.fwd_seq;
        self.fwd_seq = self.fwd_seq.checked_add(1)?;
        let mut body = seal(&keys.mf, seq, 0, cmd, stream, data)?;
        for i in (0..=hop).rev() {
            xor_layer(&self.keys[i].kf, seq, 0, &mut body);
        }
        Some(Out { to: self.path[0].id, bytes: cell_packet(self.cid, seq, &body) })
    }

    pub fn send_exit(&mut self, cmd: u8, stream: u16, data: &[u8]) -> Option<Out> {
        let last = self.path.len().checked_sub(1)?;
        if !self.done {
            return None;
        }
        self.send_to(last, cmd, stream, data)
    }

    pub fn destroy(&self) -> Out {
        Out { to: self.path[0].id, bytes: small(PKT_DESTROY, self.cid, &[1]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Net {
        routers: HashMap<NodeId, Router>,
        keys: HashMap<NodeId, VerifyingKey>,
        /// всё, что видели «провода» (пакет, откуда, куда) — для проверок наблюдателя
        wire: Vec<(NodeId, NodeId, Vec<u8>)>,
        events: Vec<(NodeId, Event)>,
    }

    fn id(n: u8) -> NodeId {
        [n; 32]
    }

    impl Net {
        fn new(nodes: &[u8]) -> Net {
            let mut routers = HashMap::new();
            let mut keys = HashMap::new();
            for &n in nodes {
                let sk = SigningKey::from_bytes(&[n; 32]);
                keys.insert(id(n), sk.verifying_key());
                routers.insert(id(n), Router::new(id(n), sk));
            }
            Net { routers, keys, wire: vec![], events: vec![] }
        }
        fn path(&self, ns: &[u8]) -> Vec<PathHop> {
            ns.iter().map(|&n| PathHop { id: id(n), key: self.keys[&id(n)] }).collect()
        }
        /// прогнать пакеты, пока есть что передавать; пакеты первому узлу — от отправителя `CLIENT`
        fn run(&mut self, c: &mut Circuit, start: Vec<Out>) -> Vec<ClientEvent> {
            let client = id(0xC0);
            let mut q: VecDeque<(NodeId, Out)> = start.into_iter().map(|o| (client, o)).collect();
            let mut cev = vec![];
            let mut now = 0;
            while let Some((from, o)) = q.pop_front() {
                now += 1;
                self.wire.push((from, o.to, o.bytes.clone()));
                if o.to == client {
                    let (outs, evs) = c.on_packet(from, &o.bytes);
                    cev.extend(evs);
                    q.extend(outs.into_iter().map(|x| (client, x)));
                    continue;
                }
                let Some(r) = self.routers.get_mut(&o.to) else { continue };
                let (outs, evs) = r.on_packet(from, &o.bytes, now);
                for e in evs {
                    self.events.push((o.to, e));
                }
                q.extend(outs.into_iter().map(|x| (o.to, x)));
            }
            cev
        }
        fn build(&mut self, ns: &[u8]) -> (Circuit, Vec<ClientEvent>) {
            let mut c = Circuit::new(self.path(ns));
            let first = c.begin();
            let ev = self.run(&mut c, vec![first]);
            (c, ev)
        }
        /// выход (последний узел) отвечает
        fn exit_reply(&mut self, c: &mut Circuit, exit: u8, h: Handle, cmd: u8, stream: u16, data: &[u8]) -> Vec<ClientEvent> {
            let o = self.routers.get_mut(&id(exit)).unwrap().reply(h, cmd, stream, data).unwrap();
            // ответ идёт назад: от выхода к отправителю через узлы
            let mut q: VecDeque<(NodeId, Out)> = VecDeque::new();
            q.push_back((id(exit), o));
            let client = id(0xC0);
            let mut cev = vec![];
            while let Some((from, o)) = q.pop_front() {
                self.wire.push((from, o.to, o.bytes.clone()));
                if o.to == client {
                    let (outs, evs) = c.on_packet(from, &o.bytes);
                    cev.extend(evs);
                    assert!(outs.is_empty());
                    continue;
                }
                let (outs, _) = self.routers.get_mut(&o.to).unwrap().on_packet(from, &o.bytes, 1);
                q.extend(outs.into_iter().map(|x| (o.to, x)));
            }
            cev
        }
    }

    #[test]
    fn a_three_hop_circuit_is_built_and_data_travels_both_ways_but_only_the_exit_reads_it() {
        let mut net = Net::new(&[1, 2, 3]);
        let (mut c, ev) = net.build(&[1, 2, 3]);
        assert_eq!(ev, vec![ClientEvent::Ready]);
        assert!(c.is_ready());

        let secret = b"BEGIN example.org:443";
        let o = c.send_exit(CMD_BEGIN, 7, secret).unwrap();
        net.events.clear();
        net.run(&mut c, vec![o]);
        assert_eq!(net.events.len(), 1, "only one node got the cell");
        let (who, e) = net.events[0].clone();
        assert_eq!(who, id(3), "the exit");
        let Event::Cell { handle, cmd, stream, data } = e else { panic!() };
        assert_eq!((cmd, stream, data.as_slice()), (CMD_BEGIN, 7, &secret[..]));

        // назад
        let ev = net.exit_reply(&mut c, 3, handle, CMD_DATA, 7, b"hello from the web");
        assert_eq!(ev, vec![ClientEvent::Cell { hop: 2, cmd: CMD_DATA, stream: 7, data: b"hello from the web".to_vec() }]);

        // наблюдатель на любом проводе не видит открытого содержимого, а все ячейки одного размера
        for (_, _, bytes) in &net.wire {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret));
            assert!(!bytes.windows(18).any(|w| w == b"hello from the web"));
            assert!(bytes.len() == 9 + 8 + BODY || bytes[0] == PKT_CREATE || bytes[0] == PKT_CREATED);
        }
        // средний и входной узлы не видели ничего, что адресовано выходу
        assert!(net.events.iter().all(|(w, _)| *w == id(3)));
    }

    #[test]
    fn the_same_cell_looks_different_on_every_link() {
        let mut net = Net::new(&[1, 2, 3]);
        let (mut c, _) = net.build(&[1, 2, 3]);
        net.wire.clear();
        let o = c.send_exit(CMD_DATA, 1, b"same").unwrap();
        net.run(&mut c, vec![o]);
        let cells: Vec<&Vec<u8>> = net.wire.iter().filter(|(_, _, b)| b[0] == PKT_CELL).map(|(_, _, b)| b).collect();
        assert_eq!(cells.len(), 3);
        assert_ne!(cells[0][17..], cells[1][17..], "a layer was peeled: bodies differ");
        assert_ne!(cells[1][17..], cells[2][17..]);
        assert_ne!(cells[0][1..9], cells[1][1..9], "numbers differ link by link");
    }

    #[test]
    fn replayed_and_tampered_cells_are_dropped() {
        let mut net = Net::new(&[1, 2, 3]);
        let (mut c, _) = net.build(&[1, 2, 3]);
        let o = c.send_exit(CMD_DATA, 1, b"once").unwrap();
        net.events.clear();
        net.run(&mut c, vec![o.clone()]);
        assert_eq!(net.events.len(), 1);
        net.run(&mut c, vec![o.clone()]);
        assert_eq!(net.events.len(), 1, "a replay is not delivered twice");
        // порча одного байта: метка не сошлась ни у кого, ячейка уходит в пустоту и до выхода не доходит
        let mut bad = c.send_exit(CMD_DATA, 1, b"twice").unwrap();
        bad.bytes[40] ^= 1;
        net.run(&mut c, vec![bad]);
        assert_eq!(net.events.len(), 1, "a damaged cell is not delivered");
        let good = c.send_exit(CMD_DATA, 1, b"thrice").unwrap();
        net.run(&mut c, vec![good]);
        assert_eq!(net.events.len(), 2, "the circuit still works");
    }

    #[test]
    fn a_middle_node_cannot_pose_as_the_next_one() {
        // узел 2 «продлевает» цепочку не к настоящему 3, а подсовывает своё рукопожатие
        let mut net = Net::new(&[1, 2, 3]);
        let impostor = SigningKey::from_bytes(&[0x77; 32]);
        net.routers.insert(id(3), Router::new(id(3), impostor)); // под номером 3 сидит узел с другим ключом
        let (_, ev) = net.build(&[1, 2, 3]);
        assert_eq!(ev, vec![ClientEvent::Failed("bad signature")]);
    }

    #[test]
    fn destroy_travels_along_the_circuit_and_the_exit_learns_about_it() {
        let mut net = Net::new(&[1, 2, 3]);
        let (mut c, _) = net.build(&[1, 2, 3]);
        net.events.clear();
        let d = c.destroy();
        net.run(&mut c, vec![d]);
        assert!(net.events.iter().any(|(w, e)| *w == id(3) && matches!(e, Event::Closed { .. })));
        assert_eq!(net.routers.values().map(|r| r.circuits()).sum::<usize>(), 0);
    }

    #[test]
    fn idle_circuits_are_forgotten_and_the_table_is_bounded() {
        let mut r = Router::new(id(1), SigningKey::from_bytes(&[1; 32]));
        let (_s, x) = Circuit::fresh();
        r.on_packet(id(9), &small(PKT_CREATE, 5, &x), 100);
        assert_eq!(r.circuits(), 1);
        assert!(r.gc(100 + IDLE_SECS).is_empty());
        assert!(!r.gc(100 + IDLE_SECS + 1).is_empty());
        assert_eq!(r.circuits(), 0);
        for i in 0..MAX_CIRCUITS as u64 {
            r.on_packet(id(9), &small(PKT_CREATE, i, &x), 0);
        }
        let (out, _) = r.on_packet(id(9), &small(PKT_CREATE, u64::MAX, &x), 0);
        assert_eq!(out[0].bytes[0], PKT_DESTROY, "no room — a refusal, not a crash");
    }

    #[test]
    fn the_replay_window_accepts_late_but_fresh_numbers_once() {
        let mut w = Window::default();
        assert!(w.accept(10));
        assert!(w.accept(8), "reordered but new");
        assert!(!w.accept(8), "seen");
        assert!(!w.accept(10));
        assert!(w.accept(100));
        assert!(!w.accept(10), "too old now");
        assert!(w.accept(99));
    }

    #[test]
    fn an_unreachable_next_hop_is_reported_to_the_sender() {
        let mut net = Net::new(&[1, 2]);
        net.keys.insert(id(9), SigningKey::from_bytes(&[9; 32]).verifying_key()); // 9 — никого нет, но ключ известен отправителю
        let mut c = Circuit::new(net.path(&[1, 2, 9]));
        let first = c.begin();
        // узел 2 не смог отправить CREATE узлу 9: ядро сети в тесте сообщает об этом
        let mut ev = vec![];
        let mut q: VecDeque<(NodeId, Out)> = VecDeque::from([(id(0xC0), first)]);
        while let Some((from, o)) = q.pop_front() {
            if o.to == id(0xC0) {
                let (outs, e) = c.on_packet(from, &o.bytes);
                ev.extend(e);
                q.extend(outs.into_iter().map(|x| (id(0xC0), x)));
                continue;
            }
            if o.to == id(9) {
                assert_eq!(o.bytes[0], PKT_CREATE);
                let cid = u64::from_be_bytes(o.bytes[1..9].try_into().unwrap());
                let outs = net.routers.get_mut(&from).unwrap().extend_failed(id(9), cid);
                q.extend(outs.into_iter().map(|x| (from, x)));
                continue;
            }
            let (outs, _) = net.routers.get_mut(&o.to).unwrap().on_packet(from, &o.bytes, 1);
            q.extend(outs.into_iter().map(|x| (o.to, x)));
        }
        assert_eq!(ev, vec![ClientEvent::Failed("next hop unreachable")]);
    }

    #[test]
    fn packets_padded_with_zeros_by_the_link_are_understood() {
        let mut net = Net::new(&[1, 2, 3]);
        let mut c = Circuit::new(net.path(&[1, 2, 3]));
        // как основная связь: каждый пакет дополнен нулями
        let pad = |mut o: Out| {
            o.bytes.extend_from_slice(&[0u8; 37]);
            o
        };
        let client = id(0xC0);
        let mut q: VecDeque<(NodeId, Out)> = VecDeque::from([(client, pad(c.begin()))]);
        let mut ready = false;
        while let Some((from, o)) = q.pop_front() {
            if o.to == client {
                let (outs, evs) = c.on_packet(from, &o.bytes);
                ready |= evs.contains(&ClientEvent::Ready);
                q.extend(outs.into_iter().map(|x| (client, pad(x))));
            } else {
                let (outs, _) = net.routers.get_mut(&o.to).unwrap().on_packet(from, &o.bytes, 1);
                q.extend(outs.into_iter().map(|x| (o.to, pad(x))));
            }
        }
        assert!(ready);
    }

    #[test]
    fn garbage_packets_do_nothing() {
        let mut r = Router::new(id(1), SigningKey::from_bytes(&[1; 32]));
        for p in [vec![], vec![PKT_CELL], vec![PKT_CELL; 100], vec![PKT_CREATE; 10], vec![0xFF; 2000], vec![PKT_CREATED; 105]] {
            let (o, e) = r.on_packet(id(9), &p, 0);
            assert!(o.is_empty() && e.is_empty());
        }
    }

    #[test]
    fn a_long_payload_is_refused_not_cut() {
        let mut net = Net::new(&[1, 2]);
        let (mut c, ev) = net.build(&[1, 2]);
        assert_eq!(ev, vec![ClientEvent::Ready]);
        assert!(c.send_exit(CMD_DATA, 1, &vec![0u8; DATA_MAX + 1]).is_none());
        assert!(c.send_exit(CMD_DATA, 1, &vec![7u8; DATA_MAX]).is_some());
    }
}
