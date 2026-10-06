//! Запасной путь по TCP/TLS: связь не пропадает, когда провайдер режет или задерживает UDP (например, в часы пик).
//!
//! **Идея.** Узлы по-прежнему шлют датаграммы через те же сокеты, но сокет — это [`CSocket`]: он сам выбирает носитель.
//! Обычно датаграмма уходит по UDP. Если UDP заметно не доходит (несколько отправленных датаграмм без единого ответа за 10 секунд),
//! узел устанавливает TLS-соединение к тому же узлу и дальше шлёт те же датаграммы внутри него (кадры `[длина][вид][тело]`).
//! Остальной код (рукопожатия, шифрование звена, цепочки, прокси) **ничего не знает** про носитель: адреса узлов остаются
//! настоящими, ответ на датаграмму, пришедшую по TCP, уходит тоже по TCP.
//!
//! * **Возврат на UDP.** Раз в 15 секунд при работе по TCP ещё и пробная датаграмма по UDP; пришёл ответ по UDP — снова UDP.
//! * **Безопасность.** TLS здесь — это не защита, а маскировка под обычный HTTPS: содержимое уже зашифровано и подписано на
//!   уровне звена (приветствия подписаны, пакеты сеанса аутентифицированы), поэтому сертификат собеседника не проверяется — так же,
//!   как нельзя «проверить» отправителя UDP-пакета. Подделать можно только датаграмму «от адреса соединения» (своего адреса
//!   чужим не выдать), а уровень звена отвергает всё, что не подписано.
//! * **Порт.** TCP слушается на том же номере, что и UDP-обнаружение (по первому байту отличается от проверки достижимости).
//! * **Пределы.** До 256 соединений на узел, до 16 на адрес, кадр не больше 70 000 байт, очередь соединения ограничена
//!   (при переполнении датаграмма теряется, как на UDP).
//! * **Режимы** (`YANDI_TCP_CARRIER`): по умолчанию — автоматический; `off` — только UDP; `tcp-only` — для проверок: UDP не
//!   используется вообще (ни отправка, ни приём), чтобы доказать, что вся связь живёт по TCP.
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs, UdpSocket};
use tokio::sync::mpsc;

const FRAME_PORTS: u8 = 1;
const FRAME_DGRAM: u8 = 2;
const MAX_FRAME: usize = 70_000;
const QUEUE: usize = 2048;
pub const MAX_CONNS: usize = 256;
pub const MAX_CONNS_PER_IP: usize = 16;
/// UDP считается неработающим: столько датаграмм без ответа…
const UNHEALTHY_SENT: u32 = 2;
/// …за столько времени.
const UNHEALTHY_AFTER: Duration = Duration::from_secs(10);
/// Датаграммы, пришедшие по TCP недавно: отвечаем тоже по TCP.
const TCP_RX_FRESH: Duration = Duration::from_secs(20);
/// При работе по TCP раз в столько времени пробуем UDP.
const UDP_PROBE_EVERY: Duration = Duration::from_secs(15);
const DIAL_EVERY: Duration = Duration::from_secs(10);
const PENDING_MAX: usize = 16;
const PENDING_TTL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Auto,
    Off,
    TcpOnly,
}

fn mode_from_env() -> Mode {
    match std::env::var("YANDI_TCP_CARRIER").as_deref() {
        Ok("off") => Mode::Off,
        Ok("tcp-only") => Mode::TcpOnly,
        _ => Mode::Auto,
    }
}

struct Conn {
    id: u64,
    tx: mpsc::Sender<Vec<u8>>,
    remote_ip: IpAddr,
    alive: AtomicBool,
    /// порт, на который мы стучались (для входящих — не задан)
    dialed: Option<u16>,
}

struct Health {
    first_unanswered: Option<Instant>,
    unanswered: u32,
    last_udp_probe: Instant,
}

#[derive(Default)]
struct Inner {
    sockets: HashMap<u16, mpsc::Sender<(Vec<u8>, SocketAddr)>>,
    by_dest: HashMap<SocketAddr, Arc<Conn>>,
    health: HashMap<IpAddr, Health>,
    hints: HashMap<IpAddr, Vec<u16>>,
    dialing: HashMap<SocketAddr, Instant>,
    tcp_rx: HashMap<SocketAddr, Instant>,
    /// датаграммы, ждущие установки соединения: по адресу назначения
    pending: HashMap<SocketAddr, VecDeque<(Vec<u8>, u16, Instant)>>,
    conns: Vec<Arc<Conn>>,
}

pub struct Carrier {
    inner: Mutex<Inner>,
    mode: Mode,
    node_hex: String,
    next_id: AtomicU64,
}

fn cell() -> &'static OnceLock<Arc<Carrier>> {
    static C: OnceLock<Arc<Carrier>> = OnceLock::new();
    &C
}

/// Носитель этого узла (создаётся при запуске основной связи).
pub fn global() -> Option<Arc<Carrier>> {
    cell().get().cloned()
}

pub fn install(node_hex: String) -> Arc<Carrier> {
    cell().get_or_init(|| Arc::new(Carrier { inner: Mutex::new(Inner::default()), mode: mode_from_env(), node_hex, next_id: AtomicU64::new(1) })).clone()
}

enum Route {
    Udp,
    Tcp(Arc<Conn>),
    /// по TCP, плюс пробная датаграмма по UDP
    TcpAndUdp(Arc<Conn>),
    /// носителя пока нет: датаграмма ждёт соединения (и по UDP уходит, если режим позволяет)
    WaitForTcp,
}

impl Carrier {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Подсказка, на какой TCP-порт этого адреса стучаться (порт обнаружения узла).
    pub fn hint(&self, addr: SocketAddr) {
        let mut g = self.lock();
        let v = g.hints.entry(addr.ip()).or_default();
        if !v.contains(&addr.port()) && v.len() < 8 {
            v.push(addr.port());
        }
    }

    fn register(self: &Arc<Self>, port: u16, tx: mpsc::Sender<(Vec<u8>, SocketAddr)>) {
        let conns = {
            let mut g = self.lock();
            g.sockets.insert(port, tx);
            g.conns.clone()
        };
        // порты сменились (ротация) — соседям по TCP сообщается заново
        let frame = self.ports_frame();
        for c in conns {
            let _ = c.tx.try_send(frame.clone());
        }
    }

    fn ports_frame(&self) -> Vec<u8> {
        let g = self.lock();
        let mut ports: Vec<u16> = g.sockets.keys().copied().collect();
        ports.sort();
        let mut body = vec![FRAME_PORTS, ports.len().min(16) as u8];
        for p in ports.iter().take(16) {
            body.extend_from_slice(&p.to_be_bytes());
        }
        frame(&body)
    }

    fn note_udp_sent(&self, ip: IpAddr) {
        let mut g = self.lock();
        let h = g.health.entry(ip).or_insert(Health { first_unanswered: None, unanswered: 0, last_udp_probe: Instant::now() });
        if h.first_unanswered.is_none() {
            h.first_unanswered = Some(Instant::now());
        }
        h.unanswered = h.unanswered.saturating_add(1);
    }

    fn note_udp_rx(&self, ip: IpAddr) {
        if let Some(h) = self.lock().health.get_mut(&ip) {
            h.first_unanswered = None;
            h.unanswered = 0;
        }
    }

    fn udp_unhealthy(g: &Inner, ip: IpAddr) -> bool {
        g.health.get(&ip).map(|h| h.unanswered >= UNHEALTHY_SENT && h.first_unanswered.map(|t| t.elapsed() >= UNHEALTHY_AFTER).unwrap_or(false)).unwrap_or(false)
    }

    fn route(self: &Arc<Self>, dest: SocketAddr) -> Route {
        let ip = dest.ip();
        let mut dial = false;
        let route = {
            let mut g = self.lock();
            let conn = g.by_dest.get(&dest).filter(|c| c.alive.load(Ordering::Relaxed)).cloned();
            let tcp_fresh = g.tcp_rx.get(&dest).map(|t| t.elapsed() < TCP_RX_FRESH).unwrap_or(false);
            let unhealthy = Self::udp_unhealthy(&g, ip);
            match (self.mode, conn) {
                (Mode::Off, _) => Route::Udp,
                (Mode::TcpOnly, Some(c)) => Route::Tcp(c),
                (Mode::TcpOnly, None) => {
                    dial = true;
                    Route::WaitForTcp
                }
                (Mode::Auto, Some(c)) if tcp_fresh || unhealthy => {
                    let probe = g.health.get(&ip).map(|h| h.last_udp_probe.elapsed() >= UDP_PROBE_EVERY).unwrap_or(true);
                    if probe {
                        if let Some(h) = g.health.get_mut(&ip) {
                            h.last_udp_probe = Instant::now();
                        }
                        Route::TcpAndUdp(c)
                    } else {
                        Route::Tcp(c)
                    }
                }
                (Mode::Auto, None) if unhealthy => {
                    dial = true;
                    Route::WaitForTcp
                }
                _ => Route::Udp,
            }
        };
        if dial {
            self.dial_later(ip);
        }
        route
    }

    /// Постучаться по TCP на все известные порты этого адреса, где соединения ещё нет (на одном адресе может быть несколько узлов —
    /// например, на тренировочной сети; для каждого порта — не чаще раза в `DIAL_EVERY`).
    fn dial_later(self: &Arc<Self>, ip: IpAddr) {
        let ports: Vec<u16> = {
            let mut g = self.lock();
            if g.conns.iter().filter(|c| c.remote_ip == ip).count() >= MAX_CONNS_PER_IP {
                return;
            }
            let hints = g.hints.get(&ip).cloned().unwrap_or_default();
            let mut todo = vec![];
            for p in hints {
                let to = SocketAddr::new(ip, p);
                let connected = g.conns.iter().any(|c| c.remote_ip == ip && c.dialed == Some(p) && c.alive.load(Ordering::Relaxed));
                let recent = g.dialing.get(&to).map(|t| t.elapsed() < DIAL_EVERY).unwrap_or(false);
                if !connected && !recent {
                    g.dialing.insert(to, Instant::now());
                    todo.push(p);
                }
            }
            todo
        };
        for p in ports {
            let me = self.clone();
            tokio::spawn(async move {
                let _ = me.dial(SocketAddr::new(ip, p)).await;
            });
        }
    }

    async fn dial(self: Arc<Self>, to: SocketAddr) -> io::Result<()> {
        let tcp = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(to)).await.map_err(|_| io::Error::other("connect timeout"))??;
        let _ = tcp.set_nodelay(true);
        let cfg = crate::netlayer::tls_cert::build_client_config_insecure().map_err(|e| io::Error::other(e.to_string()))?;
        let name = rustls::pki_types::ServerName::try_from("localhost").map_err(|e| io::Error::other(e.to_string()))?;
        let tls = tokio::time::timeout(Duration::from_secs(8), tokio_rustls::TlsConnector::from(cfg).connect(name, tcp)).await.map_err(|_| io::Error::other("tls timeout"))??;
        println!("[carrier] установлено TCP-соединение с {to}");
        self.run_conn(tls, to.ip(), Some(to.port())).await;
        Ok(())
    }

    /// Принять TCP-соединение (TLS) на порту основной связи.
    pub async fn accept(self: Arc<Self>, tcp: TcpStream) -> io::Result<()> {
        let peer = tcp.peer_addr()?;
        {
            let g = self.lock();
            if g.conns.len() >= MAX_CONNS || g.conns.iter().filter(|c| c.remote_ip == peer.ip()).count() >= MAX_CONNS_PER_IP {
                return Ok(());
            }
        }
        let _ = tcp.set_nodelay(true);
        let identity = crate::netlayer::tls_cert::TlsIdentity::load_or_generate_default(&self.node_hex).map_err(|e| io::Error::other(e.to_string()))?;
        let cfg = crate::netlayer::tls_cert::build_server_config(&identity).map_err(|e| io::Error::other(e.to_string()))?;
        let tls = tokio::time::timeout(Duration::from_secs(10), tokio_rustls::TlsAcceptor::from(cfg).accept(tcp)).await.map_err(|_| io::Error::other("tls timeout"))??;
        println!("[carrier] принято TCP-соединение от {}", peer.ip());
        self.run_conn(tls, peer.ip(), None).await;
        Ok(())
    }

    async fn run_conn<S>(self: Arc<Self>, stream: S, remote_ip: IpAddr, dialed: Option<u16>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        let conn = Arc::new(Conn { id: self.next_id.fetch_add(1, Ordering::Relaxed), tx, remote_ip, alive: AtomicBool::new(true), dialed });
        self.lock().conns.push(conn.clone());
        let (mut rd, mut wr) = tokio::io::split(stream);
        // сразу сообщаем свои порты
        let _ = conn.tx.try_send(self.ports_frame());
        let writer = tokio::spawn(async move {
            while let Some(f) = rx.recv().await {
                if wr.write_all(&f).await.is_err() {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        });
        let result = self.read_loop(&mut rd, &conn).await;
        let _ = result;
        conn.alive.store(false, Ordering::Relaxed);
        {
            let mut g = self.lock();
            g.by_dest.retain(|_, c| c.id != conn.id);
            g.conns.retain(|c| c.id != conn.id);
        }
        writer.abort();
        println!("[carrier] TCP-соединение с {remote_ip} закрыто");
    }

    async fn read_loop<R: tokio::io::AsyncRead + Unpin>(&self, rd: &mut R, conn: &Arc<Conn>) -> io::Result<()> {
        loop {
            let mut len = [0u8; 4];
            rd.read_exact(&mut len).await?;
            let n = u32::from_be_bytes(len) as usize;
            if n == 0 || n > MAX_FRAME {
                return Err(io::Error::other("bad frame length"));
            }
            let mut body = vec![0u8; n];
            rd.read_exact(&mut body).await?;
            match body[0] {
                FRAME_PORTS => {
                    let cnt = *body.get(1).unwrap_or(&0) as usize;
                    if body.len() != 2 + cnt * 2 || cnt > 16 {
                        return Err(io::Error::other("bad ports frame"));
                    }
                    let mut g = self.lock();
                    for i in 0..cnt {
                        let p = u16::from_be_bytes([body[2 + 2 * i], body[3 + 2 * i]]);
                        g.by_dest.insert(SocketAddr::new(conn.remote_ip, p), conn.clone());
                    }
                    drop(g);
                    self.flush_pending(conn);
                }
                FRAME_DGRAM => {
                    if body.len() < 5 {
                        return Err(io::Error::other("bad datagram frame"));
                    }
                    let dst = u16::from_be_bytes([body[1], body[2]]);
                    let src = u16::from_be_bytes([body[3], body[4]]);
                    let from = SocketAddr::new(conn.remote_ip, src);
                    let tx = {
                        let mut g = self.lock();
                        g.by_dest.insert(from, conn.clone());
                        g.tcp_rx.insert(from, Instant::now());
                        if g.tcp_rx.len() > 4096 {
                            g.tcp_rx.retain(|_, t| t.elapsed() < TCP_RX_FRESH);
                        }
                        g.sockets.get(&dst).cloned()
                    };
                    if let Some(tx) = tx {
                        let _ = tx.try_send((body[5..].to_vec(), from));
                    }
                }
                _ => return Err(io::Error::other("unknown frame")),
            }
        }
    }

    fn flush_pending(&self, conn: &Arc<Conn>) {
        let mut g = self.lock();
        let ready: Vec<SocketAddr> = g.pending.keys().filter(|d| d.ip() == conn.remote_ip && g.by_dest.contains_key(d)).copied().collect();
        for d in ready {
            if let Some(q) = g.pending.remove(&d) {
                for (data, src, at) in q {
                    if at.elapsed() < PENDING_TTL {
                        let _ = conn.tx.try_send(dgram_frame(d.port(), src, &data));
                    }
                }
            }
        }
    }

    fn queue_pending(&self, dest: SocketAddr, src_port: u16, data: &[u8]) {
        let mut g = self.lock();
        if g.pending.len() > 256 {
            g.pending.clear();
        }
        let q = g.pending.entry(dest).or_default();
        while q.front().map(|(_, _, t)| t.elapsed() >= PENDING_TTL).unwrap_or(false) || q.len() >= PENDING_MAX {
            q.pop_front();
        }
        q.push_back((data.to_vec(), src_port, Instant::now()));
    }
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut f = (body.len() as u32).to_be_bytes().to_vec();
    f.extend_from_slice(body);
    f
}

fn dgram_frame(dst_port: u16, src_port: u16, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(5 + data.len());
    body.push(FRAME_DGRAM);
    body.extend_from_slice(&dst_port.to_be_bytes());
    body.extend_from_slice(&src_port.to_be_bytes());
    body.extend_from_slice(data);
    frame(&body)
}

/// Сокет основной связи: UDP, а когда UDP не доходит — TCP/TLS; для остального кода ведёт себя как `UdpSocket`.
pub struct CSocket {
    udp: Arc<UdpSocket>,
    carrier: Arc<Carrier>,
    rx: tokio::sync::Mutex<mpsc::Receiver<(Vec<u8>, SocketAddr)>>,
    port: u16,
}

impl CSocket {
    pub fn new(udp: Arc<UdpSocket>, carrier: Arc<Carrier>) -> io::Result<Arc<CSocket>> {
        let port = udp.local_addr()?.port();
        let (tx, rx) = mpsc::channel(QUEUE);
        carrier.register(port, tx);
        Ok(Arc::new(CSocket { udp, carrier, rx: tokio::sync::Mutex::new(rx), port }))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp.local_addr()
    }

    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], target: A) -> io::Result<usize> {
        let dest = tokio::net::lookup_host(target).await?.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        let dest = SocketAddr::new(dest.ip().to_canonical(), dest.port());
        match self.carrier.route(dest) {
            Route::Udp => {
                self.carrier.note_udp_sent(dest.ip());
                self.udp.send_to(buf, dest).await
            }
            Route::Tcp(c) => {
                let _ = c.tx.try_send(dgram_frame(dest.port(), self.port, buf));
                Ok(buf.len())
            }
            Route::TcpAndUdp(c) => {
                let _ = c.tx.try_send(dgram_frame(dest.port(), self.port, buf));
                let _ = self.udp.send_to(buf, dest).await;
                Ok(buf.len())
            }
            Route::WaitForTcp => {
                self.carrier.queue_pending(dest, self.port, buf);
                if self.carrier.mode == Mode::TcpOnly {
                    return Ok(buf.len());
                }
                self.carrier.note_udp_sent(dest.ip());
                self.udp.send_to(buf, dest).await
            }
        }
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut rx = self.rx.lock().await;
        loop {
            tokio::select! {
                r = self.udp.recv_from(buf) => {
                    let (n, from) = r?;
                    if self.carrier.mode == Mode::TcpOnly {
                        continue; // проверочный режим: UDP не принимаем вовсе
                    }
                    self.carrier.note_udp_rx(from.ip().to_canonical());
                    return Ok((n, from));
                }
                f = rx.recv() => {
                    let Some((data, from)) = f else { return Err(io::Error::other("carrier closed")) };
                    let n = data.len().min(buf.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    return Ok((n, from));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn node(mode: Mode, name: &str) -> (Arc<Carrier>, Arc<CSocket>, SocketAddr) {
        let udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = udp.local_addr().unwrap();
        let carrier = Arc::new(Carrier { inner: Mutex::new(Inner::default()), mode, node_hex: name.to_string(), next_id: AtomicU64::new(1) });
        let sock = CSocket::new(udp, carrier.clone()).unwrap();
        (carrier, sock, addr)
    }

    /// слушатель TCP на порту сокета: как у узла (TLS-соединения отдаются носителю)
    async fn listen(carrier: Arc<Carrier>, port: u16) {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l.accept().await else { return };
                let c = carrier.clone();
                tokio::spawn(async move {
                    let _ = c.accept(s).await;
                });
            }
        });
    }

    #[tokio::test]
    async fn with_the_carrier_off_or_healthy_udp_nothing_changes() {
        let (_ca, a, _) = node(Mode::Auto, "aa").await;
        let (_cb, b, b_addr) = node(Mode::Auto, "bb").await;
        a.send_to(b"hello", b_addr).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = tokio::time::timeout(Duration::from_secs(2), b.recv_from(&mut buf)).await.unwrap().unwrap();
        assert_eq!((&buf[..n], from), (&b"hello"[..], a.local_addr().unwrap()));
    }

    #[tokio::test]
    async fn datagrams_flow_both_ways_over_tcp_in_the_check_mode_and_replies_return_the_same_way() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        std::env::set_var("HOME", tempfile::tempdir().unwrap().into_path());
        let (ca, a, a_addr) = node(Mode::TcpOnly, "aa11aa11").await;
        let (cb, b, b_addr) = node(Mode::TcpOnly, "bb22bb22").await;
        listen(cb.clone(), b_addr.port()).await; // B слушает TCP на порту своего сокета
        ca.hint(b_addr);
        // первая датаграмма ждёт соединения и уходит сама, как только оно установлено
        a.send_to(b"first", b_addr).await.unwrap();
        let mut buf = [0u8; 2048];
        let (n, from) = tokio::time::timeout(Duration::from_secs(15), b.recv_from(&mut buf)).await.expect("delivered over TCP").unwrap();
        assert_eq!(&buf[..n], b"first");
        assert_eq!(from, a_addr, "B sees A's real address, so replies find the same connection");
        // ответ идёт обратно по тому же соединению
        b.send_to(b"reply", from).await.unwrap();
        let (n, from_b) = tokio::time::timeout(Duration::from_secs(5), a.recv_from(&mut buf)).await.unwrap().unwrap();
        assert_eq!((&buf[..n], from_b), (&b"reply"[..], b_addr));
        // и много данных подряд
        for i in 0..200u32 {
            a.send_to(&i.to_be_bytes(), b_addr).await.unwrap();
        }
        let mut got = 0;
        while got < 200 {
            let (n, _) = tokio::time::timeout(Duration::from_secs(5), b.recv_from(&mut buf)).await.unwrap().unwrap();
            assert_eq!(n, 4);
            assert_eq!(u32::from_be_bytes(buf[..4].try_into().unwrap()), got);
            got += 1;
        }
    }

    #[tokio::test]
    async fn udp_that_does_not_answer_switches_the_route_to_tcp_and_a_live_udp_switches_it_back() {
        let (ca, _a, _) = node(Mode::Auto, "aa33").await;
        let dest: SocketAddr = "203.0.113.5:9000".parse().unwrap();
        let ip = dest.ip();
        assert!(matches!(ca.route(dest), Route::Udp));
        // ответов нет: две датаграммы без ответа и прошло 10 секунд
        {
            let mut g = ca.lock();
            g.health.insert(ip, Health { first_unanswered: Some(Instant::now() - UNHEALTHY_AFTER - Duration::from_secs(1)), unanswered: 3, last_udp_probe: Instant::now() });
            let (tx, _rx) = mpsc::channel(8);
            let c = Arc::new(Conn { id: 1, tx, remote_ip: ip, alive: AtomicBool::new(true), dialed: None });
            g.by_dest.insert(dest, c);
        }
        assert!(matches!(ca.route(dest), Route::Tcp(_)), "no replies over UDP: use the TCP connection");
        // ответ по UDP пришёл — обратно на UDP
        ca.note_udp_rx(ip);
        assert!(matches!(ca.route(dest), Route::Udp));
        // нездоровый UDP, но соединения нет: ждём соединения (и датаграмма не теряется)
        {
            let mut g = ca.lock();
            g.by_dest.clear();
            g.health.insert(ip, Health { first_unanswered: Some(Instant::now() - UNHEALTHY_AFTER - Duration::from_secs(1)), unanswered: 3, last_udp_probe: Instant::now() });
        }
        assert!(matches!(ca.route(dest), Route::WaitForTcp));
    }

    #[tokio::test]
    async fn broken_frames_close_the_connection_and_do_not_hurt_the_node() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        std::env::set_var("HOME", tempfile::tempdir().unwrap().into_path());
        let (cb, b, b_addr) = node(Mode::Auto, "bb44bb44").await;
        listen(cb.clone(), b_addr.port()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let cfg = crate::netlayer::tls_cert::build_client_config_insecure().unwrap();
        for bad in [vec![0, 0, 0, 0], vec![0xFF, 0xFF, 0xFF, 0xFF, 1], {
            let mut v = 3u32.to_be_bytes().to_vec();
            v.extend_from_slice(&[9, 9, 9]);
            v
        }] {
            let tcp = TcpStream::connect(b_addr).await.unwrap();
            let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
            let mut tls = tokio_rustls::TlsConnector::from(cfg.clone()).connect(name, tcp).await.unwrap();
            tls.write_all(&bad).await.unwrap();
            let mut sink = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(3), tls.read_to_end(&mut sink)).await;
        }
        // узел жив: UDP работает как раньше
        let (_ca, a, _) = node(Mode::Auto, "aa55").await;
        a.send_to(b"still alive", b_addr).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), b.recv_from(&mut buf)).await.unwrap().unwrap();
        assert_eq!(&buf[..n], b"still alive");
    }
}
