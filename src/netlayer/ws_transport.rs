// src/netlayer/ws_transport.rs
//! WebSocket-over-TLS транспорт между Mobile и Anchor.
//!
//! **Зачем:** мобильный оператор режет UDP / странные TCP-порты, но почти никогда не блокирует
//! TLS на 443. Mobile подключается к anchor'у как обычный HTTPS-клиент к WebSocket endpoint'у —
//! из сети неотличимо от любого WebSocket-приложения.
//!
//! **Что внутри:** WS upgrade → binary-frames. Каждый binary-frame несёт один зашифрованный
//! wagon (тот же wire-format, что и в UDP — `[sender_id:32][nonce:12][ciphertext][tag:16]`).
//! Это позволяет анчору обрабатывать WS-peer'ов через ту же диспатч-логику, что и UDP-peer'ов.
//!
//! **Архитектура:**
//! - `WsServer` (на Anchor) — принимает соединения, выдаёт `WsConnection` per-client.
//! - `WsClient` (на Mobile) — установить и держать `WsConnection` к anchor'у.
//! - `WsConnection` — owns пару mpsc-каналов (incoming, outgoing). Wagon приходит/уходит через них.
//! - Bridge в основной транспорт — отдельный модуль (Iter 2.6).

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::{
    accept_async_with_config, client_async,
    tungstenite::{protocol::{Message, WebSocketConfig}, Result as WsResult},
    WebSocketStream,
};

use crate::netlayer::tls_cert::TlsIdentity;

/// How long an established connection may stay completely silent (heartbeats and pings keep it alive).
const IDLE_LIMIT: Duration = Duration::from_secs(300);

/// Размер канала per-connection в обе стороны.
const CHANNEL_CAPACITY: usize = 256;

/// Одно WS-соединение, абстрактно (server- или client-сторона).
/// Передаёт raw bytes (encrypted wagons) в обе стороны через mpsc-каналы.
pub struct WsConnection {
    /// Канал для отправки (мы кладём — соединение шлёт по сети).
    pub outgoing: mpsc::Sender<Vec<u8>>,
    /// Канал для приёма (соединение кладёт — мы читаем).
    pub incoming: mpsc::Receiver<Vec<u8>>,
    /// Адрес peer'а (для логов).
    pub peer_addr: String,
    /// Хэндл pump-задачи. Drop = закрытие соединения.
    _pump_handle: tokio::task::JoinHandle<()>,
    /// Место в общем пределе установленных соединений (освобождается при закрытии).
    _slot: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl WsConnection {
    /// Внутренний конструктор: оборачивает уже готовый WebSocketStream и спавнит pump-задачу.
    fn from_stream<S>(
        ws: WebSocketStream<S>,
        peer_addr: String,
    ) -> Self
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAPACITY);
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAPACITY);

        let peer_addr_for_pump = peer_addr.clone();
        let pump = tokio::spawn(async move {
            let (mut sink, mut stream) = ws.split();
            // a connection that sends nothing at all (not even a ping) for this long is closed: silent peers must not keep a slot forever
            let mut last_heard = tokio::time::Instant::now();

            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(last_heard + IDLE_LIMIT) => {
                        eprintln!("[ws] {} silent for {:?}, closing", peer_addr_for_pump, IDLE_LIMIT);
                        break;
                    }
                    // App кладёт байты на отправку.
                    msg = out_rx.recv() => {
                        match msg {
                            Some(bytes) => {
                                if let Err(e) = sink.send(Message::Binary(bytes.into())).await {
                                    eprintln!("[ws] send to {} failed: {}", peer_addr_for_pump, e);
                                    break;
                                }
                            }
                            None => break, // отправляющая сторона закрыла канал
                        }
                    }
                    // По сети пришли байты.
                    next = stream.next() => {
                        if next.is_some() { last_heard = tokio::time::Instant::now(); }
                        match next {
                            Some(Ok(Message::Binary(data))) => {
                                if in_tx.send(data.to_vec()).await.is_err() {
                                    break; // принимающая сторона ушла
                                }
                            }
                            Some(Ok(Message::Close(_))) => {
                                break;
                            }
                            Some(Ok(Message::Ping(p))) => {
                                let _ = sink.send(Message::Pong(p)).await;
                            }
                            Some(Ok(_)) => { /* ignore Text/Pong/Frame */ }
                            Some(Err(e)) => {
                                eprintln!("[ws] recv from {} error: {}", peer_addr_for_pump, e);
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
            let _ = sink.send(Message::Close(None)).await;
        });

        Self {
            outgoing: out_tx,
            incoming: in_rx,
            peer_addr,
            _pump_handle: pump,
            _slot: None,
        }
    }
}

// ---------- Server ----------

/// Пределы сервиса, открытого в интернет ДО проверки подлинности. Без них один адрес мог держать тысячи «молчащих» соединений (TLS без ответа),
/// исчерпывая дескрипторы и задачи, а каждое соединение могло потребовать до 64 МиБ памяти на одно сообщение.
#[derive(Clone, Debug)]
pub struct WsLimits {
    /// Сколько времени на TLS-рукопожатие и на переход в WebSocket (каждое).
    pub handshake_timeout: Duration,
    /// Сколько соединений одновременно могут находиться на стадии рукопожатия (всего).
    pub max_handshaking: usize,
    /// ... и с одного адреса.
    pub max_handshaking_per_ip: usize,
    /// Сколько установленных соединений держим одновременно.
    pub max_established: usize,
    /// Наибольшее сообщение и кадр WebSocket.
    pub max_message_bytes: usize,
}

impl Default for WsLimits {
    fn default() -> Self {
        Self { handshake_timeout: Duration::from_secs(10), max_handshaking: 256, max_handshaking_per_ip: 8, max_established: 2048, max_message_bytes: 1 << 20 }
    }
}

/// Учёт рукопожатий в процессе по адресам; место освобождается при выходе из области видимости.
struct HandshakeSlot {
    ip: IpAddr,
    map: Arc<StdMutex<HashMap<IpAddr, usize>>>,
    _total: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        if let Ok(mut m) = self.map.lock() {
            if let Some(n) = m.get_mut(&self.ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    m.remove(&self.ip);
                }
            }
        }
    }
}

/// WS-сервер: bind на TLS endpoint, принимает входящие соединения, отдаёт каждое
/// в виде `WsConnection` через accept-канал.
pub struct WsServer {
    /// Канал, из которого читать новые принятые соединения.
    pub accept_rx: mpsc::Receiver<WsConnection>,
    /// Локальный адрес, на котором сервер реально забиндился.
    pub local_addr: SocketAddr,
    _accept_handle: tokio::task::JoinHandle<()>,
}

impl WsServer {
    /// Поднять WS-over-TLS сервер на указанном адресе с готовой TLS identity (пределы по умолчанию).
    pub async fn bind(bind_addr: SocketAddr, tls: &TlsIdentity) -> Result<Self> {
        Self::bind_with_limits(bind_addr, tls, WsLimits::default()).await
    }

    pub async fn bind_with_limits(bind_addr: SocketAddr, tls: &TlsIdentity, limits: WsLimits) -> Result<Self> {
        let server_cfg = crate::netlayer::tls_cert::build_server_config(tls)?;
        let acceptor = TlsAcceptor::from(server_cfg);

        let listener = TcpListener::bind(bind_addr)
            .await
            .with_context(|| format!("WsServer TcpListener::bind {}", bind_addr))?;
        let local_addr = listener.local_addr()?;

        let (accept_tx, accept_rx) = mpsc::channel::<WsConnection>(32);
        let handshaking = Arc::new(tokio::sync::Semaphore::new(limits.max_handshaking));
        let established = Arc::new(tokio::sync::Semaphore::new(limits.max_established));
        let per_ip: Arc<StdMutex<HashMap<IpAddr, usize>>> = Arc::new(StdMutex::new(HashMap::new()));

        let handle = tokio::spawn(async move {
            loop {
                let (tcp, peer) = match listener.accept().await {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[ws] accept TCP failed: {}", e);
                        // при исчерпании дескрипторов не крутимся впустую
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                // Место для рукопожатия: общий предел и предел на адрес. Нет места — соединение закрывается сразу, ничего не читая и не выделяя.
                let total = match handshaking.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let ip = peer.ip();
                {
                    let mut m = per_ip.lock().unwrap_or_else(|e| e.into_inner());
                    let n = m.entry(ip).or_insert(0);
                    if *n >= limits.max_handshaking_per_ip {
                        continue;
                    }
                    *n += 1;
                }
                let slot = HandshakeSlot { ip, map: per_ip.clone(), _total: total };
                let acceptor_clone = acceptor.clone();
                let accept_tx_clone = accept_tx.clone();
                let established = established.clone();
                let (hs_timeout, max_msg) = (limits.handshake_timeout, limits.max_message_bytes);
                tokio::spawn(async move {
                    let tls_stream = match tokio::time::timeout(hs_timeout, acceptor_clone.accept(tcp)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {
                            eprintln!("[ws] TLS handshake failed from {}: {}", peer, e);
                            return;
                        }
                        Err(_) => return, // молчит: закрываем без шума в журнале (иначе журнал — цель атаки)
                    };
                    let mut cfg = WebSocketConfig::default();
                    cfg.max_message_size = Some(max_msg);
                    cfg.max_frame_size = Some(max_msg);
                    let ws_stream: WebSocketStream<_> = match tokio::time::timeout(hs_timeout, accept_async_with_config(tls_stream, Some(cfg))).await {
                        Ok(Ok(ws)) => ws,
                        Ok(Err(e)) => {
                            eprintln!("[ws] WS upgrade failed from {}: {}", peer, e);
                            return;
                        }
                        Err(_) => return,
                    };
                    drop(slot); // рукопожатие закончено
                    let permit = match established.try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => return, // слишком много установленных соединений
                    };
                    let mut conn = WsConnection::from_stream(ws_stream, peer.to_string());
                    conn._slot = Some(permit);
                    if accept_tx_clone.send(conn).await.is_err() {
                        // Сервер завершён, тихо выходим.
                    }
                });
            }
        });

        Ok(Self {
            accept_rx,
            local_addr,
            _accept_handle: handle,
        })
    }
}

// ---------- Client ----------

/// Подключиться к anchor'у по `wss://host:port/`.
/// `expected_fingerprint_hex` — SHA-256 fingerprint TLS-сертификата anchor'а (pinning).
/// Должен быть получен при pairing'е (Iter 4) и сохранён локально.
pub async fn connect_to_anchor(
    anchor_url: &str,
    expected_fingerprint_hex: &str,
) -> Result<WsConnection> {
    let url = anchor_url
        .parse::<url::Url>()
        .with_context(|| format!("parse {}", anchor_url))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("anchor URL без host"))?
        .to_string();
    let port = url
        .port()
        .ok_or_else(|| anyhow::anyhow!("anchor URL без port"))?;

    let client_cfg =
        crate::netlayer::tls_cert::build_client_config_pinned(expected_fingerprint_hex)?;
    let connector = TlsConnector::from(client_cfg);

    // every phase has a deadline: an anchor that does not answer must not hang the caller (the reconnect loop) forever
    const PHASE: Duration = Duration::from_secs(15);
    let tcp = tokio::time::timeout(PHASE, TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| anyhow::anyhow!("TCP connect {}:{} timed out", host, port))?
        .with_context(|| format!("TCP connect {}:{}", host, port))?;
    let server_name = rustls::pki_types::ServerName::try_from(host.clone())
        .with_context(|| format!("ServerName parse {}", host))?;
    let tls_stream = tokio::time::timeout(PHASE, connector.connect(server_name, tcp))
        .await
        .map_err(|_| anyhow::anyhow!("TLS connect timed out"))?
        .context("TLS connect")?;

    let (ws_stream, _resp) = tokio::time::timeout(PHASE, client_async(anchor_url, tls_stream))
        .await
        .map_err(|_| anyhow::anyhow!("WS handshake timed out"))?
        .context("WS handshake")?;

    let peer_addr = format!("{}:{}", host, port);
    Ok(WsConnection::from_stream(ws_stream, peer_addr))
}

// Reference to suppress unused warning when only one side of the module is used.
#[allow(dead_code)]
fn _arc_marker(_: Arc<()>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Спавним сервер и клиента на localhost, гоняем туда-сюда два бинарных фрейма.
    /// Проверяет TLS-handshake с pinning, WS-upgrade и round-trip данных.
    #[tokio::test]
    async fn ws_server_client_roundtrip() {
        let dir = tempdir().unwrap();
        let id = TlsIdentity::load_or_generate_in(dir.path(), "test01").unwrap();
        let fp = id.fingerprint_hex.clone();

        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut server = WsServer::bind(bind, &id).await.unwrap();
        let port = server.local_addr.port();
        let url = format!("wss://localhost:{}/", port);

        // Принимаем в фоне.
        let server_task = tokio::spawn(async move {
            let mut conn = server.accept_rx.recv().await.expect("server accepts conn");
            // эхо: принимаем один пакет и шлём обратно
            let pkt = conn.incoming.recv().await.expect("server gets data");
            conn.outgoing.send(pkt).await.expect("server echoes");
            // держим живым ещё немного, чтобы клиент успел прочесть
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });

        // Клиент.
        let mut client = connect_to_anchor(&url, &fp).await.expect("client connects");
        let payload = b"hello-yandi-ws".to_vec();
        client.outgoing.send(payload.clone()).await.unwrap();
        let echoed = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            client.incoming.recv(),
        )
        .await
        .expect("recv timeout")
        .expect("recv None");

        assert_eq!(echoed, payload);
        let _ = server_task.await;
    }

    /// Pin mismatch — клиент не должен подключиться.
    #[tokio::test]
    async fn ws_pin_mismatch_rejects() {
        let dir = tempdir().unwrap();
        let id = TlsIdentity::load_or_generate_in(dir.path(), "test02").unwrap();
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = WsServer::bind(bind, &id).await.unwrap();
        let port = server.local_addr.port();
        let url = format!("wss://localhost:{}/", port);

        // Подсовываем неправильный fingerprint (32 нуля).
        let bad_fp = "0".repeat(64);
        let res = connect_to_anchor(&url, &bad_fp).await;
        assert!(res.is_err(), "client must reject anchor with mismatched pin");
    }

    // ───────── пределы сервиса, открытого в интернет до проверки подлинности ─────────

    async fn tls_server(limits: WsLimits) -> (WsServer, String, String, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let id = TlsIdentity::load_or_generate_in(dir.path(), "limits").unwrap();
        let fp = id.fingerprint_hex.clone();
        let server = WsServer::bind_with_limits("127.0.0.1:0".parse().unwrap(), &id, limits).await.unwrap();
        let url = format!("wss://localhost:{}/", server.local_addr.port());
        (server, url, fp, dir)
    }

    /// Соединения, которые молчат (TCP открыт, TLS не начат), занимают места только на время тайм-аута; сверх предела на адрес закрываются сразу;
    /// после этого честный клиент подключается.
    #[tokio::test]
    async fn silent_connections_are_capped_per_address_and_time_out() {
        let limits = WsLimits { handshake_timeout: Duration::from_millis(700), max_handshaking_per_ip: 4, ..WsLimits::default() };
        let (mut server, url, fp, _d) = tls_server(limits).await;
        let addr = server.local_addr;
        // 40 молчащих соединений с одного адреса
        let mut held = vec![];
        for _ in 0..40 {
            held.push(tokio::net::TcpStream::connect(addr).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        // Состояние всех соединений снимается в один момент (try_read не ждёт): закрыто сервером = конец потока, открыто = «ещё нет данных».
        let closed_now = |held: &Vec<tokio::net::TcpStream>| {
            held.iter()
                .filter(|s| {
                    let mut b = [0u8; 1];
                    matches!(s.try_read(&mut b), Ok(0))
                })
                .count()
        };
        let closed_at_once = closed_now(&held);
        assert_eq!(closed_at_once, 36, "сверх предела на адрес (4) соединения закрываются сразу: закрыто {closed_at_once} из 40");
        // остальные закрываются по тайм-ауту
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let closed_later = closed_now(&held);
        assert_eq!(closed_later, 40, "молчащие соединения не живут дольше тайм-аута (закрыто {closed_later} из 40)");
        drop(held);
        // честный клиент подключается и обменивается данными
        let server_task = tokio::spawn(async move {
            let mut conn = server.accept_rx.recv().await.expect("accept");
            let pkt = conn.incoming.recv().await.expect("data");
            conn.outgoing.send(pkt).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let mut client = connect_to_anchor(&url, &fp).await.expect("honest client connects after the attack");
        client.outgoing.send(b"still alive".to_vec()).await.unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(3), client.incoming.recv()).await.unwrap().unwrap();
        assert_eq!(echoed, b"still alive");
        let _ = server_task.await;
    }

    /// Сообщение крупнее предела разрывает соединение, а не выделяет под него память.
    #[tokio::test]
    async fn an_oversized_message_closes_the_connection() {
        let limits = WsLimits { max_message_bytes: 64 * 1024, ..WsLimits::default() };
        let (mut server, url, fp, _d) = tls_server(limits).await;
        let received = tokio::spawn(async move {
            let mut conn = server.accept_rx.recv().await.expect("accept");
            // первый небольшой пакет доходит, потом огромный — соединение закрывается без доставки огромного
            let first = conn.incoming.recv().await;
            let second = tokio::time::timeout(Duration::from_secs(3), conn.incoming.recv()).await;
            (first, second)
        });
        let client = connect_to_anchor(&url, &fp).await.unwrap();
        client.outgoing.send(b"small".to_vec()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        client.outgoing.send(vec![7u8; 4 * 1024 * 1024]).await.unwrap();
        let (first, second) = received.await.unwrap();
        assert_eq!(first.as_deref(), Some(&b"small"[..]));
        match second {
            Ok(None) => {}                // соединение закрыто
            Err(_) => panic!("сервер не закрыл соединение после огромного сообщения"),
            Ok(Some(big)) => panic!("огромное сообщение доставлено ({} байт)", big.len()),
        }
    }

    /// Предел установленных соединений: сверх него новые закрываются, прежние продолжают работать.
    #[tokio::test]
    async fn established_connections_have_a_ceiling() {
        let limits = WsLimits { max_established: 2, max_handshaking_per_ip: 16, ..WsLimits::default() };
        let (mut server, url, fp, _d) = tls_server(limits).await;
        let mut accepted = 0;
        let mut clients = vec![];
        for _ in 0..4 {
            if let Ok(c) = connect_to_anchor(&url, &fp).await {
                clients.push(c);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut conns = vec![];
        while let Ok(Some(c)) = tokio::time::timeout(Duration::from_millis(200), server.accept_rx.recv()).await {
            accepted += 1;
            conns.push(c);
        }
        assert_eq!(accepted, 2, "принято ровно столько, сколько разрешено");
        // лишние клиенты закрыты сервером, а не стоят в очереди: у них поток входящих сразу завершается
        let mut closed = 0;
        let mut alive = 0;
        for c in clients.iter_mut() {
            match tokio::time::timeout(Duration::from_millis(300), c.incoming.recv()).await {
                Ok(None) => closed += 1,
                _ => alive += 1,
            }
        }
        assert_eq!((closed, alive), (clients.len() - 2, 2), "сверх предела соединения закрываются, а не ждут");
    }
}
