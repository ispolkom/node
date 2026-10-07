//! Цепочки через несколько узлов в живом узле: подключение чистого ядра (`hops`) к связи, выходу в интернет и SOCKS-прокси.
//!
//! * **Каждый узел — посредник** для чужих цепочек (ограничено числом цепочек) и **выход**, если владелец разрешил: выход
//!   открывает соединение с целью только по правилам (`exit_policy`: адрес в интернете, не домашняя сеть) и **только на веб-порты**
//!   (80, 443; для проверок `YANDI_EXIT_PORTS=any`). Выход не знает, кто отправитель, поэтому «только доверенные» означает:
//!   цепочка пришла от доверенного узла.
//! * **Отправитель** строит цепочку `вход → средний → выход` по каталогу карточек (выход — из очереди `exit_select`, остальные —
//!   любые узлы с карточкой и живой связью), ключи узлов берёт из карточек. Одна цепочка обслуживает несколько соединений и
//!   меняется каждые 10 минут.
//! * Данные соединения идут ячейками; каждая несёт номер, чтобы приёмник собрал поток по порядку, даже если ячейки пришли вразброс.
//!   Управления потоком (окон) пока нет: быстрый источник упирается в очередь отправки.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ed25519_dalek::{SigningKey, VerifyingKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::hops::{self, Circuit, ClientEvent, Event, Handle, NodeId, Out, PathHop, Router};
use crate::netlayer::transport::P2PTransport;
use crate::util::HashId;

/// Сколько байт потока в одной ячейке (4 байта — номер ячейки потока).
const CHUNK: usize = hops::DATA_MAX - 4;
const MAX_STREAMS_PER_CIRCUIT: usize = 16;
const CIRCUIT_LIFE_SECS: u64 = 600;
const BEGIN_TIMEOUT: Duration = Duration::from_secs(20);
const BUILD_TIMEOUT: Duration = Duration::from_secs(25);
/// Сколько новых соединений выход открывает по одной цепочке в минуту.
const BEGINS_PER_MINUTE: u32 = 30;

pub const END_REFUSED: u8 = 2;
pub const END_FAILED: u8 = 5;

/// Общий секрет «своих устройств» этого узла: создаётся один раз, лежит в папке данных (0600). Его знает владелец (получает при
/// сопряжении устройства); по нему узел отличает своё устройство от чужого и пускает его как в личный шлюз: любые порты,
/// никаких правил «выхода для всех». Домашняя сеть и адрес самого компьютера всё равно закрыты.
pub fn device_secret() -> std::io::Result<[u8; 32]> {
    let path = crate::util::data_dir::data_dir().join("device_secret");
    // first use creates it exactly once, even when several callers (or nodes sharing the folder) ask at the same moment
    let bytes = crate::util::private_file::read_or_create_private(
        &path,
        &|| {
            let mut a = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut a);
            hex::encode(a).into_bytes()
        },
        &|b| std::str::from_utf8(b).ok().and_then(|s| hex::decode(s.trim()).ok()).map_or(false, |v| v.len() == 32),
    )?;
    let decoded = hex::decode(String::from_utf8_lossy(&bytes).trim()).map_err(std::io::Error::other)?;
    <[u8; 32]>::try_from(decoded).map_err(|_| std::io::Error::other("device secret has the wrong length"))
}

/// Метка своего устройства под просьбой «соединиться с `цель`».
fn personal_tag(secret: &[u8; 32], target: &str) -> String {
    let mut h = blake3::Hasher::new_keyed(secret);
    h.update(b"yandi-personal-begin-v1\0");
    h.update(target.as_bytes());
    hex::encode(&h.finalize().as_bytes()[..16])
}

/// Разобрать просьбу `BEGIN`: `хост:порт` (общий выход) или `P:<метка>:хост:порт` (своё устройство).
/// Возвращает цель и признак «метка верна» (`None` в признаке — метки не было).
fn parse_begin(data: &str, secret: Option<&[u8; 32]>) -> (String, Option<bool>) {
    if let Some(rest) = data.strip_prefix("P:") {
        if let Some((tag, target)) = rest.split_once(':') {
            let ok = secret.map(|s| personal_tag(s, target) == tag).unwrap_or(false);
            return (target.to_string(), Some(ok));
        }
    }
    (data.to_string(), None)
}

fn web_port(port: u16) -> bool {
    std::env::var("YANDI_EXIT_PORTS").map(|v| v == "any").unwrap_or(false) || port == 80 || port == 443
}

/// Собирает поток из ячеек, пришедших в любом порядке.
#[derive(Default)]
struct Reorder {
    next: u32,
    held: BTreeMap<u32, Vec<u8>>,
}

impl Reorder {
    fn push(&mut self, seq: u32, data: Vec<u8>) -> Vec<Vec<u8>> {
        // слишком далеко вперёд — отбрасываем (не копим память по чужой прихоти)
        if seq < self.next || seq - self.next > 256 {
            return vec![];
        }
        self.held.insert(seq, data);
        let mut ready = vec![];
        while let Some(d) = self.held.remove(&self.next) {
            ready.push(d);
            self.next += 1;
        }
        ready
    }
}

fn with_seq(seq: u32, data: &[u8]) -> Vec<u8> {
    let mut v = seq.to_be_bytes().to_vec();
    v.extend_from_slice(data);
    v
}

fn split_seq(d: &[u8]) -> Option<(u32, Vec<u8>)> {
    (d.len() >= 4).then(|| (u32::from_be_bytes(d[..4].try_into().unwrap()), d[4..].to_vec()))
}

/// Every cell for the send queue goes through here so the queue's size is known.
fn push_out(tx: &mpsc::UnboundedSender<Out>, o: Out) -> Result<(), ()> {
    let n = o.bytes.len();
    OUT_QUEUED.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    tx.send(o).map_err(|_| {
        OUT_QUEUED.fetch_sub(n, std::sync::atomic::Ordering::Relaxed);
    })
}

// ---------------------------------------------------------------- состояние

/// Most bytes of an exit's data one client stream may hold for a slow application before the stream is closed.
const CLIENT_STREAM_QUEUE_MAX: usize = 2 * 1024 * 1024;
/// A circuit with open streams is kept this many times longer than an idle one before it is retired.
const BUSY_CIRCUIT_FACTOR: u64 = 6;

/// Most bytes one exit stream may have waiting for its (possibly slow or silent) destination before the stream is closed.
const STREAM_QUEUE_MAX: usize = 2 * 1024 * 1024;
/// Bytes waiting in the shared send queue above which exits stop reading from their destinations (backpressure).
const OUT_QUEUE_HIGH: usize = 8 * 1024 * 1024;
static OUT_QUEUED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct ExitStream {
    to_target: mpsc::UnboundedSender<Option<Vec<u8>>>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    reorder: Reorder,
}

struct ExitCircuit {
    streams: HashMap<u16, ExitStream>,
    begins: (u64, u32),
}

enum ToApp {
    Data(Vec<u8>),
    End,
}

struct ClientStream {
    connected: Option<oneshot::Sender<Result<(), u8>>>,
    to_app: mpsc::UnboundedSender<ToApp>,
    /// bytes handed to the application side and not yet delivered to it
    pending: Arc<std::sync::atomic::AtomicUsize>,
    reorder: Reorder,
}

struct ClientCircuit {
    circuit: Circuit,
    exit: NodeId,
    /// по какому маршруту построена (общий выход или заданный путь) — чтобы переиспользовать подходящую цепочку
    route: String,
    born: u64,
    next_stream: u16,
    streams: HashMap<u16, ClientStream>,
    ready: Option<oneshot::Sender<Result<(), &'static str>>>,
    dead: bool,
}

pub struct Hops {
    transport: Arc<P2PTransport>,
    me: NodeId,
    router: Mutex<Router>,
    out: mpsc::UnboundedSender<Out>,
    exit_circuits: Mutex<HashMap<Handle, ExitCircuit>>,
    clients: Mutex<HashMap<u64, Arc<Mutex<ClientCircuit>>>>,
}

fn cell() -> &'static OnceLock<Arc<Hops>> {
    static H: OnceLock<Arc<Hops>> = OnceLock::new();
    &H
}

fn now() -> u64 {
    crate::network_offers::now_secs()
}

/// Включить цепочки на этом узле (один раз при запуске).
pub fn start(transport: Arc<P2PTransport>, me: NodeId, key: SigningKey) {
    let (tx, rx) = mpsc::unbounded_channel::<Out>();
    let rx = Arc::new(tokio::sync::Mutex::new(rx));
    let h = Arc::new(Hops { transport: transport.clone(), me, router: Mutex::new(Router::new(me, key)), out: tx, exit_circuits: Default::default(), clients: Default::default() });
    if cell().set(h.clone()).is_err() {
        return;
    }
    // единая очередь отправки: ячейки одной цепочки уходят в том порядке, в каком созданы
    let hh = h.clone();
    crate::supervisor::supervise("hops_sender", crate::supervisor::Policy::restart(), move || {
        let (rx, transport, hh) = (rx.clone(), transport.clone(), hh.clone());
        async move {
        let mut rx = rx.lock().await;
        while let Some(o) = rx.recv().await {
            OUT_QUEUED.fetch_sub(o.bytes.len(), std::sync::atomic::Ordering::Relaxed);
            // one stalled neighbour must not stop every circuit: a send gets a deadline
            let sent = tokio::time::timeout(Duration::from_secs(5), transport.send_encrypted(HashId(o.to), &o.bytes)).await.unwrap_or_else(|_| Err("send timeout".to_string()));
            if sent.is_err() && o.bytes.first() == Some(&hops::PKT_CREATE) && o.bytes.len() >= 9 {
                let cid = u64::from_be_bytes(o.bytes[1..9].try_into().unwrap());
                let outs = hh.router.lock().unwrap_or_else(|e| e.into_inner()).extend_failed(o.to, cid);
                for x in outs {
                    let _ = push_out(&hh.out, x);
                }
            }
        }
    }
    });
    // уборка: простаивающие цепочки и старые отправительские
    let hh = h.clone();
    crate::supervisor::supervise("hops_cleanup", crate::supervisor::Policy::restart(), move || {
        let hh = hh.clone();
        async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let outs = hh.router.lock().unwrap_or_else(|e| e.into_inner()).gc(now());
            for o in outs {
                let _ = push_out(&hh.out, o);
            }
            let t = now();
            hh.clients.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, c| {
                let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
                let age = t.saturating_sub(c.born);
                // a circuit that still carries streams is not cut off from its packets: it is kept (longer) and only retired
                // with its streams told to end, so no application socket is left waiting on a circuit nobody listens to
                let keep = !c.dead && (age < CIRCUIT_LIFE_SECS * 2 || (!c.streams.is_empty() && age < CIRCUIT_LIFE_SECS * 2 * BUSY_CIRCUIT_FACTOR));
                if !keep {
                    for (_, st) in c.streams.drain() {
                        let _ = st.to_app.send(ToApp::End);
                    }
                    c.dead = true;
                }
                keep
            });
        }
    }
    });
}

/// Пакет цепочки от соседа (0xD9..0xDC) — из общей раздачи пакетов основной связи.
pub fn on_packet(sender: &[u8; 32], plain: &[u8]) {
    let Some(h) = cell().get().cloned() else { return };
    let Some(plain) = hops::normalize(plain) else { return };
    // пакет «нашей» цепочки (мы отправитель) или чужой (мы посредник/выход)
    if plain.len() >= 9 {
        let cid = u64::from_be_bytes(plain[1..9].try_into().unwrap());
        let mine = h.clients.lock().unwrap_or_else(|e| e.into_inner()).get(&cid).cloned();
        if let Some(c) = mine {
            h.client_packet(c, *sender, plain);
            return;
        }
    }
    let (outs, events) = h.router.lock().unwrap_or_else(|e| e.into_inner()).on_packet(*sender, plain, now());
    for o in outs {
        let _ = push_out(&h.out, o);
    }
    for e in events {
        h.exit_event(e);
    }
}

// ---------------------------------------------------------------- выход

impl Hops {
    fn exit_event(self: &Arc<Self>, e: Event) {
        match e {
            Event::Closed { handle } => {
                if let Some(c) = self.exit_circuits.lock().unwrap_or_else(|e| e.into_inner()).remove(&handle) {
                    for (_, s) in c.streams {
                        let _ = s.to_target.send(None);
                    }
                }
            }
            Event::Cell { handle, cmd, stream, data } => match cmd {
                hops::CMD_BEGIN => self.exit_begin(handle, stream, data),
                hops::CMD_DATA => {
                    let Some((seq, bytes)) = split_seq(&data) else { return };
                    let mut g = self.exit_circuits.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(s) = g.get_mut(&handle).and_then(|c| c.streams.get_mut(&stream)) {
                        for d in s.reorder.push(seq, bytes) {
                            // the destination is not keeping up: do not buffer without limit, close the stream
                            if s.queued.fetch_add(d.len(), std::sync::atomic::Ordering::Relaxed) + d.len() > STREAM_QUEUE_MAX {
                                let _ = s.to_target.send(None);
                                break;
                            }
                            let _ = s.to_target.send(Some(d));
                        }
                    }
                }
                hops::CMD_END => {
                    if let Some(s) = self.exit_circuits.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&handle).and_then(|c| c.streams.remove(&stream)) {
                        let _ = s.to_target.send(None);
                    }
                }
                _ => {}
            },
        }
    }

    fn reply(&self, h: Handle, cmd: u8, stream: u16, data: &[u8]) {
        let o = self.router.lock().unwrap_or_else(|e| e.into_inner()).reply(h, cmd, stream, data);
        if let Some(o) = o {
            let _ = push_out(&self.out, o);
        }
    }

    fn exit_begin(self: &Arc<Self>, handle: Handle, stream: u16, data: Vec<u8>) {
        let me = self.clone();
        let refuse = move |code: u8| me.reply(handle, hops::CMD_END, stream, &[code]);
        let secret = device_secret().ok();
        let (target, personal) = parse_begin(&String::from_utf8_lossy(&data), secret.as_ref());
        println!("[hops] выход: просьба соединиться с {target} по цепочке от узла {}{}", hex::encode(&handle.0[..4]), if personal == Some(true) { " (своё устройство)" } else { "" });
        let port = target.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok());
        match personal {
            // метка была, но неверна: чужой, выдающий себя за своего
            Some(false) => {
                eprintln!("[hops] ⛔ неверная метка своего устройства");
                return refuse(END_REFUSED);
            }
            // своё устройство: любые порты, правила выхода для всех не нужны
            Some(true) => {
                if port.is_none() {
                    return refuse(END_REFUSED);
                }
            }
            None => {
                // кто может выходить: правило владельца (по цепочке видно только предыдущий узел)
                if !crate::exit_policy::may_exit_anonymous(&HashId(handle.0)) {
                    crate::exit_policy::refuse_log(&HashId(handle.0), "цепочка");
                    return refuse(END_REFUSED);
                }
                if port.filter(|p| web_port(*p)).is_none() {
                    return refuse(END_REFUSED);
                }
            }
        }
        {
            let mut g = self.exit_circuits.lock().unwrap_or_else(|e| e.into_inner());
            let c = g.entry(handle).or_insert_with(|| ExitCircuit { streams: HashMap::new(), begins: (0, 0) });
            let t = now();
            if t / 60 != c.begins.0 {
                c.begins = (t / 60, 0);
            }
            c.begins.1 += 1;
            if c.begins.1 > BEGINS_PER_MINUTE || c.streams.len() >= MAX_STREAMS_PER_CIRCUIT || c.streams.contains_key(&stream) {
                drop(g);
                return refuse(END_REFUSED);
            }
        }
        let me = self.clone();
        tokio::spawn(async move {
            let stream_conn = match crate::exit_policy::connect_public(&target, Duration::from_secs(10)).await {
                Ok(s) => s,
                Err(e) => {
                    let code = if e.kind() == std::io::ErrorKind::PermissionDenied { END_REFUSED } else { END_FAILED };
                    me.reply(handle, hops::CMD_END, stream, &[code]);
                    return;
                }
            };
            let _ = stream_conn.set_nodelay(true);
            let (mut rd, mut wr) = stream_conn.into_split();
            let (tx, mut rx) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
            let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            {
                let mut g = me.exit_circuits.lock().unwrap_or_else(|e| e.into_inner());
                let Some(c) = g.get_mut(&handle) else { return };
                c.streams.insert(stream, ExitStream { to_target: tx, queued: queued.clone(), reorder: Reorder::default() });
            }
            me.reply(handle, hops::CMD_CONNECTED, stream, &[]);
            // от отправителя к цели
            tokio::spawn(async move {
                while let Some(Some(d)) = rx.recv().await {
                    let n = d.len();
                    let r = wr.write_all(&d).await;
                    queued.fetch_sub(n, std::sync::atomic::Ordering::Relaxed);
                    if r.is_err() {
                        break;
                    }
                }
                let _ = wr.shutdown().await;
            });
            // от цели к отправителю
            let mut seq = 0u32;
            let mut buf = vec![0u8; CHUNK];
            loop {
                // backpressure: while the shared send queue is full, stop reading from the destination
                while OUT_QUEUED.load(std::sync::atomic::Ordering::Relaxed) > OUT_QUEUE_HIGH {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        me.reply(handle, hops::CMD_DATA, stream, &with_seq(seq, &buf[..n]));
                        seq += 1;
                    }
                }
            }
            me.reply(handle, hops::CMD_END, stream, &[0]);
            if let Some(c) = me.exit_circuits.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&handle) {
                c.streams.remove(&stream);
            }
        });
    }

    // ---------------------------------------------------------------- отправитель

    fn client_packet(self: &Arc<Self>, c: Arc<Mutex<ClientCircuit>>, from: NodeId, plain: &[u8]) {
        let (outs, events) = {
            let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
            g.circuit.on_packet(from, plain)
        };
        for o in outs {
            let _ = push_out(&self.out, o);
        }
        let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
        for e in events {
            match e {
                ClientEvent::Ready => {
                    if let Some(r) = g.ready.take() {
                        let _ = r.send(Ok(()));
                    }
                }
                ClientEvent::Failed(why) => {
                    g.dead = true;
                    if let Some(r) = g.ready.take() {
                        let _ = r.send(Err(why));
                    }
                }
                ClientEvent::Closed => {
                    g.dead = true;
                    for (_, s) in g.streams.drain() {
                        let _ = s.to_app.send(ToApp::End);
                    }
                    if let Some(r) = g.ready.take() {
                        let _ = r.send(Err("closed"));
                    }
                }
                ClientEvent::Cell { hop, cmd, stream, data } => {
                    // stream replies come from the EXIT only: a middle node that guesses a stream number must not be able to
                    // answer in its place
                    if hop + 1 != g.circuit.len() {
                        continue;
                    }
                    let Some(s) = g.streams.get_mut(&stream) else { continue };
                    match cmd {
                        hops::CMD_CONNECTED => {
                            if let Some(t) = s.connected.take() {
                                let _ = t.send(Ok(()));
                            }
                        }
                        hops::CMD_DATA => {
                            // data may overtake CONNECTED on the network; what the application has not read is capped
                            let mut overflow = false;
                            if let Some((seq, bytes)) = split_seq(&data) {
                                for d in s.reorder.push(seq, bytes) {
                                    let n = d.len();
                                    if s.pending.fetch_add(n, std::sync::atomic::Ordering::Relaxed) + n > CLIENT_STREAM_QUEUE_MAX {
                                        overflow = true;
                                        break;
                                    }
                                    let _ = s.to_app.send(ToApp::Data(d));
                                }
                            }
                            if overflow {
                                // the application is not keeping up: end the stream instead of buffering without limit
                                let _ = s.to_app.send(ToApp::End);
                                g.streams.remove(&stream);
                                if let Some(o) = g.circuit.send_exit(hops::CMD_END, stream, &[0]) {
                                    let _ = push_out(&self.out, o);
                                }
                            }
                        }
                        hops::CMD_END => {
                            let code = data.first().copied().unwrap_or(0);
                            if let Some(t) = s.connected.take() {
                                let _ = t.send(Err(if code == 0 { END_FAILED } else { code }));
                            }
                            let _ = s.to_app.send(ToApp::End);
                            g.streams.remove(&stream);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Выбрать узлы для цепочки с выходом `exit`: вход и средний — любые узлы с карточкой и живой связью.
    async fn pick_path(&self, exit: NodeId) -> Result<Vec<PathHop>, String> {
        let peers = self.transport.get_peers().await;
        let connected: std::collections::HashSet<String> = peers.iter().map(|p| p.id.to_hex()).collect();
        let (offers, _) = crate::network_offers::directory_snapshot(None);
        let hop = |o: &crate::network_offers::NodeOffer| -> Option<PathHop> {
            let id: NodeId = hex::decode(&o.node_id).ok()?.try_into().ok()?;
            let kb: [u8; 32] = hex::decode(&o.key).ok()?.try_into().ok()?;
            Some(PathHop { id, key: VerifyingKey::from_bytes(&kb).ok()? })
        };
        let exit_hex = hex::encode(exit);
        let exit_hop = offers.iter().find(|o| o.node_id == exit_hex).and_then(hop).ok_or("карточки выхода нет")?;
        use rand::seq::SliceRandom;
        let mut others: Vec<PathHop> = offers.iter().filter(|o| o.node_id != exit_hex && hex::decode(&o.node_id).ok().as_deref() != Some(&self.me[..]) && connected.contains(&o.node_id)).filter_map(hop).collect();
        others.shuffle(&mut rand::thread_rng());
        // вход должен быть связан с нами напрямую; средний — тот, до кого вход дотянется (проверится при построении)
        let mut path: Vec<PathHop> = others.into_iter().take(2).collect();
        if path.is_empty() {
            return Err("нет узлов для цепочки".into());
        }
        path.push(exit_hop);
        Ok(path)
    }

    async fn build(self: &Arc<Self>, exit: NodeId) -> Result<Arc<Mutex<ClientCircuit>>, String> {
        let path = self.pick_path(exit).await?;
        self.build_path(path, String::new()).await
    }

    /// Построить цепочку по заданному пути (`route` — метка маршрута для повторного использования).
    async fn build_path(self: &Arc<Self>, path: Vec<PathHop>, route: String) -> Result<Arc<Mutex<ClientCircuit>>, String> {
        let exit = path.last().map(|p| p.id).ok_or("пустой путь")?;
        let names: Vec<String> = path.iter().map(|p| hex::encode(&p.id[..4])).collect();
        println!("[hops] строю цепочку: {}", names.join(" → "));
        let mut circuit = Circuit::new(path);
        let first = circuit.begin();
        let cid = circuit.cid();
        let (tx, rx) = oneshot::channel();
        let c = Arc::new(Mutex::new(ClientCircuit { circuit, exit, route, born: now(), next_stream: 1, streams: HashMap::new(), ready: Some(tx), dead: false }));
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).insert(cid, c.clone());
        let _ = push_out(&self.out, first);
        match tokio::time::timeout(BUILD_TIMEOUT, rx).await {
            Ok(Ok(Ok(()))) => Ok(c),
            other => {
                self.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(&cid);
                c.lock().unwrap_or_else(|e| e.into_inner()).dead = true;
                Err(match other {
                    Ok(Ok(Err(w))) => format!("цепочка не построена: {w}"),
                    _ => "цепочка не построена: время вышло".into(),
                })
            }
        }
    }

    /// Готовая подходящая цепочка по метке маршрута или новая (до трёх попыток).
    async fn circuit_for_route(self: &Arc<Self>, route: &str, path: &[PathHop]) -> Result<Arc<Mutex<ClientCircuit>>, String> {
        let t = now();
        let existing = self.clients.lock().unwrap_or_else(|e| e.into_inner()).values().find(|c| {
            let c = c.lock().unwrap_or_else(|e| e.into_inner());
            !c.dead && c.route == route && c.circuit.is_ready() && t.saturating_sub(c.born) < CIRCUIT_LIFE_SECS && c.streams.len() < MAX_STREAMS_PER_CIRCUIT
        }).cloned();
        if let Some(c) = existing {
            return Ok(c);
        }
        let mut last = String::new();
        for _ in 0..3 {
            match self.build_path(path.to_vec(), route.to_string()).await {
                Ok(c) => return Ok(c),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    async fn circuit_for(self: &Arc<Self>, exit: NodeId) -> Result<Arc<Mutex<ClientCircuit>>, String> {
        let t = now();
        let existing = self.clients.lock().unwrap_or_else(|e| e.into_inner()).values().find(|c| {
            let c = c.lock().unwrap_or_else(|e| e.into_inner());
            !c.dead && c.exit == exit && c.route.is_empty() && c.circuit.is_ready() && t.saturating_sub(c.born) < CIRCUIT_LIFE_SECS && c.streams.len() < MAX_STREAMS_PER_CIRCUIT
        }).cloned();
        if let Some(c) = existing {
            return Ok(c);
        }
        // до трёх попыток с разными соседями
        let mut last = String::new();
        for _ in 0..3 {
            match self.build(exit).await {
                Ok(c) => return Ok(c),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

/// Соединение с `host:port` через выход `exit` по цепочке.
pub async fn connect(exit: NodeId, host: &str, port: u16) -> Result<(HopWriter, HopReader), u8> {
    let h = cell().get().cloned().ok_or(END_FAILED)?;
    let circuit = h.circuit_for(exit).await.map_err(|e| {
        eprintln!("[hops] {e}");
        END_FAILED
    })?;
    open_stream(h, circuit, format!("{host}:{port}")).await
}

/// Соединение с `host:port` через личный шлюз `path.last()` (свой компьютер за NAT или дом): путь задан явно, а просьба несёт
/// метку своего устройства. `secret` — общий секрет устройств (`device_secret` шлюза, получен при сопряжении).
pub async fn connect_personal(path: Vec<PathHop>, secret: &[u8; 32], host: &str, port: u16) -> Result<(HopWriter, HopReader), u8> {
    let h = cell().get().cloned().ok_or(END_FAILED)?;
    let route = format!("own:{}", path.iter().map(|p| hex::encode(&p.id[..8])).collect::<Vec<_>>().join(">"));
    let circuit = h.circuit_for_route(&route, &path).await.map_err(|e| {
        eprintln!("[hops] {e}");
        END_FAILED
    })?;
    let target = format!("{host}:{port}");
    open_stream(h, circuit, format!("P:{}:{target}", personal_tag(secret, &target))).await
}

async fn open_stream(h: Arc<Hops>, circuit: Arc<Mutex<ClientCircuit>>, begin: String) -> Result<(HopWriter, HopReader), u8> {
    let (ctx, crx) = oneshot::channel();
    let (to_app, mut from_net) = mpsc::unbounded_channel::<ToApp>();
    // towards the application the queue is bounded: a slow reader slows the forwarder, and the byte counter below ends the stream
    let (rx_tx, rx) = mpsc::channel::<Option<Vec<u8>>>(64);
    let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stream = {
        let mut g = circuit.lock().unwrap_or_else(|e| e.into_inner());
        // the next FREE stream number (numbers wrap; an occupied one must never be overwritten)
        let mut s = g.next_stream;
        let mut tries = 0u32;
        while g.streams.contains_key(&s) {
            s = s.wrapping_add(1).max(1);
            tries += 1;
            if tries > u16::MAX as u32 {
                return Err(END_FAILED);
            }
        }
        g.next_stream = s.wrapping_add(1).max(1);
        g.streams.insert(s, ClientStream { connected: Some(ctx), to_app, pending: pending.clone(), reorder: Reorder::default() });
        let o = g.circuit.send_exit(hops::CMD_BEGIN, s, begin.as_bytes());
        if let Some(o) = o {
            let _ = push_out(&h.out, o);
        } else {
            g.streams.remove(&s);
            return Err(END_FAILED);
        }
        s
    };
    tokio::spawn(async move {
        while let Some(m) = from_net.recv().await {
            match m {
                ToApp::Data(d) => {
                    pending.fetch_sub(d.len().min(pending.load(std::sync::atomic::Ordering::Relaxed)), std::sync::atomic::Ordering::Relaxed);
                    if rx_tx.send(Some(d)).await.is_err() {
                        break;
                    }
                }
                ToApp::End => {
                    let _ = rx_tx.send(None).await;
                    break;
                }
            }
        }
    });
    match tokio::time::timeout(BEGIN_TIMEOUT, crx).await {
        Ok(Ok(Ok(()))) => Ok((HopWriter { hops: h, circuit, stream, seq: 0, done: false }, rx)),
        Ok(Ok(Err(code))) => Err(code),
        _ => {
            circuit.lock().unwrap_or_else(|e| e.into_inner()).streams.remove(&stream);
            Err(END_FAILED)
        }
    }
}

/// Запись в соединение цепочки (чтение — отдельный приёмник: `Some(данные)` или `None` при закрытии).
pub struct HopWriter {
    hops: Arc<Hops>,
    circuit: Arc<Mutex<ClientCircuit>>,
    stream: u16,
    seq: u32,
    /// the stream has been closed from this side (closing is done once, also when the writer is simply dropped)
    done: bool,
}

type HopReader = mpsc::Receiver<Option<Vec<u8>>>;

impl Drop for HopWriter {
    fn drop(&mut self) {
        // a writer that is dropped without close() (e.g. the SOCKS hand-off failed) must not leave its stream open
        self.close();
    }
}

impl HopWriter {
    /// Передать данные цели (режутся на ячейки).
    pub fn write(&mut self, data: &[u8]) -> bool {
        let mut g = self.circuit.lock().unwrap_or_else(|e| e.into_inner());
        for chunk in data.chunks(CHUNK) {
            let Some(o) = g.circuit.send_exit(hops::CMD_DATA, self.stream, &with_seq(self.seq, chunk)) else { return false };
            self.seq += 1;
            if push_out(&self.hops.out, o).is_err() {
                return false;
            }
        }
        true
    }

    pub fn close(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let mut g = self.circuit.lock().unwrap_or_else(|e| e.into_inner());
        g.streams.remove(&self.stream);
        if let Some(o) = g.circuit.send_exit(hops::CMD_END, self.stream, &[0]) {
            let _ = push_out(&self.hops.out, o);
        }
    }
}

/// Перекачка между клиентом SOCKS и соединением цепочки до закрытия любой стороны.
pub async fn pump(client: tokio::net::TcpStream, mut w: HopWriter, mut rx: HopReader) {
    let (mut cr, mut cw) = client.into_split();
    let up = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            // do not read from the local application while the shared send queue is full (backpressure)
            while OUT_QUEUED.load(std::sync::atomic::Ordering::Relaxed) > OUT_QUEUE_HIGH {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            match cr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if !w.write(&buf[..n]) {
                        break;
                    }
                }
            }
        }
    };
    let down = async {
        while let Some(Some(d)) = rx.recv().await {
            if cw.write_all(&d).await.is_err() {
                break;
            }
        }
        let _ = cw.shutdown().await;
    };
    // когда закрылась одна сторона — закрываем и другую
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
    w.close();
}

/// Сопряжение своего устройства (под проверкой входа владельца): номер, ключ и секрет шлюза — их вводят на устройстве
/// (в приложении — через QR). Секрет даёт доступ к личному шлюзу: показывать только владельцу.
pub fn router<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
    async fn pairing() -> axum::response::Response {
        use axum::response::IntoResponse;
        let (Some(card), Ok(secret)) = (crate::web::peers::my_card(), device_secret()) else {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, axum::Json(serde_json::json!({"error": "узел ещё не готов"}))).into_response();
        };
        // вход для телефона по TLS: порт и отпечаток сертификата (телефон сверяет его при подключении)
        let tls = crate::mobile_tls::configured_port().and_then(|port| {
            crate::netlayer::tls_cert::TlsIdentity::load_or_generate_default(&card.id).ok().map(|i| serde_json::json!({"port": port, "fingerprint": i.fingerprint_hex}))
        });
        axum::Json(serde_json::json!({"node": card.id, "key": card.key, "secret": hex::encode(secret), "addr": card.addr, "tls": tls})).into_response()
    }
    axum::Router::new().route("/api/devices/pairing", axum::routing::get(pairing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_put_back_in_order_and_far_future_cells_are_not_kept() {
        let mut r = Reorder::default();
        assert!(r.push(1, b"b".to_vec()).is_empty());
        assert_eq!(r.push(0, b"a".to_vec()), vec![b"a".to_vec(), b"b".to_vec()]);
        assert!(r.push(0, b"again".to_vec()).is_empty(), "already delivered");
        assert!(r.push(10_000, b"x".to_vec()).is_empty(), "too far ahead");
        assert!(r.held.is_empty());
        assert_eq!(r.push(2, b"c".to_vec()), vec![b"c".to_vec()]);
    }

    #[test]
    fn the_stream_number_rides_inside_the_data() {
        let d = with_seq(7, b"hello");
        assert_eq!(split_seq(&d), Some((7, b"hello".to_vec())));
        assert_eq!(split_seq(&[1, 2, 3]), None);
        assert!(CHUNK + 4 == hops::DATA_MAX);
    }

    #[test]
    fn a_personal_request_carries_a_tag_only_the_owner_can_make() {
        let secret = [7u8; 32];
        let target = "example.org:22";
        let begin = format!("P:{}:{target}", personal_tag(&secret, target));
        assert_eq!(parse_begin(&begin, Some(&secret)), (target.to_string(), Some(true)));
        assert_eq!(parse_begin(&begin, Some(&[8u8; 32])), (target.to_string(), Some(false)), "another secret");
        assert_eq!(parse_begin(&begin.replace("example.org", "other.org"), Some(&secret)).1, Some(false), "the tag is for this target only");
        assert_eq!(parse_begin("example.org:443", Some(&secret)), ("example.org:443".to_string(), None), "a plain exit request has no tag");
        assert_eq!(parse_begin(&begin, None).1, Some(false), "no secret on this node: nobody is personal");
    }

    #[test]
    fn only_web_ports_by_default() {
        assert!(web_port(80) && web_port(443));
        assert!(!web_port(22) && !web_port(25) && !web_port(3306));
    }
}
